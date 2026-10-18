//! 型サンプル「外部機器への再生」の Web 経路（親 #99、Issue #389）。
//!
//! `fandhe_core::screens::cast_show`（端末依存ゼロ）の状態機械を [`super::mount::mount_with_effect`] の
//! effect として直接駆動する。`take_command` で要求を観測し、Remote Playback API
//! （`HTMLMediaElement.remote`）へ渡して、`apply_update` / `expire` で結果を書き戻す。
//! `MediaCast` trait は `Send + Sync` で `JsValue` を持てないため通らない（他の Web モジュールと同じ）。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外（従来どおり `Unsupported`）。
//!
//! # 方式
//!
//! - 音源は `fandhe_device::media_playback::demo_wav` の WAV を `blob:` URL にして `<audio>` へ渡し、ループ再生する。
//!   `<audio>` は mount root の再構築で消えないよう `document.body` 直下へ不可視で置く
//! - `remote` は web-sys に束縛が無いため `Reflect` + `Function` で呼ぶ。Cast Web Sender SDK と
//!   Presentation API は使わない。非対応（`remote` / `prompt` が無い。Firefox 等）は `Unsupported`
//! - `play()` と `prompt()` は利用者のクリックと同じタスク（effect の同期区間）で呼ぶ
//!   （`await` 後だと transient activation を失う）。準備は `loadedmetadata` を待たない
//!   （iOS Safari は操作が無いと読み込まず期限切れになるため）
//! - 出力先の名前は仕様上ページに渡されない。接続は常に `Connected(None)`
//!
//! # 結果の種類
//!
//! - `prompt()` の `NotAllowedError` = 利用者が閉じた（キャンセル。失敗にせず実際の `remote.state` から経路を戻す）、
//!   `NotFoundError` = 出力先なし、`NotSupportedError` = この音源は送れない
//! - `play()` の `NotAllowedError` = `Blocked`、`AbortError` = キャンセル（失敗にしない）
//!
//! # 期限とライフサイクル
//!
//! 準備・再生・停止は `START_DEADLINE`、`connecting` は `CONNECT_DEADLINE`。`Choosing` に期限は付けない。
//! 時間は `performance.now()`（単調時計）で測り、期限後に届いた `connect` は成功にしない。
//! `pagehide` で鳴っていれば中断し、復帰しても自動で再開しない（`visibilitychange` は使わない。
//! タブを切り替えてもキャストが続くのが正しい動作）。失敗・期限切れ・マウント破棄・準備のやり直しでは
//! タイマー → リスナー → `disableRemotePlayback`（接続を UA に切断させる）→ 要素 → `blob:` URL の順に解放する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の操作順と世代の照合、非対応・拒否・期限切れを接続中や再生中と見せないこと、
//! 資源の解放、`pagehide` 後に勝手に再開しないこと。例外の `message` は転送しない（security.md A09）。
//! 守らない: 複数タブ・複数マウント、受信機側で再開した再生の反映（画面は「停止しました」のまま）、
//! 受信機の品質や遅延、出力先の名前、egui-proxy の wasm canvas 経路、
//! `prompt()` が選択を待たず解決するブラウザでの `Choosing` 表示の長さ。

use fandhe_core::screens::cast_show::{CastShowFailure, CastShowRoute};
use std::time::Duration;

/// 失敗の固定文言（`fandhe-egui-proxy` のドライバと同じ。例外の `message` は使わない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_CAST: &str = "外部機器への再生";

/// `remote.state` → 経路。未知の値を `Connected` にしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn route_from_state(state: &str) -> CastShowRoute {
    match state {
        "connecting" => CastShowRoute::Connecting,
        "connected" => CastShowRoute::Connected(None),
        _ => CastShowRoute::Local,
    }
}

/// `prompt()` の結果の分類。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PromptOutcome {
    /// 利用者が閉じた。失敗にしない。
    Cancel,
    /// 出力先が見つからない。
    NoDevices,
    /// 失敗。
    Failed(CastShowFailure),
}

