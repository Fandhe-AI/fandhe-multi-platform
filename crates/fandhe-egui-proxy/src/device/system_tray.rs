//! Phase 11（親 #1038）型サンプル「システムトレイ」（Issue #1039）の effect ドライバ。
//! `fandhe_core::screens::system_tray`（端末依存ゼロ）の `pending` 要求を観測し、
//! `fandhe_device::system_tray::SystemTray` で OS へアイコンの表示 / 非表示を依頼して
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`device/notification.rs` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # `show`/`hide` を別スレッドへ逃がす理由（P1 是正、PR #1041 codex 指摘）
//!
//! Windows 実装（`windows_system_tray.rs`）の `show` はアイコン専用スレッドの
//! 起動完了を、`hide` は後始末メッセージの処理完了を、いずれも期限無しで待つ
//! （同モジュール doc「待ち時間切れを持たない理由」参照）。macOS 実装
//! （`apple_system_tray.rs`）は AppKit 呼び出しのため main thread 必須だが、
//! `.m` 側が呼び出しスレッドが main でなければ `dispatch_sync` で main へ
//! 寄せる（`fandhe_apple_system_tray.m` 冒頭コメント参照）ため、呼び出し元は
//! main でなくてよい。これらの待ちを UI スレッド（[`drive`] の呼び出し元、
//! eframe のイベントループ）で行うと、待ちが長引く間 UI 全体が固まる。
//! `notification.rs::drive` と同型に `std::thread::spawn` した作業スレッドへ
//! OS 呼び出し（`tray.show`/`tray.hide`）を逃がし、結果を `sender.send(...)` で
//! 返す。固まるのは作業スレッドだけで、UI スレッドは固まらない。
//!
//! # 二重 dispatch を防ぐ理由（`mark_showing`/`mark_hiding`）
//!
//! 以前はプロセス全体の `static Mutex<u64>`（「最後に dispatch した世代」）で
//! 二重 dispatch を防いでいたが、`SystemTrayState::generation` は画面インスタンス
//! ごとに 0 始まりで進むため、画面を作り直す（新しい `SystemTrayState` を作る）と
//! 別インスタンスの dispatch 済み世代と衝突し、新しい「出す」要求が
//! `ShowRequested` のまま固着しうった（codex-review 指摘、Issue #1039 是正、
//! `device-bridge-contracts.md`「2. 並行性・世代」）。`notification.rs::drive` と
//! 同型に、OS 呼び出し前へ `sender.send(Box::new(mark_showing))`/
//! `mark_hiding` を挟んで状態側を「要求済み→処理中」（`Showing`/`Hiding`）へ
//! 進めることで、二重 dispatch 防止をプロセス全体の static ではなく各
//! `SystemTrayState` 自身の `phase` で表現する（インスタンス間で衝突しない、
//! `fandhe_core::screens::system_tray` モジュール doc 参照）。`mark_showing`/
//! `mark_hiding` の送信は OS 呼び出し（作業スレッド）を起動する前に完了させる
//! ため（本モジュール doc「別スレッドへ逃がす理由」参照）、`finish_show`/
//! `finish_hide` より必ず先にチャネルへ乗る。次回フレームの `drain_completions`
//! は両方とも順番どおり（mark が先）に適用する。

use std::sync::Arc;

use fandhe_core::screens::system_tray::{
    apply_selection, finish_hide, finish_show, mark_hiding, mark_showing, take_action, HideResult,
    ShowResult, SystemTrayState, TrayActionKind, MENU_ITEMS, TRAY_TITLE,
};
use fandhe_device::system_tray::{HideOutcome, ShowOutcome, SystemTray, TrayMenu};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;
use crate::device::request_slot::POLL_INTERVAL;

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言のみを転送する（`DeviceError` の内部構造は漏らさない、
/// security.md A09。`notification.rs::to_message` と同型）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => {
            "この OS ではメニューバー・システムトレイに対応していません".to_string()
        }
        other => other.to_string(),
    }
}

