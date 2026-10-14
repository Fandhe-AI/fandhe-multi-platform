//! iOS のソフトキーボード（日本語変換を含む）の確定・未確定文字列を egui の入力
//! イベントへ橋渡しする（Issue #795）。
//!
//! winit 0.30.13 の iOS ビュー（クラス名 `WinitUIView`）は `UIKeyInput` のみ実装し
//! `UITextInput`（marked text）を持たないため、日本語キーボードの変換中文字列が
//! winit を経由して egui へ届かない（実機実測）。`ios/fandhe_ios_text_input.m`
//! （`build.rs` が iOS ターゲットで常時コンパイルする ObjC）が、ゼロサイズの
//! `UITextField` を代わりに first responder にすることで `UITextInput` 実装一式を
//! 借用し、変更のたびに本モジュールの `#[no_mangle]` コールバックを呼ぶ。
//!
//! 変換ロジック本体（差分 → egui イベント列）は Android（#796）と共有する
//! [`crate::soft_keyboard`] の純関数へ委譲する。`.m` 側のコールバックは 3 種類
//! （文字列変更 `_changed` / Backspace / Enter）が UIKit のメインスレッドから任意の
//! 順序・タイミングで届く（イベント駆動でありポーリングではない）。egui へ届ける際は
//! この到着順を保つ必要がある（例: 「あ」入力 → 確定 → Enter の順に届いたのに
//! Enter が確定より先に注入されると壊れる）ため、[`PendingInput`] の順序付きキュー
//! （[`QUEUE`]、`VecDeque`）へいったん積み、[`TextInputApp::update`] が毎フレーム
//! 先頭から順に処理する（[`drain_pending`]）。
//!
//! `keyboard_events` の `defer` 契約（未確定領域を閉じるだけの 1 フレーム目、実際の
//! Backspace は「次回の評価」で発火する設計）はフレーム単位で 1 回だけ評価される
//! ことを前提にしている。そのため [`drain_pending`] は `Snapshot` を変換した結果が
//! まだ defer 中（`keyboard_events` の戻り値の `prev` が渡した snapshot と不一致）の
//! 場合、そのスナップショットをキュー先頭へ戻して処理を打ち切る
//! （同一フレーム内で defer の 1 フレーム目・2 フレーム目が両方消化されるのを防ぎ、
//! かつキュー内の後続アイテムがこの `Snapshot` を追い越して先に配送されるのも防ぐ）。
//!
//! egui 側で focus が別ウィジェットへ直接移った場合（A → B、`resignFirstResponder`/
//! `becomeFirstResponder` の往復を経ない egui 内部の focus 遷移）、ObjC 側の
//! `FandheTextField` はテキストを保持したままになる。[`TextInputApp::update`] は
//! `self.inner.update`（今回のフレームの入力処理）の後に
//! `ctx.memory(|m| m.focused())` を見て変化を検知し、[`PREV`]/[`QUEUE`] のリセットと
//! `fandhe_ios_text_input_clear()`（`.m` 側、フィールドの `text` を直接空にする）の
//! 両方を行う（今回受け取った入力を消費し終えてから切り替えるため、`inner.update` の
//! 後に検知する）。

use std::collections::VecDeque;

use crate::soft_keyboard::{keyboard_events, KeyboardSnapshot};

/// [`QUEUE`] の要素。ObjC 側コールバックの到着順をそのまま保持する
/// （モジュール doc 参照）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingInput {
    /// `_changed` コールバックが伝えた生スナップショット（未変換）。
    Snapshot(KeyboardSnapshot),
    /// `_backspace`/`_return` が直接発火する egui イベント（`soft_keyboard` の
    /// 差分ロジックを経由しない）。
    Key(Vec<egui::Event>),
}

