//! Android の入口（`#[fandhe::main]` が生成する `android_main` から呼ぶ。Issue #1200）。
//!
//! Activity 方式は GameActivity（Issue #242、決定記録 `docs/architecture/android-activity-method.md`）、
//! バックエンドは Vulkan 固定（REQ-1、`fandhe_render_egui::launch::run_android`）。利用者の `MainActivity`
//! （`GameActivity` を継承し `System.loadLibrary` で cdylib を読む）から `android_main` が呼ばれ、ここへ来る。
//!
//! ここで行うこと: `AndroidApp` の保持（`fandhe-platform-android`）、選んだ端末機能の保留中の要求の破棄
//! （Activity 再生成で画面状態は作り直されるため、`super::lifecycle`）、終了処理の登録、起床口の登録、
//! window insets の配線、ソフトキーボードの入力の橋渡し（winit が GameTextInput の更新を egui へ渡さないため）。

/// `android_main` の引数型。利用者が `winit` に直接依存せずに済むよう `fandhe::__private` から出す。
pub use winit::platform::android::activity::AndroidApp;

use fandhe_platform_android::{android_app, set_android_app};

/// Android アプリの本体。`build` が返した画面を最初に出す。
pub(crate) fn run(app: AndroidApp, title: &str, build: super::BuildFn) {
    // 片付けより前に登録する。機能の破棄（通知の閉じ込み等）は `android_app()` を使うため、新しいプロセスで
    // 未登録のまま呼ぶと何も消えない。`configChanges` で止められない再生成で `android_main` が新しい
    // `AndroidApp` で再度呼ばれた場合も毎回上書きし、破棄済みの Activity を指したまま残さない。
    set_android_app(app.clone());
    super::lifecycle::reset_android();
    super::lifecycle::register_teardown_hooks();

    if let Err(err) = fandhe_render_egui::launch::run_android(app, title, move |lc| {
        let ctx = lc.egui_ctx();
        // 機能 crate が描画スレッドの外から次フレームを起こす口（`fandhe_runtime::request_repaint`）。
        fandhe_runtime::set_repaint_waker(std::sync::Arc::new(
            fandhe_render_egui::app::EguiWaker::new(ctx.clone()),
        ));
        let mut app = build(ctx);
        app.set_inset_margin_supplier(Box::new(android_inset_margin));
        Box::new(SoftKeyboardApp { inner: app })
    }) {
        eprintln!("[fandhe] Android の起動に失敗: {err:?}");
    }
}

/// [`run`] が組み立てる画面を包み、`update` の先頭で [`poll_soft_keyboard`] を呼んでから
/// フレーム本体（`inner`）に委譲する（Issue #796）。終了処理（`on_exit`）と背景色も `inner` へ渡す
/// （`ProxyApp`・`Menu` は `on_exit` で端末機能を止める）。
struct SoftKeyboardApp {
    inner: Box<dyn fandhe_render_egui::EguiApp>,
}

impl fandhe_render_egui::EguiApp for SoftKeyboardApp {
    fn update(&mut self, ctx: &egui::Context) {
        poll_soft_keyboard(ctx);
        self.inner.update(ctx);
    }

    fn on_exit(&mut self) {
        self.inner.on_exit();
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }
}

