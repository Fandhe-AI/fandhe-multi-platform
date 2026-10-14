//! Phase 11 の拡充ループ（親 #1143、Issue #1410）: 作業フォルダ 1 つの「直下の通常ファイル」の
//! 作成・更新・削除を見張る契約。`fandhe-egui-proxy` の `device/file_watch.rs`（effect ドライバ）が
//! [`FileWatch::start`] で OS の監視を開始し、各 OS 実装（macOS: FSEvents #1410、Windows: #1411、
//! iOS: #1412、Android: #1413）が本トレイトを実装する。Web は `fandhe-web-dom::file_watch`（#1414）が
//! `FileSystemObserver` を使い、本トレイトは経由せず、純関数（[`folder_scan_from`]・[`diff`]・
//! サンプル名の規則）だけを共用する。
//!
//! # 方式: OS の通知は「合図」、記録は自前の再走査と差分
//!
//! OS の通知はまとめて届く・取りこぼす・順序が変わることがある。そのため通知の中身は読まず、
//! [`WatchSignal::Changed`] という合図として扱う。ドライバが合図ごとに [`scan`] で作業フォルダを
//! 走査し、前回との [`diff`] を取って記録にする。取りこぼし・あふれは `Changed` に畳めば
//! 再走査が吸収する。範囲は作業フォルダ直下の通常ファイルのみ（サブフォルダ・リンク・dot 始まり・
//! 非 UTF-8 名は対象外）。任意フォルダの監視は #1408。
//!
//! # 結果の種類（契約表 10）
//!
//! - 利用者の停止 = 正常。[`WatchGuard`] の drop は失敗を生まない
//! - 実行中の停止（フォルダ消失・走査失敗・[`WatchSignal::Failed`]）= 失敗。成功を装わない
//! - あふれ・取りこぼし（`MustScanSubDirs` / `*Dropped` / 0 バイト完了 / `ERROR_NOTIFY_ENUM_DIR` /
//!   Web の `unknown` 等）= `Changed`（失敗にしない）
//! - 非対応: `Err(Unsupported("file_watch"))`、開始の失敗: `Err(Internal("file_watch start"))`
//!
//! `PermissionDenied` は返さない（アプリ専用の作業フォルダで権限が要らない。Web の許可は画面側）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序（開始・停止・合図の処理は UI スレッドの 1 本の effect に集約）、
//! 嘘の成功の禁止（失敗・停止を成功へ畳まない、`truncated` を出す）、資源の解放
//! （[`WatchGuard`] の drop で OS 監視を止め、戻った時点でコールバックは走らない）。
//! 守らない範囲: 複数タブ・複数プロセス、同一ミリ秒の変化の順序、終了処理との重なり、
//! OS 側の非同期確定との時間差による一時的な表示の食い違い、[`MAX_SCAN_ENTRIES`] 超の件数、
//! 走査の間に起きた「作成して即削除」の検出。
//!
//! # 後続 OS の要点
//!
//! OS 資源は生ポインタではなく `id`（ドライバの登録表が発行）で所有し、停止後に遅れて届いた
//! コールバックは登録表に無い `id` として捨てる。停止は「登録解除 → OS 停止 → 実行中コールバックの
//! 終了待ち → 解放」の順で、ロックの内側では待たない。
//!
//! # 既知の天井
//!
//! ponytail: 走査は UI スレッドで同期。全件集めて名前順に並べるため、作業フォルダに数十万件を
//! 置かれると固まる。アプリ専用領域なので許容し、問題になれば上位 64 件の選択かワーカーへ移す。

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::DeviceError;

/// 1 回の走査で残す最大件数。仮置きで、実機で足りなければ上げる。
pub const MAX_SCAN_ENTRIES: usize = 64;

/// サンプル操作の対象にするファイル名の番号の上限（`sample-{1..=999999}.txt`）。
pub const MAX_SAMPLE_NUMBER: u64 = 999_999;
/// `update_sample` が追記を続ける上限。超えたら固定の初期内容へ戻す。
pub const MAX_SAMPLE_BYTES: u64 = 64 * 1024;
/// サンプルファイルの初期内容。
pub const SAMPLE_INITIAL: &str = "sample\n";
/// 更新ごとに足す 1 行（サイズを確実に増やして `Modified` を出す）。
const SAMPLE_APPEND: &str = "update\n";

