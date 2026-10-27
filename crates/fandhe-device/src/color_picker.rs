//! システムの色選択ダイアログの契約（Phase 11 の拡充ループ、親 #184、Issue #941）。
//!
//! 固定の初期色 [`INITIAL_RGB`] から OS 標準の色選択 UI で 1 色を選ばせ、sRGB の `#rrggbb` で返す。
//! 呼び出し元は `fandhe-egui-proxy` の effect ドライバ（`device/color_picker.rs`。macOS・Windows・iOS）と、
//! Web 子（#943）の `fandhe-web-dom`。画面から色を拾う eye-dropper とは別機能。
//! 色の型は [`crate::eye_dropper::PickedColor`] を使い、エラー識別子だけ `color_picker` へ付け替える。
//!
//! # 結果の区別（嘘の成功を作らない）
//!
//! | 種別 | macOS | Windows | iOS |
//! | --- | --- | --- | --- |
//! | `Ok(PickedColor)` | 閉じるまでに色の変化通知が 1 回以上 | `ChooseColorW` が非 0 | 閉じるまでに選択通知が 1 回以上 |
//! | `Cancelled("color_picker")` | 変化通知 0 回のまま閉じた | 戻り値 0 かつ拡張エラー 0 | 選択通知 0 回のまま閉じた |
//! | `Internal(..)` | `NSApp` 無し・sRGB 変換失敗・NaN | owner 窓無し・拡張エラー非 0・`rgbResult` の最上位バイトが非 0 | 提示先無し・変換失敗 |
//!
//! `Unsupported("color_picker")` は Android・Linux など実装の無い OS が返す。権限は要らないので
//! `PermissionDenied` は返さない（該当なし）。「選んだ」は OS の通知・戻り値だけで決め、
//! 初期色との比較では推測しない。Web の「やめる」は OS の結果ではなく画面側の操作。
//!
//! # OS を正にする
//!
//! 期限・タイマーを持たない。確定は OS の閉じた通知と戻り値だけで決める。初期色はプログラムで
//! 入れてから購読する（初期設定による通知を「選んだ」と取り違えない）。
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、取り消し・不正値・失敗を成功に見せないこと、購読の解放。
//! 守らない: macOS で非アクティブ中の表示（`hidesOnDeactivate`）、閉じた通知が届かない場合の復旧、
//! Web で色を変えずに閉じた場合と初期色と同じ色を選んだ場合の区別、sRGB 外の色、ブラウザごとの差、
//! 複数タブ・複数プロセス・複数窓。Windows の `ChooseColorW` はモーダルで、表示中は egui の描画が止まる。
//!
//! # 情報非保持
//!
//! 選んだ色（フィンガープリントに使える）、HRESULT・拡張エラーの値はログ・エラーに出さず永続化しない。

use crate::DeviceError;
use crate::eye_dropper::{PickedColor, color_from_rgb, color_from_srgb_hex};

/// ダイアログに入れる固定の初期色（0xRRGGBB）。
pub const INITIAL_RGB: u32 = 0x33_66cc;

/// eye-dropper 側の識別子を `color_picker` のものへ付け替える。
fn rgb_err(e: DeviceError) -> DeviceError {
    match e {
        DeviceError::Internal(_) => DeviceError::Internal("color_picker rgb"),
        other => other,
    }
}

/// Apple（macOS・iOS 共用）の `.m` 側の結果コード → 結果の写像（SSOT）。0 = 選択（`rgb` を検査）、
/// 1 = 選ばずに閉じた、2 = sRGB 変換の失敗・NaN、3 = 提示できなかった。未知は `Internal`。
pub fn result_from_apple_code(code: i32, rgb: i32) -> Result<PickedColor, DeviceError> {
    match code {
        0 => color_from_rgb(i64::from(rgb)).map_err(rgb_err),
        1 => Err(DeviceError::Cancelled("color_picker")),
        2 => Err(DeviceError::Internal("color_picker convert")),
        3 => Err(DeviceError::Internal("color_picker present")),
        _ => Err(DeviceError::Internal("color_picker outcome")),
    }
}