/// `prompt()` の拒否（例外の `name`）を分類する。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn prompt_outcome(name: Option<&str>) -> PromptOutcome {
    match name {
        Some("NotAllowedError") => PromptOutcome::Cancel,
        Some("NotFoundError") => PromptOutcome::NoDevices,
        Some("NotSupportedError") => PromptOutcome::Failed(CastShowFailure::SourceNotSupported),
        _ => PromptOutcome::Failed(CastShowFailure::Failed(FAILED_CAST.to_string())),
    }
}

/// `play()` の拒否 → 失敗。`None` は AbortError（キャンセル扱いで失敗にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn play_failure(name: Option<&str>) -> Option<CastShowFailure> {
    match name {
        Some("NotAllowedError") => Some(CastShowFailure::Blocked),
        Some("AbortError") => None,
        _ => Some(CastShowFailure::Failed(FAILED_CAST.to_string())),
    }
}

/// 起点から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64, deadline: Duration) -> bool {
    now_ms - started_ms >= deadline.as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
pub use web::cast_show_effect;

/// マウント破棄時に要素・URL・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::cast_show::{
        apply_update, expire, mark_command_taken, prepare, take_command, CastCommand,
        CastShowFailure, CastShowPlayback, CastShowRoute, CastShowState, CastShowUpdate,
    };
    use fandhe_device::media_cast::{CONNECT_DEADLINE, START_DEADLINE};
    use fandhe_device::media_playback::demo_wav;
    use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{Blob, BlobPropertyBag, EventTarget, HtmlMediaElement, Url};

    use super::{
        is_late, play_failure, prompt_outcome, route_from_state, PromptOutcome, FAILED_CAST,
    };
    use crate::mount::{is_live, Applier};

    type State = CastShowState;
    type Timer = (i32, Closure<dyn FnMut()>);
    type Listener = (EventTarget, &'static str, Closure<dyn FnMut()>);

    const AUDIO_STYLE: &str =
        "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none";

    /// マウントごとの再生 1 件。`teardown` で解除してから drop する。
    struct Session {
        el: HtmlMediaElement,
        remote: JsValue,
        url: String,
        /// 準備要求の世代。セッションの識別子として不変（ピッカー結果の照合に使う）。
        id: u64,
        /// effect が最後に見た画面の世代。
        gen: u64,
        /// 再生開始の確定待ち（世代, 起点 `performance.now()`）。
        pending: Option<(u64, f64)>,
        start_timer: Option<Timer>,
        connecting_since: Option<f64>,
        connect_timer: Option<Timer>,
        listeners: Vec<Listener>,
    }

    #[derive(Default)]
    struct Mount {
        /// 処理済みの要求の世代（同じ要求を二重に起動しない）。
        dispatched: u64,
        session: Option<Session>,
    }

    thread_local! {
        static MOUNTS: RefCell<HashMap<u64, Mount>> = RefCell::new(HashMap::new());
    }

    /// 既存の Session にだけ作用する。破棄済みマウントでは `None`。
    fn with<R>(mount: u64, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        MOUNTS.with(|m| {
            m.borrow_mut()
                .get_mut(&mount)
                .and_then(|x| x.session.as_mut())
                .map(f)
        })
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    /// 例外の `name` だけを読む（`message` は使わない）。
    fn err_name(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    fn remote_state(remote: &JsValue) -> String {
        get(remote, "state").as_string().unwrap_or_default()
    }

    fn fixed_failure() -> CastShowFailure {
        CastShowFailure::Failed(FAILED_CAST.to_string())
    }

    fn clear_timer(t: Option<Timer>) {
        if let (Some((id, _cb)), Some(w)) = (t, web_sys::window()) {
            w.clear_timeout_with_handle(id);
        }
    }

    fn set_timer(ms: u128, f: impl FnMut() + 'static) -> Option<Timer> {
        let cb: Closure<dyn FnMut()> = Closure::new(f);
        let id = web_sys::window()?
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                ms as i32,
            )
            .ok()?;
        Some((id, cb))
    }

    /// 通知を非同期に書き戻す（イベント・タイマーのコールバックから同期 apply しない）。
    fn post(applier: &Applier<State>, f: impl FnOnce(&mut State) + 'static) {
        let a = applier.clone();
        spawn_local(async move { a.apply(f) });
    }

    fn post_update(applier: &Applier<State>, g: u64, u: CastShowUpdate) {
        post(applier, move |s| {
            apply_update(s, g, u);
        });
    }

    /// 経路を現在の世代で書き戻す。
    fn post_route(applier: &Applier<State>, mount: u64, route: CastShowRoute) {
        if let Some(g) = with(mount, |s| s.gen) {
            post_update(applier, g, CastShowUpdate::Route(route));
        }
    }

    /// 保持フラグではなく実際の `remote.state` を読んで経路を戻す（契約表 1）。
    fn post_current_route(applier: &Applier<State>, mount: u64) {
        if let Some((g, r)) = with(mount, |s| {
            (s.gen, route_from_state(&remote_state(&s.remote)))
        }) {
            post_update(applier, g, CastShowUpdate::Route(r));
        }
    }

    /// 現在の世代で期限切れにする（core は遷移中・`Connecting` のときだけ受ける）。
    fn post_expire(applier: &Applier<State>) {
        post(applier, |s| {
            let g = s.generation;
            expire(s, g);
        });
    }

    /// 解除を済ませてから drop する（契約表 9）。
    fn teardown(mut s: Session) {
        clear_timer(s.start_timer.take());
        clear_timer(s.connect_timer.take());
        for (target, name, cb) in s.listeners.drain(..) {
            let _ = target.remove_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
        }
        // 接続中なら UA に切断させる。
        let _ = Reflect::set(
            &s.el,
            &JsValue::from_str("disableRemotePlayback"),
            &JsValue::TRUE,
        );
        let _ = s.el.pause();
        let _ = s.el.remove_attribute("src");
        s.el.remove();
        if !s.url.is_empty() {
            let _ = Url::revoke_object_url(&s.url);
        }
    }

    fn drop_session(mount: u64) {
        let old = MOUNTS.with(|m| {
            m.borrow_mut()
                .get_mut(&mount)
                .and_then(|x| x.session.take())
        });
        if let Some(s) = old {
            teardown(s);
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        if let Some(m) = MOUNTS.with(|m| m.borrow_mut().remove(&mount)) {
            if let Some(s) = m.session {
                teardown(s);
            }
        }
    }

    fn listen(
        listeners: &mut Vec<Listener>,
        target: EventTarget,
        name: &'static str,
        f: impl FnMut() + 'static,
    ) {
        let cb: Closure<dyn FnMut()> = Closure::new(f);
        if target
            .add_event_listener_with_callback(name, cb.as_ref().unchecked_ref())
            .is_ok()
        {
            listeners.push((target, name, cb));
        }
    }

    /// 要素・`blob:` URL・リスナーを作る。失敗したら作りかけを解放して失敗を返す。
    fn build(applier: &Applier<State>, mount: u64, gen: u64) -> Result<Session, CastShowFailure> {
        let window = web_sys::window().ok_or(CastShowFailure::Unsupported)?;
        let document = window.document().ok_or(CastShowFailure::Unsupported)?;
        let el: HtmlMediaElement = document
            .create_element("audio")
            .map_err(|_| fixed_failure())?
            .dyn_into()
            .map_err(|_| fixed_failure())?;
        let remote = get(&el, "remote");
        if !remote.is_object() || !get(&remote, "prompt").is_function() {
            return Err(CastShowFailure::Unsupported);
        }
        let body = document.body().ok_or_else(fixed_failure)?;
        el.set_loop(true);
        let _ = el.set_attribute("aria-hidden", "true");
        let _ = el.set_attribute("style", AUDIO_STYLE);

        // リスナーを先に付け、保存までの間に届く通知を落とさない（契約表 1）。
        let mut listeners = Vec::new();
        let et: EventTarget = el.clone().into();
        let a = applier.clone();
        listen(&mut listeners, et.clone(), "pause", move || {
            // 受信機・OS の UI から止められたときに「再生中」を出し続けない。重複は core が捨てる。
            // 開始の確定待ち中の pause は通常の停止にしない（期限切れ・play() の結果が確定させる）。
            if let Some(g) = with(mount, |s| s.pending.is_none().then_some(s.gen)).flatten() {
                post_update(&a, g, CastShowUpdate::Playback(CastShowPlayback::Stopped));
            }
        });
        let a = applier.clone();
        listen(&mut listeners, et, "error", move || {
            if let Some(g) = with(mount, |s| s.gen) {
                post_update(&a, g, CastShowUpdate::Failed(fixed_failure()));
            }
        });
        let rt: Option<EventTarget> = remote.clone().dyn_into().ok();
        if let Some(rt) = rt {
            let a = applier.clone();
            listen(&mut listeners, rt.clone(), "connecting", move || {
                let a2 = a.clone();
                let timer = set_timer(CONNECT_DEADLINE.as_millis(), move || {
                    let a3 = a2.clone();
                    spawn_local(async move { connect_expired(&a3, mount) });
                });
                let old = with(mount, |s| {
                    s.connecting_since = Some(now());
                    std::mem::replace(&mut s.connect_timer, timer)
                });
                clear_timer(old.flatten());
                post_route(&a, mount, CastShowRoute::Connecting);
            });
            let a = applier.clone();
            listen(&mut listeners, rt.clone(), "connect", move || {
                let (late, old) = with(mount, |s| {
                    let late = s
                        .connecting_since
                        .take()
                        .is_some_and(|t| is_late(t, now(), CONNECT_DEADLINE));
                    (late, s.connect_timer.take())
                })
                .unwrap_or((false, None));
                clear_timer(old);
                if late {
                    // 期限後の接続は成功にしない（契約表 3）。
                    post_expire(&a);
                } else {
                    post_route(&a, mount, CastShowRoute::Connected(None));
                }
            });
            let a = applier.clone();
            listen(&mut listeners, rt, "disconnect", move || {
                let old = with(mount, |s| {
                    s.connecting_since = None;
                    s.connect_timer.take()
                });
                clear_timer(old.flatten());
                post_route(&a, mount, CastShowRoute::Disconnected);
            });
        }
        let a = applier.clone();
        listen(&mut listeners, window.into(), "pagehide", move || {
            interrupt(&a, mount)
        });

        let mut s = Session {
            el,
            remote,
            url: String::new(),
            id: gen,
            gen,
            pending: None,
            start_timer: None,
            connecting_since: None,
            connect_timer: None,
            listeners,
        };
        let url = (|| {
            let bytes = Uint8Array::from(&demo_wav()[..]);
            let opts = BlobPropertyBag::new();
            opts.set_type("audio/wav");
            let blob =
                Blob::new_with_u8_array_sequence_and_options(&Array::of1(&bytes), &opts).ok()?;
            Url::create_object_url_with_blob(&blob).ok()
        })();
        match url {
            Some(u) if body.append_child(&s.el).is_ok() => {
                s.el.set_src(&u);
                s.url = u;
                Ok(s)
            }
            other => {
                if let Some(u) = other {
                    let _ = Url::revoke_object_url(&u);
                }
                teardown(s);
                Err(fixed_failure())
            }
        }
    }

    /// 接続期限。まだ `connecting` のときだけ失敗にする（core は経路が `Connecting` のときだけ受ける）。
    fn connect_expired(applier: &Applier<State>, mount: u64) {
        let hit = with(mount, |s| {
            s.connect_timer = None;
            remote_state(&s.remote) == "connecting"
        });
        if hit == Some(true) {
            post_expire(applier);
        }
    }

    /// 鳴っている・確定待ちのときだけ中断する。復帰しても自動で再開しない。
    fn interrupt(applier: &Applier<State>, mount: u64) {
        let hit = with(mount, |s| {
            if s.el.paused() && s.pending.is_none() {
                return None;
            }
            s.pending = None;
            clear_timer(s.start_timer.take());
            let _ = s.el.pause();
            Some(s.gen)
        })
        .flatten();
        if let Some(g) = hit {
            post_update(
                applier,
                g,
                CastShowUpdate::Playback(CastShowPlayback::Interrupted),
            );
        }
    }

    fn do_prepare(applier: &Applier<State>, mount: u64, g: u64) {
        drop_session(mount);
        match build(applier, mount, g) {
            Ok(s) => {
                let route = route_from_state(&remote_state(&s.remote));
                MOUNTS.with(|m| {
                    if let Some(x) = m.borrow_mut().get_mut(&mount) {
                        x.session = Some(s);
                    }
                });
                post(applier, move |st| {
                    apply_update(st, g, CastShowUpdate::Playback(CastShowPlayback::Ready));
                    apply_update(st, g, CastShowUpdate::Route(route));
                });
            }
            Err(f) => post_update(applier, g, CastShowUpdate::Failed(f)),
        }
    }

    /// 確定待ちが世代 `g` のままなら期限切れとして確定する。`pause()` が保留中の `play()` を
    /// AbortError で終わらせるため、遅れて鳴り出さない。
    fn expire_now(applier: &Applier<State>, mount: u64, g: u64) {
        let hit = with(mount, |s| {
            if s.pending.map(|p| p.0) != Some(g) {
                return false;
            }
            s.pending = None;
            clear_timer(s.start_timer.take());
            let _ = s.el.pause();
            s.el.set_current_time(0.0);
            true
        });
        if hit == Some(true) {
            post(applier, move |s| expire(s, g));
        }
    }

    fn do_play(applier: &Applier<State>, mount: u64, g: u64) {
        let started = now();
        let a = applier.clone();
        let el = with(mount, |s| {
            s.gen = g;
            s.pending = Some((g, started));
            let t = set_timer(START_DEADLINE.as_millis(), move || {
                let a2 = a.clone();
                spawn_local(async move { expire_now(&a2, mount, g) });
            });
            clear_timer(std::mem::replace(&mut s.start_timer, t));
            s.el.clone()
        });
        let Some(el) = el else {
            return post_update(applier, g, CastShowUpdate::Failed(fixed_failure()));
        };
        // effect の同期区間で呼ぶ（クリックと同じタスク。ユーザー操作の扱いが保たれる）。
        let promise = match el.play() {
            Ok(p) => p,
            Err(e) => return reject_play(applier, mount, g, &e),
        };
        let a = applier.clone();
        spawn_local(async move {
            let r = JsFuture::from(promise).await;
            let t = now();
            // await 再開後に、まだ現在の要求かを確かめる（契約表 2）。
            let mine = is_live(mount) && with(mount, |s| s.pending.map(|p| p.0)) == Some(Some(g));
            if !mine {
                return;
            }
            match r {
                Err(e) => reject_play(&a, mount, g, &e),
                Ok(_) if is_late(started, t, START_DEADLINE) => expire_now(&a, mount, g),
                Ok(_) => {
                    let route = with(mount, |s| {
                        s.pending = None;
                        clear_timer(s.start_timer.take());
                        route_from_state(&remote_state(&s.remote))
                    });
                    if let Some(route) = route {
                        a.apply(move |st| {
                            apply_update(
                                st,
                                g,
                                CastShowUpdate::Playback(CastShowPlayback::Playing),
                            );
                            apply_update(st, g, CastShowUpdate::Route(route));
                        });
                    }
                }
            }
        });
    }

    fn reject_play(applier: &Applier<State>, mount: u64, g: u64, e: &JsValue) {
        let mine = with(mount, |s| {
            let mine = s.pending.map(|p| p.0) == Some(g);
            if mine {
                s.pending = None;
                clear_timer(s.start_timer.take());
            }
            mine
        }) == Some(true);
        match play_failure(err_name(e).as_deref()) {
            Some(f) => post_update(applier, g, CastShowUpdate::Failed(f)),
            // AbortError: 開始中に利用者・OS が止めた。失敗にせず停止として確定する。
            None if mine => post_update(
                applier,
                g,
                CastShowUpdate::Playback(CastShowPlayback::Stopped),
            ),
            None => {}
        }
    }

    fn do_stop(applier: &Applier<State>, mount: u64, g: u64) {
        with(mount, |s| {
            s.pending = None;
            clear_timer(s.start_timer.take());
            let _ = s.el.pause();
        });
        post_update(
            applier,
            g,
            CastShowUpdate::Playback(CastShowPlayback::Stopped),
        );
    }

    fn do_show_picker(applier: &Applier<State>, mount: u64, g: u64) {
        let Some((remote, sid)) = with(mount, |s| (s.remote.clone(), s.id)) else {
            return post_update(applier, g, CastShowUpdate::Failed(fixed_failure()));
        };
        // await の後では transient activation を失うため、ここ（同期）で呼ぶ。
        let ret = get(&remote, "prompt")
            .dyn_into::<Function>()
            .and_then(|f| f.call0(&remote));
        // Choosing を先に積む（同期失敗の結果も後続の post が受けられるように）。
        post_update(applier, g, CastShowUpdate::Route(CastShowRoute::Choosing));
        let promise = match ret {
            Ok(p) => p.dyn_into::<Promise>().ok(),
            Err(e) => return after_prompt(applier, mount, sid, Err(e)),
        };
        let Some(p) = promise else {
            return post_current_route(applier, mount);
        };
        let a = applier.clone();
        spawn_local(async move {
            let r = JsFuture::from(p).await.map(|_| ());
            if is_live(mount) {
                after_prompt(&a, mount, sid, r);
            }
        });
    }

    /// 要求が現存するセッションの結果を反映してよい経路か。ピッカー保留中に再生が先に成功すると
    /// 経路が `Choosing` から `Local` へ戻るため、`Local` でも拒否（出力先なし・非対応音源）を捨てない。
    /// 接続系の経路（`Connecting` / `Connected` 等）では別の確定が優先なので触らない。
    fn picker_open(r: &CastShowRoute) -> bool {
        matches!(r, CastShowRoute::Choosing | CastShowRoute::Local)
    }

    /// 選択結果を現在の状態へ確定させる。再生操作で世代が進んでいても、`Choosing` のままなら回収する。
    /// 停止・中断中は core が `Failed` を受けないため、ここで直接失敗へ移して `Choosing` を残さない。
    /// 要求時のセッション `sid` が現存しなければ結果を捨てる（再準備後の新セッションを触らない）。
    fn after_prompt(applier: &Applier<State>, mount: u64, sid: u64, r: Result<(), JsValue>) {
        if with(mount, |s| s.id) != Some(sid) {
            return;
        }
        let outcome = match r {
            Ok(()) => return post_current_route(applier, mount),
            Err(e) => prompt_outcome(err_name(&e).as_deref()),
        };
        match outcome {
            PromptOutcome::Cancel => post_current_route(applier, mount),
            PromptOutcome::NoDevices => post(applier, |s| {
                if picker_open(&s.route) {
                    s.route = CastShowRoute::NoDevices;
                }
            }),
            PromptOutcome::Failed(f) => post(applier, move |s| {
                if !picker_open(&s.route) {
                    return;
                }
                let g = s.generation;
                apply_update(s, g, CastShowUpdate::Failed(f.clone()));
                if !matches!(s.playback, CastShowPlayback::Failed(_)) {
                    s.playback = CastShowPlayback::Failed(f);
                    s.route = CastShowRoute::Local;
                }
            }),
        }
    }

    /// マウントごとに 1 度だけ Mount を作り、画面を開いたときの自動準備を要求する。再生は利用者の操作に限る。
    fn install(applier: &Applier<State>, mount: u64) {
        let fresh = MOUNTS.with(|m| match m.borrow_mut().entry(mount) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Mount::default());
                true
            }
            _ => false,
        });
        if fresh {
            applier.apply(prepare);
        }
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn cast_show_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        install(applier, mount);
        // 失敗・期限切れが確定したら資源を全部解放する（全経路をここ 1 か所で拾う）。
        if matches!(state.playback, CastShowPlayback::Failed(_)) {
            drop_session(mount);
        }
        with(mount, |s| {
            s.gen = state.generation;
            if s.pending.is_some_and(|p| p.0 != state.generation) {
                s.pending = None;
                clear_timer(s.start_timer.take());
            }
        });
        let Some((cmd, g)) = take_command(state) else {
            return;
        };
        let fresh = MOUNTS.with(|m| {
            m.borrow_mut().get_mut(&mount).is_some_and(|x| {
                let f = x.dispatched != g;
                x.dispatched = g;
                f
            })
        });
        if !fresh {
            return;
        }
        applier.apply(move |s| mark_command_taken(s, g));
        match cmd {
            CastCommand::Prepare => do_prepare(applier, mount, g),
            CastCommand::Play => do_play(applier, mount, g),
            CastCommand::Stop => do_stop(applier, mount, g),
            CastCommand::ShowPicker => do_show_picker(applier, mount, g),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::media_cast::{CONNECT_DEADLINE, START_DEADLINE};

    #[test]
    fn route_never_invents_connected() {
        assert_eq!(route_from_state("connecting"), CastShowRoute::Connecting);
        assert_eq!(
            route_from_state("connected"),
            CastShowRoute::Connected(None)
        );
        assert_eq!(route_from_state("disconnected"), CastShowRoute::Local);
        assert_eq!(route_from_state("???"), CastShowRoute::Local);
    }

    #[test]
    fn prompt_cancel_is_not_failure() {
        assert_eq!(
            prompt_outcome(Some("NotAllowedError")),
            PromptOutcome::Cancel
        );
        assert_eq!(
            prompt_outcome(Some("NotFoundError")),
            PromptOutcome::NoDevices
        );
        assert_eq!(
            prompt_outcome(Some("NotSupportedError")),
            PromptOutcome::Failed(CastShowFailure::SourceNotSupported)
        );
        let fixed = PromptOutcome::Failed(CastShowFailure::Failed(FAILED_CAST.to_string()));
        assert_eq!(prompt_outcome(Some("Other")), fixed);
        assert_eq!(prompt_outcome(None), fixed);
    }

    #[test]
    fn play_failure_mapping() {
        assert_eq!(
            play_failure(Some("NotAllowedError")),
            Some(CastShowFailure::Blocked)
        );
        assert_eq!(play_failure(Some("AbortError")), None);
        assert_eq!(
            play_failure(Some("X")),
            Some(CastShowFailure::Failed(FAILED_CAST.to_string()))
        );
    }

    #[test]
    fn late_boundary() {
        for d in [START_DEADLINE, CONNECT_DEADLINE] {
            let ms = d.as_secs_f64() * 1000.0;
            assert!(!is_late(100.0, 100.0 + ms - 1.0, d));
            assert!(is_late(100.0, 100.0 + ms, d));
        }
    }
}
