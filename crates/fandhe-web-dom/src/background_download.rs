//! 型サンプル「バックグラウンドダウンロード」の Web 経路（親 #100、Issue #393）。
//!
//! `fandhe_core::screens::background_download`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Background Fetch API
//! （専用 Service Worker `web/background-download-sw.js`）でダウンロードをブラウザへ
//! 依頼する。Chromium だけが対応し、他のブラウザは `Unsupported` を返す。
//!
//! Web は `fandhe-device` の `BackgroundDownload` trait を通らない（Promise ベースの
//! 非同期 API で同期 trait に載らないため。`background_task.rs` と同じ理由）。共有するのは
//! 判定 [`completion_status`]、URL 検証 [`check_url`]、保存名 `FILE_NAME` の 3 つ。
//!
//! # 構成と保存先（Cache Storage、ページと Service Worker の両方が読む）
//!
//! キャッシュ名 `fandhe-background-download` に 3 つの key を置く。
//!
//! - `current`: 今の世代の fetch id（ページが書く。開始・取り消しのたびに変える）
//! - `result`: その世代の結果 `{id, kind, status, bytes, url, reason}`（SW が書く。
//!   取り消しはページが書く）
//! - `FILE_NAME`: 2xx かつ 1 バイト以上の本文（SW が書く）
//!
//! SW の URL・scope は `background_task` と重ならない専用名にし、既存の SW を置き換えない。
//! `register()` の戻り値をそのまま使い、`container.ready()` は使わない
//! （`background_task.rs` の同名の節と同じ理由）。
//!
//! # 世代（契約表 2）
//!
//! 開始・取り消しのたびに `current` の id を新しくする。SW は完了イベントの
//! `registration.id` が `current` と一致するときだけ書き、書いた後でもう一度 `current` を
//! 読んで食い違えば本文を消す。取り消しは先に `current` を差し替えてから本文を消すので、
//! 飛行中の古い完了が後から結果を上書きしても status には現れない（id 不一致で捨てる）。
//! id の連番は一意にするためだけの `Date::now()` で、経過時間は測らないので壁時計でよい。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! - 守る: 1 マウント内の開始・取り消し・完了の順序、古い世代の完了の破棄、失敗・HTTP
//!   エラー・0 バイト・保存ファイル欠落・許可外の最終 URL を成功にしないこと、
//!   `fetch()` が拒否されたとき `current` を残さないこと
//! - 守らない: 複数タブの同時操作、再起動後の完了理由の正確さ、ブラウザ UI から取り消した
//!   ときの表示反映の時間差、SW の完了処理が途中で死んだ場合の「転送中」の張り付き
//!   （`ponytail:` 張り付きを検知する期限は置かない）。実ブラウザでの継続確認は #380
//!
//! # 秘密値・ログ（security.md A09）
//!
//! 画面とログに URL・例外の `message`・`failureReason` の生文字列を出さない。
//! 最終 URL は Cache Storage に置くだけで表示しない。

use fandhe_core::screens::background_download::{DownloadView, UNSUPPORTED};
use fandhe_device::background_download::{completion_status, DownloadStatus};
use fandhe_device::DeviceError;

#[cfg(target_arch = "wasm32")]
use fandhe_device::background_download::FILE_NAME;
use fandhe_device::http::check_url;

/// SW が `result` に書く結果の 1 世代分（`web/background-download-sw.js` と形を合わせる）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResultRecord {
    pub id: String,
    /// `completed` / `failed` / `cancelled`。
    pub kind: String,
    pub http_status: Option<u16>,
    pub bytes: u64,
    pub final_url: String,
    pub reason: String,
}

/// `failureReason`（Background Fetch の失敗理由）を固定文言へ写す（A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_text(reason: &str) -> &'static str {
    match reason {
        "bad-status" => "HTTP エラー",
        "fetch-error" => "接続できません",
        "quota-exceeded" => "保存領域が足りません",
        _ => "転送に失敗しました",
    }
}

/// 開始・取り消しが投げた例外の `name` を [`DeviceError`] へ写す。`message` は読まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn error_from_name(name: Option<&str>) -> DeviceError {
    match name {
        Some("NotAllowedError") => DeviceError::PermissionDenied("background fetch"),
        Some("SecurityError") => DeviceError::Unsupported("background download"),
        _ => DeviceError::Internal("background download"),
    }
}