fn to_show_result(outcome: ShowOutcome) -> ShowResult {
    match outcome {
        ShowOutcome::Added => ShowResult::Added,
        ShowOutcome::AlreadyShown => ShowResult::AlreadyShown,
    }
}

fn to_hide_result(outcome: HideOutcome) -> HideResult {
    match outcome {
        HideOutcome::Removed => HideResult::Removed,
        HideOutcome::NotShown => HideResult::NotShown,
    }
}

/// 現在のプラットフォームに応じた `SystemTray` 実装を返す。macOS/Windows 以外
/// （Linux・Web・iOS・Android、対応表 `system-tray.json` で `n-a`）は
/// `UnsupportedSystemTray` 固定（`gamepad.rs::platform_gamepad` と同型の cfg
/// カスケード）。`Arc` で返すのは [`drive`] が `show`/`hide` を作業スレッドへ
/// 逃がすため（`'static` 参照ではなく `std::thread::spawn` へ move できる所有権が
/// 要る、`notification.rs::implementation` と同型）。
fn platform_tray() -> Arc<dyn SystemTray> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SystemTray>> = OnceLock::new();
        Arc::clone(
            INSTANCE.get_or_init(|| Arc::new(super::apple_system_tray::AppleSystemTray) as _),
        )
    }
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SystemTray>> = OnceLock::new();
        Arc::clone(
            INSTANCE.get_or_init(|| Arc::new(super::windows_system_tray::WindowsSystemTray) as _),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SystemTray>> = OnceLock::new();
        Arc::clone(
            INSTANCE
                .get_or_init(|| Arc::new(fandhe_device::system_tray::UnsupportedSystemTray) as _),
        )
    }
}

/// `MENU_ITEMS`（`&[&str; 3]`）を `TrayMenu::new` が要求する `&[&str]` へ変換する。
fn tray_menu() -> Result<TrayMenu, DeviceError> {
    let items: Vec<&str> = MENU_ITEMS.to_vec();
    TrayMenu::new(TRAY_TITLE, &items)
}

/// `system_tray` の要求 1 件を処理する effect ドライバ本体（テストのため `tray`
/// を差し込めるよう `system_tray_effect` 本体から切り出す、`notification.rs::drive`
/// と同型）。
pub(crate) fn drive(
    state: &SystemTrayState,
    sender: &CompletionSender<SystemTrayState>,
    tray: Arc<dyn SystemTray>,
) {
    if let Some(action) = take_action(state) {
        // 起動済みマーキング（二重 dispatch 防止、モジュール doc「二重 dispatch
        // を防ぐ理由」参照。`notification.rs::drive` の `mark_sending` と同型）。
        // OS 呼び出しは作業スレッドへ逃がす（モジュール doc「別スレッドへ逃がす
        // 理由」参照）ため、mark の送信はスレッド生成前にここで済ませる。
        let generation = action.generation;
        match action.kind {
            TrayActionKind::Show => {
                let _ = sender.send(Box::new(mark_showing));
                let tray = Arc::clone(&tray);
                let sender = sender.clone();
                std::thread::spawn(move || {
                    let result = tray_menu()
                        .and_then(|menu| tray.show(&menu))
                        .map(to_show_result)
                        .map_err(to_message);
                    let _ = sender.send(Box::new(move |s: &mut SystemTrayState| {
                        finish_show(s, generation, result);
                    }));
                });
            }
            TrayActionKind::Hide => {
                let _ = sender.send(Box::new(mark_hiding));
                let tray = Arc::clone(&tray);
                let sender = sender.clone();
                std::thread::spawn(move || {
                    let result = tray.hide().map(to_hide_result).map_err(to_message);
                    let _ = sender.send(Box::new(move |s: &mut SystemTrayState| {
                        finish_hide(s, generation, result);
                    }));
                });
            }
        }
        return;
    }

    if state.phase != fandhe_core::screens::system_tray::TrayPhase::Shown {
        // 失敗状態（Hidden）・処理中では再描画を予約しない
        // （`device-bridge-contracts.md`「4. ポーリング・再描画」の
        // 「失敗状態のまま再描画し続けない」）。
        return;
    }

    // 表示中はメニュー選択を空になるまで読み、画面へ反映する
    // （`HIDE_ITEM_INDEX` は `apply_selection` 内部で `request_hide` と同じ経路へ
    // 載る、`fandhe-core` 側のモジュール doc 参照）。
    while let Some(index) = tray.take_selection() {
        let _ = sender.send(Box::new(move |s: &mut SystemTrayState| {
            apply_selection(s, index);
        }));
    }
    // pull 型の次回ポーリングを必ず予約する（`device-bridge-contracts.md`
    // 「4. ポーリング・再描画」の「送信を省略する分岐でも次回のポーリングを
    // 予約する」）。
    sender.request_repaint_after(POLL_INTERVAL);
}

