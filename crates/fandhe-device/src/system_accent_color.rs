//! システムのアクセント色の契約（Issue #421、親 #103）。OS のアクセント色を 1 つ
//! （sRGB 8bit×3、アルファは捨てる）取得するための橋渡しで、`fandhe-egui-proxy` の
//! `device/system_accent_color.rs` が間隔つきで [`AccentColorSource::read`] を呼ぶ
//! （pull 型。OS の変化通知は購読しない）。Web は `fandhe-web-dom` が計算済みスタイルを読む。
//!
//! # 契約
//!
//! - 結果は [`Rgb`] か `DeviceError`（`Unsupported` / `Internal` のみ）。失敗・非対応で
//!   既定色を返さない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **三値**（契約表 10）: 結果は「読めた」「非対応（`Unsupported`）」「失敗（`Internal`）」。
//!   利用者のキャンセル・権限ダイアログ・入力は無いので、キャンセルと権限拒否は該当なし
//! - 購読・保留中の要求・世代を持たないので、古い世代の完了・期限・キャンセルの問題は
//!   構造上起きない（契約表 1・2・3 は該当なし）
//! - 色の値は端末のフィンガープリントに使えるため、ログへ出さない
//! - 「変更に追従」は最大 [`READ_INTERVAL`] + 1 フレームで反映される、と読み替える
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の色を出さないこと（失敗・非対応で既定色・前回値を返さない）、1 プロセス内の取得。
//! 守らない: 反映の遅れ（最大 1 秒 + 1 フレーム。Web の背景タブはさらに遅れる）、OS 間で色の
//! 意味が少しずつ違うこと（Android は `system_accent1_500`、macOS は外観に合わせて解決した値、
//! Windows は `Accent` そのもの）、Android の AOSP 既定値と macOS の「マルチカラー」で返る青を
//! 本物と区別しないこと、Web でブラウザが返す固定値、複数タブ・複数プロセス間の食い違い。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// sRGB 8bit の色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// 赤。
    pub r: u8,
    /// 緑。
    pub g: u8,
    /// 青。
    pub b: u8,
}

/// アクセント色を 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait AccentColorSource: Send + Sync {
    /// 現在のアクセント色を返す。失敗後に直前の値を返し続けてはならない。
    fn read(&self) -> Result<Rgb, DeviceError>;
}

/// 常に非対応を返す実装（未実装 OS の dispatch 先・スタブ）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedAccentColor;

impl AccentColorSource for UnsupportedAccentColor {
    fn read(&self) -> Result<Rgb, DeviceError> {
        Err(DeviceError::Unsupported("system_accent_color"))
    }
}

/// ネイティブ層（macOS の C ABI・Android の JNI）が返す `i32` を契約の値へ写す。SSOT。
///
/// - `0..=0xFF_FFFF`: `0xRRGGBB`
/// - `-1`: 読み取り失敗（`Internal`）
/// - `-2`: この OS 版では取れない（`Unsupported`。Android API 30 以下）
/// - それ以外の負値・`0xFF_FFFF` 超: 未定義コード（`Internal`。成功にしない）
///
/// 値域の検査は変換の前に行う。
pub fn rgb_from_code(code: i32) -> Result<Rgb, DeviceError> {
    match code {
        0..=0xFF_FFFF => Ok(Rgb {
            r: ((code >> 16) & 0xFF) as u8,
            g: ((code >> 8) & 0xFF) as u8,
            b: (code & 0xFF) as u8,
        }),
        -2 => Err(DeviceError::Unsupported("system_accent_color")),
        _ => Err(DeviceError::Internal("system_accent_color read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedAccentColor.read(),
            Err(DeviceError::Unsupported("system_accent_color"))
        ));
    }

    #[test]
    fn code_splits_channels() {
        assert_eq!(rgb_from_code(0).unwrap(), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            rgb_from_code(0xFF_FFFF).unwrap(),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(
            rgb_from_code(0x0A_84FF).unwrap(),
            Rgb {
                r: 0x0A,
                g: 0x84,
                b: 0xFF
            }
        );
    }

    #[test]
    fn sentinel_and_out_of_range_codes_never_succeed() {
        assert!(matches!(
            rgb_from_code(-1),
            Err(DeviceError::Internal("system_accent_color read"))
        ));
        assert!(matches!(
            rgb_from_code(-2),
            Err(DeviceError::Unsupported("system_accent_color"))
        ));
        for c in [-3, i32::MIN, 0x100_0000, i32::MAX] {
            assert!(
                matches!(rgb_from_code(c), Err(DeviceError::Internal(_))),
                "{c}"
            );
        }
    }
}
