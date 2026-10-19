//! ファイルの関連付け（OS から渡されたファイルをアプリで開く）の受け口（Phase 11 の拡充ループ、
//! 親 #102、Issue #416）。
//!
//! 扱う範囲は「OS が渡した 1 件のファイルを検査して読み、名前・サイズ・先頭
//! [`HEAD_BYTES`] バイトだけを保持して画面が読めるようにする」ところまで。中身の解釈・
//! 描画はしない。関連付ける種類は `.txt`（`text/plain`）1 つだけで、既定アプリを奪わない
//! 順位で登録する（Apple は `LSHandlerRank=Alternate`）。呼び出し元は `fandhe-egui-proxy` の
//! `device/file_association.rs`（画面向け effect ドライバ）と各 OS 実装（macOS は
//! `device/apple_file_association.rs`）。`url_scheme` の [`UrlInbox`](crate::url_scheme::UrlInbox)
//! と同じ継続配信型で、`RequestSlot` 型ではない。
//!
//! 読み取りは [`drag_drop`](crate::drag_drop) の部品（O_NONBLOCK・フォルダ判定・上限
//! [`MAX_FILE_BYTES`](crate::file_picker::MAX_FILE_BYTES)）をそのまま使う。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果の種類は 4 つ**: 受理（`latest = Some(Ok)`）、失敗（`latest = Some(Err)`。
//!   [`OpenFailure`] の 3 種）、拒否（名前・拡張子の検査違反。件数 [`FileSnapshot::rejected`]
//!   だけ増え `latest` は変わらない）、未受信（通常起動。失敗ではない）。非対応 OS は
//!   `Err(Unsupported)`（[`UnsupportedFileAssociation`]。空のスナップショットを成功として返さない）
//! - **最新の結果は 1 フィールド**: 成功と失敗を同時に持たないので、前の成功と新しい失敗が
//!   並んで見えることはない
//! - **保持するのは先頭 [`HEAD_BYTES`] バイトのみ**。名前・パス・中身はログにもエラー文言にも出さない
//! - **コールドスタート**: 受け口は static に置き、UI の有無に関係なく書き込む
//!
//! 関連付けの種類を変えるときは、定数 [`EXTENSION`]・[`MIME`] のほか Info.plist
//! （bundle スクリプトと iOS）・Android manifest の intent-filter・egui-proxy の
//! レジストリ登録の 4 か所を直す。
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順、不正入力の非表示、コールドスタート分を落とさないこと、
//! 判定と更新を同じ lock の内側で行うこと。
//! 守らない: ポーリング間隔の間や 1 回の odoc で複数件届いたときは最後の 1 件だけ表示される
//! こと（[`FileSnapshot::seq`] は残る）、複数プロセス・複数タブ、Windows の UserChoice や
//! 他アプリによる上書き、macOS の素の実行体（.app 化していないもの）には届かないこと、
//! 遅いボリュームでの main thread 同期読み取り、Android のプロセスが死んだ後の再表示、
//! Web の未インストール PWA や Chromium 以外のブラウザ。

use std::fs::File;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::DeviceError;
use crate::drag_drop::{DropError, PickedFile, read_dropped, read_dropped_from};

/// 関連付ける拡張子（小文字、ドットなし）。
pub const EXTENSION: &str = "txt";
/// 関連付ける MIME（Android 用）。
pub const MIME: &str = "text/plain";
/// 保持する先頭バイト数。
pub const HEAD_BYTES: usize = 16;
/// 受け付けるファイル名の最大バイト数。
pub const MAX_NAME_BYTES: usize = 255;
/// 受け付ける Windows パスの最大バイト数。
pub const MAX_PATH_BYTES: usize = 2048;

/// ファイル名を検査する。違反は `InvalidInput("file_association")`。
///
/// 空・[`MAX_NAME_BYTES`] 超過・制御文字（NUL を含む）・区切り文字（`/` `\`）を含むものを弾く。
pub fn validate_name(name: &str) -> Result<(), DeviceError> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(DeviceError::InvalidInput("file_association"));
    }
    Ok(())
}

/// 名前と MIME から、関連付けた種類として受理してよいかを返す。
///
/// 名前が [`validate_name`] を通り、かつ拡張子が [`EXTENSION`] と（大文字小文字を区別せず）
/// 一致するか、MIME の `;` より前が [`MIME`] と一致すること。`.txt` だけの名前は拡張子なし扱い。
pub fn accepts(name: Option<&str>, mime: Option<&str>) -> bool {
    let Some(name) = name.filter(|n| validate_name(n).is_ok()) else {
        return false;
    };
    let ext_ok = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION));
    let mime_ok = mime.is_some_and(|m| {
        m.split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case(MIME))
    });
    ext_ok || mime_ok
}

