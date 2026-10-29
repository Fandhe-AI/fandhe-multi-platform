//! メール作成画面へ渡す添付の一時ファイルの置き場（Issue #1042、親 #156）。
//! `device/macos_compose_mail_sms.rs`（`NSSharingService`、渡した後も残す）と、後続 #1043 の
//! `device/windows_compose_mail_sms.rs`（Simple MAPI、戻ったらすぐ消す）が共用する。契約は
//! `fandhe_device::compose_mail_sms` のモジュール doc。
//!
//! 置き場は `temp_dir()/fandhe-compose/<pid>-<seq>/<検査済みファイル名>`。ファイル名は
//! `ComposeAttachment::new` が `[A-Za-z0-9._-]`（先頭 `.` 不可）に限るため、`..` やパス区切りは入らない。
//! 呼び出し側は [`Store::prepare`] の後に必ず [`Store::keep`] か [`Store::discard`] を 1 回呼ぶ
//! （契約表 9）。解放に失敗したものは `stale` に積み、次の [`Store::prepare`] の前に消し直す。
//! 消せなければ OS を呼ばせず `CLEANUP` で失敗させる。経過時間は単調時計（`Instant`）で測る。
//!
//! [`AttachmentFile`] は [`Store`] を借用しない。Windows が modal の間ロックを握らずに済むよう、
//! lock → prepare → unlock → OS → lock → discard の順で使える。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fandhe_device::compose_mail_sms::{ComposeAttachment, ATTACHMENT_FILE, CLEANUP};
use fandhe_device::DeviceError;

/// 渡した後に残す一時ファイルの回収までの時間（Rust 側だけに置く）。
const RECYCLE_AFTER: Duration = Duration::from_secs(600);

/// 用意した添付 1 件。`Drop` では何もしないので、呼び出し側が `keep` / `discard` を呼ぶ。
#[derive(Debug)]
pub(crate) struct AttachmentFile {
    dir: PathBuf,
    path: PathBuf,
}

impl AttachmentFile {
    /// OS へ渡すファイルのパス。
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// 一時ファイルの状態。プロセス内で 1 つ（[`STORE`]）。テストは自前の値を作る。
pub(crate) struct Store {
    /// `None` は `temp_dir()/fandhe-compose`。
    pub(crate) root: Option<PathBuf>,
    seq: u64,
    pub(crate) kept: Vec<(PathBuf, Instant)>,
    pub(crate) stale: Vec<PathBuf>,
    swept: bool,
    pub(crate) remove: fn(&Path) -> io::Result<()>,
}

/// 本番の状態。要求は UI スレッド（macOS）か作業スレッド（Windows）から直列に来る。
pub(crate) static STORE: Mutex<Store> = Mutex::new(Store::new());

fn remove_dir(p: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

impl Store {
    pub(crate) const fn new() -> Self {
        Store {
            root: None,
            seq: 0,
            kept: Vec::new(),
            stale: Vec::new(),
            swept: false,
            remove: remove_dir,
        }
    }

    pub(crate) fn root(&self) -> PathBuf {
        self.root
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("fandhe-compose"))
    }

    /// 消して、失敗したら `stale` に積む。
    fn remove_or_stale(&mut self, p: PathBuf) {
        if (self.remove)(&p).is_err() {
            self.stale.push(p);
        }
    }

    /// 添付を一時ファイルへ書く。前回の消し残しが消せないときは OS を呼ばせず `CLEANUP`。
    pub(crate) fn prepare(
        &mut self,
        a: &ComposeAttachment,
        now: Instant,
    ) -> Result<AttachmentFile, DeviceError> {
        let stale = std::mem::take(&mut self.stale);
        for p in stale {
            self.remove_or_stale(p);
        }
        if !self.stale.is_empty() {
            return Err(DeviceError::Internal(CLEANUP));
        }
        let (old, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.kept)
            .into_iter()
            .partition(|(_, t)| now.saturating_duration_since(*t) >= RECYCLE_AFTER);
        self.kept = kept;
        for (p, _) in old {
            self.remove_or_stale(p);
        }
        let root = self.root();
        // 前のプロセスの残りは初回だけ掃除する（登録済みのものは触らない）。
        if !self.swept {
            self.swept = true;
            if let Ok(rd) = std::fs::read_dir(&root) {
                for e in rd.flatten() {
                    if !self.kept.iter().any(|(p, _)| *p == e.path()) {
                        self.remove_or_stale(e.path());
                    }
                }
            }
        }
        if !self.stale.is_empty() {
            return Err(DeviceError::Internal(CLEANUP));
        }
        let fail = || DeviceError::Internal(ATTACHMENT_FILE);
        std::fs::create_dir_all(&root).map_err(|_| fail())?;
        // 置き場がシンボリックリンクに差し替えられていないことを確かめる。
        let meta = std::fs::symlink_metadata(&root).map_err(|_| fail())?;
        if !meta.is_dir() {
            return Err(fail());
        }
        self.seq += 1;
        let dir = root.join(format!("{}-{}", std::process::id(), self.seq));
        std::fs::create_dir(&dir).map_err(|_| fail())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).is_err() {
                self.remove_or_stale(dir);
                return Err(fail());
            }
        }
        let path = dir.join(a.file_name());
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut f| f.write_all(a.data()));
        if written.is_err() {
            self.remove_or_stale(dir);
            return Err(fail());
        }
        Ok(AttachmentFile { dir, path })
    }

    /// 受け手が後から読むため残す。次の添付要求で 10 分を過ぎていれば回収する。
    pub(crate) fn keep(&mut self, f: AttachmentFile, now: Instant) {
        self.kept.push((f.dir, now));
    }

    /// すぐ消す。失敗は `stale` に積み、次の添付要求の前に消し直す。
    pub(crate) fn discard(&mut self, f: AttachmentFile) {
        self.remove_or_stale(f.dir);
    }
}