/// [`QUEUE`] を先頭から処理する純関数（`egui::Context` に依存せず単体テスト可能）。
///
/// `Snapshot` は [`keyboard_events`] で `prev` との差分に変換し、`Key` はそのまま
/// 積む。`Snapshot` の変換結果がまだ defer 中（戻り値の `prev` が渡した snapshot と
/// 不一致）の場合は、そのスナップショットをキュー先頭へ戻して処理を打ち切る
/// （モジュール doc 参照）。
///
/// 同一フレーム内で `Ime(_)` イベント（preedit 更新・Commit）を 1 つでも出した後は
/// `Key` を消費しない（キュー先頭へ戻して打ち切る）: egui は IME 有効状態で開始した
/// フレームの Backspace 等のキーイベントを `remove_ime_incompatible_events` で捨てる
/// ため、同フレームで `Ime` の直後に `Key` を送っても egui 側に届かない
/// （`soft_keyboard.rs` の `keyboard_events` doc 参照）。`Snapshot` は `Ime` 発行後も
/// 続けて処理してよい（`keyboard_events` 自体の defer 規則で自然に止まる）。
///
/// 戻り値は `(発火する egui イベント列, 次フレームの prev, 未処理のまま残ったキュー)`。
pub(crate) fn drain_pending(
    mut prev: KeyboardSnapshot,
    mut queue: VecDeque<PendingInput>,
) -> (Vec<egui::Event>, KeyboardSnapshot, VecDeque<PendingInput>) {
    let mut events = Vec::new();
    let mut ime_emitted = false;
    while let Some(item) = queue.pop_front() {
        match item {
            PendingInput::Key(evts) if ime_emitted => {
                queue.push_front(PendingInput::Key(evts));
                break;
            }
            PendingInput::Key(evts) => events.extend(evts),
            PendingInput::Snapshot(snap) => {
                let applied = keyboard_events(&prev, &snap);
                ime_emitted |= applied
                    .events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Ime(_)));
                events.extend(applied.events);
                let deferred = applied.prev != snap;
                prev = applied.prev;
                if deferred {
                    queue.push_front(PendingInput::Snapshot(snap));
                    break;
                }
            }
        }
    }
    (events, prev, queue)
}

#[cfg(target_os = "ios")]
mod platform {
    use std::ffi::CStr;
    use std::os::raw::c_char;
    use std::sync::Mutex;

    use super::*;

    /// 直前フレームまでに [`drain_pending`] が確定させた [`KeyboardSnapshot`]。
    static PREV: Mutex<KeyboardSnapshot> = Mutex::new(KeyboardSnapshot {
        committed: String::new(),
        compose: None,
    });

    /// `.m` 側コールバックの到着順キュー（モジュール doc 参照）。
    static QUEUE: Mutex<VecDeque<PendingInput>> = Mutex::new(VecDeque::new());

    /// 直近の `update` で渡された [`egui::Context`]。ObjC 側コールバックが `.m` の
    /// メインスレッドから届いた時点で `request_repaint` を呼び、次の描画サイクルで
    /// [`QUEUE`] を確実に処理させるために使う（`update` を呼ぶ前にコールバックが
    /// 来た場合は `None` のままで、次の `update` 自体が `QUEUE` を消費するため
    /// 実害はない）。
    static CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

    /// [`TextInputApp::update`] が直前フレームに見た `ctx.memory(|m| m.focused())`。
    /// egui 内部の focus 遷移（A → B）を検知して [`PREV`]/[`QUEUE`] のリセットと
    /// [`fandhe_ios_text_input_clear`] 呼び出しを行うために使う（モジュール doc 参照）。
    static FOCUSED_ID: Mutex<Option<egui::Id>> = Mutex::new(None);

    /// 既知の [`egui::Context`] があれば再描画を要求する（`.m` 側のコールバックは
    /// egui のフレームループ外・UIKit のメインスレッドから任意タイミングで届くため）。
    fn request_repaint() {
        let ctx = CTX.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }

    /// `.m` 側コールバック（[`fandhe_ios_text_input_changed`]）が呼ぶ。[`QUEUE`] の
    /// 末尾が既に `Snapshot` ならその場で置き換え（合体）、そうでなければ新規に積む
    /// （最新スナップショットが直前の未確定内容を包含するため、単純な置き換えで足りる。
    /// モジュール doc 参照）。
    fn push_snapshot(next: KeyboardSnapshot) {
        let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(PendingInput::Snapshot(last)) = queue.back_mut() {
            *last = next;
        } else {
            queue.push_back(PendingInput::Snapshot(next));
        }
        drop(queue);
        request_repaint();
    }

