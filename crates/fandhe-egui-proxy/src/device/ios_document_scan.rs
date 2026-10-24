//! iOS 向け書類スキャン実装（親 #145、Issue #748）。`device/document_scan.rs` の iOS 経路（dispatch）から
//! [`implementation`] が呼ばれ、`ios/fandhe_ios_document_scan.m`（VisionKit
//! `VNDocumentCameraViewController`。`build.rs` が feature `device` 有効時にコンパイルして VisionKit を
//! リンクする）を `unsafe extern "C"` で呼ぶ。`unsafe` はこの FFI 宣言と受け口に閉じ、各所に
//! `// SAFETY:` を書く（coding-rust.md の FFI 例外規定）。契約の SSOT は `fandhe_device::document_scan`。
//!
//! 期限（撮影 UI を出すまでの 10 秒）・世代照合・二重完了の破棄・画像の復号検査は effect ドライバの
//! `bridge` が担い、ここは撮影 UI の表示依頼・結果の受け取り・完了コードの写像だけをする。
//! 契約表との対応:
//! - 表示前の背景遷移は `Interrupted`、受付時に背景なら `NotForeground`（1）。
//! - 表示直前（権限ダイアログより前）に `try_begin_present` で現在の要求かを確かめ、期限切れ後は
//!   表示しない（2・3）。
//! - 完了は `.m` が撮影 UI を閉じた後に 1 回だけ渡す。受け口は結果を渡した直後に
//!   `crate::ios::request_repaint` で次フレームを起こす（4・5）。
//! - 非対応・権限拒否・0 ページ・内部エラー・未知のコードは成功に畳まない（6）。キャンセルは失敗にしない（10）。
//! - 画像は受け口の中で上限を確かめてからコピーし、ポインタを保持しない（9）。
//! - 画像の中身・OS のエラー文はログに出さない（8）。新しい権限・依存は足さない。
//!
//! 守らない範囲: 撮影 UI の表示中に Rust 側から閉じること・OS ごとの補正結果の違い・複数プロセス・
//! プロセス死後の結果。

use std::sync::Arc;

use fandhe_device::document_scan::{
    DocumentScanFailure, DocumentScanOutcome, DocumentScanner, ScannedPage,
};

#[cfg(not(target_os = "ios"))]
use fandhe_device::document_scan::UnsupportedDocumentScanner;
#[cfg(target_os = "ios")]
use fandhe_device::document_scan::MAX_PAGE_BYTES;

// SAFETY: `ios/fandhe_ios_document_scan.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_document_scan_start(request_id: u64) -> i32;
}

type Outcome = Result<DocumentScanOutcome, DocumentScanFailure>;

/// 完了コードを結果へ写す（`.m` 冒頭のコード表と一致させる）。
///
/// - `bytes`: code 0 のときだけ使う。`None` は 0 バイト扱い（`Undecodable`）
/// - `over_limit`: 受け口で上限超過を検出済み（code 0 のときだけ `TooLarge`）
/// - 未知のコードは `Internal`。成功に畳まない
fn outcome_from_ios(
    code: i32,
    bytes: Option<Vec<u8>>,
    over_limit: bool,
    page_count: u32,
) -> Outcome {
    match code {
        0 => {
            if over_limit {
                return Err(DocumentScanFailure::TooLarge);
            }
            ScannedPage::from_os(bytes.unwrap_or_default(), page_count)
                .map(DocumentScanOutcome::Scanned)
        }
        1 => Ok(DocumentScanOutcome::Cancelled),
        2 => Err(DocumentScanFailure::Unsupported),
        3 => Err(DocumentScanFailure::PermissionDenied),
        4 => Err(DocumentScanFailure::Empty),
        5 => Err(DocumentScanFailure::NotForeground),
        6 => Err(DocumentScanFailure::Interrupted),
        _ => Err(DocumentScanFailure::Internal),
    }
}

/// 同期の受理コードを写す。0 = 受理、それ以外は `Internal`。
fn start_result(code: i32) -> Result<(), DocumentScanFailure> {
    match code {
        0 => Ok(()),
        _ => Err(DocumentScanFailure::Internal),
    }
}

#[cfg(target_os = "ios")]
struct IosDocumentScanner;

#[cfg(target_os = "ios")]
impl DocumentScanner for IosDocumentScanner {
    #[allow(unsafe_code)]
    fn scan(&self, request_id: u64) -> Result<(), DocumentScanFailure> {
        // SAFETY: 引数は値渡しの `u64` のみでポインタを渡さない。`.m` は main queue へ積んで戻る。
        let code = unsafe { fandhe_ios_document_scan_start(request_id) };
        start_result(code)
    }
}