/// 観測した材料から状態を決める純関数（モジュール doc 参照）。
///
/// - `current_id`: 今の世代の id
/// - `running`: この id の Background Fetch 登録が残っていれば受信済みバイト数
/// - `record`: `result`（id が違えば無視する）
/// - `file_bytes`: キャッシュに保存された本文のバイト数
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn status_from(
    current_id: &str,
    running: Option<u64>,
    record: Option<&ResultRecord>,
    file_bytes: Option<u64>,
) -> DownloadStatus {
    if let Some(r) = record.filter(|r| r.id == current_id) {
        match r.kind.as_str() {
            "completed" => {
                if check_url(&r.final_url).is_err() {
                    return DownloadStatus::Failed("許可されていない転送先です".to_string());
                }
                return match completion_status(r.http_status, r.bytes) {
                    // 嘘の成功を防ぐ: 本文が実在しサイズが合うときだけ完了にする。
                    DownloadStatus::Completed { bytes } if file_bytes != Some(bytes) => {
                        DownloadStatus::Failed("保存したファイルが見つかりません".to_string())
                    }
                    other => other,
                };
            }
            "failed" => return DownloadStatus::Failed(failure_text(&r.reason).to_string()),
            "cancelled" => return DownloadStatus::Cancelled,
            _ => {}
        }
    }
    match running {
        Some(received) => DownloadStatus::Running {
            received,
            total: None,
        },
        None => DownloadStatus::Idle,
    }
}

/// `DeviceError` を画面へ渡す文言へ変換する（`fandhe-egui-proxy` の同名関数と同じ文言）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNSUPPORTED.to_string(),
        other => other.to_string(),
    }
}

