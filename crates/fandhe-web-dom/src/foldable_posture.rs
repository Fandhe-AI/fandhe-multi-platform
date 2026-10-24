//! 型サンプル「折りたたみの姿勢・ヒンジ」の Web 経路（親 #141、Issue #724）。
//!
//! `fandhe_core::screens::foldable_posture_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`demo.rs` のエントリポイントが
//! effect を渡し、`mount::sweep_mounts` が破棄時に `release_mount` を呼ぶ。
//! 値の取得は Device Posture API（`navigator.devicePosture.type`）と Viewport Segments API
//! （`window.viewport.segments`）を `js_sys::Reflect` で読む（web-sys に型が無い）。
//! 共有するのは `fandhe_device::foldable_posture` の `READ_INTERVAL` と契約の写像
//! `reading_from_raw`（SSOT）だけ。`FoldablePostureSource` トレイトは経由しない。
//!
//! # 方式
//!
//! `setInterval(READ_INTERVAL)` を 1 マウント 1 本だけ張り、tick ごとに読み直す。`change` は
//! 購読しない（非表示の文書には届かず、bfcache から戻ると古い値が残るため）。
//! 前回と異なるときだけ apply する。経過時間は測らない（壁時計は使わない）。
//! 姿勢 API が無ければ区画は見ずに非対応にする。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 非対応・壊れた値を「平ら」や折れ目なしへ倒さない、クランプしない、破棄時と登録失敗時に
//! `clearInterval` と closure を解放する、値を `console` やエラー文言へ出さない。
//! 守らない範囲: 反映の遅れ（最大 `READ_INTERVAL` + 1 フレーム。背景タブではさらに遅れる）、
//! 折りたためない端末と広げた端末の区別、複数タブ間の食い違い、zoom が 1 でないときの位置のずれ。

use fandhe_core::screens::foldable_posture_show as screen;
use fandhe_core::screens::foldable_posture_show::{ReadFailure, FAILED_LABEL};
use fandhe_device::foldable_posture as dev;
use fandhe_device::DeviceError;

/// `navigator.devicePosture` から読んだ生の姿勢。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum RawPosture {
    /// `devicePosture` が無い・undefined・null。
    Missing,
    /// オブジェクトはあるが `type` が文字列でない。
    NotString,
    /// `type` の文字列。
    Type(String),
}