/// 走査で見つけた通常ファイル 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// ファイル名（UTF-8、dot 始まりでない）。
    pub name: String,
    /// バイト数。
    pub size: u64,
    /// 更新時刻。等値比較にだけ使い、差は取らない（壁時計の巻き戻りで壊れない）。
    pub modified: Option<SystemTime>,
}

/// フォルダ走査の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderScan {
    /// 名前の昇順で最大 [`MAX_SCAN_ENTRIES`] 件。
    pub files: Vec<FileEntry>,
    /// 上限で切り捨てたか。
    pub truncated: bool,
}

/// 変化の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// 新しく現れた。
    Created,
    /// 同名でサイズか更新時刻が変わった。
    Modified,
    /// 無くなった。名前の変更は `Deleted` + `Created` として出る。
    Deleted,
}

/// 差分 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// 種類。
    pub kind: ChangeKind,
    /// ファイル名。
    pub name: String,
}

/// OS から届く合図。中身（どのファイルか）は持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSignal {
    /// 何か変わったかもしれない（あふれ・取りこぼしも含む）。再走査で確かめる。
    Changed,
    /// OS の監視が実行中に止まった（フォルダ消失等）。失敗として扱う。
    Failed,
}

/// 実行中の監視。drop で停止する。契約: drop が戻った時点でコールバックはもう走らず、OS 資源は解放済み。
pub trait WatchGuard: Send {}

/// 合図の送り先。任意のスレッドから呼ばれうる。
pub type OnSignal = Arc<dyn Fn(WatchSignal) + Send + Sync>;

