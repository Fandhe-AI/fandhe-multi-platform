//! iOS 向け印刷（Issue #327）。`device/print.rs` の iOS 経路（`begin_print`）から呼ばれ、
//! `ios/fandhe_ios_print.m`（`UIPrintInteractionController`）を `unsafe extern "C"` 2 関数（文章・画像）で呼ぶ。
//! `unsafe` はこの呼び出しと結果コールバックに閉じる。画像（print-screen・#1011）は `.m` が
//! return 前に画素をコピーして `CGImage` → `UIImage` を `printingItem` に渡す。
//!
//! 結果は `.m` の completionHandler から [`fandhe_rust_print_outcome`] が呼ばれ、
//! `print::bridge`（pull 型・`RequestSlot`）へ置く。コード表は `print::outcome_from_ios_code`。
//!
//! 契約表の対応: 1（背景遷移で取り消さない）、2（holder の `done` と `request_id` 照合で二重・古い通知を捨てる）、
//! 4（コールバック直後に `request_repaint`）、5（main queue・`@autoreleasepool`・completion 1 回）、
//! 6（error・提示失敗・印刷不可・未知コードを成功に畳まない）、10（取り消しは `Cancelled`）。
//! 守らない範囲: 印刷シートを開いた後の利用者の操作、背景遷移中のシートの扱い（OS に任せる）、紙に出たか。
//! 見出し・本文・画素・NSError はログへ出さない。

use fandhe_device::print::{PrintImage, PrintRequest};
use fandhe_device::DeviceError;

// SAFETY: `ios/fandhe_ios_print.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_print_present(
        request_id: u64,
        title: *const std::os::raw::c_char,
        body: *const std::os::raw::c_char,
    ) -> i32;
    fn fandhe_ios_print_present_image(
        request_id: u64,
        rgba: *const u8,
        len: usize,
        width: u32,
        height: u32,
    ) -> i32;
}

/// 印刷シートの表示を依頼する。`Err` は依頼自体が受理されなかった（呼び出し元がその場で
/// `deliver` する）。`Ok` なら結果は後で [`fandhe_rust_print_outcome`] が届ける。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_print(request_id: u64, req: &PrintRequest) -> Result<(), DeviceError> {
    use std::ffi::CString;
    const PRESENT: DeviceError = DeviceError::Internal("print present");
    // `PrintRequest` が NUL を含む制御文字を弾いているため通常は失敗しない。
    let (Ok(title), Ok(body)) = (CString::new(req.title()), CString::new(req.body())) else {
        return Err(PRESENT);
    };
    // SAFETY: 2 つの `CString` は呼び出しが返るまで生存する NUL 終端の有効な領域。`.m` 側は
    // return 前に `NSString` へコピーするため、所有権は移らず返却後は参照されない。
    let code = unsafe { fandhe_ios_print_present(request_id, title.as_ptr(), body.as_ptr()) };
    if code == 0 {
        Ok(())
    } else {
        Err(PRESENT)
    }
}

/// ホストのテストビルドでは実体が無いため非対応を返す。
#[cfg(not(target_os = "ios"))]
pub(crate) fn begin_print(_request_id: u64, _req: &PrintRequest) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("print"))
}

/// 画面の画像（print-screen、親 #341）の印刷シート表示を依頼する。`Err` / `Ok` の意味は
/// [`begin_print`] と同じ。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_print_image(request_id: u64, img: &PrintImage) -> Result<(), DeviceError> {
    let rgba = img.rgba();
    // SAFETY: `PrintImage` の不変条件（各辺 1〜8192、`rgba.len() == width*height*4`）により、
    // `rgba` は呼び出しが返るまで生存する有効な領域。`.m` 側は return 前に `NSData` へコピーし、
    // 返却後は参照しない。
    let code = unsafe {
        fandhe_ios_print_present_image(
            request_id,
            rgba.as_ptr(),
            rgba.len(),
            img.width(),
            img.height(),
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("print present"))
    }
}

/// ホストのテストビルドでは実体が無いため非対応を返す。
#[cfg(not(target_os = "ios"))]
#[allow(dead_code)]
pub(crate) fn begin_print_image(_request_id: u64, _img: &PrintImage) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("print"))
}

/// `.m` 側の completionHandler から呼ばれる。コードの解釈は `print::outcome_from_ios_code`。
/// 結果を置いた直後に次フレームを起こす（egui は入力が無いと描画せず、画面が処理中のまま止まるため）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_print_outcome(request_id: u64, code: i32) {
    super::print::bridge::deliver(request_id, super::print::outcome_from_ios_code(code));
    crate::ios::request_repaint();
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;

    #[test]
    fn host_is_unsupported() {
        let req = PrintRequest::new("t", "b").unwrap();
        assert!(matches!(
            begin_print(1, &req),
            Err(DeviceError::Unsupported("print"))
        ));
        let img = PrintImage::new(vec![0; 4], 1, 1).unwrap();
        assert!(matches!(
            begin_print_image(1, &img),
            Err(DeviceError::Unsupported("print"))
        ));
    }
}
