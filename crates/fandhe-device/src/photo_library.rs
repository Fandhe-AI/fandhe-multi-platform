//! 画像を 1 枚、写真ライブラリへ保存する機能の抽象（Phase 11 の拡充ループ、親 #1121、Issue #1254）。
//!
//! 範囲は「アプリが作った PNG 1 枚を保存するだけ」。アルバムの作成・一覧・削除は扱わない。
//! iOS / macOS は `PHPhotoLibrary`（add-only 認可）、Android（#1256）は `MediaStore`、
//! Windows は Pictures フォルダへのファイル書き込み（[`FolderPhotoLibrary`]）で保存する。
//! iOS / macOS / Android は OS がコールバック型のため [`PhotoLibrary`] を通らず、
//! `fandhe_core::screens::photo_save`（端末依存ゼロの状態機械）へ pull 型 `bridge` で書き戻す
//! （`calendar` と同じ判断）。それ以外は [`UnsupportedPhotoLibrary`]。
//!
//! # 結果の種類
//!
//! 成功は [`SavedTo`]（保存先の種類）だけ。利用者がキャンセルする UI が無いため
//! `Cancelled` は返さない。権限ダイアログでの拒否は失敗ではなく「権限拒否」として案内する。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 保存確定（`performChanges` 成功、MediaStore の `IS_PENDING=0` 確定、書き込みと sync の完了） | `Ok(SavedTo::…)` |
//! | 権限拒否・制限（restricted 含む）、Android 24〜28 の実行時権限拒否 | `PermissionDenied("photo_library")` |
//! | 入力不正（PNG でない・空・上限超過） | `InvalidInput("photo_library png")` |
//! | 書き込み・OS 呼び出しの失敗（ライブラリ未設定、iCloud 側の失敗を含む） | `Internal("photo_library …")` |
//! | 非対応（Linux・Web、macOS 11 / iOS 14 未満、まだスタブの OS） | `Unsupported("photo_library")` |
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の要求と完了の順序（`request_id` と画面の世代の照合）、保存確定以外を
//! 「保存しました」と見せないこと、完了を 1 回だけ反映すること、失敗時に作りかけのファイルを
//! 消す（消せなければ `Internal("photo_library cleanup")` で区別して返す）こと、入力不正（[`PngImage::new`] を通らない値）を OS へ渡さないこと。
//!
//! 守らない: 複数プロセスからの同時保存、保存後に利用者が写真 App やエクスプローラで編集・
//! 削除すること、iCloud 写真や OneDrive リダイレクトの同期完了（ローカル保存の確定までを成功
//! とする）、保存中のアプリ終了（OS が片付ける。Windows で書き込み途中に強制終了されると
//! 部分ファイルが残りうる既知の制約）。
//!
//! 固定タイムアウトは掛けない（TCC の許可ダイアログは利用者の応答を待つため）。
//! 画像のバイト列・保存パスはログにも画面にも出さない（security.md A09）。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::DeviceError;

/// 受け付ける PNG の最大バイト数（OS へ渡す前の資源枯渇対策、security.md A04）。
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// 検査済みの PNG。[`PngImage::new`] 以外では作れない（検査の SSOT、
/// `calendar::EventDraft::parse` と同じ方式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngImage(Vec<u8>);

impl PngImage {
    /// 空・PNG シグネチャ不一致・[`MAX_PNG_BYTES`] 超過を `InvalidInput("photo_library png")`
    /// として弾く（復号の成否までは見ない。OS 側が最終判断する）。
    pub fn new(bytes: Vec<u8>) -> Result<Self, DeviceError> {
        if bytes.is_empty() || bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(&PNG_SIGNATURE) {
            return Err(DeviceError::InvalidInput("photo_library png"));
        }
        Ok(Self(bytes))
    }

    /// PNG のバイト列。
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// 保存先の種類。画面の完了文言の出し分けに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedTo {
    /// iOS / macOS の写真ライブラリ。
    PhotoLibrary,
    /// Android の MediaStore（ギャラリー）。
    MediaStore,
    /// Windows の Pictures フォルダ。
    PicturesFolder,
}

/// 同期に保存できる実装向けの窓口（Windows）。コールバック型 OS は bridge を使う。
pub trait PhotoLibrary: Send + Sync {
    /// PNG を 1 枚保存し、確定した保存先を返す。契約はモジュール doc の表。
    fn save_png(&self, image: &PngImage) -> Result<SavedTo, DeviceError>;
}

/// 非対応プラットフォーム用。常に `Unsupported("photo_library")`。
pub struct UnsupportedPhotoLibrary;

impl PhotoLibrary for UnsupportedPhotoLibrary {
    fn save_png(&self, _image: &PngImage) -> Result<SavedTo, DeviceError> {
        Err(DeviceError::Unsupported("photo_library"))
    }
}

/// 指定フォルダへ `fandhe-photo-<UNIX ミリ秒>.png` を書く実装（Windows の Pictures 用。
/// フォルダの解決は `fandhe-egui-proxy` の `windows_photo_library` が担う）。
/// 既存ファイルは上書きせず、衝突したら `-1`〜`-9` を付けて再試行する。
pub struct FolderPhotoLibrary {
    dir: PathBuf,
}

