//! Phase 11 の拡充ループ（親 #128、Issue #655）「画面の向き」の effect ドライバ。
//! `fandhe_device::screen_orientation::ScreenOrientation` へ固定・解除を依頼し、読み戻しを
//! `fandhe_core::screens::screen_orientation_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる。ワーカーは作らない）。
//! 契約（依頼と完了の分離・エラー種別・守る範囲）の SSOT は `fandhe_device::screen_orientation`。
//!
//! 固定・解除は依頼だけでは確定せず、毎フレーム読み戻して「実際の向きが固定した種類に入り、OS 側の
//! 希望も一致した」ときに成功を `RequestSlot` 経由で返す。期限（`CONFIRM_TIMEOUT`、起点は依頼の
//! 開始、`Instant` で測る）が来たら、結果を届ける前に固定の希望を直前の値へ戻す依頼を出し、失敗で
//! 確定する（契約表 2・3）。要求が無いときは `READ_INTERVAL` ごと、またはビューポートの大きさが
//! 変わったときに読み戻す間隔つき pull で、次の読み取りまでの再描画を必ず予約する（契約表 4）。
//! 読み取りに失敗・非対応のときは予約しない（失敗状態で再描画し続けない）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象。Web は本経路を通らず
//! `fandhe-web-dom`（#658）が `screen.orientation` へ直接配線する。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。iOS は `ios_screen_orientation.rs`（#656）、
//! Android は `android_screen_orientation.rs`（#657）が返す。macOS は `n-a`、Windows は
//! `deferred` のため `UnsupportedScreenOrientation` 固定。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::screen_orientation_show::{
    apply_read, clear_failure_reason, finish, lock_satisfied, mark_applying, take_request,
    wants_read, Orientation as CoreOrientation, OrientationFailure, Reading,
    ScreenOrientationShowState, NOT_LOCKED_REASON, NOT_UNLOCKED_REASON,
};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::screen_orientation::UnsupportedScreenOrientation;
use fandhe_device::screen_orientation::{
    Orientation, ScreenOrientation, CONFIRM_TIMEOUT, READ_INTERVAL,
};
use fandhe_device::DeviceError;

use super::request_slot;
use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `ScreenOrientation` 実装を返す（プロセスで 1 個）。
/// 全 OS の最終形（後続の子は触らない）。
fn implementation() -> Arc<dyn ScreenOrientation> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn ScreenOrientation>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                super::ios_screen_orientation::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_screen_orientation::implementation()
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedScreenOrientation)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。`Unsupported` 以外（契約外の `PermissionDenied` /
/// `Cancelled` / `InvalidInput` を含む）は固定文言に畳み、OS のエラーコードを画面・ログへ出さない
/// （security.md A09）。
fn to_failure(e: DeviceError) -> OrientationFailure {
    match e {
        DeviceError::Unsupported(_) => OrientationFailure::Unsupported,
        _ => OrientationFailure::Failed("画面の向きの操作".to_string()),
    }
}

fn to_core(o: Orientation) -> CoreOrientation {
    match o {
        Orientation::Portrait => CoreOrientation::Portrait,
        Orientation::Landscape => CoreOrientation::Landscape,
    }
}

fn from_core(o: CoreOrientation) -> Orientation {
    match o {
        CoreOrientation::Portrait => Orientation::Portrait,
        CoreOrientation::Landscape => Orientation::Landscape,
    }
}

/// 復元に失敗したとき利用者へ見せる理由。
const RESTORE_FAILED_REASON: &str = "画面の向きの固定を元に戻せませんでした。再試行します。";

fn read_core(imp: &dyn ScreenOrientation) -> Result<Reading, OrientationFailure> {
    imp.read()
        .map(|r| Reading {
            orientation: to_core(r.orientation),
            angle: r.rotation.degrees(),
            lock: r.lock.map(to_core),
            can_lock: imp.can_lock(),
        })
        .map_err(to_failure)
}

/// 確認が確定した結果（`finish` へ渡す値）。`RequestSlot` に置いて `poll_and_finish` で画面へ返す。
#[derive(Clone)]
struct Outcome {
    generation: u64,
    result: Result<(), OrientationFailure>,
    reading: Result<Reading, OrientationFailure>,
}

