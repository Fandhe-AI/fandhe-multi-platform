//! 型サンプル「ロック画面・OS のメディア操作」の Web 経路（親 #98、Issue #384）。
//!
//! `fandhe_core::screens::audio_play`（端末依存ゼロ）が `Playing` / `Paused` の間だけ、Media Session API
//! （`navigator.mediaSession`）へ曲名と再生状態を渡し、ブラウザ・OS のメディア操作（メディアキー・通知・
//! コントロールセンター）から来た再生・一時停止を画面へ返す。`media_playback::audio_play_effect` の最後から
//! [`sync`] が毎フレーム呼ばれる（`fandhe-egui-proxy` の `device/now_playing.rs` の Web 版。同じ手順）。
//! `NowPlaying` trait（`Send + Sync`）は `JsValue` / `Closure` を持てないため通らない。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外（従来どおり `Unsupported`）。契約の SSOT は
//! `fandhe_device::now_playing`。
//!
//! # Reflect を使う理由
//!
//! web-sys の `MediaSession` / `MediaMetadata` は `web_sys_unstable_apis` が必須の不安定 API。
//! ビルド設定を変えないため `js_sys::Reflect` で呼ぶ（`screen_wake_lock` と同方針）。
//!
//! # 並行性
//!
//! 受け口（`play` / `pause` の action handler）は [`SharedOsCommand`] へ `push` するだけで、画面の状態には
//! 触れない。effect が毎回 `take` して `Applier::apply` で画面へ返す。取り下げでは **`take` より先に**
//! `disarm` するので、停止を確定した後に届いた古い操作では再開しない。`Loading`（再開の確定待ち）の間は
//! 操作を取り出さず残す。
//!
//! handler は取り下げ（`clear`）では null に戻さず、マウント破棄（[`release_mount`]）で外す。null に戻すと
//! ブラウザの既定動作が `<audio>` を直接再生・一時停止し、画面は Stopped なのに鳴る食い違いが起きるため。
//! 取り下げ後・背景（`Interrupted`、および一時停止中の `document.hidden`）で届いた操作は push されず、
//! 積まれていても effect で捨てる。
//! 背景遷移は `media_playback` が `Interrupted` にし、そのあとの effect で本モジュールが取り下げる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の操作順、停止・中断後に届いた古い操作で再開しないこと、非対応・失敗を成功へ畳まない
//! こと（Firefox Android は API があっても操作 UI が無いので「OS へ渡しました」までしか言わない）、
//! handler・Closure の解放。例外の `message` は画面にも console にも出さない（security.md A09）。
//! 守らない: 複数タブ・複数マウント（`mediaSession` はページ全体で 1 つのため最後の持ち主が勝つ）、
//! Chrome が短いメディア（素材は 5 秒）に通知を出さない場合があること、位置表示（`setPositionState`）、
//! `Loading` に入る前の既定動作、egui-proxy の wasm canvas 経路。

use fandhe_core::screens::audio_play::OsCommand;
use fandhe_device::now_playing::OsMediaCommand;

/// `MediaSession.playbackState` の値。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn playback_state(playing: bool) -> &'static str {
    if playing {
        "playing"
    } else {
        "paused"
    }
}

