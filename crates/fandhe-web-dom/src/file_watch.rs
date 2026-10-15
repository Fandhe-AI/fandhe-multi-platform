//! 型サンプル「フォルダの変更監視」の Web 経路（親 #1143、Issue #1414）。
//!
//! `fandhe_core::screens::file_watch_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_start` / `take_op` で要求を
//! 観測し、Chrome 133+ の `FileSystemObserver` と OPFS（`navigator.storage.getDirectory()`、
//! 許可ダイアログなし）の専用フォルダ `file-watch` へ渡して、結果を `finish_start` / `apply_scan` /
//! `mark_failed` / `finish_op` で書き戻す。`fandhe-device` の `FileWatch` trait は通らず、
//! 共有するのは走査結果の整形・差分・サンプル名の規則（`fandhe_device::file_watch` の純関数）だけ。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。任意フォルダの監視は #1408。
//!
//! # 方式（ネイティブ 4 OS と同じ）
//!
//! オブザーバーの通知は「合図」で、`type` 以外（`changedHandle` 等）は読まない。合図ごとに
//! `dir.entries()` で作業フォルダ直下を再走査し、前回との差分を記録にする。合図が走査中に
//! 届いたら `dirty` を立て、走査が終わったらもう 1 周する（捨てない）。
//!
//! # Reflect を使う理由
//!
//! web-sys に `FileSystemObserver` が無く、OPFS の `createWritable` 等も不安定 API のため
//! `js_sys::Reflect` で呼ぶ（`screen_wake_lock` と同方針。`web-sys` の feature は足さない）。
//!
//! # 結果の三値
//!
//! - 利用者の停止・破棄 = 正常（`disconnect` して `Closure` を drop。失敗文言を出さない）
//! - `errored`・走査失敗（フォルダ消失等） = 失敗（`mark_failed`。「監視が止まりました」）
//! - `unknown`・あふれ・未知の種別 = `Changed`（再走査が吸収する。失敗にしない）
//! - 非対応（secure context 外・`FileSystemObserver` 無し・`getDirectory` 無し） = `Unsupported`、
//!   OPFS を開けない（プライベートブラウズ等）・`observe` の失敗 = 開始の失敗。0 件の成功へ畳まない
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の成功の禁止（`truncated` / `dropped` の表示を含む）、1 マウント内の
//! 「合図 → 再走査 → 差分」の順序と世代照合（await 再開後は生存と世代を再確認）、
//! 停止・破棄・開始失敗で `disconnect` と `Closure` を解放、`sample-N.txt` 以外を作らない・消さない。
//! 守らない: Chromium デスクトップ以外（非対応と表示）、複数タブ・複数マウントが同じ OPFS フォルダを
//! 書く場合の表示の食い違い（書き込み操作自体は Web Locks で共有フォルダ単位に排他し、追記は失わない。Web Locks が無ければ操作は失敗）、走査と走査の間の「作成して即削除」、名前変更の区別（削除 + 作成で出す）、
//! 合図から反映までのタスク順による遅れ、64 件超、`mount` が `Screen<S>` を作り直さないための
//! トグルボタンのラベル固定（`demo.rs` に停止ボタンを足して回避）。
//!
//! 例外の `message`・ファイル名・内容は画面にも console にも出さない（security.md A09）。
//!
//! # 既知の天井
//!
//! ponytail: 走査は全エントリの `getFile()` を逐次待つ。数千件置かれると遅い。アプリ専用領域なので
//! 許容し、問題になれば `getFile` を上位 64 件の名前に絞る。

use std::time::{Duration, SystemTime};

use fandhe_core::screens::file_watch_show::RecordKind;
use fandhe_device::file_watch::{
    diff, folder_scan_from, sample_number, ChangeKind, FileEntry, FolderScan, WatchSignal,
};

/// 開始失敗の固定文言（`fandhe-egui-proxy` のドライバと同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const START_FAILED: &str = "監視の開始";
/// 操作失敗の固定文言（同上）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const OP_FAILED: &str = "ファイルの操作";

/// 一覧取りで得た 1 エントリの生値（JS の値を検証前のまま持つ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawEntry {
    pub name: String,
    pub is_file: bool,
    pub size: f64,
    pub last_modified: f64,
}

