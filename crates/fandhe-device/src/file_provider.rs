//! 自アプリの保存先を OS のファイル画面に出す（Phase 11 の拡充ループ、親 #194、Issue #1019）。
//!
//! 扱う範囲は「アプリ専用フォルダ直下の通常ファイルを、一覧する・見本を置く・消す」ところまで。
//! フォルダを OS のファイル画面（Android の Files アプリ等）へ見せる provider 本体は OS 側
//! （Android は #1020 の `DocumentsProvider`）が担い、本モジュールはフォルダを引数に取る
//! 共通実装 [`DirFileProvider`] と契約を持つ。呼び出し元は `fandhe-egui-proxy` の
//! `device/file_provider.rs`（画面向け effect ドライバ）。対象 OS は Android だけで、
//! iOS・macOS・Windows は保留、Web は対象外。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! | 操作 | 成功 | 失敗 |
//! | --- | --- | --- |
//! | 一覧 | `Ok(Vec<Entry>)`（0 件も成功） | `Err(Internal)` |
//! | 置く | `Ok(PutOutcome)`（通知の成否を並べて返す） | `Err(InvalidInput)`（名前の規則違反）/ `Err(Internal)` |
//! | 消す | `Ok(DeleteOutcome)` | `Err(Internal(`[`NOT_FOUND`]`))`（既に無い）/ `Err(Internal)` / `Err(InvalidInput)` |
//!
//! - **非対応は `Err(Unsupported)`**: 到達するのは iOS・macOS・Windows・Web・Linux。初期化できない
//!   実装も [`UnsupportedFileProvider`] へ倒す。ダミーの成功へは倒さない
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 利用者のキャンセル操作も
//!   権限ダイアログも無い。結果は 成功 / 0 件（成功）/ 失敗 / 非対応 の四つで、0 件と失敗を混同しない
//! - **OS を正にする**: 一覧はファイルシステムだけを正とし、アプリ内に写しを持たない。操作の後は
//!   読み直す。置くは上書きしない（`create_new`、重複は「名前 (n).拡張子」へずらす）
//! - **一覧に出す範囲**: 名前が UTF-8 で [`validate_name`] を通る通常ファイルだけ。ディレクトリ・
//!   シンボリックリンク・規則外の名前は出さない（provider 側も同じ条件にする）。項目の読み出しが
//!   1 件でも失敗したら全体を失敗にし、飛ばして成功にしない
//! - **名前の規則**: [`validate_name`] に集約する。Java 側（#1020）も同じ表で検証する
//! - **通知**: ファイル画面への変更通知は書けた後・消せた後だけ行い、失敗しても操作は成功のまま
//!   `notified = false` で返す（成功と通知失敗を並べて見せる）
//! - **書き込み失敗**: 作りかけは消してから `Err(Internal)`。消す操作自体が失敗した場合も `Err` の
//!   ままで、残った作りかけは次の一覧に出る（OS を正にする）
//! - **時刻**: `modified_unix_ms` は表示専用で、経過時間の計測に使わない。取れない・エポック前は `None`
//! - 名前・中身・パスをログ・エラー文に出さない（`device-bridge-contracts.md` 8）
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 名前の規則（パストラバーサルの拒否）、上書きしないこと、嘘の成功の禁止、作りかけの後始末。
//! 守らない: 他アプリが同じフォルダを同時に書き換える競合、複数プロセス、ネットワーク同期、
//! 通知からファイル画面の表示更新までの時間差、フォルダの容量上限。

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::DeviceError;

/// アプリ専用フォルダ名。Android は `getFilesDir()/file-provider`（Java 側と一致させる）。
pub const FOLDER_NAME: &str = "file-provider";
/// 名前の UTF-8 バイト数の上限。
pub const MAX_NAME_BYTES: usize = 255;
/// 重複時にずらす番号の上限。超えたら失敗にする。
pub const MAX_DEDUP: u32 = 99;
/// 「既に無い」を表す `Internal` の識別子（`DeviceError` に列挙子を足さない）。
pub const NOT_FOUND: &str = "file_provider not_found";
/// 名前の規則違反を表す `InvalidInput` の識別子。
pub const INVALID_NAME: &str = "file_provider name";
const INTERNAL: &str = "file_provider";