/// `window.viewport.segments` から読んだ生の区画。
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) enum RawSegments {
    /// `viewport` が無い、または `segments` が null・undefined。
    Missing,
    /// `segments` が配列でない。
    NotArray,
    /// 区画の矩形。数値でない成分は `f64::NAN`（値域検査で失敗にする）。
    List(Vec<dev::SegmentRect>),
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn failed() -> ReadFailure {
    ReadFailure::Failed(FAILED_LABEL.to_string())
}

fn map_posture(p: dev::Posture) -> screen::Posture {
    match p {
        dev::Posture::Flat => screen::Posture::Flat,
        dev::Posture::Folded => screen::Posture::Folded,
    }
}

fn map_fold(f: dev::FoldInfo) -> screen::FoldInfo {
    match f {
        dev::FoldInfo::Unknown => screen::FoldInfo::Unknown,
        dev::FoldInfo::Known(f) => screen::FoldInfo::Known(screen::Fold {
            orientation: match f.orientation {
                dev::FoldOrientation::Vertical => screen::FoldOrientation::Vertical,
                dev::FoldOrientation::Horizontal => screen::FoldOrientation::Horizontal,
            },
            start: f.start,
            thickness: f.thickness,
            extent: f.extent,
        }),
    }
}

/// 生の値を画面の入力へ写す純関数。姿勢 API が無いときは区画を見ずに非対応、
/// 型の違い・壊れた値は固定文言の失敗にする（「平ら」や折れ目なしへ倒さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify(
    posture: RawPosture,
    segments: RawSegments,
) -> Result<screen::PostureReading, ReadFailure> {
    let type_str = match posture {
        RawPosture::Missing => return Err(ReadFailure::Unsupported),
        RawPosture::NotString => return Err(failed()),
        RawPosture::Type(s) => s,
    };
    let list = match &segments {
        RawSegments::Missing => None,
        RawSegments::NotArray => return Err(failed()),
        RawSegments::List(v) => Some(v.as_slice()),
    };
    match dev::reading_from_raw(&type_str, list) {
        Ok(r) => Ok(screen::PostureReading {
            posture: map_posture(r.posture),
            fold: map_fold(r.fold),
        }),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(failed()),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::foldable_posture_show_effect;

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::foldable_posture_show::{
        apply_read, FoldablePostureShowState, PostureReading, ReadFailure,
    };
    use fandhe_device::foldable_posture::{SegmentRect, READ_INTERVAL};
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;

    use super::{classify, failed, RawPosture, RawSegments};
    use crate::mount::{is_live, Applier};

    struct Entry {
        handle: i32,
        // 破棄まで closure を生存させるために保持する。
        _closure: Closure<dyn FnMut()>,
    }

    thread_local! {
        /// マウントごとのタイマー（1 マウント 1 本）。`None` は登録失敗済みの印。
        static TIMERS: RefCell<HashMap<u64, Option<Entry>>> = RefCell::new(HashMap::new());
    }

    type ReadResult = Result<PostureReading, ReadFailure>;

    fn get(target: &JsValue, key: &str) -> Result<JsValue, ReadFailure> {
        Reflect::get(target, &JsValue::from_str(key)).map_err(|_| failed())
    }

    fn is_nullish(v: &JsValue) -> bool {
        v.is_undefined() || v.is_null()
    }

    fn read_posture(window: &web_sys::Window) -> Result<RawPosture, ReadFailure> {
        let nav: JsValue = window.navigator().into();
        let dp = get(&nav, "devicePosture")?;
        if is_nullish(&dp) {
            return Ok(RawPosture::Missing);
        }
        Ok(match get(&dp, "type")?.as_string() {
            Some(s) => RawPosture::Type(s),
            None => RawPosture::NotString,
        })
    }

    fn read_segments(window: &web_sys::Window) -> Result<RawSegments, ReadFailure> {
        let w: &JsValue = window.as_ref();
        let vp = get(w, "viewport")?;
        if is_nullish(&vp) {
            return Ok(RawSegments::Missing);
        }
        let segs = get(&vp, "segments")?;
        if is_nullish(&segs) {
            return Ok(RawSegments::Missing);
        }
        if !js_sys::Array::is_array(&segs) {
            return Ok(RawSegments::NotArray);
        }
        let mut out = Vec::new();
        for item in js_sys::Array::from(&segs).iter() {
            let num = |k: &str| -> Result<f64, ReadFailure> {
                Ok(get(&item, k)?.as_f64().unwrap_or(f64::NAN))
            };
            out.push(SegmentRect {
                x: num("x")?,
                y: num("y")?,
                width: num("width")?,
                height: num("height")?,
            });
        }
        Ok(RawSegments::List(out))
    }

    /// 姿勢 API の有無を先に判定し、無ければ区画は読まない。値は `console` へ出さない。
    fn read() -> ReadResult {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        let posture = read_posture(&window)?;
        if posture == RawPosture::Missing {
            return Err(ReadFailure::Unsupported);
        }
        let segments = read_segments(&window)?;
        classify(posture, segments)
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけタイマーを張る。
    pub fn foldable_posture_show_effect(
        _state: &FoldablePostureShowState,
        applier: &Applier<FoldablePostureShowState>,
    ) {
        let mount_id = applier.mount_id();
        if TIMERS.with(|t| t.borrow().contains_key(&mount_id)) {
            return;
        }
        let initial = read();
        let last = Rc::new(RefCell::new(initial.clone()));

        let tick_applier = applier.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let r = read();
            if r == *last.borrow() {
                return;
            }
            *last.borrow_mut() = r.clone();
            tick_applier.apply(move |s| apply_read(s, r));
        });

        let handle = web_sys::window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                READ_INTERVAL.as_millis() as i32,
            )
            .ok()
        });
        match handle {
            Some(handle) => {
                TIMERS.with(|t| {
                    t.borrow_mut().insert(
                        mount_id,
                        Some(Entry {
                            handle,
                            _closure: closure,
                        }),
                    );
                });
                applier.apply(move |s| apply_read(s, initial));
            }
            // 最初の値を出し続けず失敗表示にする（嘘の成功を避ける）。
            None => {
                TIMERS.with(|t| t.borrow_mut().insert(mount_id, None));
                applier.apply(|s| apply_read(s, Err(failed())));
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(Some(entry)) = TIMERS.with(|t| t.borrow_mut().remove(&mount_id)) {
            if let Some(w) = web_sys::window() {
                w.clear_interval_with_handle(entry.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screen::{FoldInfo, FoldOrientation, Posture};

    fn seg(x: f64, y: f64, w: f64, h: f64) -> dev::SegmentRect {
        dev::SegmentRect {
            x,
            y,
            width: w,
            height: h,
        }
    }
    fn ty(s: &str) -> RawPosture {
        RawPosture::Type(s.to_string())
    }
    fn is_failed(r: &Result<screen::PostureReading, ReadFailure>) -> bool {
        matches!(r, Err(ReadFailure::Failed(m)) if m == FAILED_LABEL)
    }

    #[test]
    fn missing_posture_is_unsupported_even_with_broken_segments() {
        assert_eq!(
            classify(RawPosture::Missing, RawSegments::NotArray),
            Err(ReadFailure::Unsupported)
        );
    }

    #[test]
    fn not_string_posture_fails() {
        assert!(is_failed(&classify(
            RawPosture::NotString,
            RawSegments::Missing
        )));
    }

    #[test]
    fn continuous_without_segments_is_flat_unknown() {
        let r = classify(ty("continuous"), RawSegments::Missing).unwrap();
        assert_eq!(r.posture, Posture::Flat);
        assert_eq!(r.fold, FoldInfo::Unknown);
    }

    #[test]
    fn folded_with_side_by_side_segments_is_vertical() {
        let segs = RawSegments::List(vec![
            seg(0.0, 0.0, 400.0, 800.0),
            seg(420.0, 0.0, 400.0, 800.0),
        ]);
        let r = classify(ty("folded"), segs).unwrap();
        assert_eq!(r.posture, Posture::Folded);
        match r.fold {
            FoldInfo::Known(f) => {
                assert_eq!(f.orientation, FoldOrientation::Vertical);
                assert_eq!(f.start, 400.0);
                assert_eq!(f.thickness, 20.0);
                assert_eq!(f.extent, 820.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn stacked_segments_are_horizontal() {
        let segs = RawSegments::List(vec![
            seg(0.0, 0.0, 800.0, 400.0),
            seg(0.0, 420.0, 800.0, 400.0),
        ]);
        let r = classify(ty("folded"), segs).unwrap();
        assert!(matches!(
            r.fold,
            FoldInfo::Known(f) if f.orientation == FoldOrientation::Horizontal
        ));
    }

    #[test]
    fn broken_segments_fail() {
        assert!(is_failed(&classify(ty("folded"), RawSegments::NotArray)));
        let nan = RawSegments::List(vec![
            seg(f64::NAN, 0.0, 400.0, 800.0),
            seg(420.0, 0.0, 400.0, 800.0),
        ]);
        assert!(is_failed(&classify(ty("folded"), nan)));
        assert!(is_failed(&classify(
            ty("folded"),
            RawSegments::List(vec![])
        )));
    }

    #[test]
    fn unknown_type_fails() {
        for t in ["closed", " folded", ""] {
            assert!(is_failed(&classify(ty(t), RawSegments::Missing)), "{t:?}");
        }
    }

    #[test]
    fn one_or_three_segments_are_unknown() {
        let one = RawSegments::List(vec![seg(0.0, 0.0, 400.0, 800.0)]);
        assert_eq!(
            classify(ty("continuous"), one).unwrap().fold,
            FoldInfo::Unknown
        );
        let three = RawSegments::List(vec![
            seg(0.0, 0.0, 100.0, 800.0),
            seg(110.0, 0.0, 100.0, 800.0),
            seg(220.0, 0.0, 100.0, 800.0),
        ]);
        assert_eq!(
            classify(ty("continuous"), three).unwrap().fold,
            FoldInfo::Unknown
        );
    }

    #[test]
    fn failed_label_has_no_digits() {
        assert!(!FAILED_LABEL.chars().any(|c| c.is_ascii_digit()));
    }
}
