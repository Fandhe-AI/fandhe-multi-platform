//! Phase 11 の拡充ループ（親 #95、Issue #355）「ペン・筆圧入力」の effect ドライバ。
//! `fandhe_core::screens::pen_draw`（端末依存ゼロ）のキャンバス上のポインタ位置を egui から取り
//! （`CompletionSender::canvas_pointer`）、押下中だけ `fandhe_device::pen_stylus::PenStylus::latest`
//! で種類・筆圧・傾きを pull して結果を注入する（`registry.rs` の `effect` 区画から毎フレーム同期に
//! 呼ばれる）。位置は egui、種類・筆圧・傾きは OS 層、という分担は winit / egui-winit がそれらを
//! 渡さないため。
//!
//! **押下中の pull**: 押下中は `request_repaint_after` で次フレームを必ず予約する。送信を省く
//! 分岐でも予約する（入力が無いと egui が描画せず、静止したままの筆圧変化を拾えなくなる、契約表 4）。
//! 離したら `end_stroke` を 1 回だけ送る（画面側の表示値の有無で判定するため繰り返さない）。
//! 失敗（非対応・読み取り失敗）は画面の状態が `Failure` になった後は何もしない（ループさせない）。
//!
//! dispatch（[`implementation`]）は全対象 OS の最終形。macOS は `macos_pen_stylus.rs`、iOS は `ios_pen_stylus.rs`（#356）、
//! Android は `android_pen_stylus.rs`（#357）、Windows は `windows_pen_stylus.rs`（#359）、
//! Web は `fandhe-web-dom` 側、Linux は非対応。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fandhe_core::screens::pen_draw::{
    apply_failure, apply_sample, end_stroke, PenDrawState, PenFailureView, PenSampleView,
    PenStatus, PointerKindView,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::pen_stylus::UnsupportedPenStylus;
use fandhe_device::pen_stylus::{PenSample, PenStylus, PointerKind};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 押下中に OS を読み直す間隔（約 60Hz）。
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// 現在のプラットフォームに応じた実装を返す（プロセスで 1 個。OS 側の登録を二重に作らないため
/// `OnceLock` で保持する）。
fn implementation() -> Arc<dyn PenStylus> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn PenStylus>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_pen_stylus::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_pen_stylus::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_pen_stylus::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_pen_stylus::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "windows"
            )))]
            {
                Arc::new(UnsupportedPenStylus)
            }
        })
        .clone()
}

/// 契約の値を画面の入力へ写す。OS 固有の値は画面へ漏らさない（security.md A09）。
fn to_view(s: PenSample) -> PenSampleView {
    PenSampleView {
        kind: match s.kind {
            PointerKind::Pen => PointerKindView::Pen,
            PointerKind::Touch => PointerKindView::Touch,
            PointerKind::Mouse => PointerKindView::Mouse,
            PointerKind::Unknown => PointerKindView::Unknown,
        },
        pressure: s.pressure,
        altitude_deg: s.altitude_deg,
    }
}

fn to_failure(e: &DeviceError) -> PenFailureView {
    match e {
        DeviceError::Unsupported(_) => PenFailureView::Unsupported,
        _ => PenFailureView::Failed,
    }
}

/// 直前に送った入力（同じ入力の送信を省くための記憶）。`Some((位置, None))` は「押下中に OS 側が
/// 接触なしを返し、線を終え済み」を表す（離したときに点を足し直さないため）。
type Last = Option<((f32, f32), Option<PenSample>)>;

static LAST: Mutex<Last> = Mutex::new(None);

