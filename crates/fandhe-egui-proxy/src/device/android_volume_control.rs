//! Android 向けシステム音量実装（Issue #787、親 #166）。`device/volume_control.rs` の dispatch
//! （Android 経路、egui スレッドから同期）から呼ばれ、契約は `fandhe_device::volume_control`。
//! `AudioManager` の `STREAM_MUSIC`（メディア音量）だけを扱う。Java 側の SSOT は
//! `VolumeControlHelper`（結果コード）。
//!
//! JNI 窓口（`MainActivity`。Java・本ファイルの `jni_sig!`・非 Android 版の doc の 3 か所でそろえる）:
//!
//! - `int[] volumeControlRead()`: `()[I`。`{index, maxIndex, muted(0/1), fixed(0/1)}`。読めないときは null
//! - `int volumeControlAdjust(int)`: `(I)I`。1 = 上げる / -1 = 下げる。結果コードを返す
//!
//! 写像: `index / maxIndex` を `level`、`isStreamMute` を `muted`（minSdk 24 では常に `Some`）、
//! `isVolumeFixed` が真なら `NotChangeable`、偽なら `Steps`。`step` は「変更前の読み取り → 変更 →
//! 同じ呼び出しの中で読み戻し」で、読み戻しに失敗したら変更を成功として返さず `Internal`。
//! 上限・下限で値が変わらないときは `Unchanged`（失敗ではない）、消音が解けたときは `Changed`。
//! `SecurityException`（おやすみモード）は `PermissionDenied`。OS から返った整数は割り算の前に
//! 範囲を検証し、`validate_level` を通す。
//!
//! スレッド・期限: egui スレッドから同期で呼び、`AudioManager` は binder の同期呼び出しだけなので
//! UI キューへ post しない（期限後に遅れて適用される経路がない）。
//!
//! ライフサイクル: Java 側は要求 id も保留状態も持たず、Activity 再生成で破棄する保留要求や
//! 遅れて届く結果はない。要求の照合は driver の `RequestSlot` と ticket が持つ。
//!
//! 守る範囲: 1 プロセス内の順序・失敗を成功と見せないこと・権限不要（`uses-permission` なし）。
//! 守らない範囲: 他アプリによる音量変更への追従（driver が 1 秒ごとに読み直す）。
//! `STREAM_MUSIC` の最小音量が 0 より大きい端末では、契約どおり `index / max` で表示するため、
//! 下限で「音量が変わりませんでした」になることがある（表示上の差で、嘘の成功ではない）。
//! 消音中に OS が `getStreamVolume` へ 0 を返す場合、`ADJUST_RAISE` で消音が解ければ `Changed`。

use std::sync::Arc;

#[cfg_attr(not(any(target_os = "android", test)), allow(unused_imports))]
use fandhe_device::volume_control::{
    classify_step, validate_level, StepDirection, StepOutcome, UnsupportedVolumeControl,
    VolumeControl, VolumeControlMode, VolumeReading,
};
use fandhe_device::DeviceError;

/// 変更の結果コード（SSOT は Java 側 `VolumeControlHelper.ADJUST_*`）。
const ADJUST_OK: i32 = 0;
const ADJUST_PERMISSION_DENIED: i32 = 1;
#[cfg(test)]
const ADJUST_FAILED: i32 = 2;
#[cfg(test)]
const ADJUST_INVALID: i32 = 3;

/// Java へ渡す向き（`volumeControlAdjust` の引数）。
const DIR_UP: i32 = 1;
const DIR_DOWN: i32 = -1;

/// 向きを Java へ渡すコードへ写す純関数。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn encode_direction(dir: StepDirection) -> i32 {
    match dir {
        StepDirection::Up => DIR_UP,
        StepDirection::Down => DIR_DOWN,
    }
}

