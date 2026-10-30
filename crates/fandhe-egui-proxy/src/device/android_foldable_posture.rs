//! Android 向け折りたたみ姿勢実装（Issue #1098、親 #141）。`device/foldable_posture.rs` の
//! Android 経路から呼ばれ、Jetpack WindowManager の `WindowLayoutInfo` を Java 側
//! `FoldablePostureHelper`（`onStart` で購読、`onStop` で解除）が保持したスナップショットとして
//! 読み、`fandhe_device::foldable_posture` の「Android の写像」表どおりに契約の値へ写す。
//!
//! JNI 窓口（`MainActivity`。Java・本ファイルの `jni_sig!`・本 doc の 3 か所でそろえる）:
//!
//! - `int[] foldablePostureRead()`: `()[I`。`{status}`（未受信 0・失敗 2）または受信済み
//!   `{1, 窓の幅 px, 窓の高さ px, densityDpi, 件数 n, (state, orientation, left, top, right, bottom) × n}`
//!
//! コード表の SSOT は Java 側 `FoldablePostureHelper`。値の検査は [`decode`] が写像の前に f64 で行い、
//! 壊れた値・窓外の bounds はクランプせず `Internal`（成功値にしない）。null・JNI 失敗も `Internal`
//! で、未受信（`Ok(None)`）へ畳まない。
//!
//! ライフサイクル: 結果は呼び出しごとに同期で返り、Rust 側に Android 固有の保留状態は持たない。
//! Activity 再生成では旧インスタンスが破棄とキャッシュ消去を済ませ、新インスタンスは未受信から
//! 始まるため、旧値は新しい読み取りに混ざらない。権限は不要。
//!
//! 守る範囲・守らない範囲は `fandhe_device::foldable_posture` の doc と同じ。

use std::sync::Arc;

use fandhe_device::foldable_posture::{
    Fold, FoldInfo, FoldOrientation, FoldablePostureSource, Posture, PostureReading, MAX_EXTENT,
};
use fandhe_device::DeviceError;

/// スナップショットの status（SSOT は Java 側 `FoldablePostureHelper.STATUS_*`）。
/// 失敗（2）と未知の status は `decode` の既定分岐で `Internal` になる。
const STATUS_NONE: i32 = 0;
const STATUS_READY: i32 = 1;
/// 受信済みの先頭 5 要素（status・窓の幅・高さ・densityDpi・件数）と 1 件あたりの要素数。
const HEADER_LEN: usize = 5;
const FEATURE_LEN: usize = 6;
/// state / orientation の値（SSOT は Java 側）。未知（-1 を含む）は `Internal`。
const STATE_FLAT: i32 = 0;
const STATE_HALF_OPENED: i32 = 1;
const ORIENTATION_VERTICAL: i32 = 0;
const ORIENTATION_HORIZONTAL: i32 = 1;
/// dp 換算の基準密度（`DisplayMetrics.DENSITY_DEFAULT`）。
const DENSITY_DEFAULT: f64 = 160.0;

fn internal() -> DeviceError {
    DeviceError::Internal("foldable-posture")
}

/// スナップショット（`None` は null / JNI 失敗）を契約の結果へ写す純関数。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode(values: Option<Vec<i32>>) -> Result<Option<PostureReading>, DeviceError> {
    let v = values.ok_or_else(internal)?;
    match v.first().copied() {
        Some(STATUS_NONE) if v.len() == 1 => return Ok(None),
        Some(STATUS_READY) => {}
        _ => return Err(internal()),
    }
    if v.len() < HEADER_LEN || v[4] < 0 {
        return Err(internal());
    }
    let n = v[4] as usize;
    let expected = n
        .checked_mul(FEATURE_LEN)
        .and_then(|x| x.checked_add(HEADER_LEN))
        .ok_or_else(internal)?;
    if v.len() != expected {
        return Err(internal());
    }
    if v[1] <= 0 || v[2] <= 0 || v[3] <= 0 {
        return Err(internal());
    }
    let (w, h, dpi) = (f64::from(v[1]), f64::from(v[2]), f64::from(v[3]));
    let d = dpi / DENSITY_DEFAULT;
    if w / d > MAX_EXTENT || h / d > MAX_EXTENT {
        return Err(internal());
    }

    let mut folded = false;
    let mut folds = Vec::with_capacity(n);
    for f in v[HEADER_LEN..].chunks(FEATURE_LEN) {
        let (state, orientation) = (f[0], f[1]);
        let (l, t, r, b) = (
            f64::from(f[2]),
            f64::from(f[3]),
            f64::from(f[4]),
            f64::from(f[5]),
        );
        folded |= match state {
            STATE_FLAT => false,
            STATE_HALF_OPENED => true,
            _ => return Err(internal()),
        };
        if !(0.0 <= l && l <= r && r <= w && 0.0 <= t && t <= b && b <= h) {
            return Err(internal());
        }
        let fold = match orientation {
            ORIENTATION_VERTICAL => Fold {
                orientation: FoldOrientation::Vertical,
                start: (l / d) as f32,
                thickness: ((r - l) / d) as f32,
                extent: (w / d) as f32,
            },
            ORIENTATION_HORIZONTAL => Fold {
                orientation: FoldOrientation::Horizontal,
                start: (t / d) as f32,
                thickness: ((b - t) / d) as f32,
                extent: (h / d) as f32,
            },
            _ => return Err(internal()),
        };
        folds.push(fold);
    }
    let posture = if folded {
        Posture::Folded
    } else {
        Posture::Flat
    };
    // 折れ目が 1 つだけのときのみ位置が分かる。0 件・2 件以上は「折れ目なし」とは言わず不明。
    let fold = match folds.as_slice() {
        [one] => FoldInfo::Known(*one),
        _ => FoldInfo::Unknown,
    };
    Ok(Some(PostureReading { posture, fold }))
}

