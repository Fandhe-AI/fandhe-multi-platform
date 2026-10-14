//! Windows 向け触覚フィードバック実装（Issue #1091）。`device/haptics.rs` の Windows
//! 経路から呼ばれ、WinRT `VibrationDevice` / `SimpleHapticsController` で軽い振動を
//! 1 回要求する（`Haptics` トレイトを実装。呼び出し側がワーカースレッドで呼ぶ前提、
//! `.get()` が同期待ちのため）。
//!
//! # 流れ
//!
//! 1. `VibrationDevice::RequestAccessAsync().get()`: `Allowed` 以外は `Internal`
//!    （契約上 `PermissionDenied` は返さない。成功にもしない）
//! 2. `VibrationDevice::GetDefaultAsync().get()`: null（振動デバイス無し）は `Unsupported`、
//!    それ以外のエラーは `Internal`
//! 3. `SupportedFeedback` から Click 波形だけを選ぶ（`haptics::select_feedback_index`）。
//!    Click が無ければ `Unsupported`（連続波形等は止められないため使わない）
//! 4. `SendHapticFeedback`。失敗は `Internal`
//!
//! `HRESULT`・デバイス情報はログにも画面にも出さず固定ラベルへ畳む（security.md A09）。
//! `unsafe` と `RoInitialize` は使わない（既存の Windows 実装群と同じ前提）。
//!
//! # 既知の天井
//!
//! パッケージ無しの素の exe から `RequestAccessAsync` / `GetDefaultAsync` が届くかは
//! 一次情報で未確認で、実機確認待ち。届かないと判明したら対応表の `done` を取り下げる
//! （`windows_share.rs` の前例と同じ扱い）。ローカルに MSVC が無く、型検査は
//! `x86_64-pc-windows-gnu` のみ。`.get()` に期限は設けない。

use fandhe_device::haptics::Haptics;
use fandhe_device::DeviceError;
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, VibrationAccessStatus, VibrationDevice,
};

use super::haptics::select_feedback_index;

/// `VibrationDevice` で軽い振動を 1 回要求する実装。
pub(crate) struct WindowsHaptics;

impl Haptics for WindowsHaptics {
    fn tap(&self) -> Result<(), DeviceError> {
        let access = VibrationDevice::RequestAccessAsync()
            .and_then(|op| op.get())
            .map_err(|_| DeviceError::Internal("haptics access"))?;
        if access != VibrationAccessStatus::Allowed {
            return Err(DeviceError::Internal("haptics access"));
        }

        let device = match VibrationDevice::GetDefaultAsync().and_then(|op| op.get()) {
            Ok(device) => device,
            // 振動デバイス無しは `GetDefaultAsync` が null（S_OK + 空）を返す
            // （`windows_bluetooth_le.rs` と同じ判定）。
            Err(e) if e.code().0 == 0 => return Err(DeviceError::Unsupported("haptics")),
            Err(_) => return Err(DeviceError::Internal("haptics init")),
        };

        let controller = device
            .SimpleHapticsController()
            .map_err(|_| DeviceError::Internal("haptics init"))?;
        let supported = controller
            .SupportedFeedback()
            .map_err(|_| DeviceError::Internal("haptics init"))?;
        let feedbacks: Vec<_> = supported.into_iter().collect();
        let mut waveforms = Vec::with_capacity(feedbacks.len());
        for f in &feedbacks {
            waveforms.push(
                f.Waveform()
                    .map_err(|_| DeviceError::Internal("haptics init"))?,
            );
        }
        let click = KnownSimpleHapticsControllerWaveforms::Click()
            .map_err(|_| DeviceError::Internal("haptics init"))?;
        let Some(index) = select_feedback_index(&waveforms, click) else {
            return Err(DeviceError::Unsupported("haptics"));
        };

        controller
            .SendHapticFeedback(&feedbacks[index])
            .map_err(|_| DeviceError::Internal("haptics send"))
    }
}
