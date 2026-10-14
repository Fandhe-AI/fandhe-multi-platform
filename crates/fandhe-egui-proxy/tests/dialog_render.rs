//! `fandhe_core::screens::confirm_dialog`（Issue #729 / #739）の egui-proxy 実描画
//! 結合テスト。`render_node`（`lib.rs`）の `Node::Widget` 分岐が `DialogWidget` を
//! `egui::Modal` で実描画し、パネル外タップを `dismiss_label` と同名のパネル内
//! `Button` へ変換して dispatch することを egui_kittest で検証する。
//!
//! `tests/desktop_render.rs` と同型のハーネス構築（`egui_kittest::wgpu::WgpuTestRenderer`、
//! GPU アダプタ欠如環境でのスキップ判定・`GPU_SERIAL` による直列化）を踏襲する。

use egui_kittest::kittest::Queryable;
use fandhe_core::screens::confirm_dialog::{confirm_dialog_screen, ConfirmDialogState};
use fandhe_core::{DialogWidget, Node, Screen, StateStore, WidgetId};

/// ハーネスのウィンドウサイズ。`egui::Modal` は既定で中央配置（`Align2::CENTER_CENTER`）
/// のため、この右下隅近くの座標（`OUTSIDE_PANEL_POS`）はパネル矩形から確実に外れる
/// （パネル外タップ検証に使う）。
const WINDOW_SIZE: egui::Vec2 = egui::Vec2::new(800.0, 600.0);

/// パネル外タップの座標。ウィンドウ右下隅から 8px 内側（`WINDOW_SIZE` の右下近く、
/// 見出し・本文が描かれる左上寄りの領域から十分離れている）。
const OUTSIDE_PANEL_POS: egui::Pos2 = egui::Pos2::new(WINDOW_SIZE.x - 8.0, WINDOW_SIZE.y - 8.0);

/// GPU アダプタ取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`desktop_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `confirm_dialog_screen(store.state())` を毎フレーム引き直しつつ `render_screen` で
/// 描画するループを `Harness` 上で再現する（`nav` 契約、`screen_stack_render.rs` と
/// 同型。`confirm_dialog_screen` は状態に応じて画面全体を都度組み直す
/// `fn(&S) -> Screen<S>` のため、事前に 1 度だけ組んだ `Screen` を使い回すと
/// `pending` の変化を反映できない）。
fn build_harness() -> egui_kittest::Harness<'static, StateStore<ConfirmDialogState>> {
    egui_kittest::Harness::builder()
        .with_size(WINDOW_SIZE)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<ConfirmDialogState>| {
                let screen = confirm_dialog_screen(store.state());
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(ConfirmDialogState::default()),
        )
}

/// アダプタ欠如由来の panic メッセージにのみ現れる部分文字列（`desktop_render.rs` と同一）。
const ADAPTER_UNAVAILABLE_PANIC_MARKERS: [&str; 2] =
    ["CustomNativeAdapterSelectionError", "No adapter found"];

fn is_adapter_unavailable_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let message: Option<&str> = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    match message {
        Some(message) => ADAPTER_UNAVAILABLE_PANIC_MARKERS
            .iter()
            .any(|marker| message.contains(marker)),
        None => false,
    }
}

/// `build_harness` を呼び出し、wgpu アダプタ欠如による構築時 panic のみを捕捉して
/// `None` を返す（テストをスキップさせる。`desktop_render.rs` と同方針）。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<ConfirmDialogState>>>
{
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、ダイアログ実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build_harness)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、ダイアログ実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 初期表示（`pending == None`）では確認パネルが閉じているため、「キャンセル」は
/// AX ツリーに現れないことを検証する。
#[test]
fn initial_render_has_no_cancel_button() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    assert!(harness.query_by_label("キャンセル").is_none());
}

/// 一覧の「削除」（最初の 1 件）をクリックすると `pending == Some(0)` となり、
/// パネル内の「キャンセル」「削除」が AX ツリーに現れることを検証する。
#[test]
fn clicking_first_delete_opens_panel_with_cancel_and_confirm() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    // 一覧側の「削除」は 3 行分あり `get_by_label` はラベル一意性を要求するため、
    // `get_all_by_label` の列挙順（先頭 = 最初のメモ行）で 1 件目を選ぶ。
    harness
        .get_all_by_label("削除")
        .next()
        .expect("一覧側の削除ボタンが 1 件以上ある")
        .click();
    harness.run();

    assert_eq!(harness.state().state().pending, Some(0));
    harness.get_by_label("キャンセル");
    // パネル側「削除」が加わり、一覧側 3 件と合わせて計 4 件になる。
    assert_eq!(harness.get_all_by_label("削除").count(), 4);
}

/// パネル外（ウィンドウ右下隅付近）へポインタ press → release すると、`DialogWidget`
/// の backdrop クリックが「キャンセル」ボタンの path へ変換され dismiss される
/// （`pending == None`、`memos` は変わらない、`Node::Widget` 分岐の backdrop 判定）。
#[test]
fn tapping_outside_the_panel_dismisses_it() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness
        .get_all_by_label("削除")
        .next()
        .expect("一覧側の削除ボタンが 1 件以上ある")
        .click();
    harness.run();
    assert_eq!(harness.state().state().pending, Some(0));

    harness.drag_at(OUTSIDE_PANEL_POS);
    harness.drop_at(OUTSIDE_PANEL_POS);
    harness.run();

    assert_eq!(harness.state().state().pending, None);
    assert!(harness.query_by_label("キャンセル").is_none());
    assert_eq!(harness.state().state().memos.len(), 3);
}

