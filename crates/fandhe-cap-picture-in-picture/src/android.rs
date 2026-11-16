//! Android 向けピクチャ・イン・ピクチャ実装（Issue #366、親 #97 の b-android）。
//! `driver.rs` の `platform_picture_in_picture` の Android 経路から egui スレッドで呼ばれ、
//! `PictureInPictureHelper.prepare` / `start` / `stop` / `snapshot` / `shutdown`（static、第 1 引数は Activity）を同期 JNI で呼ぶ
//! pull 型。Java 側は `PictureInPictureHelper`。Java から Rust への native コールバックは無い
//! （ドライバが `snapshot` をポーリングする）ので、JNI 署名の照合先は Java のみ。
//!
//! # コード表（SSOT は Java `PictureInPictureHelper`）
//!
//! - 操作の戻り: 0 = 受付 / 1 = 失敗 / 2 = 非対応（API 26 未満・feature なし）/ 3 = 小窓にできない / 4 = 世代不一致
//! - snapshot: `long[2]` = {世代, phase}。phase は 0 Idle / 1 Ready / 2 Starting / 3 Active /
//!   4 Stopping / 5 Exited / 6 Failed(NotPossible) / 7 Failed(Internal)。未知の値は `Failed(Internal)`
//!
//! # 世代・ライフサイクル
//!
//! 世代は本モジュールが `AtomicU64` で採番する（1 から。Activity の作り直しでも戻さない）。
//! Activity は `configChanges` により小窓への遷移で作り直されない。作り直し（`onDestroy`）では
//! Java 側が状態を捨て、Rust 側の保留記録は `driver.rs` の `reset_pending_tracking` が捨てる。
//! winit は小窓の間 `onPause` で描画を止めるため、Java は `enterPictureInPictureMode` が true を返した
//! 時点で Active にする（ドライバは `start` 直後の同フレームで読む）。`onPause` は中断扱いにしない。
//! 利用者が小窓を閉じた・戻した・開始中の取り消しは `Exited`（成功）。
//!
//! 守る範囲: 1 プロセス内の順序・世代の照合・失敗を成功値へ畳まないこと・資源の解放。
//! 守らない範囲: 小窓中の画面静止（winit の制約）、`onPictureInPictureModeChanged` 到着までの表示の
//! 遅れ、小窓の位置・大きさ（OS が決める）、複数プロセス。実機確認は #363。

#[cfg(any(target_os = "android", test))]
use crate::contract::{PipFailure, PipPhase, PipSnapshot};
#[cfg(any(target_os = "android", test))]
use crate::error::PictureInPictureError;

/// 操作の戻りコードを契約へ写す。0 以外は固定の種別へ畳む。
#[cfg(any(target_os = "android", test))]
fn result_from_code(code: i32) -> Result<(), PictureInPictureError> {
    match code {
        0 => Ok(()),
        2 => Err(PictureInPictureError::Unsupported),
        3 => Err(PictureInPictureError::Disabled("picture_in_picture")),
        4 => Err(PictureInPictureError::InvalidInput(
            "picture_in_picture generation",
        )),
        _ => Err(PictureInPictureError::Internal("picture_in_picture")),
    }
}

/// phase コードを契約の状態へ写す。未知の値は `Failed(Internal)`。
#[cfg(any(target_os = "android", test))]
fn phase_from_code(code: i64) -> PipPhase {
    match code {
        0 => PipPhase::Idle,
        1 => PipPhase::Ready,
        2 => PipPhase::Starting,
        3 => PipPhase::Active,
        4 => PipPhase::Stopping,
        5 => PipPhase::Exited,
        6 => PipPhase::Failed(PipFailure::NotPossible),
        _ => PipPhase::Failed(PipFailure::Internal),
    }
}

