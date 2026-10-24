//! Phase 11（親 #150）型サンプル「グローバルホットキー」（Issue #766）の effect ドライバ。
//! `fandhe_core::screens::global_hotkey`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::global_hotkey::GlobalHotkey` で OS へ登録・解除し、結果と押下を
//! `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。
//! 契約と守る範囲は `fandhe_device::global_hotkey` のモジュール doc を参照。
//!
//! 登録・解除は OS が即時に結果を返す軽い呼び出しなので UI スレッドで同期に呼ぶ（macOS は
//! main thread 必須で、eframe の UI スレッドは macOS では main thread）。待ち・期限・
//! `request_repaint_after` は持たない（押下は OS の通知 → `CompletionSender::send` が UI を起こす）。
//! **dispatch はここで最終形にしてあり、後続 issue（Windows は #767）は OS 別モジュール
//! （`windows_global_hotkey.rs`）だけを埋める**。macOS は `macos_global_hotkey.rs`、
//! それ以外は `UnsupportedGlobalHotkey` で「非対応」を表示する。
//!
//! 押下は「世代つきのまとめ送り」にする: 未適用件数を数え、0 → 1 になったときだけ完了を 1 件送る。
//! 押下が大量に届いても完了キューが伸びない。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use fandhe_core::screens::global_hotkey::{
    apply_platform, apply_presses, finish_register, finish_unregister, mark_in_flight, take_action,
    GlobalHotkeyState, HotkeyAction, HotkeyFailure, Platform, RegisterResult, UnregisterResult,
};
#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
use fandhe_device::global_hotkey::UnsupportedGlobalHotkey;
use fandhe_device::global_hotkey::{GlobalHotkey, OnPress, RegisterOutcome, UnregisterOutcome};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 画面へ注入する OS 区分。
fn platform_kind() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Unknown
    }
}

/// 登録結果を画面用へ写像する。内部構造・OS のコードは出さない（security.md A09）。
fn to_register_result(
    result: Result<RegisterOutcome, DeviceError>,
) -> Result<RegisterResult, HotkeyFailure> {
    match result {
        Ok(RegisterOutcome::Registered) => Ok(RegisterResult::Registered),
        Ok(RegisterOutcome::InUse) => Ok(RegisterResult::InUse),
        Err(DeviceError::Unsupported(_)) => Ok(RegisterResult::Unsupported),
        Err(_) => Err(HotkeyFailure),
    }
}

/// 解除結果を画面用へ写像する。
fn to_unregister_result(
    result: Result<UnregisterOutcome, DeviceError>,
) -> Result<UnregisterResult, HotkeyFailure> {
    match result {
        Ok(UnregisterOutcome::Unregistered) => Ok(UnregisterResult::Unregistered),
        Ok(UnregisterOutcome::NotRegistered) => Ok(UnregisterResult::NotRegistered),
        Err(_) => Err(HotkeyFailure),
    }
}

/// 世代つきのまとめ送りの `OnPress` を作る。
fn make_on_press(sender: CompletionSender<GlobalHotkeyState>, generation: u64) -> OnPress {
    let pending = Arc::new(AtomicU64::new(0));
    Arc::new(move || {
        // 0 → 1 のときだけ完了を送る。適用側が swap(0) で取り切るので、その後の押下は再び送られる。
        if pending.fetch_add(1, Ordering::SeqCst) == 0 {
            let pending = Arc::clone(&pending);
            let _ = sender.send(Box::new(move |s: &mut GlobalHotkeyState| {
                let n = pending.swap(0, Ordering::SeqCst);
                apply_presses(s, generation, n);
            }));
        }
    })
}

