//! Phase 11 の拡充ループ（親 #125）型サンプル「無操作時間の検知」（Issue #631）の effect ドライバ。
//! `fandhe_device::user_idle::UserIdle` を 1 秒間隔で UI スレッドから同期 pull し、
//! `fandhe_core::screens::user_idle_show`（端末依存ゼロ）へ結果を `CompletionSender` 経由で渡す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::user_idle`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_user_idle`] の cfg 分岐が全対象 OS の最終形。macOS は `macos_user_idle`、Windows は
//! `windows_user_idle` が `implementation()` を持つ。iOS・Android は OS 全体の値を返す公開 API が
//! 無く対応表で `no` のため、fallback の `UnsupportedUserIdle`（常に `Unsupported`）が受け持ち、
//! 専用のスタブモジュールは置かない。Web は `fandhe-web-dom`（#632）で本 dispatch を通らない。
//! ダミーの成功実装へは倒さない。
//!
//! # 再描画の予約（契約表 4）
//!
//! - 間隔未満・結果が前回と同じ・読み取り成功・一時失敗: どれも次回読み取り時刻に再描画を予約する
//!   （入力が無いと egui が描画せず、pull が止まるため）
//! - 非対応（`Unsupported`）だけは予約しない。恒久的で回復しないため

use std::sync::Mutex;
use std::time::{Duration, Instant};

use fandhe_core::screens::user_idle_show::{
    apply_read, elapsed_secs, ReadFailure, UserIdleShowState,
};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use fandhe_device::user_idle::UnsupportedUserIdle;
use fandhe_device::user_idle::{UserIdle, READ_INTERVAL};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 直近の読み取り時刻（単調時計）。
/// ponytail: プロセス内 1 画面前提のグローバル。複数 Context が要るなら Context 単位へ移す。
static LAST_READ: Mutex<Option<Instant>> = Mutex::new(None);

/// 現在のプラットフォームに応じた `UserIdle` 実装を返す（状態を持たないため呼び出しごとに構築）。
fn platform_user_idle() -> Box<dyn UserIdle> {
    #[cfg(target_os = "macos")]
    {
        super::macos_user_idle::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_user_idle::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(UnsupportedUserIdle)
    }
}

/// 読み取りの失敗を画面用の理由へ写す。画面へは固定文言のみ（OS の生コードは出さない、
/// security.md A09）。
fn to_failure(e: DeviceError) -> ReadFailure {
    match e {
        DeviceError::Unsupported(_) => ReadFailure::Unsupported,
        DeviceError::PermissionDenied(_) => ReadFailure::PermissionDenied,
        other => ReadFailure::Failed(other.to_string()),
    }
}

/// 現在の画面状態が `result` と同じ内容か（同じなら送らない）。
fn unchanged(state: &UserIdleShowState, result: &Result<Duration, ReadFailure>) -> bool {
    let mut probe = state.clone();
    apply_read(&mut probe, result.clone());
    probe.view == state.view
}

/// 1 回分の処理。戻り値は予約した再描画の遅延（テスト用。予約しなければ `None`）。
fn drive(
    state: &UserIdleShowState,
    sender: &CompletionSender<UserIdleShowState>,
    idle: &dyn UserIdle,
    now: Instant,
    last: &mut Option<Instant>,
) -> Option<Duration> {
    // 非対応が確定済みなら間隔判定より先に止める（再描画・再読み取りを予約しない）。
    if matches!(
        state.view,
        fandhe_core::screens::user_idle_show::IdleView::Unsupported
    ) {
        return None;
    }
    if let Some(prev) = *last {
        let elapsed = now.saturating_duration_since(prev);
        if elapsed < READ_INTERVAL {
            let remaining = READ_INTERVAL - elapsed;
            sender.request_repaint_after(remaining);
            return Some(remaining);
        }
    }
    *last = Some(now);

    let result = idle.idle_for().map_err(to_failure);
    // 経過秒は 1 秒未満の差を捨てた秒で比べる（同じ秒なら送らない）。
    let same_secs = matches!((&result, elapsed_secs(state)), (Ok(d), Some(s)) if d.as_secs() == s);
    if !same_secs && !unchanged(state, &result) {
        // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
        let r = result.clone();
        let _ = sender.send(Box::new(move |s: &mut UserIdleShowState| apply_read(s, r)));
    }
    if matches!(result, Err(ReadFailure::Unsupported)) {
        return None;
    }
    sender.request_repaint_after(READ_INTERVAL);
    Some(READ_INTERVAL)
}