#[cfg(target_os = "android")]
struct AndroidFoldablePosture;

/// `MainActivity.foldablePosture*` を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_screen_orientation.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity<T>(
    f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, super::android::JniCallFailed>,
) -> Option<T> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<T, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        f(env, &activity)
    })
    .ok()
}

#[cfg(target_os = "android")]
impl FoldablePostureSource for AndroidFoldablePosture {
    fn read(&self) -> Result<Option<PostureReading>, DeviceError> {
        use jni::objects::JIntArray;
        let values = call_activity(|env, a| {
            let ret = env.call_method(
                a,
                jni::jni_str!("foldablePostureRead"),
                jni::jni_sig!(() -> int[]),
                &[],
            )?;
            let arr = env.cast_local::<JIntArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さの妥当性は decode が検証する（実長を読むので範囲外アクセスはない）。
            let len = arr.len(env)?;
            let mut buf = vec![0i32; len];
            arr.get_region(env, 0, &mut buf)?;
            Ok(Some(buf))
        })
        .flatten();
        decode(values)
    }
}

/// Android の `FoldablePostureSource` 実装を返す（`device/foldable_posture.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn FoldablePostureSource> {
    Arc::new(AndroidFoldablePosture)
}

/// ホストのテストビルドにも載るための形（dispatch は Android でしか呼ばない）。
/// 非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn FoldablePostureSource> {
    Arc::new(fandhe_device::foldable_posture::UnsupportedFoldablePosture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(w: i32, h: i32, dpi: i32, feats: &[[i32; 6]]) -> Option<Vec<i32>> {
        let mut v = vec![1, w, h, dpi, feats.len() as i32];
        for f in feats {
            v.extend_from_slice(f);
        }
        Some(v)
    }

    fn is_internal(r: Result<Option<PostureReading>, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal(_)))
    }

    #[test]
    fn not_received_is_none() {
        assert_eq!(decode(Some(vec![0])).unwrap(), None);
    }

    #[test]
    fn broken_snapshots_are_internal() {
        assert!(is_internal(decode(None)));
        assert!(is_internal(decode(Some(vec![]))));
        assert!(is_internal(decode(Some(vec![2]))));
        assert!(is_internal(decode(Some(vec![9]))));
        assert!(is_internal(decode(Some(vec![0, 0]))));
        assert!(is_internal(decode(Some(vec![1, 100, 100, 160]))));
        assert!(is_internal(decode(Some(vec![1, 100, 100, 160, -1]))));
        assert!(is_internal(decode(Some(vec![1, 100, 100, 160, 1]))));
        assert!(is_internal(decode(Some(vec![1, 100, 100, 160, i32::MAX]))));
    }

    #[test]
    fn no_feature_is_flat_unknown() {
        let r = decode(ready(1000, 800, 160, &[])).unwrap().unwrap();
        assert_eq!(r.posture, Posture::Flat);
        assert_eq!(r.fold, FoldInfo::Unknown);
    }

    #[test]
    fn vertical_half_opened_is_folded_known_in_dp() {
        // density 2.0（320dpi）。窓 2000x1600 px、帯 960..1040。
        let r = decode(ready(2000, 1600, 320, &[[1, 0, 960, 0, 1040, 1600]]))
            .unwrap()
            .unwrap();
        assert_eq!(r.posture, Posture::Folded);
        assert_eq!(
            r.fold,
            FoldInfo::Known(Fold {
                orientation: FoldOrientation::Vertical,
                start: 480.0,
                thickness: 40.0,
                extent: 1000.0,
            })
        );
    }

    #[test]
    fn horizontal_flat_and_zero_width_band() {
        let r = decode(ready(1000, 800, 160, &[[0, 1, 0, 400, 1000, 400]]))
            .unwrap()
            .unwrap();
        assert_eq!(r.posture, Posture::Flat);
        assert_eq!(
            r.fold,
            FoldInfo::Known(Fold {
                orientation: FoldOrientation::Horizontal,
                start: 400.0,
                thickness: 0.0,
                extent: 800.0,
            })
        );
    }

    #[test]
    fn two_features_fold_unknown_and_any_half_opened_folds() {
        let r = decode(ready(
            1000,
            800,
            160,
            &[[0, 0, 100, 0, 110, 800], [1, 0, 500, 0, 510, 800]],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(r.posture, Posture::Folded);
        assert_eq!(r.fold, FoldInfo::Unknown);
    }

    #[test]
    fn invalid_values_are_internal_without_clamp() {
        let ok = [1, 0, 500, 0, 510, 800];
        assert!(decode(ready(1000, 800, 160, &[ok])).is_ok());
        for bad in [
            [2, 0, 500, 0, 510, 800],
            [-1, 0, 500, 0, 510, 800],
            [1, 2, 500, 0, 510, 800],
            [1, -1, 500, 0, 510, 800],
            [1, 0, -1, 0, 510, 800],
            [1, 0, 510, 0, 500, 800],
            [1, 0, 500, 0, 1001, 800],
            [1, 0, 500, 0, 510, 801],
        ] {
            assert!(
                is_internal(decode(ready(1000, 800, 160, &[bad]))),
                "{bad:?}"
            );
        }
        assert!(is_internal(decode(ready(1000, 800, 0, &[ok]))));
        assert!(is_internal(decode(ready(0, 800, 160, &[ok]))));
        assert!(is_internal(decode(ready(1000, 0, 160, &[ok]))));
        assert!(is_internal(decode(ready(30_000, 800, 160, &[]))));
    }
}
