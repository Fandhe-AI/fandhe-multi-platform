//! メール作成画面へ渡す添付の一時ファイルの置き場（Issue #1042、親 #156）。
//! `device/macos_compose_mail_sms.rs`（`NSSharingService`、渡した後も残す）と、#1043 の
//! `device/windows_compose_mail_sms.rs`（Simple MAPI、戻ったらすぐ消す）が共用する。契約は
//! `fandhe_device::compose_mail_sms` のモジュール doc。
//!
//! 置き場（root）は固定名にせず、プロセスごとに初回の [`Store::prepare`] で
//! `temp_dir()/fandhe-compose-<pid>-<n>` を `create_dir`（unix は 0o700）で新しく作る。既存のものは受け入れず
//! （`AlreadyExists` なら `n` を進める）、`TMPDIR=/tmp` のような共有の一時領域で別の利用者が先に作った
//! ディレクトリを使わない。root は作った直後に実ディレクトリか確かめて `Store` に覚え、以後使い回す。
//! 添付は `root/<pid>-<seq>/<検査済みファイル名>`。ファイル名は
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
    /// root を作る親。`None` は `temp_dir()`。テストが差し込む。
    pub(crate) parent: Option<PathBuf>,
    /// このプロセスが新しく作った root。初回の `prepare` まで `None`。
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
            parent: None,
            root: None,
            seq: 0,
            kept: Vec::new(),
            stale: Vec::new(),
            remove: remove_dir,
        }
    }

    /// root を返す。`prepare` の先頭（どの削除より前）で呼ぶ。覚えた root は毎回 `symlink_metadata` で確かめる。
    /// 実ディレクトリならそのまま返す。消えていれば（OS の一時領域の掃除等）中の `kept` / `stale` も無いので
    /// 空にして新しく作り直す。シンボリックリンク・ファイルへ差し替わった、または取得失敗なら、
    /// そのパス経由では何も消さず `CLEANUP` を返す（差し替え先を消さないため）。
    /// root が無ければ親の下に新しく作る（既存は受け入れない）。
    fn ensure_root(&mut self) -> Result<PathBuf, DeviceError> {
        if let Some(r) = &self.root {
            match std::fs::symlink_metadata(r) {
                Ok(m) if m.is_dir() => return Ok(r.clone()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    self.root = None;
                    self.kept.clear();
                    self.stale.clear();
                }
                _ => return Err(DeviceError::Internal(CLEANUP)),
            }
        }
        let fail = || DeviceError::Internal(ATTACHMENT_FILE);
        let parent = self.parent.clone().unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&parent).map_err(|_| fail())?;
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        for n in 0..100 {
            let root = parent.join(format!("fandhe-compose-{}-{n}", std::process::id()));
            match builder.create(&root) {
                Ok(()) => {
                    // 作った直後に実ディレクトリ（シンボリックリンクでない）か確かめる。
                    if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) {
                        return Err(fail());
                    }
                    self.root = Some(root.clone());
                    return Ok(root);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(fail()),
            }
        }
        Err(fail())
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
        let fail = || DeviceError::Internal(ATTACHMENT_FILE);
        let root = self.ensure_root()?;
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
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // Windows は `discard` だけ使う
    pub(crate) fn keep(&mut self, f: AttachmentFile, now: Instant) {
        self.kept.push((f.dir, now));
    }

    /// すぐ消す。失敗は `stale` に積み、次の添付要求の前に消し直す。
    pub(crate) fn discard(&mut self, f: AttachmentFile) {
        self.remove_or_stale(f.dir);
    }
}

