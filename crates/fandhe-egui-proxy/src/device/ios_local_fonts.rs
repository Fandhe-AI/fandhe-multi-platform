//! iOS 向け OS 搭載フォント一覧実装（Issue #1377）。`ios/fandhe_ios_local_fonts.m`
//! （UIKit `UIFont.familyNames`。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/local_fonts.rs` の iOS 経路（`drive_sync`）から UI スレッドで呼ばれ、契約は
//! `fandhe_device::local_fonts`。バッファの解釈は macOS と共用の `families_from_nul_buffer`。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: ファミリー名以外を取らない（書体名は読まない）。名前をログに出さない。OS が 1 件以上返したのに
//!   名前が 1 件も残らなければ `Internal`。バッファに収まらなければ `truncated` で正直に出す
//! - 守らない: ダウンロード型・利用者が別経路で入れたフォントが載るか、読み取り後の追加・削除、
//!   バッファ超過時に昇順の先頭が残る決定性
//!
//! # 再描画
//!
//! 同期の 1 回読みで OS コールバックが無く `crate::ios::request_repaint` は呼ばない。結果は
//! `CompletionSender::send` が注入直後に `ctx.request_repaint()` で次フレームを起こす。
//!
//! # 既知の天井
//!
//! ponytail: 受け取りバッファは固定 1 MiB（macOS と同じ。iOS の搭載ファミリーは百件規模で溢れない見込み）。
//! 溢れても `.m` が途中で止まり `truncated` が立つ。溢れる実機があれば OS の返す必要長で取り直す。

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::local_fonts::{LocalFontFamilies, LocalFonts};
    use fandhe_device::DeviceError;

    use crate::device::local_fonts::families_from_nul_buffer;

    // SAFETY: `ios/fandhe_ios_local_fonts.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_local_fonts_list(
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            out_overflowed: *mut i32,
        ) -> i32;
    }

    /// 成功なら `(書いたバイト数, OS が返した件数, 溢れたか)`。
    #[allow(unsafe_code)]
    fn list(buf: &mut [u8]) -> Option<(usize, u32, bool)> {
        let (mut len, mut count, mut overflowed) = (0usize, 0u32, 0i32);
        // SAFETY: `buf` は `cap` バイトの有効な排他領域、残りの 3 つは呼び出しの間有効なローカル変数への
        // 排他ポインタで、`.m` はどれも保持しない。
        let rc = unsafe {
            fandhe_ios_local_fonts_list(
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                &mut overflowed,
            )
        };
        (rc == 0 && len <= buf.len()).then_some((len, count, overflowed != 0))
    }

    struct IosLocalFonts;

    impl LocalFonts for IosLocalFonts {
        fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
            let mut buf = vec![0u8; 1 << 20];
            let (len, count, overflowed) =
                list(&mut buf).ok_or(DeviceError::Internal("local_fonts list"))?;
            families_from_nul_buffer(&buf[..len], count, overflowed)
        }
    }

    /// iOS の `LocalFonts` 実装を返す（`device/local_fonts.rs` の dispatch から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn LocalFonts> {
        Arc::new(IosLocalFonts)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;
