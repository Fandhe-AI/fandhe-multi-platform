//! 自アプリの保存先を OS のファイル画面に出す（Phase 11 の拡充ループ、親 #194、Issue #1019）。
//!
//! 扱う範囲は「アプリ専用フォルダの書類を、一覧する・見本を置く・消す」ところまで。
//! フォルダを OS のファイル画面（Android の Files アプリ等）へ見せる provider 本体は OS 側
//! （Android は `DocumentsProvider`）が担い、本モジュールはフォルダを引数に取る
//! 共通実装 [`DirFileProvider`] と契約を持つ。呼び出し元は `fandhe-egui-proxy` の
//! `device/file_provider.rs`（画面向け effect ドライバ）。対象 OS は Android だけで、
//! iOS・macOS・Windows は保留、Web は対象外。
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
//!   （旧方式の `dedup_name`・`MAX_DEDUP` は削除済み）。置くは常に新しい ID の書類を作るので、
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
//!   閉じ込めも兼ねる）。ID は英数字のみで、空・長すぎるものを拒否する。Java 側も同じ表で検証する
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
/// 書類は直下の ID サブフォルダ `<FOLDER_NAME>/<id>/<表示名>` に 1 つずつ置く。
pub const FOLDER_NAME: &str = "file-provider";
/// 表示名の UTF-8 バイト数の上限。
pub const MAX_NAME_BYTES: usize = 255;
/// 書類 ID の文字数の上限（英数字のみ）。
pub const MAX_ID_LEN: usize = 64;
/// 「既に無い」を表す `Internal` の識別子（`DeviceError` に列挙子を足さない）。
pub const NOT_FOUND: &str = "file_provider not_found";
/// 名前（表示名と ID）の規則違反を表す `InvalidInput` の識別子。
pub const INVALID_NAME: &str = "file_provider name";
const INTERNAL: &str = "file_provider";

/// 一覧の 1 項目（1 書類）。識別（削除・URI）は `id`、`name` は表示だけに使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 再利用しない書類 ID（英数字のみ。`<id>/<表示名>` の ID）。
    pub id: String,
    /// 表示名（ユーザーに見せるだけで、識別には使わない。重複しうる）。
    pub name: String,
    /// バイト数。
    pub size: u64,
    /// 更新時刻（UNIX エポックからのミリ秒、表示専用）。取れなければ `None`。
    pub modified_unix_ms: Option<u64>,
}

/// 置いた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    /// 新しく採番した書類 ID。
    pub id: String,
    /// 置いた書類の表示名（渡した名前のまま。重複を許しずらさない）。
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
/// 識別は再利用しない書類 ID で行う（モジュール doc の「書類 ID 方式」）。
pub trait FileProvider: Send + Sync {
    /// 書類を表示名順（同名は ID 順）で返す。対象は「ID サブフォルダの中身がちょうど 1 つの通常ファイル」
    /// だけで、作りかけ（空・2 つ以上）は出さず失敗にもしない。失敗は I/O エラーに限り、1 件でも
    /// 読めなければ全体を失敗にする。
    fn list(&self) -> Result<Vec<Entry>, DeviceError>;
    /// `base_name` を表示名として、新しい ID の書類を置く。表示名の重複は許し、既存の書類を上書きも
    /// ずらしもしない。ID は `create_dir` で一意性を確かめて採番する。
    fn put(&self, base_name: &str, content: &[u8]) -> Result<PutOutcome, DeviceError>;
    /// 書類を消す。削除キーは ID。ファイルを消してから ID のサブフォルダを消し、サブフォルダの
    /// 削除失敗は握りつぶさず `Err(Internal)` にする。消した ID は二度と使わない。
    /// ID のサブフォルダが無ければ [`NOT_FOUND`]。
    fn delete(&self, id: &str) -> Result<DeleteOutcome, DeviceError>;
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
    fn delete(&self, _id: &str) -> Result<DeleteOutcome, DeviceError> {
        Err(DeviceError::Unsupported("file_provider"))
    }
}

/// 表示名の規則。空・`.`・`..`・`/`・NUL・制御文字・先頭 `.`・255 バイト超を `InvalidInput` で拒否する。
/// 表示名は `<id>/<表示名>` のパス 1 要素になるため、パスの閉じ込めも兼ねる。
/// 一覧に出す条件にも使う（provider 側も同じ表で検証する）。ID の規則は [`validate_id`]。
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

/// 書類 ID の規則。空・ASCII 英数字以外・[`MAX_ID_LEN`] 超を `InvalidInput` で拒否する。
/// `/`・`..`・NUL を含められないため、パスの閉じ込めも兼ねる（Java 側も同じ表で検証する）。
pub fn validate_id(id: &str) -> Result<(), DeviceError> {
    if id.is_empty() || id.len() > MAX_ID_LEN || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Err(DeviceError::InvalidInput(INVALID_NAME))
    } else {
        Ok(())
    }
}

