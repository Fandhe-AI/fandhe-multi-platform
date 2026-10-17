//! 型サンプル「ペン・筆圧で描く」の Web 経路（親 #95、Issue #358）。
//!
//! `fandhe_core::screens::pen_draw`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`]
//! の effect として駆動する。`fandhe-egui-proxy` の wasm canvas 経路は対象外
//! （`UnsupportedPenStylus` のまま）。`fandhe_device::pen_stylus::PenStylus` trait（`Send + Sync`）は
//! JS の Closure を持てないため通さず、`PenSample::new` / `altitude_from_tilt_deg` で値を検証して
//! core の `apply_sample` / `end_stroke` / `apply_failure` へ直接流す。
//!
//! # 方式
//!
//! - `mount.rs` はイベントのたびに root の子を全部作り直すため、キャンバス要素にはリスナーも
//!   ポインタキャプチャも置けない。変わらない root 要素に Pointer Events（down / move / up /
//!   cancel / lostpointercapture）を 1 度だけ登録し、`pointerdown` の対象が `.fandhe-canvas` の内側
//!   のときだけ主ポインタ 1 本を追う
//! - イベントは操作のキューへ積むだけ。画面への反映は `requestAnimationFrame` の 1 本だけが
//!   1 フレーム 1 回の `apply` でまとめて行う（書き手 1 本。全再構築も 1 フレーム 1 回）
//! - 筆圧と傾きは `pointerType == "pen"` のときだけ使う。マウス・指は固定値（マウスは 0.5）が
//!   返るため、値に関わらず None（嘘の成功の禁止）
//! - 離す（終点を積んでから終える。筆圧は最後の接触サンプルを使う）・キャンセル・キャプチャ喪失・`blur`・`pagehide`・非表示で接触なしに戻す
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の down / move / up の順序、上の終了契機で接触なしに戻すこと、マウスと指で
//! 筆圧を使わないこと、非対応を成功に畳まないこと、リスナー・rAF・キャプチャの解放、古い世代の破棄。
//! 守らない: 主ポインタ 1 本以外、coalesced / predicted events（ブラウザが間引いた点は取らない）、
//! 筆圧を報告しないペンが仕様上返す 0.5（本物と区別できない）、傾きを報告しない機器の 0、
//! `altitudeAngle` / `azimuthAngle`・ホバー・消しゴム、点の上限 2000 と全再構築のコスト
//! （`pen_draw::MAX_POINTS`）、複数タブ。
//!
//! 筆圧・座標・例外の message は DOM・ログへ出さない（security.md A09）。

use fandhe_core::screens::pen_draw::{
    apply_sample, end_stroke, PenDrawState, PenSampleView, PointerKindView, CANVAS_SIZE,
};
use fandhe_device::pen_stylus::{altitude_from_tilt_deg, PenSample, PointerKind};

