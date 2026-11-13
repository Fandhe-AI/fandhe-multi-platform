//! 機能 crate「書類スキャン」（document-scan）。契約（本ファイル）・effect ドライバ（`driver`）・
//! iOS の FFI（`ios`、`ios/*.m`）・Android の JNI（`android`、`android/java/*`）・機能宣言
//! （`Cargo.toml` の `[package.metadata.fandhe]`）をこの crate に持つ（TASK-21.3 / Issue #1171、
//! 構造の設計は `docs/architecture/consumer-app-structure.md`）。
//! 利用側は [`document_scan_show_effect`] を画面（`fandhe_core::screens::document_scan_show`）の
//! effect として登録する。Activity 再生成では Android の入口から [`reset_pending_tracking`] を呼ぶ。
//! egui・winit・`fandhe-device` には依存しない。
//!
//! 書類スキャンの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/document-scan.json`、親 #145、Issue #747）。
//!
//! 扱う範囲は「OS のスキャン UI を出し、返ってきた 1 ページ目の画像を表示する」ことまで。
//! 2 ページ目以降・PDF 化・文字認識・保存・共有は扱わない。対象は iOS（`VisionKit` の
//! `VNDocumentCameraViewController`、実体は #748）と Android（ML Kit Document Scanner、
//! 実体は #1125。#1124 ではスタブ）。macOS・Windows は `no`、Web は対象外。effect ドライバ
//! （`driver.rs`）が本モジュールの trait を呼び、OS 層が実装する。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「スキャンした」（[`DocumentScanOutcome::Scanned`]）/
//!   「キャンセル」（[`DocumentScanOutcome::Cancelled`]。利用者が閉じた。失敗にしない）/
//!   失敗（[`DocumentScanFailure`] の理由別、非対応を含む）
//! - **失敗の理由**（写像表。iOS 欄は #748 への指示）:
//!
//! | 状況 | iOS | Android | 返す値 |
//! | --- | --- | --- | --- |
//! | 非対応 | `isSupported == NO`、または fallback の OS | GMS 判定（`isGooglePlayServicesAvailable`）が SUCCESS 以外、または `getStartScanIntent` の失敗が `MlKitException.UNSUPPORTED`（低 RAM 等） | `Unsupported` |
//! | カメラ権限の拒否・制限 | `AVCaptureDevice` が denied / restricted、またはダイアログで拒否 | 出さない（撮影 UI とカメラは Play services 側が扱う） | `PermissionDenied` |
//! | 0 ページ | `pageCount == 0` | `RESULT_OK` で結果が null、またはページが 0 | `Empty` |
//! | 画像が上限超過 | — | 受け口で `MAX_PAGE_BYTES` を超える | `TooLarge` |
//! | 画像が 0 バイト・形式不明・復号不可 | — | URI を読めない、または復号できない | `Undecodable` |
//! | 前面でない | — | 受付時に Activity が RESUMED でない | `NotForeground` |
//! | 表示前の期限切れ | Rust の期限 | Rust の期限 | `TimedOut` |
//! | 表示前の中断（背景遷移等） | — | `onDestroy` の時点で要求が保留中 | `Interrupted` |
//! | 内部エラー・未知のコード | `didFailWithError` 等 | その他の例外・未知のコード・launch の失敗 | `Internal` |
//! | 利用者が閉じた | `documentCameraViewControllerDidCancel` | `RESULT_CANCELED` | `Ok(Cancelled)` |
//! | スキャンした | `didFinishWithScan` | `RESULT_OK` と 1 ページ目 | `Ok(Scanned(page))` |
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
//! - **OS 層の義務**（契約表 1・5・9）。共通:
//!   - 結果の受け渡し領域は成功・失敗・キャンセルの全経路で解放し、`complete_from_os` は 1 要求につき
//!     1 回だけ呼ぶ。画像は長辺 2048 画素以下に縮めて JPEG で渡す。画像と OS の文言はログに出さない
//! - **OS 層の義務** iOS:
//!   - delegate の 3 経路（完了・キャンセル・失敗）すべてで撮影 UI を閉じる
//!   - 結果の受け渡し領域（`NSData` 等）は成功・失敗・キャンセルの全経路で解放する
//!   - `complete_from_os` は 1 要求につき 1 回だけ呼ぶ
//!   - UI の提示は main queue で行う
//!   - 権限の応答時にアプリが前面になければ、UI を出さず `Interrupted` を返す
//!   - 結果を共有状態へ入れた直後に `fandhe_runtime::request_repaint` を呼ぶ
//!   - `UIImage` は長辺 2048 画素以下に縮めてから JPEG へ変換して渡す（親 #145 の実装方針。
//!     もともと 2048 以下なら描き直さない）。縮小は OS 層の義務で、Rust 側では縮めない
//!   - 結果は撮影 UI が閉じたのを確かめてから返す（遷移中は遷移の完了を待って閉じる）
//!   - 提示の完了コールバックが猶予（5 秒）内に来なければ `Internal` で確定する。この失敗だけは
//!     撮影 UI が出ていない前提で先に返し、遅れて出た UI は提示の完了後に閉じる
//!   - `imageOfPageAtIndex:` の前に `pageCount` を確かめる
//! - **OS 層の義務** Android（#1125 が trait doc を変えずに実装する）:
//!   - `try_begin_present` が true のときだけ launch する。`getStartScanIntent` の Task は main executor で受ける
//!   - オプションは `setGalleryImportAllowed(false)`・`setPageLimit(1)`・JPEG。更新ダイアログは出さない
//!   - 結果の処理（読み込み・縮小・`Bitmap` の recycle）は main 以外の単一スレッドで行う
//!   - ページのファイルは読み終えた後、自アプリの `cacheDir` 配下にあるときだけ消す（他所は消さない）。
//!     消せなかったら結果を返さず `Internal`（契約表 9）
//!   - 期限の既知の天井: `PRESENT_DEADLINE`（10 秒）には GMS 判定と `getStartScanIntent`
//!     （初回のモジュールダウンロードを含む）が入る。初回は `TimedOut` になりうる
//! - **秘密値**（契約表 8）: 画像の中身と OS のエラー文をログ・画面のエラー文・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（非対応・権限拒否・0 ページ・
//! 復号できない画像を成功にしない）、撮影 UI を閉じることと受け渡し領域の解放を全経路で行うこと。
//! 守らない範囲: 撮影 UI の表示中に Rust 側から閉じること、OS ごとの補正結果の違い、複数プロセス、
//! プロセスが落ちた後の結果、2 ページ目以降、Play services 側 UI の挙動、GMS 非搭載端末での
//! 公式文書にない挙動、初回モジュールダウンロードの所要時間。

