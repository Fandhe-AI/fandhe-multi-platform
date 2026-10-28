//! macOS 向け印刷の実装（Issue #325）。`macos/fandhe_macos_print.m`（`NSPrintPanel` +
//! `NSPrintOperation` の 2 段構成、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/print.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::print`。見出し・本文は `PrintRequest` で検査済みのものだけを渡す。
//!
//! 画面の内容の印刷（print-screen、Issue #1010）は `fandhe_macos_print_image`（`NSBitmapImageRep` を
//! `NSImageView` に入れ、縦横比を保って印刷可能領域に収める）。パネルと結果コードは文章と共用する。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 成功は `Submitted`（OS の印刷系へジョブを渡した。PDF 保存を含む）に留める。紙に出たかは不明
//! - 2 段構成でパネルの PDF 保存が効かない場合は Mac 目視（#324）で分かる。その場合は 1 回の
//!   `NSPrintOperation` に戻す（NO を `Cancelled` に畳まない）

use fandhe_device::print::PrintOutcome;
use fandhe_device::DeviceError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = Submitted / 1 = Cancelled / 2 = 提示不可 / 3 = 実行失敗。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<PrintOutcome, DeviceError> {
    match code {
        0 => Ok(PrintOutcome::Submitted),
        1 => Ok(PrintOutcome::Cancelled),
        2 => Err(DeviceError::Internal("print present")),
        3 => Err(DeviceError::Internal("print failed")),
        _ => Err(DeviceError::Internal("print outcome")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;

    // SAFETY: `macos/fandhe_macos_print.m`（`build.rs` が macOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_print_text(
            title: *const std::ffi::c_char,
            body: *const std::ffi::c_char,
        ) -> i32;
    }

    // SAFETY: 上と同じ。`.m` は rgba を呼び出し中だけ読み、`len` を再検査する。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_print_image(rgba: *const u8, len: usize, width: u32, height: u32) -> i32;
    }

    /// 画面の画像を渡して結果コードを返す。
    #[allow(unsafe_code)]
    pub(super) fn print_image(img: &fandhe_device::print::PrintImage) -> i32 {
        let rgba = img.rgba();
        // SAFETY: `rgba` は `PrintImage` の不変条件（`len == width * height * 4`、各辺 1〜8192）を満たし、
        // この呼び出しの間だけ生きる。`.m` はポインタを呼び出し中にだけ読み、保持しない。
        unsafe { fandhe_macos_print_image(rgba.as_ptr(), rgba.len(), img.width(), img.height()) }
    }

    /// 結果コードを返す。`CString` を作れなければ 2（提示不可と同じ扱い。検査で NUL は
    /// 弾かれるため到達しない想定）。
    #[allow(unsafe_code)]
    pub(super) fn print_text(title: &str, body: &str) -> i32 {
        let (Ok(t), Ok(b)) = (CString::new(title), CString::new(body)) else {
            return 2;
        };
        // SAFETY: `t` / `b` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを
        // 呼び出し中にだけ読み、保持しない。
        unsafe { fandhe_macos_print_text(t.as_ptr(), b.as_ptr()) }
    }
}

#[cfg(target_os = "macos")]
struct MacPrint;

#[cfg(target_os = "macos")]
impl fandhe_device::print::Print for MacPrint {
    fn print(&self, req: &fandhe_device::print::PrintRequest) -> Result<PrintOutcome, DeviceError> {
        outcome_from_code(ffi::print_text(req.title(), req.body()))
    }

    fn print_image(
        &self,
        img: &fandhe_device::print::PrintImage,
    ) -> Result<PrintOutcome, DeviceError> {
        outcome_from_code(ffi::print_image(img))
    }
}

/// macOS の `Print` 実装を返す（`device/print.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::print::Print> {
    Box::new(MacPrint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert_eq!(outcome_from_code(0).unwrap(), PrintOutcome::Submitted);
        assert_eq!(outcome_from_code(1).unwrap(), PrintOutcome::Cancelled);
        for code in [2, 3, -1, 4, i32::MAX] {
            assert!(outcome_from_code(code).is_err());
        }
    }
}