/// 本体（テストから実装とポインタ位置を差し込めるよう切り出す）。
fn drive(
    state: &PenDrawState,
    sender: &CompletionSender<PenDrawState>,
    pen: &dyn PenStylus,
    pointer: Option<(f32, f32)>,
    new_contact: bool,
    released: Option<(f32, f32)>,
    last: &mut Last,
) {
    if matches!(state.status, PenStatus::Failure(_)) {
        return;
    }
    let generation = state.generation;
    let Some(pos) = pointer else {
        // 前のフレームまでにこの接触を見ていたか（見ていれば `Some`）と、そのときの値。
        let seen = last.take();
        // 接触前に OS 側の登録（macOS の NSEvent モニタ等、main thread で 1 回）を済ませる。
        // 最初の押下が未登録のまま通過して種類・筆圧を落とさないため。非対応・登録失敗は
        // ここで画面へ出す（押下を待たずに失敗を見せる）。
        let latest = match pen.latest() {
            Ok(sample) => sample,
            Err(e) => {
                let f = to_failure(&e);
                let _ = sender.send(Box::new(move |s: &mut PenDrawState| apply_failure(s, f)));
                return;
            }
        };
        // このフレームで接触が終わった: 最後の 1 点を描いてから線を終える。1 フレームに収まった
        // 短いタップでも点を落とさない。OS 側は離した時点で値を消すため、前のフレームの値を優先し、
        // 無ければ一定の太さで描く（嘘の筆圧は作らない）。
        // ただし押下中に OS 側が先に接触なしを返して線を終え済みなら、点を足さない（離れた点が残る）。
        if let Some(pos) = released {
            if seen.is_none() || state.is_stroking() {
                let view = seen.and_then(|(_, sample)| sample).or(latest).map(to_view);
                let _ = sender.send(Box::new(move |s: &mut PenDrawState| {
                    apply_sample(s, generation, pos, view);
                    end_stroke(s, generation);
                }));
            }
            return;
        }
        if state.is_stroking() {
            let _ = sender.send(Box::new(move |s: &mut PenDrawState| {
                end_stroke(s, generation);
            }));
        }
        return;
    };
    match pen.latest() {
        Err(e) => {
            let f = to_failure(&e);
            let _ = sender.send(Box::new(move |s: &mut PenDrawState| apply_failure(s, f)));
        }
        Ok(None) => {
            // 接触なし（キャンセル含む）は最後の値を出し続けず、ストロークを終える。値は捨てるが、
            // この接触を見たことは残す（離したフレームで終え済みの線に点を足し直さないため）。
            *last = Some((pos, None));
            if state.is_stroking() {
                let _ = sender.send(Box::new(move |s: &mut PenDrawState| {
                    end_stroke(s, generation);
                }));
            }
            sender.request_repaint_after(POLL_INTERVAL);
        }
        Ok(sample) => {
            // 同じフレームに「離す → 押す」が届いた（新しい接触）なら、前のストロークを先に終える。
            // 終えないと前の点から新しい点まで補間され、別の線がつながる。
            if new_contact && state.is_stroking() {
                *last = None;
                let _ = sender.send(Box::new(move |s: &mut PenDrawState| {
                    end_stroke(s, generation);
                }));
            }
            if *last != Some((pos, sample)) {
                *last = Some((pos, sample));
                let view = sample.map(to_view);
                let _ = sender.send(Box::new(move |s: &mut PenDrawState| {
                    apply_sample(s, generation, pos, view);
                }));
            }
            sender.request_repaint_after(POLL_INTERVAL);
        }
    }
}

