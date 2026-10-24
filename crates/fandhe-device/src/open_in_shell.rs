//! ファイルを既定アプリで開く・ファイルマネージャーで選択表示する機能の抽象
//! （Phase 11 の拡充ループ、親 #146、Issue #749）。
//!
//! 範囲は「指定ファイルを既定アプリで開く」「ファイルマネージャーで選択表示する」の 2 操作だけ。
//! ゴミ箱は破壊的なので範囲外。検査の SSOT は [`ShellPath::new`] と [`ShellPath::check_openable`]
//! （これ以外では OS へ渡すパスを作れない）。`fandhe-egui-proxy` の effect ドライバが値を検査して
//! から OS 実装（macOS は `NSWorkspace`、Windows は #750）を呼ぶ。
//!
//! # 結果の三値
//!
//! | 区分 | 返す値 |
//! |---|---|
//! | 渡した（OS が受理した。アプリ・ウィンドウが開いたかは確認できない） | `Ok(ShellOutcome::HandedOff)` |
//! | 開けなかった（扱うアプリが無い。「開く」だけで起きる） | `Ok(ShellOutcome::NoHandler)` |
//! | 開けなかった（パス不正） | `Err(InvalidInput("shell path"))` |
//! | 開けなかった（実行可能・アプリ・パッケージ・ディレクトリ） | `Err(InvalidInput("shell type"))` |
//! | 開けなかった（OS の失敗） | `Err(Internal("shell ..."))` |
//! | 非対応 OS | `Err(Unsupported("open in shell"))` |
//!
//! # パスの検査
//!
//! [`ShellPath::new`] は絶対パス・NUL なし・UTF-8・[`MAX_PATH_BYTES`] 以内・存在することを確かめ、
//! `canonicalize` でシンボリックリンクを解決した後のパスを保持する。「開く」の前には
//! [`ShellPath::check_openable`] を通す（通常のファイルだけ。ディレクトリ = `.app` 等のバンドルを
//! 含む、と unix の実行ビット付きは拒否）。OS 側でも型を重ねて確かめる（macOS は resource key、
//! Windows は `AssocIsDangerous`）。選択表示はファイルとディレクトリの両方を受け付ける。
//! OS へはパスを 1 つの引数として渡し、シェルやコマンドラインは組み立てない。
//!
//! # 守る範囲 / 守らない範囲（契約表 10・11）
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代番号）、不正な入力を OS へ渡さないこと、
//! 実行可能なファイル・アプリ・パッケージを起動しないこと、「扱うアプリが無い」と「非対応」を
//! 成功に畳まないこと、パスと OS のエラー文をログやエラー文言に出さないこと。
//!
//! 守らない: 検査してから OS を呼ぶまでの間のファイルの差し替え（TOCTOU）、表示したアプリ・
//! ウィンドウの確認、実行可能でない文書型を受け取ったアプリ側の動作、複数プロセス、ゴミ箱。
//!
//! 期限は設けない。OS の呼び出しは Bool を同期で返し、待つ処理を持たないため（契約表 3）。

use std::path::{Path, PathBuf};

use crate::DeviceError;

/// パスの長さの上限（バイト）。macOS の `PATH_MAX`。
pub const MAX_PATH_BYTES: usize = 1024;

/// 検査済みのパス。OS へ渡せるのはこの型だけ（検査の SSOT）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPath {
    path: PathBuf,
}

impl ShellPath {
    /// 絶対・NUL なし・UTF-8・長さ上限内・存在するパスだけを受け付け、正規化後を保持する。
    pub fn new(path: &Path) -> Result<Self, DeviceError> {
        let bad = || DeviceError::InvalidInput("shell path");
        let Some(s) = path.to_str() else {
            return Err(bad());
        };
        if s.is_empty() || s.len() > MAX_PATH_BYTES || s.contains('\0') || !path.is_absolute() {
            return Err(bad());
        }
        let canonical = std::fs::canonicalize(path).map_err(|_| bad())?;
        // 正規化後も同じ制約を満たすこと（シンボリックリンク先が長い・非 UTF-8 の場合を拒否する）。
        match canonical.to_str() {
            Some(c) if c.len() <= MAX_PATH_BYTES && !c.contains('\0') => {
                Ok(Self { path: canonical })
            }
            _ => Err(bad()),
        }
    }