/// `STORE` のロックを取る。poison は中身を使い続ける（`request_slot.rs` と同じ）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // Windows は引数つきの `Mutex<Store>` を直接ロックする
pub(crate) fn lock() -> std::sync::MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// テスト用: 一意な親を持つ `Store`（pid と連番で並列テストと衝突しない）。root は親の下に `prepare` が作る。
#[cfg(test)]
pub(crate) fn test_store() -> (Store, PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let parent = std::env::temp_dir().join(format!(
        "fandhe-compose-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let mut s = Store::new();
    s.parent = Some(parent.clone());
    (s, parent)
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

    /// 同名の既存ディレクトリ・シンボリックリンクは置き場に使わず、別名で新しく作る。
    #[cfg(unix)]
    #[test]
    fn preexisting_root_name_is_never_adopted() {
        let (mut s, parent) = test_store();
        std::fs::create_dir_all(&parent).unwrap();
        let pid = std::process::id();
        // 0 番は他人が先に作ったディレクトリ、1 番は別の場所を指すシンボリックリンク。
        let squat = parent.join(format!("fandhe-compose-{pid}-0"));
        std::fs::create_dir(&squat).unwrap();
        let target = parent.join("target");
        let victim = target.join("1-1");
        std::fs::create_dir_all(&victim).unwrap();
        std::os::unix::fs::symlink(&target, parent.join(format!("fandhe-compose-{pid}-1")))
            .unwrap();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        let root = s.root.clone().unwrap();
        assert_eq!(root, parent.join(format!("fandhe-compose-{pid}-2")));
        assert!(f.dir.starts_with(&root));
        assert_eq!(std::fs::read_dir(&squat).unwrap().count(), 0);
        assert!(victim.exists());
        s.discard(f);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[cfg(unix)]
    #[test]
    fn root_is_owner_only_and_reused_within_process() {
        use std::os::unix::fs::PermissionsExt;
        let (mut s, parent) = test_store();
        let f = s.prepare(&att(), Instant::now()).unwrap();
        let root = s.root.clone().unwrap();
        let mode = std::fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let g = s.prepare(&att(), Instant::now()).unwrap();
        assert_eq!(s.root.as_ref(), Some(&root));
        s.discard(f);
        s.discard(g);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn reused_pid_leftover_does_not_block_prepare() {
        let (mut s, root) = test_store();
        // 1 回目の prepare で root を作り、次に取られる連番のディレクトリを先に置いておく。
        let first = s.prepare(&att(), Instant::now()).unwrap();
        let clash = s
            .root
            .clone()
            .unwrap()
            .join(format!("{}-2", std::process::id()));
        std::fs::create_dir_all(&clash).unwrap();
        s.discard(first);
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
        let first = s.prepare(&att(), Instant::now()).unwrap();
        s.discard(first);
        let foreign = s.root.clone().unwrap().join("99999-1");
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

    #[test]
    fn vanished_root_is_recreated_and_lists_are_cleared() {
        let (mut s, parent) = test_store();
        let t0 = Instant::now();
        let f = s.prepare(&att(), t0).unwrap();
        s.keep(f, t0);
        s.stale.push(PathBuf::from("gone"));
        std::fs::remove_dir_all(s.root.as_ref().unwrap()).unwrap();
        let g = s.prepare(&att(), t0).unwrap();
        let root = s.root.clone().unwrap();
        assert!(g.dir.starts_with(&root) && root.is_dir());
        assert!(s.kept.is_empty() && s.stale.is_empty());
        s.discard(g);
        let _ = std::fs::remove_dir_all(parent);
    }

    /// root がシンボリックリンクに差し替わったら、リンク先を消さずに `CLEANUP` で止める。
    #[cfg(unix)]
    #[test]
    fn swapped_root_stops_with_cleanup_and_spares_link_target() {
        let (mut s, parent) = test_store();
        let t0 = Instant::now();
        let f = s.prepare(&att(), t0).unwrap();
        s.keep(f, t0);
        let root = s.root.clone().unwrap();
        let target = parent.join("target");
        let victim = target.join("1-1");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&target, &root).unwrap();
        // 回収対象の時間を過ぎていても、リンク経由では何も消さない。
        let e = s.prepare(&att(), t0 + RECYCLE_AFTER).unwrap_err();
        assert!(matches!(e, DeviceError::Internal(l) if l == CLEANUP));
        assert!(victim.exists());
        let _ = std::fs::remove_dir_all(parent);
    }
}
