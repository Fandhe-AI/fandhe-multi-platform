//! Phase 11 の拡充ループ（親 #141、Issue #723）「折りたたみの姿勢」の effect ドライバ。
//! `fandhe_device::foldable_posture::FoldablePostureSource` を一定間隔で読み、結果を
//! `fandhe_core::screens::foldable_posture_show`（端末依存ゼロ）へ注入する
//! （`registry.rs` の `effect` 区画から毎フレーム同期に呼ばれる）。
//!
//! **間隔つき pull**（`safe_area_insets.rs` と同型）: 最終読取から `READ_INTERVAL` 未満なら
//! 読まない。状態が変わらなければ送信を省くが、次の読み取りまでの再描画は予約する
//! （入力が無いと egui が描画せずポーリングが止まる、契約表 4）。ただし直近の結果が
//! `Unsupported` のときは予約しない（ネイティブの非対応はプロセス中に変わらず、
//! 失敗状態のまま再描画し続けないため）。
//!
//! dispatch（[`implementation`]）は**最終形**。ネイティブは全 OS が対応表で保留・未評価・
//! 対象外・できないのいずれかなので `UnsupportedFoldablePosture` に固定する。将来ネイティブを
//! 足すときは `implementation()` に cfg の分岐を足す。Web は `fandhe-web-dom` が担う。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::foldable_posture_show::{
    self as core_screen, apply_read, FoldablePostureShowState, ReadFailure, FAILED_LABEL,
};
use fandhe_device::foldable_posture::{
    self as dev, FoldablePostureSource, UnsupportedFoldablePosture, READ_INTERVAL,
};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた実装を返す。全ネイティブ OS で非対応。
fn implementation() -> Arc<dyn FoldablePostureSource> {
    Arc::new(UnsupportedFoldablePosture)
}

fn map_posture(p: dev::Posture) -> core_screen::Posture {
    match p {
        dev::Posture::Flat => core_screen::Posture::Flat,
        dev::Posture::Folded => core_screen::Posture::Folded,
    }
}

fn map_fold(f: dev::FoldInfo) -> core_screen::FoldInfo {
    match f {
        dev::FoldInfo::Unknown => core_screen::FoldInfo::Unknown,
        dev::FoldInfo::Known(f) => core_screen::FoldInfo::Known(core_screen::Fold {
            orientation: match f.orientation {
                dev::FoldOrientation::Vertical => core_screen::FoldOrientation::Vertical,
                dev::FoldOrientation::Horizontal => core_screen::FoldOrientation::Horizontal,
            },
            start: f.start,
            thickness: f.thickness,
            extent: f.extent,
        }),
    }
}

/// 契約の結果を画面の入力へ写す。`Unsupported` 以外の失敗は固定文言へ畳み、
/// OS 固有値を画面へ漏らさない（security.md A09）。
fn to_result(
    r: Result<dev::PostureReading, DeviceError>,
) -> Result<core_screen::PostureReading, ReadFailure> {
    match r {
        Ok(r) => Ok(core_screen::PostureReading {
            posture: map_posture(r.posture),
            fold: map_fold(r.fold),
        }),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed(FAILED_LABEL.to_string())),
    }
}

/// 最終読取の時刻と直近の結果が非対応だったか（プロセス全体で 1 つ）。
#[derive(Default)]
struct Timing {
    last_read: Option<Instant>,
    unsupported: bool,
}

static TIMING: Mutex<Timing> = Mutex::new(Timing {
    last_read: None,
    unsupported: false,
});

/// 間隔判定・読取・送信省略を行う本体（テストから時刻と実装を差し込めるよう切り出す）。
/// 次に読み直すまでの待ち時間を返す（予約しないときは `None`）。
fn drive(
    state: &FoldablePostureShowState,
    sender: &CompletionSender<FoldablePostureShowState>,
    src: &dyn FoldablePostureSource,
    now: Instant,
    timing: &mut Timing,
) -> Option<Duration> {
    let due = timing
        .last_read
        .is_none_or(|prev| now.saturating_duration_since(prev) >= READ_INTERVAL);
    let result = if due {
        timing.last_read = Some(now);
        let r = to_result(src.read());
        timing.unsupported = matches!(r, Err(ReadFailure::Unsupported));
        Some(r)
    } else {
        None
    };
    // 送信を省く分岐でも次回の読み取りを予約する（契約表 4）。非対応は変わらないので予約しない。
    let wait = if timing.unsupported {
        None
    } else {
        let since_read = timing
            .last_read
            .map_or(Duration::ZERO, |p| now.saturating_duration_since(p));
        Some(READ_INTERVAL.saturating_sub(since_read))
    };

    if let Some(result) = result {
        let mut next = state.clone();
        apply_read(&mut next, result.clone());
        // 同じ状態のまま毎フレーム送らない。
        if next != *state {
            let _ = sender.send(Box::new(move |s: &mut FoldablePostureShowState| {
                apply_read(s, result.clone())
            }));
        }
    }
    wait
}

