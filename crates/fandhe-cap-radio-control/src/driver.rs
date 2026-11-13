//! Phase 11（親 #189）型サンプル「無線の状態と切り替え」（Issue #972）の effect ドライバ。
//! `fandhe_core::screens::radio_control_show`（端末依存ゼロ）の要求を観測し、`RadioControl` で
//! 状態を 1 回読む / 切り替えを 1 回要求し、`CompletionSender` 経由で結果を返す
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、
//! `fandhe_runtime::EffectFn` 参照）。契約の SSOT は `crate::contract`。
//!
//! # スレッド
//!
//! macOS の管理者パスワードのダイアログや Windows の同意 UI（許可要求の開始だけ UI スレッド）を待つブロッキング呼び出しになりうるため、
//! UI を止めないよう OS を問わずワーカースレッドで呼ぶ。切り替え（`set`）の後は成否によらず同じ
//! ワーカーで `read` を 1 回呼び、画面は読み直した値だけで状態を出す。完了は画面状態の世代照合つきで
//! 入れる（古い世代は捨てられる）。要求の追跡は世代照合で足りるため `RequestSlot` は使わない
//! （`wifi_connect.rs` と同じ判断）。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを埋める
//! （macOS: `macos`（#972）、Windows: `windows`（#973）、
//! Android: `android`（#974））。それ以外（iOS・Linux 等）は
//! `UnsupportedRadioControl` 固定で、ダミーの成功実装へは倒さない。

use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
use crate::contract::UnsupportedRadioControl;
use crate::contract::{DENIED_BY_SYSTEM, RadioControl, RadioKind, RadioState, SwitchOutcome};
use crate::error::RadioControlError;
use fandhe_core::screens::radio_control_show::{
    Action, Failure, Kind, RadioControlShowState, Reading, Switched, finish_read, finish_switch,
    mark_busy, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `RadioControl` 実装を返す（プロセスで 1 個）。
/// 画面の状態型に依存しないため、フレームワークの外のアプリも使える。
pub fn platform_radio_control() -> Arc<dyn RadioControl> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn RadioControl>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                crate::macos::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                crate::windows::implementation()
            }
            #[cfg(target_os = "android")]
            {
                crate::android::implementation()
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
            {
                Arc::new(UnsupportedRadioControl)
            }
        })
        .clone()
}

/// `RadioControlError` を画面の失敗種別へ写す（全呼び出し箇所でこの 1 関数を通す）。その他は `Failed` へ
/// 畳み、OS 固有の値を画面へ漏らさない（security.md A09）。
fn to_failure(e: RadioControlError) -> Failure {
    match e {
        RadioControlError::Unsupported(_) => Failure::Unsupported,
        RadioControlError::PermissionDenied(_) => Failure::PermissionDenied,
        RadioControlError::Internal(DENIED_BY_SYSTEM) => Failure::DeniedBySystem,
        _ => Failure::Failed,
    }
}

fn to_reading(s: RadioState) -> Reading {
    match s {
        RadioState::On => Reading::On,
        RadioState::Off => Reading::Off,
        RadioState::Disabled => Reading::Disabled,
        RadioState::Transitioning => Reading::Transitioning,
        RadioState::Unknown => Reading::Unknown,
        RadioState::NotPresent => Reading::NotPresent,
    }
}

fn to_switched(o: SwitchOutcome) -> Switched {
    match o {
        SwitchOutcome::Applied => Switched::Applied,
        SwitchOutcome::Accepted => Switched::Accepted,
        SwitchOutcome::Opened => Switched::Opened,
        SwitchOutcome::NotChanged => Switched::NotChanged,
    }
}

fn to_radio_kind(k: Kind) -> RadioKind {
    match k {
        Kind::Wifi => RadioKind::Wifi,
        Kind::Bluetooth => RadioKind::Bluetooth,
    }
}

/// 切り替え前の許可要求の開始関数（UI スレッドで呼ぶ）。結果は戻り値のチャネルへ 1 回流れる。
type AccessStarter = fn() -> Receiver<Result<(), RadioControlError>>;

