//! Windows 向け触覚フィードバック実装（Issue #1091）。`driver.rs` の Windows
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
//! 3. `SupportedFeedback` から Click 波形だけを選ぶ（`crate::driver::select_feedback_index`）。
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

use crate::contract::Haptics;
use crate::error::HapticsError;
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, VibrationAccessStatus, VibrationDevice,
};

use crate::driver::select_feedback_index;

/// `VibrationDevice` で軽い振動を 1 回要求する実装。
pub(crate) struct WindowsHaptics;

impl Haptics for WindowsHaptics {
    fn tap(&self) -> Result<(), HapticsError> {
        let access = VibrationDevice::RequestAccessAsync()
            .and_then(|op| op.get())
            .map_err(|_| HapticsError::Internal)?;
        if access != VibrationAccessStatus::Allowed {
            return Err(HapticsError::Internal);
        }

        let device = match VibrationDevice::GetDefaultAsync().and_then(|op| op.get()) {
            Ok(device) => device,
            // 振動デバイス無しは `GetDefaultAsync` が null（S_OK + 空）を返す
            // （`windows_bluetooth_le.rs` と同じ判定）。
            Err(e) if e.code().0 == 0 => return Err(HapticsError::Unsupported),
            Err(_) => return Err(HapticsError::Internal),
        };

        let controller = device
            .SimpleHapticsController()
            .map_err(|_| HapticsError::Internal)?;
        let supported = controller
            .SupportedFeedback()
            .map_err(|_| HapticsError::Internal)?;
        let feedbacks: Vec<_> = supported.into_iter().collect();
        let mut waveforms = Vec::with_capacity(feedbacks.len());
        for f in &feedbacks {
            waveforms.push(f.Waveform().map_err(|_| HapticsError::Internal)?);
        }
        let click =
            KnownSimpleHapticsControllerWaveforms::Click().map_err(|_| HapticsError::Internal)?;
        let Some(index) = select_feedback_index(&waveforms, click) else {
            return Err(HapticsError::Unsupported);
        };

        controller
            .SendHapticFeedback(&feedbacks[index])
            .map_err(|_| HapticsError::Internal)
    }
}
