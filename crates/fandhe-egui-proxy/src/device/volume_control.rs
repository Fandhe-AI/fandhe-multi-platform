//! Phase 11 の拡充ループ（親 #166、Issue #785）「システム音量」の effect ドライバ。
//! `fandhe_device::volume_control::VolumeControl` へ要求を渡し、読み戻しを
//! `fandhe_core::screens::volume_control_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。契約の SSOT は
//! `fandhe_device::volume_control`。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を持つ
//! モジュールだけを埋める（macOS: `macos_volume_control`、Windows: `windows_volume_control`、
//! iOS: `ios_volume_control`（#786）、Android: `android_volume_control`（#787））。それ以外は
//! `UnsupportedVolumeControl` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 1 フレームの流れ
//!
//! 1. OS 呼び出しの完了待ち（`is_busy`）なら何もしない（完了・期限切れの注入で再描画される）
//! 2. 保留中の「＋」「－」があれば `begin` → OS 呼び出し → 結果の注入（読み取りより優先）
//! 3. 読み直しが要るなら、`READ_INTERVAL` 経過後に `begin` → 読み取り。間隔内なら残り時間で
//!    再描画を予約する（読まない分岐でも予約する、契約表 4）。読み取りに失敗・非対応のときは
//!    予約しない（失敗状態で再描画し続けない）
//!
//! # 同期とワーカー
//!
//! macOS / iOS / Android は UI スレッドから同期で呼ぶ。Windows は COM 初期化の都合（UI スレッドは
//! STA、実装は MTA）と戻らない場合の備えのため、ワーカーで呼び 5 秒の期限を `Instant`（単調時計）で
//! 測る。期限切れは `expire` で失敗を確定し、遅れて届いた結果は受信側を drop 済みのため捨てる。
//! 前回の OS 呼び出しが未終了の間は新しい呼び出しを起こさず失敗で確定する（スレッドの累積防止）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::volume_control_show::{
    apply_read, begin, expire, finish_step, take_step, wants_read, ControlMode, OpKind, Reading,
    StepDir, StepResult, VolumeControlShowState, VolumeFailure, BUSY_REASON, OPERATION_REASON,
    TIMEOUT_REASON,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::volume_control::UnsupportedVolumeControl;
use fandhe_device::volume_control::{
    StepDirection, StepOutcome, VolumeControl, VolumeControlMode, VolumeReading, OS_CALL_DEADLINE,
    READ_INTERVAL,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// OS 呼び出し（内側スレッド）が実行中か。期限切れで画面を失敗にしても呼び出しが戻るまでは
/// true のままで、その間は新しい OS スレッドを起こさない。
static OS_CALL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 現在のプラットフォームに応じた `VolumeControl` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn VolumeControl> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn VolumeControl>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_volume_control::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_volume_control::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_volume_control::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_volume_control::implementation()
            }
            #[cfg(not(any(
                target_os = "windows",
                target_os = "macos",
                target_os = "ios",
                target_os = "android"
            )))]
            {
                Arc::new(UnsupportedVolumeControl)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。OS のエラーコードは固定文言に畳み、画面・ログへ出さない
/// （security.md A09）。
fn to_failure(e: DeviceError) -> VolumeFailure {
    match e {
        DeviceError::Unsupported(_) => VolumeFailure::Unsupported,
        DeviceError::PermissionDenied(_) => VolumeFailure::PermissionDenied,
        _ => VolumeFailure::Failed(OPERATION_REASON.to_string()),
    }
}

fn to_mode(m: VolumeControlMode) -> ControlMode {
    match m {
        VolumeControlMode::Steps => ControlMode::Steps,
        VolumeControlMode::OsSlider => ControlMode::OsSlider,
        VolumeControlMode::NotChangeable => ControlMode::NotChangeable,
    }
}

fn to_reading(r: VolumeReading) -> Reading {
    Reading {
        level: r.level,
        muted: r.muted,
        mode: to_mode(r.mode),
    }
}

fn to_step_result(o: StepOutcome) -> StepResult {
    match o {
        StepOutcome::Changed(r) => StepResult::Changed(to_reading(r)),
        StepOutcome::Unchanged(r) => StepResult::Unchanged(to_reading(r)),
        StepOutcome::NotChangeable(r) => StepResult::NotChangeable(to_reading(r)),
    }
}

fn to_direction(d: StepDir) -> StepDirection {
    match d {
        StepDir::Up => StepDirection::Up,
        StepDir::Down => StepDirection::Down,
    }
}

/// OS 呼び出しの結果（種類ごと）。
enum Output {
    Read(Result<Reading, VolumeFailure>),
    Step(Result<StepResult, VolumeFailure>),
}

fn run(imp: &dyn VolumeControl, kind: OpKind) -> Output {
    match kind {
        OpKind::Read => Output::Read(imp.read().map(to_reading).map_err(to_failure)),
        OpKind::Step(d) => Output::Step(
            imp.step(to_direction(d))
                .map(to_step_result)
                .map_err(to_failure),
        ),
    }
}

fn failed_output(kind: OpKind, reason: &str) -> Output {
    let f = VolumeFailure::Failed(reason.to_string());
    match kind {
        OpKind::Read => Output::Read(Err(f)),
        OpKind::Step(_) => Output::Step(Err(f)),
    }
}

fn send_output(sender: &CompletionSender<VolumeControlShowState>, ticket: u64, out: Output) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut VolumeControlShowState| match out {
        Output::Read(r) => apply_read(s, ticket, r),
        Output::Step(r) => finish_step(s, ticket, r),
    }));
}