/// `STORE` のロックを取る。poison は中身を使い続ける（`request_slot.rs` と同じ）。
pub(crate) fn lock() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// テスト用: 一意な root を持つ `Store`（pid と連番で並列テストと衝突しない）。
#[cfg(test)]
pub(crate) fn test_store() -> (Store, PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "fandhe-compose-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let mut s = Store::new();
    s.root = Some(root.clone());
    (s, root)
}

/// テスト用: 常に失敗する削除。
#[cfg(test)]
pub(crate) fn fail_remove(_: &Path) -> io::Result<()> {
    Err(io::Error::other("x"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att() -> ComposeAttachment {
        ComposeAttachment::new("fandhe-sample.txt", "text/plain", b"hello\n").unwrap()
    }

    #[test]
    fn prepare_writes_file_and_discard_removes_it() {
        let (mut s, root) = test_store();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        assert_eq!(std::fs::read(f.path()).unwrap(), b"hello\n");
        let dir = f.dir.clone();
        s.discard(f);
        assert!(!dir.exists() && s.stale.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn kept_file_is_recycled_at_ten_minutes_on_monotonic_clock() {
        let (mut s, root) = test_store();
        let t0 = Instant::now();
        let f = s.prepare(&att(), t0).unwrap();
        let dir = f.dir.clone();
        s.keep(f, t0);
        let g = s
            .prepare(&att(), t0 + RECYCLE_AFTER - Duration::from_millis(1))
            .unwrap();
        assert!(dir.exists());
        s.discard(g);
        let g = s.prepare(&att(), t0 + RECYCLE_AFTER).unwrap();
        assert!(!dir.exists());
        s.discard(g);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn write_failure_returns_label_and_leaves_no_dir() {
        let (mut s, root) = test_store();
        std::fs::create_dir_all(&root).unwrap();
        // 次の `<pid>-<seq>` と同名のファイルを置いて `create_dir` を失敗させる。
        s.swept = true;
        std::fs::write(root.join(format!("{}-1", std::process::id())), b"x").unwrap();
        let e = s.prepare(&att(), Instant::now()).unwrap_err();
        assert!(matches!(e, DeviceError::Internal(l) if l == ATTACHMENT_FILE));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_removal_blocks_next_prepare_until_it_succeeds() {
        let (mut s, root) = test_store();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        let dir = f.dir.clone();
        s.remove = fail_remove;
        s.discard(f);
        assert_eq!(s.stale, vec![dir.clone()]);
        let e = s.prepare(&att(), Instant::now()).unwrap_err();
        assert!(matches!(e, DeviceError::Internal(l) if l == CLEANUP));
        assert!(dir.exists());
        s.remove = remove_dir;
        let g = s.prepare(&att(), Instant::now()).unwrap();
        assert!(!dir.exists() && s.stale.is_empty());
        s.discard(g);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn first_prepare_sweeps_foreign_leftovers_only_once() {
        let (mut s, root) = test_store();
        let old = root.join("99999-1");
        std::fs::create_dir_all(&old).unwrap();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        assert!(!old.exists());
        s.keep(f, Instant::now());
        // 2 回目以降は、登録済みでないものも新たには消さない。
        std::fs::create_dir_all(&old).unwrap();
        let g = s.prepare(&att(), Instant::now()).unwrap();
        assert!(old.exists());
        s.discard(g);
        let _ = std::fs::remove_dir_all(root);
    }
}
