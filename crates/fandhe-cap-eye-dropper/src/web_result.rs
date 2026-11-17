//! Web の `EyeDropper.open()` の結果（resolve した `sRGBHex`・reject した例外名）を `Result` へ写す純関数。
//! 呼び出し元は `web.rs`（wasm32）。ブラウザ API に触れないため全ターゲットでテストできる。
//! 契約（`sRGBHex` の検査・例外名の写像）は `fandhe_device::eye_dropper` が SSOT で、本モジュールは
//! その `DeviceError` を機能 crate の `EyeDropperError` へ写すだけ。拾った色・例外の `message` は読まない・出さない。

use fandhe_device::eye_dropper::{PickedColor, color_from_srgb_hex, error_from_dom_exception_name};

use crate::error::EyeDropperError;

/// resolve 値の `sRGBHex` から結果を作る（trim しない）。取れない・形式不正は失敗（嘘の成功にしない）。
pub(crate) fn result_from_resolved(hex: Option<&str>) -> Result<PickedColor, EyeDropperError> {
    match hex {
        Some(h) => color_from_srgb_hex(h).map_err(EyeDropperError::from_device),
        None => Err(EyeDropperError::Internal("eye_dropper hex")),
    }
}

/// reject の例外 `name` から失敗を作る。`AbortError` だけが `Cancelled`。名前が無ければ `Internal`。
pub(crate) fn error_from_name(name: Option<&str>) -> EyeDropperError {
    match name {
        Some(n) => EyeDropperError::from_device(error_from_dom_exception_name(n)),
        None => EyeDropperError::Internal("eye_dropper exception"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_hex_is_normalized_and_picked() {
        assert_eq!(
            result_from_resolved(Some("#AABBCC")).map(|c| c.hex()),
            Ok("#aabbcc".to_string())
        );
    }

    #[test]
    fn invalid_hex_never_picked() {
        for h in [
            None,
            Some("#abc"),
            Some(" #aabbcc"),
            Some("aabbcc"),
            Some("#aabbcc "),
        ] {
            assert!(
                matches!(result_from_resolved(h), Err(EyeDropperError::Internal(_))),
                "{h:?}"
            );
        }
    }

    #[test]
    fn only_abort_error_is_cancelled() {
        assert_eq!(
            error_from_name(Some("AbortError")),
            EyeDropperError::Cancelled
        );
        for n in [
            Some("NotAllowedError"),
            Some("InvalidStateError"),
            Some("OperationError"),
            Some("Whatever"),
            None,
        ] {
            assert!(
                matches!(error_from_name(n), EyeDropperError::Internal(_)),
                "{n:?}"
            );
        }
    }
}
