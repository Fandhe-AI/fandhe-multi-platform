//! 自アプリの保存先を OS のファイル画面に出す（Phase 11 の拡充ループ、親 #194、Issue #1019）。
//!
//! 扱う範囲は「アプリ専用フォルダの書類を、一覧する・見本を置く・消す」ところまで。
//! フォルダを OS のファイル画面（Android の Files アプリ等）へ見せる provider 本体は OS 側
//! （Android は #1020 の `DocumentsProvider`）が担い、本モジュールはフォルダを引数に取る
//! 共通実装 [`DirFileProvider`] と契約を持つ。呼び出し元は `fandhe-egui-proxy` の
//! `device/file_provider.rs`（画面向け effect ドライバ）。対象 OS は Android だけで、
//! iOS・macOS・Windows は保留、Web は対象外。
//!
//! **本書は「再利用しない書類 ID」方式の契約である。現行コードはまだ旧方式（ファイル名を識別キーにし、
//! フォルダ直下に平らに置く）のままで、#1020 が本書へ合わせて実装する。** 食い違う箇所は各項目に
//! 「#1020 で実装」と書く。
//!
//! # 書類 ID 方式（契約の中心）
//!
//! 旧方式はファイル名をそのまま document ID（SAF の URI の一部）にしていた。消した直後に同じ名前で
//! 作り直すと、消えた書類の URI に権限を持つ他アプリが、新しい別の書類まで読み書きできてしまう。
//! Rust 側（[`DirFileProvider`] の直接ファイル操作）と Java 側（`DocumentsProvider`）の 2 経路が
//! 同じフォルダを触るため、世代表や排他制御を足しても隙間が残った。そこで識別をファイル名から切り離す。
//!
//! - **1 書類 = 再利用しない ID 1 つ**。ディスク上の配置は `<アプリ専用フォルダ>/<id>/<表示名>` とする
//!   （ID のサブフォルダに通常ファイルを 1 つだけ置く。メタデータの別ファイルは持たない）
//! - **ID の作り方**: ランダムで十分長く（128 bit 相当、16 進なら 32 文字以上）、文字種は英数字のみ。
//!   作成は `create_dir`（既にあれば失敗）で行い、一意性を OS に確かめさせる。`create_dir_all` は使わない。
//!   衝突したら別の ID で作り直し、既存の ID には触れない。**消した ID は二度と使わない**
//! - **識別と表示の分離**: document ID・URI・Rust API の削除キーは ID。表示名はユーザーに見せるだけで、
//!   識別には使わない
//! - **表示名の重複を許す**: ID が違えば別の書類。旧方式の「名前 (n).拡張子」へのずらしは**廃止**する
//!   （[`dedup_name`]・[`MAX_DEDUP`] は #1020 で削除）。置くは常に新しい ID の書類を作るので、
//!   既存の書類は上書きされない
//! - **古い URI の無効化**: 消えた ID を指す URI は、開くと「無い」になる。同じ表示名で作り直しても
//!   新しい ID になるので、古い URI の権限は新しい書類に及ばない。このため世代表も、Rust 側と
//!   Java 側の作成・削除に共通の排他も要らない（ID が衝突しないため）
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! | 操作 | 成功 | 失敗 |
//! | --- | --- | --- |
//! | 一覧 | `Ok(Vec<Entry>)`（0 件も成功） | `Err(Internal)`（I/O エラーに限る） |
//! | 置く | `Ok(PutOutcome)`（新しい ID と通知の成否を返す） | `Err(InvalidInput)`（表示名の規則違反）/ `Err(Internal)` |
//! | 消す | `Ok(DeleteOutcome)` | `Err(Internal(`[`NOT_FOUND`]`))`（その ID が無い）/ `Err(Internal)`（サブフォルダの削除失敗を含む）/ `Err(InvalidInput)`（ID の規則違反） |
//!
//! - **非対応は `Err(Unsupported)`**: 到達するのは iOS・macOS・Windows・Web・Linux。初期化できない
//!   実装も [`UnsupportedFileProvider`] へ倒す。ダミーの成功へは倒さない
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 利用者のキャンセル操作も
//!   権限ダイアログも無い。結果は 成功 / 0 件（成功）/ 失敗 / 非対応 の四つで、0 件と失敗を混同しない
//! - **OS を正にする**: 一覧はファイルシステムだけを正とし、アプリ内に写しを持たない。操作の後は
//!   読み直す。置くは上書きしない（新しい ID のサブフォルダを `create_dir` で作り、その中に
//!   `create_new` で書く）
//! - **一覧に出す範囲**: フォルダ直下のうち、(1) 通常のディレクトリで、名前が ID の規則を通るもの、
//!   (2) その中身がちょうど 1 つの通常ファイルで、名前が UTF-8 で [`validate_name`] を通るもの、
//!   だけ。シンボリックリンク・ID の規則外の名前・規則外の表示名は出さない（provider 側も同じ条件にする）。
//!   並びは表示名順、同じ表示名は ID 順
//! - **作りかけの扱い**: 空のサブフォルダ・ファイルが 2 つ以上・中身がディレクトリ等は、作成中か
//!   中断の跡とみなして**一覧に出さず、失敗にもしない**（他経路が作成中の書類を失敗に巻き込まない）。
//!   一覧を失敗にするのは I/O エラー（フォルダや項目の `metadata` を読めない等）に限り、
//!   1 件でもあれば全体を失敗にして、飛ばして成功にしない
//! - **名前の規則**: 表示名は [`validate_name`] に集約する（`<id>/<表示名>` の 1 要素なのでパスの
//!   閉じ込めも兼ねる）。ID は英数字のみで、空・長すぎるものを拒否する。Java 側（#1020）も同じ表で検証する
//! - **通知**: ファイル画面への変更通知は書けた後・消せた後だけ行い、失敗しても操作は成功のまま
//!   `notified = false` で返す（成功と通知失敗を並べて見せる）
//! - **消す**: ファイルを消してから ID のサブフォルダを消す。サブフォルダの削除失敗は握りつぶさず
//!   `Err(Internal)` にする（`device-bridge-contracts.md` 9）。ID のサブフォルダが無ければ [`NOT_FOUND`]
//! - **書き込み失敗**: 作りかけは、書類のファイル→ID のサブフォルダの順に消してから `Err(Internal)`。
//!   消す操作自体が失敗した場合も `Err` のままで、残った作りかけは一覧に出ない。ID は再利用しないので、
//!   残骸が後から作る書類に影響することはない（残骸の回収は行わず、容量は守らない範囲）
//! - **時刻**: `modified_unix_ms` は表示専用で、経過時間の計測に使わない。取れない・エポック前は `None`
//! - ID・名前・中身・パスをログ・エラー文に出さない（`device-bridge-contracts.md` 8）
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 表示名の規則（パストラバーサルの拒否）、上書きしないこと、**消した書類の URI 権限が、
//! 後から作った書類へ引き継がれないこと**、嘘の成功の禁止、サブフォルダの削除失敗を握りつぶさないこと、
//! 作りかけの後始末。
//! 守らない: 他アプリが同じフォルダを同時に書き換える競合、複数プロセス、ネットワーク同期、
//! 通知からファイル画面の表示更新までの時間差、フォルダの容量上限、後始末に失敗した作りかけ
//! （一覧に出ない残骸）の容量。

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::DeviceError;

