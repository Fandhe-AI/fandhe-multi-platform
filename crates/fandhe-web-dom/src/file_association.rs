//! 型サンプル「ファイルの関連付け」の Web 経路（親 #102、Issue #419）。
//!
//! `fandhe_core::screens::file_association_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。受信の検査・保持は
//! `fandhe_device::file_association`（契約の SSOT）の `FileInbox` と `accepts` を使う。
//! Web は `fandhe-device` の trait を通らない（`url_scheme.rs` と同じ理由）。
//!
//! # 方式
//!
//! PWA の manifest `file_handlers`（`web/manifest.webmanifest`）で `.txt` を宣言し、インストール済みの
//! アプリから開かれると `window.launchQueue` の consumer に `FileSystemFileHandle` が届く。
//! web-sys に `LaunchQueue` は無く、`FileSystemFileHandle` も unstable API のため
//! `js_sys::Reflect` で呼ぶ。読み取りは `file_picker::read` を使い回さず `read_launched` を置く
//! （`read` のエラーは文言で、大きすぎると読めないを型で区別できないため）。上限は
//! `MAX_FILE_BYTES` だけを参照する。
//!
//! consumer はマウントごとではなくページに 1 回だけ登録し、closure も 1 個だけ保持する
//! （`forget()` しない）。破棄済みマウントの時点で届いた launch を落とすと、trait doc の契約
//! 「受け口は static に置き、UI の有無に関係なく書き込む」に反するため。マウントごとの資源は
//! 通知先の `Applier` だけで、`release_mount` で必ず外す。
//! launch が連続しても受信順を保つため、キューに積んで 1 件ずつ await する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の受信順、非対応ブラウザで空の Waiting を成功として出さないこと、
//! 通常起動（`files` が空）を失敗として数えないこと、読めない・大きすぎるを成功にしないこと、
//! 名前・中身・例外を `console` へ出さないこと。
//! 守らない: 未インストールの PWA、Chromium 以外・Chrome Android（`launchQueue` が無く
//! 「未対応」を出す）、`launch_handler` を指定しないため複数ウィンドウが開きうること、複数タブ。

use fandhe_core::screens::file_association_show::{LatestFile, OpenFailureKind};
use fandhe_device::drag_drop::DropError;
use fandhe_device::file_association::{accepts, OpenFailure, OpenedFile};
use fandhe_device::file_picker::MAX_FILE_BYTES;

/// ハンドルを読んでよいか。ディレクトリ・名前が取れない・拡張子の不一致は拒否として数える。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn admit(kind: Option<&str>, name: Option<&str>) -> bool {
    kind == Some("file") && accepts(name, None)
}

/// サイズを検査する。`usize` へ変換する前に `f64` のまま比べ、NaN・負値も弾く。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn check_size(len: f64) -> Result<(), DropError> {
    if len >= 0.0 && len <= MAX_FILE_BYTES as f64 {
        Ok(())
    } else {
        Err(DropError::TooLarge)
    }
}

/// 画面側の失敗種別への写像（core は device に依存しないため、ここで橋渡しする）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_kind(f: OpenFailure) -> OpenFailureKind {
    match f {
        OpenFailure::TooLarge => OpenFailureKind::TooLarge,
        OpenFailure::Unreadable => OpenFailureKind::Unreadable,
        OpenFailure::Folder => OpenFailureKind::Folder,
    }
}

