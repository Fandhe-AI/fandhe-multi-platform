//! macOS 向けシステム音量実装（Issue #785）。`macos/fandhe_macos_volume_control.m`（Core Audio の
//! `VirtualMainVolume` / `Mute`、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルし CoreAudio・AudioToolbox をリンクする）を `unsafe extern "C"` 2 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/volume_control.rs` の
//! macOS 経路（UI スレッドから同期）から呼ばれ、契約は `fandhe_device::volume_control`。
//!
//! `.m` が返す生値は [`from_raw`] で契約の値へ写す。写像と 1 段階の決定はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 1 段階の幅は `fandhe_device::volume_control::MACOS_STEPS` 分の 1（段の幅を返す API が無い）。
//!   設定値は機器が選べる値へ丸められるため、読み戻しが要求値と少しずれることがある（読み戻しを表示する）
//! - 音量の制御を持たない出力機器（HDMI 等）は `level: None` / `NotChangeable`
//! - 読み取りと設定の間に既定の出力機器が変わった場合の 1 段階の起点のずれは守らない範囲
//! - 実機での動作確認は #780

use fandhe_device::volume_control::{validate_level, VolumeControlMode, VolumeReading};
use fandhe_device::DeviceError;

#[cfg(target_os = "macos")]
use fandhe_device::volume_control::{
    classify_step, macos_next_level, StepDirection, StepOutcome, VolumeControl,
};
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// `.m` の `FandheVolumeRaw` と同じレイアウト。
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Raw {
    has_device: i32,
    has_level: i32,
    level: f32,
    settable: i32,
    has_mute: i32,
    muted: i32,
}

/// `.m` の生値を契約の値へ写す。音量は丸め・変換の前に検証する。
fn from_raw(raw: &Raw) -> Result<VolumeReading, DeviceError> {
    if raw.has_device == 0 || raw.has_level == 0 {
        // 出力機器なし・音量を持たない出力機器は失敗ではなく「変えられない」。
        return Ok(VolumeReading {
            level: None,
            muted: (raw.has_device != 0 && raw.has_mute != 0).then_some(raw.muted != 0),
            mode: VolumeControlMode::NotChangeable,
        });
    }
    Ok(VolumeReading {
        level: Some(validate_level(raw.level)?),
        muted: (raw.has_mute != 0).then_some(raw.muted != 0),
        mode: if raw.settable != 0 {
            VolumeControlMode::Steps
        } else {
            VolumeControlMode::NotChangeable
        },
    })
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_volume_control.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_volume_control_read(out: *mut Raw) -> i32;
        fn fandhe_macos_volume_control_set(level: f32, out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn read(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_volume_control_read(out) }
    }

    /// 0 = 成功。`level` は呼び出し側が検証済みの 0.0〜1.0。
    #[allow(unsafe_code)]
    pub(super) fn set(level: f32, out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_volume_control_set(level, out) }
    }
}

#[cfg(target_os = "macos")]
struct MacVolumeControl;

#[cfg(target_os = "macos")]
impl VolumeControl for MacVolumeControl {
    fn read(&self) -> Result<VolumeReading, DeviceError> {
        let mut raw = Raw::default();
        if ffi::read(&mut raw) != 0 {
            return Err(DeviceError::Internal("volume-control read"));
        }
        from_raw(&raw)
    }

    fn step(&self, direction: StepDirection) -> Result<StepOutcome, DeviceError> {
        let before = self.read()?;
        // 変えられない環境は OS へ渡さない。
        if before.mode != VolumeControlMode::Steps {
            return Ok(StepOutcome::NotChangeable(before));
        }
        let Some(current) = before.level else {
            return Ok(StepOutcome::NotChangeable(before));
        };
        // 上限・下限では設定せず、変わらなかったものとして返す（失敗ではない）。
        let Some(next) = macos_next_level(current, direction) else {
            return Ok(StepOutcome::Unchanged(before));
        };
        let next = validate_level(next)?;
        let mut raw = Raw::default();
        if ffi::set(next, &mut raw) != 0 {
            return Err(DeviceError::Internal("volume-control set"));
        }
        // 要求値ではなく、同じ呼び出しの中で読み戻した値で結果を決める。
        Ok(classify_step(&before, from_raw(&raw)?))
    }
}

/// macOS の `VolumeControl` 実装を返す（`device/volume_control.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(MacVolumeControl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(level: f32, settable: i32, mute: Option<bool>) -> Raw {
        Raw {
            has_device: 1,
            has_level: 1,
            level,
            settable,
            has_mute: mute.is_some() as i32,
            muted: mute.unwrap_or(false) as i32,
        }
    }

    #[test]
    fn maps_normal_values() {
        let r = from_raw(&raw(0.5, 1, Some(true))).unwrap();
        assert_eq!(r.level, Some(0.5));
        assert_eq!(r.muted, Some(true));
        assert_eq!(r.mode, VolumeControlMode::Steps);
    }

    #[test]
    fn mute_property_missing_is_none() {
        assert_eq!(from_raw(&raw(0.5, 1, None)).unwrap().muted, None);
    }

    #[test]
    fn not_settable_is_not_changeable() {
        let r = from_raw(&raw(0.5, 0, Some(false))).unwrap();
        assert_eq!(r.mode, VolumeControlMode::NotChangeable);
        assert_eq!(r.level, Some(0.5));
    }

    #[test]
    fn no_device_or_no_volume_property_is_not_a_failure() {
        let none = from_raw(&Raw::default()).unwrap();
        assert_eq!(none.level, None);
        assert_eq!(none.mode, VolumeControlMode::NotChangeable);
        let hdmi = Raw {
            has_device: 1,
            ..Raw::default()
        };
        let r = from_raw(&hdmi).unwrap();
        assert_eq!(r.level, None);
        assert_eq!(r.mode, VolumeControlMode::NotChangeable);
    }

    #[test]
    fn invalid_level_is_rejected_before_use() {
        for bad in [f32::NAN, f32::INFINITY, -0.5, 1.5] {
            assert!(
                matches!(from_raw(&raw(bad, 1, None)), Err(DeviceError::Internal(_))),
                "{bad}"
            );
        }
    }

    /// 実機の既定出力を読むだけ（音量は変えない）。機器の有無・種類に関わらず成功する。
    #[cfg(target_os = "macos")]
    #[test]
    fn real_os_read_succeeds_without_changing_volume() {
        assert!(implementation().read().is_ok());
    }
}