    /// 正規化済みのパス。
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// 正規化済みのパス（UTF-8 は [`ShellPath::new`] で確認済み）。
    pub fn as_str(&self) -> &str {
        self.path.to_str().unwrap_or_default()
    }

    /// 「開く」の対象にしてよい型か確かめる。通常のファイルだけを通す。ディレクトリ
    /// （`.app` 等のバンドルを含む）と、unix で実行ビットが立つファイルは
    /// `InvalidInput("shell type")`。
    pub fn check_openable(&self) -> Result<(), DeviceError> {
        let rejected = || DeviceError::InvalidInput("shell type");
        let meta =
            std::fs::metadata(&self.path).map_err(|_| DeviceError::InvalidInput("shell path"))?;
        if !meta.is_file() {
            return Err(rejected());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 != 0 {
                return Err(rejected());
            }
        }
        Ok(())
    }
}

/// OS への受け渡し結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellOutcome {
    /// OS が依頼を受理した。アプリ・ウィンドウが開いたかは分からない。
    HandedOff,
    /// 扱えるアプリが無い（「開く」だけで起きる）。失敗ではない。
    NoHandler,
}

/// 既定アプリで開く・ファイルマネージャーで選択表示する OS 機能。
///
/// 呼び出し元は `fandhe-egui-proxy` の effect ドライバ（UI スレッドで同期に呼ぶ）。
/// 引数は [`ShellPath`] 検査済みで、「開く」は事前に [`ShellPath::check_openable`] 済みの前提。
pub trait OpenInShell: Send + Sync {
    /// 既定のアプリでファイルを開く。契約は本モジュールの doc。
    fn open(&self, path: &ShellPath) -> Result<ShellOutcome, DeviceError>;
    /// ファイルマネージャー（Finder / Explorer）で選択表示する。`NoHandler` は返さない。
    fn reveal(&self, path: &ShellPath) -> Result<ShellOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedOpenInShell;

impl OpenInShell for UnsupportedOpenInShell {
    fn open(&self, _path: &ShellPath) -> Result<ShellOutcome, DeviceError> {
        Err(DeviceError::Unsupported("open in shell"))
    }

    fn reveal(&self, _path: &ShellPath) -> Result<ShellOutcome, DeviceError> {
        Err(DeviceError::Unsupported("open in shell"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fandhe-ois-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rejects_bad_paths() {
        for p in ["", "relative/path.txt", "/nonexistent/fandhe-ois/x"] {
            assert!(
                matches!(
                    ShellPath::new(Path::new(p)),
                    Err(DeviceError::InvalidInput("shell path"))
                ),
                "{p}"
            );
        }
        assert!(ShellPath::new(Path::new("/tmp/a\0b")).is_err());
        let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
        assert!(ShellPath::new(Path::new(&long)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let p = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe"));
        assert!(ShellPath::new(p).is_err());
    }

    #[test]
    fn accepts_regular_file() {
        let d = temp_dir("ok");
        let f = d.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let sp = ShellPath::new(&f).unwrap();
        assert!(sp.as_path().is_absolute());
        assert!(sp.as_str().ends_with("a.txt"));
        assert!(sp.check_openable().is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_resolved() {
        let d = temp_dir("link");
        let f = d.join("real.txt");
        std::fs::write(&f, b"x").unwrap();
        let l = d.join("link.txt");
        std::os::unix::fs::symlink(&f, &l).unwrap();
        let sp = ShellPath::new(&l).unwrap();
        assert_eq!(sp.as_path(), std::fs::canonicalize(&f).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn directory_is_rejected_for_open_but_accepted_as_path() {
        let d = temp_dir("dir");
        let sp = ShellPath::new(&d).unwrap();
        assert!(matches!(
            sp.check_openable(),
            Err(DeviceError::InvalidInput("shell type"))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn executable_file_is_rejected_for_open() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp_dir("exec");
        let f = d.join("run.sh");
        std::fs::write(&f, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sp = ShellPath::new(&f).unwrap();
        assert!(matches!(
            sp.check_openable(),
            Err(DeviceError::InvalidInput("shell type"))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unsupported_never_succeeds() {
        let d = temp_dir("unsup");
        let f = d.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let sp = ShellPath::new(&f).unwrap();
        assert!(matches!(
            UnsupportedOpenInShell.open(&sp),
            Err(DeviceError::Unsupported("open in shell"))
        ));
        assert!(matches!(
            UnsupportedOpenInShell.reveal(&sp),
            Err(DeviceError::Unsupported("open in shell"))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }
}
