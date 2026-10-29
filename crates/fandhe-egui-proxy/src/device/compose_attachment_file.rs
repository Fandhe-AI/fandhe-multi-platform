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
    /// 渡した後に残している、このプロセスが作ったディレクトリと渡した時刻。
    pub(crate) kept: Vec<(PathBuf, Instant)>,
    /// 削除に失敗した、このプロセスが作ったディレクトリ。
    pub(crate) stale: Vec<PathBuf>,
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
        let root = self.root();
        let fail = || DeviceError::Internal(ATTACHMENT_FILE);
        std::fs::create_dir_all(&root).map_err(|_| fail())?;
        // 置き場がシンボリックリンクに差し替えられていないことを、掃除（削除）より先に確かめる。
        let meta = std::fs::symlink_metadata(&root).map_err(|_| fail())?;
        if !meta.is_dir() {
            return Err(fail());
        }
        let stale = std::mem::take(&mut self.stale);
        for p in stale {
            self.remove_or_stale(p);
        }
        let (old, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.kept)
            .into_iter()
            .partition(|(_, t)| now.saturating_duration_since(*t) >= RECYCLE_AFTER);
        self.kept = kept;
        for (p, _) in old {
            self.remove_or_stale(p);
        }
        if !self.stale.is_empty() {
            return Err(DeviceError::Internal(CLEANUP));
        }
        // 既存のディレクトリは再利用しない。PID 再利用で前プロセスの残りと同名になったら連番を進めて別名を取る。
        let mut tries = 0;
        let dir = loop {
            self.seq += 1;
            let dir = root.join(format!("{}-{}", std::process::id(), self.seq));
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 100 => tries += 1,
                Err(_) => return Err(fail()),
            }
        };
        // 作った直後に実ディレクトリであること（シンボリックリンクでない）を確かめてから中へ書く。
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            self.remove_or_stale(dir);
            return Err(fail());
        }
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

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_rejected_without_deleting_target() {
        let (mut s, root) = test_store();
        let target = root.with_extension("target");
        let victim = target.join("1-1");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("x"), b"x").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::open(&victim)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::os::unix::fs::symlink(&target, &root).unwrap();
        let e = s.prepare(&att(), Instant::now()).unwrap_err();
        assert!(matches!(e, DeviceError::Internal(l) if l == ATTACHMENT_FILE));
        assert!(victim.join("x").exists());
        let _ = std::fs::remove_file(&root);
        let _ = std::fs::remove_dir_all(target);
    }

    #[test]
    fn reused_pid_leftover_does_not_block_prepare() {
        let (mut s, root) = test_store();
        let clash = root.join(format!("{}-1", std::process::id()));
        std::fs::create_dir_all(&clash).unwrap();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        assert_ne!(f.dir, clash);
        assert!(clash.exists());
        s.discard(f);
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
    fn files_outside_own_list_are_never_touched() {
        let (mut s, root) = test_store();
        // 別プロセスが残したもの（古くても、自プロセスの一覧に無ければ走査も回収もしない）。
        let foreign = root.join("99999-1");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::File::open(&foreign)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - RECYCLE_AFTER * 2)
            .unwrap();
        let t0 = Instant::now();
        let f = s.prepare(&att(), t0).unwrap();
        s.keep(f, t0);
        let g = s.prepare(&att(), t0 + RECYCLE_AFTER * 2).unwrap();
        assert!(foreign.exists());
        s.discard(g);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_recycle_of_old_kept_is_retained_and_blocks_next_prepare() {
        let (mut s, root) = test_store();
        let t0 = Instant::now();
        let f = s.prepare(&att(), t0).unwrap();
        let dir = f.dir.clone();
        s.keep(f, t0);
        s.remove = fail_remove;
        let e = s.prepare(&att(), t0 + RECYCLE_AFTER).unwrap_err();
        assert!(matches!(e, DeviceError::Internal(l) if l == CLEANUP));
        assert_eq!(s.stale, vec![dir.clone()]);
        assert!(s.kept.is_empty() && dir.exists());
        s.remove = remove_dir;
        let g = s.prepare(&att(), t0 + RECYCLE_AFTER).unwrap();
        assert!(!dir.exists() && s.stale.is_empty());
        s.discard(g);
        let _ = std::fs::remove_dir_all(root);
    }
}
