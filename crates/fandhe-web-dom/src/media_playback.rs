//! 型サンプル「音声の再生」の Web 経路（親 #86、Issue #271）。
//!
//! `fandhe_core::screens::audio_play`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`take_command` で要求を観測し、DOM に差し込まない `<audio>`
//! （`HTMLMediaElement`）へ `fandhe_device::media_playback::demo_wav` の WAV を `blob:` URL で渡して鳴らし、
//! 結果を `apply_update` で書き戻す。`MediaPlayback` trait（`Send + Sync`）は `JsValue` を持てないため通らない。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外（従来どおり `Unsupported`）。
//!
//! # 結果の種類
//!
//! - `play()` の `NotAllowedError` = 自動再生ブロック（`Blocked`、失敗）。`NotSupportedError` = デコード不可。
//!   `AbortError` = 確定前の一時停止・停止・中断（キャンセル。失敗にしない）
//! - `ended` = 最後まで再生（成功）、`error` = 失敗。両者を取り違えない
//! - `play()` はクリックと同じタスク（effect の同期区間）で呼び、ユーザー操作の扱いを保つ
//!
//! # 期限と中断
//!
//! 期限の起点は effect での `play()` 呼び出しで、`performance.now()`（単調時計）で測る。期限切れ・
//! 確定が期限超過のときは `pause()` して先頭へ戻し、`expire` で失敗を確定する（`pause()` が保留中の
//! `play()` を AbortError で終わらせるため、遅れて鳴り始めない）。`pagehide` / hidden な
//! `visibilitychange` は、要素の実際の `paused` と保留の有無で判断して中断し、復帰しても自動では再開しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の操作順と世代照合、失敗・ブロック・期限切れを成功へ畳まないこと、object URL・
//! リスナー・タイマーの解放、中断後に勝手に再開しないこと。
//! 守らない: 位置表示のポーリング間隔ぶんの遅れ、複数タブ・複数マウント間の一致、他タブの音声との混ざり、
//! egui-proxy の wasm canvas 経路。OS のメディア操作への連携は `now_playing` が担う。例外の `message` は画面にも console にも出さない（security.md A09）。

use fandhe_core::screens::audio_play::{AudioFailure, AudioPhase, AudioUpdate};
use fandhe_device::media_playback::{secs_to_ms, START_DEADLINE};

/// 再生失敗の固定文言（`fandhe-egui-proxy` のドライバと同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_PLAY: &str = "音声の再生";
/// デコード失敗の固定文言（`fandhe-egui-proxy` のドライバと同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_DECODE: &str = "音声を読み込めません";

/// `play()` の拒否（例外の `name`）→ 失敗。`None` は AbortError（キャンセル扱いで失敗にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_play_error(name: Option<&str>) -> Option<AudioFailure> {
    match name {
        Some("NotAllowedError") => Some(AudioFailure::Blocked),
        Some("AbortError") => None,
        Some("NotSupportedError") => Some(AudioFailure::Failed(FAILED_DECODE.to_string())),
        _ => Some(AudioFailure::Failed(FAILED_PLAY.to_string())),
    }
}

/// 要素の位置・長さ（秒）から通知を作る。NaN・無限大・負値は未取得（丸める前に検証）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn snapshot_update(
    phase: AudioPhase,
    current_secs: f64,
    duration_secs: f64,
) -> AudioUpdate {
    AudioUpdate::Snapshot {
        phase,
        position_ms: secs_to_ms(current_secs),
        duration_ms: secs_to_ms(duration_secs),
    }
}

/// 開始から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    now_ms - started_ms >= START_DEADLINE.as_secs_f64() * 1000.0
}

/// 中断すべきか。保持フラグではなく要素の実際の `paused` と保留の有無で判断する（二重通知で二重に動かない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn should_interrupt(paused: bool, pending: bool) -> bool {
    !paused || pending
}

#[cfg(target_arch = "wasm32")]
pub use web::audio_play_effect;