/// 読み取りの 4 要素を `VolumeReading` へ写す純関数。`None`（読めない）・範囲外は `Internal`。
/// 範囲は整数のまま割り算の前に検証する。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_read(values: Option<[i32; 4]>) -> Result<VolumeReading, DeviceError> {
    let [index, max, muted, fixed] = values.ok_or(DeviceError::Internal("volume-control"))?;
    if max <= 0
        || index < 0
        || index > max
        || !(0..=1).contains(&muted)
        || !(0..=1).contains(&fixed)
    {
        return Err(DeviceError::Internal("volume-control"));
    }
    let level = validate_level(index as f32 / max as f32)?;
    Ok(VolumeReading {
        level: Some(level),
        muted: Some(muted == 1),
        mode: if fixed == 1 {
            VolumeControlMode::NotChangeable
        } else {
            VolumeControlMode::Steps
        },
    })
}

/// 変更の結果コードを契約の結果へ写す純関数。未知値は成功にせず `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_adjust(code: i32) -> Result<(), DeviceError> {
    match code {
        ADJUST_OK => Ok(()),
        ADJUST_PERMISSION_DENIED => Err(DeviceError::PermissionDenied("volume-control")),
        _ => Err(DeviceError::Internal("volume-control")),
    }
}

/// 「変更前の読み取り → 変更 → 読み戻し」を組み立てる純関数（OS 呼び出しは引数で受ける）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn step_with(
    read: impl Fn() -> Result<VolumeReading, DeviceError>,
    adjust: impl FnOnce(i32) -> Result<(), DeviceError>,
    dir: StepDirection,
) -> Result<StepOutcome, DeviceError> {
    let before = read()?;
    if before.mode != VolumeControlMode::Steps {
        return Ok(StepOutcome::NotChangeable(before));
    }
    adjust(encode_direction(dir))?;
    // 読み戻しに失敗したら変更を成功として返さない。
    let after = read().map_err(|_| DeviceError::Internal("volume-control"))?;
    Ok(classify_step(&before, after))
}

#[cfg(target_os = "android")]
struct AndroidVolumeControl;

