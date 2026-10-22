//! Phase 11（親 #120）型サンプル「画面キャプチャの防止・検知」（Issue #582）の effect ドライバ。
//! `fandhe_core::screens::capture_protection_show`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::capture_protection::CaptureProtection` で防止を掛ける / 外し、直後に
//! [`CaptureProtection::is_protected`] で実際の状態を読み戻して `CompletionSender` 経由で返す。
//! 要求がないときは [`READ_INTERVAL`] ごとの間隔付き pull で保護状態・録画・スクショ回数を更新する
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約（エラー種別・守る範囲）の SSOT は `fandhe_device::capture_protection`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # スレッド
//!
//! 全 OS とも呼ばれたスレッド（egui の update ループ = UI スレッド）で同期に呼び、
//! ワーカースレッドは作らない（Windows の `SetWindowDisplayAffinity` はウィンドウを作ったスレッドから）。
//!
//! # 対応 OS と後続
//!
//! Windows: `windows_capture_protection`（防止のみ）。iOS（#583）: `ios_capture_protection`、
//! Android（#584）: `android_capture_protection`（いずれも現状は `Unsupported` を返すスタブ）。
//! dispatch・共通コア画面は本 PR で最終形にしてある。macOS は対応表が `no`（公式が旧定数として
//! 用途を否定）で、Linux などとともに `UnsupportedCaptureProtection` 固定。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use fandhe_core::screens::capture_protection_show::{
    apply_read, finish, mark_applying, take_request, CaptureFailure, CaptureProtectionShowState,
    ReadView, SupportView,
};
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
use fandhe_device::capture_protection::UnsupportedCaptureProtection;
use fandhe_device::capture_protection::{CaptureProtection, READ_INTERVAL};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `CaptureProtection` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn CaptureProtection> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn CaptureProtection>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_capture_protection::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_capture_protection::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_capture_protection::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedCaptureProtection)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外は固定文言へ畳み、OS のエラーコードを
/// 画面・ログへ出さない（security.md A09）。`PermissionDenied` / `Cancelled` は契約上返らないが、
/// 返ってきても成功表示にはしない。
fn to_failure(e: DeviceError) -> CaptureFailure {
    match e {
        DeviceError::Unsupported(_) => CaptureFailure::Unsupported,
        _ => CaptureFailure::Failed("OS の呼び出しに失敗しました".to_string()),
    }
}

/// `GetWindowDisplayAffinity` の値を保護状態へ写す。未知の値は `None`（嘘の成功の禁止）。
/// `WDA_MONITOR`(1) は Windows 10 2004 より前の代替で、撮影には黒く写るため保護中と扱う。
/// ホストでも検証できるよう OS 非依存の純関数にしてある。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn affinity_to_protected(raw: u32) -> Option<bool> {
    match raw {
        0x0 => Some(false),
        0x1 | 0x11 => Some(true),
        _ => None,
    }
}

/// 最終読取の時刻（プロセス全体で 1 つ）。単調時計で測る。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing { last_read: None });

/// OS から現在の状態を読む。
fn read(imp: &dyn CaptureProtection) -> ReadView {
    let support = imp.support();
    let obs = imp.observe();
    ReadView {
        support: SupportView {
            prevent: support.prevent,
            recording: support.detect_recording,
            screenshot: support.detect_screenshot,
        },
        protected: imp.is_protected(),
        recording: obs.recording,
        screenshot_count: obs.screenshot_count,
    }
}