/// 一覧の生値を走査結果へ写す純関数。dot 始まり・ファイル以外・不正な `size` は捨てる。
/// 更新時刻は等値比較にだけ使うので、不正値は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn scan_from_listing(entries: Vec<RawEntry>) -> FolderScan {
    let files = entries
        .into_iter()
        .filter(|e| e.is_file && !e.name.is_empty() && !e.name.starts_with('.'))
        .filter(|e| e.size.is_finite() && e.size >= 0.0 && e.size.fract() == 0.0)
        .map(|e| FileEntry {
            name: e.name,
            size: e.size as u64,
            modified: (e.last_modified.is_finite() && e.last_modified >= 0.0)
                .then(|| {
                    SystemTime::UNIX_EPOCH
                        .checked_add(Duration::from_millis(e.last_modified as u64))
                })
                .flatten(),
        })
        .collect();
    folder_scan_from(files)
}

/// オブザーバーの `records[].type` から合図を決める。`errored` だけが失敗で、
/// `unknown`・空・未知の種別は `Changed`（再走査で確かめる）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn signal_from_types(types: &[String]) -> WatchSignal {
    if types.iter().any(|t| t == "errored") {
        WatchSignal::Failed
    } else {
        WatchSignal::Changed
    }
}

/// 前回と今回の走査の差分を画面の記録へ写す。表示名は整形し、空になれば `?`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn records_from(prev: &FolderScan, next: &FolderScan) -> Vec<(RecordKind, String)> {
    diff(prev, next)
        .into_iter()
        .map(|c| {
            let kind = match c.kind {
                ChangeKind::Created => RecordKind::Created,
                ChangeKind::Modified => RecordKind::Modified,
                ChangeKind::Deleted => RecordKind::Deleted,
            };
            let name =
                fandhe_device::device_info::clean_text(&c.name).unwrap_or_else(|| "?".to_string());
            (kind, name)
        })
        .collect()
}

/// 一覧全体（64 件の窓に依らない）から `sample-N.txt` の最大番号を返す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn latest_sample(entries: &[RawEntry]) -> Option<u64> {
    entries
        .iter()
        .filter(|e| e.is_file)
        .filter_map(|e| sample_number(&e.name))
        .max()
}

#[cfg(target_arch = "wasm32")]
pub use web::file_watch_show_effect;