/// 操作が画面の状態を変えるか。変えない操作（再生中の Play など）は読み飛ばす。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn changes_state(cmd: OsMediaCommand, playing: bool) -> bool {
    match cmd {
        OsMediaCommand::Play => !playing,
        OsMediaCommand::Pause => playing,
        OsMediaCommand::Toggle => true,
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_screen_command(c: OsMediaCommand) -> OsCommand {
    match c {
        OsMediaCommand::Play => OsCommand::Play,
        OsMediaCommand::Pause => OsCommand::Pause,
        OsMediaCommand::Toggle => OsCommand::Toggle,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::{release_mount, sync};

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;

    use fandhe_core::screens::audio_play::{
        apply_os_command, os_media_cleared, set_os_media_status, AudioPhase, AudioPlayState,
        OsMediaStatus,
    };
    use fandhe_device::now_playing::{OsMediaCommand, SharedOsCommand, DEMO_TITLE};
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen::prelude::*;

    use super::{changes_state, playback_state, to_screen_command};
    use crate::mount::Applier;

    type State = AudioPlayState;
    type Handler = Closure<dyn FnMut(JsValue)>;

    /// ページ全体で 1 つの `mediaSession` の持ち主（1 マウント）。
    struct Entry {
        id: u64,
        /// OS へ出している再生状態（`Some(playing)`）。`None` は取り下げ済み。
        shown: Option<bool>,
        cmds: SharedOsCommand,
        /// `play` / `pause` の handler。`None` は未登録。
        handlers: Option<(Handler, Handler)>,
    }

    thread_local! {
        /// wasm32 は単一スレッド。
        static ENTRY: RefCell<Option<Entry>> = const { RefCell::new(None) };
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn set(o: &JsValue, key: &str, v: &JsValue) -> Result<(), JsValue> {
        Reflect::set(o, &JsValue::from_str(key), v).map(|_| ())
    }

    /// `navigator.mediaSession`。非対応では `None`（型に現れない実態を検証する、A03）。
    fn media_session() -> Option<JsValue> {
        let nav = web_sys::window()?.navigator();
        let ms = get(&nav, "mediaSession");
        ms.is_object().then_some(ms)
    }

    fn set_handler(ms: &JsValue, name: &str, f: &JsValue) -> Result<(), JsValue> {
        let m: Function = get(ms, "setActionHandler").dyn_into()?;
        m.call2(ms, &JsValue::from_str(name), f).map(|_| ())
    }

    /// 曲名と再生状態を消す（handler は残す。モジュール doc 参照）。
    fn clear(ms: &JsValue) {
        let _ = set(ms, "metadata", &JsValue::NULL);
        let _ = set(ms, "playbackState", &JsValue::from_str("none"));
    }

    /// 曲名と再生状態を渡す。例外は固定の `Failed` に潰し、作りかけを取り下げる。
    fn publish(playing: bool) -> Result<(), OsMediaStatus> {
        let ms = media_session().ok_or(OsMediaStatus::Unsupported)?;
        let ctor: Function = Reflect::get(&js_sys::global(), &JsValue::from_str("MediaMetadata"))
            .ok()
            .and_then(|c| c.dyn_into().ok())
            .ok_or(OsMediaStatus::Unsupported)?;
        let r = (|| -> Result<(), JsValue> {
            let init = Object::new();
            set(&init, "title", &JsValue::from_str(DEMO_TITLE))?;
            let meta = Reflect::construct(&ctor, &Array::of1(&init))?;
            set(&ms, "metadata", &meta)?;
            set(
                &ms,
                "playbackState",
                &JsValue::from_str(playback_state(playing)),
            )
        })();
        r.map_err(|_| {
            clear(&ms);
            OsMediaStatus::Failed
        })
    }

    /// 背景（`document.hidden`）か。一時停止中は `media_playback::interrupt` が何もしないため、
    /// 背景での再生開始はここで防ぐ（前景だけで扱う契約）。
    fn is_hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }

    /// handler の中身は push だけ。積めたときだけ effect を起こす（契約表 11）。背景で届いた操作は捨てる。
    fn handler(cmds: &SharedOsCommand, cmd: OsMediaCommand, applier: &Applier<State>) -> Handler {
        let (cmds, applier) = (cmds.clone(), applier.clone());
        Closure::new(move |_: JsValue| {
            if !is_hidden() && cmds.push(cmd) {
                applier.apply(|_| {});
            }
        })
    }

    /// handler を未登録なら登録する。途中の失敗では登録済みの分を戻す（契約表 9）。
    fn install(e: &mut Entry, applier: &Applier<State>) -> Result<(), OsMediaStatus> {
        if e.handlers.is_some() {
            return Ok(());
        }
        let ms = media_session().ok_or(OsMediaStatus::Unsupported)?;
        let play = handler(&e.cmds, OsMediaCommand::Play, applier);
        let pause = handler(&e.cmds, OsMediaCommand::Pause, applier);
        let ok = set_handler(&ms, "play", play.as_ref())
            .and_then(|()| set_handler(&ms, "pause", pause.as_ref()));
        if ok.is_err() {
            let _ = set_handler(&ms, "play", &JsValue::NULL);
            let _ = set_handler(&ms, "pause", &JsValue::NULL);
            return Err(OsMediaStatus::Failed);
        }
        e.handlers = Some((play, pause));
        Ok(())
    }

    /// 受付を止め、表示を消し、handler を外して Closure を drop する。
    fn teardown(e: Entry) {
        e.cmds.disarm();
        if let Some(ms) = media_session() {
            clear(&ms);
            if e.handlers.is_some() {
                let _ = set_handler(&ms, "play", &JsValue::NULL);
                let _ = set_handler(&ms, "pause", &JsValue::NULL);
            }
        }
    }

    /// マウント破棄時の後片付け。`mount::sweep_mounts` から呼ばれる。
    pub(crate) fn release_mount(id: u64) {
        let owned = ENTRY.with(|c| {
            let mut s = c.borrow_mut();
            if s.as_ref().is_some_and(|e| e.id == id) {
                s.take()
            } else {
                None
            }
        });
        if let Some(e) = owned {
            teardown(e);
        }
    }

    /// 毎フレームの同期。`media_playback::audio_play_effect` の最後から呼ぶ。
    pub(crate) fn sync(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        let playing = match state.phase {
            AudioPhase::Playing => true,
            AudioPhase::Paused => false,
            AudioPhase::Loading => {
                // 再開の確定待ち。表示も保留中の操作も残す。
                ENTRY.with(|c| {
                    if let Some(e) = c.borrow_mut().as_mut().filter(|e| e.id == id) {
                        let _ = install(e, applier);
                    }
                });
                return;
            }
            _ => {
                ENTRY.with(|c| {
                    let mut s = c.borrow_mut();
                    let Some(e) = s.as_mut().filter(|e| e.id == id) else {
                        return;
                    };
                    if e.shown.take().is_some() {
                        e.cmds.disarm();
                        if let Some(ms) = media_session() {
                            clear(&ms);
                        }
                        applier.apply(os_media_cleared);
                    }
                });
                return;
            }
        };
        ENTRY.with(|c| {
            let mut s = c.borrow_mut();
            // 別マウントが持っていたら先に片付ける（複数マウントは守らない範囲）。
            if s.as_ref().is_some_and(|e| e.id != id) {
                if let Some(old) = s.take() {
                    teardown(old);
                }
            }
            let e = s.get_or_insert_with(|| Entry {
                id,
                shown: None,
                cmds: SharedOsCommand::new(),
                handlers: None,
            });
            // 状態を変える操作は 1 回の effect で 1 件だけ返す（変えない操作は読み飛ばす）。
            let hidden = is_hidden();
            while let Some(cmd) = e.cmds.take() {
                // 背景では取り出して捨てる（積まれた後に hidden になった操作でも再生しない）。
                if hidden {
                    continue;
                }
                if changes_state(cmd, playing) {
                    let cmd = to_screen_command(cmd);
                    applier.apply(move |s| apply_os_command(s, cmd));
                    break;
                }
            }
            // 変わったときだけ渡す。それ以外は apply しない（再描画が止まらなくなるのを防ぐ、契約表 4）。
            if e.shown != Some(playing) {
                e.shown = Some(playing);
                e.cmds.arm();
                let status = match install(e, applier).and_then(|()| publish(playing)) {
                    Ok(()) => OsMediaStatus::Sent,
                    Err(st) => {
                        e.cmds.disarm();
                        st
                    }
                };
                applier.apply(move |s| set_os_media_status(s, status));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_state_truth_table() {
        use OsMediaCommand::*;
        assert!(!changes_state(Play, true));
        assert!(changes_state(Play, false));
        assert!(changes_state(Pause, true));
        assert!(!changes_state(Pause, false));
        assert!(changes_state(Toggle, true));
        assert!(changes_state(Toggle, false));
    }

    #[test]
    fn playback_state_values() {
        assert_eq!(playback_state(true), "playing");
        assert_eq!(playback_state(false), "paused");
    }
}