/// 要求の処理と間隔付き pull の本体（テストから時刻と実装を差し込めるよう切り出す）。
fn drive(
    state: &CaptureProtectionShowState,
    sender: &CompletionSender<CaptureProtectionShowState>,
    imp: &dyn CaptureProtection,
    now: Instant,
    timing: &mut Timing,
) {
    if let Some(request) = take_request(state) {
        let generation = request.generation;
        // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
        // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
        let _ = sender.send(Box::new(mark_applying));
        let result = imp.set_protected(request.on).map_err(to_failure);
        // 成否に関わらず、結果を届ける前に実際の状態を読み戻す（嘘の成功の禁止）。
        let protected = imp.is_protected();
        let _ = sender.send(Box::new(move |s: &mut CaptureProtectionShowState| {
            finish(s, generation, result, protected);
        }));
        // 要求を処理したフレームでは pull しない。次の読み取りは間隔後に予約する。
        timing.last_read = Some(now);
        sender.request_repaint_after(READ_INTERVAL);
        return;
    }

    let due = timing
        .last_read
        .is_none_or(|prev| now.saturating_duration_since(prev) >= READ_INTERVAL);
    if due {
        timing.last_read = Some(now);
    }
    // 送信を省く分岐でも次回の読み取りを予約する（契約表 4）。
    let since_read = timing.last_read.map_or(std::time::Duration::ZERO, |p| {
        now.saturating_duration_since(p)
    });
    sender.request_repaint_after(READ_INTERVAL.saturating_sub(since_read));
    if !due || state.phase != fandhe_core::screens::capture_protection_show::CapturePhase::Idle {
        return;
    }

    let view = read(imp);
    // 同じ値なら送信を省く。
    let mut next = state.clone();
    apply_read(&mut next, view);
    if next == *state {
        return;
    }
    let _ = sender.send(Box::new(move |s: &mut CaptureProtectionShowState| {
        apply_read(s, view);
    }));
}