/// 要求の追跡（採番・結果・完了処理）は共通部品 `RequestSlot` / `poll_and_finish` に任せる。
/// 本機能は OS が結果を push しないため、毎フレームの読み戻しが目標に一致するか期限が来た時点で
/// 自分で `deliver` する。
static SLOT: request_slot::RequestSlot<Outcome> = request_slot::RequestSlot::new();

/// 読み戻しと期限判定に要る、固定固有の確認条件（`SLOT` の要求 id に紐づく）。
struct Watch {
    request_id: u64,
    generation: u64,
    target: Option<Orientation>,
    /// 期限切れのとき固定の希望を戻す先（依頼の直前に読んだ `lock`）。
    previous: Option<Orientation>,
    started: Instant,
}

/// ドライバの状態。確認待ち・最終読取の時刻・前回のビューポートを 1 つのロックにまとめ、判定と
/// 更新を同じロックの内側で行う（契約表 2）。
#[derive(Default)]
struct Driver {
    watch: Option<Watch>,
    last_read: Option<Instant>,
    last_viewport: Option<(f32, f32)>,
    /// 期限切れの復元（`request_lock`）に失敗し、OS に希望が残っている固定の戻し先。
    /// 成功するまで毎フレーム再試行し、残っている間は新しい固定要求を受け付けない。
    pending_restore: Option<Option<Orientation>>,
}

// ponytail: 1 プロセス 1 ウィンドウ前提の static。複数 Context を扱うなら `ctx.data` へ移す。
// 読み書きは UI スレッドからだけ。
static DRIVER: Mutex<Driver> = Mutex::new(Driver {
    watch: None,
    last_read: None,
    last_viewport: None,
    pending_restore: None,
});

/// 復元に失敗して OS の希望状態が残っていることを伝える失敗（利用者へ見せる）。
fn failed_restore() -> OrientationFailure {
    OrientationFailure::Failed(RESTORE_FAILED_REASON.to_string())
}

fn failed_not_satisfied(target: Option<Orientation>) -> OrientationFailure {
    let reason = if target.is_some() {
        NOT_LOCKED_REASON
    } else {
        NOT_UNLOCKED_REASON
    };
    OrientationFailure::Failed(reason.to_string())
}

/// 固定の希望を `prev` へ戻す依頼を出し、読み戻しで OS 側の希望が `prev` になったことまで確認する。
/// 依頼の `Ok` は受理しただけなので成功とみなさない。読み取り失敗・不一致は未復元（`false`）。
fn restore_lock(
    imp: &dyn ScreenOrientation,
    prev: Option<Orientation>,
) -> (bool, Result<Reading, OrientationFailure>) {
    let requested = imp.request_lock(prev).is_ok();
    let reading = read_core(imp);
    let confirmed = requested && reading.as_ref().is_ok_and(|r| r.lock == prev.map(to_core));
    (confirmed, reading)
}