/// ソフトキーボードの状態バッファ（`AndroidApp::text_input_state()`）を毎フレーム
/// ポーリングし、前回との差分を egui の入力イベントへ変換して積む（Issue #796）。
///
/// winit の Android バックエンド（0.30.13）は `InputEvent::TextEvent`（GameTextInput の
/// コンポジション更新）を処理しないため、Gboard 等のソフトキーボードの入力が
/// `InputConnection` 経由で書き換える状態バッファは winit を経由しても egui へ届かない
/// （#660 実測）。
/// `AndroidApp::text_input_state()` は `take = false` の読み出しで winit が
/// `input_events_iter` を占有していても現在値をいつでも読めるため、本関数は winit の
/// イベントループとは独立に毎フレーム状態を読み、差分だけを egui イベントへ変換する
/// （変換ロジック本体は [`super::soft_keyboard`] の純関数へ委譲）。
///
/// [`egui::Context::wants_keyboard_input`] が `false`（テキスト編集 UI にフォーカスなし）
/// の間はポーリングしない。フォーカス中の `egui::Id`（`ctx.memory(|m| m.focused())`）が
/// 前回と異なる場合（`None → Some` の新規フォーカスに加え、`Some(a) → Some(b)` の
/// 別欄への移動も含む）は、前のフォーカス先の入力文脈が Gboard 側に残らないよう
/// 状態バッファを明示的にリセットする。`wants_keyboard_input` の true/false 遷移だけを
/// 見ていた旧実装では、preedit 表示中に Tab 等で同じ「wants = true」のまま別の
/// テキスト欄へ移動するケースを検出できなかった（レビュー指摘）。
///
/// preedit 表示中に別欄へ移った場合、旧 preedit 自体は egui 側がフォーカス喪失を検知して
/// `ime_enabled = false` にする（egui 標準動作）ため、本関数側は次回のポーリングに向けて
/// `prev` を初期化するだけでよい（旧 preedit を閉じる `Ime` イベントを明示的に送る必要はない）。
///
/// 既知の制約: このリセットにより GameTextInput バッファは空になるため、既存テキストの
/// あるフィールドへフォーカスした直後に 1 回だけ Backspace を押すと、バッファ側には
/// 消せる文字が無く `committed_diff` が差分なしと判定してイベントを発火できない
/// （実機で要確認。Gboard は空バッファに対して `KEYCODE_DEL` の `KeyEvent` を送ることが
/// 多く、その場合は本関数ではなく winit の既存 `KeyEvent` 経路で届く想定だが、
/// 未検証のため記録は実機確認後に行う）。
fn poll_soft_keyboard(ctx: &egui::Context) {
    let Some(app) = android_app() else {
        return;
    };

    let wants = ctx.wants_keyboard_input();
    if !wants {
        // ロックは状態フィールドの書き換えのみに限定し、`app.*` 呼び出しはロック外で行う
        // （codex-review 指摘: ロック区間から FFI 越しの呼び出しを外す）。
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        state.focused = None;
        return;
    }

    // フォーカス先 Id の変化判定だけ先に済ませ、ロックは一旦解放してから
    // `app.set_text_input_state` / `app.text_input_state`（いずれも JNI 越しの呼び出し）を行う。
    let focused = ctx.memory(|m| m.focused());
    let just_focused = {
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        let changed = state.focused != focused;
        state.focused = focused;
        changed
    };
    if just_focused {
        app.set_text_input_state(
            winit::platform::android::activity::input::TextInputState::default(),
        );
    }

    let raw = app.text_input_state();
    let compose = raw.compose_region.map(|span| (span.start, span.end));
    let next = super::soft_keyboard::snapshot(&raw.text, compose);

    let events = {
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        if just_focused {
            state.prev = super::soft_keyboard::KeyboardSnapshot::default();
        }
        let applied = super::soft_keyboard::keyboard_events(&state.prev, &next);
        // `applied.prev` は defer 中かどうかに関わらず「次フレームの prev として
        // そのまま使うべき値」（`keyboard_events` の doc 参照）。呼び出し側で
        // defer を判定して更新有無を切り替える必要はない。
        state.prev = applied.prev;
        applied.events
    };
    if !events.is_empty() {
        ctx.input_mut(|i| i.events.extend(events));
    }

    // ponytail: 固定 20Hz ポーリング（TextEvent 到達をフックして起こす経路は未実装）。
    // 入力レイテンシが問題になれば android-activity の `TextEvent` キューを直接監視する
    // 経路に上げる。フォーカスが外れれば `wants` が false になり早期 return するため
    // 常時ポーリングにはならない。
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

/// [`poll_soft_keyboard`] が前回フレームとの差分検出に使う状態（未確定領域含む直前の
/// [`super::soft_keyboard::KeyboardSnapshot`] と、直前フレームでフォーカスしていた
/// `egui::Id`。`None` はテキスト編集 UI へのフォーカスなしを表す）。
struct SoftKeyboardState {
    prev: super::soft_keyboard::KeyboardSnapshot,
    focused: Option<egui::Id>,
}

static SOFT_KEYBOARD: std::sync::Mutex<SoftKeyboardState> =
    std::sync::Mutex::new(SoftKeyboardState {
        prev: super::soft_keyboard::KeyboardSnapshot {
            committed: String::new(),
            compose: None,
        },
        focused: None,
    });

/// `fandhe_platform_android::window_insets_px`（`HostInsets` が JNI 経由で書く物理 px）を読み、
/// `fandhe_render_egui::app` の純関数 `insets_px_to_margin` へ橋渡しして、`CentralPanel` の
/// `inner_margin` へ加算する `egui::Margin`（論理 pt）を返す（Issue #579）。未受信は 0。
///
/// `ProxyAppDyn::set_inset_margin_supplier`
/// へ渡すクロージャから毎フレーム呼ばれる。値の更新は OS のリスナー発火時のみ（イベント駆動）で、
/// 回転・折りたたみ等の変化にも次フレームで追従する。
fn android_inset_margin(ctx: &egui::Context) -> egui::Margin {
    let [top, right, bottom, left] = fandhe_platform_android::window_insets_px().unwrap_or([0; 4]);
    let insets_px = (left as f32, top as f32, right as f32, bottom as f32);
    fandhe_render_egui::app::insets_px_to_margin(insets_px, ctx.pixels_per_point())
}