/// `registry.rs` の `effect` 区画から `system_tray` 画面用に配線される effect
/// ドライバ。
pub fn system_tray_effect(state: &SystemTrayState, sender: &CompletionSender<SystemTrayState>) {
    drive(state, sender, platform_tray());
}

/// アプリ終了時（`device.rs::shutdown`）に呼ぶ。表示中のアイコンを消す
/// （`hide()` の結果は無視する純関数、テストから呼べるよう `stop_shared_system_tray`
/// 本体から切り出す。`HIDE_ITEM_INDEX` を使わず直接 `hide` を呼ぶのは、終了時は
/// 画面状態（`SystemTrayState`）が既に破棄されている可能性があり、状態機械を
/// 経由せず OS 側のみ後始末すれば足りるため）。
pub(crate) fn cleanup(tray: &dyn SystemTray) {
    let _ = tray.hide();
}

/// `fandhe-egui-proxy::device::shutdown()` から呼ぶ。プロセス終了時に Windows の
/// 通知領域アイコンが残る（`NIM_DELETE` 忘れ）のを防ぐ（モジュール doc「終了時」
/// 契約、`device.rs::shutdown` 参照）。
pub fn stop_shared_system_tray() {
    cleanup(&*platform_tray());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::system_tray::{request_hide, request_show, TrayPhase};

    fn sender_with_channel() -> (
        CompletionSender<SystemTrayState>,
        std::sync::mpsc::Receiver<crate::app::Completion<SystemTrayState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// `DummySystemTray` を `drive` へ渡せる `Arc<dyn SystemTray>` へ包む
    /// （内部状態は `Arc<Mutex<..>>` で共有されるため、元の `tray` 変数で
    /// `show_calls()` 等を観測できる）。
    fn arc_dummy(tray: &fandhe_device::system_tray::DummySystemTray) -> Arc<dyn SystemTray> {
        Arc::new(tray.clone())
    }

    #[test]
    fn drive_is_noop_without_pending_action_when_hidden() {
        let (sender, rx) = sender_with_channel();
        let state = SystemTrayState::default();
        drive(
            &state,
            &sender,
            Arc::new(fandhe_device::system_tray::DummySystemTray::new()),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_shows_and_reports_added() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);

        let tray = fandhe_device::system_tray::DummySystemTray::new();
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_showing が送られる")(&mut state);
        assert_eq!(state.phase, TrayPhase::Showing);
        let apply = rx.recv().expect("完了が送られる");
        apply(&mut state);
        assert_eq!(state.phase, TrayPhase::Shown);
        assert_eq!(tray.show_calls(), 1);
    }

    #[test]
    fn drive_does_not_dispatch_while_showing_is_in_flight() {
        // `mark_showing` が「要求済み」を「処理中」へ進めるため、完了が
        // 反映された後に再び `drive` を呼んでも、処理中は `take_action` が
        // `None` を返し二重 dispatch しない（`LAST_DISPATCHED_GEN` を使わない
        // 新しい防止策の回帰確認、codex-review 指摘 Issue #1039 是正）。
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);

        let tray = fandhe_device::system_tray::DummySystemTray::new();
        drive(&state, &sender, arc_dummy(&tray));
        // mark_showing のみ反映する（フレーム境界で完了が未反映のまま次フレームの
        // 先頭まで進んだ状況を模す）。
        rx.recv().expect("mark_showing が送られる")(&mut state);
        assert_eq!(state.phase, TrayPhase::Showing);

        // 処理中に再度 drive が呼ばれても take_action は None を返すため
        // 再 dispatch しない。
        drive(&state, &sender, arc_dummy(&tray));

        // `show` は作業スレッドで非同期に実行されるため、呼び出し回数の確認は
        // 完了（`finish_show`）を受け取った後に行う（スレッド未完了のうちに
        // 読むと `show_calls()` が 0 のままで読めてしまう競合を避ける）。
        let apply = rx.recv().expect("finish_show の完了が送られる");
        apply(&mut state);
        assert_eq!(state.phase, TrayPhase::Shown);
        assert_eq!(tray.show_calls(), 1, "処理中は再 dispatch しない");
        assert!(rx.try_recv().is_err(), "完了は 1 件のみ送られる");
    }

    #[test]
    fn drive_dispatches_new_generation_after_previous_completes() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        let tray = fandhe_device::system_tray::DummySystemTray::new();
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_showing が送られる")(&mut state);
        rx.recv().expect("show 完了が送られる")(&mut state);

        request_hide(&mut state);
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_hiding が送られる")(&mut state);
        // `hide` も作業スレッドで非同期に実行されるため、完了を待ってから
        // 呼び出し回数を確認する（前テストの競合対策と同じ理由）。
        rx.recv().expect("hide 完了が送られる")(&mut state);
        assert_eq!(tray.hide_calls(), 1);
    }

    #[test]
    fn drive_hides_and_reports_removed() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        let tray = fandhe_device::system_tray::DummySystemTray::new();
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_showing が送られる")(&mut state);
        rx.recv().expect("show 完了")(&mut state);

        request_hide(&mut state);
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_hiding が送られる")(&mut state);
        assert_eq!(state.phase, TrayPhase::Hiding);
        let apply = rx.recv().expect("hide 完了");
        apply(&mut state);
        assert_eq!(state.phase, TrayPhase::Hidden);
    }

    #[test]
    fn drive_reports_unsupported_as_fixed_message() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);

        drive(
            &state,
            &sender,
            Arc::new(fandhe_device::system_tray::UnsupportedSystemTray),
        );
        rx.recv().expect("mark_showing が送られる")(&mut state);
        let apply = rx.recv().expect("完了が送られる");
        apply(&mut state);
        assert_eq!(state.phase, TrayPhase::Hidden);
        assert!(state.error.contains("対応していません"));
    }

    #[test]
    fn drive_polls_selection_and_applies_to_message_while_shown() {
        let (sender, rx) = sender_with_channel();
        let mut state = SystemTrayState::default();
        request_show(&mut state);
        let tray = fandhe_device::system_tray::DummySystemTray::new();
        drive(&state, &sender, arc_dummy(&tray));
        rx.recv().expect("mark_showing が送られる")(&mut state);
        rx.recv().expect("show 完了")(&mut state);
        assert_eq!(state.phase, TrayPhase::Shown);

        tray.select(0);
        drive(&state, &sender, arc_dummy(&tray));
        let apply = rx.recv().expect("選択の完了が送られる");
        apply(&mut state);
        assert_eq!(state.message, "おはようございます");
    }

    #[test]
    fn drive_does_not_schedule_repaint_while_hidden() {
        let (sender, rx) = sender_with_channel();
        let state = SystemTrayState::default();
        // `request_repaint_after` 呼び出し自体は直接観測できないため、
        // 少なくとも完了が送られないことのみを確認する（Hidden は early return
        // する実装であることの回帰確認）。
        drive(
            &state,
            &sender,
            Arc::new(fandhe_device::system_tray::DummySystemTray::new()),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn cleanup_calls_hide_and_ignores_result() {
        let tray = fandhe_device::system_tray::DummySystemTray::new();
        cleanup(&tray);
        assert_eq!(tray.hide_calls(), 1);
    }
}