impl FolderPhotoLibrary {
    /// 保存先フォルダを指定する（存在しなければ保存時に `Internal`）。
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// 書き込み処理を差し替えられる本体（テストで書き込み失敗を注入するため）。
    fn write_new_with(
        &self,
        bytes: &[u8],
        write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> Result<(), DeviceError> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        for attempt in 0..10u8 {
            let name = if attempt == 0 {
                format!("fandhe-photo-{millis}.png")
            } else {
                format!("fandhe-photo-{millis}-{attempt}.png")
            };
            let path = self.dir.join(name);
            // create_new で既存ファイルの上書きを防ぐ（security.md A01）。
            let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(map_io(&e, true)),
            };
            // Windows は開いたままのファイルを消せないため、ブロックを抜けてハンドルを閉じてから消す。
            let result = {
                let mut file = file;
                write(&mut file, bytes)
            };
            if let Err(e) = result {
                // 作りかけのファイルを残さない（契約表 9）。削除まで失敗したら元の失敗とは
                // 区別して返す（Pictures に部分ファイルが残ったことを隠さない）。
                if fs::remove_file(&path).is_err() {
                    return Err(DeviceError::Internal("photo_library cleanup"));
                }
                return Err(map_io(&e, false));
            }
            return Ok(());
        }
        Err(DeviceError::Internal("photo_library name"))
    }
}

/// io エラーを契約上の種別へ写す。パス・OS の詳細は載せない（A09）。
fn map_io(e: &io::Error, opening: bool) -> DeviceError {
    match e.kind() {
        io::ErrorKind::PermissionDenied => DeviceError::PermissionDenied("photo_library"),
        io::ErrorKind::NotFound if opening => DeviceError::Internal("photo_library folder"),
        _ => DeviceError::Internal("photo_library write"),
    }
}

impl PhotoLibrary for FolderPhotoLibrary {
    fn save_png(&self, image: &PngImage) -> Result<SavedTo, DeviceError> {
        self.write_new_with(image.bytes(), |f, b| {
            f.write_all(b)?;
            f.sync_all()
        })?;
        Ok(SavedTo::PicturesFolder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> PngImage {
        let mut b = PNG_SIGNATURE.to_vec();
        b.extend_from_slice(b"body");
        PngImage::new(b).unwrap()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fandhe-photolib-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn png_image_rejects_empty_non_png_and_oversize() {
        let mut big = PNG_SIGNATURE.to_vec();
        big.resize(MAX_PNG_BYTES + 1, 0);
        for bad in [vec![], b"not png".to_vec(), big] {
            assert!(matches!(
                PngImage::new(bad),
                Err(DeviceError::InvalidInput("photo_library png"))
            ));
        }
        assert!(PngImage::new(PNG_SIGNATURE.to_vec()).is_ok());
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedPhotoLibrary.save_png(&png()),
            Err(DeviceError::Unsupported("photo_library"))
        ));
    }

    #[test]
    fn folder_saves_exact_bytes_and_never_overwrites() {
        let dir = tmp("save");
        let lib = FolderPhotoLibrary::new(dir.clone());
        for _ in 0..3 {
            assert!(matches!(lib.save_png(&png()), Ok(SavedTo::PicturesFolder)));
        }
        let files: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(files.len(), 3);
        for f in files {
            assert_eq!(fs::read(f.unwrap().path()).unwrap(), png().bytes());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_is_internal_and_leaves_nothing() {
        let dir = std::env::temp_dir().join("fandhe-photolib-missing-dir-xyz");
        let _ = fs::remove_dir_all(&dir);
        let lib = FolderPhotoLibrary::new(dir.clone());
        assert!(matches!(
            lib.save_png(&png()),
            Err(DeviceError::Internal("photo_library folder"))
        ));
        assert!(!dir.exists());
    }

    #[test]
    fn write_failure_is_internal_and_removes_partial_file() {
        let dir = tmp("fail");
        let lib = FolderPhotoLibrary::new(dir.clone());
        let r = lib.write_new_with(png().bytes(), |f, _| {
            f.write_all(b"partial")?;
            Err(io::Error::other("injected"))
        });
        assert!(matches!(
            r,
            Err(DeviceError::Internal("photo_library write"))
        ));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    // Windows はハンドルを保持している間その名前を差し替えられないため、この失敗の作り方は使えない。
    #[cfg(not(windows))]
    #[test]
    fn cleanup_failure_is_reported_separately() {
        let dir = tmp("cleanup");
        let lib = FolderPhotoLibrary::new(dir.clone());
        // 書き込み失敗時に部分ファイルを消せない状況を、ファイルをディレクトリへ差し替えて作る。
        let d2 = dir.clone();
        let r = lib.write_new_with(png().bytes(), move |_, _| {
            let target = fs::read_dir(&d2).unwrap().next().unwrap().unwrap().path();
            fs::remove_file(&target).unwrap();
            fs::create_dir(&target).unwrap();
            Err(io::Error::other("injected"))
        });
        assert!(matches!(
            r,
            Err(DeviceError::Internal("photo_library cleanup"))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn io_errors_map_to_contract() {
        let perm = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(matches!(
            map_io(&perm, true),
            DeviceError::PermissionDenied("photo_library")
        ));
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert!(matches!(
            map_io(&nf, false),
            DeviceError::Internal("photo_library write")
        ));
    }
}