/// `registry.rs` の `effect` 区画から `pen_draw` 画面用に配線される effect ドライバ。
pub fn pen_draw_effect(state: &PenDrawState, sender: &CompletionSender<PenDrawState>) {
    let pointer = sender.canvas_pointer(fandhe_core::screens::pen_draw::CANVAS_ID);
    let pressed = sender.canvas_pressed(fandhe_core::screens::pen_draw::CANVAS_ID);
    let released = sender.canvas_released(fandhe_core::screens::pen_draw::CANVAS_ID);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    drive(
        state,
        sender,
        implementation().as_ref(),
        pointer,
        pressed,
        released,
        &mut last,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::pen_draw::{clear, PenStatus};
    use fandhe_device::pen_stylus::UnsupportedPenStylus as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<PenDrawState>>;

    fn channel() -> (CompletionSender<PenDrawState>, Rx) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn apply_all(rx: &Rx, s: &mut PenDrawState) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(s);
            n += 1;
        }
        n
    }

    struct Fake {
        out: fn() -> Result<Option<PenSample>, DeviceError>,
        calls: AtomicUsize,
    }

    impl PenStylus for Fake {
        fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.out)()
        }
    }

    fn fake(out: fn() -> Result<Option<PenSample>, DeviceError>) -> Fake {
        Fake {
            out,
            calls: AtomicUsize::new(0),
        }
    }

    fn pen_sample() -> Result<Option<PenSample>, DeviceError> {
        Ok(Some(PenSample::new(
            PointerKind::Pen,
            Some(0.8),
            Some(60.0),
        )))
    }

    #[test]
    fn unsupported_sends_failure_once_and_never_succeeds() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let mut last = None;
        drive(
            &s,
            &tx,
            &Unsupported,
            Some((5.0, 5.0)),
            false,
            None,
            &mut last,
        );
        assert_eq!(apply_all(&rx, &mut s), 1);
        assert_eq!(s.status, PenStatus::Failure(PenFailureView::Unsupported));
        // 失敗後は何も送らない。描画も受け付けない。
        drive(
            &s,
            &tx,
            &Unsupported,
            Some((6.0, 6.0)),
            false,
            None,
            &mut last,
        );
        assert_eq!(apply_all(&rx, &mut s), 0);
        assert_eq!(s.point_count(), 0);
    }

    #[test]
    fn internal_error_becomes_failed() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let f = fake(|| Err(DeviceError::Internal("pen_stylus")));
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut None);
        apply_all(&rx, &mut s);
        assert_eq!(s.status, PenStatus::Failure(PenFailureView::Failed));
    }

    #[test]
    fn sample_is_drawn_and_duplicates_are_skipped() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let f = fake(pen_sample);
        let mut last = None;
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 1);
        assert_eq!(s.point_count(), 1);
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 0);
        drive(&s, &tx, &f, Some((9.0, 5.0)), false, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 1);
    }

    #[test]
    fn release_sends_end_stroke_once() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let f = fake(pen_sample);
        let mut last = None;
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut last);
        apply_all(&rx, &mut s);
        assert!(s.is_stroking());
        drive(&s, &tx, &f, None, false, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 1);
        assert_eq!(s.kind, None);
        drive(&s, &tx, &f, None, false, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 0);
    }

    #[test]
    fn no_contact_ends_stroke_instead_of_drawing_unknown() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let mut last = None;
        drive(
            &s,
            &tx,
            &fake(pen_sample),
            Some((5.0, 5.0)),
            false,
            None,
            &mut last,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.point_count(), 1);
        drive(
            &s,
            &tx,
            &fake(|| Ok(None)),
            Some((9.0, 5.0)),
            false,
            None,
            &mut last,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.point_count(), 1);
        assert!(!s.is_stroking());
    }

    #[test]
    fn idle_frame_surfaces_unsupported_without_pointer() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        drive(&s, &tx, &Unsupported, None, false, None, &mut None);
        assert_eq!(apply_all(&rx, &mut s), 1);
        assert_eq!(s.status, PenStatus::Failure(PenFailureView::Unsupported));
    }

    #[test]
    fn idle_frame_primes_the_os_layer() {
        let (tx, _rx) = channel();
        let s = PenDrawState::default();
        let f = fake(|| Ok(None));
        drive(&s, &tx, &f, None, false, None, &mut None);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let f = fake(pen_sample);
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut None);
        // 完了が反映される前にクリアされた場合、古い世代の完了は捨てられる。
        clear(&mut s);
        apply_all(&rx, &mut s);
        assert_eq!(s.point_count(), 0);
    }

    #[test]
    fn new_contact_in_same_frame_starts_a_new_stroke() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let f = fake(pen_sample);
        let mut last = None;
        drive(&s, &tx, &f, Some((5.0, 5.0)), false, None, &mut last);
        apply_all(&rx, &mut s);
        assert!(s.is_stroking());
        // 同じフレームに離して別の位置を押した: 前の点から補間せず、新しい点 1 つだけになる。
        drive(&s, &tx, &f, Some((60.0, 5.0)), true, None, &mut last);
        assert_eq!(apply_all(&rx, &mut s), 2);
        assert_eq!(s.point_count(), 2);
        assert!(s.is_stroking());
    }

    #[test]
    fn tap_within_one_frame_draws_one_point() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        // 押下から解放までが 1 フレームに収まった: 押下中の位置は無く、離した位置だけが届く。
        // OS 側は離した時点で値を消しているので、一定の太さの点 1 つを描いて線を終える。
        drive(
            &s,
            &tx,
            &fake(|| Ok(None)),
            None,
            false,
            Some((5.0, 5.0)),
            &mut None,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.point_count(), 1);
        assert!(!s.is_stroking());
    }

    #[test]
    fn release_draws_last_point_with_previous_sample() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let mut last = None;
        drive(
            &s,
            &tx,
            &fake(pen_sample),
            Some((5.0, 5.0)),
            false,
            None,
            &mut last,
        );
        apply_all(&rx, &mut s);
        let before = s.point_count();
        // 離したフレーム: OS 側の値は消えているが、前のフレームの値で最後の点を描く。
        drive(
            &s,
            &tx,
            &fake(|| Ok(None)),
            None,
            false,
            Some((9.0, 5.0)),
            &mut last,
        );
        apply_all(&rx, &mut s);
        assert!(s.point_count() > before);
        assert!(!s.is_stroking());
    }

    #[test]
    fn release_after_os_ended_stroke_adds_no_point() {
        let (tx, rx) = channel();
        let mut s = PenDrawState::default();
        let mut last = None;
        drive(
            &s,
            &tx,
            &fake(pen_sample),
            Some((5.0, 5.0)),
            false,
            None,
            &mut last,
        );
        apply_all(&rx, &mut s);
        // 押下中に OS 側が先に接触なしを返した: ここで線を終える。
        drive(
            &s,
            &tx,
            &fake(|| Ok(None)),
            Some((9.0, 5.0)),
            false,
            None,
            &mut last,
        );
        apply_all(&rx, &mut s);
        let before = s.point_count();
        assert!(!s.is_stroking());
        // 次のフレームで egui が離した位置を報告しても、離れた点を足さない。
        drive(
            &s,
            &tx,
            &fake(|| Ok(None)),
            None,
            false,
            Some((40.0, 5.0)),
            &mut last,
        );
        apply_all(&rx, &mut s);
        assert_eq!(s.point_count(), before);
    }
}