/// 新しい書類 ID（小文字 16 進 32 桁）。
///
/// ponytail: `RandomState`（プロセスごとに OS 乱数から鍵を取る SipHash）にカウンタと時刻を混ぜる簡易版。
/// 暗号論的な予測不能性は契約外（URI の権限は URI 単位で与えられ、ID を推測できても権限は得られない）。
/// 一意性は呼び出し側の `create_dir` が OS に確かめさせる。必要になれば getrandom 等へ差し替える。
fn new_id() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let rs = RandomState::new();
    let mut out = String::with_capacity(32);
    for salt in 0u8..2 {
        out.push_str(&format!("{:016x}", rs.hash_one((n, now, salt))));
    }
    out
}

/// ID の採番を作り直す上限。
const MAX_ID_TRIES: usize = 8;

/// フォルダを引数に取る共通実装。Android は `data_dir()/FOLDER_NAME` と JNI の通知を渡し、
/// ホストのテストは一時フォルダと偽の notifier を渡す。
///
/// 配置は `<dir>/<id>/<表示名>`（モジュール doc）。Java 側の `DocumentsProvider` とは作成・削除の
/// 排他を共有しない（ID が衝突しないため）。
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

/// ID サブフォルダ `sub` の中身を 1 書類として読む。作りかけ・規則外は `Ok(None)`。
/// 他経路が同時に消したこと（`NotFound`）も飛ばす。それ以外の I/O エラーは `Err`。
fn read_doc(id: String, sub: &std::path::Path) -> io::Result<Option<Entry>> {
    let mut items = match fs::read_dir(sub) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(first) = items.next() else {
        return Ok(None);
    };
    let first = first?;
    if items.next().is_some() {
        return Ok(None);
    }
    // `DirEntry::metadata` はシンボリックリンクを辿らない。
    let meta = match first.metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_file() {
        return Ok(None);
    }
    let Ok(name) = first.file_name().into_string() else {
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
        id,
        name,
        size: meta.len(),
        modified_unix_ms,
    }))
}

