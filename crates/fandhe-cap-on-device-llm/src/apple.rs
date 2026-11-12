//! macOS / iOS 向け端末内 LLM 実装（Issue #345 macOS、Issue #346 iOS）。
//! `fandhe-cap-on-device-llm/src/driver.rs` の macOS 経路と iOS 経路（`ios_on_device_llm` 経由）から呼ばれ、`apple/fandhe_apple_on_device_llm.swift`
//! （FoundationModels。`build.rs` が feature `device` 有効時に swiftc で静的ライブラリ化）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出し 1 か所に閉じる
//! （coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・`device.rs` の mod cfg と
//! 同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_cap_on_device_llm`。戻りコード → `OnDeviceLlmError` の写像の SSOT は
//! 本ファイルの `match`。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::time::Duration;

use crate::contract::{
    BUSY, MAX_RESPONSE_TOKENS, MODEL_NOT_READY, OnDeviceLlm, REFUSED, RESPONSE_BUF_BYTES, TIMEOUT,
    check_response, validate_prompt,
};
use crate::error::OnDeviceLlmError;

// SAFETY: `apple/fandhe_apple_on_device_llm.swift` が `@_cdecl` で定義する C ABI（`build.rs` が
// macOS ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は swift 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_on_device_llm_respond(
        prompt: *const u8,
        len: usize,
        deadline_ms: u64,
        max_tokens: i32,
        out: *mut u8,
        cap: usize,
        out_len: *mut usize,
    ) -> i32;
}

/// FoundationModels（`SystemLanguageModel`）で応答を得る実装。
pub(crate) struct AppleOnDeviceLlm;

impl OnDeviceLlm for AppleOnDeviceLlm {
    #[allow(unsafe_code)]
    fn respond(&self, prompt: &str, remaining: Duration) -> Result<String, OnDeviceLlmError> {
        validate_prompt(prompt)?;
        let deadline_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        let mut buf = vec![0u8; RESPONSE_BUF_BYTES];
        let mut out_len = 0usize;
        // SAFETY: `prompt`・`buf`・`out_len` は呼び出しの間生存する領域で、Swift は呼び出しの間だけ
        // 参照し保持しない（プロンプトは Swift 側で String へコピーしてから Task へ渡す。期限切れで
        // 戻った後に走る生成は Swift の箱だけを触り、`buf` には書かない）。`out` は `cap`（= `buf.len()`）
        // バイトの書き込み先、`out_len` は 1 要素の書き込み先。
        let code = unsafe {
            fandhe_apple_on_device_llm_respond(
                prompt.as_ptr(),
                prompt.len(),
                deadline_ms,
                MAX_RESPONSE_TOKENS,
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_len,
            )
        };
        match code {
            0 => {
                let bytes = buf
                    .get(..out_len)
                    .ok_or(OnDeviceLlmError::Internal("on_device_llm"))?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| OnDeviceLlmError::Internal("on_device_llm"))?;
                check_response(text)
            }
            1 => Err(OnDeviceLlmError::Unsupported),
            2 => Err(OnDeviceLlmError::Disabled("on_device_llm")),
            3 => Err(OnDeviceLlmError::Internal(MODEL_NOT_READY)),
            4 => Err(OnDeviceLlmError::Internal(REFUSED)),
            5 => Err(OnDeviceLlmError::Internal(BUSY)),
            6 => Err(OnDeviceLlmError::Internal(TIMEOUT)),
            8 => Err(OnDeviceLlmError::InvalidInput),
            _ => Err(OnDeviceLlmError::Internal("on_device_llm")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::contract::RESPOND_DEADLINE;

    #[test]
    fn respond_never_fake_success() {
        match AppleOnDeviceLlm.respond("日本の四季を一文で紹介してください。", RESPOND_DEADLINE)
        {
            Ok(t) => assert!(!t.trim().is_empty()),
            Err(OnDeviceLlmError::Unsupported | OnDeviceLlmError::Disabled(_)) => {}
            Err(OnDeviceLlmError::Internal(id)) => {
                assert!(
                    [MODEL_NOT_READY, BUSY, REFUSED, TIMEOUT].contains(&id),
                    "{id}"
                )
            }
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn swift_entry_rejects_oversized_prompt() {
        let prompt = vec![b'a'; 4097];
        let mut buf = vec![0u8; 16];
        let mut out_len = 0usize;
        // SAFETY: 各領域は呼び出しの間生存する。上限の guard は availability 判定より前で、
        // モデルには触れず 8 を返す。
        let code = unsafe {
            fandhe_apple_on_device_llm_respond(
                prompt.as_ptr(),
                prompt.len(),
                1000,
                MAX_RESPONSE_TOKENS,
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_len,
            )
        };
        assert_eq!(code, 8);
    }

    #[test]
    fn bad_prompt_is_rejected_before_ffi() {
        for bad in ["", "  ", "a\0b"] {
            assert!(matches!(
                AppleOnDeviceLlm.respond(bad, RESPOND_DEADLINE),
                Err(OnDeviceLlmError::InvalidInput)
            ));
        }
    }
}