/// 最終読取の時刻（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing { last_read: None });

/// 実行経路。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    /// UI スレッドで同期に呼ぶ（macOS・iOS・Android）。
    Sync,
    /// ワーカーで呼び、期限を付ける（Windows）。
    Worker { deadline: Duration },
}

/// OS 呼び出し 1 件を起こす。`begin` を先に送り（FIFO で完了より前に適用される）、完了または
/// 期限切れを同じ ticket で送る。
fn issue(
    state: &VolumeControlShowState,
    sender: &CompletionSender<VolumeControlShowState>,
    imp: &Arc<dyn VolumeControl>,
    kind: OpKind,
    path: Path,
    in_flight: &'static AtomicBool,
) {
    let ticket = state.ticket() + 1;
    let _ = sender.send(Box::new(move |s: &mut VolumeControlShowState| {
        begin(s, ticket, kind)
    }));

    let deadline = match path {
        Path::Sync => {
            send_output(sender, ticket, run(imp.as_ref(), kind));
            return;
        }
        Path::Worker { deadline } => deadline,
    };

    // 前回の OS 呼び出しが未終了なら追加で起こさず、失敗として確定する（資源の累積防止）。
    if in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        send_output(sender, ticket, failed_output(kind, BUSY_REASON));
        return;
    }

    let worker_sender = sender.clone();
    let imp = Arc::clone(imp);
    let spawned = std::thread::Builder::new()
        .name("volume-control".to_string())
        .spawn(move || {
            let (tx, rx) = mpsc::channel();
            let inner = std::thread::Builder::new()
                .name("volume-control-os".to_string())
                .spawn(move || {
                    let out = run(imp.as_ref(), kind);
                    // 呼び出しが戻ってから解放する（期限切れ後でも戻るまでは次を起こさない）。
                    in_flight.store(false, Ordering::SeqCst);
                    let _ = tx.send(out);
                });
            match inner {
                // 期限は呼び出しの開始から `Instant`（単調時計）で測る。
                Ok(_) => match rx.recv_timeout(deadline) {
                    Ok(out) => send_output(&worker_sender, ticket, out),
                    Err(_) => {
                        let _ =
                            worker_sender.send(Box::new(move |s: &mut VolumeControlShowState| {
                                expire(s, ticket)
                            }));
                    }
                },
                Err(_) => {
                    in_flight.store(false, Ordering::SeqCst);
                    send_output(&worker_sender, ticket, failed_output(kind, TIMEOUT_REASON));
                }
            }
        });
    if spawned.is_err() {
        in_flight.store(false, Ordering::SeqCst);
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        send_output(sender, ticket, failed_output(kind, TIMEOUT_REASON));
    }
}