/// `registry.rs` の `effect` 区画から `foldable_posture_show` 画面用に配線される effect ドライバ。
pub fn foldable_posture_show_effect(
    state: &FoldablePostureShowState,
    sender: &CompletionSender<FoldablePostureShowState>,
) {
    let wait = {
        let mut timing = TIMING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drive(
            state,
            sender,
            implementation().as_ref(),
            Instant::now(),
            &mut timing,
        )
    };
    if let Some(wait) = wait {
        sender.request_repaint_after(wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_screen::FoldablePostureView as View;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<FoldablePostureShowState>>;

    fn sender_with_channel() -> (CompletionSender<FoldablePostureShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<dev::PostureReading, DeviceError>,
        reads: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<dev::PostureReading, DeviceError>) -> Self {
            Self {
                result,
                reads: AtomicUsize::new(0),
            }
        }
        fn reading(posture: dev::Posture) -> Self {
            Self::new(Ok(dev::PostureReading {
                posture,
                fold: dev::FoldInfo::Unknown,
            }))
        }
    }

    impl FoldablePostureSource for Fake {
        fn read(&self) -> Result<dev::PostureReading, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match &self.result {
                Ok(r) => Ok(*r),
                Err(DeviceError::Unsupported(n)) => Err(DeviceError::Unsupported(n)),
                Err(DeviceError::Cancelled(n)) => Err(DeviceError::Cancelled(n)),
                Err(DeviceError::PermissionDenied(n)) => Err(DeviceError::PermissionDenied(n)),
                Err(_) => Err(DeviceError::Internal("x")),
            }
        }
    }

    fn apply_all(rx: &Rx, s: &mut FoldablePostureShowState) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(s);
            n += 1;
        }
        n
    }

    #[test]
    fn skips_read_within_interval() {
        let (tx, _rx) = sender_with_channel();
        let fake = Fake::reading(dev::Posture::Flat);
        let state = FoldablePostureShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(&state, &tx, &fake, t0, &mut timing);
        drive(
            &state,
            &tx,
            &fake,
            t0 + Duration::from_millis(500),
            &mut timing,
        );
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        drive(&state, &tx, &fake, t0 + READ_INTERVAL, &mut timing);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn same_value_is_not_sent_but_change_is_and_next_read_is_scheduled() {
        let (tx, rx) = sender_with_channel();
        let mut state = FoldablePostureShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        let w = drive(
            &state,
            &tx,
            &Fake::reading(dev::Posture::Flat),
            t0,
            &mut timing,
        );
        assert_eq!(w, Some(READ_INTERVAL));
        assert_eq!(apply_all(&rx, &mut state), 1);
        let w = drive(
            &state,
            &tx,
            &Fake::reading(dev::Posture::Flat),
            t0 + READ_INTERVAL,
            &mut timing,
        );
        assert!(rx.try_recv().is_err());
        assert!(w.is_some());
        drive(
            &state,
            &tx,
            &Fake::reading(dev::Posture::Folded),
            t0 + READ_INTERVAL * 2,
            &mut timing,
        );
        assert_eq!(apply_all(&rx, &mut state), 1);
        assert!(matches!(
            state.view,
            View::Read(r) if r.posture == core_screen::Posture::Folded
        ));
    }

    #[test]
    fn failures_never_become_read() {
        for e in [
            DeviceError::Unsupported("foldable_posture"),
            DeviceError::Internal("x"),
            DeviceError::Cancelled("x"),
            DeviceError::PermissionDenied("x"),
        ] {
            let (tx, rx) = sender_with_channel();
            let mut state = FoldablePostureShowState::default();
            drive(
                &state,
                &tx,
                &Fake::new(Err(e)),
                Instant::now(),
                &mut Timing::default(),
            );
            apply_all(&rx, &mut state);
            assert!(matches!(state.view, View::Unsupported | View::Failed(_)));
        }
    }

    #[test]
    fn previous_value_is_not_kept_after_failure() {
        let (tx, rx) = sender_with_channel();
        let mut state = FoldablePostureShowState::default();
        let t0 = Instant::now();
        let mut timing = Timing::default();
        drive(
            &state,
            &tx,
            &Fake::reading(dev::Posture::Folded),
            t0,
            &mut timing,
        );
        apply_all(&rx, &mut state);
        let failing = Fake::new(Err(DeviceError::Internal("x")));
        drive(&state, &tx, &failing, t0 + READ_INTERVAL, &mut timing);
        apply_all(&rx, &mut state);
        assert!(!matches!(state.view, View::Read(_)));
    }

    #[test]
    fn unsupported_impl_reports_unsupported_and_does_not_reschedule() {
        let (tx, rx) = sender_with_channel();
        let mut state = FoldablePostureShowState::default();
        let w = drive(
            &state,
            &tx,
            &UnsupportedFoldablePosture,
            Instant::now(),
            &mut Timing::default(),
        );
        apply_all(&rx, &mut state);
        assert_eq!(state.view, View::Unsupported);
        assert_eq!(w, None);
    }

    #[test]
    fn internal_failure_reschedules() {
        let (tx, _rx) = sender_with_channel();
        let state = FoldablePostureShowState::default();
        let w = drive(
            &state,
            &tx,
            &Fake::new(Err(DeviceError::Internal("x"))),
            Instant::now(),
            &mut Timing::default(),
        );
        assert!(w.is_some());
    }

    #[test]
    fn known_fold_is_mapped() {
        let r = dev::PostureReading {
            posture: dev::Posture::Folded,
            fold: dev::FoldInfo::Known(dev::Fold {
                orientation: dev::FoldOrientation::Horizontal,
                start: 1.0,
                thickness: 2.0,
                extent: 3.0,
            }),
        };
        let out = to_result(Ok(r)).unwrap();
        assert_eq!(out.posture, core_screen::Posture::Folded);
        assert!(matches!(
            out.fold,
            core_screen::FoldInfo::Known(f)
                if f.orientation == core_screen::FoldOrientation::Horizontal
                    && f.start == 1.0 && f.thickness == 2.0 && f.extent == 3.0
        ));
    }
}