/// 1 フレーム分の処理（テストから実装・時刻・画面サイズを差し込めるよう切り出す）。
fn drive(
    state: &ScreenOrientationShowState,
    sender: &CompletionSender<ScreenOrientationShowState>,
    imp: &dyn ScreenOrientation,
    now: Instant,
    viewport: (f32, f32),
    d: &mut Driver,
) {
    // 未復元の固定があれば、新しい要求より先に戻しを再試行する（成功するまで保持する）。
    if let Some(prev) = d.pending_restore {
        let (confirmed, reading) = restore_lock(imp, prev);
        if confirmed {
            d.pending_restore = None;
            // 読み戻しで復元を確認できたので「元に戻せません」の表示を消し、読み戻した値を反映する。
            let _ = sender.send(Box::new(move |s: &mut ScreenOrientationShowState| {
                clear_failure_reason(s, RESTORE_FAILED_REASON);
                if let Ok(r) = reading {
                    apply_read(s, Ok(r));
                }
            }));
            sender.request_repaint_after(Duration::ZERO);
        } else {
            sender.request_repaint_after(request_slot::POLL_INTERVAL);
        }
    }

    if let Some(req) = take_request(state) {
        // 直前の `mark_applying` がまだ画面へ反映されていないフレームで、同じ要求を OS へ二重に渡さない。
        if d.watch
            .as_ref()
            .is_some_and(|w| w.generation == req.generation)
        {
            sender.request_repaint_after(request_slot::POLL_INTERVAL);
            return;
        }
        start_request(
            sender,
            imp,
            now,
            req.generation,
            req.target.map(from_core),
            d,
        );
        return;
    }

    if let Some(w) = d.watch.as_ref() {
        if !state.applying() || state.generation() != w.generation {
            // 古い世代・完了済みの確認待ちは捨てる（結果は画面へ返さない）。
            d.watch = None;
            SLOT.reset();
        } else {
            if let Some(unrestored) = watch_step(imp, now, w) {
                d.pending_restore = Some(unrestored);
            }
            if SLOT.snapshot().is_some_and(|(_, o)| o.is_some()) {
                d.watch = None;
            }
        }
    }

    // 確認待ちの再描画予約と、確定結果の画面反映（共通部品）。
    request_slot::poll_and_finish(
        &SLOT,
        sender,
        |_| {},
        |s: &mut ScreenOrientationShowState, o: Outcome| {
            finish(s, o.generation, o.result, o.reading);
            true
        },
    );
    if SLOT.snapshot().is_some() {
        return;
    }

    pull(state, sender, imp, now, viewport, d);
}

/// 要求 1 件を処理する。期限の起点 `now` は OS を呼ぶ前に取る。
fn start_request(
    sender: &CompletionSender<ScreenOrientationShowState>,
    imp: &dyn ScreenOrientation,
    now: Instant,
    generation: u64,
    target: Option<Orientation>,
    d: &mut Driver,
) {
    d.watch = None;
    SLOT.reset();
    d.last_read = None;
    let _ = sender.send(Box::new(move |s: &mut ScreenOrientationShowState| {
        mark_applying(s, generation)
    }));

    let before = read_core(imp);
    let core_target = target.map(to_core);
    let send_finish = |result: Result<(), OrientationFailure>,
                       reading: Result<Reading, OrientationFailure>| {
        let _ = sender.send(Box::new(move |s: &mut ScreenOrientationShowState| {
            finish(s, generation, result, reading);
        }));
    };

    // 未復元の固定が残っているうちは、満たされて見えても成功にしない
    // （残留ロックは復元で打ち消されるため）。OS の状態も重ねて変えない。
    if d.pending_restore.is_some() {
        send_finish(Err(failed_restore()), before);
        return;
    }
    // 同じ固定の再依頼は OS を呼ばずに成功にする（読み戻しで既に満たされているときだけ）。
    if before
        .as_ref()
        .is_ok_and(|r| lock_satisfied(core_target, r))
    {
        send_finish(Ok(()), before);
        return;
    }
    if !imp.can_lock() {
        send_finish(Err(OrientationFailure::Unsupported), before);
        return;
    }
    // 直前値が読めないと、期限切れで戻す先が分からず元の固定を失う。OS を変えず読み取り失敗で終える。
    let previous = match before.as_ref() {
        Ok(r) => r.lock.map(from_core),
        Err(e) => {
            let failure = e.clone();
            send_finish(Err(failure), before);
            return;
        }
    };
    match imp.request_lock(target) {
        Err(e) => {
            let failure = to_failure(e);
            send_finish(Err(failure), read_core(imp));
        }
        Ok(()) => {
            d.watch = Some(Watch {
                request_id: SLOT.begin(),
                generation,
                target,
                previous,
                started: now,
            });
            sender.request_repaint_after(request_slot::POLL_INTERVAL);
        }
    }
}