/// `MainActivity.volumeControl*` を JNI で呼ぶ。`None` は呼び出し失敗。
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
impl VolumeControl for AndroidVolumeControl {
    fn read(&self) -> Result<VolumeReading, DeviceError> {
        use jni::objects::JIntArray;
        let values = call_activity(|env, a| {
            let ret = env.call_method(
                a,
                jni::jni_str!("volumeControlRead"),
                jni::jni_sig!(() -> int[]),
                &[],
            )?;
            let arr = env.cast_local::<JIntArray>(ret.l()?)?;
            if arr.is_null() {
                return Ok(None);
            }
            // 長さを検証してから読む。
            if arr.len(env)? != 4 {
                return Ok(None);
            }
            let mut v = [0i32; 4];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten();
        decode_read(values)
    }

    fn step(&self, direction: StepDirection) -> Result<StepOutcome, DeviceError> {
        step_with(
            || self.read(),
            |dir| {
                let code = call_activity(|env, a| {
                    Ok(env
                        .call_method(
                            a,
                            jni::jni_str!("volumeControlAdjust"),
                            jni::jni_sig!((int) -> int),
                            &[jni::objects::JValue::Int(dir)],
                        )?
                        .i()?)
                });
                match code {
                    Some(c) => decode_adjust(c),
                    None => Err(DeviceError::Internal("volume-control")),
                }
            },
            direction,
        )
    }
}

/// Android の `VolumeControl` 実装を返す（`device/volume_control.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(AndroidVolumeControl)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形
/// （JNI 署名 `()[I` / `(I)I` は Android 版と同じ）。dispatch は Android でしか呼ばない。
/// 非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(UnsupportedVolumeControl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn steps(level: f32, muted: bool) -> VolumeReading {
        VolumeReading {
            level: Some(level),
            muted: Some(muted),
            mode: VolumeControlMode::Steps,
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        assert_eq!(
            (
                ADJUST_OK,
                ADJUST_PERMISSION_DENIED,
                ADJUST_FAILED,
                ADJUST_INVALID
            ),
            (0, 1, 2, 3)
        );
        assert_eq!((DIR_UP, DIR_DOWN), (1, -1));
        assert_eq!(encode_direction(StepDirection::Up), 1);
        assert_eq!(encode_direction(StepDirection::Down), -1);
    }

    #[test]
    fn decode_read_maps_valid_values() {
        let r = decode_read(Some([5, 10, 0, 0])).unwrap();
        assert_eq!(r, steps(0.5, false));
        let r = decode_read(Some([0, 15, 1, 0])).unwrap();
        assert_eq!(r.muted, Some(true));
        let r = decode_read(Some([5, 10, 0, 1])).unwrap();
        assert_eq!(r.mode, VolumeControlMode::NotChangeable);
    }

    #[test]
    fn decode_read_rejects_unreadable_and_out_of_range() {
        assert!(matches!(decode_read(None), Err(DeviceError::Internal(_))));
        for v in [
            [0, 0, 0, 0],
            [1, -1, 0, 0],
            [-1, 10, 0, 0],
            [11, 10, 0, 0],
            [5, 10, 2, 0],
            [5, 10, -1, 0],
            [5, 10, 0, 2],
            [5, 10, 0, -1],
        ] {
            assert!(
                matches!(decode_read(Some(v)), Err(DeviceError::Internal(_))),
                "{v:?}"
            );
        }
    }

    #[test]
    fn decode_adjust_never_treats_unknown_as_success() {
        assert!(decode_adjust(ADJUST_OK).is_ok());
        assert!(matches!(
            decode_adjust(ADJUST_PERMISSION_DENIED),
            Err(DeviceError::PermissionDenied(_))
        ));
        for c in [ADJUST_FAILED, ADJUST_INVALID, -1, 99] {
            assert!(matches!(decode_adjust(c), Err(DeviceError::Internal(_))));
        }
    }

    /// 呼び出しごとに順に値を返す read を作る。
    fn seq(vals: Vec<Option<VolumeReading>>) -> impl Fn() -> Result<VolumeReading, DeviceError> {
        let i = Cell::new(0usize);
        move || {
            let n = i.get();
            i.set(n + 1);
            vals[n.min(vals.len() - 1)].ok_or(DeviceError::Internal("volume-control"))
        }
    }

    #[test]
    fn step_raise_is_changed() {
        let out = step_with(
            seq(vec![Some(steps(0.5, false)), Some(steps(0.6, false))]),
            |d| {
                assert_eq!(d, DIR_UP);
                Ok(())
            },
            StepDirection::Up,
        )
        .unwrap();
        assert_eq!(out, StepOutcome::Changed(steps(0.6, false)));
    }

    #[test]
    fn step_at_limit_is_unchanged() {
        let out = step_with(
            seq(vec![Some(steps(1.0, false)), Some(steps(1.0, false))]),
            |_| Ok(()),
            StepDirection::Up,
        )
        .unwrap();
        assert_eq!(out, StepOutcome::Unchanged(steps(1.0, false)));
    }

    #[test]
    fn step_unmute_is_changed() {
        let out = step_with(
            seq(vec![Some(steps(0.0, true)), Some(steps(0.0, false))]),
            |_| Ok(()),
            StepDirection::Up,
        )
        .unwrap();
        assert!(matches!(out, StepOutcome::Changed(_)));
    }

    #[test]
    fn step_fixed_does_not_call_adjust() {
        let fixed = VolumeReading {
            mode: VolumeControlMode::NotChangeable,
            ..steps(0.5, false)
        };
        let out = step_with(
            seq(vec![Some(fixed)]),
            |_| panic!("固定では adjust を呼ばない"),
            StepDirection::Down,
        )
        .unwrap();
        assert_eq!(out, StepOutcome::NotChangeable(fixed));
    }

    #[test]
    fn step_propagates_permission_denied() {
        let r = step_with(
            seq(vec![Some(steps(0.5, false))]),
            |_| Err(DeviceError::PermissionDenied("volume-control")),
            StepDirection::Up,
        );
        assert!(matches!(r, Err(DeviceError::PermissionDenied(_))));
    }

    #[test]
    fn step_readback_failure_is_internal() {
        let r = step_with(
            seq(vec![Some(steps(0.5, false)), None]),
            |_| Ok(()),
            StepDirection::Up,
        );
        assert!(matches!(r, Err(DeviceError::Internal(_))));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        let v = implementation();
        assert!(v.read().is_err());
        assert!(v.step(StepDirection::Down).is_err());
    }
}