/// 現在の OS が切り替え前に要する許可要求（Windows のみ）。注入した実装は OS の許可状態に依存しない。
fn access_starter() -> Option<AccessStarter> {
    #[cfg(target_os = "windows")]
    {
        Some(crate::windows::begin_access)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装と許可要求を差し込めるよう切り出す）。
fn drive(
    state: &RadioControlShowState,
    sender: &CompletionSender<RadioControlShowState>,
    radio: Arc<dyn RadioControl>,
    start_access: Option<AccessStarter>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let (generation, kind, action) = (request.generation, request.kind, request.action);

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_busy));

    // 許可要求（Windows）は UI スレッド（ここ）から始める。ワーカーは結果をチャネルで待つだけ。
    let access = start_access
        .filter(|_| matches!(action, Action::Set(_)))
        .map(|f| f());

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("radio-control".to_string())
        .spawn(move || {
            let radio_kind = to_radio_kind(kind);
            match action {
                Action::Read => {
                    let result = radio.read(radio_kind).map(to_reading).map_err(to_failure);
                    let _ = worker_sender.send(Box::new(move |s: &mut RadioControlShowState| {
                        finish_read(s, generation, kind, result);
                    }));
                }
                Action::Set(on) => {
                    let granted = access.map_or(Ok(()), |rx| {
                        rx.recv()
                            .unwrap_or(Err(RadioControlError::Internal(crate::contract::FAILED)))
                    });
                    let result = granted
                        .and_then(|()| radio.set(radio_kind, on))
                        .map(to_switched)
                        .map_err(to_failure);
                    // 成否によらず読み直し、状態が変わっていないことも見せる。
                    let reread = radio.read(radio_kind).map(to_reading).map_err(to_failure);
                    let _ = worker_sender.send(Box::new(move |s: &mut RadioControlShowState| {
                        finish_switch(s, generation, kind, on, result, reread);
                    }));
                }
            }
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut RadioControlShowState| {
            finish_spawn_failed(s, generation, kind, action);
        }));
    }
}

/// スレッド生成に失敗した要求を、種類に合う失敗文言（読み取り / 切り替え）で確定する。
/// OS を呼べていないため、切り替えの読み直しも失敗として渡す。
fn finish_spawn_failed(s: &mut RadioControlShowState, generation: u64, kind: Kind, action: Action) {
    match action {
        Action::Read => finish_read(s, generation, kind, Err(Failure::Failed)),
        Action::Set(on) => finish_switch(
            s,
            generation,
            kind,
            on,
            Err(Failure::Failed),
            Err(Failure::Failed),
        ),
    }
}

/// Android: Activity 再生成直後に gallery の `android_main` から呼ぶ。保留中の切り替え要求を破棄し、
/// 待機中のワーカーを失敗で戻す（復元しない）。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    crate::android::reset_pending_tracking();
}

