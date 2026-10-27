//! iOS 向けシステムの色選択ダイアログの骨格（親 #184、Issue #941 (a)）。`device/color_picker.rs` の
//! iOS 経路から呼ばれる。実体は (b-ios) #942 が埋める。それまでは `Unsupported` を返し、成功にしない
//! （対応表の iOS は `possible` のまま）。
//!
//! (b-ios) への申し送り:
//! - `.m` で `UIColorPickerViewController`（`supportsAlpha = NO`）を提示し、結果コードは
//!   `fandhe_device::color_picker::result_from_apple_code` の表（0〜3）に合わせる
//! - 受け口で `bridge::deliver` の直後に `crate::ios::request_repaint` を呼ぶ（device-bridge-contracts.md 類型 4）
//! - `DidFinish` と `presentationControllerDidDismiss:` は first-wins で 1 回だけ通知する
//! - 背景や提示先が無いときは code 3。`build.rs` の iOS 分岐へ `.m` を足す
//! - `present` の引数に `request_id` を足し、`device/color_picker.rs` の iOS 側 `present` 呼び出しをそろえる

use fandhe_device::DeviceError;

/// 色選択の提示を依頼する。骨格のため常に `Unsupported`（呼び出し元が `bridge::deliver` へ畳む）。
pub(crate) fn present() -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("color_picker"))
}