    /// [`egui::Event::Key`]（1 回分の press + release）を [`QUEUE`] へ積む。
    /// Backspace/Enter の生コールバック（[`fandhe_ios_text_input_backspace`] /
    /// [`fandhe_ios_text_input_return`]）から使う。
    fn push_key_event(key: egui::Key) {
        let events = vec![
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        QUEUE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(PendingInput::Key(events));
        request_repaint();
    }

    /// [`PREV`]/[`QUEUE`] を空へ戻す。ObjC 側の `becomeFirstResponder` 差し込み経路
    /// （[`fandhe_ios_text_input_reset`]）と、egui 内部の focus 遷移検知
    /// （[`TextInputApp::update`]）の両方から呼ばれる。
    fn reset_state() {
        *PREV.lock().unwrap_or_else(|e| e.into_inner()) = KeyboardSnapshot::default();
        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// `ios/fandhe_ios_text_input.m` の `FandheTextField` が `UIControlEventEditingChanged` /
    /// `markedTextRange` の変化時に呼ぶ（UIKit のメインスレッドから）。`text_utf8` の契約は
    /// `.m` ファイル冒頭コメント参照（NUL 終端 UTF-8、呼び出し中のみ有効な非所有ポインタ）。
    /// `compose_start`/`compose_end` は UTF-16 コード単位オフセット、未確定領域なしなら両方 -1。
    ///
    /// # Safety
    /// この関数が含む `unsafe` 操作は `CStr::from_ptr` 1 箇所のみ。`text_utf8` は NULL の
    /// 場合を先に弾いており、非 NULL なら `.m` 側の契約により NUL 終端された有効な UTF-8
    /// バイト列を指す（`NSString.UTF8String` が返すバッファ、呼び出し完了まで有効）。
    /// この関数は呼び出し中に `to_string_lossy` でコピーを作るのみで、ポインタを保存しない。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_ios_text_input_changed(
        text_utf8: *const c_char,
        compose_start: isize,
        compose_end: isize,
    ) {
        let text = if text_utf8.is_null() {
            String::new()
        } else {
            // SAFETY: 上記 doc 参照。
            unsafe { CStr::from_ptr(text_utf8) }
                .to_string_lossy()
                .into_owned()
        };
        let compose = if compose_start < 0 || compose_end < 0 {
            None
        } else {
            Some((compose_start as usize, compose_end as usize))
        };
        let next = crate::soft_keyboard::snapshot(&text, compose);
        push_snapshot(next);
    }

    /// `FandheTextField.deleteBackward` が「消せる文字が既に無い」場合に呼ぶ
    /// （`.m` ファイル冒頭コメント参照）。`soft_keyboard` の差分検出を経由せず、
    /// 直接 Backspace の press/release イベントを積む。
    ///
    /// # Safety
    /// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はシンボル名を
    /// `.m` 側の `extern` 宣言と一致させるための固定名であり、本クレートは
    /// アプリ本体へのみリンクされるため他シンボルと衝突しない
    /// （[`fandhe_ios_text_input_changed`] と同じ根拠）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_ios_text_input_backspace() {
        push_key_event(egui::Key::Backspace);
    }