/// スナップショットの最新結果を画面の形へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn latest_file(r: Result<OpenedFile, OpenFailure>) -> LatestFile {
    match r {
        Ok(f) => Ok((f.name, f.size, f.head, f.received_at_unix_ms)),
        Err(e) => Err(failure_kind(e)),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::file_association_show_effect;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};

    use fandhe_core::screens::file_association_show::{
        apply_snapshot, apply_unsupported, set_web_hint, FileAssociationShowState,
    };
    use fandhe_device::drag_drop::DropError;
    use fandhe_device::file_association::FileInbox;
    use fandhe_device::file_picker::PickedFile;
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::File;

    use super::{admit, check_size, latest_file};
    use crate::mount::Applier;

    static INBOX: FileInbox = FileInbox::new();

    type Watcher = Applier<FileAssociationShowState>;
    type ConsumerSlot = RefCell<Option<Closure<dyn FnMut(JsValue)>>>;

    thread_local! {
        /// ページに 1 個だけの consumer closure（破棄しない。マウントごとに増えない）。
        static CONSUMER: ConsumerSlot = const { RefCell::new(None) };
        /// consumer 登録の結果（初回に確定）。
        static SUPPORTED: Cell<Option<bool>> = const { Cell::new(None) };
        /// 未処理のハンドル。受信順を保つため 1 件ずつ処理する。
        static QUEUE: RefCell<VecDeque<JsValue>> = const { RefCell::new(VecDeque::new()) };
        static DRAINING: Cell<bool> = const { Cell::new(false) };
        /// マウントごとの通知先。`None` は「非対応を送信済み」の印。
        static WATCHERS: RefCell<HashMap<u64, Option<Watcher>>> = RefCell::new(HashMap::new());
    }

    fn get(obj: &JsValue, key: &str) -> Option<JsValue> {
        Reflect::get(obj, &JsValue::from_str(key)).ok()
    }

    /// `launchQueue.setConsumer` を 1 回だけ登録する。無い・例外ならブラウザ非対応。
    fn install_consumer() -> bool {
        if let Some(v) = SUPPORTED.with(Cell::get) {
            return v;
        }
        let ok = try_install();
        SUPPORTED.with(|s| s.set(Some(ok)));
        ok
    }

    fn try_install() -> bool {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let w: JsValue = window.into();
        let Some(queue) = get(&w, "launchQueue").filter(JsValue::is_object) else {
            return false;
        };
        let Some(set) = get(&queue, "setConsumer").filter(JsValue::is_function) else {
            return false;
        };
        let closure = Closure::<dyn FnMut(JsValue)>::new(on_launch);
        let ok = Function::from(set)
            .call1(&queue, closure.as_ref().unchecked_ref())
            .is_ok();
        if ok {
            CONSUMER.with(|c| *c.borrow_mut() = Some(closure));
        }
        ok
    }

    /// launch ごとに呼ばれる。`files` が空（通常のナビゲーション起動）は何も数えない。
    fn on_launch(params: JsValue) {
        let Some(files) = get(&params, "files").filter(JsValue::is_object) else {
            return;
        };
        let files: Array = files.unchecked_into();
        if files.length() == 0 {
            return;
        }
        QUEUE.with(|q| q.borrow_mut().extend(files.iter()));
        if !DRAINING.with(|d| d.replace(true)) {
            wasm_bindgen_futures::spawn_local(drain());
        }
    }

    async fn drain() {
        while let Some(handle) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
            handle_one(handle).await;
            notify();
        }
        DRAINING.with(|d| d.set(false));
    }

    async fn handle_one(handle: JsValue) {
        let kind = get(&handle, "kind").and_then(|v| v.as_string());
        let name = get(&handle, "name").and_then(|v| v.as_string());
        if !admit(kind.as_deref(), name.as_deref()) {
            INBOX.reject();
            return;
        }
        let name = name.unwrap_or_default();
        let result = match get_file(&handle).await {
            Some(file) => read_launched(file, name).await,
            None => Err(DropError::Unreadable),
        };
        // 受信時刻は表示用なので壁時計でよい（経過時間には使わない）。
        INBOX.record(result, js_sys::Date::now() as u64);
    }

    async fn get_file(handle: &JsValue) -> Option<File> {
        let f = get(handle, "getFile").filter(JsValue::is_function)?;
        let p = Function::from(f).call0(handle).ok()?;
        let v = JsFuture::from(js_sys::Promise::from(p)).await.ok()?;
        v.dyn_into::<File>().ok()
    }

    /// 読む前の `File.size()` と読んだ後の `byte_length()` で 2 回サイズを確かめる。
    // ponytail: 最大 8 MiB を Vec へ一度コピーする（`record` が bytes.len() でサイズを決めるため）。
    // 問題になったら `record` にサイズを別に渡す口を足す。
    async fn read_launched(file: File, name: String) -> Result<PickedFile, DropError> {
        check_size(file.size())?;
        let raw = JsFuture::from(file.array_buffer())
            .await
            .map_err(|_| DropError::Unreadable)?;
        let buf: js_sys::ArrayBuffer = raw.dyn_into().map_err(|_| DropError::Unreadable)?;
        check_size(buf.byte_length() as f64)?;
        Ok(PickedFile {
            name,
            bytes: js_sys::Uint8Array::new(&buf).to_vec(),
        })
    }

    /// 全マウントへ現在のスナップショットを流す。apply が同期で再描画しても
    /// `WATCHERS` を借用していないよう、先に複製する。
    fn notify() {
        let watchers: Vec<Watcher> =
            WATCHERS.with(|w| w.borrow().values().flatten().cloned().collect());
        for a in watchers {
            push_snapshot(&a, false);
        }
    }

    fn push_snapshot(a: &Watcher, hint: bool) {
        let snap = INBOX.snapshot();
        a.apply(move |s| {
            if hint {
                set_web_hint(s);
            }
            apply_snapshot(s, snap.seq, snap.latest.map(latest_file), snap.rejected);
        });
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 回だけ通知先を登録する。
    pub fn file_association_show_effect(
        _state: &FileAssociationShowState,
        applier: &Applier<FileAssociationShowState>,
    ) {
        let id = applier.mount_id();
        if WATCHERS.with(|w| w.borrow().contains_key(&id)) {
            return;
        }
        if install_consumer() {
            WATCHERS.with(|w| w.borrow_mut().insert(id, Some(applier.clone())));
            push_snapshot(applier, true);
        } else {
            // 非対応は 1 回だけ送る。再描画のループを作らない。
            WATCHERS.with(|w| w.borrow_mut().insert(id, None));
            applier.apply(apply_unsupported);
        }
    }

    /// マウント破棄時に通知先を外す。consumer と受け口はページの寿命のあいだ残す。
    pub(crate) fn release_mount(mount_id: u64) {
        WATCHERS.with(|w| w.borrow_mut().remove(&mount_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_only_txt_files() {
        assert!(admit(Some("file"), Some("a.TXT")));
        for (k, n) in [
            (Some("directory"), Some("a.txt")),
            (None, Some("a.txt")),
            (Some("file"), None),
            (Some("file"), Some("a.md")),
            (Some("file"), Some("a/b.txt")),
            (Some("file"), Some(".txt")),
        ] {
            assert!(!admit(k, n), "{k:?} {n:?}");
        }
    }

    #[test]
    fn check_size_boundaries() {
        assert!(check_size(MAX_FILE_BYTES as f64).is_ok());
        assert!(check_size(0.0).is_ok());
        for bad in [MAX_FILE_BYTES as f64 + 1.0, f64::NAN, -1.0] {
            assert_eq!(check_size(bad), Err(DropError::TooLarge));
        }
    }

    #[test]
    fn failure_mapping() {
        assert_eq!(
            failure_kind(OpenFailure::TooLarge),
            OpenFailureKind::TooLarge
        );
        assert_eq!(
            failure_kind(OpenFailure::Unreadable),
            OpenFailureKind::Unreadable
        );
        assert_eq!(failure_kind(OpenFailure::Folder), OpenFailureKind::Folder);
        let ok = OpenedFile {
            name: "a.txt".into(),
            size: 3,
            head: vec![1],
            received_at_unix_ms: 9,
        };
        assert_eq!(latest_file(Ok(ok)), Ok(("a.txt".into(), 3, vec![1], 9)));
        assert_eq!(
            latest_file(Err(OpenFailure::Folder)),
            Err(OpenFailureKind::Folder)
        );
    }
}