/// Windows の起動引数のパスを検査する（Windows の子が使う純関数。ホストでテストできるよう
/// cfg を付けない）。`X:\` 形の絶対パスだけを通し、UNC（`\\server`）・`\\?\`・`\\.\` は拒否する
/// （SMB 認証の漏れとデバイスパスを避ける）。
pub fn validate_windows_path(path: &str) -> Result<(), DeviceError> {
    let invalid = || DeviceError::InvalidInput("file_association");
    if path.len() > MAX_PATH_BYTES || path.chars().any(char::is_control) || path.starts_with("\\\\")
    {
        return Err(invalid());
    }
    let b = path.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// 受理した 1 件。中身は先頭 [`HEAD_BYTES`] バイトだけ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedFile {
    /// ファイル名（パスを含まない）。
    pub name: String,
    /// ファイル全体のサイズ（バイト）。
    pub size: usize,
    /// 先頭 [`HEAD_BYTES`] バイト以下。
    pub head: Vec<u8>,
    /// 受信時刻（UNIX エポックからのミリ秒）。呼び出し側が渡す（wasm32 でも本クレートが
    /// コンパイルされるため `SystemTime::now()` を呼ばない）。
    pub received_at_unix_ms: u64,
}

/// 検査を通ったが開けなかった理由。名前もパスも持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailure {
    /// 上限（8 MiB）を超えた。
    TooLarge,
    /// 読めなかった（存在しない・権限なし・件数超過を含む）。
    Unreadable,
    /// フォルダだった。
    Folder,
}

impl From<DropError> for OpenFailure {
    fn from(e: DropError) -> Self {
        match e {
            DropError::TooLarge => Self::TooLarge,
            DropError::Folder => Self::Folder,
            DropError::Unreadable | DropError::TooMany => Self::Unreadable,
        }
    }
}

/// [`FileInbox`] の整合した読み出し結果（lock 1 回分）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileSnapshot {
    /// 最後の結果。未受信なら `None`。
    pub latest: Option<Result<OpenedFile, OpenFailure>>,
    /// 検査で弾いた件数。
    pub rejected: u64,
    /// 受信（受理 + 失敗 + 拒否）の通算回数。画面側が変化を検出する世代番号。
    pub seq: u64,
}

/// 受信したファイルを保持する共有状態。static に置ける（[`FileInbox::new`] は `const fn`）。
pub struct FileInbox(Mutex<FileSnapshot>);

impl FileInbox {
    /// 空の受け口を作る。
    pub const fn new() -> Self {
        Self(Mutex::new(FileSnapshot {
            latest: None,
            rejected: 0,
            seq: 0,
        }))
    }

    /// poison していても中身を使う（panic を契約にしない）。
    fn guard(&self) -> MutexGuard<'_, FileSnapshot> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 検査で弾いたものを数える。`latest` は変えない。
    pub fn reject(&self) {
        let mut inner = self.guard();
        inner.seq += 1;
        inner.rejected += 1;
    }

    /// 読み取り結果を記録する。`latest` を置き換え、成功時は先頭だけ残して本体を捨てる。
    pub fn record(&self, result: Result<PickedFile, DropError>, now_ms: u64) {
        let latest = match result {
            Ok(f) => Ok(OpenedFile {
                size: f.bytes.len(),
                head: f.bytes.iter().copied().take(HEAD_BYTES).collect(),
                name: f.name,
                received_at_unix_ms: now_ms,
            }),
            Err(e) => Err(OpenFailure::from(e)),
        };
        let mut inner = self.guard();
        inner.seq += 1;
        inner.latest = Some(latest);
    }

    /// パスで届いたファイルを検査して読む（macOS・iOS・Windows 用）。`None`（パスが取れない）と
    /// 名前の検査違反は拒否。検査と読み取りは lock の外、更新は [`Self::record`] で 1 回。
    pub fn receive_path(&self, path: Option<&Path>, now_ms: u64) {
        let Some(path) = path else {
            return self.reject();
        };
        let name = path.file_name().and_then(|n| n.to_str());
        if !accepts(name, None) {
            return self.reject();
        }
        self.record(read_dropped(path), now_ms);
    }

    /// 開いた `File` で届いたファイルを検査して読む（Android 用。名前と MIME は OS が教える）。
    pub fn receive_file(&self, name: Option<&str>, mime: Option<&str>, file: File, now_ms: u64) {
        let Some(name) = name.filter(|_| accepts(name, mime)) else {
            return self.reject();
        };
        self.record(read_dropped_from(file, name.to_string()), now_ms);
    }

    /// 現在の状態を 1 回の lock で読む。
    pub fn snapshot(&self) -> FileSnapshot {
        self.guard().clone()
    }
}

impl Default for FileInbox {
    fn default() -> Self {
        Self::new()
    }
}

