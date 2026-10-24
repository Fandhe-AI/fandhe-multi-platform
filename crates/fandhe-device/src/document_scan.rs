//! 書類スキャンの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/document-scan.json`、親 #145、Issue #747）。
//!
//! 扱う範囲は「OS のスキャン UI を出し、返ってきた 1 ページ目の画像を表示する」ことまで。
//! 2 ページ目以降・PDF 化・文字認識・保存・共有は扱わない。対象は iOS（`VisionKit` の
//! `VNDocumentCameraViewController`、実体は #748）のみ。Android は GMS 依存のため保留
//! （OS 別モジュールを置かず `Unsupported`。保留を解くときは dispatch の変更を別 issue で扱う）、
//! macOS・Windows は `no`、Web は対象外。`fandhe-egui-proxy` の effect ドライバ
//! （`device/document_scan.rs`）が本モジュールの trait を呼び、OS 層が実装する。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「スキャンした」（[`DocumentScanOutcome::Scanned`]）/
//!   「キャンセル」（[`DocumentScanOutcome::Cancelled`]。利用者が閉じた。失敗にしない）/
//!   失敗（[`DocumentScanFailure`] の理由別、非対応を含む）
//! - **失敗の理由**（写像表。iOS 欄は #748 への指示）:
//!
//! | 状況 | iOS | 返す値 |
//! | --- | --- | --- |
//! | 非対応 | `isSupported == NO`、または fallback の OS | `Unsupported` |
//! | カメラ権限の拒否・制限 | `AVCaptureDevice` が denied / restricted、またはダイアログで拒否 | `PermissionDenied` |
//! | 0 ページ | `pageCount == 0` | `Empty` |
//! | 画像が上限超過 | — | `TooLarge` |
//! | 画像が 0 バイト・形式不明・復号不可 | — | `Undecodable` |
//! | 前面でない | — | `NotForeground` |
//! | 表示前の期限切れ | Rust の期限 | `TimedOut` |
//! | 表示前の中断（背景遷移等） | — | `Interrupted` |
//! | `didFailWithError`・未知のコード | — | `Internal` |
//! | 利用者が閉じた | `documentCameraViewControllerDidCancel` | `Ok(Cancelled)` |
//! | スキャンした | `didFinishWithScan` | `Ok(Scanned(page))` |
//!
//! - **期限**（契約表 3）: 撮影 UI（権限ダイアログを含む最初の UI）が出るまでの
//!   [`PRESENT_DEADLINE`]。起点は要求を観測した時の `Instant`（単調時計）。UI が出始めた後は
//!   期限を持たない。期限切れと表示開始は同じロックで排他にし、期限切れ・中断を確定した後は
//!   OS に UI を出させない。値は仮で、実機確認（#739）で見直す
//! - **同時 1 件と世代**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と
//!   画面の世代の両方で捨てる
//! - **成功値の条件**（契約表 6）: `Ok(Scanned)` と `Ok(Cancelled)` は表示開始（bridge の
//!   `try_begin_present`）を記録した要求にだけ認める。記録のない要求の成功値は `Internal`。
//!   成功として見せてよいのは 2 段の検査（[`ScannedPage::from_os`] と橋渡し層での復号）を
//!   通った画像だけ
//! - **OS 層の義務**（契約表 1・5・9）:
//!   - delegate の 3 経路（完了・キャンセル・失敗）すべてで撮影 UI を閉じる
//!   - 結果の受け渡し領域（`NSData` 等）は成功・失敗・キャンセルの全経路で解放する
//!   - `complete_from_os` は 1 要求につき 1 回だけ呼ぶ
//!   - UI の提示は main queue で行う
//!   - 権限の応答時にアプリが前面になければ、UI を出さず `Interrupted` を返す
//!   - 結果を共有状態へ入れた直後に `crate::ios::request_repaint` を呼ぶ
//!   - `UIImage` は JPEG（または PNG）へ変換して渡す
//!   - `imageOfPageAtIndex:` の前に `pageCount` を確かめる
//! - **秘密値**（契約表 8）: 画像の中身と OS のエラー文をログ・画面のエラー文・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（非対応・権限拒否・0 ページ・
//! 復号できない画像を成功にしない）、撮影 UI を閉じることと受け渡し領域の解放を全経路で行うこと。
//! 守らない範囲: 撮影 UI の表示中に Rust 側から閉じること、OS ごとの補正結果の違い、複数プロセス、
//! プロセスが落ちた後の結果、2 ページ目以降。

use std::fmt;
use std::time::Duration;

use crate::DeviceError;
use crate::photo_picker::{PickedPhoto, validate_photo};

/// 要求を観測してから撮影 UI を出すまでの期限（Rust のドライバだけが持つ）。
/// 仮の値で、実機確認（#739）で見直す。
pub const PRESENT_DEADLINE: Duration = Duration::from_secs(10);

/// 1 ページの画像のバイト数上限。写真選択の上限（SSOT）を参照し、値を複製しない。
pub const MAX_PAGE_BYTES: usize = crate::photo_picker::MAX_PHOTO_BYTES;

