//! macOS / iOS 向け単語分割実装（Issue #1094・#1095）。`device/nlp.rs` の macOS / iOS 経路から呼ばれ、
//! `apple/fandhe_apple_nlp.m`（NaturalLanguage `NLTokenizer`。`build.rs` が feature `device`
//! 有効時にコンパイル）を `unsafe extern "C"` 1 関数で呼ぶ。トークンは `.m` から
//! コールバックで 1 件ずつ Rust の `Vec<String>` へ積み、所有権とアロケータは境界を
//! 越えない。`unsafe` はこの呼び出しとコールバックの 2 箇所に閉じる（coding-rust.md の
//! FFI 例外規定準拠）。`build.rs` のコンパイル条件・`device.rs` の mod cfg と同じ OS 集合に
//! 保つ。
//!
//! 契約の SSOT は `fandhe_device::nlp`: OS を呼ぶ前に `validate_input`、空入力は
//! `Ok(vec![])`、得たトークンは必ず `normalize_words` を通す。

use std::ffi::{c_char, c_void, CStr, CString};

use fandhe_device::nlp::{normalize_words, validate_input, WordSegmenter};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_nlp.m` が定義する C ABI（`build.rs` が macOS / iOS ターゲットの
// feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_nlp_segment(
        text: *const c_char,
        ctx: *mut c_void,
        on_token: extern "C" fn(*mut c_void, *const c_char),
    ) -> i32;
}

/// `.m` から同期に呼ばれ、トークンを `ctx`（`&mut Vec<String>`）へ積む。panic しない処理だけにする
/// （unwind を FFI 越しに伝えない）。
#[allow(unsafe_code)]
extern "C" fn push_token(ctx: *mut c_void, token: *const c_char) {
    // SAFETY: `ctx` は `segment` が渡した `&mut Vec<String>` で、`.m` は呼び出しの間だけ
    // 同期に使い保持しない。`token` は NUL 終端の UTF-8 で、このコールバックの間だけ有効。
    let (words, token) = unsafe { (&mut *(ctx as *mut Vec<String>), CStr::from_ptr(token)) };
    words.push(token.to_string_lossy().into_owned());
}

/// NaturalLanguage `NLTokenizer`（単語単位）で分割する実装。
pub(crate) struct AppleWordSegmenter;

impl WordSegmenter for AppleWordSegmenter {
    #[allow(unsafe_code)]
    fn segment(&self, text: &str) -> Result<Vec<String>, DeviceError> {
        validate_input(text)?;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        // NUL は `validate_input` が弾く。万一失敗しても嘘の成功にはしない。
        let c_text = CString::new(text).map_err(|_| DeviceError::InvalidInput("nlp text"))?;
        let mut raw: Vec<String> = Vec::new();
        // SAFETY: `c_text` は NUL 終端で呼び出しの間生存する。`raw` への可変参照は
        // この呼び出しの間だけ `ctx` として渡し、`.m` 側は保持しない。
        let code = unsafe {
            fandhe_apple_nlp_segment(
                c_text.as_ptr(),
                &mut raw as *mut Vec<String> as *mut c_void,
                push_token,
            )
        };
        if code != 0 {
            return Err(DeviceError::Internal("nlp"));
        }
        Ok(normalize_words(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::nlp::MAX_INPUT_CHARS;

    #[test]
    fn over_limit_and_nul_are_invalid_input() {
        let s = AppleWordSegmenter;
        assert!(matches!(
            s.segment(&"a".repeat(MAX_INPUT_CHARS + 1)),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            s.segment("a\0b"),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn empty_and_whitespace_are_empty_success() {
        let s = AppleWordSegmenter;
        assert_eq!(s.segment("").unwrap(), Vec::<String>::new());
        assert_eq!(s.segment("   ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn mixed_text_satisfies_contract_properties() {
        let text = "Hello, world! 日本語の文章です。";
        let words = AppleWordSegmenter.segment(text).unwrap();
        assert!(!words.is_empty());
        for w in &words {
            assert!(!w.is_empty());
            assert!(text.contains(w.as_str()), "{w} は入力の部分文字列");
            assert!(w.chars().any(char::is_alphanumeric));
        }
    }
}