/// アプリ専用フォルダ名。Android は `getFilesDir()/file-provider`（Java 側と一致させる）。
/// 書類は直下の ID サブフォルダ `<FOLDER_NAME>/<id>/<表示名>` に 1 つずつ置く（#1020 で実装）。
pub const FOLDER_NAME: &str = "file-provider";
/// 表示名の UTF-8 バイト数の上限。
pub const MAX_NAME_BYTES: usize = 255;
/// 旧方式（表示名の重複を「名前 (n).拡張子」へずらす）の番号の上限。再利用しない書類 ID 方式では
/// 表示名の重複を許してずらさないため廃止する（#1020 で削除。それまでの現行コードだけが使う）。
pub const MAX_DEDUP: u32 = 99;
/// 「既に無い」を表す `Internal` の識別子（`DeviceError` に列挙子を足さない）。
pub const NOT_FOUND: &str = "file_provider not_found";
/// 名前（表示名。#1020 以降は ID も）の規則違反を表す `InvalidInput` の識別子。
pub const INVALID_NAME: &str = "file_provider name";
const INTERNAL: &str = "file_provider";

/// 一覧の 1 項目（1 書類）。
///
/// 契約上は「再利用しない書類 ID」と「表示名」を持つ。現行コードは表示名の `name` だけで、
/// ID の欄は #1020 で足す。以降、識別（削除・URI）は ID、`name` は表示だけに使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 表示名（`<id>/<表示名>` の表示名。ユーザーに見せるだけで、識別には使わない。重複しうる）。
    pub name: String,
    /// バイト数。
    pub size: u64,
    /// 更新時刻（UNIX エポックからのミリ秒、表示専用）。取れなければ `None`。
    pub modified_unix_ms: Option<u64>,
}