/// マウント破棄時にオブザーバーと発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use fandhe_core::screens::file_watch_show::{
        apply_scan, finish_op, finish_pick, finish_start, mark_failed, take_op, take_pick,
        take_start, FileWatchShowState, OpKind, OpRequest, StartInfo, WatchFailure, WatchPhase,
        WEB_FOLDER_LABEL,
    };
    use fandhe_device::file_watch::{
        next_sample_body, next_sample_number, sample_name, FolderScan, WatchSignal,
        MAX_SAMPLE_BYTES, SAMPLE_INITIAL,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{
        latest_sample, records_from, scan_from_listing, signal_from_types, RawEntry, OP_FAILED,
        START_FAILED,
    };
    use crate::mount::{is_live, Applier};

    type State = FileWatchShowState;
    type ObserverCb = Closure<dyn FnMut(JsValue, JsValue)>;

    /// 作業フォルダ名（OPFS 直下）。
    const DIR_NAME: &str = "file-watch";

    /// マウントごとの監視 1 件。`observer` を `disconnect` してから drop する。
    struct Watch {
        generation: u64,
        observer: JsValue,
        _callback: ObserverCb,
        dir: JsValue,
        /// 基準の走査が済むまで `None`（その間の合図は `dirty` に溜める）。
        prev: Option<FolderScan>,
        scanning: bool,
        dirty: bool,
    }

    thread_local! {
        /// 発送済みの開始世代（同じ世代を二重に始めない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        /// 発送済みの操作世代。
        static OP_DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        static WATCHES: RefCell<HashMap<u64, Watch>> = RefCell::new(HashMap::new());
        /// effect が最後に見た状態（開始世代, Idle 以外か）。登録直前に「今も現在の開始か」を照合する。
        static START_STATE: RefCell<HashMap<u64, (u64, bool)>> = RefCell::new(HashMap::new());
        /// 実行中のファイル操作があるマウント。停止・再開始をまたいでも操作を直列化する。
        static OP_BUSY: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
        /// 実行中の操作の後ろで待つ未開始の操作（最新 1 件。停止で取り消す）。
        static OP_PENDING: RefCell<HashMap<u64, OpRequest>> = RefCell::new(HashMap::new());
    }

    /// この開始要求がまだ現在（停止・差し替えされていない）か。
    fn start_current(mount: u64, generation: u64) -> bool {
        START_STATE.with(|s| s.borrow().get(&mount) == Some(&(generation, true)))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn call(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        let a = Array::new();
        for v in args {
            a.push(v);
        }
        f.apply(obj, &a)
    }

    /// `obj.method(...args)` を呼び、返った Promise を待つ。
    async fn call_await(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let p: Promise = call(obj, method, args)?.dyn_into()?;
        JsFuture::from(p).await
    }

    fn opts(pairs: &[(&str, bool)]) -> JsValue {
        let o = Object::new();
        for (k, v) in pairs {
            let _ = Reflect::set(&o, &JsValue::from_str(k), &JsValue::from_bool(*v));
        }
        o.into()
    }

    fn error_name(e: &JsValue) -> String {
        get(e, "name").as_string().unwrap_or_default()
    }

    /// 機能検出。secure context・`FileSystemObserver`・`navigator.storage.getDirectory` が
    /// 揃うときだけ `(コンストラクタ, storage)`。ブラウザ名では分岐しない。
    fn detect() -> Option<(Function, JsValue)> {
        let window = web_sys::window()?;
        if get(&window, "isSecureContext").as_bool() != Some(true) {
            return None;
        }
        let ctor: Function = get(&window, "FileSystemObserver").dyn_into().ok()?;
        let storage = get(&window.navigator(), "storage");
        get(&storage, "getDirectory")
            .is_function()
            .then_some((ctor, storage))
    }

    /// 切り離した `Watch` を止める。`disconnect` を先に呼んでから `Closure` を drop する。
    fn teardown(w: Watch) {
        let _ = call(&w.observer, "disconnect", &[]);
        drop(w);
    }

    fn with_watch<R>(mount: u64, generation: u64, f: impl FnOnce(&mut Watch) -> R) -> Option<R> {
        WATCHES.with(|w| {
            w.borrow_mut()
                .get_mut(&mount)
                .filter(|x| x.generation == generation)
                .map(f)
        })
    }

    fn still_mine(mount: u64, generation: u64) -> bool {
        is_live(mount) && with_watch(mount, generation, |_| ()).is_some()
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount));
        OP_DISPATCHED.with(|d| d.borrow_mut().remove(&mount));
        START_STATE.with(|d| d.borrow_mut().remove(&mount));
        OP_PENDING.with(|d| d.borrow_mut().remove(&mount));
        if let Some(w) = WATCHES.with(|w| w.borrow_mut().remove(&mount)) {
            teardown(w);
        }
    }

    /// 作業フォルダ直下の一覧を生値で取る。エントリ単位の失敗（一覧と `getFile` の間に消えた等）は
    /// そのエントリだけ捨て、一覧自体の失敗は `Err`。
    async fn list_raw(dir: &JsValue) -> Result<Vec<RawEntry>, ()> {
        let it = call(dir, "entries", &[]).map_err(|_| ())?;
        let mut out = Vec::new();
        loop {
            let r = call_await(&it, "next", &[]).await.map_err(|_| ())?;
            if get(&r, "done").as_bool().unwrap_or(true) {
                break;
            }
            let pair = get(&r, "value");
            let name = Reflect::get_u32(&pair, 0).ok().and_then(|n| n.as_string());
            let handle = Reflect::get_u32(&pair, 1).unwrap_or(JsValue::UNDEFINED);
            let Some(name) = name else { continue };
            if get(&handle, "kind").as_string().as_deref() != Some("file") {
                continue;
            }
            let Ok(file) = call_await(&handle, "getFile", &[]).await else {
                continue;
            };
            out.push(RawEntry {
                name,
                is_file: true,
                size: get(&file, "size").as_f64().unwrap_or(f64::NAN),
                last_modified: get(&file, "lastModified").as_f64().unwrap_or(f64::NAN),
            });
        }
        Ok(out)
    }

    /// 合図ごとの再走査。走査中の合図は `dirty` で受け、終わったらもう 1 周する。
    fn kick_scan(applier: Applier<State>, mount: u64, generation: u64) {
        let start = with_watch(mount, generation, |w| {
            if w.prev.is_none() || w.scanning {
                w.dirty = true;
                false
            } else {
                w.scanning = true;
                true
            }
        })
        .unwrap_or(false);
        if start {
            spawn_local(scan_loop(applier, mount, generation));
        }
    }

    async fn scan_loop(applier: Applier<State>, mount: u64, generation: u64) {
        loop {
            let Some(dir) = with_watch(mount, generation, |w| {
                w.dirty = false;
                w.dir.clone()
            }) else {
                return;
            };
            let listed = list_raw(&dir).await;
            // await 再開後の再確認（契約表 2）。破棄・差し替え済みなら捨てる。
            if !still_mine(mount, generation) {
                return;
            }
            let Ok(raw) = listed else {
                with_watch(mount, generation, |w| w.scanning = false);
                applier.apply(move |s| mark_failed(s, generation));
                return;
            };
            let next = scan_from_listing(raw);
            let Some((changes, count, truncated, again)) = with_watch(mount, generation, |w| {
                let changes = w
                    .prev
                    .as_ref()
                    .map(|p| records_from(p, &next))
                    .unwrap_or_default();
                let (count, truncated) = (next.files.len(), next.truncated);
                w.prev = Some(next);
                let again = w.dirty;
                if !again {
                    w.scanning = false;
                }
                (changes, count, truncated, again)
            }) else {
                return;
            };
            applier.apply(move |s| apply_scan(s, generation, changes, count, truncated));
            if !again {
                return;
            }
        }
    }

    enum StartError {
        /// 世代・生存が変わった。後始末は照合側が済ませている。
        Stale,
        Failed,
    }

    async fn open_and_observe(
        applier: &Applier<State>,
        mount: u64,
        generation: u64,
        ctor: Function,
        storage: JsValue,
    ) -> Result<StartInfo, StartError> {
        let fail = |_| StartError::Failed;
        let root = call_await(&storage, "getDirectory", &[])
            .await
            .map_err(fail)?;
        let dir = call_await(
            &root,
            "getDirectoryHandle",
            &[JsValue::from_str(DIR_NAME), opts(&[("create", true)])],
        )
        .await
        .map_err(fail)?;
        // 停止・差し替え済みの開始は登録しない（古い開始が新しい Watch を置換するのを防ぐ）。
        if !is_live(mount) || !start_current(mount, generation) {
            return Err(StartError::Stale);
        }
        let cb_applier = applier.clone();
        let callback: ObserverCb = Closure::new(move |records: JsValue, _observer: JsValue| {
            let types: Vec<String> = records
                .dyn_into::<Array>()
                .map(|a| {
                    a.iter()
                        .filter_map(|r| get(&r, "type").as_string())
                        .collect()
                })
                .unwrap_or_default();
            let signal = signal_from_types(&types);
            let applier = cb_applier.clone();
            // 同期 apply は再描画 → 照合 → この Closure の drop に至りうるため、必ず逃がす。
            spawn_local(async move {
                match signal {
                    WatchSignal::Failed => applier.apply(move |s| mark_failed(s, generation)),
                    WatchSignal::Changed => kick_scan(applier, mount, generation),
                }
            });
        });
        let args = Array::new();
        args.push(callback.as_ref());
        let observer = Reflect::construct(&ctor, &args).map_err(fail)?;
        // observe の前に登録する（登録から保存までの合図を落とさない。契約表 1）。
        let replaced = WATCHES.with(|w| {
            w.borrow_mut().insert(
                mount,
                Watch {
                    generation,
                    observer: observer.clone(),
                    _callback: callback,
                    dir: dir.clone(),
                    prev: None,
                    scanning: false,
                    dirty: false,
                },
            )
        });
        // 念のため、既存の Watch を置換したときは必ず disconnect してから drop する。
        if let Some(old) = replaced {
            teardown(old);
        }
        // 以降の失敗・中断は、登録済みの Watch を呼び出し側が外して teardown する。
        call_await(
            &observer,
            "observe",
            &[dir.clone(), opts(&[("recursive", false)])],
        )
        .await
        .map_err(fail)?;
        if !still_mine(mount, generation) || !start_current(mount, generation) {
            return Err(StartError::Stale);
        }
        let raw = list_raw(&dir).await.map_err(|()| StartError::Failed)?;
        if !still_mine(mount, generation) {
            return Err(StartError::Stale);
        }
        let scan = scan_from_listing(raw);
        let info = StartInfo {
            folder: WEB_FOLDER_LABEL.to_string(),
            count: scan.files.len(),
            truncated: scan.truncated,
        };
        let dirty = with_watch(mount, generation, |w| {
            w.prev = Some(scan);
            w.dirty
        })
        .ok_or(StartError::Stale)?;
        if dirty {
            // 準備中に届いた合図は、開始完了の反映後に再走査する。
            let a = applier.clone();
            spawn_local(async move { kick_scan(a, mount, generation) });
        }
        Ok(info)
    }

    async fn start_watch(
        applier: Applier<State>,
        mount: u64,
        generation: u64,
        ctor: Function,
        storage: JsValue,
    ) {
        let result = match open_and_observe(&applier, mount, generation, ctor, storage).await {
            Ok(info) => Ok(info),
            Err(StartError::Stale) => return,
            Err(StartError::Failed) => {
                // 確保済みのオブザーバーを外す（契約表 9）。
                let taken = WATCHES.with(|w| {
                    let mut w = w.borrow_mut();
                    match w.get(&mount) {
                        Some(x) if x.generation == generation => w.remove(&mount),
                        _ => None,
                    }
                });
                if let Some(w) = taken {
                    teardown(w);
                }
                Err(WatchFailure::Failed(START_FAILED.to_string()))
            }
        };
        applier.apply(move |s| finish_start(s, generation, result));
    }

    /// 例外が `NotFoundError` なら対象無し（`Ok(false)`）、それ以外は失敗。
    fn missing_or_failed(e: &JsValue) -> Result<bool, ()> {
        if error_name(e) == "NotFoundError" {
            Ok(false)
        } else {
            Err(())
        }
    }

    /// `createWritable` で全置換する。`write` / `close` が失敗したら `abort` して swap を残さない。
    async fn write_file(handle: &JsValue, body: &[u8]) -> Result<(), ()> {
        let w = call_await(handle, "createWritable", &[])
            .await
            .map_err(|_| ())?;
        let r = async {
            call_await(&w, "write", &[Uint8Array::from(body).into()]).await?;
            call_await(&w, "close", &[]).await
        }
        .await;
        if r.is_err() {
            let _ = call_await(&w, "abort", &[]).await;
            return Err(());
        }
        Ok(())
    }

    /// 共有フォルダ単位（Web Locks `file-watch`）で `run_op` を排他する。マウント間・タブ間で
    /// 「一覧 → 番号選択」「読み取り → 全置換」が交差して追記が消えるのを防ぐ。
    /// Web Locks が無い環境は失敗にする（排他なしで書いて嘘の成功にしない）。
    async fn run_op_locked(dir: JsValue, kind: OpKind) -> Result<bool, ()> {
        let window = web_sys::window().ok_or(())?;
        let locks = get(&window.navigator(), "locks");
        if !get(&locks, "request").is_function() {
            return Err(());
        }
        let cb = Closure::once_into_js(move || {
            wasm_bindgen_futures::future_to_promise(async move {
                run_op(&dir, kind)
                    .await
                    .map(JsValue::from_bool)
                    .map_err(|()| JsValue::NULL)
            })
        });
        let p = call(&locks, "request", &[JsValue::from_str(DIR_NAME), cb]).map_err(|_| ())?;
        let p: Promise = p.dyn_into().map_err(|_| ())?;
        JsFuture::from(p).await.map_err(|_| ())?.as_bool().ok_or(())
    }

    /// 確認用の操作。名前は一覧から得た `sample-N.txt` か `sample_name` で作ったものだけ（A03）。
    async fn run_op(dir: &JsValue, kind: OpKind) -> Result<bool, ()> {
        let raw = list_raw(dir).await?;
        let latest = latest_sample(&raw);
        match kind {
            OpKind::Create => {
                let n = next_sample_number(latest).ok_or(())?;
                let h = call_await(
                    dir,
                    "getFileHandle",
                    &[
                        JsValue::from_str(&sample_name(n)),
                        opts(&[("create", true)]),
                    ],
                )
                .await
                .map_err(|_| ())?;
                write_file(&h, SAMPLE_INITIAL.as_bytes()).await?;
                Ok(true)
            }
            OpKind::Update => {
                let Some(n) = latest else { return Ok(false) };
                let h =
                    match call_await(dir, "getFileHandle", &[JsValue::from_str(&sample_name(n))])
                        .await
                    {
                        Ok(h) => h,
                        Err(e) => return missing_or_failed(&e),
                    };
                let file = call_await(&h, "getFile", &[]).await.map_err(|_| ())?;
                let size = get(&file, "size").as_f64().ok_or(())?;
                let current = if size <= MAX_SAMPLE_BYTES as f64 {
                    let buf = call_await(&file, "arrayBuffer", &[])
                        .await
                        .map_err(|_| ())?;
                    Some(Uint8Array::new(&buf).to_vec())
                } else {
                    None
                };
                write_file(&h, &next_sample_body(current.as_deref())).await?;
                Ok(true)
            }
            OpKind::Delete => {
                let Some(n) = latest else { return Ok(false) };
                match call_await(dir, "removeEntry", &[JsValue::from_str(&sample_name(n))]).await {
                    Ok(_) => Ok(true),
                    Err(e) => missing_or_failed(&e),
                }
            }
        }
    }

    /// 画面の effect。照合 → 開始 → 操作の順に、要求を 1 回ずつ発送する。
    pub fn file_watch_show_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        // 0. フォルダ選択（Issue #37）。Web は未対応と答えて「選択中」のまま残さない。
        //    世代照合は finish_pick が持つため、二重に送っても後続は捨てられる。
        //    `showDirectoryPicker` での実装は後続。
        if let Some(generation) = take_pick(state) {
            applier.apply(move |s| finish_pick(s, generation, Err(WatchFailure::Unsupported)));
        }
        START_STATE.with(|s| {
            s.borrow_mut().insert(
                mount,
                (state.watch_generation, state.phase != WatchPhase::Idle),
            )
        });

        // 1. 照合。停止・失敗・古い世代を 1 か所で吸収する。
        let stale = WATCHES.with(|w| {
            let mut w = w.borrow_mut();
            let drop_it = w.get(&mount).is_some_and(|x| {
                state.phase == WatchPhase::Idle || state.watch_generation != x.generation
            });
            if drop_it {
                w.remove(&mount)
            } else {
                None
            }
        });
        if let Some(w) = stale {
            teardown(w);
        }

        // 2. 開始。
        if let Some(generation) = take_start(state) {
            let fresh =
                DISPATCHED.with(|d| d.borrow_mut().insert(mount, generation) != Some(generation));
            if fresh {
                match detect() {
                    // 非対応は同期区間で確定する（effect 実行中の apply は退避される）。
                    None => applier.apply(move |s| {
                        finish_start(s, generation, Err(WatchFailure::Unsupported))
                    }),
                    Some((ctor, storage)) => spawn_local(start_watch(
                        applier.clone(),
                        mount,
                        generation,
                        ctor,
                        storage,
                    )),
                }
            }
        }

        // 3. 操作。変化そのものは合図 → 再走査の経路で記録に出る。
        let Some(op) = take_op(state) else {
            // 停止などで取り消された未開始の操作は捨てる。
            OP_PENDING.with(|p| p.borrow_mut().remove(&mount));
            return;
        };
        let fresh = OP_DISPATCHED
            .with(|d| d.borrow_mut().insert(mount, op.generation) != Some(op.generation));
        if !fresh {
            return;
        }
        if OP_BUSY.with(|b| b.borrow().contains(&mount)) {
            // 前の操作（停止前のものを含む）の完了を待つ。
            OP_PENDING.with(|p| p.borrow_mut().insert(mount, op));
            return;
        }
        OP_BUSY.with(|b| b.borrow_mut().insert(mount));
        let applier = applier.clone();
        spawn_local(async move {
            let mut op = op;
            loop {
                let dir = WATCHES.with(|w| w.borrow().get(&mount).map(|x| x.dir.clone()));
                let result = match dir {
                    Some(d) => run_op_locked(d, op.kind).await,
                    None => Err(()),
                }
                .map_err(|()| WatchFailure::Failed(OP_FAILED.to_string()));
                OP_BUSY.with(|b| b.borrow_mut().remove(&mount));
                let generation = op.generation;
                applier.apply(move |s| finish_op(s, generation, result));
                let Some(next) = OP_PENDING.with(|p| p.borrow_mut().remove(&mount)) else {
                    return;
                };
                if OP_BUSY.with(|b| !b.borrow_mut().insert(mount)) {
                    // apply 中の effect が別の操作を始めた。直列性を保つため待機へ戻す。
                    OP_PENDING.with(|p| p.borrow_mut().insert(mount, next));
                    return;
                }
                op = next;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, is_file: bool, size: f64, lm: f64) -> RawEntry {
        RawEntry {
            name: name.into(),
            is_file,
            size,
            last_modified: lm,
        }
    }

    #[test]
    fn listing_drops_dot_dirs_and_bad_sizes() {
        let s = scan_from_listing(vec![
            raw("b.txt", true, 2.0, 5.0),
            raw(".hidden", true, 1.0, 1.0),
            raw("dir", false, 0.0, 1.0),
            raw("neg", true, -1.0, 1.0),
            raw("nan", true, f64::NAN, 1.0),
            raw("frac", true, 1.5, 1.0),
            raw("a.txt", true, 0.0, f64::NAN),
        ]);
        let names: Vec<_> = s.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        assert_eq!(s.files[0].modified, None);
        assert!(s.files[1].modified.is_some());
    }

    #[test]
    fn listing_truncates_deterministically_and_empty_is_ok() {
        let all: Vec<_> = (0..70)
            .rev()
            .map(|i| raw(&format!("f{i:03}"), true, 1.0, 1.0))
            .collect();
        let s = scan_from_listing(all);
        assert_eq!(s.files.len(), 64);
        assert!(s.truncated);
        assert_eq!(s.files[0].name, "f000");
        let e = scan_from_listing(vec![]);
        assert!(e.files.is_empty() && !e.truncated);
    }

    #[test]
    fn only_errored_is_failure() {
        let t =
            |v: &[&str]| signal_from_types(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(t(&["appeared", "errored"]), WatchSignal::Failed);
        for ok in [
            "appeared",
            "disappeared",
            "modified",
            "moved",
            "unknown",
            "x",
        ] {
            assert_eq!(t(&[ok]), WatchSignal::Changed);
        }
        assert_eq!(t(&[]), WatchSignal::Changed);
    }

    #[test]
    fn records_cover_create_modify_delete_and_sanitize() {
        let p = scan_from_listing(vec![
            raw("a", true, 1.0, 1.0),
            raw("b", true, 1.0, 1.0),
            raw("c", true, 1.0, 1.0),
        ]);
        let n = scan_from_listing(vec![
            raw("a", true, 2.0, 1.0),
            raw("b", true, 1.0, 9.0),
            raw("d\u{0}\n", true, 1.0, 1.0),
        ]);
        assert_eq!(
            records_from(&p, &n),
            [
                (RecordKind::Modified, "a".to_string()),
                (RecordKind::Modified, "b".to_string()),
                (RecordKind::Deleted, "c".to_string()),
                (RecordKind::Created, "d".to_string()),
            ]
        );
        let ctrl = scan_from_listing(vec![raw("\u{0}", true, 1.0, 1.0)]);
        assert_eq!(
            records_from(&FolderScan::default(), &ctrl),
            [(RecordKind::Created, "?".to_string())]
        );
    }

    #[test]
    fn latest_sample_ignores_non_matching() {
        let e = vec![
            raw("sample-7.txt", true, 1.0, 1.0),
            raw("sample-007.txt", true, 1.0, 1.0),
            raw("sample-x.txt", true, 1.0, 1.0),
            raw("sample-1234567.txt", true, 1.0, 1.0),
            raw("sample-9.txt", false, 0.0, 1.0),
        ];
        assert_eq!(latest_sample(&e), Some(7));
        assert_eq!(latest_sample(&[]), None);
    }
}
