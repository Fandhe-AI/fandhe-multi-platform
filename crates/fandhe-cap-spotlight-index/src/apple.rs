//! macOS / iOS 向け OS の検索（Core Spotlight）への登録実装（Issue #560 macOS、iOS は #561）。
//! `apple/fandhe_apple_spotlight_index.m` の C ABI を呼ぶ薄いラッパーで、
//! `driver::platform_spotlight_index` から [`implementation`] 経由で使われる。契約（結果の
//! 種類・期限・守る範囲）の SSOT は `crate::contract`。
//!
//! macOS は `.app` バンドルでないプロセス（`cargo run` の素の実行）では Core Spotlight が使えないため
//! 失敗（`Internal`）を返し、成功を装わない。開かれたときの受信は [`register`] が足す。macOS は
//! `NSApp.delegate` へ `continueUserActivity` を足し、iOS は scene の通知を受ける
//! （`ios` 参照）。どちらも `fandhe-cap-url-scheme` の受け口（`apple.rs`）へ流す。
//! 識別子・題名・本文・OS のエラー文はログへ出さない。

use std::sync::Arc;
use std::time::Duration;

use crate::contract::{DOMAIN_ID, IndexItem, SpotlightIndex, TIMEOUT};
use crate::error::SpotlightIndexError;

// SAFETY: `apple/fandhe_apple_spotlight_index.m` が定義する C ABI（`build.rs` がコンパイル・リンク）。
// 文字列は UTF-8 のポインタと長さで、呼び出し中だけ有効。`.m` 側は同期でコピーし保持しない。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_spotlight_index(
        id: *const u8,
        id_len: usize,
        title: *const u8,
        title_len: usize,
        text: *const u8,
        text_len: usize,
        domain: *const u8,
        domain_len: usize,
        timeout_ms: i64,
    ) -> i32;
    fn fandhe_apple_spotlight_remove_all(
        domain: *const u8,
        domain_len: usize,
        timeout_ms: i64,
    ) -> i32;
    fn fandhe_apple_spotlight_register(prefix: *const u8, prefix_len: usize);
}

/// `.m` の戻り値を契約上の結果へ写す（SSOT）。0 以外は成功にしない。
pub(crate) fn status_from(code: i32) -> Result<(), SpotlightIndexError> {
    match code {
        0 => Ok(()),
        -2 => Err(SpotlightIndexError::InvalidInput("spotlight_index")),
        -4 => Err(SpotlightIndexError::Unsupported("spotlight_index")),
        -6 => Err(SpotlightIndexError::Internal(TIMEOUT)),
        _ => Err(SpotlightIndexError::Internal("spotlight_index")),
    }
}

/// 残り時間を ms にする。0 は期限切れとして扱う（FFI へ渡さない）。上限はここで丸める。
fn timeout_ms(remaining: Duration) -> Result<i64, SpotlightIndexError> {
    let ms = remaining.as_millis().min(i64::MAX as u128) as i64;
    if ms == 0 {
        Err(SpotlightIndexError::Internal(TIMEOUT))
    } else {
        Ok(ms)
    }
}

/// Core Spotlight の `SpotlightIndex` 実装。
pub(crate) struct AppleSpotlightIndex;

impl SpotlightIndex for AppleSpotlightIndex {
    #[allow(unsafe_code)]
    fn index(&self, item: &IndexItem<'_>, remaining: Duration) -> Result<(), SpotlightIndexError> {
        let ms = timeout_ms(remaining)?;
        // SAFETY: 各ポインタは本呼び出しの間だけ有効な `&str` 由来で、長さは各バイト数と一致する。
        // `.m` 側は同期でコピーして保持しない。ワーカースレッドからの呼び出しを想定した関数。
        let code = unsafe {
            fandhe_apple_spotlight_index(
                item.id.as_ptr(),
                item.id.len(),
                item.title.as_ptr(),
                item.title.len(),
                item.text.as_ptr(),
                item.text.len(),
                DOMAIN_ID.as_ptr(),
                DOMAIN_ID.len(),
                ms,
            )
        };
        status_from(code)
    }

    #[allow(unsafe_code)]
    fn remove_all(&self, remaining: Duration) -> Result<(), SpotlightIndexError> {
        let ms = timeout_ms(remaining)?;
        // SAFETY: `DOMAIN_ID` は 'static な `&str` で、長さと一致する。`.m` 側は同期でコピーする。
        let code =
            unsafe { fandhe_apple_spotlight_remove_all(DOMAIN_ID.as_ptr(), DOMAIN_ID.len(), ms) };
        status_from(code)
    }
}

/// `driver::platform_spotlight_index`（macOS）、`ios`（#561）が使う実装。
pub(crate) fn implementation() -> Arc<dyn SpotlightIndex> {
    Arc::new(AppleSpotlightIndex)
}

/// 検索結果から開かれたときの受信を登録する。main thread から呼ぶ（macOS は `run_native` より前、
/// iOS は `UIApplicationMain` より前）。URL の接頭辞 `prefix`（例: `fandhe://spotlight/`）は呼び出し側のアプリが渡す。冪等。
#[allow(unsafe_code)]
pub fn register(prefix: &'static str) {
    // SAFETY: `prefix` は 'static な `&str` で、長さと一致する。`.m` 側は同期でコピーして保持する。
    // main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_apple_spotlight_register(prefix.as_ptr(), prefix.len()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_codes_never_become_ok() {
        for code in [-1, -2, -3, -4, -5, -6, -7, 1, 99] {
            assert!(status_from(code).is_err(), "{code}");
        }
        assert!(status_from(0).is_ok());
    }

    #[test]
    fn status_mapping_is_distinct() {
        assert!(matches!(
            status_from(-4),
            Err(SpotlightIndexError::Unsupported(_))
        ));
        assert!(matches!(
            status_from(-6),
            Err(SpotlightIndexError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            status_from(-2),
            Err(SpotlightIndexError::InvalidInput(_))
        ));
        assert!(matches!(
            status_from(-5),
            Err(SpotlightIndexError::Internal(_))
        ));
    }

    #[test]
    fn zero_remaining_is_timeout_without_calling_os() {
        let item = IndexItem {
            id: "demo-memo",
            title: "t",
            text: "x",
        };
        assert!(matches!(
            AppleSpotlightIndex.index(&item, Duration::ZERO),
            Err(SpotlightIndexError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            AppleSpotlightIndex.remove_all(Duration::ZERO),
            Err(SpotlightIndexError::Internal(TIMEOUT))
        ));
    }
}