/// 確認待ちを 1 歩進める。期限判定を一致判定より先に行う（期限後に一致しても成功にしない）。
///
/// 期限切れで戻しに失敗したときは、未復元の戻し先を `Some` で返す（呼び出し側が保持して再試行する）。
fn watch_step(imp: &dyn ScreenOrientation, now: Instant, w: &Watch) -> Option<Option<Orientation>> {
    let core_target = w.target.map(to_core);
    if now.saturating_duration_since(w.started) >= CONFIRM_TIMEOUT {
        // 結果を届ける前に、固定の希望を直前の値へ戻す。失敗は握りつぶさず利用者へ伝える。
        let (restored, reading) = restore_lock(imp, w.previous);
        SLOT.deliver(
            w.request_id,
            Outcome {
                generation: w.generation,
                result: Err(if restored {
                    failed_not_satisfied(w.target)
                } else {
                    failed_restore()
                }),
                reading,
            },
        );
        return (!restored).then_some(w.previous);
    }
    let reading = read_core(imp);
    if reading
        .as_ref()
        .is_ok_and(|r| lock_satisfied(core_target, r))
    {
        SLOT.deliver(
            w.request_id,
            Outcome {
                generation: w.generation,
                result: Ok(()),
                reading,
            },
        );
    }
    // 読み取りが失敗している間、または未一致の間は期限まで待つ。
    None
}

/// 要求が無いときの間隔つき pull。
fn pull(
    state: &ScreenOrientationShowState,
    sender: &CompletionSender<ScreenOrientationShowState>,
    imp: &dyn ScreenOrientation,
    now: Instant,
    viewport: (f32, f32),
    d: &mut Driver,
) {
    if !wants_read(state) || state.busy() {
        return;
    }
    let since = d.last_read.map(|prev| now.saturating_duration_since(prev));
    let viewport_changed = d.last_viewport != Some(viewport);
    match since {
        Some(elapsed) if elapsed < READ_INTERVAL && !viewport_changed => {
            // 読まない分岐でも次回の読み取りを予約する（契約表 4）。
            sender.request_repaint_after(READ_INTERVAL - elapsed);
        }
        _ => {
            let result = read_core(imp);
            let keep_polling = result.is_ok();
            // 失敗時は間隔ゲートを残さず、利用者の明示 reload を即時に読めるようにする。
            d.last_read = keep_polling.then_some(now);
            d.last_viewport = keep_polling.then_some(viewport);
            // 状態が変わらないなら送らない（毎秒の無駄な反映を避ける）。
            let mut next = state.clone();
            apply_read(&mut next, result.clone());
            if next != *state {
                let _ = sender.send(Box::new(move |s: &mut ScreenOrientationShowState| {
                    apply_read(s, result);
                }));
            }
            sender.request_repaint_after(if keep_polling {
                READ_INTERVAL
            } else {
                Duration::ZERO
            });
        }
    }
}