/// `long[2]` を契約のスナップショットへ。形が違う・世代が負なら世代 0 の `Failed(Internal)`
/// （世代 0 はドライバが照合で捨てるので成功には化けない）。
#[cfg(any(target_os = "android", test))]
fn decode_snapshot(v: &[i64]) -> PipSnapshot {
    let bad = PipSnapshot {
        generation: 0,
        phase: PipPhase::Failed(PipFailure::Internal),
    };
    let [generation, phase] = *v else {
        return bad;
    };
    let Ok(generation) = u64::try_from(generation) else {
        return bad;
    };
    PipSnapshot {
        generation,
        phase: phase_from_code(phase),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::contract::{
        FRAME_HEIGHT, FRAME_WIDTH, PictureInPicture, PipFailure, PipPhase, PipSnapshot,
    };
    use crate::error::PictureInPictureError;
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::{JClass, JLongArray, JObject, JValue};

    use super::{decode_snapshot, result_from_code};

    /// `prepare` ごとに進める世代（0 は「なし」を表すため 1 から）。
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    pub(super) struct AndroidPictureInPicture;

    /// `PictureInPictureHelper`（機能 crate の Java）の static メソッドへ `f` を実行する。
    /// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
    /// Activity 未確立・JNI 失敗は `None`（例外は共通層が消す）。Helper が無い（生成前・破棄後）ときは
    /// Java 側が失敗コード・`null` を返す（成功には畳まない）。
    fn call_helper<T>(
        f: impl FnOnce(&mut jni::Env, &JObject, &JClass) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.PictureInPictureHelper"),
            f,
        )
        .ok()
    }

    fn by_generation(start: bool, generation: u64) -> Result<(), PictureInPictureError> {
        let code = call_helper(|env, activity, class| {
            let name = if start {
                jni::jni_str!("start")
            } else {
                jni::jni_str!("stop")
            };
            Ok(env
                .call_static_method(
                    class,
                    name,
                    jni::jni_sig!((android.app.Activity, long) -> int),
                    &[
                        JValue::Object(activity),
                        JValue::Long(generation as jni::sys::jlong),
                    ],
                )?
                .i()?)
        })
        .ok_or(PictureInPictureError::Internal("picture_in_picture"))?;
        result_from_code(code)
    }

    impl PictureInPicture for AndroidPictureInPicture {
        fn prepare(&self) -> Result<u64, PictureInPictureError> {
            let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
            let code = call_helper(|env, activity, class| {
                Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("prepare"),
                        jni::jni_sig!((android.app.Activity, long, int, int) -> int),
                        &[
                            JValue::Object(activity),
                            JValue::Long(generation as jni::sys::jlong),
                            JValue::Int(FRAME_WIDTH as i32),
                            JValue::Int(FRAME_HEIGHT as i32),
                        ],
                    )?
                    .i()?)
            })
            .ok_or(PictureInPictureError::Internal("picture_in_picture"))?;
            result_from_code(code).map(|()| generation)
        }

        fn start(&self, generation: u64) -> Result<(), PictureInPictureError> {
            by_generation(true, generation)
        }

        fn stop(&self, generation: u64) -> Result<(), PictureInPictureError> {
            by_generation(false, generation)
        }

        fn snapshot(&self) -> PipSnapshot {
            let values = call_helper(|env, activity, class| {
                let ret = env.call_static_method(
                    class,
                    jni::jni_str!("snapshot"),
                    jni::jni_sig!((android.app.Activity) -> long[]),
                    &[JValue::Object(activity)],
                )?;
                let arr = env.cast_local::<JLongArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(None);
                }
                // 長さを検証してから読む。
                if arr.len(env)? != 2 {
                    return Ok(None);
                }
                let mut v = [0i64; 2];
                arr.get_region(env, 0, &mut v)?;
                Ok(Some(v))
            })
            .flatten();
            match values {
                Some(v) => decode_snapshot(&v),
                None => PipSnapshot {
                    generation: 0,
                    phase: PipPhase::Failed(PipFailure::Internal),
                },
            }
        }

        fn shutdown(&self) {
            let _ = call_helper(|env, activity, class| {
                env.call_static_method(
                    class,
                    jni::jni_str!("shutdown"),
                    jni::jni_sig!((android.app.Activity) -> ()),
                    &[JValue::Object(activity)],
                )?;
                Ok(())
            });
        }
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn crate::contract::PictureInPicture> {
    std::sync::Arc::new(imp::AndroidPictureInPicture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_contract_errors() {
        assert!(result_from_code(0).is_ok());
        assert!(matches!(
            result_from_code(2),
            Err(PictureInPictureError::Unsupported)
        ));
        assert!(matches!(
            result_from_code(3),
            Err(PictureInPictureError::Disabled("picture_in_picture"))
        ));
        assert!(matches!(
            result_from_code(4),
            Err(PictureInPictureError::InvalidInput(
                "picture_in_picture generation"
            ))
        ));
        for c in [1, 99, -1] {
            assert!(matches!(
                result_from_code(c),
                Err(PictureInPictureError::Internal(_))
            ));
        }
    }

    #[test]
    fn phase_codes() {
        use PipPhase::*;
        let want = [
            Idle,
            Ready,
            Starting,
            Active,
            Stopping,
            Exited,
            Failed(PipFailure::NotPossible),
            Failed(PipFailure::Internal),
        ];
        for (i, p) in want.into_iter().enumerate() {
            assert_eq!(phase_from_code(i as i64), p);
        }
        for c in [8, 42, -1] {
            assert_eq!(phase_from_code(c), Failed(PipFailure::Internal));
        }
    }

    #[test]
    fn snapshot_decoding_never_fakes_success() {
        let s = decode_snapshot(&[3, 3]);
        assert_eq!((s.generation, s.phase), (3, PipPhase::Active));
        for bad in [&[][..], &[1][..], &[1, 2, 3][..], &[-1, 3][..]] {
            let s = decode_snapshot(bad);
            assert_eq!(s.generation, 0);
            assert_eq!(s.phase, PipPhase::Failed(PipFailure::Internal));
        }
    }
}