/// 関連付けの受け取り状態を読む契約。受信は OS が push するため、読むだけの 1 メソッド。
pub trait FileAssociation: Send + Sync {
    /// 現在の状態を返す。非対応 OS は `Err(Unsupported)`。
    fn snapshot(&self) -> Result<FileSnapshot, DeviceError>;
}

/// 非対応 OS 用。常に `Err(Unsupported)`（空のスナップショットを成功として返さない）。
pub struct UnsupportedFileAssociation;

impl FileAssociation for UnsupportedFileAssociation {
    fn snapshot(&self) -> Result<FileSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("file_association"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_picker::MAX_FILE_BYTES;

    /// 一意な一時ディレクトリ（テスト後に消す）。
    fn temp_dir() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fandhe-file-assoc-test-{}-{}",
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_name_rejects_bad() {
        let long = "a".repeat(256);
        for bad in ["", long.as_str(), "a\nb", "a\0b", "a/b", "a\\b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name("a.TXT").is_ok());
    }

    #[test]
    fn accepts_by_extension_or_mime() {
        assert!(accepts(Some("a.TXT"), None));
        assert!(!accepts(Some("a.md"), None));
        assert!(!accepts(Some(".txt"), None));
        assert!(!accepts(None, Some("text/plain")));
        assert!(accepts(Some("123"), Some("text/plain; charset=utf-8")));
        assert!(!accepts(Some("123"), Some("image/png")));
        assert!(!accepts(Some("a/b.txt"), None));
    }

    #[test]
    fn windows_path_validation() {
        let long = format!("C:\\{}", "a".repeat(2049));
        for bad in [
            "\\\\server\\s\\a.txt",
            "\\\\?\\C:\\a.txt",
            "\\\\.\\pipe\\x",
            "a.txt",
            "C:a.txt",
            long.as_str(),
            "C:\\a\u{7}.txt",
        ] {
            assert!(validate_windows_path(bad).is_err(), "{bad:?}");
        }
        assert!(validate_windows_path("C:\\Users\\a.txt").is_ok());
    }

    #[test]
    fn receive_path_success_keeps_only_head() {
        let dir = temp_dir();
        let p = dir.join("a.txt");
        std::fs::write(&p, b"0123456789abcdefXYZ").unwrap();
        let inbox = FileInbox::new();
        inbox.receive_path(Some(&p), 7);
        let snap = inbox.snapshot();
        let f = snap.latest.unwrap().unwrap();
        assert_eq!((f.name.as_str(), f.size), ("a.txt", 19));
        assert_eq!(f.head, b"0123456789abcdef");
        assert_eq!((f.received_at_unix_ms, snap.rejected, snap.seq), (7, 0, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn receive_path_failures_are_not_success() {
        let dir = temp_dir();
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![0u8; MAX_FILE_BYTES + 1]).unwrap();
        let folder = dir.join("d.txt");
        std::fs::create_dir(&folder).unwrap();
        let missing = dir.join("none.txt");
        let inbox = FileInbox::new();
        for (p, want) in [
            (&big, OpenFailure::TooLarge),
            (&folder, OpenFailure::Folder),
            (&missing, OpenFailure::Unreadable),
        ] {
            inbox.receive_path(Some(p), 1);
            assert_eq!(inbox.snapshot().latest, Some(Err(want)));
        }
        assert_eq!(inbox.snapshot().rejected, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_keep_latest_and_failure_replaces_success() {
        let dir = temp_dir();
        let ok = dir.join("a.txt");
        std::fs::write(&ok, b"hi").unwrap();
        let inbox = FileInbox::new();
        inbox.receive_path(Some(&ok), 1);
        let before = inbox.snapshot().latest;
        inbox.receive_path(Some(&dir.join("a.md")), 2);
        inbox.receive_path(None, 3);
        let snap = inbox.snapshot();
        assert_eq!(snap.latest, before);
        assert_eq!((snap.rejected, snap.seq), (2, 3));
        // 成功の後の失敗は古い成功を残さない。
        inbox.receive_path(Some(&dir.join("none.txt")), 4);
        assert_eq!(inbox.snapshot().latest, Some(Err(OpenFailure::Unreadable)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn receive_file_unreadable_for_write_only_handle() {
        let dir = temp_dir();
        let p = dir.join("w.txt");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&p)
            .unwrap();
        let inbox = FileInbox::new();
        inbox.receive_file(Some("w.txt"), None, file, 1);
        assert_eq!(inbox.snapshot().latest, Some(Err(OpenFailure::Unreadable)));
        inbox.receive_file(Some("w.md"), None, File::open(&p).unwrap(), 2);
        assert_eq!(inbox.snapshot().rejected, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsupported_never_returns_ok() {
        assert!(matches!(
            UnsupportedFileAssociation.snapshot(),
            Err(DeviceError::Unsupported("file_association"))
        ));
    }
}
