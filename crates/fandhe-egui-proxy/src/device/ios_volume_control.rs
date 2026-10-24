//! iOS 向けシステム音量実装（Issue #786、親 #166）。`AVAudioSession.outputVolume` を読み、
//! 変更は画面の下端に置く `MPVolumeView`（OS の音量スライダー）を利用者が操作する。
//! `device/volume_control.rs` の dispatch（iOS 経路、egui の update と同じ main thread から同期）から
//! 呼ばれ、契約は `fandhe_device::volume_control` の「OS ごとの写像」節。
//!
//! - `level` = `outputVolume`（[`validate_level`] を通す）、`muted` = `None`（消音スイッチを読む
//!   公開 API は無い）、`mode` = 常に `OsSlider`
//! - `step` は OS へ渡さず `Ok(StepOutcome::NotChangeable(reading))` を返す（失敗にしない）
//! - 音声セッションの category・有効化は変えない。通知は購読せず、dispatch が 1 秒ごとに読み直す
//!   （pull 型）。値を取った直後に `crate::ios::request_repaint` で次フレームを起こす
//!
//! # 守る範囲と守らない範囲
//!
//! `MPVolumeView` は 1 個を使い回し、プロセス終了まで残す（画面の離脱を知らせるフックは無く、
//! 1 プロセスに画面は 1 つ。OS が片付ける）。守らないのは、音声セッション無効時に
//! `outputVolume` が古い値を返す OS 側の挙動、シミュレータで音量を変えられないこと、
//! 読み直しの間隔ぶんの表示の遅れ。いずれも表示が一時的に食い違うだけで、嘘の成功には至らない。

#[cfg(target_os = "ios")]
use std::sync::Arc;

use fandhe_device::volume_control::{
    validate_level, StepOutcome, VolumeControlMode, VolumeReading,
};
#[cfg(target_os = "ios")]
use fandhe_device::volume_control::{StepDirection, VolumeControl};
use fandhe_device::DeviceError;

// `ios/fandhe_ios_volume_control.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_volume_control_read(out_level: *mut f32) -> i32;
}

/// 戻りコード（0 = 成功、2 = main 以外、4 = 引数不正、未知値）を契約のエラー種別へ写す。
/// OS 側の事情は固定文言に畳み、ログにも出さない。
fn decode_status(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        _ => Err(DeviceError::Internal("volume-control")),
    }
}

/// `read` の戻りコードと out 値を読み戻しへ写す。値域の検証は変換の前に行う。
fn reading_from_os(code: i32, raw: f32) -> Result<VolumeReading, DeviceError> {
    decode_status(code)?;
    Ok(VolumeReading {
        level: Some(validate_level(raw)?),
        muted: None,
        mode: VolumeControlMode::OsSlider,
    })
}

/// `step` は OS へ渡さず、読み取りが成功したら `NotChangeable`、失敗はそのまま返す。
fn step_from_read(r: Result<VolumeReading, DeviceError>) -> Result<StepOutcome, DeviceError> {
    r.map(StepOutcome::NotChangeable)
}

/// iOS の `VolumeControl` 実装（`AVAudioSession.outputVolume`）。
#[cfg(target_os = "ios")]
struct IosVolumeControl;

#[cfg(target_os = "ios")]
impl VolumeControl for IosVolumeControl {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<VolumeReading, DeviceError> {
        let mut level: f32 = 0.0;
        // SAFETY: ポインタはこのスタック上の有効な変数を指し、呼び出しの間だけ使われる。
        // main スレッドの検査は `.m` 側で行い、main 以外では UIKit に触れず 2 を返す。
        let code = unsafe { fandhe_ios_volume_control_read(&mut level) };
        let r = reading_from_os(code, level);
        // egui は入力が無いと描画しないため、値を取った直後に次フレームを起こす。
        crate::ios::request_repaint();
        r
    }

    fn step(&self, _direction: StepDirection) -> Result<StepOutcome, DeviceError> {
        step_from_read(self.read())
    }
}

/// `device/volume_control.rs` の iOS 経路から呼ばれる。iOS の実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(IosVolumeControl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_status_maps_non_zero_to_internal() {
        assert!(decode_status(0).is_ok());
        for c in [2, 4, 99] {
            assert!(matches!(decode_status(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn reading_accepts_valid_levels() {
        for v in [0.0_f32, 0.5, 1.0] {
            let r = reading_from_os(0, v).expect("有効な値");
            assert_eq!(r.level, Some(v));
            assert_eq!(r.muted, None);
            assert_eq!(r.mode, VolumeControlMode::OsSlider);
        }
    }

    #[test]
    fn reading_rejects_out_of_range() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert!(matches!(
                reading_from_os(0, v),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn reading_fails_on_error_code_regardless_of_value() {
        assert!(matches!(
            reading_from_os(2, 0.5),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn step_is_never_changed() {
        let r = reading_from_os(0, 0.5).unwrap();
        assert_eq!(
            step_from_read(Ok(r)).unwrap(),
            StepOutcome::NotChangeable(r)
        );
        assert!(matches!(
            step_from_read(Err(DeviceError::Internal("volume-control"))),
            Err(DeviceError::Internal(_))
        ));
    }
}