/// 要求の処理・間隔つき pull・再描画予約を行う本体（テストから実装と時刻を差し込めるよう切り出す）。
fn drive(
    state: &VolumeControlShowState,
    sender: &CompletionSender<VolumeControlShowState>,
    imp: &Arc<dyn VolumeControl>,
    path: Path,
    now: Instant,
    timing: &mut Timing,
    in_flight: &'static AtomicBool,
) {
    if state.is_busy() {
        return;
    }

    if let Some(dir) = take_step(state) {
        issue(state, sender, imp, OpKind::Step(dir), path, in_flight);
        // 変更の読み戻しも読み取りなので、次の pull までの間隔を取り直す。
        timing.last_read = Some(now);
        sender.request_repaint_after(READ_INTERVAL);
        return;
    }

    if !wants_read(state) {
        return;
    }
    // 初回・読み直し（Unread）は間隔を待たない。pull の間だけ間隔で絞る。
    let pulling = matches!(
        state.read_state(),
        fandhe_core::screens::volume_control_show::ReadState::Read(_)
    );
    let since = timing
        .last_read
        .map(|prev| now.saturating_duration_since(prev));
    match since {
        Some(d) if pulling && d < READ_INTERVAL => {
            // 読まない分岐でも次回の読み取りを予約する（契約表 4）。
            sender.request_repaint_after(READ_INTERVAL - d);
        }
        _ => {
            issue(state, sender, imp, OpKind::Read, path, in_flight);
            timing.last_read = Some(now);
            // 失敗した場合は次の完了で状態が Failed になり、以後 `wants_read` が偽で予約されない。
            sender.request_repaint_after(READ_INTERVAL);
        }
    }
}