/// パネル内「削除」をクリックすると該当メモが消え（`memos` が 2 件）、
/// `pending` もクリアされることを検証する。
#[test]
fn clicking_confirm_inside_panel_removes_memo() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness
        .get_all_by_label("削除")
        .next()
        .expect("一覧側の削除ボタンが 1 件以上ある")
        .click();
    harness.run();
    assert_eq!(harness.state().state().pending, Some(0));

    // パネル内「削除」は一覧側の 3 件に続く 4 件目として現れる（末尾要素）。
    harness
        .get_all_by_label("削除")
        .next_back()
        .expect("パネル側の削除ボタンが最後に見つかる")
        .click();
    harness.run();

    assert_eq!(harness.state().state().pending, None);
    assert_eq!(harness.state().state().memos.len(), 2);
}

/// カウンタのみを持つ最小状態（非モーダル・disabled 検証用の独自画面で使う）。
#[derive(Default)]
struct CountState {
    count: u32,
}

fn increment(state: &mut CountState) {
    state.count += 1;
}

/// `new_static` の `set` 引数用の no-op（両テストとも `DialogWidget` の開閉状態を
/// dispatch で変更しないため、呼ばれても状態を書き換えない）。
fn noop_set(_state: &mut CountState, _open: bool) {}

fn expand_panel_button(_state: &CountState) -> Vec<Node<CountState>> {
    vec![Node::Button {
        label: "パネル内ボタン".to_string(),
        on_click: increment,
        id: WidgetId::from_raw(100),
    }]
}

/// `screen` を `CountState` 用の `Harness` に組んで返す（`build_harness_or_skip` と
/// 同じスキップ判定を、`Screen<CountState>` を外部から渡せる形に一般化したもの。
/// 追加した 2 テスト（非モーダル・disabled）が共に必要とするため切り出した）。
fn build_count_harness_or_skip(
    screen: Screen<CountState>,
) -> Option<egui_kittest::Harness<'static, StateStore<CountState>>> {
    let build = move |ui: &mut egui::Ui, store: &mut StateStore<CountState>| {
        fandhe_egui_proxy::render_screen(ui, &screen, store);
    };

    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(
            egui_kittest::Harness::builder()
                .with_size(WINDOW_SIZE)
                .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
                .build_ui_state(build, StateStore::new(CountState::default())),
        );
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、ダイアログ実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        egui_kittest::Harness::builder()
            .with_size(WINDOW_SIZE)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
            .build_ui_state(build, StateStore::new(CountState::default()))
    })) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }
            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、ダイアログ実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// PR #792 codex-review 指摘 B: `with_modal(false)` の `DialogWidget` は背景入力を
/// 遮断してはならない。背後の「カウント」ボタンをクリックして状態が更新されることを
/// 検証する（`egui::Modal` の backdrop に吸われると `count` は変化しない）。
#[test]
fn non_modal_dialog_does_not_block_background_input() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let screen: Screen<CountState> = Screen::new("test")
        .button(WidgetId::from_raw(1), "カウント", increment)
        .with_node(Node::Widget(Box::new(
            DialogWidget::new_static(WidgetId::from_raw(2), "情報", "", "閉じる", true, noop_set)
                .with_modal(false)
                .with_content(expand_panel_button),
        )));
    let Some(mut harness) = build_count_harness_or_skip(screen) else {
        return;
    };
    harness.run();

    harness.get_by_label("カウント").click();
    harness.run();

    assert_eq!(harness.state().state().count, 1);
}

/// PR #792 codex-review 指摘 A: `set_disabled(true)` の開状態 `DialogWidget` は
/// 表示を維持しつつ入力を遮断する。パネル内ボタンのクリックが `count` を変えず、
/// パネル外クリック（backdrop dismiss 相当の位置）でもパネルの表示状態
/// （`query_by_label` で内部ボタンが引き続き見える）に影響しないことを検証する。
#[test]
fn disabled_dialog_ignores_panel_click_but_keeps_content_visible() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let mut dialog =
        DialogWidget::new_static(WidgetId::from_raw(3), "情報", "", "閉じる", true, noop_set)
            .with_content(expand_panel_button);
    dialog.set_disabled(true);
    let screen: Screen<CountState> = Screen::new("test").with_node(Node::Widget(Box::new(dialog)));
    let Some(mut harness) = build_count_harness_or_skip(screen) else {
        return;
    };
    harness.run();

    // disabled でも表示は維持される（`content_nodes` は disabled を見ない）。
    harness.get_by_label("パネル内ボタン").click();
    harness.run();
    assert_eq!(harness.state().state().count, 0);

    harness.drag_at(OUTSIDE_PANEL_POS);
    harness.drop_at(OUTSIDE_PANEL_POS);
    harness.run();

    assert_eq!(harness.state().state().count, 0);
    assert!(harness.query_by_label("パネル内ボタン").is_some());
}