/// 要求を観測して処理し、結果を画面へ返す。
fn drive(
    state: &GlobalHotkeyState,
    sender: &CompletionSender<GlobalHotkeyState>,
    imp: &dyn GlobalHotkey,
) {
    let kind = platform_kind();
    if kind != Platform::Unknown && state.platform == Platform::Unknown {
        // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
        let _ = sender.send(Box::new(move |s: &mut GlobalHotkeyState| {
            apply_platform(s, kind);
        }));
    }
    let Some(view) = take_action(state) else {
        return;
    };
    let generation = view.generation;
    let _ = sender.send(Box::new(mark_in_flight));
    match view.action {
        HotkeyAction::Register => {
            let result =
                to_register_result(imp.register(make_on_press(sender.clone(), generation)));
            let _ = sender.send(Box::new(move |s: &mut GlobalHotkeyState| {
                finish_register(s, generation, result);
            }));
        }
        HotkeyAction::Unregister => {
            let result = to_unregister_result(imp.unregister());
            let _ = sender.send(Box::new(move |s: &mut GlobalHotkeyState| {
                finish_unregister(s, generation, result);
            }));
        }
    }
}

/// プロセスに 1 つの実装（`shutdown` が登録と同じ実体に届くようにする）。
fn platform_global_hotkey() -> &'static Arc<dyn GlobalHotkey> {
    static IMPL: OnceLock<Arc<dyn GlobalHotkey>> = OnceLock::new();
    IMPL.get_or_init(|| {
        #[cfg(target_os = "macos")]
        return super::macos_global_hotkey::implementation();
        #[cfg(target_os = "windows")]
        return super::windows_global_hotkey::implementation();
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return Arc::new(UnsupportedGlobalHotkey);
    })
}

/// `registry.rs` から `global_hotkey` 画面用に配線される effect ドライバ。
pub fn global_hotkey_effect(
    state: &GlobalHotkeyState,
    sender: &CompletionSender<GlobalHotkeyState>,
) {
    let needs_platform =
        platform_kind() != Platform::Unknown && state.platform == Platform::Unknown;
    if take_action(state).is_none() && !needs_platform {
        return;
    }
    drive(state, sender, platform_global_hotkey().as_ref());
}