/// `registry.rs` の `effect` 区画から `volume_control_show` 画面用に配線される effect ドライバ。
pub fn volume_control_show_effect(
    state: &VolumeControlShowState,
    sender: &CompletionSender<VolumeControlShowState>,
) {
    let path = if cfg!(target_os = "windows") {
        Path::Worker {
            deadline: OS_CALL_DEADLINE,
        }
    } else {
        Path::Sync
    };
    let mut timing = TIMING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drive(
        state,
        sender,
        &implementation(),
        path,
        Instant::now(),
        &mut timing,
        &OS_CALL_IN_FLIGHT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::volume_control_show::{
        error_text, level_text, notice_text, press_down, press_up, reload, ReadState, AT_MAX_NOTE,
        LEVEL_PREFIX, PERMISSION_DENIED_REASON, UNSUPPORTED_STATUS,
    };
    use fandhe_device::volume_control::UnsupportedVolumeControl as Unsupported;
    use std::sync::atomic::AtomicUsize;

    type Rx = mpsc::Receiver<crate::app::Completion<VolumeControlShowState>>;

    fn channel() -> (CompletionSender<VolumeControlShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut VolumeControlShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    fn flag() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    fn reading(level: f32) -> VolumeReading {
        VolumeReading {
            level: Some(level),
            muted: Some(false),
            mode: VolumeControlMode::Steps,
        }
    }

    /// 音量と結果を差し込める Fake。
    struct Fake {
        level: Mutex<f32>,
        step_error: Mutex<Option<DeviceError>>,
        read_error: bool,
        delay: Duration,
        reads: AtomicUsize,
        steps: AtomicUsize,
    }

    impl Fake {
        fn new(level: f32) -> Arc<Self> {
            Arc::new(Self {
                level: Mutex::new(level),
                step_error: Mutex::new(None),
                read_error: false,
                delay: Duration::ZERO,
                reads: AtomicUsize::new(0),
                steps: AtomicUsize::new(0),
            })
        }
        fn slow(level: f32, delay: Duration) -> Arc<Self> {
            let mut f = Arc::try_unwrap(Self::new(level)).ok().unwrap();
            f.delay = delay;
            Arc::new(f)
        }
        fn failing_read() -> Arc<Self> {
            let mut f = Arc::try_unwrap(Self::new(0.5)).ok().unwrap();
            f.read_error = true;
            Arc::new(f)
        }
    }

    impl VolumeControl for Fake {
        fn read(&self) -> Result<VolumeReading, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            if self.read_error {
                return Err(DeviceError::Internal("x"));
            }
            Ok(reading(*self.level.lock().unwrap()))
        }
        fn step(&self, direction: StepDirection) -> Result<StepOutcome, DeviceError> {
            self.steps.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            if let Some(e) = self.step_error.lock().unwrap().take() {
                return Err(e);
            }
            let mut l = self.level.lock().unwrap();
            let before = reading(*l);
            let next = match direction {
                StepDirection::Up => (*l + 0.1).min(1.0),
                StepDirection::Down => (*l - 0.1).max(0.0),
            };
            *l = next;
            let after = reading(next);
            Ok(fandhe_device::volume_control::classify_step(&before, after))
        }
    }

    fn dynamic(f: Arc<Fake>) -> Arc<dyn VolumeControl> {
        f
    }

    /// 同期経路で 1 フレーム回し、届いた完了をすべて適用する。
    fn frame(
        s: &mut VolumeControlShowState,
        tx: &CompletionSender<VolumeControlShowState>,
        rx: &Rx,
        imp: &Arc<dyn VolumeControl>,
        now: Instant,
        timing: &mut Timing,
    ) {
        drive(s, tx, imp, Path::Sync, now, timing, flag());
        apply_all(rx, s);
    }

    #[test]
    fn first_frame_reads_and_shows_level() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        frame(
            &mut s,
            &tx,
            &rx,
            &imp,
            Instant::now(),
            &mut Timing::default(),
        );
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}50%"));
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pull_respects_interval_then_rereads() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        frame(
            &mut s,
            &tx,
            &rx,
            &imp,
            t0 + Duration::from_millis(400),
            &mut timing,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        // 表示が同じでも読み直しは省かない。
        frame(&mut s, &tx, &rx, &imp, t0 + READ_INTERVAL, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
        // 外部からの変更を反映する。
        *fake.level.lock().unwrap() = 0.8;
        frame(&mut s, &tx, &rx, &imp, t0 + READ_INTERVAL * 2, &mut timing);
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}80%"));
    }

    #[test]
    fn step_up_shows_readback_value() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        press_up(&mut s);
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(fake.steps.load(Ordering::SeqCst), 1);
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}60%"));
    }

    #[test]
    fn step_at_limit_is_unchanged_not_failure() {
        let (tx, rx) = channel();
        let fake = Fake::new(1.0);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        press_up(&mut s);
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(error_text(&s), "");
        assert_eq!(notice_text(&s), AT_MAX_NOTE);
    }

    #[test]
    fn permission_denied_is_its_own_failure() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        *fake.step_error.lock().unwrap() = Some(DeviceError::PermissionDenied("volume-control"));
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        press_down(&mut s);
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(error_text(&s), PERMISSION_DENIED_REASON);
    }

    #[test]
    fn unsupported_never_succeeds_and_stops_polling() {
        let (tx, rx) = channel();
        let imp: Arc<dyn VolumeControl> = Arc::new(Unsupported);
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(s.read_state(), &ReadState::Unsupported);
        assert_eq!(level_text(&s), UNSUPPORTED_STATUS);
        press_up(&mut s);
        frame(&mut s, &tx, &rx, &imp, t0 + READ_INTERVAL * 3, &mut timing);
        assert!(rx.try_recv().is_err(), "非対応では要求を出さない");
    }

    #[test]
    fn failed_read_stops_polling_and_reload_reads_immediately() {
        let (tx, rx) = channel();
        let fake = Fake::failing_read();
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(s.read_state(), &ReadState::Failed);
        frame(&mut s, &tx, &rx, &imp, t0 + READ_INTERVAL * 3, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1, "失敗後は読み直さない");
        reload(&mut s);
        // 間隔を待たずに読む。
        frame(&mut s, &tx, &rx, &imp, t0 + READ_INTERVAL * 3, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn busy_state_does_not_double_issue() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        // 完了を適用しないまま begin だけ適用した状態（OS 呼び出し中）を作る。
        begin(&mut s, 1, OpKind::Read);
        drive(
            &s,
            &tx,
            &imp,
            Path::Sync,
            Instant::now(),
            &mut Timing::default(),
            flag(),
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.reads.load(Ordering::SeqCst), 0);
    }

    /// Windows 経路で、戻らない OS 呼び出しは期限で失敗になり、遅れた結果は成功にならない。
    #[test]
    fn worker_deadline_expires_and_late_result_is_dropped() {
        let (tx, rx) = channel();
        let fake = Fake::slow(0.5, Duration::from_millis(400));
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let path = Path::Worker {
            deadline: Duration::from_millis(50),
        };
        drive(
            &s,
            &tx,
            &imp,
            path,
            Instant::now(),
            &mut Timing::default(),
            flag(),
        );
        // begin と expire が届くまで待つ。
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && s.read_state() != &ReadState::Failed {
            apply_all(&rx, &mut s);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(!s.is_busy());
        // OS 呼び出しが遅れて戻っても、何も届かず状態も変わらない。
        std::thread::sleep(Duration::from_millis(500));
        let before = s.clone();
        apply_all(&rx, &mut s);
        assert_eq!(s, before);
    }

    #[test]
    fn worker_does_not_stack_calls_while_previous_in_flight() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let busy = flag();
        busy.store(true, Ordering::SeqCst);
        let mut s = VolumeControlShowState::default();
        drive(
            &s,
            &tx,
            &imp,
            Path::Worker {
                deadline: Duration::from_secs(1),
            },
            Instant::now(),
            &mut Timing::default(),
            busy,
        );
        apply_all(&rx, &mut s);
        assert_eq!(
            fake.reads.load(Ordering::SeqCst),
            0,
            "新しい OS 呼び出しを起こさない"
        );
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(!s.is_busy());
    }

    #[test]
    fn pending_step_during_read_is_issued_after_completion() {
        let (tx, rx) = channel();
        let fake = Fake::new(0.5);
        let imp = dynamic(fake.clone());
        let mut s = VolumeControlShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        press_up(&mut s);
        // 呼び出し中に押された体（begin 済み）で保留される。
        let t = s.ticket() + 1;
        begin(&mut s, t, OpKind::Read);
        press_down(&mut s);
        drive(&s, &tx, &imp, Path::Sync, t0, &mut timing, flag());
        assert_eq!(fake.steps.load(Ordering::SeqCst), 0);
        apply_read(
            &mut s,
            t,
            Ok(Reading {
                level: Some(0.5),
                muted: Some(false),
                mode: ControlMode::Steps,
            }),
        );
        frame(&mut s, &tx, &rx, &imp, t0, &mut timing);
        assert_eq!(fake.steps.load(Ordering::SeqCst), 1);
        assert_eq!(level_text(&s), format!("{LEVEL_PREFIX}40%"));
    }

    #[test]
    fn error_mapping_keeps_codes_out_of_screen() {
        assert_eq!(
            to_failure(DeviceError::Internal("volume-control hr=0x80004005")),
            VolumeFailure::Failed(OPERATION_REASON.to_string())
        );
        assert_eq!(
            to_failure(DeviceError::Unsupported("volume-control")),
            VolumeFailure::Unsupported
        );
    }

    /// 対象外 OS のフォールバックは成功を返さない。
    #[test]
    fn fallback_never_returns_success() {
        let u = Unsupported;
        assert!(u.read().is_err());
        assert!(u.step(StepDirection::Up).is_err());
    }
}