/// スキャンした 1 ページ目。[`ScannedPage::from_os`] を通ったものだけが作れる。
/// `Debug` は画像のバイト列を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct ScannedPage {
    photo: PickedPhoto,
    page_count: u32,
}

impl ScannedPage {
    /// OS から届いた 1 ページ目のバイト列とページ数を検査する（2 段検査の 1 段目）。
    ///
    /// - `page_count == 0` は画像の有無にかかわらず `Empty`
    /// - 上限超過は `TooLarge`
    /// - 0 バイト・形式不明は `Undecodable`
    ///
    /// `Ok` は「復号できる」ことを意味しない。橋渡し層が表示前に復号して確かめる。
    pub fn from_os(bytes: Vec<u8>, page_count: u32) -> Result<ScannedPage, DocumentScanFailure> {
        if page_count == 0 {
            return Err(DocumentScanFailure::Empty);
        }
        match validate_photo(bytes) {
            Ok(photo) => Ok(ScannedPage { photo, page_count }),
            Err(DeviceError::InvalidInput(_)) => Err(DocumentScanFailure::TooLarge),
            Err(_) => Err(DocumentScanFailure::Undecodable),
        }
    }

    /// 1 ページ目の画像。
    pub fn photo(&self) -> &PickedPhoto {
        &self.photo
    }

    /// スキャン全体のページ数（1 以上）。
    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    /// 画像を取り出す。
    pub fn into_photo(self) -> PickedPhoto {
        self.photo
    }
}

impl fmt::Debug for ScannedPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScannedPage")
            .field("format", &self.photo.format)
            .field("len", &self.photo.bytes.len())
            .field("page_count", &self.page_count)
            .finish()
    }
}

/// 成功側の結果（失敗ではない 2 値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentScanOutcome {
    /// スキャンした。
    Scanned(ScannedPage),
    /// 利用者が撮影 UI を閉じた（失敗にしない）。
    Cancelled,
}

/// スキャンできなかった理由（画像・OS のエラー文は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentScanFailure {
    /// 非対応 OS・端末。
    Unsupported,
    /// カメラ権限の拒否・制限。
    PermissionDenied,
    /// 0 ページ。
    Empty,
    /// 画像が上限を超えた。
    TooLarge,
    /// 画像が 0 バイト・形式不明・復号できない。
    Undecodable,
    /// 前面でない。
    NotForeground,
    /// 撮影 UI を出すまでの期限切れ。
    TimedOut,
    /// 表示前の中断。
    Interrupted,
    /// 未知のコードなど。
    Internal,
}

/// 書類スキャンの OS 実装。`scan` は「受理」だけを同期で返し、結果は OS 層が
/// `fandhe-egui-proxy` の bridge（`complete_from_os`）へ 1 回だけ非同期に届ける。
pub trait DocumentScanner: Send + Sync {
    /// `request_id` の要求として撮影 UI の表示を OS へ依頼する。
    /// 表示の直前に bridge の `try_begin_present` を呼び、`false` なら表示しない。
    fn scan(&self, request_id: u64) -> Result<(), DocumentScanFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedDocumentScanner;

impl DocumentScanner for UnsupportedDocumentScanner {
    fn scan(&self, _request_id: u64) -> Result<(), DocumentScanFailure> {
        Err(DocumentScanFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_HEAD: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedDocumentScanner.scan(1),
            Err(DocumentScanFailure::Unsupported)
        );
    }

    #[test]
    fn zero_pages_is_empty_even_with_valid_image() {
        assert_eq!(
            ScannedPage::from_os(PNG_HEAD.to_vec(), 0),
            Err(DocumentScanFailure::Empty)
        );
    }

    #[test]
    fn empty_and_unknown_bytes_are_undecodable() {
        assert_eq!(
            ScannedPage::from_os(Vec::new(), 1),
            Err(DocumentScanFailure::Undecodable)
        );
        assert_eq!(
            ScannedPage::from_os(vec![1, 2, 3, 4], 1),
            Err(DocumentScanFailure::Undecodable)
        );
    }

    #[test]
    fn oversize_is_too_large() {
        let mut big = PNG_HEAD.to_vec();
        big.resize(MAX_PAGE_BYTES + 1, 0);
        assert_eq!(
            ScannedPage::from_os(big, 1),
            Err(DocumentScanFailure::TooLarge)
        );
    }

    #[test]
    fn png_and_jpeg_heads_pass_stage_one() {
        let p = ScannedPage::from_os(PNG_HEAD.to_vec(), 3).unwrap();
        assert_eq!(p.page_count(), 3);
        assert_eq!(p.photo().bytes, PNG_HEAD.to_vec());
        assert!(ScannedPage::from_os(vec![0xFF, 0xD8, 0xFF], 1).is_ok());
    }

    #[test]
    fn debug_hides_bytes() {
        let mut bytes = PNG_HEAD.to_vec();
        bytes.extend_from_slice(b"SecretPixels");
        let p = ScannedPage::from_os(bytes, 1).unwrap();
        let d = format!("{p:?} {:?}", DocumentScanOutcome::Scanned(p.clone()));
        assert!(!d.contains("Secret"), "{d}");
        assert!(!d.contains("137"), "{d}");
    }
}
