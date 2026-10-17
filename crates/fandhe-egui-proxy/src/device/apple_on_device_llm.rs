//! macOS / iOS 向け端末内 LLM 実装（Issue #345 macOS、iOS は b-ios が使う）。
//! `device/on_device_llm.rs` の macOS 経路から呼ばれ、`apple/fandhe_apple_on_device_llm.swift`
//! （FoundationModels。`build.rs` が feature `device` 有効時に swiftc で静的ライブラリ化）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出し 1 か所に閉じる
//! （coding-rust.md の FFI 例外規定準拠）。`build.rs` のコンパイル条件・`device.rs` の mod cfg と
//! 同じ OS 集合に保つ。
//!
//! 契約の SSOT は `fandhe_device::on_device_llm`。戻りコード → `DeviceError` の写像の SSOT は
//! 本ファイルの `match`。呼び出しはブロックするため、呼び出し元はワーカーで呼ぶ。

use std::time::Duration;

use fandhe_device::on_device_llm::{
    check_response, validate_prompt, OnDeviceLlm, BUSY, MAX_RESPONSE_TOKENS, MODEL_NOT_READY,
    REFUSED, RESPONSE_BUF_BYTES, TIMEOUT,
};
use fandhe_device::DeviceError;

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
    fn respond(&self, prompt: &str, remaining: Duration) -> Result<String, DeviceError> {
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
                    .ok_or(DeviceError::Internal("on_device_llm"))?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| DeviceError::Internal("on_device_llm"))?;
                check_response(text)
            }
            1 => Err(DeviceError::Unsupported("on_device_llm")),
            2 => Err(DeviceError::Disabled("on_device_llm")),
            3 => Err(DeviceError::Internal(MODEL_NOT_READY)),
            4 => Err(DeviceError::Internal(REFUSED)),
            5 => Err(DeviceError::Internal(BUSY)),
            6 => Err(DeviceError::Internal(TIMEOUT)),
            8 => Err(DeviceError::InvalidInput("on_device_llm prompt")),
            _ => Err(DeviceError::Internal("on_device_llm")),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use fandhe_device::on_device_llm::RESPOND_DEADLINE;

    #[test]
    fn respond_never_fake_success() {
        match AppleOnDeviceLlm.respond("日本の四季を一文で紹介してください。", RESPOND_DEADLINE)
        {
            Ok(t) => assert!(!t.trim().is_empty()),
            Err(DeviceError::Unsupported(_) | DeviceError::Disabled(_)) => {}
            Err(DeviceError::Internal(id)) => {
                assert!(
                    [MODEL_NOT_READY, BUSY, REFUSED, TIMEOUT].contains(&id),
                    "{id}"
                )
            }
            Err(e) => panic!("想定外の失敗: {e:?}"),
        }
    }

    #[test]
    fn bad_prompt_is_rejected_before_ffi() {
        for bad in ["", "  ", "a\0b"] {
            assert!(matches!(
                AppleOnDeviceLlm.respond(bad, RESPOND_DEADLINE),
                Err(DeviceError::InvalidInput(_))
            ));
        }
    }
}