/// `registry.rs` の `effect` 区画から `user_idle_show` 画面用に配線される effect ドライバ。
pub fn user_idle_show_effect(
    state: &UserIdleShowState,
    sender: &CompletionSender<UserIdleShowState>,
) {
    let mut last = LAST_READ
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        platform_user_idle().as_ref(),
        Instant::now(),
        &mut last,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::user_idle_show::IdleView;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mock {
        count: AtomicUsize,
        f: Box<dyn Fn() -> Result<Duration, DeviceError> + Send + Sync>,
    }

    impl UserIdle for Mock {
        fn idle_for(&self) -> Result<Duration, DeviceError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            (self.f)()
        }
    }

    fn mock(f: impl Fn() -> Result<Duration, DeviceError> + Send + Sync + 'static) -> Mock {
        Mock {
            count: AtomicUsize::new(0),
            f: Box::new(f),
        }
    }

    fn channel() -> (
        CompletionSender<UserIdleShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<UserIdleShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<UserIdleShowState>>,
    ) -> UserIdleShowState {
        let mut s = UserIdleShowState::default();
        (rx.recv().expect("完了が送られる"))(&mut s);
        s
    }

    #[test]
    fn first_read_sends_and_schedules() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(Duration::from_secs(3)));
        let mut last = None;
        let d = drive(
            &UserIdleShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, Some(READ_INTERVAL));
        assert_eq!(apply(&rx).view, IdleView::Elapsed(3));
    }

    #[test]
    fn within_interval_does_not_read_but_schedules_remaining() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(Duration::from_secs(3)));
        let t0 = Instant::now();
        let mut last = Some(t0);
        let d = drive(
            &UserIdleShowState::default(),
            &sender,
            &ps,
            t0 + Duration::from_millis(400),
            &mut last,
        );
        assert_eq!(d, Some(Duration::from_millis(600)));
        assert_eq!(ps.count.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn same_second_is_not_sent_but_repaint_is_scheduled() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(Duration::from_millis(3400)));
        let mut state = UserIdleShowState::default();
        apply_read(&mut state, Ok(Duration::from_secs(3)));
        let mut last = None;
        let d = drive(&state, &sender, &ps, Instant::now(), &mut last);
        assert_eq!(d, Some(READ_INTERVAL));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_sends_and_stops_repaint() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::Unsupported("user_idle")));
        let mut last = None;
        let d = drive(
            &UserIdleShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, None);
        assert_eq!(apply(&rx).view, IdleView::Unsupported);
    }

    #[test]
    fn unsupported_state_stops_before_interval_check() {
        let (sender, rx) = channel();
        let ps = mock(|| Ok(Duration::from_secs(1)));
        let mut state = UserIdleShowState::default();
        apply_read(&mut state, Err(ReadFailure::Unsupported));
        let t0 = Instant::now();
        let mut last = Some(t0);
        let d = drive(
            &state,
            &sender,
            &ps,
            t0 + Duration::from_millis(400),
            &mut last,
        );
        assert_eq!(d, None);
        assert_eq!(ps.count.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn internal_error_sends_failed_and_keeps_polling() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::Internal("user_idle read")));
        let mut last = None;
        let d = drive(
            &UserIdleShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        assert_eq!(d, Some(READ_INTERVAL));
        assert!(matches!(apply(&rx).view, IdleView::Failed(_)));
    }

    #[test]
    fn permission_denied_is_not_shown_as_active() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::PermissionDenied("user_idle")));
        let mut last = None;
        drive(
            &UserIdleShowState::default(),
            &sender,
            &ps,
            Instant::now(),
            &mut last,
        );
        let s = apply(&rx);
        assert_eq!(s.view, IdleView::PermissionDenied);
        assert_eq!(
            fandhe_core::screens::user_idle_show::presence_text(&s),
            "在席: 判定できません"
        );
    }

    #[test]
    fn success_then_failure_drops_previous_value() {
        let (sender, rx) = channel();
        let ps = mock(|| Err(DeviceError::Internal("user_idle read")));
        let mut state = UserIdleShowState::default();
        apply_read(&mut state, Ok(Duration::from_secs(5)));
        let mut last = None;
        drive(&state, &sender, &ps, Instant::now(), &mut last);
        (rx.recv().expect("完了が送られる"))(&mut state);
        assert_eq!(elapsed_secs(&state), None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn fallback_platform_is_unsupported() {
        assert!(matches!(
            platform_user_idle().idle_for(),
            Err(DeviceError::Unsupported("user_idle"))
        ));
    }
}