/// マウント破棄時に要素・URL・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::audio_play::{
        apply_update, expire, mark_command_taken, take_command, AudioCommand, AudioFailure,
        AudioPhase, AudioPlayState, AudioUpdate,
    };
    use fandhe_device::media_playback::{demo_wav, POLL_INTERVAL, START_DEADLINE};
    use js_sys::{Array, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{Blob, BlobPropertyBag, HtmlMediaElement, Url};

    use super::{
        failure_from_play_error, is_late, should_interrupt, snapshot_update, FAILED_DECODE,
        FAILED_PLAY,
    };
    use crate::mount::{is_live, Applier};

    type State = AudioPlayState;
    type Cb = Closure<dyn FnMut(JsValue)>;
    type Timer = (i32, Closure<dyn FnMut()>);

    /// マウントごとの再生 1 件。`teardown` で解除してから drop する。
    struct Session {
        el: HtmlMediaElement,
        url: String,
        /// effect が最後に見た画面の世代。
        gen: u64,
        /// 処理済みの要求の世代（同じ要求を二重に始めない）。
        dispatched: u64,
        /// 開始確定待ち（世代, 起点 `performance.now()`）。
        pending: Option<(u64, f64)>,
        timer: Option<Timer>,
        poll: Option<Timer>,
        on_ended: Cb,
        on_error: Cb,
        on_pagehide: Cb,
        on_visibility: Cb,
    }

    thread_local! {
        static SESSIONS: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    /// 例外の `name` だけを読む（`message` は使わない）。
    fn err_name(e: &JsValue) -> Option<String> {
        Reflect::get(e, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
    }

    fn errored(el: &HtmlMediaElement) -> bool {
        Reflect::get(el, &JsValue::from_str("error"))
            .is_ok_and(|v| !v.is_null() && !v.is_undefined())
    }

    fn clear_timer(t: Option<Timer>, interval: bool) {
        if let Some((id, _cb)) = t {
            if let Some(w) = web_sys::window() {
                if interval {
                    w.clear_interval_with_handle(id);
                } else {
                    w.clear_timeout_with_handle(id);
                }
            }
        }
    }

    fn func(c: &Cb) -> js_sys::Function {
        c.as_ref().unchecked_ref::<js_sys::Function>().clone()
    }

    /// 解除を済ませてから Closure を drop する（契約表 9）。
    fn teardown(mut s: Session) {
        clear_timer(s.timer.take(), false);
        clear_timer(s.poll.take(), true);
        let _ = s.el.pause();
        let _ =
            s.el.remove_event_listener_with_callback("ended", &func(&s.on_ended));
        let _ =
            s.el.remove_event_listener_with_callback("error", &func(&s.on_error));
        if let Some(w) = web_sys::window() {
            let _ = w.remove_event_listener_with_callback("pagehide", &func(&s.on_pagehide));
            if let Some(d) = w.document() {
                let _ = d.remove_event_listener_with_callback(
                    "visibilitychange",
                    &func(&s.on_visibility),
                );
            }
        }
        let _ = s.el.remove_attribute("src");
        if !s.url.is_empty() {
            let _ = Url::revoke_object_url(&s.url);
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        if let Some(s) = SESSIONS.with(|m| m.borrow_mut().remove(&mount)) {
            teardown(s);
        }
    }

    /// 通知を非同期に書き戻す（イベント・タイマーのコールバックから同期 apply しない）。
    fn post(applier: &Applier<State>, f: impl FnOnce(&mut State) + 'static) {
        let a = applier.clone();
        spawn_local(async move { a.apply(f) });
    }

    /// 要素の現在値を通知にする。
    fn snap(el: &HtmlMediaElement, phase: AudioPhase) -> AudioUpdate {
        snapshot_update(phase, el.current_time(), el.duration())
    }

    /// 中断。`pagehide` と hidden な `visibilitychange` から呼ぶ。
    fn interrupt(applier: &Applier<State>, mount: u64) {
        let upd = SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            let s = m.get_mut(&mount)?;
            if !should_interrupt(s.el.paused(), s.pending.is_some()) {
                return None;
            }
            s.pending = None;
            clear_timer(s.timer.take(), false);
            let _ = s.el.pause();
            Some((s.gen, snap(&s.el, AudioPhase::Interrupted)))
        });
        if let Some((g, u)) = upd {
            post(applier, move |s| apply_update(s, g, u));
        }
    }

    fn listen(target: &web_sys::EventTarget, ev: &str, cb: &Cb) {
        let _ = target.add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
    }

    /// 要素・リスナー・`blob:` URL を作る。失敗したら作りかけを解放して失敗を返す。
    fn build(applier: &Applier<State>, mount: u64, gen: u64) -> Result<Session, AudioFailure> {
        let failed = || AudioFailure::Failed(FAILED_PLAY.to_string());
        let window = web_sys::window().ok_or(AudioFailure::Unsupported)?;
        let document = window.document().ok_or(AudioFailure::Unsupported)?;
        let el: HtmlMediaElement = document
            .create_element("audio")
            .map_err(|_| failed())?
            .dyn_into()
            .map_err(|_| failed())?;
        let (a1, a2, a3, a4) = (
            applier.clone(),
            applier.clone(),
            applier.clone(),
            applier.clone(),
        );
        // 終了と失敗は別のイベント（契約表 6）。現在の世代は発火時に Session から読む。
        let on_ended: Cb = Closure::new(move |_| {
            let u = SESSIONS.with(|m| {
                m.borrow().get(&mount).map(|s| {
                    let d = s.el.duration();
                    (s.gen, snapshot_update(AudioPhase::Ended, d, d))
                })
            });
            if let Some((g, u)) = u {
                post(&a1, move |s| apply_update(s, g, u));
            }
        });
        let on_error: Cb = Closure::new(move |_| {
            let g = SESSIONS.with(|m| m.borrow().get(&mount).map(|s| s.gen));
            if let Some(g) = g {
                let f = AudioFailure::Failed(FAILED_DECODE.to_string());
                post(&a2, move |s| apply_update(s, g, AudioUpdate::Failed(f)));
            }
        });
        let on_pagehide: Cb = Closure::new(move |_| interrupt(&a3, mount));
        let on_visibility: Cb = Closure::new(move |_| {
            if web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|d| d.hidden())
            {
                interrupt(&a4, mount);
            }
        });
        // リスナーを先に付け、src 設定後に届く通知を落とさない（契約表 1）。
        listen(&el, "ended", &on_ended);
        listen(&el, "error", &on_error);
        listen(&window, "pagehide", &on_pagehide);
        listen(&document, "visibilitychange", &on_visibility);
        let mut s = Session {
            el,
            url: String::new(),
            gen,
            dispatched: 0,
            pending: None,
            timer: None,
            poll: None,
            on_ended,
            on_error,
            on_pagehide,
            on_visibility,
        };
        let url = (|| {
            let bytes = Uint8Array::from(&demo_wav()[..]);
            let parts = Array::of1(&bytes);
            let opts = BlobPropertyBag::new();
            opts.set_type("audio/wav");
            let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &opts).ok()?;
            Url::create_object_url_with_blob(&blob).ok()
        })();
        match url {
            Some(u) => {
                s.el.set_src(&u);
                s.url = u;
                Ok(s)
            }
            None => {
                teardown(s);
                Err(failed())
            }
        }
    }

    fn set_timer(
        w: &web_sys::Window,
        interval: bool,
        ms: i32,
        f: impl FnMut() + 'static,
    ) -> Option<Timer> {
        let cb: Closure<dyn FnMut()> = Closure::new(f);
        let f = cb.as_ref().unchecked_ref();
        let id = if interval {
            w.set_interval_with_callback_and_timeout_and_arguments_0(f, ms)
        } else {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(f, ms)
        };
        id.ok().map(|id| (id, cb))
    }

    /// 開始確定待ちが世代 `g` のままなら期限切れとして確定する。
    fn expire_now(applier: &Applier<State>, mount: u64, g: u64) {
        let hit = SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            let Some(s) = m
                .get_mut(&mount)
                .filter(|s| s.pending.map(|p| p.0) == Some(g))
            else {
                return false;
            };
            s.pending = None;
            clear_timer(s.timer.take(), false);
            let _ = s.el.pause();
            s.el.set_current_time(0.0);
            true
        });
        if hit {
            post(applier, move |s| expire(s, g));
        }
    }

    /// `play()` の拒否を反映する。AbortError は失敗にしない。
    fn reject(applier: &Applier<State>, mount: u64, g: u64, e: &JsValue) {
        let Some(f) = failure_from_play_error(err_name(e).as_deref()) else {
            return;
        };
        SESSIONS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&mount) {
                s.pending = None;
                clear_timer(s.timer.take(), false);
            }
        });
        post(applier, move |s| apply_update(s, g, AudioUpdate::Failed(f)));
    }

    fn play(applier: &Applier<State>, mount: u64, g: u64) {
        let started = now();
        let reuse = SESSIONS.with(|m| m.borrow().get(&mount).is_some_and(|s| !errored(&s.el)));
        if !reuse {
            let old = SESSIONS.with(|m| m.borrow_mut().remove(&mount));
            if let Some(old) = old {
                teardown(old);
            }
            match build(applier, mount, g) {
                Ok(mut s) => {
                    s.dispatched = g;
                    SESSIONS.with(|m| m.borrow_mut().insert(mount, s));
                }
                Err(f) => {
                    return post(applier, move |s| apply_update(s, g, AudioUpdate::Failed(f)))
                }
            }
        }
        let el = SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            let s = m.get_mut(&mount)?;
            s.gen = g;
            s.pending = Some((g, started));
            clear_timer(s.timer.take(), false);
            let a = applier.clone();
            s.timer = web_sys::window().and_then(|w| {
                set_timer(&w, false, START_DEADLINE.as_millis() as i32, move || {
                    let a = a.clone();
                    spawn_local(async move { expire_now(&a, mount, g) });
                })
            });
            Some(s.el.clone())
        });
        let Some(el) = el else { return };
        // effect の同期区間で呼ぶ（クリックと同じタスク。ユーザー操作の扱いが保たれる）。
        let promise = match el.play() {
            Ok(p) => p,
            Err(e) => return reject(applier, mount, g, &e),
        };
        let a = applier.clone();
        spawn_local(async move {
            let r = JsFuture::from(promise).await;
            let t = now();
            // await 再開後に、まだ現在の要求かを確かめる（契約表 2）。
            let mine = is_live(mount)
                && SESSIONS.with(|m| {
                    m.borrow()
                        .get(&mount)
                        .is_some_and(|s| s.pending.map(|p| p.0) == Some(g))
                });
            if !mine {
                return;
            }
            match r {
                Err(e) => reject(&a, mount, g, &e),
                Ok(_) if is_late(started, t) => expire_now(&a, mount, g),
                Ok(_) => {
                    let u = SESSIONS.with(|m| {
                        let mut m = m.borrow_mut();
                        let s = m.get_mut(&mount)?;
                        s.pending = None;
                        clear_timer(s.timer.take(), false);
                        Some(snap(&s.el, AudioPhase::Playing))
                    });
                    if let Some(u) = u {
                        a.apply(move |s| apply_update(s, g, u));
                    }
                }
            }
        });
    }

    /// 一時停止・停止。保留中の確定待ちと期限タイマーも解除する。
    fn halt(mount: u64, rewind: bool) {
        SESSIONS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&mount) {
                s.pending = None;
                clear_timer(s.timer.take(), false);
                let _ = s.el.pause();
                if rewind {
                    s.el.set_current_time(0.0);
                }
            }
        });
    }

    /// 再生中の位置表示用ポーリング。確定済みで鳴っている間だけ通知する（契約表 4）。
    fn ensure_poll(applier: &Applier<State>, mount: u64) {
        SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            let Some(s) = m.get_mut(&mount).filter(|s| s.poll.is_none()) else {
                return;
            };
            let a = applier.clone();
            s.poll = web_sys::window().and_then(|w| {
                set_timer(&w, true, POLL_INTERVAL.as_millis() as i32, move || {
                    let u = SESSIONS.with(|m| {
                        let m = m.borrow();
                        let s = m.get(&mount)?;
                        (s.pending.is_none() && !s.el.paused())
                            .then(|| (s.gen, snap(&s.el, AudioPhase::Playing)))
                    });
                    if let Some((g, u)) = u {
                        post(&a, move |s| apply_update(s, g, u));
                    }
                })
            });
        });
    }

    /// 画面の effect。世代の照合 → 要求の発送 → ポーリングの張り外しの順。
    pub fn audio_play_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        // 1. 画面の世代を反映し、古い世代の確定待ちは捨てる。
        SESSIONS.with(|m| {
            if let Some(s) = m.borrow_mut().get_mut(&mount) {
                s.gen = state.generation;
                if s.pending.is_some_and(|p| p.0 != state.generation) {
                    s.pending = None;
                    clear_timer(s.timer.take(), false);
                }
            }
        });
        // 2. 発送。同じ世代は二重に始めない（契約表 2）。Session 未作成の Play は常に新規。
        if let Some((cmd, g)) = take_command(state) {
            let fresh = SESSIONS.with(|m| {
                let mut m = m.borrow_mut();
                match m.get_mut(&mount) {
                    Some(s) if s.dispatched == g => false,
                    Some(s) => {
                        s.dispatched = g;
                        true
                    }
                    None => true,
                }
            });
            if fresh {
                applier.apply(move |s| mark_command_taken(s, g));
                match cmd {
                    AudioCommand::Play => play(applier, mount, g),
                    AudioCommand::Pause => halt(mount, false),
                    AudioCommand::Stop => halt(mount, true),
                }
            }
        }
        // 3. ポーリングは Loading / Playing の間だけ。
        if matches!(state.phase, AudioPhase::Loading | AudioPhase::Playing) {
            ensure_poll(applier, mount);
        } else {
            let p = SESSIONS.with(|m| m.borrow_mut().get_mut(&mount).and_then(|s| s.poll.take()));
            clear_timer(p, true);
        }
        // 4. OS のメディア操作（Media Session）へ同期する。
        crate::now_playing::sync(state, applier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_error_names_map() {
        assert_eq!(
            failure_from_play_error(Some("NotAllowedError")),
            Some(AudioFailure::Blocked)
        );
        assert_eq!(failure_from_play_error(Some("AbortError")), None);
        assert_eq!(
            failure_from_play_error(Some("NotSupportedError")),
            Some(AudioFailure::Failed(FAILED_DECODE.to_string()))
        );
        let other = Some(AudioFailure::Failed(FAILED_PLAY.to_string()));
        assert_eq!(failure_from_play_error(Some("X")), other);
        assert_eq!(failure_from_play_error(None), other);
    }

    #[test]
    fn snapshot_rejects_non_finite() {
        let AudioUpdate::Snapshot {
            position_ms,
            duration_ms,
            ..
        } = snapshot_update(AudioPhase::Playing, 1.5, f64::NAN)
        else {
            panic!()
        };
        assert_eq!((position_ms, duration_ms), (Some(1500), None));
        let AudioUpdate::Snapshot {
            position_ms,
            duration_ms,
            ..
        } = snapshot_update(AudioPhase::Playing, -1.0, f64::INFINITY)
        else {
            panic!()
        };
        assert_eq!((position_ms, duration_ms), (None, None));
    }

    #[test]
    fn late_boundary() {
        assert!(!is_late(100.0, 5099.0));
        assert!(is_late(100.0, 5100.0));
    }

    #[test]
    fn interrupt_truth_table() {
        assert!(should_interrupt(false, false));
        assert!(should_interrupt(true, true));
        assert!(should_interrupt(false, true));
        assert!(!should_interrupt(true, false));
    }
}