/// 状態の読み出し結果を画面の表示値へ写す。非対応は未観測（`Unknown`）、読み出しの失敗は
/// `Idle` と区別する（`fandhe-egui-proxy` の `to_view` と同じ規則）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_view(status: Result<DownloadStatus, DeviceError>) -> DownloadView {
    match status {
        Ok(DownloadStatus::Idle) => DownloadView::Idle,
        Ok(DownloadStatus::Running { received, total }) => {
            DownloadView::Running { received, total }
        }
        Ok(DownloadStatus::Completed { bytes }) => DownloadView::Completed { bytes },
        Ok(DownloadStatus::Cancelled) => DownloadView::Cancelled,
        Ok(DownloadStatus::Failed(m)) => DownloadView::Failed(m),
        Err(DeviceError::Unsupported(_)) => DownloadView::Unknown,
        Err(e) => DownloadView::ReadFailed(to_message(e)),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use fandhe_core::screens::background_download::{
        apply_snapshot, finish_action, mark_in_flight, take_request, Action,
        BackgroundDownloadState, Phase,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{RegistrationOptions, ServiceWorkerRegistration, Window};

    use super::*;
    use crate::mount::Applier;

    /// 取得先。`FANDHE_DOWNLOAD_URL`（ビルド時）で差し替える。既定は同一オリジンの
    /// `items.json`（`build-wasm-dom.sh` が dist へ置く 40 B の fixture）。外部 URL は入れない。
    const DOWNLOAD_URL: &str = match option_env!("FANDHE_DOWNLOAD_URL") {
        Some(u) => u,
        None => "items.json",
    };
    const SW_URL: &str = "background-download-sw.js";
    /// `background_task` の scope と重ならない専用のサブパス。
    const SW_SCOPE: &str = "fandhe-background-download-scope/";
    const CACHE_NAME: &str = "fandhe-background-download";
    const KEY_CURRENT: &str = "fandhe-background-download-current";
    const KEY_RESULT: &str = "fandhe-background-download-result";
    const POLL_INTERVAL_MS: i32 = 500;

    /// 保留中タイマーの `(handle, closure)`。
    type Timer = (i32, Closure<dyn FnMut()>);

    thread_local! {
        /// ポーリングが飛行中のマウント ID（`background_task.rs::POLLING` と同型）。
        static POLLING: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
        /// マウントごとの保留中タイマー（破棄時に `clearTimeout` してから closure を drop する）。
        static TIMERS: RefCell<HashMap<u64, Timer>> =
            RefCell::new(HashMap::new());
    }

    fn internal<E>(_: E) -> DeviceError {
        DeviceError::Internal("background download")
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    fn js_error_name(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    /// `target[name](...args)` を呼び、返り値（Promise でなくてもよい）を待つ。
    /// web-sys に型が無い API（Background Fetch）と Cache Storage をまとめて Reflect で呼ぶ。
    async fn call(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = Reflect::get(target, &JsValue::from_str(name))?
            .dyn_into()
            .map_err(|_| JsValue::from_str("not a function"))?;
        let list = Array::new();
        for a in args {
            list.push(a);
        }
        let v = Reflect::apply(&f, target, &list)?;
        JsFuture::from(Promise::resolve(&v)).await
    }

    fn is_supported(w: &Window) -> bool {
        if !w.is_secure_context() {
            return false;
        }
        let nav: JsValue = w.navigator().into();
        let global: JsValue = js_sys::global().into();
        Reflect::has(&nav, &JsValue::from_str("serviceWorker")).unwrap_or(false)
            && get(&global, "BackgroundFetchManager").is_function()
            && Reflect::has(&global, &JsValue::from_str("caches")).unwrap_or(false)
    }

    async fn open_cache(w: &Window) -> Result<JsValue, JsValue> {
        let caches = get(w.as_ref(), "caches");
        call(&caches, "open", &[JsValue::from_str(CACHE_NAME)]).await
    }

    /// key の本文を文字列で返す（無ければ `None`）。
    async fn cache_text(cache: &JsValue, key: &str) -> Result<Option<String>, JsValue> {
        let r = call(cache, "match", &[JsValue::from_str(key)]).await?;
        if r.is_undefined() {
            return Ok(None);
        }
        Ok(call(&r, "text", &[]).await?.as_string())
    }

    async fn cache_len(cache: &JsValue, key: &str) -> Result<Option<u64>, JsValue> {
        let r = call(cache, "match", &[JsValue::from_str(key)]).await?;
        if r.is_undefined() {
            return Ok(None);
        }
        let blob = call(&r, "blob", &[]).await?;
        Ok(get(&blob, "size").as_f64().map(|f| f as u64))
    }

    async fn cache_put(cache: &JsValue, key: &str, text: &str) -> Result<(), JsValue> {
        let resp = web_sys::Response::new_with_opt_str(Some(text))?;
        call(cache, "put", &[JsValue::from_str(key), resp.into()]).await?;
        Ok(())
    }

    async fn cache_delete(cache: &JsValue, key: &str) -> Result<(), JsValue> {
        call(cache, "delete", &[JsValue::from_str(key)]).await?;
        Ok(())
    }

    fn json(pairs: &[(&str, JsValue)]) -> String {
        let o = Object::new();
        for (k, v) in pairs {
            let _ = Reflect::set(&o, &JsValue::from_str(k), v);
        }
        js_sys::JSON::stringify(&o)
            .ok()
            .and_then(|s| s.as_string())
            .unwrap_or_default()
    }

    /// 壊れた記録は `Err`（成功や未観測へ畳まない、契約表 6）。
    fn parse_record(text: &str) -> Result<ResultRecord, DeviceError> {
        let v = js_sys::JSON::parse(text).map_err(internal)?;
        let s = |k: &str| get(&v, k).as_string();
        Ok(ResultRecord {
            id: s("id").ok_or(internal(()))?,
            kind: s("kind").ok_or(internal(()))?,
            http_status: get(&v, "status").as_f64().map(|f| f as u16),
            bytes: get(&v, "bytes").as_f64().map(|f| f as u64).unwrap_or(0),
            final_url: s("url").unwrap_or_default(),
            reason: s("reason").unwrap_or_default(),
        })
    }

    /// `current` の id。無ければ `None`、壊れていれば `Err`。
    async fn current_id(cache: &JsValue) -> Result<Option<String>, DeviceError> {
        let Some(text) = cache_text(cache, KEY_CURRENT).await.map_err(internal)? else {
            return Ok(None);
        };
        let v = js_sys::JSON::parse(&text).map_err(internal)?;
        get(&v, "id").as_string().map(Some).ok_or(internal(()))
    }

    fn new_id() -> String {
        format!("fandhe-background-download-{}", js_sys::Date::now() as u64)
    }

    async fn write_current(cache: &JsValue, id: &str) -> Result<(), JsValue> {
        cache_put(cache, KEY_CURRENT, &json(&[("id", JsValue::from_str(id))])).await
    }

    /// 専用 scope の登録を返す（無ければ `None`）。ポーリングで登録を作らないために使う。
    async fn find_registration(w: &Window) -> Result<Option<JsValue>, JsValue> {
        let container: JsValue = w.navigator().service_worker().into();
        let r = call(
            &container,
            "getRegistration",
            &[JsValue::from_str(SW_SCOPE)],
        )
        .await?;
        Ok((!r.is_undefined()).then_some(r))
    }

    /// `id` の Background Fetch 登録（残っていれば）。
    async fn find_fetch(reg: &JsValue, id: &str) -> Result<Option<JsValue>, JsValue> {
        let bf = get(reg, "backgroundFetch");
        let r = call(&bf, "get", &[JsValue::from_str(id)]).await?;
        Ok((!r.is_undefined()).then_some(r))
    }

    fn absolute_url(w: &Window) -> Result<String, DeviceError> {
        let base = w
            .document()
            .and_then(|d| d.base_uri().ok().flatten())
            .ok_or(internal(()))?;
        web_sys::Url::new_with_base(DOWNLOAD_URL, &base)
            .map(|u| u.href())
            .map_err(internal)
    }

    async fn do_start(w: Window) -> Result<(), DeviceError> {
        if !is_supported(&w) {
            return Err(DeviceError::Unsupported("background download"));
        }
        // 取得先は実行時にも検証する（ビルド時の差し替え値を含む、A02・A03）。
        let abs = absolute_url(&w)?;
        check_url(&abs)?;

        let cache = open_cache(&w).await.map_err(internal)?;
        let container = w.navigator().service_worker();
        let options = RegistrationOptions::new();
        options.set_scope(SW_SCOPE);
        let reg: ServiceWorkerRegistration =
            JsFuture::from(container.register_with_options(SW_URL, &options))
                .await
                .map_err(|e| error_from_name(js_error_name(&e).as_deref()))?
                .unchecked_into();
        crate::background_task::wait_until_active(&w, &reg).await?;
        let reg: JsValue = reg.into();

        // 進行中なら何もしない（二重開始を作らない）。
        if let Some(id) = current_id(&cache).await? {
            if let Some(f) = find_fetch(&reg, &id).await.map_err(internal)? {
                if get(&f, "result").as_string().as_deref() == Some("") {
                    return Ok(());
                }
            }
        }

        let id = new_id();
        cache_delete(&cache, FILE_NAME).await.map_err(internal)?;
        cache_delete(&cache, KEY_RESULT).await.map_err(internal)?;
        write_current(&cache, &id).await.map_err(internal)?;

        let opts = Object::new();
        let _ = Reflect::set(&opts, &"title".into(), &"ダウンロード".into());
        let bf = get(&reg, "backgroundFetch");
        let requests = Array::of1(&JsValue::from_str(&abs));
        let args = [JsValue::from_str(&id), requests.into(), opts.into()];
        if let Err(e) = call(&bf, "fetch", &args).await {
            // 失敗した開始の世代を残さない（契約表 9）。
            let _ = cache_delete(&cache, KEY_CURRENT).await;
            return Err(error_from_name(js_error_name(&e).as_deref()));
        }
        Ok(())
    }

    async fn do_cancel(w: Window) -> Result<(), DeviceError> {
        if !is_supported(&w) {
            return Err(DeviceError::Unsupported("background download"));
        }
        let cache = open_cache(&w).await.map_err(internal)?;
        let old = current_id(&cache).await?;
        // 先に世代を差し替える。飛行中の古い完了は id 不一致で捨てられる。
        let id = new_id();
        write_current(&cache, &id).await.map_err(internal)?;
        if let (Some(old), Some(reg)) = (old, find_registration(&w).await.map_err(internal)?) {
            if let Some(f) = find_fetch(&reg, &old).await.map_err(internal)? {
                call(&f, "abort", &[]).await.map_err(internal)?;
            }
        }
        cache_delete(&cache, FILE_NAME).await.map_err(internal)?;
        let rec = json(&[
            ("id", JsValue::from_str(&id)),
            ("kind", JsValue::from_str("cancelled")),
        ]);
        cache_put(&cache, KEY_RESULT, &rec).await.map_err(internal)
    }

    async fn do_status(w: &Window) -> Result<DownloadStatus, DeviceError> {
        if !is_supported(w) {
            return Err(DeviceError::Unsupported("background download"));
        }
        let cache = open_cache(w).await.map_err(internal)?;
        let Some(id) = current_id(&cache).await? else {
            return Ok(DownloadStatus::Idle);
        };
        let record = match cache_text(&cache, KEY_RESULT).await.map_err(internal)? {
            Some(t) => Some(parse_record(&t)?),
            None => None,
        };
        let file_bytes = cache_len(&cache, FILE_NAME).await.map_err(internal)?;
        let mut running = None;
        if record.as_ref().is_none_or(|r| r.id != id) {
            if let Some(reg) = find_registration(w).await.map_err(internal)? {
                if let Some(f) = find_fetch(&reg, &id).await.map_err(internal)? {
                    // result が確定済みでも SW の完了処理が書き終えるまでは転送中に見せる。
                    running = Some(get(&f, "downloaded").as_f64().unwrap_or(0.0) as u64);
                }
            }
        }
        Ok(status_from(&id, running, record.as_ref(), file_bytes))
    }

    /// 保留中のタイマーを 1 本に保つ（古いものは止めてから closure を drop する）。
    fn schedule_next_poll(applier: &Applier<BackgroundDownloadState>) {
        let id = applier.mount_id();
        let Some(w) = web_sys::window() else { return };
        if !crate::mount::is_live(id) {
            return;
        }
        let a = applier.clone();
        let cb = Closure::wrap(Box::new(move || a.apply(|_| {})) as Box<dyn FnMut()>);
        let handle = w
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                POLL_INTERVAL_MS,
            )
            .unwrap_or(0);
        let old = TIMERS.with(|t| t.borrow_mut().insert(id, (handle, cb)));
        if let Some((h, _)) = old {
            w.clear_timeout_with_handle(h);
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）にタイマーと飛行中記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        POLLING.with(|p| {
            p.borrow_mut().remove(&mount_id);
        });
        if let Some((h, _cb)) = TIMERS.with(|t| t.borrow_mut().remove(&mount_id)) {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(h);
            }
        }
    }

    /// `mount_with_effect` へ渡す effect（要求の処理とポーリング、`background_task_effect` と同型）。
    pub fn background_download_effect(
        state: &BackgroundDownloadState,
        applier: &Applier<BackgroundDownloadState>,
    ) {
        let mount_id = applier.mount_id();

        if let Some(request) = take_request(state) {
            applier.apply(mark_in_flight);
            let (generation, action) = (request.generation, request.action);
            let applier = applier.clone();
            let Some(w) = web_sys::window() else {
                applier.apply(move |s| {
                    finish_action(s, generation, Err(to_message(DeviceError::Unsupported(""))))
                });
                return;
            };
            wasm_bindgen_futures::spawn_local(async move {
                let result = match action {
                    Action::Start => do_start(w.clone()).await,
                    Action::Cancel => do_cancel(w.clone()).await,
                };
                // 完了の確定と直後の状態を 1 回の apply で反映する（古い view が 1 フレーム
                // 出て二重開始できる窓を作らない）。
                let view = to_view(do_status(&w).await);
                applier.apply(move |s| {
                    finish_action(s, generation, result.map_err(to_message));
                    apply_snapshot(s, view);
                });
            });
            return;
        }

        if state.phase != Phase::Idle {
            return;
        }
        if !POLLING.with(|p| p.borrow_mut().insert(mount_id)) {
            return;
        }
        let Some(w) = web_sys::window() else {
            POLLING.with(|p| {
                p.borrow_mut().remove(&mount_id);
            });
            return;
        };
        let prev = state.view.clone();
        let started_generation = state.generation;
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let status = do_status(&w).await;
            POLLING.with(|p| {
                p.borrow_mut().remove(&mount_id);
            });
            // 転送中と読み出し失敗は、送信を省く回でも次回を予約する（契約表 4）。
            let keep_polling = matches!(
                status,
                Ok(DownloadStatus::Running { .. })
                    | Err(DeviceError::Internal(_))
                    | Err(DeviceError::Http(_))
            );
            let view = to_view(status);
            if view != prev {
                applier.apply(move |s| {
                    if s.generation == started_generation && s.phase == Phase::Idle {
                        apply_snapshot(s, view);
                    }
                });
            }
            if keep_polling {
                schedule_next_poll(&applier);
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::background_download_effect;
/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: &str) -> ResultRecord {
        ResultRecord {
            id: "a".into(),
            kind: kind.into(),
            http_status: Some(200),
            bytes: 40,
            final_url: "http://127.0.0.1:8787/items.json".into(),
            reason: String::new(),
        }
    }

    #[test]
    fn running_without_record() {
        assert_eq!(
            status_from("a", Some(10), None, None),
            DownloadStatus::Running {
                received: 10,
                total: None
            }
        );
    }

    #[test]
    fn stale_record_is_ignored() {
        let mut r = rec("completed");
        r.id = "old".into();
        assert_eq!(
            status_from("a", None, Some(&r), Some(40)),
            DownloadStatus::Idle
        );
        assert!(matches!(
            status_from("a", Some(1), Some(&r), Some(40)),
            DownloadStatus::Running { .. }
        ));
    }

    #[test]
    fn nothing_observed_is_idle() {
        assert_eq!(status_from("a", None, None, None), DownloadStatus::Idle);
    }

    #[test]
    fn completed_requires_status_size_file_and_url() {
        let ok = rec("completed");
        assert_eq!(
            status_from("a", None, Some(&ok), Some(40)),
            DownloadStatus::Completed { bytes: 40 }
        );
        assert!(matches!(
            status_from("a", None, Some(&ok), None),
            DownloadStatus::Failed(_)
        ));
        assert!(matches!(
            status_from("a", None, Some(&ok), Some(3)),
            DownloadStatus::Failed(_)
        ));
        let mut r = rec("completed");
        r.http_status = Some(404);
        assert_eq!(
            status_from("a", None, Some(&r), None),
            DownloadStatus::Failed("HTTP 404".into())
        );
        let mut r = rec("completed");
        r.bytes = 0;
        assert!(matches!(
            status_from("a", None, Some(&r), Some(0)),
            DownloadStatus::Failed(_)
        ));
        let mut r = rec("completed");
        r.final_url = "http://example.com/x".into();
        assert_eq!(
            status_from("a", None, Some(&r), Some(40)),
            DownloadStatus::Failed("許可されていない転送先です".into())
        );
    }

    #[test]
    fn failed_and_cancelled() {
        let mut r = rec("failed");
        r.reason = "fetch-error".into();
        assert_eq!(
            status_from("a", None, Some(&r), None),
            DownloadStatus::Failed("接続できません".into())
        );
        assert_eq!(
            status_from("a", None, Some(&rec("cancelled")), None),
            DownloadStatus::Cancelled
        );
    }

    #[test]
    fn failure_text_maps_unknown_to_generic() {
        assert_eq!(failure_text("bad-status"), "HTTP エラー");
        assert_eq!(failure_text("quota-exceeded"), "保存領域が足りません");
        assert_eq!(
            failure_text("download-total-exceeded"),
            "転送に失敗しました"
        );
        assert_eq!(failure_text("???"), "転送に失敗しました");
    }

    #[test]
    fn error_names_map() {
        assert!(matches!(
            error_from_name(Some("NotAllowedError")),
            DeviceError::PermissionDenied(_)
        ));
        assert!(matches!(
            error_from_name(Some("SecurityError")),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            error_from_name(Some("TypeError")),
            DeviceError::Internal(_)
        ));
        assert!(matches!(error_from_name(None), DeviceError::Internal(_)));
    }

    #[test]
    fn view_mapping_separates_unsupported_and_read_failure() {
        assert_eq!(
            to_view(Err(DeviceError::Unsupported("x"))),
            DownloadView::Unknown
        );
        assert!(matches!(
            to_view(Err(DeviceError::Internal("x"))),
            DownloadView::ReadFailed(_)
        ));
        assert_eq!(to_view(Ok(DownloadStatus::Idle)), DownloadView::Idle);
        assert_eq!(
            to_view(Ok(DownloadStatus::Cancelled)),
            DownloadView::Cancelled
        );
    }

    #[test]
    fn default_download_url_is_allowed_on_loopback() {
        assert!(check_url("http://127.0.0.1:8787/items.json").is_ok());
        assert!(check_url("http://example.com/items.json").is_err());
    }
}
