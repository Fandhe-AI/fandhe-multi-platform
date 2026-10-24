//! 画面のスポイト（色の取得）の契約（Phase 11 の拡充ループ、親 #148、Issue #762）。
//!
//! 利用者が画面上で選んだ 1 点の色を sRGB の `#rrggbb` で返す。呼び出し元は
//! `fandhe-egui-proxy` の effect ドライバ（`device/eye_dropper.rs`、macOS）と、
//! Web 子（#763）の `fandhe-web-dom`。既存 id の color-picker（色選択ダイアログ）とは別機能。
//!
//! # 結果の区別（嘘の成功を作らない）
//!
//! - 選んだ: `Ok(`[`PickedColor`]`)`
//! - 取り消した: `Cancelled("eye_dropper")`。失敗として見せない
//! - 非対応: `Unsupported("eye_dropper")`
//! - 失敗: `Internal(..)`（内部識別子のみ。色・例外の `message` は含めない）
//! - 権限拒否（`PermissionDenied`）は返さない（該当なし）。macOS の `NSColorSampler` は
//!   画面収録の許可を要らず、Web の `NotAllowedError` は失敗として扱う
//!
//! # OS を正にする
//!
//! 対応の有無・ユーザー操作の有無を先に問い合わせず、呼んだ結果で判定する（macOS は nil と
//! sRGB 変換の結果、Web はコンストラクタの有無と例外名）。サンプラーが開いているかを
//! アプリ側で記憶せず、期限も持たない。完了は OS の通知だけで決める。
//!
//! # 要求の扱い
//!
//! - 処理中の再押下で要求を増やさない（画面が `Idle` 以外ではボタンを出さない）
//! - 古い完了は、macOS では request_id、Web では画面の世代で捨てる。完了の多重通知は
//!   first-wins で、成功へ化けない
//!
//! # macOS の注記
//!
//! `showSamplerWithSelectionHandler:` はすぐ戻り、ハンドラは main thread で呼ばれる。
//! nil は取り消し。サンプラーを閉じる公開 API は無い。ハンドラが必ず一度呼ばれるかは
//! 公式に記述がない（守らない範囲）。
//!
//! # Web の注記
//!
//! secure context 限定で、`open()` はユーザー操作の中で呼ぶ。Web 子は画面の破棄時に
//! `AbortController` で中断する。DOM 例外名は [`error_from_dom_exception_name`]、
//! `sRGBHex` は [`color_from_srgb_hex`] で写す。コンストラクタが無い場合の `Unsupported` は
//! Web 子が判定する。
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、取り消し・非対応・不正値・失敗を成功に見せないこと、
//! サンプラーの強参照の解放。守らない: macOS でハンドラが呼ばれないまま残った場合の復旧、
//! sRGB の外の色の正確さ、ブラウザ・OS ごとのサンプリング位置や色管理の差、複数タブ・
//! 複数プロセスでの同時操作。
//!
//! # 情報非保持
//!
//! 拾った色（フィンガープリントに使える）と例外の `message` はログ・エラーメッセージに出さず、
//! 永続化しない。

use crate::DeviceError;

/// 選ばれた色。`rgb` は `0..=0xFF_FFFF` を保証する（0xRRGGBB）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickedColor {
    /// 0xRRGGBB。
    rgb: u32,
}

impl PickedColor {
    /// 0xRRGGBB の値。
    pub fn rgb(&self) -> u32 {
        self.rgb
    }

    /// 小文字の `#rrggbb`。
    pub fn hex(&self) -> String {
        format!("#{:06x}", self.rgb)
    }
}

/// 整数の RGB を契約へ写す。範囲外は `Internal("eye_dropper rgb")`（成功にしない）。
pub fn color_from_rgb(rgb: i64) -> Result<PickedColor, DeviceError> {
    if !(0..=0xFF_FFFF).contains(&rgb) {
        return Err(DeviceError::Internal("eye_dropper rgb"));
    }
    Ok(PickedColor { rgb: rgb as u32 })
}

/// Web の `sRGBHex` を契約へ写す。受理するのは `#` + 16 進 6 桁だけ（大文字小文字は問わない）。
/// trim はせず、届いた文字列のまま判定する。出力は [`PickedColor::hex`] で正規化し直す。
pub fn color_from_srgb_hex(s: &str) -> Result<PickedColor, DeviceError> {
    let err = DeviceError::Internal("eye_dropper hex");
    let Some(digits) = s.strip_prefix('#') else {
        return Err(err);
    };
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err);
    }
    match u32::from_str_radix(digits, 16) {
        Ok(v) => Ok(PickedColor { rgb: v }),
        Err(_) => Err(err),
    }
}