/// アプリ終了時に登録を外す（`device::shutdown` から、eframe の `on_exit`（main thread）で呼ばれる）。
/// 終了経路なので結果は見ない。
pub(super) fn shutdown() {
    let _ = platform_global_hotkey().unregister();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::global_hotkey as screen;
    use screen::HotkeyPhase;
    use std::sync::Mutex;

    /// 呼び出しを数え、決めた結果を返し、渡された `OnPress` を保持する偽実装。
    struct Fake {
        register_calls: Mutex<u32>,
        unregister_calls: Mutex<u32>,
        register_result: fn() -> Result<RegisterOutcome, DeviceError>,
        unregister_result: fn() -> Result<UnregisterOutcome, DeviceError>,
        on_press: Mutex<Option<OnPress>>,
    }

    impl GlobalHotkey for Fake {
        fn register(&self, on_press: OnPress) -> Result<RegisterOutcome, DeviceError> {
            *self.register_calls.lock().unwrap() += 1;
            let r = (self.register_result)();
            if matches!(r, Ok(RegisterOutcome::Registered)) {
                *self.on_press.lock().unwrap() = Some(on_press);
            }
            r
        }
        fn unregister(&self) -> Result<UnregisterOutcome, DeviceError> {
            *self.unregister_calls.lock().unwrap() += 1;
            (self.unregister_result)()
        }
    }

    fn fake(
        register_result: fn() -> Result<RegisterOutcome, DeviceError>,
        unregister_result: fn() -> Result<UnregisterOutcome, DeviceError>,
    ) -> Fake {
        Fake {
            register_calls: Mutex::new(0),
            unregister_calls: Mutex::new(0),
            register_result,
            unregister_result,
            on_press: Mutex::new(None),
        }
    }

    fn channel() -> (
        CompletionSender<GlobalHotkeyState>,
        std::sync::mpsc::Receiver<crate::app::Completion<GlobalHotkeyState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 1 回ドライバを回し、届いた完了をすべて適用する。
    fn step(
        s: &mut GlobalHotkeyState,
        sender: &CompletionSender<GlobalHotkeyState>,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<GlobalHotkeyState>>,
        imp: &dyn GlobalHotkey,
    ) {
        drive(s, sender, imp);
        while let Ok(apply) = rx.try_recv() {
            apply(s);
        }
    }

    fn ok_fake() -> Fake {
        fake(
            || Ok(RegisterOutcome::Registered),
            || Ok(UnregisterOutcome::Unregistered),
        )
    }

    #[test]
    fn register_dispatches_once_then_registered() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, HotkeyPhase::Registered);
        // 完了後に回しても二重に呼ばない。
        step(&mut s, &sender, &rx, &f);
        assert_eq!(*f.register_calls.lock().unwrap(), 1);
    }

    #[test]
    fn in_use_is_reported_and_not_registered() {
        let f = fake(
            || Ok(RegisterOutcome::InUse),
            || Ok(UnregisterOutcome::NotRegistered),
        );
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, HotkeyPhase::Unregistered);
        assert_eq!(s.error, screen::IN_USE_ERROR);
    }

    #[test]
    fn unsupported_and_failure_never_register() {
        let f = fake(
            || Err(DeviceError::Unsupported("global_hotkey")),
            || Ok(UnregisterOutcome::NotRegistered),
        );
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, HotkeyPhase::Unregistered);
        assert_eq!(s.notice, screen::UNSUPPORTED_STATUS);

        let f = fake(
            || Err(DeviceError::Internal("global_hotkey register")),
            || Ok(UnregisterOutcome::NotRegistered),
        );
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, HotkeyPhase::Unregistered);
        assert_eq!(s.error, screen::REGISTER_FAILED_ERROR);
    }

    #[test]
    fn unregister_failure_keeps_registered() {
        let f = fake(
            || Ok(RegisterOutcome::Registered),
            || Err(DeviceError::Internal("global_hotkey unregister")),
        );
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        screen::request_unregister(&mut s);
        step(&mut s, &sender, &rx, &f);
        assert_eq!(s.phase, HotkeyPhase::Registered);
        assert_eq!(s.error, screen::UNREGISTER_FAILED_ERROR);
    }

    #[test]
    fn presses_are_batched_into_one_completion() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        let on_press = f.on_press.lock().unwrap().clone().unwrap();
        for _ in 0..5 {
            on_press();
        }
        let mut completions = Vec::new();
        while let Ok(apply) = rx.try_recv() {
            completions.push(apply);
        }
        assert_eq!(completions.len(), 1);
        for apply in completions {
            apply(&mut s);
        }
        assert_eq!(s.count, 5);
        // 取り切った後の押下は再び 1 件送られる。
        on_press();
        let apply = rx.try_recv().unwrap();
        apply(&mut s);
        assert_eq!(s.count, 6);
    }

    #[test]
    fn stale_generation_press_is_dropped() {
        let f = ok_fake();
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        let old = f.on_press.lock().unwrap().clone().unwrap();
        screen::request_unregister(&mut s);
        step(&mut s, &sender, &rx, &f);
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &f);
        old();
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.count, 0);
        let new = f.on_press.lock().unwrap().clone().unwrap();
        new();
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        assert_eq!(s.count, 1);
    }

    #[test]
    fn unsupported_implementation_never_registers() {
        let (sender, rx) = channel();
        let mut s = GlobalHotkeyState::default();
        screen::request_register(&mut s);
        step(&mut s, &sender, &rx, &UnsupportedGlobalHotkey);
        assert_eq!(s.phase, HotkeyPhase::Unregistered);
        assert_eq!(s.notice, screen::UNSUPPORTED_STATUS);
    }

    // Windows ホストでは本物の `RegisterHotKey` を呼ぶため、非 Windows ホストだけで確かめる。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn windows_impl_is_unsupported_on_non_windows_host() {
        let imp = super::super::windows_global_hotkey::implementation();
        assert!(matches!(
            imp.register(Arc::new(|| {})),
            Err(DeviceError::Unsupported("global_hotkey"))
        ));
    }

    #[test]
    fn shutdown_reaches_shared_impl_without_panic() {
        // 共有実装に届くこと自体（パニックしない）だけを確かめる。実際の解除は #764 の目視で扱う。
        shutdown();
    }
}