/// `registry.rs` の `effect` 区画から `capture_protection_show` 画面用に配線される effect ドライバ。
pub fn capture_protection_show_effect(
    state: &CaptureProtectionShowState,
    sender: &CompletionSender<CaptureProtectionShowState>,
) {
    let mut timing = TIMING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        implementation().as_ref(),
        Instant::now(),
        &mut timing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::capture_protection_show::{
        protection_text, turn_off, turn_on, CapturePhase, FAILED_PREFIX,
        PREVENT_UNSUPPORTED_STATUS, PROTECTED_STATUS, UNPROTECTED_STATUS,
    };
    use fandhe_device::capture_protection::{
        CaptureObservation, CaptureSupport, UnsupportedCaptureProtection as Unsupported,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<CaptureProtectionShowState>>;

    fn sender_with_channel() -> (CompletionSender<CaptureProtectionShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 成功なら保護状態を要求どおりに更新し、失敗なら変えない Fake。
    struct Fake {
        protected: AtomicBool,
        fail: Option<fn() -> DeviceError>,
        /// `Ok` を返すのに状態を変えない（嘘の成功の再現）。
        ignore: bool,
        /// 読み戻しが常に `None`。
        unreadable: bool,
        reads: AtomicUsize,
    }

    impl CaptureProtection for Fake {
        fn support(&self) -> CaptureSupport {
            CaptureSupport {
                prevent: true,
                detect_recording: false,
                detect_screenshot: false,
            }
        }
        fn set_protected(&self, on: bool) -> Result<(), DeviceError> {
            if let Some(f) = self.fail {
                return Err(f());
            }
            if !self.ignore {
                self.protected.store(on, Ordering::SeqCst);
            }
            Ok(())
        }
        fn is_protected(&self) -> Option<bool> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            (!self.unreadable).then(|| self.protected.load(Ordering::SeqCst))
        }
        fn observe(&self) -> CaptureObservation {
            CaptureObservation::default()
        }
    }

    fn fake(fail: Option<fn() -> DeviceError>, ignore: bool, unreadable: bool) -> Arc<Fake> {
        Arc::new(Fake {
            protected: AtomicBool::new(false),
            fail,
            ignore,
            unreadable,
            reads: AtomicUsize::new(0),
        })
    }

    /// 要求を立てて `drive` し、届いた完了（mark_applying + finish）をすべて適用する。
    fn run(
        state: &mut CaptureProtectionShowState,
        imp: &dyn CaptureProtection,
        on: impl Fn(&mut CaptureProtectionShowState),
    ) {
        let (sender, rx) = sender_with_channel();
        on(state);
        drive(state, &sender, imp, Instant::now(), &mut Timing::default());
        for _ in 0..2 {
            let c = rx.try_recv().expect("mark_applying と完了が届く");
            c(state);
        }
    }

    #[test]
    fn idle_before_interval_sends_nothing_and_does_not_read() {
        let (sender, rx) = sender_with_channel();
        let f = fake(None, false, false);
        let now = Instant::now();
        let mut timing = Timing {
            last_read: Some(now),
        };
        drive(
            &CaptureProtectionShowState::default(),
            &sender,
            f.as_ref(),
            now + Duration::from_millis(100),
            &mut timing,
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn due_read_sends_once_and_skips_same_value() {
        let (sender, rx) = sender_with_channel();
        let f = fake(None, false, false);
        let mut state = CaptureProtectionShowState::default();
        let mut timing = Timing::default();
        let now = Instant::now();
        drive(&state, &sender, f.as_ref(), now, &mut timing);
        rx.try_recv().expect("初回は送る")(&mut state);
        assert_eq!(state.protected, Some(false));
        // 間隔後でも同じ値なら送信を省く。
        drive(
            &state,
            &sender,
            f.as_ref(),
            now + READ_INTERVAL + Duration::from_millis(1),
            &mut timing,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn on_off_round_trip() {
        let f = fake(None, false, false);
        let mut state = CaptureProtectionShowState::default();
        run(&mut state, f.as_ref(), turn_on);
        assert_eq!(protection_text(&state), PROTECTED_STATUS);
        run(&mut state, f.as_ref(), turn_off);
        assert_eq!(protection_text(&state), UNPROTECTED_STATUS);
        assert_eq!(state.phase, CapturePhase::Idle);
    }

    #[test]
    fn ok_without_effect_is_failure_not_protected() {
        let mut state = CaptureProtectionShowState::default();
        run(&mut state, fake(None, true, false).as_ref(), turn_on);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_ne!(protection_text(&state), PROTECTED_STATUS);
    }

    #[test]
    fn ok_with_unreadable_state_is_failure_not_protected() {
        let mut state = CaptureProtectionShowState::default();
        run(&mut state, fake(None, false, true).as_ref(), turn_on);
        assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
        assert_ne!(protection_text(&state), PROTECTED_STATUS);
    }

    #[test]
    fn unsupported_is_failure_not_success() {
        let mut state = CaptureProtectionShowState::default();
        run(&mut state, &Unsupported, turn_on);
        assert_eq!(state.error, PREVENT_UNSUPPORTED_STATUS);
        assert_ne!(protection_text(&state), PROTECTED_STATUS);
    }

    #[test]
    fn every_error_kind_is_failure_not_success() {
        let kinds: [fn() -> DeviceError; 3] = [
            || DeviceError::Internal("capture-protection"),
            || DeviceError::PermissionDenied("capture-protection"),
            || DeviceError::Cancelled("capture-protection"),
        ];
        for kind in kinds {
            let mut state = CaptureProtectionShowState::default();
            run(&mut state, fake(Some(kind), false, false).as_ref(), turn_on);
            assert!(state.error.starts_with(FAILED_PREFIX), "{}", state.error);
            assert_ne!(protection_text(&state), PROTECTED_STATUS);
        }
    }

    #[test]
    fn stale_completion_is_ignored() {
        let f = fake(None, false, false);
        let mut state = CaptureProtectionShowState::default();
        let (sender, rx) = sender_with_channel();
        turn_on(&mut state);
        drive(
            &state,
            &sender,
            f.as_ref(),
            Instant::now(),
            &mut Timing::default(),
        );
        // 世代が進んだ後に古い完了が届いても捨てる。
        state.generation = state.generation.wrapping_add(1);
        state.phase = CapturePhase::Applying;
        for _ in 0..2 {
            rx.try_recv().expect("届く")(&mut state);
        }
        assert_eq!(state.protected, None);
    }

    #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            implementation().set_protected(true),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn affinity_mapping() {
        assert_eq!(affinity_to_protected(0x0), Some(false));
        assert_eq!(affinity_to_protected(0x1), Some(true));
        assert_eq!(affinity_to_protected(0x11), Some(true));
        assert_eq!(affinity_to_protected(0x2), None);
        assert_eq!(affinity_to_protected(u32::MAX), None);
    }
}