/// 作業フォルダの監視を開始する。
pub trait FileWatch: Send + Sync {
    /// `dir` の監視を開始し、合図を `on_signal` へ送る。停止は返した [`WatchGuard`] の drop。
    /// 開始に失敗したら OS 資源を残さず `Err` を返す。
    fn start(&self, dir: &Path, on_signal: OnSignal) -> Result<Box<dyn WatchGuard>, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedFileWatch;

impl FileWatch for UnsupportedFileWatch {
    fn start(&self, _dir: &Path, _on_signal: OnSignal) -> Result<Box<dyn WatchGuard>, DeviceError> {
        Err(DeviceError::Unsupported("file_watch"))
    }
}

/// 通常ファイルなら `FileEntry` を返す。リンク・ディレクトリ・dot 始まり・非 UTF-8・stat 失敗は `None`。
fn entry_of(e: &fs::DirEntry) -> Option<FileEntry> {
    let name = e.file_name().into_string().ok()?;
    if name.starts_with('.') {
        return None;
    }
    // リンクは辿らない（symlink_metadata）。
    let md = fs::symlink_metadata(e.path()).ok()?;
    md.file_type().is_file().then(|| FileEntry {
        name,
        size: md.len(),
        modified: md.modified().ok(),
    })
}

/// `dir` 直下の通常ファイルを走査する。`Ok` で空は「0 件」の成功、`read_dir` 自体の失敗は `Err`。
pub fn scan(dir: &Path) -> Result<FolderScan, DeviceError> {
    let rd = fs::read_dir(dir).map_err(|_| DeviceError::Internal("file_watch scan"))?;
    // 走査中に消えた等のエントリ単位の失敗は、そのエントリだけ捨てる。
    Ok(folder_scan_from(
        rd.filter_map(|e| entry_of(&e.ok()?)).collect(),
    ))
}

/// 集めた通常ファイルを名前の昇順に並べ、[`MAX_SCAN_ENTRIES`] で切り捨てる（順序不定の入力でも
/// 同じ結果になる）。ネイティブの [`scan`] と Web の一覧取りが共用する。
pub fn folder_scan_from(mut files: Vec<FileEntry>) -> FolderScan {
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let truncated = files.len() > MAX_SCAN_ENTRIES;
    files.truncate(MAX_SCAN_ENTRIES);
    FolderScan { files, truncated }
}

/// 前回と今回の走査の差分を名前順に返す（純関数）。
pub fn diff(prev: &FolderScan, next: &FolderScan) -> Vec<FileChange> {
    use std::cmp::Ordering;
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    // 切り捨てた側の末尾名より後ろは未観測。存在・不存在を断定せず、両側で見える範囲だけ比べる。
    let bound = [prev, next]
        .iter()
        .filter(|s| s.truncated)
        .filter_map(|s| s.files.last().map(|f| f.name.as_str()))
        .min();
    let visible = |s: &FolderScan| -> Vec<FileEntry> {
        s.files
            .iter()
            .filter(|f| bound.is_none_or(|b| f.name.as_str() <= b))
            .cloned()
            .collect()
    };
    let (p, n) = (&visible(prev), &visible(next));
    let mut push = |kind, name: &str| {
        out.push(FileChange {
            kind,
            name: name.to_string(),
        })
    };
    while i < p.len() || j < n.len() {
        let ord = match (p.get(i), n.get(j)) {
            (Some(a), Some(b)) => a.name.cmp(&b.name),
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Greater,
        };
        match ord {
            Ordering::Less => {
                push(ChangeKind::Deleted, &p[i].name);
                i += 1;
            }
            Ordering::Greater => {
                push(ChangeKind::Created, &n[j].name);
                j += 1;
            }
            Ordering::Equal => {
                if p[i].size != n[j].size || p[i].modified != n[j].modified {
                    push(ChangeKind::Modified, &n[j].name);
                }
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// `sample-{1..6 桁}.txt` なら番号を返す。
pub fn sample_number(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("sample-")?.strip_suffix(".txt")?;
    // 先頭ゼロは `sample_name` が作る名前と一致しないため対象外。
    let ok = (1..=6).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| digits.parse().ok()).flatten()
}

/// 番号からサンプルのファイル名を作る。
pub fn sample_name(n: u64) -> String {
    format!("sample-{n}.txt")
}

/// サンプルの通常ファイルのうち最大番号を返す（64 件の窓に依らず `read_dir` 全体から探す）。
fn max_sample(dir: &Path) -> Result<Option<u64>, DeviceError> {
    let rd = fs::read_dir(dir).map_err(|_| DeviceError::Internal("file_watch sample"))?;
    Ok(rd
        .filter_map(|e| entry_of(&e.ok()?))
        .filter_map(|f| sample_number(&f.name))
        .max())
}

/// dot 始まりの一時ファイルへ書いて rename で差し替える（監視側に中途半端な内容を見せない）。
fn write_atomically(dir: &Path, name: &str, body: &[u8]) -> Result<(), DeviceError> {
    let tmp = dir.join(format!(".{name}.tmp"));
    // create_new で排他的に作る。既存の一時パス（リンク含む）は上書きも削除もせず失敗にする。
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|_| DeviceError::Internal("file_watch sample"))?;
    let r = f
        .write_all(body)
        .and_then(|()| fs::rename(&tmp, dir.join(name)));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r.map_err(|_| DeviceError::Internal("file_watch sample"))
}

/// 既存の最大番号から、次に作るサンプルの番号を返す。上限超過は `None`。
pub fn next_sample_number(max: Option<u64>) -> Option<u64> {
    let next = max.map_or(Some(1), |n| n.checked_add(1))?;
    (next <= MAX_SAMPLE_NUMBER).then_some(next)
}

/// 更新後のサンプル本文を作る。`current` が `None`（[`MAX_SAMPLE_BYTES`] 超過）なら初期内容へ戻す。
pub fn next_sample_body(current: Option<&[u8]>) -> Vec<u8> {
    let mut body = current.map_or_else(|| SAMPLE_INITIAL.as_bytes().to_vec(), <[u8]>::to_vec);
    body.extend_from_slice(SAMPLE_APPEND.as_bytes());
    body
}

/// 既存の最大番号 + 1 のサンプルを作り、その名前を返す。
pub fn create_sample(dir: &Path) -> Result<String, DeviceError> {
    let next =
        next_sample_number(max_sample(dir)?).ok_or(DeviceError::Internal("file_watch sample"))?;
    let name = sample_name(next);
    write_atomically(dir, &name, SAMPLE_INITIAL.as_bytes())?;
    Ok(name)
}

/// 最大番号のサンプルに 1 行追記してサイズを増やす。対象が無ければ `Ok(false)`。
pub fn update_sample(dir: &Path) -> Result<bool, DeviceError> {
    let Some(n) = max_sample(dir)? else {
        return Ok(false);
    };
    let name = sample_name(n);
    let path = dir.join(&name);
    let err = |_| DeviceError::Internal("file_watch sample");
    let len = fs::metadata(&path).map_err(err)?.len();
    let current = if len <= MAX_SAMPLE_BYTES {
        Some(fs::read(&path).map_err(err)?)
    } else {
        None
    };
    let body = next_sample_body(current.as_deref());
    write_atomically(dir, &name, &body)?;
    Ok(true)
}

/// 最大番号のサンプルを消す。対象が無ければ `Ok(false)`。
pub fn delete_sample(dir: &Path) -> Result<bool, DeviceError> {
    let Some(n) = max_sample(dir)? else {
        return Ok(false);
    };
    fs::remove_file(dir.join(sample_name(n)))
        .map_err(|_| DeviceError::Internal("file_watch sample"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "fandhe-file-watch-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(s: &FolderScan) -> Vec<&str> {
        s.files.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn scan_skips_dirs_dot_files_and_links() {
        let t = Tmp::new("scan");
        fs::write(t.0.join("b.txt"), "x").unwrap();
        fs::write(t.0.join("a.txt"), "xy").unwrap();
        fs::write(t.0.join(".hidden"), "x").unwrap();
        fs::create_dir(t.0.join("sub")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.0.join("a.txt"), t.0.join("link.txt")).unwrap();
        let s = scan(&t.0).unwrap();
        assert_eq!(names(&s), ["a.txt", "b.txt"]);
        assert_eq!(s.files[0].size, 2);
        assert!(!s.truncated);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn scan_skips_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;
        let t = Tmp::new("utf8");
        let bad = std::ffi::OsStr::from_bytes(b"bad-\xff.txt");
        fs::write(t.0.join(bad), "x").unwrap();
        fs::write(t.0.join("ok.txt"), "x").unwrap();
        assert_eq!(names(&scan(&t.0).unwrap()), ["ok.txt"]);
    }

    #[test]
    fn scan_truncates_deterministically() {
        let t = Tmp::new("trunc");
        for i in 0..(MAX_SCAN_ENTRIES + 5) {
            fs::write(t.0.join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let s = scan(&t.0).unwrap();
        assert_eq!(s.files.len(), MAX_SCAN_ENTRIES);
        assert!(s.truncated);
        assert_eq!(s.files[0].name, "f000.txt");
    }

    #[test]
    fn scan_empty_is_ok_and_missing_is_err() {
        let t = Tmp::new("empty");
        let s = scan(&t.0).unwrap();
        assert!(s.files.is_empty() && !s.truncated);
        assert!(matches!(
            scan(&t.0.join("nope")),
            Err(DeviceError::Internal("file_watch scan"))
        ));
    }

    fn fe(name: &str, size: u64, m: u64) -> FileEntry {
        FileEntry {
            name: name.into(),
            size,
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(m)),
        }
    }
    fn scan_of(files: Vec<FileEntry>) -> FolderScan {
        FolderScan {
            files,
            truncated: false,
        }
    }

    #[test]
    fn diff_detects_create_modify_delete_in_name_order() {
        let prev = scan_of(vec![
            fe("a", 1, 1),
            fe("b", 1, 1),
            fe("c", 1, 1),
            fe("d", 1, 1),
        ]);
        let next = scan_of(vec![
            fe("a", 1, 1),
            fe("b", 2, 1),
            fe("c", 1, 2),
            fe("e", 1, 1),
        ]);
        let d = diff(&prev, &next);
        let got: Vec<_> = d.iter().map(|c| (c.kind, c.name.as_str())).collect();
        assert_eq!(
            got,
            [
                (ChangeKind::Modified, "b"),
                (ChangeKind::Modified, "c"),
                (ChangeKind::Deleted, "d"),
                (ChangeKind::Created, "e"),
            ]
        );
        assert!(diff(&next, &next).is_empty());
    }

    #[test]
    fn rename_is_delete_plus_create() {
        let d = diff(&scan_of(vec![fe("a", 1, 1)]), &scan_of(vec![fe("b", 1, 1)]));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].kind, ChangeKind::Deleted);
        assert_eq!(d[1].kind, ChangeKind::Created);
    }

    #[test]
    fn samples_create_update_delete() {
        let t = Tmp::new("sample");
        assert_eq!(create_sample(&t.0).unwrap(), "sample-1.txt");
        assert_eq!(create_sample(&t.0).unwrap(), "sample-2.txt");
        let before = fs::metadata(t.0.join("sample-2.txt")).unwrap().len();
        assert!(update_sample(&t.0).unwrap());
        assert!(fs::metadata(t.0.join("sample-2.txt")).unwrap().len() > before);
        // 一時ファイルは残らない。
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 2);
        assert!(delete_sample(&t.0).unwrap());
        assert!(!t.0.join("sample-2.txt").exists() && t.0.join("sample-1.txt").exists());
    }

    #[test]
    fn samples_ignore_other_files_and_report_missing() {
        let t = Tmp::new("other");
        assert!(!update_sample(&t.0).unwrap());
        assert!(!delete_sample(&t.0).unwrap());
        fs::write(t.0.join("sample-1.txt.bak"), "x").unwrap();
        fs::write(t.0.join("other.txt"), "x").unwrap();
        fs::write(t.0.join("sample-1234567.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.0.join("other.txt"), t.0.join("sample-9.txt")).unwrap();
        assert!(!update_sample(&t.0).unwrap());
        assert!(!delete_sample(&t.0).unwrap());
        assert_eq!(create_sample(&t.0).unwrap(), "sample-1.txt");
    }

    #[test]
    fn diff_ignores_unobserved_tail_when_truncated() {
        let mut prev = scan_of(vec![fe("a", 1, 1), fe("b", 1, 1)]);
        prev.truncated = true;
        // b より後ろは未観測: c の出現を Created と断定しない。
        let next = scan_of(vec![fe("a", 1, 1), fe("b", 1, 1), fe("c", 1, 1)]);
        assert!(diff(&prev, &next).is_empty());
    }

    #[test]
    fn leading_zero_sample_is_ignored() {
        let t = Tmp::new("zero");
        fs::write(t.0.join("sample-01.txt"), "x").unwrap();
        assert!(!update_sample(&t.0).unwrap());
        assert_eq!(create_sample(&t.0).unwrap(), "sample-1.txt");
    }

    #[cfg(unix)]
    #[test]
    fn stale_tmp_symlink_is_not_followed() {
        let t = Tmp::new("tmplink");
        let victim = t.0.join("victim.txt");
        fs::write(&victim, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, t.0.join(".sample-1.txt.tmp")).unwrap();
        assert!(create_sample(&t.0).is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn update_resets_oversized_sample() {
        let t = Tmp::new("big");
        fs::write(
            t.0.join("sample-1.txt"),
            vec![b'x'; (MAX_SAMPLE_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(update_sample(&t.0).unwrap());
        assert!(fs::metadata(t.0.join("sample-1.txt")).unwrap().len() < 100);
    }

    #[test]
    fn folder_scan_from_is_order_independent_and_truncates() {
        let files: Vec<_> = (0..(MAX_SCAN_ENTRIES + 3))
            .rev()
            .map(|i| fe(&format!("f{i:03}"), 1, 1))
            .collect();
        let s = folder_scan_from(files);
        assert_eq!(s.files.len(), MAX_SCAN_ENTRIES);
        assert!(s.truncated);
        assert_eq!(s.files[0].name, "f000");
    }

    #[test]
    fn next_sample_number_has_ceiling() {
        assert_eq!(next_sample_number(None), Some(1));
        assert_eq!(next_sample_number(Some(5)), Some(6));
        assert_eq!(next_sample_number(Some(MAX_SAMPLE_NUMBER)), None);
        assert_eq!(
            next_sample_number(Some(MAX_SAMPLE_NUMBER - 1)),
            Some(MAX_SAMPLE_NUMBER)
        );
        assert_eq!(next_sample_number(Some(u64::MAX)), None);
        assert_eq!(next_sample_body(None), b"sample\nupdate\n");
    }

    #[test]
    fn unsupported_is_not_success() {
        let r = UnsupportedFileWatch.start(Path::new("."), Arc::new(|_| {}));
        assert!(matches!(r, Err(DeviceError::Unsupported("file_watch"))));
    }
}
