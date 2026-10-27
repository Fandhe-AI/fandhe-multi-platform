//! Windows 向けシステムの色選択ダイアログ実装（Issue #941）。`ChooseColorW`（従来型の簡易ダイアログ）を
//! 固定の初期色で開き、OK で非 0 の `rgbResult`（`COLORREF` = `0x00bbggrr`）を受ける。
//! `device/color_picker.rs` の Windows 経路から呼ばれ、契約は `fandhe_device::color_picker`。
//! UI スレッドで同期に呼ぶ（print と同じ同期モーダル）。
//!
//! 結果の写像（戻り値 0 かつ拡張エラー 0 は取り消し、`COLORREF` → `0xRRGGBB`）は
//! `fandhe_device::color_picker::result_from_choose_color` の純関数に任せ、このファイルは ffi だけを持つ。
//!
//! # 既知の制約
//!
//! - `ChooseColorW` は自前のメッセージループを回すため、表示中は egui の描画が止まる
//! - `lpCustColors`（16 要素）は呼び出しごとのローカルで、カスタム色は保存しない
//! - 取り消し・エラー時の `rgbResult` は未定義なので読まない
//! - HRESULT・拡張エラーの値・選んだ色はログに出さない

use fandhe_device::color_picker::{
    colorref_from_rgb, result_from_choose_color, ColorPicker, INITIAL_RGB,
};
use fandhe_device::eye_dropper::PickedColor;
use fandhe_device::DeviceError;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CommDlgExtendedError, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};

struct WindowsColorPicker;

impl ColorPicker for WindowsColorPicker {
    #[allow(unsafe_code)]
    fn pick(&self) -> Result<PickedColor, DeviceError> {
        let hwnd =
            super::windows_share::window().ok_or(DeviceError::Internal("color_picker owner"))?;
        let mut custom = [COLORREF(0); 16];
        let mut cc = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: hwnd,
            rgbResult: COLORREF(colorref_from_rgb(INITIAL_RGB)),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        // SAFETY: `cc` / `custom` はこの呼び出しの間だけ生きるローカル値で、他から参照されない。
        // `lStructSize` は構造体の大きさ、`lpCustColors` は必須の 16 要素。`hwnd` は
        // `windows_share::window()` が返したメインウィンドウ（`ProxyApp` が生きている間は有効）。
        let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
        // 0 のときだけ拡張エラーを読む（取り消しとエラーの区別）。
        let ext = if ok {
            0
        } else {
            // SAFETY: 直前の呼び出しの失敗理由を同一スレッドで読むだけ。
            unsafe { CommDlgExtendedError() }.0
        };
        result_from_choose_color(ok, ext, cc.rgbResult.0)
    }
}

/// Windows の `ColorPicker` 実装を返す（`device/color_picker.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Box<dyn ColorPicker> {
    Box::new(WindowsColorPicker)
}