/// 0xRRGGBB → `COLORREF`（`0x00bbggrr`）。最上位バイトは 0 になる。
pub fn colorref_from_rgb(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

/// `ChooseColorW` の結果の写像。`ok == false` のとき `rgb_result` は未定義なので読まない。
/// 拡張エラー 0 は取り消し、非 0 は失敗。`COLORREF` の最上位バイトが非 0 なら成功にしない。
pub fn result_from_choose_color(
    ok: bool,
    ext_error: u32,
    rgb_result: u32,
) -> Result<PickedColor, DeviceError> {
    if !ok {
        return if ext_error == 0 {
            Err(DeviceError::Cancelled("color_picker"))
        } else {
            Err(DeviceError::Internal("color_picker dialog"))
        };
    }
    if rgb_result >> 24 != 0 {
        return Err(DeviceError::Internal("color_picker rgb"));
    }
    color_from_rgb(i64::from(colorref_from_rgb(rgb_result))).map_err(rgb_err)
}

/// Web の `<input type="color">` の `value` を契約へ写す（`#` + 16 進 6 桁のみ、trim しない）。
pub fn color_from_input_value(s: &str) -> Result<PickedColor, DeviceError> {
    color_from_srgb_hex(s).map_err(|_| DeviceError::Internal("color_picker hex"))
}

/// 色選択ダイアログの契約。同期モーダルな OS（Windows）が実装する。コールバック型 OS
/// （macOS・iOS）は `fandhe-egui-proxy` の pull 型 bridge を使いこのトレイトを通らない。
pub trait ColorPicker: Send + Sync {
    /// ダイアログを 1 回開き、選ばれた色を返す。取り消しは `Err(Cancelled)`。
    fn pick(&self) -> Result<PickedColor, DeviceError>;
}

/// 実装の無い OS 用。ダミーの成功実装へは倒さない。
pub struct UnsupportedColorPicker;

impl ColorPicker for UnsupportedColorPicker {
    fn pick(&self) -> Result<PickedColor, DeviceError> {
        Err(DeviceError::Unsupported("color_picker"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_code_table() {
        assert_eq!(
            result_from_apple_code(0, 0x12_3456).unwrap().hex(),
            "#123456"
        );
        for rgb in [-1, 0x100_0000] {
            assert!(matches!(
                result_from_apple_code(0, rgb),
                Err(DeviceError::Internal("color_picker rgb"))
            ));
        }
        assert!(matches!(
            result_from_apple_code(1, 0),
            Err(DeviceError::Cancelled("color_picker"))
        ));
        assert!(matches!(
            result_from_apple_code(2, 0),
            Err(DeviceError::Internal("color_picker convert"))
        ));
        assert!(matches!(
            result_from_apple_code(3, 0),
            Err(DeviceError::Internal("color_picker present"))
        ));
        for code in [-1, 4, 99] {
            assert!(matches!(
                result_from_apple_code(code, 0x12_3456),
                Err(DeviceError::Internal("color_picker outcome"))
            ));
        }
    }

    #[test]
    fn colorref_swaps_channels() {
        assert_eq!(colorref_from_rgb(0x12_3456), 0x0056_3412);
        assert_eq!(colorref_from_rgb(colorref_from_rgb(0xAB_CDEF)), 0xAB_CDEF);
    }

    #[test]
    fn choose_color_table() {
        assert_eq!(
            result_from_choose_color(true, 0, 0x0056_3412)
                .unwrap()
                .hex(),
            "#123456"
        );
        assert!(matches!(
            result_from_choose_color(true, 0, 0x0156_3412),
            Err(DeviceError::Internal("color_picker rgb"))
        ));
        // 取り消し時の rgbResult は未定義。何が入っていても成功にならない。
        for junk in [0, 0x0056_3412, u32::MAX] {
            assert!(matches!(
                result_from_choose_color(false, 0, junk),
                Err(DeviceError::Cancelled("color_picker"))
            ));
        }
        assert!(matches!(
            result_from_choose_color(false, 0x1234, 0x0056_3412),
            Err(DeviceError::Internal("color_picker dialog"))
        ));
    }

    #[test]
    fn input_value_rejects_invalid() {
        assert_eq!(color_from_input_value("#AABBCC").unwrap().hex(), "#aabbcc");
        for s in ["", "#abc", "aabbcc", " #aabbcc", "#gggggg"] {
            assert!(matches!(
                color_from_input_value(s),
                Err(DeviceError::Internal("color_picker hex"))
            ));
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedColorPicker.pick(),
            Err(DeviceError::Unsupported("color_picker"))
        ));
    }
}
