//! Windows 向け文字サイズ追従実装（Issue #913）。`device/text_scale.rs` の
//! `shared_text_scale()` の Windows 経路から呼ばれ、WinRT
//! `Windows.UI.ViewManagement.UISettings.TextScaleFactor`（値域 1.0〜2.25、
//! <https://learn.microsoft.com/en-us/uwp/api/windows.ui.viewmanagement.uisettings.textscalefactor>）
//! を読む。
//!
//! `windows_biometric.rs`/`windows_share.rs` と異なり `unsafe` は不要
//! （`UISettings::new`/`TextScaleFactor` は `windows` クレート側で safe fn として
//! ラップ済み。`IUserConsentVerifierInterop`/`IDataTransferManagerInterop` のような
//! ウィンドウ束縛 interop も要らない値なので HWND も取らない）。よって `lib.rs` の
//! Windows `deny(unsafe_code)` に対する `#[allow(unsafe_code)]` も付けない。
//!
//! `TextScaleFactorChanged` イベントは配線しない（Issue #913 は任意扱い。画面側は
//! `OsScale::Unread` のときに 1 回読むだけで、設定変更後の再読込トリガが
//! `fandhe-core` 側に無いため、購読しても反映先が無い。設定変更への追従が
//! 要件化されたら別 Issue で追加する）。
//!
//! WinRT の activation factory は明示初期化なしでも `CO_E_NOTINITIALIZED` 時に
//! MTA へフォールバックする（`windows-core` の既定挙動、#898/#907 と同じ前提）ため
//! 本実装では `RoInitialize` を呼ばない。失敗は `Result` でそのまま返る。
//!
//! `HRESULT` や設定値の生値をログへ出さず、固定ラベルの `DeviceError::Internal` へ
//! 畳む（`windows_share.rs`/`windows_biometric.rs` と同じ方針、security.md A09）。

use fandhe_device::text_scale::TextScale;
use fandhe_device::DeviceError;
use windows::UI::ViewManagement::UISettings;

/// [`TextScale`] の Windows 実装。状態を持たないため呼び出しごとに `UISettings` を
/// 生成する（読むのは画面表示につき 1 回のため常駐コストを避ける）。
pub(crate) struct WindowsTextScale;

impl TextScale for WindowsTextScale {
    fn scale_factor(&self) -> Result<f32, DeviceError> {
        let settings =
            UISettings::new().map_err(|_| DeviceError::Internal("text_scale settings"))?;
        let factor = settings
            .TextScaleFactor()
            .map_err(|_| DeviceError::Internal("text_scale read"))?;
        Ok(factor as f32)
    }
}