// unsafe は iOS の FFI（`ios.rs`）と Android の JNI エクスポート（`android.rs`）だけ。
// それ以外の OS では禁止し、モバイルでは SAFETY コメント付きの最小範囲にだけ `allow` する。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

use std::fmt;
use std::time::Duration;

mod driver;
pub use driver::{
    ScanResult, document_scan_show_effect, platform_document_scan, poll_scan,
    reset_pending_tracking, start_scan,
};

// OS 別の実体。ホストのテストでも写像（純関数）を検査するため `test` でも載せる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

/// 要求を観測してから撮影 UI を出すまでの期限（Rust のドライバだけが持つ）。
/// 仮の値で、実機確認（#739）で見直す。
pub const PRESENT_DEADLINE: Duration = Duration::from_secs(10);

/// 1 ページの画像のバイト数上限。本 crate を正とし、Android へは `DocumentScanHelper.scan` の引数で渡す
/// （Java 側に複製しない）。値は写真選択の上限（8 MiB）と同じ。
// ponytail: 写真選択（fandhe-device）と同じ値・同じ復号上限を別々に持つ。共有の置き場は #1183 で決める。
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// 1 ページ目の画像形式。OS 層は JPEG（Android・iOS とも）か PNG で渡す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFormat {
    /// JPEG。
    Jpeg,
    /// PNG。
    Png,
}

/// スキャンした 1 ページ目。[`ScannedPage::from_os`] を通ったものだけが作れる。
/// `Debug` は画像のバイト列を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct ScannedPage {
    bytes: Vec<u8>,
    format: PageFormat,
    page_count: u32,
}

impl ScannedPage {
    /// OS から届いた 1 ページ目のバイト列とページ数を検査する（2 段検査の 1 段目）。
    ///
    /// - `page_count == 0` は画像の有無にかかわらず `Empty`
    /// - 上限超過は `TooLarge`
    /// - 0 バイト・形式不明は `Undecodable`（上限超過以外の `InvalidInput` を含む）
    ///
    /// `Ok` は「復号できる」ことを意味しない。橋渡し層が表示前に復号して確かめる。
    pub fn from_os(bytes: Vec<u8>, page_count: u32) -> Result<ScannedPage, DocumentScanFailure> {
        if page_count == 0 {
            return Err(DocumentScanFailure::Empty);
        }
        // 上限超過だけを `TooLarge` にする。0 バイト・JPEG/PNG 以外（GIF/WebP を含む。
        // 本 crate は復号しない）は復号できない画像として扱う。
        if bytes.len() > MAX_PAGE_BYTES {
            return Err(DocumentScanFailure::TooLarge);
        }
        let format = match image::guess_format(&bytes) {
            Ok(image::ImageFormat::Jpeg) => PageFormat::Jpeg,
            Ok(image::ImageFormat::Png) => PageFormat::Png,
            _ => return Err(DocumentScanFailure::Undecodable),
        };
        Ok(ScannedPage {
            bytes,
            format,
            page_count,
        })
    }

    /// 1 ページ目の画像のバイト列。
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 1 ページ目の画像形式（先頭バイトで判定済み）。
    pub fn format(&self) -> PageFormat {
        self.format
    }

    /// スキャン全体のページ数（1 以上）。
    pub fn page_count(&self) -> u32 {
        self.page_count
    }
}

impl fmt::Debug for ScannedPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScannedPage")
            .field("format", &self.format)
            .field("len", &self.bytes.len())
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
/// `driver` の bridge（`complete_from_os`）へ 1 回だけ非同期に届ける。
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
    fn invalid_input_other_than_size_is_undecodable() {
        // 上限以内で形式不明（`InvalidInput` になりうる入力）は `TooLarge` にしない。
        assert_eq!(
            ScannedPage::from_os(vec![0u8; 16], 1),
            Err(DocumentScanFailure::Undecodable)
        );
        let mut at_limit = vec![0u8; MAX_PAGE_BYTES];
        at_limit[0] = 1;
        assert_eq!(
            ScannedPage::from_os(at_limit, 1),
            Err(DocumentScanFailure::Undecodable)
        );
    }

    #[test]
    fn png_and_jpeg_heads_pass_stage_one() {
        let p = ScannedPage::from_os(PNG_HEAD.to_vec(), 3).unwrap();
        assert_eq!(p.page_count(), 3);
        assert_eq!(p.bytes(), PNG_HEAD.as_slice());
        assert_eq!(p.format(), PageFormat::Png);
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