    /// `FandheTextField.textFieldShouldReturn:` が呼ぶ（改行キー押下）。
    ///
    /// # Safety
    /// この関数自体は `unsafe` 操作を含まない（上記 `fandhe_ios_text_input_backspace`
    /// と同じ根拠）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_ios_text_input_return() {
        push_key_event(egui::Key::Enter);
    }

    /// `WinitUIView.becomeFirstResponder`（`.m` 側の差し込み実装）がフォーカスを新しい
    /// フィールドへ移すたびに呼ぶ。前のフィールドの内容を [`PREV`]/[`QUEUE`] に残さない。
    ///
    /// # Safety
    /// この関数自体は `unsafe` 操作を含まない（上記 `fandhe_ios_text_input_backspace`
    /// と同じ根拠）。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_ios_text_input_reset() {
        reset_state();
    }

    // SAFETY: `ios/fandhe_ios_text_input.m` が定義する C ABI（`build.rs` が iOS
    // ターゲットで常時コンパイル・リンクする）。`fandhe_ios_text_input_install` は
    // 引数・戻り値を持たず、[`TextInputApp::update`] からメインスレッド（eframe の
    // update はメインスレッドで呼ばれる）上でのみ呼ぶ契約。`.m` 側は冪等
    // （2 回目以降は no-op）で、対象クラス `WinitUIView` が未登録の場合も安全に
    // 何もしない（`.m` ファイル冒頭コメント参照）。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_ios_text_input_install();
    }

    // SAFETY: `ios/fandhe_ios_text_input.m` が定義する C ABI（上記
    // `fandhe_ios_text_input_install` と同じ根拠）。`fandhe_ios_text_input_clear` は
    // 引数・戻り値を持たず、[`TextInputApp::update`] からメインスレッド上でのみ呼ぶ契約。
    // `.m` 側はフィールド未生成なら no-op で、`text` をプログラム的に空にするのみ
    // （`UIControlEventEditingChanged` を発火させないため `fandhe_ios_text_input_changed`
    // の再入は起きない、`.m` ファイル冒頭コメント参照）。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_ios_text_input_clear();
    }

    /// [`crate::ios::run`] が組み立てる実描画 `Box<dyn eframe::App>` を包み、`update` の
    /// 先頭で ObjC 側の `WinitUIView` メソッド差し込み（`.m` 側で冪等、`WinitUIView` 未登録
    /// なら次フレームも無害に再試行される契約）・[`egui::Context`] の保存・[`QUEUE`] の
    /// 消化を行い、`inner.update` に委譲した後で focus 遷移検知を行う
    /// （`android.rs::SoftKeyboardApp` と同じ役割の iOS 版。処理順序はモジュール doc 参照）。
    pub(crate) struct TextInputApp {
        inner: Box<dyn eframe::App>,
    }

    impl TextInputApp {
        pub(crate) fn new(inner: Box<dyn eframe::App>) -> Self {
            Self { inner }
        }
    }

    impl eframe::App for TextInputApp {
        fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            // SAFETY: 上記 `unsafe extern "C"` ブロックの SAFETY コメント参照。
            // `update` は eframe のメインスレッド上でのみ呼ばれる（Xcode/`main.rs`
            // 経由のアプリライフサイクルの契約）。`.m` 側は冪等（`WinitUIView` 差し込み
            // 済みなら 2 回目以降 no-op）なので毎フレーム呼んでよい。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_ios_text_input_install();
            }

            *CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx.clone());

            // このフレームで届いている入力を到着順に変換・注入する（モジュール doc
            // 参照。`defer` の 2 フレーム契約はこの「1 フレーム 1 回」の呼び出し頻度に
            // 依存するため、`inner.update` より前に 1 回だけ呼ぶ）。
            let prev = PREV.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let queue = std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()));
            let (events, new_prev, remaining) = drain_pending(prev, queue);
            *PREV.lock().unwrap_or_else(|e| e.into_inner()) = new_prev;
            let remaining_nonempty = !remaining.is_empty();
            *QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = remaining;
            if !events.is_empty() {
                ctx.input_mut(|i| i.events.extend(events));
            }
            if remaining_nonempty {
                // defer で打ち切った分を次フレームで処理させる（モジュール doc 参照）。
                request_repaint();
            }

            self.inner.update(ctx, frame);

            // egui 内部の focus 遷移（A → B、フィールド間を resign/become の往復なしに
            // 直接移る場合）を検知し、前のフィールドの内容を引き継がない（モジュール doc
            // 参照）。`inner.update`（今回の入力処理）の後に見ることで、今回届いていた
            // 入力を消費し終えてから切り替える。`becomeFirstResponder` 差し込み側
            // （`fandhe_ios_text_input_reset`）は ObjC 側のフォーカス遷移
            // （他 UIView → `FandheTextField`）のみを見るため、egui 側だけで完結する
            // focus 遷移はこちらでしか検知できない。
            let current_focus = ctx.memory(|m| m.focused());
            let mut focused = FOCUSED_ID.lock().unwrap_or_else(|e| e.into_inner());
            if *focused != current_focus {
                *focused = current_focus;
                drop(focused);
                reset_state();
                // SAFETY: 上記 `unsafe extern "C"` ブロックの SAFETY コメント参照。
                #[allow(unsafe_code)]
                unsafe {
                    fandhe_ios_text_input_clear();
                }
            }
        }

        /// Issue #800: winit の iOS ビューは `inner_size` を safe area の大きさで返すが、
        /// wgpu の `CAMetalLayer`（drawable）は全画面分あり、egui-wgpu は描画前に
        /// attachment 全体を `clear_color` でクリアしてから egui のパネル（safe area
        /// 大）を描く。既定の `clear_color`（eframe 既定 = ほぼ黒の
        /// `Color32::from_rgba_premultiplied(12, 12, 12, 180)`）のままだと、
        /// safe area 外（ノッチ・ホームインジケータ周辺）だけ黒帯として見える。
        /// パネル背景色（`visuals.panel_fill`）で塗ることで地の色と揃える
        /// （UIWindow/UIView の背景色は `CAMetalLayer` の描画に隠れるため効果がない、
        /// 実機実測済み）。
        fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) use platform::TextInputApp;

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(committed: &str, compose: Option<&str>) -> KeyboardSnapshot {
        KeyboardSnapshot {
            committed: committed.to_string(),
            compose: compose.map(str::to_string),
        }
    }

    fn key_count(events: &[egui::Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            .count()
    }

    /// レビュー指摘: `Snapshot` → `Key` の到着順が変換後も保たれることを確認する
    /// （changed("a") → return() が Text("a") → Enter の順で発火する）。
    #[test]
    fn drain_pending_preserves_snapshot_then_key_order() {
        let mut queue = VecDeque::new();
        queue.push_back(PendingInput::Snapshot(snap("a", None)));
        queue.push_back(PendingInput::Key(vec![
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]));

        let (events, prev, remaining) = drain_pending(KeyboardSnapshot::default(), queue);

        assert!(remaining.is_empty());
        assert_eq!(prev, snap("a", None));
        assert_eq!(
            events,
            vec![
                egui::Event::Text("a".to_string()),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        );
    }

    /// レビュー指摘: defer（IME 変換中の Backspace、`soft_keyboard` の 2 フレーム契約）が
    /// 起きた `Snapshot` はキュー先頭へ戻され、後続アイテムより先に次フレームで
    /// 再処理されることを確認する。
    #[test]
    fn drain_pending_defers_and_keeps_snapshot_at_front_for_next_frame() {
        let prev = snap("x", Some("か"));
        let deferred_snap = snap("", Some("か"));
        let mut queue = VecDeque::new();
        queue.push_back(PendingInput::Snapshot(deferred_snap.clone()));
        queue.push_back(PendingInput::Key(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]));

        // 1 フレーム目: defer が起き、`Enter` はまだ処理されない（打ち切り）。
        let (events, prev_after_first, remaining) = drain_pending(prev, queue);
        assert_eq!(key_count(&events), 0);
        assert_eq!(remaining.len(), 2);
        assert_eq!(
            remaining.front(),
            Some(&PendingInput::Snapshot(deferred_snap.clone()))
        );

        // 2 フレーム目: 呼び出し元は `prev_after_first`/`remaining` をそのまま渡す。
        // 今度は実際に Backspace が発火する。ただし同フレームで Ime（変換再開の
        // `Enabled`/`Preedit`）も発火するため、レビュー指摘（Ime 後の Key は打ち切り）に
        // より後ろの `Enter` はこのフレームでは処理されず残る。
        let (events2, prev_after_second, remaining2) = drain_pending(prev_after_first, remaining);
        assert_eq!(key_count(&events2), 1); // Backspace のみ（Enter はまだ）
        assert!(events2
            .iter()
            .any(|e| matches!(e, egui::Event::Ime(egui::ImeEvent::Preedit(_)))));
        assert_eq!(remaining2.len(), 1);
        assert_eq!(prev_after_second, deferred_snap);
    }

    /// レビュー指摘: 同一フレーム内で `Ime(_)` を発行した後は `Key` を消費しない
    /// （egui は IME 有効状態で開始したフレームの Backspace 等を捨てるため）。
    /// `Snapshot` は `Ime` 発行後も続けて処理してよい（ここでは 1 個のみ）。
    #[test]
    fn drain_pending_holds_back_key_after_ime_event_in_same_frame() {
        let prev = snap("x", Some("か"));
        let mut queue = VecDeque::new();
        queue.push_back(PendingInput::Snapshot(snap("x", None)));
        queue.push_back(PendingInput::Key(vec![
            egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]));

        // 1 フレーム目: Snapshot が Ime（Preedit("")/Commit("")）だけ出し、
        // 後続の Key(Backspace) はキューに残る。
        let (events, prev_after_first, remaining) = drain_pending(prev, queue);
        assert!(events.iter().all(|e| matches!(e, egui::Event::Ime(_))));
        assert!(!events.is_empty());
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining.front(),
            Some(&PendingInput::Key(vec![
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]))
        );

        // 2 フレーム目: 残った Backspace が発火する。
        let (events2, _prev_after_second, remaining2) = drain_pending(prev_after_first, remaining);
        assert_eq!(key_count(&events2), 1);
        assert!(remaining2.is_empty());
    }
}