/// 置いた結果。
///
/// 契約上は、新しく採番した書類 ID と通知の成否を返す。ID の欄は #1020 で足す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    /// 置いた書類の表示名。再利用しない書類 ID 方式では渡した名前のまま
    /// （重複を許しずらさない）。現行コードは重複時にずらした名前を返す（#1020 で廃止）。
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
///
/// 識別は再利用しない書類 ID で行う（モジュール doc の「書類 ID 方式」）。現行のシグネチャは旧方式の
/// 名前キーのままで、ID を受け渡す形へは #1020 で変える。
pub trait FileProvider: Send + Sync {
    /// 書類を表示名順（同名は ID 順）で返す。対象は「ID サブフォルダの中身がちょうど 1 つの通常ファイル」
    /// だけで、作りかけ（空・2 つ以上）は出さず失敗にもしない。失敗は I/O エラーに限り、1 件でも
    /// 読めなければ全体を失敗にする（#1020 で実装。現行は平らなフォルダの通常ファイルを返す）。
    fn list(&self) -> Result<Vec<Entry>, DeviceError>;
    /// `base_name` を表示名として、新しい ID の書類を置く。表示名の重複は許し、既存の書類を上書きも
    /// ずらしもしない。ID は `create_dir` で一意性を確かめて採番する（#1020 で実装。現行は
    /// 同名があれば「名前 (n).拡張子」へずらす）。
    fn put(&self, base_name: &str, content: &[u8]) -> Result<PutOutcome, DeviceError>;
    /// 書類を消す。削除キーは ID（#1020 で実装。現行は名前をキーにする）。ファイルを消してから
    /// ID のサブフォルダを消し、サブフォルダの削除失敗は握りつぶさず `Err(Internal)` にする。
    /// 消した ID は二度と使わない。ID のサブフォルダが無ければ [`NOT_FOUND`]。
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

/// 表示名の規則。空・`.`・`..`・`/`・NUL・制御文字・先頭 `.`・255 バイト超を `InvalidInput` で拒否する。
/// 表示名は `<id>/<表示名>` のパス 1 要素になるため、パスの閉じ込めも兼ねる。
/// 一覧に出す条件にも使う（provider 側も同じ表で検証する）。ID の規則（英数字のみ）は別で、#1020 で足す。
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

/// 旧方式の重複回避名。`n == 0` は元の名前、それ以外は「名前 (n).拡張子」（拡張子は最後の `.` 以降）。
/// ずらした名前も [`validate_name`] を通す。再利用しない書類 ID 方式では表示名の重複を許すため
/// 使わなくなる。#1020 で本関数ごと削除する（それまでは現行の `put` が使う）。
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
///
/// 契約は再利用しない書類 ID 方式（`<dir>/<id>/<表示名>`、モジュール doc）。現行の実装は旧方式
/// （`<dir>/<名前>` に平らに置き、名前で消す）のままで、#1020 が契約に合わせて書き直す。
/// Java 側の `DocumentsProvider` とは作成・削除の排他を共有しない（ID が衝突しないため）。
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
