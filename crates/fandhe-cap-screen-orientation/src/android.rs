//! Android 向け画面向き実装（Issue #657、親 #128）。`driver` の Android
//! 経路から呼ばれ、`Activity.setRequestedOrientation` で縦・横の固定と解除を行い、
//! `Display.getRotation` / `Configuration.orientation` / `getRequestedOrientation` で読み戻す。
//! Java 側の SSOT は `ScreenOrientationHelper`（コード表・結果コード・UI スレッドへの橋渡し）。
//!
//! JNI 窓口（`ScreenOrientationHelper` の static メソッド。Java・本ファイルの `jni_sig!`・非 Android 版の doc の 3 か所でそろえる）:
//!
//! - `int request(Activity, int)`: `(Landroid/app/Activity;I)I`。0 = 解除 / 1 = 縦 / 2 = 横。UI スレッドで適用し最長 1 秒待つ。helper が無ければ失敗
//! - `int[] read(Activity)`: `(Landroid/app/Activity;)[I`。`{orientation, degrees, lock}`。読めないときは null
//!
//! コード表は `crate::contract::parse_reading` と同値で、読み取りは必ず
//! `parse_reading` で検証してから使う（未知値は `Internal`、成功値にしない）。
//!
//! 期限: 待ちは Java 側で最長 1 秒（GameActivity の `onPause` との相互待ちを避け、driver の確認期限
//! 3 秒より短い）。期限切れは `Internal` で、期限後に遅れて適用されることはない。
//! `Ok` は「OS 側の希望を設定し読み戻せた」だけで、実際の向きの一致は driver が `read` で確定する。
//!
//! ライフサイクル: 結果は呼び出しごとに同期で返るため、Rust 側に Android 固有の保留状態は持たない
//! （古い確認の破棄は driver の世代管理に任せる）。Activity 破棄後の旧要求は Java 側で OS に触れず
//! 失敗になる。固定は再生成へ持ち越さない。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・期限後に適用しないこと・失敗時に OS 側の
//! 希望を残さないこと。守らない範囲: 分割画面・マルチウィンドウ・大画面での OS による無視
//! （読み戻しの失敗として出る）、背景遷移中の状態、再生成での固定の引き継ぎ。
//! 返すエラーは `Internal` のみ（権限不要で、取得も固定も `Unsupported` にならない）。

use std::sync::Arc;

use crate::contract::{Orientation, OrientationReading, ScreenOrientation, parse_reading};
use crate::error::ScreenOrientationError;

/// 結果コード（SSOT は Java 側 `ScreenOrientationHelper.RESULT_*`）。
const RESULT_OK: i32 = 0;
#[cfg(test)]
const RESULT_TIMEOUT: i32 = 1;
#[cfg(test)]
const RESULT_FAILED: i32 = 2;
#[cfg(test)]
const RESULT_INVALID: i32 = 3;

/// 依頼する固定コード（SSOT は Java 側 `LOCK_*`）。
const LOCK_NONE: i32 = 0;
const LOCK_PORTRAIT: i32 = 1;
const LOCK_LANDSCAPE: i32 = 2;

/// 固定の希望から Java へ渡すコードへ写す純関数。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn encode_lock(target: Option<Orientation>) -> i32 {
    match target {
        None => LOCK_NONE,
        Some(Orientation::Portrait) => LOCK_PORTRAIT,
        Some(Orientation::Landscape) => LOCK_LANDSCAPE,
    }
}

/// 結果コードを契約の結果へ写す純関数。0 以外はすべて `Internal`（`InvalidInput` は返さない）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_result(code: i32) -> Result<(), ScreenOrientationError> {
    if code == RESULT_OK {
        Ok(())
    } else {
        Err(ScreenOrientationError::Internal("screen-orientation"))
    }
}

/// 読み取りの 3 要素を `OrientationReading` へ写す純関数。`None`（読めない）は `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(values: Option<[i32; 3]>) -> Result<OrientationReading, ScreenOrientationError> {
    match values {
        Some([orientation, degrees, lock]) => parse_reading(orientation, degrees, lock),
        None => Err(ScreenOrientationError::Internal("screen-orientation")),
    }
}

#[cfg(target_os = "android")]
struct AndroidScreenOrientation;

