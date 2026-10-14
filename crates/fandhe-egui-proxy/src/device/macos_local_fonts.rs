//! macOS 向け OS 搭載フォント一覧実装（Issue #1376）。`macos/fandhe_macos_local_fonts.m`
//! （`NSFontManager.availableFontFamilies`。`build.rs` が macOS 分岐で feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で
//! 契約を明記する。`device/local_fonts.rs` の macOS 経路（`drive_sync`）から UI スレッドで呼ばれ、
//! 契約は `fandhe_device::local_fonts`。
//!
//! バッファの解釈は `device/local_fonts.rs` の `families_from_nul_buffer`（iOS と共用）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: ファミリー名以外を取らない。名前をログに出さない。OS が 1 件以上返したのに名前が 1 件も
//!   残らなければ `Internal`（`require_named`）。バッファに収まらなかったら `truncated` で正直に出す
//! - 守らない: バッファ超過時に昇順の先頭が残る決定性（下記の天井）。読み取り後のフォント追加・削除
//!
//! # 既知の天井
//!
//! ponytail: 受け取りバッファは固定 1 MiB（名前 1 件の上限 4095 バイトを空きの判定に使うため、末尾の約 4 KiB は使わない）。超えると `.m` が途中で止まり、
//! OS の返す順の先頭だけを渡すので昇順の先頭 [`fandhe_device::local_fonts::MAX_FAMILIES`] 件とは
//! ずれる（`truncated` は立つ）。実機で溢れるなら、OS が返す必要長で取り直す。

#[cfg(target_os = "macos")]
use super::local_fonts::families_from_nul_buffer;
#[cfg(target_os = "macos")]
use fandhe_device::local_fonts::LocalFontFamilies;
#[cfg(target_os = "macos")]
use fandhe_device::DeviceError;

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_local_fonts.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_local_fonts_list(
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            out_overflowed: *mut i32,
        ) -> i32;
    }

    /// 成功なら `(書いたバイト数, OS が返した件数, 溢れたか)`。
    #[allow(unsafe_code)]
    pub(super) fn list(buf: &mut [u8]) -> Option<(usize, u32, bool)> {
        let (mut len, mut count, mut overflowed) = (0usize, 0u32, 0i32);
        // SAFETY: `buf` は `cap` バイトの有効な排他領域、残りの 3 つは呼び出しの間有効なローカル変数への
        // 排他ポインタで、`.m` はどれも保持しない。
        let rc = unsafe {
            fandhe_macos_local_fonts_list(
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                &mut overflowed,
            )
        };
        (rc == 0 && len <= buf.len()).then_some((len, count, overflowed != 0))
    }
}

#[cfg(target_os = "macos")]
struct MacLocalFonts;

#[cfg(target_os = "macos")]
impl fandhe_device::local_fonts::LocalFonts for MacLocalFonts {
    fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
        let mut buf = vec![0u8; 1 << 20];
        let (len, count, overflowed) =
            ffi::list(&mut buf).ok_or(DeviceError::Internal("local_fonts list"))?;
        families_from_nul_buffer(&buf[..len], count, overflowed)
    }
}

/// macOS の `LocalFonts` 実装を返す（`device/local_fonts.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::local_fonts::LocalFonts> {
    std::sync::Arc::new(MacLocalFonts)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn real_os_call_succeeds() {
        assert!(implementation().list_families().is_ok());
    }
}