/// `registry.rs` の `effect` 区画から `radio_control_show` 画面用に配線される effect ドライバ。
pub fn radio_control_show_effect(
    state: &RadioControlShowState,
    sender: &CompletionSender<RadioControlShowState>,
) {
    drive(state, sender, platform_radio_control(), access_starter());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{FAILED, FEATURE};
    use fandhe_core::screens::radio_control_show::{
        FAILED_DENIED_BY_SYSTEM, FAILED_PERMISSION, FAILED_READ, FAILED_READ_UNSUPPORTED,
        FAILED_SWITCH, FAILED_SWITCH_UNSUPPORTED, Phase, STATE_OFF, STATE_ON, STATE_PREFIX,
        bluetooth_off, wifi_on, wifi_read,
    };
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    type ErrFn = fn() -> RadioControlError;
    type Reads = Mutex<Vec<Result<RadioState, ErrFn>>>;

    struct Fake {
        reads: Reads,
        set: Result<SwitchOutcome, fn() -> RadioControlError>,
        read_calls: AtomicUsize,
        set_calls: AtomicUsize,
    }

    impl Fake {
        fn new(
            reads: Vec<Result<RadioState, fn() -> RadioControlError>>,
            set: Result<SwitchOutcome, fn() -> RadioControlError>,
        ) -> Arc<Self> {
            Arc::new(Self {
                reads: Mutex::new(reads),
                set,
                read_calls: AtomicUsize::new(0),
                set_calls: AtomicUsize::new(0),
            })
        }
    }

    impl RadioControl for Fake {
        fn read(&self, _kind: RadioKind) -> Result<RadioState, RadioControlError> {
            self.read_calls.fetch_add(1, Ordering::SeqCst);
            let mut q = self.reads.lock().unwrap();
            let next = if q.len() > 1 { q.remove(0) } else { q[0] };
            next.map_err(|f| f())
        }
        fn set(&self, _kind: RadioKind, _on: bool) -> Result<SwitchOutcome, RadioControlError> {
            self.set_calls.fetch_add(1, Ordering::SeqCst);
            self.set.map_err(|f| f())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<RadioControlShowState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<RadioControlShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    /// 要求を立てて `drive` し、届いた完了（mark_busy と結果）をすべて適用した状態を返す。
    fn run(
        radio: Arc<dyn RadioControl>,
        press: fn(&mut RadioControlShowState),
    ) -> RadioControlShowState {
        let (sender, rx) = sender_with_channel();
        let mut state = RadioControlShowState::default();
        press(&mut state);
        drive(&state, &sender, radio, None);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_busy と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(vec![Ok(RadioState::On)], Ok(SwitchOutcome::Applied));
        drive(
            &RadioControlShowState::default(),
            &sender,
            fake.clone(),
            None,
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.read_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn read_calls_os_once_and_shows_state() {
        let fake = Fake::new(vec![Ok(RadioState::On)], Ok(SwitchOutcome::Applied));
        let state = run(fake.clone(), wifi_read);
        assert_eq!(state.wifi.status, format!("{STATE_PREFIX}{STATE_ON}"));
        assert_eq!(fake.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.set_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn set_is_followed_by_one_reread() {
        let fake = Fake::new(vec![Ok(RadioState::Off)], Ok(SwitchOutcome::Accepted));
        let state = run(fake.clone(), wifi_on);
        assert_eq!(fake.set_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.read_calls.load(Ordering::SeqCst), 1);
        // 受け付けたが読み直しはオフのまま。「オンになった」とは出さない。
        assert!(
            state
                .wifi
                .status
                .ends_with(&format!("{STATE_PREFIX}{STATE_OFF}"))
        );
        assert_eq!(state.wifi.error, "");
    }

    #[test]
    fn spawn_failure_uses_action_specific_error() {
        let mut state = RadioControlShowState::default();
        wifi_on(&mut state);
        let r = take_request(&state).expect("要求あり");
        finish_spawn_failed(&mut state, r.generation, r.kind, r.action);
        assert_eq!(state.wifi.error, FAILED_SWITCH);

        let mut state = RadioControlShowState::default();
        wifi_read(&mut state);
        let r = take_request(&state).expect("要求あり");
        finish_spawn_failed(&mut state, r.generation, r.kind, r.action);
        assert_eq!(state.wifi.error, FAILED_READ);
    }

    #[test]
    fn failed_set_still_rereads() {
        let fake = Fake::new(
            vec![Ok(RadioState::Off)],
            Err(|| RadioControlError::Internal(FAILED)),
        );
        let state = run(fake.clone(), wifi_on);
        assert_eq!(fake.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(state.wifi.error, FAILED_SWITCH);
        assert_eq!(state.wifi.status, format!("{STATE_PREFIX}{STATE_OFF}"));
    }

    #[test]
    fn unsupported_set_on_bluetooth_is_not_success() {
        let fake = Fake::new(
            vec![Ok(RadioState::On)],
            Err(|| RadioControlError::Unsupported(FEATURE)),
        );
        let state = run(fake, bluetooth_off);
        assert_eq!(state.bluetooth.error, FAILED_SWITCH_UNSUPPORTED);
    }

    #[test]
    fn each_error_maps_to_distinct_failure() {
        let cases: [(ErrFn, &str); 5] = [
            (
                || RadioControlError::PermissionDenied(FEATURE),
                FAILED_PERMISSION,
            ),
            (
                || RadioControlError::Internal(DENIED_BY_SYSTEM),
                FAILED_DENIED_BY_SYSTEM,
            ),
            (
                || RadioControlError::Unsupported(FEATURE),
                FAILED_READ_UNSUPPORTED,
            ),
            (|| RadioControlError::Internal(FAILED), FAILED_READ),
            // その他の種別は Failed に畳み、成功にしない。
            (|| RadioControlError::Internal("other"), FAILED_READ),
        ];
        for (f, expected) in cases {
            let fake = Fake::new(vec![Err(f)], Ok(SwitchOutcome::Applied));
            let state = run(fake, wifi_read);
            assert_eq!(state.wifi.error, expected);
            assert_eq!(state.wifi.status, "");
        }
    }

    #[test]
    fn second_drive_while_busy_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(vec![Ok(RadioState::On)], Ok(SwitchOutcome::Applied));
        let mut state = RadioControlShowState::default();
        wifi_read(&mut state);
        drive(&state, &sender, fake.clone(), None);
        rx.recv_timeout(Duration::from_secs(5)).expect("mark_busy")(&mut state);
        drive(&state, &sender, fake.clone(), None);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.read_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let mut state = RadioControlShowState::default();
        wifi_read(&mut state);
        drive(
            &state,
            &sender,
            Fake::new(vec![Ok(RadioState::On)], Ok(SwitchOutcome::Applied)),
            None,
        );
        rx.recv_timeout(Duration::from_secs(5)).expect("mark_busy")(&mut state);
        // 完了が届く前に世代が進んだ状況を再現する。
        state.phase = Phase::Idle;
        wifi_on(&mut state);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert_eq!(state.generation, 2);
        assert_eq!(state.phase, Phase::Requested);
        assert_eq!(state.wifi.status, "");
    }

    #[test]
    fn unsupported_implementation_never_yields_success() {
        let state = run(Arc::new(crate::contract::UnsupportedRadioControl), wifi_on);
        assert_eq!(state.wifi.error, FAILED_SWITCH_UNSUPPORTED);
        assert_eq!(state.wifi.status, "");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            platform_radio_control().read(RadioKind::Wifi),
            Err(RadioControlError::Unsupported(_))
        ));
    }
}
