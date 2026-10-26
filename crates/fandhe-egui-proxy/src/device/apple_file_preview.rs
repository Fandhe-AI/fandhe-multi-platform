//! macOS / iOS 向けサムネイル実装（Issue #917 macOS、iOS は b-ios が共用）。
//! `device/file_preview.rs` の macOS 経路（と `ios_file_preview`）から呼ばれ、QuickLookThumbnailing 実装
//! （`apple/fandhe_apple_file_preview.m`、`build.rs` が feature `device` 有効時のみコンパイル・
//! リンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこのモジュールの extern 宣言と
//! 呼び出し 1 か所に閉じ `// SAFETY:` で契約を明記する（coding-rust.md の FFI 例外規定準拠）。
//!
//! 固定ファイルを `temp_dir()/fandhe-file-preview/` へ書き、そのパスだけを `.m` へ渡す（外から
//! 任意のパスを受け取らない）。結果コード → 結果の写像は
//! `fandhe_device::file_preview::thumbnail_from_apple_code` に一本化する。出力バッファは Rust が
//! 確保して容量を渡し、戻り値の寸法は `check_thumbnail` で再検査する。パス・画素・OS のエラーは
//! ログへ出さない（security.md A09）。
//!
//! 同期呼び出しで、`.m` は期限で `cancelRequest:` してから戻る。呼び出し側（ワーカー）から直接呼ぶ。
//! iOS ではまだ使われない（b-ios が `ios_file_preview` から返す）ため `dead_code` を許す。

#![cfg_attr(target_os = "ios", allow(dead_code))]

use std::ffi::{c_char, CString};
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use fandhe_device::file_preview::{
    check_thumbnail, thumbnail_from_apple_code, write_fixed, FileKind, FileThumbnailer,
    ThumbnailResult, DIR_NAME, SIZE, THUMBNAIL_MAX_SIDE, WRITE,
};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_file_preview.m` が定義する C ABI。契約は `.m` 冒頭コメントと
// 下記 `thumbnail` の SAFETY コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_file_thumbnail(
        path: *const c_char,
        max_side: u32,
        timeout_ms: u32,
        out: *mut u8,
        out_cap: usize,
        w: *mut u32,
        h: *mut u32,
    ) -> i32;
}

/// QuickLookThumbnailing による [`FileThumbnailer`]。
pub struct AppleFileThumbnailer;

impl FileThumbnailer for AppleFileThumbnailer {
    #[allow(unsafe_code)]
    fn thumbnail(
        &self,
        kind: FileKind,
        bytes: &[u8],
        max_side: u32,
        timeout: Duration,
    ) -> Result<ThumbnailResult, DeviceError> {
        if bytes.is_empty() || max_side == 0 || max_side > THUMBNAIL_MAX_SIDE {
            return Err(DeviceError::InvalidInput("file_preview input"));
        }
        let path = write_fixed(&std::env::temp_dir().join(DIR_NAME), kind, bytes)?;
        let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
            let _ = std::fs::remove_file(&path);
            return Err(DeviceError::Internal(WRITE));
        };
        let side = max_side as usize;
        let Some(cap) = side.checked_mul(side).and_then(|n| n.checked_mul(4)) else {
            let _ = std::fs::remove_file(&path);
            return Err(DeviceError::Internal(SIZE));
        };
        let mut buf = vec![0u8; cap];
        let (mut w, mut h) = (0u32, 0u32);
        let timeout_ms = timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
        // SAFETY: `cpath` は NUL 終端の有効な C 文字列で、`.m` は呼び出しの間だけ読む。`buf` は
        // `cap` バイトの確保済み領域で、`.m` は `w*h*4 <= out_cap` を確かめてから書く。`w` / `h` は
        // 呼び出しの間だけ有効なローカルへの書き込み先。`.m` は期限後の completion でも `buf` に触れない。
        let code = unsafe {
            fandhe_apple_file_thumbnail(
                cpath.as_ptr(),
                max_side,
                timeout_ms,
                buf.as_mut_ptr(),
                buf.len(),
                &mut w,
                &mut h,
            )
        };
        // 同期呼び出しで OS は参照を終えているため、結果の判定より前に一時ファイルを消す。
        let _ = std::fs::remove_file(&path);
        if !thumbnail_from_apple_code(code)? {
            return Ok(ThumbnailResult::NoThumbnail);
        }
        let used = (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= buf.len())
            .ok_or(DeviceError::Internal(SIZE))?;
        buf.truncate(used);
        check_thumbnail(w, h, buf)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::device::file_preview::sample_bytes;

    const SECS_10: Duration = Duration::from_secs(10);

    // 実 QuickLook を呼ぶホストテスト。`.m` は main を要求しない（完了ハンドラは別キュー）。

    #[test]
    fn sample_png_yields_a_thumbnail() {
        let r = AppleFileThumbnailer
            .thumbnail(
                FileKind::SampleImage,
                &sample_bytes(FileKind::SampleImage),
                THUMBNAIL_MAX_SIDE,
                SECS_10,
            )
            .expect("サンプル画像のサムネイルは作れる");
        match r {
            ThumbnailResult::Thumbnail(i) => {
                assert!((1..=THUMBNAIL_MAX_SIDE).contains(&i.width));
                assert!((1..=THUMBNAIL_MAX_SIDE).contains(&i.height));
            }
            other => panic!("サムネイルのはず: {other:?}"),
        }
    }

    #[test]
    fn unknown_file_has_no_thumbnail() {
        let r = AppleFileThumbnailer.thumbnail(
            FileKind::Unknown,
            &sample_bytes(FileKind::Unknown),
            THUMBNAIL_MAX_SIDE,
            SECS_10,
        );
        assert!(matches!(r, Ok(ThumbnailResult::NoThumbnail)), "{r:?}");
    }

    #[test]
    fn invalid_arguments_fail_without_calling_os() {
        for side in [0, THUMBNAIL_MAX_SIDE + 1] {
            let r = AppleFileThumbnailer.thumbnail(FileKind::Unknown, &[1], side, SECS_10);
            assert!(matches!(r, Err(DeviceError::InvalidInput(_))));
        }
        let r = AppleFileThumbnailer.thumbnail(FileKind::Unknown, &[], THUMBNAIL_MAX_SIDE, SECS_10);
        assert!(matches!(r, Err(DeviceError::InvalidInput(_))));
    }
}