/// `ScreenOrientationHelper` の static メソッドを JNI で呼ぶ。`None` は呼び出し失敗。
/// JavaVM と Activity の取得は `fandhe_platform_android::with_activity_class` に任せ、ここは `unsafe` を持たない。
#[cfg(target_os = "android")]
fn call_helper<T>(
    f: impl FnOnce(
        &mut jni::Env,
        &jni::objects::JObject,
        &jni::objects::JClass,
    ) -> Result<T, fandhe_platform_android::JniCallFailed>,
) -> Option<T> {
    use fandhe_platform_android::{android_app, with_activity_class};
    let app = android_app()?;
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.ScreenOrientationHelper"),
        |env, activity, class| {
            let r = f(env, activity, class);
            if r.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            r
        },
    )
    .ok()
}

#[cfg(target_os = "android")]
impl ScreenOrientation for AndroidScreenOrientation {
    fn read(&self) -> Result<OrientationReading, ScreenOrientationError> {
        use jni::objects::{JIntArray, JValue};
        let values = call_helper(|env, a, class| {
            let ret = env.call_static_method(
                class,
                jni::jni_str!("read"),
                jni::jni_sig!((android.app.Activity) -> int[]),
                &[JValue::Object(a)],
            )?;
            let arr = env.cast_local::<JIntArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            if arr.len(env)? != 3 {
                return Ok(None);
            }
            let mut v = [0i32; 3];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten();
        decode_read(values)
    }

    fn request_lock(&self, target: Option<Orientation>) -> Result<(), ScreenOrientationError> {
        let lock = encode_lock(target);
        let code = call_helper(|env, a, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("request"),
                    jni::jni_sig!((android.app.Activity, int) -> int),
                    &[
                        jni::objects::JValue::Object(a),
                        jni::objects::JValue::Int(lock),
                    ],
                )?
                .i()?)
        });
        match code {
            Some(c) => decode_result(c),
            None => Err(ScreenOrientationError::Internal("screen-orientation")),
        }
    }

    fn can_lock(&self) -> bool {
        // setRequestedOrientation は API 1、SENSOR_* は API 9 から使える（minSdk 24）。
        // 分割画面などで OS が無視した場合は読み戻しで失敗として出る。
        true
    }
}

/// Android の `ScreenOrientation` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn ScreenOrientation> {
    Arc::new(AndroidScreenOrientation)
}

/// ホストのテストビルドにも載るための形
/// （JNI 署名は Android 版と同じ）。dispatch は Android でしか呼ばない。
/// 非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn ScreenOrientation> {
    Arc::new(crate::contract::UnsupportedScreenOrientation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `ScreenOrientationHelper` の定数と同じ値であること。
        assert_eq!(
            (RESULT_OK, RESULT_TIMEOUT, RESULT_FAILED, RESULT_INVALID),
            (0, 1, 2, 3)
        );
        assert_eq!((LOCK_NONE, LOCK_PORTRAIT, LOCK_LANDSCAPE), (0, 1, 2));
    }

    #[test]
    fn encode_lock_maps_three_values() {
        assert_eq!(encode_lock(None), 0);
        assert_eq!(encode_lock(Some(Orientation::Portrait)), 1);
        assert_eq!(encode_lock(Some(Orientation::Landscape)), 2);
    }

    #[test]
    fn decode_result_maps_codes() {
        assert!(decode_result(RESULT_OK).is_ok());
        for c in [RESULT_TIMEOUT, RESULT_FAILED, RESULT_INVALID, -1, 99] {
            assert!(matches!(
                decode_result(c),
                Err(ScreenOrientationError::Internal(_))
            ));
        }
    }

    #[test]
    fn decode_read_rejects_unreadable_and_unknown() {
        assert!(matches!(
            decode_read(None),
            Err(ScreenOrientationError::Internal(_))
        ));
        for v in [
            [0, 0, 0],
            [3, 0, 0],
            [1, -1, 0],
            [1, 45, 0],
            [1, 0, -1],
            [1, 0, 3],
        ] {
            assert!(
                matches!(
                    decode_read(Some(v)),
                    Err(ScreenOrientationError::Internal(_))
                ),
                "{v:?}"
            );
        }
    }

    #[test]
    fn decode_read_accepts_valid_codes() {
        for o in [1, 2] {
            for d in [0, 90, 180, 270] {
                for l in [0, 1, 2] {
                    let r = decode_read(Some([o, d, l])).unwrap();
                    assert_eq!(r.orientation == Orientation::Portrait, o == 1);
                    assert_eq!(r.rotation.degrees() as i32, d);
                    assert_eq!(r.lock.is_some(), l != 0);
                }
            }
        }
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let imp = implementation();
        assert!(imp.read().is_err());
        assert!(imp.request_lock(Some(Orientation::Portrait)).is_err());
        assert!(imp.request_lock(None).is_err());
        assert!(!imp.can_lock());
    }
}