/// 一覧の 1 項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// ファイル名。
    pub name: String,
    /// バイト数。
    pub size: u64,
    /// 更新時刻（UNIX エポックからのミリ秒、表示専用）。取れなければ `None`。
    pub modified_unix_ms: Option<u64>,
}

/// 置いた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    /// 実際に置いた名前（重複でずらした場合は元と異なる）。
    pub name: String,
    /// ファイル画面への変更通知が成功したか。
    pub notified: bool,
}

/// 消した結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// ファイル画面への変更通知が成功したか。
    pub notified: bool,
}

/// アプリ専用フォルダの一覧・置く・消す。全メソッドはブロックしうるため、呼び出し側はワーカーで呼ぶ。
pub trait FileProvider: Send + Sync {
    /// フォルダ直下の対象ファイルを名前順で返す。
    fn list(&self) -> Result<Vec<Entry>, DeviceError>;
    /// `base_name` で中身を置く。同名があれば別名へずらし、既存を上書きしない。
    fn put(&self, base_name: &str, content: &[u8]) -> Result<PutOutcome, DeviceError>;
    /// `name` のファイルを消す。
    fn delete(&self, name: &str) -> Result<DeleteOutcome, DeviceError>;
}

/// 非対応 OS 用。全メソッドが `Unsupported("file_provider")` を返す。
pub struct UnsupportedFileProvider;

impl FileProvider for UnsupportedFileProvider {
    fn list(&self) -> Result<Vec<Entry>, DeviceError> {
        Err(DeviceError::Unsupported("file_provider"))
    }
    fn put(&self, _base_name: &str, _content: &[u8]) -> Result<PutOutcome, DeviceError> {
        Err(DeviceError::Unsupported("file_provider"))
    }
    fn delete(&self, _name: &str) -> Result<DeleteOutcome, DeviceError> {
        Err(DeviceError::Unsupported("file_provider"))
    }
}

/// 名前の規則。空・`.`・`..`・`/`・NUL・制御文字・先頭 `.`・255 バイト超を `InvalidInput` で拒否する。
pub fn validate_name(name: &str) -> Result<(), DeviceError> {
    let bad = name.is_empty()
        || name.starts_with('.')
        || name.len() > MAX_NAME_BYTES
        || name.chars().any(|c| c == '/' || c.is_control());
    if bad {
        Err(DeviceError::InvalidInput(INVALID_NAME))
    } else {
        Ok(())
    }
}

/// 重複回避名。`n == 0` は元の名前、それ以外は「名前 (n).拡張子」（拡張子は最後の `.` 以降）。
/// ずらした名前も [`validate_name`] を通す。
pub fn dedup_name(base: &str, n: u32) -> Result<String, DeviceError> {
    let name = if n == 0 {
        base.to_string()
    } else {
        // 先頭 `.` は validate_name で弾かれるため、拡張子の位置が 0 になることは無い。
        match base.rfind('.') {
            Some(i) => format!("{} ({n}){}", &base[..i], &base[i..]),
            None => format!("{base} ({n})"),
        }
    };
    validate_name(&name)?;
    Ok(name)
}