/// 直下の 1 項目を書類候補として読む。ID の規則外・ディレクトリ以外は `Ok(None)`。
fn to_entry(item: io::Result<fs::DirEntry>) -> io::Result<Option<Entry>> {
    let item = item?;
    if !item.file_type()?.is_dir() {
        return Ok(None);
    }
    let Ok(id) = item.file_name().into_string() else {
        return Ok(None);
    };
    if validate_id(&id).is_err() {
        return Ok(None);
    }
    read_doc(id, &item.path())
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
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

/// 書き込み `write` を実行し、失敗したら作りかけを書類のファイル→ID のサブフォルダの順に消してから
/// `Err(Internal)` を返す。後始末自体が失敗しても `Err` のまま返し、残りは一覧に出ない。
fn write_or_cleanup(
    file: &std::path::Path,
    sub: &std::path::Path,
    write: impl FnOnce() -> io::Result<()>,
) -> Result<(), DeviceError> {
    if write().is_err() {
        let _ = fs::remove_file(file);
        let _ = fs::remove_dir(sub);
        return Err(DeviceError::Internal(INTERNAL));
    }
    Ok(())
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
        for _ in 0..MAX_ID_TRIES {
            let id = new_id();
            let sub = self.dir.join(&id);
            match fs::create_dir(&sub) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(DeviceError::Internal(INTERNAL)),
            }
            let file = sub.join(base_name);
            let mut f = match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
            {
                Ok(f) => f,
                Err(_) => {
                    let _ = fs::remove_dir(&sub);
                    return Err(DeviceError::Internal(INTERNAL));
                }
            };
            write_or_cleanup(&file, &sub, || {
                f.write_all(content)?;
                f.sync_all()
            })?;
            return Ok(PutOutcome {
                id,
                name: base_name.to_string(),
                notified: (self.notify)().is_ok(),
            });
        }
        Err(DeviceError::Internal(INTERNAL))
    }

    fn delete(&self, id: &str) -> Result<DeleteOutcome, DeviceError> {
        validate_id(id)?;
        let sub = self.dir.join(id);
        // シンボリックリンクを辿らず、ディレクトリだけを対象にする。
        let meta = fs::symlink_metadata(&sub).map_err(|e| map_remove_err(&e))?;
        if !meta.is_dir() {
            return Err(DeviceError::Internal(INTERNAL));
        }
        for item in fs::read_dir(&sub).map_err(|e| map_remove_err(&e))? {
            let item = item.map_err(|e| map_remove_err(&e))?;
            if item.file_type().map_err(|e| map_remove_err(&e))?.is_dir() {
                return Err(DeviceError::Internal(INTERNAL));
            }
            fs::remove_file(item.path()).map_err(|e| map_remove_err(&e))?;
        }
        fs::remove_dir(&sub).map_err(|e| map_remove_err(&e))?;
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
    fn validate_id_rules() {
        let long = "a".repeat(MAX_ID_LEN + 1);
        for n in ["", "a/b", "..", "a.b", "a_b", "日本", "a\0", long.as_str()] {
            assert!(
                matches!(validate_id(n), Err(DeviceError::InvalidInput(INVALID_NAME))),
                "{n:?}"
            );
        }
        for n in ["abc123", "A", "a".repeat(MAX_ID_LEN).as_str()] {
            assert!(validate_id(n).is_ok(), "{n:?}");
        }
        assert!(validate_id(&new_id()).is_ok());
    }

    #[test]
    fn put_lays_out_id_dir_and_never_overwrites_same_name() {
        let t = Tmp::new("put");
        let (p, calls) = provider(t.0.clone(), true);
        let a = p.put("a.txt", b"one").unwrap();
        let b = p.put("a.txt", b"two").unwrap();
        assert!(a.notified && a.name == "a.txt");
        assert_ne!(a.id, b.id);
        assert_eq!(fs::read(t.0.join(&a.id).join("a.txt")).unwrap(), b"one");
        assert_eq!(fs::read(t.0.join(&b.id).join("a.txt")).unwrap(), b"two");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let v = p.list().unwrap();
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|e| e.name == "a.txt"));
    }

    #[test]
    fn put_creates_missing_dir_and_reports_notify_failure() {
        let t = Tmp::new("notify");
        let (p, _) = provider(t.0.join("sub"), false);
        let r = p.put("a.txt", b"x").unwrap();
        assert!(!r.notified);
        assert!(t.0.join("sub").join(&r.id).join("a.txt").is_file());
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
    fn write_failure_removes_partial_file_then_dir() {
        let t = Tmp::new("wf");
        let sub = t.0.join("abc");
        fs::create_dir(&sub).unwrap();
        let file = sub.join("x");
        fs::write(&file, b"partial").unwrap();
        let r = write_or_cleanup(&file, &sub, || Err(io::Error::other("boom")));
        assert!(matches!(r, Err(DeviceError::Internal(_))));
        assert!(!sub.exists());
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
    fn list_sorted_by_name_then_id_and_skips_partial() {
        let t = Tmp::new("list");
        let (p, _) = provider(t.0.clone(), true);
        let mk = |id: &str, files: &[&str]| {
            fs::create_dir(t.0.join(id)).unwrap();
            for f in files {
                fs::write(t.0.join(id).join(f), b"x").unwrap();
            }
        };
        mk("bbb", &["b.txt"]);
        mk("ddd", &["a.txt"]);
        mk("ccc", &["a.txt"]);
        // 作りかけ・規則外は出さず、失敗にもしない。
        mk("empty", &[]);
        mk("two", &["a.txt", "b.txt"]);
        mk("hid", &[".hidden"]);
        fs::create_dir_all(t.0.join("dirin").join("sub")).unwrap();
        fs::create_dir_all(t.0.join("bad_id")).unwrap();
        fs::write(t.0.join("bad_id").join("a.txt"), b"x").unwrap();
        fs::write(t.0.join("flat.txt"), b"x").unwrap();
        let v = p.list().unwrap();
        let got: Vec<_> = v.iter().map(|e| (e.name.as_str(), e.id.as_str())).collect();
        assert_eq!(got, [("a.txt", "ccc"), ("a.txt", "ddd"), ("b.txt", "bbb")]);
        assert_eq!(v[0].size, 1);
    }

    #[test]
    fn collect_entries_fails_whole_on_one_error() {
        let ok = Entry {
            id: "a".into(),
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
        let (p, calls) = provider(dir.clone(), false);
        let r = p.put("a.txt", b"x").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(p.delete(&r.id).unwrap(), DeleteOutcome { notified: false });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!dir.join(&r.id).exists());
        assert!(matches!(
            p.delete(&r.id),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
        for bad in ["../outside", "a/b", ""] {
            assert!(matches!(p.delete(bad), Err(DeviceError::InvalidInput(_))));
        }
        assert!(outside.exists());
        // ID のサブフォルダ内にディレクトリがあれば消さず失敗。通常ファイルは ID の対象外。
        fs::create_dir_all(dir.join("nested").join("sub")).unwrap();
        assert!(matches!(p.delete("nested"), Err(DeviceError::Internal(m)) if m != NOT_FOUND));
        assert!(dir.join("nested").join("sub").is_dir());
        fs::write(dir.join("flat"), b"x").unwrap();
        assert!(matches!(p.delete("flat"), Err(DeviceError::Internal(m)) if m != NOT_FOUND));
        assert!(dir.join("flat").is_file());
    }

    /// 受入基準: 消した書類の ID は、同じ表示名で作り直しても引き継がれない。
    #[test]
    fn deleted_id_is_never_reused_for_same_name() {
        let t = Tmp::new("reuse");
        let (p, _) = provider(t.0.clone(), true);
        let id1 = p.put("a.txt", b"old").unwrap().id;
        p.delete(&id1).unwrap();
        let id2 = p.put("a.txt", b"new").unwrap().id;
        assert_ne!(id1, id2);
        assert!(!t.0.join(&id1).exists());
        assert!(matches!(
            p.delete(&id1),
            Err(DeviceError::Internal(NOT_FOUND))
        ));
        let v = p.list().unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, id2);
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
