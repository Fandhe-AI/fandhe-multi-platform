//! macOS / iOS 向け端末内翻訳実装（Issue #535 macOS、iOS は #536）。
//! `device/translation.rs` の macOS 経路（と #536 以降の iOS 経路）から呼ばれ、
//! `apple/fandhe_apple_translation.swift`（Translation。`build.rs` が feature `device` 有効時に
//! swiftc で静的ライブラリ化）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出し
//! 1 か所に閉じる（coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・
//! `device.rs` の mod cfg と同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::translation`。戻りコード → `DeviceError` の写像の SSOT は
//! 本ファイルの `match`。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::time::Duration;

use fandhe_device::translation::{
    check_result, validate_request, TextTranslator, LANGUAGE_UNSUPPORTED, NOT_INSTALLED, TIMEOUT,
    TRANSLATION_BUF_BYTES,
};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_translation.swift` が `@_cdecl` で定義する C ABI（`build.rs` が
// macOS・iOS ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は swift 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_translation_translate(
        text: *const u8,
        len: usize,
        src: *const u8,
        src_len: usize,
        dst: *const u8,
        dst_len: usize,
        deadline_ms: u64,
        out: *mut u8,
        cap: usize,
        out_len: *mut usize,
    ) -> i32;
}

/// Translation フレームワークで訳す実装。
pub(crate) struct AppleTextTranslator;

impl TextTranslator for AppleTextTranslator {
    #[allow(unsafe_code)]
    fn translate(
        &self,
        text: &str,
        source: &str,
        target: &str,
        remaining: Duration,
    ) -> Result<String, DeviceError> {
        validate_request(text, source, target)?;
        let deadline_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        let mut buf = vec![0u8; TRANSLATION_BUF_BYTES];
        let mut out_len = 0usize;
        // SAFETY: `text`・`source`・`target`・`buf`・`out_len` は呼び出しの間生存する領域で、Swift は
        // 呼び出しの間だけ参照し保持しない（Swift 側で String へコピーしてから Task へ渡す。期限切れで
        // 戻った後に走る翻訳は Swift の箱だけを触り、`buf` には書かない）。`out` は `cap`（= `buf.len()`）
        // バイトの書き込み先、`out_len` は 1 要素の書き込み先。
        let code = unsafe {
            fandhe_apple_translation_translate(
                text.as_ptr(),
                text.len(),
                source.as_ptr(),
                source.len(),
                target.as_ptr(),
                target.len(),
                deadline_ms,
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_len,
            )
        };
        match code {
            0 => {
                let bytes = buf
                    .get(..out_len)
                    .ok_or(DeviceError::Internal("translation"))?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| DeviceError::Internal("translation"))?;
                check_result(text)
            }
            1 => Err(DeviceError::Unsupported("translation")),
            2 => Err(DeviceError::Internal(LANGUAGE_UNSUPPORTED)),
            3 => Err(DeviceError::Internal(NOT_INSTALLED)),
            4 => Err(DeviceError::Internal(TIMEOUT)),
            6 => Err(DeviceError::InvalidInput("translation")),
            _ => Err(DeviceError::Internal("translation")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::translation::TRANSLATE_DEADLINE;

    /// 実呼び出しテストの期限。テスト実行体の main thread は runloop を回さず、Translation の
    /// 問い合わせが返らないことがあるため（実アプリでは winit の runloop が回る）、短くして
    /// 期限切れ（Timeout）を許容する。
    const TEST_DEADLINE: Duration = Duration::from_secs(3);

    const TEXT: &str = "今日は良い天気です。";

    /// 翻訳の実呼び出し。Ok なら空でない訳文、Err なら契約上の失敗のどれか。
    fn assert_honest(r: Result<String, DeviceError>) {
        match r {
            Ok(t) => assert!(!t.trim().is_empty()),
            Err(DeviceError::Unsupported(_)) => {}
            Err(DeviceError::Internal(id)) => {
                assert!(
                    [NOT_INSTALLED, LANGUAGE_UNSUPPORTED, TIMEOUT].contains(&id),
                    "{id}"
                )
            }
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
    }

    #[test]
    fn ja_to_en_never_fake_success() {
        assert_honest(AppleTextTranslator.translate(TEXT, "ja", "en", TEST_DEADLINE));
    }

    #[test]
    fn unknown_target_is_never_success() {
        let r = AppleTextTranslator.translate(TEXT, "ja", "zz", TEST_DEADLINE);
        assert!(r.is_err(), "{r:?}");
        assert_honest(r);
    }

    #[test]
    #[allow(unsafe_code)]
    fn swift_entry_rejects_oversized_text() {
        let text = vec![b'a'; 4097];
        let mut buf = vec![0u8; 16];
        let mut out_len = 0usize;
        // SAFETY: 各領域は呼び出しの間生存する。上限の guard は availability 判定より前で、
        // 翻訳には触れず 6 を返す。
        let code = unsafe {
            fandhe_apple_translation_translate(
                text.as_ptr(),
                text.len(),
                b"ja".as_ptr(),
                2,
                b"en".as_ptr(),
                2,
                1000,
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_len,
            )
        };
        assert_eq!(code, 6);
    }

    #[test]
    fn bad_input_is_rejected_before_ffi() {
        for (t, s, d) in [
            ("", "ja", "en"),
            ("a\0b", "ja", "en"),
            (TEXT, "ja_JP", "en"),
        ] {
            assert!(matches!(
                AppleTextTranslator.translate(t, s, d, TRANSLATE_DEADLINE),
                Err(DeviceError::InvalidInput(_))
            ));
        }
    }
}