/// iOS 用の実装を返す。ホストのテストビルドでは dispatch が呼ばれないため `Unsupported` を返す。
pub(crate) fn implementation() -> Arc<dyn DocumentScanner> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosDocumentScanner)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(UnsupportedDocumentScanner)
    }
}

/// `.m` が撮影 UI を出す直前に呼ぶ。1 = 表示してよい、0 = 表示しない（期限切れ・古い要求）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_document_scan_try_begin_present(request_id: u64) -> i32 {
    i32::from(super::document_scan::try_begin_present(request_id))
}

/// `.m` が結果を 1 回だけ渡す受け口。渡した直後に再描画を要求する（入力がないと egui が止まる）。
///
/// # Safety 前提（`.m` の保証）
///
/// `bytes` は NULL か、`len` バイトが読める領域で、この呼び出しの間だけ有効。戻る前にコピーする。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_document_scan_complete(
    request_id: u64,
    code: i32,
    bytes: *const u8,
    len: usize,
    page_count: u32,
) {
    // 巨大な領域はコピーせずに `TooLarge` にする。code 0 以外では bytes を見ない。
    let over_limit = code == 0 && len > MAX_PAGE_BYTES;
    let copied = if code == 0 && !over_limit && !bytes.is_null() && len > 0 {
        // SAFETY: `.m` は `bytes` が `len` バイト読める有効な領域であることを、この呼び出しの間
        // 保証する（NSData の bytes/length）。直後にコピーし、ポインタを保持しない。
        Some(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec())
    } else {
        None
    };
    super::document_scan::complete_from_os(
        request_id,
        outcome_from_ios(code, copied, over_limit, page_count),
    );
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小の有効な PNG ヘッダ付きバイト列（形式判定を通る）。
    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&[0; 32]);
        v
    }

    fn jpeg_bytes() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(&[0; 32]);
        v
    }

    #[test]
    fn non_scan_codes_map() {
        assert_eq!(
            outcome_from_ios(1, None, false, 0),
            Ok(DocumentScanOutcome::Cancelled)
        );
        let table = [
            (2, DocumentScanFailure::Unsupported),
            (3, DocumentScanFailure::PermissionDenied),
            (4, DocumentScanFailure::Empty),
            (5, DocumentScanFailure::NotForeground),
            (6, DocumentScanFailure::Interrupted),
            (7, DocumentScanFailure::Internal),
        ];
        for (code, expected) in table {
            assert_eq!(outcome_from_ios(code, None, false, 0), Err(expected));
        }
    }

    #[test]
    fn unknown_code_is_internal_never_success() {
        for code in [-1, 8, 99, i32::MIN, i32::MAX] {
            assert_eq!(
                outcome_from_ios(code, Some(jpeg_bytes()), false, 1),
                Err(DocumentScanFailure::Internal)
            );
        }
    }

    #[test]
    fn non_zero_code_ignores_attached_bytes() {
        assert_eq!(
            outcome_from_ios(2, Some(jpeg_bytes()), false, 1),
            Err(DocumentScanFailure::Unsupported)
        );
        assert_eq!(
            outcome_from_ios(1, Some(jpeg_bytes()), true, 1),
            Ok(DocumentScanOutcome::Cancelled)
        );
    }

    #[test]
    fn scanned_requires_valid_image_and_pages() {
        assert_eq!(
            outcome_from_ios(0, Some(Vec::new()), false, 1),
            Err(DocumentScanFailure::Undecodable)
        );
        assert_eq!(
            outcome_from_ios(0, None, false, 1),
            Err(DocumentScanFailure::Undecodable)
        );
        assert_eq!(
            outcome_from_ios(0, Some(b"not an image".to_vec()), false, 1),
            Err(DocumentScanFailure::Undecodable)
        );
        assert_eq!(
            outcome_from_ios(0, Some(jpeg_bytes()), false, 0),
            Err(DocumentScanFailure::Empty)
        );
        assert_eq!(
            outcome_from_ios(0, None, true, 1),
            Err(DocumentScanFailure::TooLarge)
        );
    }

    #[test]
    fn scanned_keeps_page_count() {
        for bytes in [png_bytes(), jpeg_bytes()] {
            match outcome_from_ios(0, Some(bytes), false, 3) {
                Ok(DocumentScanOutcome::Scanned(page)) => assert_eq!(page.page_count(), 3),
                other => panic!("Scanned を期待したが {other:?}"),
            }
        }
    }

    #[test]
    fn start_code_maps() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(DocumentScanFailure::Internal));
        assert_eq!(start_result(-7), Err(DocumentScanFailure::Internal));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        assert_eq!(
            implementation().scan(1),
            Err(DocumentScanFailure::Unsupported)
        );
    }
}
