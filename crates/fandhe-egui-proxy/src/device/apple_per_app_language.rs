//! macOS / iOS 向けアプリ別の言語設定実装（Issue #1073 が macOS、#1074 が iOS、親 #137）。
//! `device/per_app_language.rs::platform_per_app_language` の macOS 経路（iOS は
//! `ios_per_app_language.rs` 経由）から呼ばれ、`apple/fandhe_apple_per_app_language.m`
//! （`build.rs` が feature `device` 有効時にコンパイル）を C ABI で呼ぶ。
//! `unsafe` は固定長バッファを渡す FFI 1 箇所に閉じる。
//!
//! # 写像
//!
//! `.m` は `Bundle.main.preferredLocalizations` の先頭を NUL 終端の UTF-8 で書く。
//! status 非 0（nil・空・収まらない）・NUL 無し・UTF-8 でない・`clean_tag` で不正は `Internal`
//! （成功にしない、契約表 6）。`UTF8String` が NULL の場合も `.m` は -1 を返し、同じく `Internal`。成功は常に `LanguageSource::Unknown`（Apple にアプリ別 / OS 全体を
//! 区別する公開 API が無い）。値をログへ出さない。
//!
//! 読み取りは同期で購読を持たず、保持する資源も無い（契約表 1・2・3 は該当なし）。

use std::ffi::CStr;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::per_app_language::PerAppLanguage;
use fandhe_device::per_app_language::{clean_tag, AppLanguage, LanguageSource};
use fandhe_device::DeviceError;

const FAILED_MSG: &str = "per_app_language read";
/// `.m` へ渡すバッファ長（NUL 含む）。タグの上限 35 文字に十分な余裕を持たせた値。
const BUF_LEN: usize = 64;

/// `.m` が書いた結果を契約の値へ戻す純関数（ホストの `cargo test` で検査する）。
fn from_raw(status: i32, buf: &[u8; BUF_LEN]) -> Result<AppLanguage, DeviceError> {
    if status != 0 {
        return Err(DeviceError::Internal(FAILED_MSG));
    }
    let raw = CStr::from_bytes_until_nul(buf)
        .ok()
        .and_then(|c| c.to_str().ok())
        .and_then(clean_tag)
        .ok_or(DeviceError::Internal(FAILED_MSG))?;
    Ok(AppLanguage {
        tag: raw,
        source: LanguageSource::Unknown,
    })
}

// SAFETY: `apple/fandhe_apple_per_app_language.m` が定義する C ABI。`out` は呼び出し側が確保した
// `cap` バイト以上の書き込み可能領域で、`.m` は `cap - 1` バイトを超えて書かず NUL 終端する。
// 所有権の移転は無い。
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_per_app_language_read(out: *mut u8, cap: i32) -> i32;
}

/// `Bundle.main.preferredLocalizations` による実装。状態は持たない。
#[cfg(any(target_os = "macos", target_os = "ios"))]
struct ApplePerAppLanguage;

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl PerAppLanguage for ApplePerAppLanguage {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<AppLanguage, DeviceError> {
        let mut buf = [0u8; BUF_LEN];
        // SAFETY: 上記 extern 宣言のとおり。`buf` は `BUF_LEN` バイトの有効な可変領域。
        let status =
            unsafe { fandhe_apple_per_app_language_read(buf.as_mut_ptr(), BUF_LEN as i32) };
        from_raw(status, &buf)
    }
}

/// macOS / iOS 用の実装を返す。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn implementation() -> Box<dyn PerAppLanguage> {
    Box::new(ApplePerAppLanguage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf_of(bytes: &[u8]) -> [u8; BUF_LEN] {
        let mut b = [0u8; BUF_LEN];
        b[..bytes.len()].copy_from_slice(bytes);
        b
    }

    #[test]
    fn valid_tags_are_unknown_source() {
        for (raw, want) in [("ja", "ja"), ("en", "en"), ("zh_Hans", "zh-Hans")] {
            let l = from_raw(0, &buf_of(raw.as_bytes())).unwrap();
            assert_eq!(l.tag, want);
            assert_eq!(l.source, LanguageSource::Unknown);
        }
    }

    #[test]
    fn nonzero_status_is_error() {
        for s in [-1, -2, 1] {
            assert!(matches!(
                from_raw(s, &buf_of(b"ja")),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn malformed_buffers_are_errors() {
        // NUL 無し
        assert!(from_raw(0, &[b'a'; BUF_LEN]).is_err());
        // 空
        assert!(from_raw(0, &[0u8; BUF_LEN]).is_err());
        // UTF-8 でない
        assert!(from_raw(0, &buf_of(&[0xff, 0xfe])).is_err());
        // 不正タグ
        assert!(from_raw(0, &buf_of(b"ja jp")).is_err());
        assert!(from_raw(0, &buf_of("a".repeat(36).as_bytes())).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_read_is_not_unsupported() {
        // テスト実行体の mainBundle の言語は環境依存なので、成功までは求めない。
        let r = implementation().read();
        assert!(!matches!(r, Err(DeviceError::Unsupported(_))), "{r:?}");
    }
}