/// キューに積む操作。世代はイベント時点の画面の世代（クリア前の操作は core が捨てる）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Op {
    Sample {
        generation: u64,
        pos: (f32, f32),
        sample: PenSampleView,
    },
    End {
        generation: u64,
    },
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// `PointerEvent.pointerType` を種類へ。未知の値は `Unknown`。
pub(crate) fn kind_from_pointer_type(t: &str) -> PointerKind {
    match t {
        "pen" => PointerKind::Pen,
        "touch" => PointerKind::Touch,
        "mouse" => PointerKind::Mouse,
        _ => PointerKind::Unknown,
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// イベントの生の値を画面へ渡す 1 サンプルへ写す。筆圧と傾きはペンのときだけ取り、
/// 必ず [`PenSample::new`] を通す。傾きは変換の前に範囲を検証する（`altitude_from_tilt_deg`）。
pub(crate) fn sample_view(
    pointer_type: &str,
    pressure: f32,
    tilt_x: f32,
    tilt_y: f32,
) -> PenSampleView {
    let kind = kind_from_pointer_type(pointer_type);
    let (p, a) = if kind == PointerKind::Pen {
        (Some(pressure), altitude_from_tilt_deg(tilt_x, tilt_y))
    } else {
        (None, None)
    };
    let s = PenSample::new(kind, p, a);
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

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// 画面座標をキャンバスの論理座標（`CANVAS_SIZE` 四方）へ。矩形が潰れている・非有限なら `None`。
pub(crate) fn canvas_pos(
    client: (f64, f64),
    left_top: (f64, f64),
    size: (f64, f64),
) -> Option<(f32, f32)> {
    let (w, h) = size;
    let ok = [client.0, client.1, left_top.0, left_top.1, w, h]
        .iter()
        .all(|v| v.is_finite());
    if !ok || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let k = f64::from(CANVAS_SIZE);
    Some((
        ((client.0 - left_top.0) * k / w) as f32,
        ((client.1 - left_top.1) * k / h) as f32,
    ))
}

/// キューの操作を順に画面へ反映する。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn apply_ops(s: &mut PenDrawState, ops: &[Op]) {
    for op in ops {
        match *op {
            Op::Sample {
                generation,
                pos,
                sample,
            } => apply_sample(s, generation, pos, Some(sample)),
            Op::End { generation } => end_stroke(s, generation),
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::pen_draw_effect;

/// マウント破棄時にリスナー・rAF・キャプチャを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::pen_draw::{apply_failure, PenDrawState, PenFailureView, PenStatus};
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;

    use super::{apply_ops, canvas_pos, sample_view, Op};
    use crate::mount::{root_of, Applier};

    /// root に登録する Pointer Events。
    const POINTER_EVENTS: [&str; 5] = [
        "pointerdown",
        "pointermove",
        "pointerup",
        "pointercancel",
        "lostpointercapture",
    ];

    /// マウント単位の状態。closure は 1 度だけ作り、drop は `release_mount` だけで行う
    /// （実行中の closure を drop しない。compass と同じ方針）。
    struct Entry {
        root: web_sys::Element,
        /// 画面の世代（effect が描画のたびに写す）。
        generation: u64,
        /// 追っている pointerId。
        active: Option<i32>,
        /// 最後の接触サンプルの筆圧。`pointerup` は pressure=0 を返すため、終点にはこれを使う。
        last_pressure: f32,
        queue: Vec<Op>,
        /// 予約済みの rAF。
        frame: Option<i32>,
        pointer: Closure<dyn FnMut(web_sys::PointerEvent)>,
        lifecycle: Closure<dyn FnMut()>,
        on_frame: Closure<dyn FnMut()>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 登録したリスナー・rAF・キャプチャを外し、進行中の追跡とキューを捨てる（冪等）。
    fn teardown(e: &mut Entry) {
        for name in POINTER_EVENTS {
            let _ = e
                .root
                .remove_event_listener_with_callback(name, e.pointer.as_ref().unchecked_ref());
        }
        if let Some(w) = web_sys::window() {
            for name in ["blur", "pagehide"] {
                let _ = w.remove_event_listener_with_callback(
                    name,
                    e.lifecycle.as_ref().unchecked_ref(),
                );
            }
            if let Some(d) = w.document() {
                let _ = d.remove_event_listener_with_callback(
                    "visibilitychange",
                    e.lifecycle.as_ref().unchecked_ref(),
                );
            }
            if let Some(h) = e.frame.take() {
                let _ = w.cancel_animation_frame(h);
            }
        }
        if let Some(id) = e.active.take() {
            let _ = e.root.release_pointer_capture(id);
        }
        e.queue.clear();
    }

    fn schedule(e: &mut Entry) {
        if e.frame.is_some() {
            return;
        }
        e.frame = web_sys::window().and_then(|w| {
            w.request_animation_frame(e.on_frame.as_ref().unchecked_ref())
                .ok()
        });
    }

    /// 追跡を終え、`End` を積む（キャプチャも外す）。
    fn finish(e: &mut Entry) {
        if let Some(id) = e.active.take() {
            let _ = e.root.release_pointer_capture(id);
            e.queue.push(Op::End {
                generation: e.generation,
            });
            schedule(e);
        }
    }

    fn push_sample(e: &mut Entry, ev: &web_sys::PointerEvent, release: bool) {
        let Some(canvas) = e.root.query_selector(".fandhe-canvas").ok().flatten() else {
            return;
        };
        let r = canvas.get_bounding_client_rect();
        let Some(pos) = canvas_pos(
            (f64::from(ev.client_x()), f64::from(ev.client_y())),
            (r.left(), r.top()),
            (r.width(), r.height()),
        ) else {
            return;
        };
        if !release {
            e.last_pressure = ev.pressure();
        }
        let sample = sample_view(
            &ev.pointer_type(),
            if release {
                e.last_pressure
            } else {
                ev.pressure()
            },
            ev.tilt_x() as f32,
            ev.tilt_y() as f32,
        );
        e.queue.push(Op::Sample {
            generation: e.generation,
            pos,
            sample,
        });
        schedule(e);
    }

    fn handle_pointer(e: &mut Entry, ev: &web_sys::PointerEvent) {
        let id = ev.pointer_id();
        match ev.type_().as_str() {
            "pointerdown" => {
                let in_canvas = e.active.is_none()
                    && ev.button() == 0
                    && ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .and_then(|el| el.closest(".fandhe-canvas").ok().flatten())
                        .is_some_and(|c| e.root.contains(Some(&c)));
                if !in_canvas {
                    return;
                }
                e.active = Some(id);
                // 失敗しても続ける（キャプチャ無しでもキャンバス内は追える）。
                let _ = e.root.set_pointer_capture(id);
                ev.prevent_default();
                push_sample(e, ev, false);
            }
            "pointermove" => {
                if e.active == Some(id) {
                    push_sample(e, ev, false);
                }
            }
            // 離した位置を終点として積んでから終える（末尾の線分を失わない）。
            "pointerup" => {
                if e.active == Some(id) {
                    push_sample(e, ev, true);
                    finish(e);
                }
            }
            _ => {
                if e.active == Some(id) {
                    finish(e);
                }
            }
        }
    }

    fn new_entry(
        mount_id: u64,
        root: web_sys::Element,
        applier: &Applier<PenDrawState>,
        generation: u64,
    ) -> Entry {
        let pointer =
            Closure::<dyn FnMut(web_sys::PointerEvent)>::new(move |ev: web_sys::PointerEvent| {
                ENTRIES.with(|m| {
                    if let Some(e) = m.borrow_mut().get_mut(&mount_id) {
                        handle_pointer(e, &ev);
                    }
                });
            });
        let lifecycle = Closure::<dyn FnMut()>::new(move || {
            ENTRIES.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&mount_id) {
                    finish(e);
                }
            });
        });
        let frame_applier = applier.clone();
        let on_frame = Closure::<dyn FnMut()>::new(move || {
            // ENTRIES の borrow は apply の前に落とす（apply → effect が再び借りるため）。
            let ops = ENTRIES.with(|m| {
                m.borrow_mut()
                    .get_mut(&mount_id)
                    .map(|e| {
                        e.frame = None;
                        std::mem::take(&mut e.queue)
                    })
                    .unwrap_or_default()
            });
            if !ops.is_empty() {
                frame_applier.apply(move |s| apply_ops(s, &ops));
            }
        });
        Entry {
            root,
            generation,
            active: None,
            last_pressure: 0.0,
            queue: Vec::new(),
            frame: None,
            pointer,
            lifecycle,
            on_frame,
        }
    }

    /// root・window・document へリスナーを登録する。途中で失敗したら全部外して `Err`。
    fn register(e: &mut Entry) -> Result<(), ()> {
        let w = web_sys::window().ok_or(())?;
        let d = w.document().ok_or(())?;
        let ok = POINTER_EVENTS.iter().all(|n| {
            e.root
                .add_event_listener_with_callback(n, e.pointer.as_ref().unchecked_ref())
                .is_ok()
        }) && ["blur", "pagehide"].iter().all(|n| {
            w.add_event_listener_with_callback(n, e.lifecycle.as_ref().unchecked_ref())
                .is_ok()
        }) && d
            .add_event_listener_with_callback(
                "visibilitychange",
                e.lifecycle.as_ref().unchecked_ref(),
            )
            .is_ok();
        if ok {
            Ok(())
        } else {
            teardown(e);
            Err(())
        }
    }

    /// `mount_with_effect` へ渡す effect。再描画のたびに呼ばれるが、世代を写すだけで `apply` は
    /// 呼ばない（要求が無ければ何もしない。再描画ループを起こさない）。
    pub fn pen_draw_effect(state: &PenDrawState, applier: &Applier<PenDrawState>) {
        let mount_id = applier.mount_id();
        let failed = matches!(state.status, PenStatus::Failure(_));
        let existed = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            if let Some(e) = m.get_mut(&mount_id) {
                e.generation = state.generation;
                if failed {
                    teardown(e);
                }
                return true;
            }
            false
        });
        if existed || failed {
            return;
        }
        let Some(root) = root_of(mount_id) else {
            return;
        };
        let supported = web_sys::window()
            .is_some_and(|w| Reflect::has(&w, &JsValue::from_str("PointerEvent")).unwrap_or(false));
        if !supported {
            applier.apply(|s| apply_failure(s, PenFailureView::Unsupported));
            return;
        }
        let registered = ENTRIES.with(|m| {
            let mut e = new_entry(mount_id, root, applier, state.generation);
            let r = register(&mut e);
            m.borrow_mut().insert(mount_id, e);
            r
        });
        if registered.is_err() {
            applier.apply(|s| apply_failure(s, PenFailureView::Failed));
        }
    }

    /// マウント破棄時の解放。closure はここで drop する。
    pub(crate) fn release_mount(mount_id: u64) {
        let removed = ENTRIES.with(|m| m.borrow_mut().remove(&mount_id));
        if let Some(mut e) = removed {
            teardown(&mut e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::pen_draw::{clear, PenStatus};

    #[test]
    fn mouse_and_touch_never_carry_pressure_or_tilt() {
        for t in ["mouse", "touch"] {
            let v = sample_view(t, 0.5, 30.0, 0.0);
            assert_eq!(v.pressure, None);
            assert_eq!(v.altitude_deg, None);
        }
        assert_eq!(
            sample_view("mouse", 0.5, 0.0, 0.0).kind,
            PointerKindView::Mouse
        );
        assert_eq!(
            sample_view("touch", 1.0, 0.0, 0.0).kind,
            PointerKindView::Touch
        );
    }

    #[test]
    fn pen_values_are_validated() {
        let v = sample_view("pen", 0.8, 0.0, 0.0);
        assert_eq!(v.kind, PointerKindView::Pen);
        assert_eq!(v.pressure, Some(0.8));
        assert_eq!(v.altitude_deg, Some(90.0));
        assert_eq!(sample_view("pen", f32::NAN, 0.0, 0.0).pressure, None);
        assert_eq!(sample_view("pen", 1.5, 0.0, 0.0).pressure, Some(1.0));
        assert_eq!(sample_view("pen", 0.5, 120.0, 0.0).altitude_deg, None);
        let tilted = sample_view("pen", 0.5, 30.0, 0.0).altitude_deg.unwrap();
        assert!((tilted - 60.0).abs() < 0.01);
    }

    #[test]
    fn unknown_pointer_type_is_unknown() {
        assert_eq!(kind_from_pointer_type("hologram"), PointerKind::Unknown);
        assert_eq!(kind_from_pointer_type(""), PointerKind::Unknown);
    }

    #[test]
    fn canvas_pos_scales_and_rejects_bad_rects() {
        let p = canvas_pos((110.0, 70.0), (10.0, 20.0), (640.0, 640.0)).unwrap();
        assert_eq!(p, (50.0, 25.0));
        assert_eq!(canvas_pos((1.0, 1.0), (0.0, 0.0), (0.0, 10.0)), None);
        assert_eq!(canvas_pos((f64::NAN, 1.0), (0.0, 0.0), (10.0, 10.0)), None);
        assert_eq!(
            canvas_pos((1.0, 1.0), (0.0, 0.0), (f64::INFINITY, 10.0)),
            None
        );
    }

    #[test]
    fn ops_draw_then_reset_display_and_drop_stale_generation() {
        let mut s = PenDrawState::default();
        let sample = sample_view("pen", 0.5, 0.0, 0.0);
        let at = |x: f32| Op::Sample {
            generation: 0,
            pos: (x, 10.0),
            sample,
        };
        apply_ops(&mut s, &[at(10.0), at(30.0)]);
        assert_eq!(s.point_count(), 11);
        assert_eq!(s.status, PenStatus::Live);
        apply_ops(&mut s, &[Op::End { generation: 0 }]);
        assert_eq!(s.kind, None);
        assert_eq!(s.point_count(), 11);
        clear(&mut s);
        apply_ops(&mut s, &[at(10.0)]);
        assert_eq!(s.point_count(), 0);
    }
}