/// 項目の `io::Result` 列から一覧を作る。1 件でも `Err` なら全体を失敗にする（飛ばして成功にしない）。
fn collect_entries(
    items: impl Iterator<Item = io::Result<Option<Entry>>>,
) -> Result<Vec<Entry>, DeviceError> {
    let mut out = Vec::new();
    for item in items {
        if let Some(e) = item.map_err(|_| DeviceError::Internal(INTERNAL))? {
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 書き込み処理 `write` を実行し、失敗したら作りかけ `path` を消してから `Err(Internal)` を返す。
/// 削除自体が失敗しても `Err` のまま返し、残りは次の一覧に出る。
fn write_or_cleanup(
    path: &std::path::Path,
    write: impl FnOnce() -> io::Result<()>,
) -> Result<(), DeviceError> {
    if write().is_err() {
        let _ = fs::remove_file(path);
        return Err(DeviceError::Internal(INTERNAL));
    }
    Ok(())
}

/// フォルダを引数に取る共通実装。Android（#1020）は `data_dir()/FOLDER_NAME` と JNI の通知を渡し、
/// ホストのテストは一時フォルダと偽の notifier を渡す。
pub struct DirFileProvider {
    dir: PathBuf,
    notify: Box<dyn Fn() -> Result<(), DeviceError> + Send + Sync>,
}

impl DirFileProvider {
    /// `dir` を保存先、`notify` を変更通知とする。
    pub fn new(
        dir: PathBuf,
        notify: Box<dyn Fn() -> Result<(), DeviceError> + Send + Sync>,
    ) -> Self {
        Self { dir, notify }
    }
}

fn to_entry(item: io::Result<fs::DirEntry>) -> io::Result<Option<Entry>> {
    let item = item?;
    // `DirEntry::file_type` / `metadata` はシンボリックリンクを辿らない。
    let meta = item.metadata()?;
    if !item.file_type()?.is_file() {
        return Ok(None);
    }
    let Ok(name) = item.file_name().into_string() else {
        return Ok(None);
    };
    if validate_name(&name).is_err() {
        return Ok(None);
    }
    let modified_unix_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    Ok(Some(Entry {
        name,
        size: meta.len(),
        modified_unix_ms,
    }))
}

fn map_remove_err(e: &io::Error) -> DeviceError {
    if e.kind() == io::ErrorKind::NotFound {
        DeviceError::Internal(NOT_FOUND)
    } else {
        DeviceError::Internal(INTERNAL)
    }
}

impl FileProvider for DirFileProvider {
    fn list(&self) -> Result<Vec<Entry>, DeviceError> {
        match fs::read_dir(&self.dir) {
            Ok(rd) => collect_entries(rd.map(to_entry)),
            // まだ何も置いていない = 0 件の成功。
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err(DeviceError::Internal(INTERNAL)),
        }
    }

    fn put(&self, base_name: &str, content: &[u8]) -> Result<PutOutcome, DeviceError> {
        validate_name(base_name)?;
        fs::create_dir_all(&self.dir).map_err(|_| DeviceError::Internal(INTERNAL))?;
        for n in 0..=MAX_DEDUP {
            let name = dedup_name(base_name, n)?;
            let path = self.dir.join(&name);
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    write_or_cleanup(&path, || {
                        f.write_all(content)?;
                        f.sync_all()
                    })?;
                    return Ok(PutOutcome {
                        name,
                        notified: (self.notify)().is_ok(),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(DeviceError::Internal(INTERNAL)),
            }
        }
        Err(DeviceError::Internal(INTERNAL))
    }

    fn delete(&self, name: &str) -> Result<DeleteOutcome, DeviceError> {
        validate_name(name)?;
        let path = self.dir.join(name);
        // シンボリックリンクを辿らず、通常ファイルだけを消す。
        let meta = fs::symlink_metadata(&path).map_err(|e| map_remove_err(&e))?;
        if !meta.is_file() {
            return Err(DeviceError::Internal(INTERNAL));
        }
        fs::remove_file(&path).map_err(|e| map_remove_err(&e))?;
        Ok(DeleteOutcome {
            notified: (self.notify)().is_ok(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "fandhe-file-provider-{}-{tag}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn provider(dir: PathBuf, ok: bool) -> (DirFileProvider, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let p = DirFileProvider::new(
            dir,
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                if ok {
                    Ok(())
                } else {
                    Err(DeviceError::Internal("notify"))
                }
            }),
        );
        (p, calls)
    }

    #[test]
    fn validate_rejects_bad_names() {
        let long = "a".repeat(256);
        for n in [
            "",
            ".",
            "..",
            "a/b",
            "a\0b",
            "a\nb",
            ".hidden",
            long.as_str(),
        ] {
            assert!(
                matches!(
                    validate_name(n),
                    Err(DeviceError::InvalidInput(INVALID_NAME))
                ),
                "{n:?}"
            );
        }
        for n in ["a.txt", "日本語.txt", "a".repeat(255).as_str()] {
            assert!(validate_name(n).is_ok(), "{n:?}");
        }
    }

    #[test]
    fn dedup_names() {
        assert_eq!(dedup_name("a.txt", 0).unwrap(), "a.txt");
        assert_eq!(dedup_name("a.txt", 1).unwrap(), "a (1).txt");
        assert_eq!(dedup_name("noext", 2).unwrap(), "noext (2)");
        assert_eq!(dedup_name("a.b.txt", 1).unwrap(), "a.b (1).txt");
        assert!(dedup_name(&"a".repeat(255), 1).is_err());
    }

    #[test]
    fn put_never_overwrites_and_dedups() {
        let t = Tmp::new("put");
        let (p, calls) = provider(t.0.clone(), true);
        fs::write(t.0.join("a.txt"), b"old").unwrap();
        let r = p.put("a.txt", b"new").unwrap();
        assert_eq!(
            r,
            PutOutcome {
                name: "a (1).txt".into(),
                notified: true
            }
        );
        assert_eq!(fs::read(t.0.join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(t.0.join("a (1).txt")).unwrap(), b"new");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn put_creates_missing_dir_and_reports_notify_failure() {
        let t = Tmp::new("notify");
        let (p, _) = provider(t.0.join("sub"), false);
        let r = p.put("a.txt", b"x").unwrap();
        assert!(!r.notified);
        assert!(t.0.join("sub/a.txt").is_file());
    }

    #[test]
    fn put_fails_after_max_dedup_without_new_files() {
        let t = Tmp::new("max");
        let (p, _) = provider(t.0.clone(), true);
        fs::write(t.0.join("a.txt"), b"x").unwrap();
        for n in 1..=MAX_DEDUP {
            fs::write(t.0.join(dedup_name("a.txt", n).unwrap()), b"x").unwrap();
        }
        let before = fs::read_dir(&t.0).unwrap().count();
        assert!(matches!(
            p.put("a.txt", b"y"),
            Err(DeviceError::Internal(_))
        ));
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), before);
    }

    #[test]
    fn put_invalid_name_creates_nothing() {
        let t = Tmp::new("inv");
        let dir = t.0.join("sub");
        let (p, calls) = provider(dir.clone(), true);
        assert!(matches!(
            p.put("../x", b"y"),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(!dir.exists());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn write_failure_removes_partial_file() {
        let t = Tmp::new("wf");
        let path = t.0.join("x");
        fs::write(&path, b"partial").unwrap();
        let r = write_or_cleanup(&path, || Err(io::Error::other("boom")));
        assert!(matches!(r, Err(DeviceError::Internal(_))));
        assert!(!path.exists());
    }

    #[test]
    fn list_missing_dir_is_empty_success_and_file_dir_is_error() {
        let t = Tmp::new("list0");
        let (p, _) = provider(t.0.join("none"), true);
        assert_eq!(p.list().unwrap(), vec![]);
        fs::write(t.0.join("f"), b"x").unwrap();
        let (p, _) = provider(t.0.join("f"), true);
        assert!(matches!(p.list(), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn list_sorted_and_filters_non_files() {
        let t = Tmp::new("list");
        let (p, _) = provider(t.0.clone(), true);
        fs::write(t.0.join("b.txt"), b"bb").unwrap();
        fs::write(t.0.join("a.txt"), b"a").unwrap();
        fs::write(t.0.join(".hidden"), b"x").unwrap();
        fs::create_dir(t.0.join("dir")).unwrap();
        let v = p.list().unwrap();
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        assert_eq!(v[1].size, 2);
    }

    #[test]
    fn collect_entries_fails_whole_on_one_error() {
        let ok = Entry {
            name: "a".into(),
            size: 0,
            modified_unix_ms: None,
        };
        let items = vec![Ok(Some(ok)), Err(io::Error::other("x"))];
        assert!(matches!(
            collect_entries(items.into_iter()),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn delete_flows() {
        let t = Tmp::new("del");
        let outside = t.0.join("outside");
        fs::write(&outside, b"keep").unwrap();
        let dir = t.0.join("d");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), b"x").unwrap();
        let (p, calls) = provider(dir.clone(), false);
        assert_eq!(
            p.delete("a.txt").unwrap(),
            DeleteOutcome { notified: false }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            p.delete("a.txt"),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
        assert!(matches!(
            p.delete("../outside"),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(outside.exists());
        assert!(matches!(p.delete("sub"), Err(DeviceError::Internal(m)) if m != NOT_FOUND));
        assert!(dir.join("sub").is_dir());
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedFileProvider;
        assert!(matches!(
            u.list(),
            Err(DeviceError::Unsupported("file_provider"))
        ));
        assert!(matches!(u.put("a", b""), Err(DeviceError::Unsupported(_))));
        assert!(matches!(u.delete("a"), Err(DeviceError::Unsupported(_))));
    }
}