/// `.m` 側の結果コード → 結果の写像（SSOT）。0 = 選択（`rgb` を検査）、1 = 取り消し、
/// 2 = sRGB 変換の失敗・成分が NaN、3 = 起動できなかった。未知コードは `Internal`
/// （偽の取り消し・偽の成功を作らない）。
pub fn result_from_macos_code(code: i32, rgb: i32) -> Result<PickedColor, DeviceError> {
    match code {
        0 => color_from_rgb(i64::from(rgb)),
        1 => Err(DeviceError::Cancelled("eye_dropper")),
        2 => Err(DeviceError::Internal("eye_dropper convert")),
        3 => Err(DeviceError::Internal("eye_dropper present")),
        _ => Err(DeviceError::Internal("eye_dropper outcome")),
    }
}

/// Web の DOM 例外名 → 契約のエラー。`AbortError` は取り消し、それ以外は `Internal`
/// （識別子は名前ごとに分け、`message` は使わない）。
pub fn error_from_dom_exception_name(name: &str) -> DeviceError {
    match name {
        "AbortError" => DeviceError::Cancelled("eye_dropper"),
        "NotAllowedError" => DeviceError::Internal("eye_dropper not allowed"),
        "InvalidStateError" => DeviceError::Internal("eye_dropper invalid state"),
        "OperationError" => DeviceError::Internal("eye_dropper operation"),
        _ => DeviceError::Internal("eye_dropper exception"),
    }
}

/// スポイトの契約。同期 API（コールバック型 OS は `fandhe-egui-proxy` の pull 型 bridge を使い
/// このトレイトを通らない。`ContactPicker` と同じ分担）。
pub trait EyeDropper: Send + Sync {
    /// スポイトを 1 回開き、選ばれた色を返す。取り消しは `Err(Cancelled)`。
    fn pick(&self) -> Result<PickedColor, DeviceError>;
}

/// 対象外 OS 用。ダミーの成功実装へは倒さない。
pub struct UnsupportedEyeDropper;

impl EyeDropper for UnsupportedEyeDropper {
    fn pick(&self) -> Result<PickedColor, DeviceError> {
        Err(DeviceError::Unsupported("eye_dropper"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_normalized_to_lowercase() {
        let c = color_from_srgb_hex("#AABBCC").unwrap();
        assert_eq!(c.hex(), "#aabbcc");
        assert_eq!(color_from_rgb(0x00_00FF).unwrap().hex(), "#0000ff");
    }

    #[test]
    fn invalid_hex_is_never_success() {
        for s in [
            "",
            "#",
            "#abc",
            "#aabbccdd",
            "aabbcc",
            " #aabbcc",
            "#aabbcc ",
            "#gggggg",
            "#あいう",
            "##aabbc",
            "#+abbcc",
        ] {
            assert!(
                matches!(
                    color_from_srgb_hex(s),
                    Err(DeviceError::Internal("eye_dropper hex"))
                ),
                "{s:?}"
            );
        }
    }

    #[test]
    fn rgb_bounds() {
        assert_eq!(color_from_rgb(0).unwrap().rgb(), 0);
        assert_eq!(color_from_rgb(0xFF_FFFF).unwrap().rgb(), 0xFF_FFFF);
        for bad in [0x100_0000, -1, i64::MIN, i64::MAX] {
            assert!(matches!(
                color_from_rgb(bad),
                Err(DeviceError::Internal("eye_dropper rgb"))
            ));
        }
    }

    #[test]
    fn macos_code_table() {
        assert_eq!(
            result_from_macos_code(0, 0x12_3456).unwrap().hex(),
            "#123456"
        );
        assert!(matches!(
            result_from_macos_code(0, -1),
            Err(DeviceError::Internal("eye_dropper rgb"))
        ));
        assert!(matches!(
            result_from_macos_code(1, 0),
            Err(DeviceError::Cancelled("eye_dropper"))
        ));
        assert!(matches!(
            result_from_macos_code(2, 0),
            Err(DeviceError::Internal("eye_dropper convert"))
        ));
        assert!(matches!(
            result_from_macos_code(3, 0),
            Err(DeviceError::Internal("eye_dropper present"))
        ));
        for code in [-1, 4, 99] {
            assert!(matches!(
                result_from_macos_code(code, 0x12_3456),
                Err(DeviceError::Internal("eye_dropper outcome"))
            ));
        }
    }

    #[test]
    fn dom_exception_names() {
        assert!(matches!(
            error_from_dom_exception_name("AbortError"),
            DeviceError::Cancelled("eye_dropper")
        ));
        for n in [
            "NotAllowedError",
            "InvalidStateError",
            "OperationError",
            "TypeError",
            "",
        ] {
            assert!(matches!(
                error_from_dom_exception_name(n),
                DeviceError::Internal(_)
            ));
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedEyeDropper.pick(),
            Err(DeviceError::Unsupported("eye_dropper"))
        ));
    }
}