/// `registry.rs` の `effect` 区画から `screen_orientation_show` 画面用に配線される effect ドライバ。
pub fn screen_orientation_show_effect(
    state: &ScreenOrientationShowState,
    sender: &CompletionSender<ScreenOrientationShowState>,
) {
    let mut driver = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    drive(
        state,
        sender,
        implementation().as_ref(),
        Instant::now(),
        sender.viewport_size(),
        &mut driver,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::screen_orientation_show::{
        error_text, lock_landscape, lock_portrait, lock_text, unlock, ReadState, FAILED_PREFIX,
        LOCKED_LANDSCAPE_STATUS, LOCKED_PORTRAIT_STATUS, NOT_LOCKED_STATUS,
    };
    use fandhe_device::screen_orientation::{OrientationReading, Rotation};
    use std::sync::mpsc::Receiver;

    /// `SLOT` が static のため、テストは直列化してから初期化する。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        SLOT.reset();
        g
    }

    type Rx = Receiver<crate::app::Completion<ScreenOrientationShowState>>;
    const VP: (f32, f32) = (400.0, 800.0);

    fn sender_with_channel() -> (CompletionSender<ScreenOrientationShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    fn apply_all(rx: &Rx, s: &mut ScreenOrientationShowState) {
        while let Ok(c) = rx.try_recv() {
            c(s);
        }
    }

    /// 呼び出しを記録し、挙動を差し込める Fake。
    struct Fake {
        orientation: Mutex<Orientation>,
        lock: Mutex<Option<Orientation>>,
        /// 真なら固定の依頼で実際の向きも変わる。偽なら希望だけが変わる（嘘の成功の再現）。
        follow: bool,
        can_lock: bool,
        fail_request: Mutex<Option<DeviceError>>,
        fail_read: Mutex<bool>,
        /// 真なら解除（`None`）の依頼を `Ok` で受けるが、OS の希望は変えない（受理しただけの再現）。
        ignore_unlock: Mutex<bool>,
        calls: Mutex<Vec<Option<Orientation>>>,
        reads: Mutex<usize>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                orientation: Mutex::new(Orientation::Portrait),
                lock: Mutex::new(None),
                follow: true,
                can_lock: true,
                fail_request: Mutex::new(None),
                fail_read: Mutex::new(false),
                ignore_unlock: Mutex::new(false),
                calls: Mutex::new(Vec::new()),
                reads: Mutex::new(0),
            }
        }

        fn calls(&self) -> Vec<Option<Orientation>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ScreenOrientation for Fake {
        fn read(&self) -> Result<OrientationReading, DeviceError> {
            *self.reads.lock().unwrap() += 1;
            if *self.fail_read.lock().unwrap() {
                return Err(DeviceError::Internal("x"));
            }
            Ok(OrientationReading {
                orientation: *self.orientation.lock().unwrap(),
                rotation: Rotation::Deg0,
                lock: *self.lock.lock().unwrap(),
            })
        }
        fn request_lock(&self, target: Option<Orientation>) -> Result<(), DeviceError> {
            self.calls.lock().unwrap().push(target);
            if let Some(e) = self.fail_request.lock().unwrap().take() {
                return Err(e);
            }
            if target.is_none() && *self.ignore_unlock.lock().unwrap() {
                return Ok(());
            }
            *self.lock.lock().unwrap() = target;
            if self.follow {
                if let Some(t) = target {
                    *self.orientation.lock().unwrap() = t;
                }
            }
            Ok(())
        }
        fn can_lock(&self) -> bool {
            self.can_lock
        }
    }

    /// 読み取り済み（解除、縦）の状態にして返す。
    fn ready(
        tx: &CompletionSender<ScreenOrientationShowState>,
        rx: &Rx,
        fake: &Fake,
        d: &mut Driver,
    ) -> ScreenOrientationShowState {
        let mut s = ScreenOrientationShowState::default();
        drive(&s, tx, fake, Instant::now(), VP, d);
        apply_all(rx, &mut s);
        assert!(matches!(s.read_state(), ReadState::Read(_)));
        s
    }

    #[test]
    fn no_request_and_no_change_sends_nothing() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let t0 = Instant::now();
        let mut s = ScreenOrientationShowState::default();
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + READ_INTERVAL, VP, &mut d);
        assert!(rx.try_recv().is_err());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn lock_and_unlock_round_trip() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(100), VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(lock_text(&s), LOCKED_LANDSCAPE_STATUS);
        assert!(error_text(&s).is_empty());

        unlock(&mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(200), VP, &mut d);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(300), VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
        assert_eq!(fake.calls(), vec![Some(Orientation::Landscape), None]);
    }

    #[test]
    fn lie_ok_is_not_confirmed_before_deadline_and_fails_after() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        fake.follow = false;
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_secs(1), VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(s.applying(), "期限前は確定しない");
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(!s.applying());
        assert!(error_text(&s).contains(NOT_LOCKED_REASON));
        assert_ne!(lock_text(&s), LOCKED_LANDSCAPE_STATUS);
        // 固定の依頼のあと、直前の値（解除）へ戻す依頼が出ている。
        assert_eq!(fake.calls(), vec![Some(Orientation::Landscape), None]);
    }

    #[test]
    fn read_failure_before_request_changes_nothing() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        *fake.fail_read.lock().unwrap() = true;
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, Instant::now(), VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(!s.applying());
        assert!(d.watch.is_none());
        assert!(fake.calls().is_empty(), "OS の固定を変えない");
    }

    #[test]
    fn restore_failure_is_reported_and_retried() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        fake.follow = false;
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        *fake.fail_request.lock().unwrap() = Some(DeviceError::Internal("x"));
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(!s.applying());
        assert!(error_text(&s).contains(RESTORE_FAILED_REASON));
        assert_eq!(d.pending_restore, Some(None));
        // 次のフレームで再試行し、成功すれば保持を解く。
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        assert_eq!(d.pending_restore, None);
        assert_eq!(fake.calls().last(), Some(&None));
        // 復元に成功したら、画面の「元に戻せません」表示も消える。
        apply_all(&rx, &mut s);
        assert!(!error_text(&s).contains(RESTORE_FAILED_REASON));
    }

    #[test]
    fn restore_ok_but_not_read_back_stays_pending() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        fake.follow = false;
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        // 復元依頼は Ok を返すが、読み戻すと元の固定が残っている。
        *fake.ignore_unlock.lock().unwrap() = true;
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(error_text(&s).contains(RESTORE_FAILED_REASON));
        assert_eq!(d.pending_restore, Some(None), "初回の復元でも保持する");
        // 再試行も Ok だが読み戻しで確認できない間は保持し、表示も消さない。
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(d.pending_restore, Some(None));
        assert!(error_text(&s).contains(RESTORE_FAILED_REASON));
        // 読み戻しが失敗しても握りつぶさず保持する。
        *fake.ignore_unlock.lock().unwrap() = false;
        *fake.fail_read.lock().unwrap() = true;
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        assert_eq!(d.pending_restore, Some(None));
        // 読み戻しで復元を確認できたら解く。
        *fake.fail_read.lock().unwrap() = false;
        drive(&s, &tx, &fake, t0 + CONFIRM_TIMEOUT, VP, &mut d);
        assert_eq!(d.pending_restore, None);
    }

    #[test]
    fn satisfied_lock_is_not_success_while_restore_pending() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        // OS に横固定が残っている状態で、復元が未完了。
        *fake.orientation.lock().unwrap() = Orientation::Landscape;
        *fake.lock.lock().unwrap() = Some(Orientation::Landscape);
        d.pending_restore = Some(None);
        *fake.fail_request.lock().unwrap() = Some(DeviceError::Internal("x"));
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, Instant::now(), VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(error_text(&s).contains(RESTORE_FAILED_REASON));
        assert_eq!(d.pending_restore, Some(None));
    }

    #[test]
    fn match_after_deadline_still_fails() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        // 一致しているが、確認が期限後になった。
        drive(
            &s,
            &tx,
            &fake,
            t0 + CONFIRM_TIMEOUT + Duration::from_secs(1),
            VP,
            &mut d,
        );
        apply_all(&rx, &mut s);
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        assert_ne!(lock_text(&s), LOCKED_LANDSCAPE_STATUS);
        assert_eq!(fake.calls().last(), Some(&None));
    }

    #[test]
    fn same_lock_again_does_not_call_os() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_portrait(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(100), VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(lock_text(&s), LOCKED_PORTRAIT_STATUS);
        assert_eq!(fake.calls().len(), 1);
        lock_portrait(&mut s);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(200), VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(fake.calls().len(), 1, "同じ固定の再依頼は OS を呼ばない");
        assert_eq!(lock_text(&s), LOCKED_PORTRAIT_STATUS);
        assert!(error_text(&s).is_empty());
    }

    #[test]
    fn request_error_fails_and_sets_no_watch() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        *fake.fail_request.lock().unwrap() = Some(DeviceError::Internal("x"));
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, Instant::now(), VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(d.watch.is_none());
        assert!(SLOT.snapshot().is_none());
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
        assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
    }

    #[test]
    fn contract_violating_errors_never_succeed() {
        for e in [
            DeviceError::PermissionDenied("x"),
            DeviceError::Cancelled("x"),
            DeviceError::InvalidInput("x"),
        ] {
            let _g = guard();
            let (tx, rx) = sender_with_channel();
            let fake = Fake::new();
            let mut d = Driver::default();
            let mut s = ready(&tx, &rx, &fake, &mut d);
            *fake.fail_request.lock().unwrap() = Some(e);
            lock_landscape(&mut s);
            drive(&s, &tx, &fake, Instant::now(), VP, &mut d);
            apply_all(&rx, &mut s);
            assert!(error_text(&s).starts_with(FAILED_PREFIX));
            assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
        }
    }

    #[test]
    fn stale_watch_is_dropped_without_finish() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        fake.follow = false;
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        let t0 = Instant::now();
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        assert!(d.watch.is_some());
        // 画面側の状態が差し替わり、処理中でなくなったあとの確認待ちは捨てる（結果は返さない）。
        let fresh = ScreenOrientationShowState::default();
        drive(&fresh, &tx, &fake, t0 + CONFIRM_TIMEOUT * 2, VP, &mut d);
        assert!(d.watch.is_none());
        assert!(SLOT.snapshot().is_none());
        // 期限を過ぎていても、捨てた確認待ちから固定の戻し依頼は出ない。
        assert_eq!(fake.calls(), vec![Some(Orientation::Landscape)]);
    }

    #[test]
    fn unsupported_implementation_never_succeeds() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut s = ScreenOrientationShowState::default();
        let mut d = Driver::default();
        let stub = fandhe_device::screen_orientation::UnsupportedScreenOrientation;
        drive(&s, &tx, &stub, Instant::now(), VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(s.read_state(), &ReadState::Unsupported);
        assert!(!wants_read(&s));
    }

    #[test]
    fn cannot_lock_makes_no_request() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let mut fake = Fake::new();
        fake.can_lock = false;
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        // 画面側が can_lock = false のときは要求自体が立たず、OS も呼ばれない。
        lock_portrait(&mut s);
        unlock(&mut s);
        assert!(take_request(&s).is_none());
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn runtime_unsupported_lock_keeps_reading_and_hides_buttons() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let mut s = ready(&tx, &rx, &fake, &mut d);
        // Safari のように、固定できると読めたのに実行時 Unsupported を返す環境。
        *fake.fail_request.lock().unwrap() = Some(DeviceError::Unsupported("x"));
        lock_landscape(&mut s);
        drive(&s, &tx, &fake, Instant::now(), VP, &mut d);
        apply_all(&rx, &mut s);
        let ReadState::Read(r) = s.read_state() else {
            panic!("読み取りが残っていない");
        };
        assert!(!r.can_lock);
        assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
        assert!(error_text(&s).is_empty());
        lock_portrait(&mut s);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn failed_read_does_not_succeed_or_reschedule() {
        let _g = guard();
        let (tx, rx) = sender_with_channel();
        let fake = Fake::new();
        *fake.fail_read.lock().unwrap() = true;
        let mut d = Driver::default();
        let mut s = ScreenOrientationShowState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, VP, &mut d);
        apply_all(&rx, &mut s);
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(d.last_read.is_none());
        drive(&s, &tx, &fake, t0 + READ_INTERVAL * 3, VP, &mut d);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn pull_respects_interval_and_viewport_change() {
        let _g = guard();
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::new();
        let mut d = Driver::default();
        let s = ScreenOrientationShowState::default();
        let t0 = Instant::now();
        drive(&s, &tx, &fake, t0, VP, &mut d);
        drive(&s, &tx, &fake, t0 + Duration::from_millis(400), VP, &mut d);
        assert_eq!(*fake.reads.lock().unwrap(), 1);
        // 回転で画面サイズが変わると、間隔内でも即時に読む。
        drive(
            &s,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            (800.0, 400.0),
            &mut d,
        );
        assert_eq!(*fake.reads.lock().unwrap(), 2);
        drive(
            &s,
            &tx,
            &fake,
            t0 + Duration::from_millis(600),
            (800.0, 400.0),
            &mut d,
        );
        assert_eq!(*fake.reads.lock().unwrap(), 2);
        drive(
            &s,
            &tx,
            &fake,
            t0 + Duration::from_millis(1500),
            (800.0, 400.0),
            &mut d,
        );
        assert_eq!(*fake.reads.lock().unwrap(), 3);
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn fallback_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.read().is_err());
        assert!(imp.request_lock(Some(Orientation::Portrait)).is_err());
        assert!(imp.request_lock(None).is_err());
        assert!(!imp.can_lock());
    }
}
