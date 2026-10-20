//! 型サンプル「ライト（トーチ）」の Web 経路（親 #106、Issue #462、対応表
//! `docs/device/capabilities/flashlight.json`）。
//!
//! `fandhe_core::screens::flashlight_toggle`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_request` で要求を観測し、
//! `getUserMedia`（背面カメラ）→ トラックの `getCapabilities().torch` 確認 →
//! `applyConstraints({ advanced: [{ torch: true }] })` で点灯する。`fandhe-device` の trait は通らない
//! （他の Web モジュールと同じ。`fandhe-egui-proxy` の wasm canvas 経路は対象外）。
//!
//! # Reflect を使う理由
//!
//! web-sys には `torch` を持つ型が無く、`getCapabilities` / `getSettings` / `applyConstraints` の
//! feature も未導入。依存と feature を増やさないため `js_sys::Reflect` で呼ぶ。
//!
//! # 方式
//!
//! - 点灯: トラックを保持している間だけライトが点く。消灯は `track.stop()`（カメラも解放される）
//! - 表示: `getSettings().torch` を読み戻した値で決める。要求後は `CONFIRM_POLL` ごとに読み、
//!   `CONFIRM_TIMEOUT`（`performance.now()` の単調時計）までに一致しなければ失敗にする
//! - 外からの消灯（トラックの `ended`）への追従: `READ_INTERVAL` の `setInterval` が `readyState` を読む
//! - 背景遷移（`visibilitychange` で hidden・`pagehide`）・破棄: 自分から消灯し、戻っても点け直さない
//! - 非同期の再開後は世代（epoch）とマウントの生存を再確認し、古い結果で取れたストリームは即停止する
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内でトラックを漏らさない（消灯・失敗・非対応・世代切れ・背景・破棄の全経路で
//! `stop()`）、非対応・拒否・secure context 外を成功と見せない、期限後に届いた点灯を成功にしない。
//! 例外は `name` だけで写し `message`・デバイス ID・ラベルは出さない（security.md A09）。
//! 守らない: 外からの消灯への追従の遅れ（1 秒 + 1 フレーム）、複数タブ間の食い違い、
//! ページ終了時の後片付け（ブラウザが解放）。

use fandhe_core::screens::camera_capture::CaptureFailure;
use fandhe_core::screens::flashlight_toggle::{FlashlightFailure, NOT_CONFIRMED_REASON};

/// 許可拒否の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const PERMISSION_DENIED: &str = "カメラの許可がありません";
/// 点灯失敗の固定文言（使用中・過熱・例外）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SET_FAILED: &str = "ライトを点けられませんでした";
/// 状態の読み取り失敗の固定文言（画面側は `READ_FAILED` へ置き換える）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_ERROR: &str = "状態を読み取れませんでした";
/// 点灯の途中で背景へ回った・破棄された・消灯が先に来たときの文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const HIDDEN_REASON: &str = "点灯を中断しました";

/// 反映の確認を待つ上限（ミリ秒）。契約の `CONFIRM_TIMEOUT` と同じ値。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CONFIRM_TIMEOUT_MS: f64 = fandhe_device::flashlight::CONFIRM_TIMEOUT.as_millis() as f64;

/// カメラ画面と共通の失敗種別を、ライト画面の失敗種別へ写す。例外の `message` は受け取らない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_capture(f: CaptureFailure) -> FlashlightFailure {
    match f {
        CaptureFailure::Unsupported => FlashlightFailure::Unsupported,
        CaptureFailure::PermissionDenied => {
            FlashlightFailure::Failed(PERMISSION_DENIED.to_string())
        }
        CaptureFailure::Other(_) => FlashlightFailure::Failed(SET_FAILED.to_string()),
    }
}

/// `getCapabilities().torch` の判定。`Some(true)` のときだけ点灯を試す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn capability_outcome(torch: Option<bool>) -> Result<(), FlashlightFailure> {
    if torch == Some(true) {
        Ok(())
    } else {
        Err(FlashlightFailure::Unsupported)
    }
}

/// 読み戻した点灯状態。トラックが無い・`ended` なら消灯、`torch` が読めなければ失敗
/// （前回の値は返さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn read_state(
    holding: bool,
    ended: bool,
    torch: Option<bool>,
) -> Result<Option<bool>, FlashlightFailure> {
    if !holding || ended {
        return Ok(Some(false));
    }
    torch
        .map(Some)
        .ok_or_else(|| FlashlightFailure::Failed(READ_ERROR.to_string()))
}

/// 確認ループの結果。`Ok`/`Err` と、読み戻した点灯状態。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) type Confirmed = (Result<(), FlashlightFailure>, Option<bool>);

/// 確認ループの 1 歩。`Some` で確定、`None` で続行。読み取り失敗 → 期限 → 一致の順
/// （期限を過ぎてから届いた点灯は成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn confirm_step(
    elapsed_ms: f64,
    read: Result<Option<bool>, FlashlightFailure>,
    want: bool,
) -> Option<Confirmed> {
    let read = match read {
        Ok(v) => v,
        Err(e) => return Some((Err(e), None)),
    };
    if elapsed_ms >= CONFIRM_TIMEOUT_MS {
        let f = FlashlightFailure::Failed(NOT_CONFIRMED_REASON.to_string());
        return Some((Err(f), read));
    }
    (read == Some(want)).then_some((Ok(()), read))
}

#[cfg(target_arch = "wasm32")]
pub use web::flashlight_toggle_effect;

/// マウント破棄時にトラックとリスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::flashlight_toggle::{
        apply_read, finish, mark_applying, mark_confirming, take_request, FlashlightFailure,
        FlashlightToggleState,
    };
    use fandhe_device::flashlight::{CONFIRM_POLL, READ_INTERVAL};
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{MediaStream, MediaStreamTrack};

    use super::{
        capability_outcome, confirm_step, failure_from_capture, read_state, Confirmed,
        HIDDEN_REASON, SET_FAILED,
    };
    use crate::camera_capture::failure_from_error_name;
    use crate::mount::{is_live, Applier};
    use crate::screen_wake_lock::is_current;

    type State = FlashlightToggleState;
    type ReadResult = Result<Option<bool>, FlashlightFailure>;

    #[derive(Default)]
    struct Entry {
        /// 点灯中のトラックとそのストリーム（停止用）。
        track: Option<MediaStreamTrack>,
        stream: Option<MediaStream>,
        /// 要求・背景遷移のたびに進める世代。
        epoch: u64,
        /// 利用者が点灯を望んでいるか。
        desired: bool,
        /// effect の処理中（二重起動防止）。
        busy: bool,
        /// 非対応と判明している。tick が `Unsupported` を返し続ける。
        unsupported: bool,
        /// 最後に画面へ送った読み取り結果（同じ値を送り続けて再描画しないため）。
        last: Option<ReadResult>,
        interval: Option<(i32, Closure<dyn FnMut()>)>,
        on_visible: Option<Closure<dyn FnMut()>>,
        on_pagehide: Option<Closure<dyn FnMut()>>,
    }

    thread_local! {
        /// マウントごとの状態（wasm32 は単一スレッド）。
        static LIGHTS: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        LIGHTS.with(|l| l.borrow_mut().get_mut(&id).map(f))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn set(o: &JsValue, key: &str, v: &JsValue) {
        let _ = Reflect::set(o, &JsValue::from_str(key), v);
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    /// 例外の `name` だけを読む（`message` は使わない）。
    fn err_name(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    async fn sleep_ms(ms: i32) {
        let promise = Promise::new(&mut |resolve, _reject| match web_sys::window() {
            Some(w) => {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            }
            None => {
                let _ = resolve.call0(&JsValue::NULL);
            }
        });
        let _ = JsFuture::from(promise).await;
    }

    /// `navigator.mediaDevices`。secure context 外・非対応では `None`（型に現れない実態を検証する、A03）。
    fn media_devices() -> Option<JsValue> {
        let w = web_sys::window()?;
        if get(&w, "isSecureContext").as_bool() != Some(true) {
            return None;
        }
        let md = get(&w.navigator(), "mediaDevices");
        (md.is_object() && get(&md, "getUserMedia").is_function()).then_some(md)
    }

    /// `obj.method(arg)` を呼んで Promise を得る。例外・非 Promise は `Err`。
    fn call_promise(obj: &JsValue, method: &str, arg: &JsValue) -> Result<Promise, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        f.call1(obj, arg)?.dyn_into()
    }

    fn call0(obj: &JsValue, method: &str) -> Option<JsValue> {
        let f: Function = get(obj, method).dyn_into().ok()?;
        f.call0(obj).ok()
    }

    /// ストリームの全トラックを止める（ライトが消え、カメラ使用中の表示も消える）。
    fn stop_stream(stream: &MediaStream) {
        for t in stream.get_tracks().iter() {
            if let Ok(t) = t.dyn_into::<MediaStreamTrack>() {
                t.stop();
            }
        }
    }

    /// Entry が持つトラックを取り出して止める。持っていなくても何もしない。
    fn drop_track(id: u64) {
        let stream = with(id, |e| {
            e.track = None;
            e.stream.take()
        })
        .flatten();
        if let Some(s) = stream {
            stop_stream(&s);
        }
    }

    fn still_current(id: u64, start: u64) -> bool {
        with(id, |e| is_current(is_live(id), e.epoch, start, e.desired)).unwrap_or(false)
    }

    /// 現在のトラックから読み戻す。
    fn read_current(id: u64) -> ReadResult {
        with(id, |e| match &e.track {
            None => read_state(false, false, None),
            Some(t) => {
                let ended = get(t, "readyState").as_string().as_deref() == Some("ended");
                let torch = call0(t, "getSettings").and_then(|s| get(&s, "torch").as_bool());
                read_state(true, ended, torch)
            }
        })
        .unwrap_or(Ok(Some(false)))
    }

    fn video_constraints() -> JsValue {
        let facing = Object::new();
        set(&facing, "ideal", &JsValue::from_str("environment"));
        let video = Object::new();
        set(&video, "facingMode", &facing);
        let c = Object::new();
        set(&c, "video", &video);
        c.into()
    }

    fn torch_constraints() -> JsValue {
        let torch = Object::new();
        set(&torch, "torch", &JsValue::TRUE);
        let advanced = Array::of1(&torch);
        let c = Object::new();
        set(&c, "advanced", &advanced);
        c.into()
    }

    fn mark_unsupported(id: u64) {
        with(id, |e| e.unsupported = true);
    }

    fn aborted() -> Confirmed {
        (
            Err(FlashlightFailure::Failed(HIDDEN_REASON.into())),
            Some(false),
        )
    }

    fn failed_set() -> Confirmed {
        (
            Err(FlashlightFailure::Failed(SET_FAILED.into())),
            Some(false),
        )
    }

    async fn turn_on(applier: &Applier<State>, id: u64, generation: u64) -> Confirmed {
        let Some(md) = media_devices() else {
            mark_unsupported(id);
            return (Err(FlashlightFailure::Unsupported), None);
        };
        // 生きているトラックが既に点灯中なら何もしない（冪等）。
        if read_current(id) == Ok(Some(true)) {
            return (Ok(()), Some(true));
        }
        let Some(start) = with(id, |e| {
            e.desired = true;
            e.epoch += 1;
            e.epoch
        }) else {
            return aborted();
        };
        // 再点灯時に旧ストリームを残さない（getSettings 失敗・torch 喪失後の上書きリーク防止）。
        drop_track(id);
        let fail = |e: Option<&JsValue>| {
            let name = e.and_then(err_name);
            let f = failure_from_capture(failure_from_error_name(name.as_deref()));
            if f == FlashlightFailure::Unsupported {
                mark_unsupported(id);
            }
            (Err(f), Some(false))
        };

        let stream: MediaStream = match call_promise(&md, "getUserMedia", &video_constraints()) {
            Ok(p) => match JsFuture::from(p).await {
                Ok(v) => match v.dyn_into() {
                    Ok(s) => s,
                    Err(_) => return fail(None),
                },
                Err(e) => return fail(Some(&e)),
            },
            Err(e) => return fail(Some(&e)),
        };
        // await 再開後の再確認。古い結果で取れたストリームは即停止する（契約表 2・9）。
        if !still_current(id, start) {
            stop_stream(&stream);
            return aborted();
        }
        // applyConstraints の await 前に登録し、背景遷移・破棄の停止対象に入れる（契約表 1・9）。
        if with(id, |e| e.stream = Some(stream.clone())).is_none() {
            stop_stream(&stream);
            return aborted();
        }
        let Ok(track) = stream
            .get_video_tracks()
            .get(0)
            .dyn_into::<MediaStreamTrack>()
        else {
            stop_stream(&stream);
            drop_track(id);
            mark_unsupported(id);
            return (Err(FlashlightFailure::Unsupported), Some(false));
        };
        let torch = call0(&track, "getCapabilities").and_then(|c| get(&c, "torch").as_bool());
        if let Err(f) = capability_outcome(torch) {
            stop_stream(&stream);
            drop_track(id);
            mark_unsupported(id);
            return (Err(f), Some(false));
        }
        let applied = match call_promise(&track, "applyConstraints", &torch_constraints()) {
            Ok(p) => JsFuture::from(p).await.is_ok(),
            Err(_) => false,
        };
        if !applied {
            stop_stream(&stream);
            drop_track(id);
            return failed_set();
        }
        if !still_current(id, start) {
            stop_stream(&stream);
            drop_track(id);
            return aborted();
        }
        with(id, |e| e.track = Some(track));

        applier.apply(move |s| mark_confirming(s, generation));
        let began = now();
        let outcome = loop {
            sleep_ms(CONFIRM_POLL.as_millis() as i32).await;
            if !still_current(id, start) {
                break aborted();
            }
            if let Some(done) = confirm_step((now() - began).max(0.0), read_current(id), true) {
                break done;
            }
        };
        if outcome.0.is_err() {
            drop_track(id);
        }
        outcome
    }

    /// 消灯。何も持っていなくても `Ok`（冪等）。
    fn turn_off(id: u64) -> Confirmed {
        with(id, |e| {
            e.desired = false;
            e.epoch += 1;
        });
        drop_track(id);
        (Ok(()), Some(false))
    }

    /// 背景へ回った・ページを離れる。消灯して戻っても点け直さず、表示を消灯へ揃える。
    fn go_background(applier: &Applier<State>, id: u64) {
        if with(id, |e| {
            e.desired = false;
            e.epoch += 1;
        })
        .is_none()
        {
            return;
        }
        drop_track(id);
        with(id, |e| e.last = Some(Ok(Some(false))));
        applier.apply(|s| apply_read(s, Ok(Some(false))));
    }

    fn tick(applier: &Applier<State>, id: u64) {
        if !is_live(id) || with(id, |e| e.busy).unwrap_or(true) {
            return;
        }
        let r = if with(id, |e| e.unsupported).unwrap_or(false) {
            Err(FlashlightFailure::Unsupported)
        } else {
            read_current(id)
        };
        // 外から消された（ended）トラックは捨てる。
        if r == Ok(Some(false)) {
            drop_track(id);
        }
        let changed = with(id, |e| {
            if e.last.as_ref() == Some(&r) {
                return false;
            }
            e.last = Some(r.clone());
            true
        })
        .unwrap_or(false);
        if changed {
            applier.apply(move |s| apply_read(s, r));
        }
    }

    /// 初回だけ Entry・タイマー・リスナーを用意し、最初の読み取りを入れる。
    fn install(applier: &Applier<State>, id: u64) {
        if LIGHTS.with(|l| l.borrow().contains_key(&id)) {
            return;
        }
        LIGHTS.with(|l| l.borrow_mut().insert(id, Entry::default()));
        let Some(window) = web_sys::window() else {
            applier.apply(|s| apply_read(s, Err(FlashlightFailure::Unsupported)));
            return;
        };

        let a = applier.clone();
        let interval = Closure::<dyn FnMut()>::new(move || tick(&a, id));
        let handle = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                interval.as_ref().unchecked_ref(),
                READ_INTERVAL.as_millis() as i32,
            )
            .ok();

        let a = applier.clone();
        let on_visible = Closure::<dyn FnMut()>::new(move || {
            if web_sys::window()
                .and_then(|w| w.document())
                .is_some_and(|d| d.hidden())
            {
                go_background(&a, id);
            }
        });
        if let Some(doc) = window.document() {
            let _ = doc.add_event_listener_with_callback(
                "visibilitychange",
                on_visible.as_ref().unchecked_ref(),
            );
        }
        let a = applier.clone();
        let on_pagehide = Closure::<dyn FnMut()>::new(move || go_background(&a, id));
        let _ = window
            .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref());

        let first = if media_devices().is_some() {
            Ok(Some(false))
        } else {
            Err(FlashlightFailure::Unsupported)
        };
        with(id, |e| {
            e.unsupported = first.is_err();
            e.last = Some(first.clone());
            e.interval = handle.map(|h| (h, interval));
            e.on_visible = Some(on_visible);
            e.on_pagehide = Some(on_pagehide);
        });
        applier.apply(move |s| apply_read(s, first));
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn flashlight_toggle_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        install(applier, id);
        let Some(req) = take_request(state) else {
            return;
        };
        if !with(id, |e| !std::mem::replace(&mut e.busy, true)).unwrap_or(false) {
            return;
        }
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            applier.apply(mark_applying);
            let (res, read) = if req.on {
                turn_on(&applier, id, req.generation).await
            } else {
                turn_off(id)
            };
            // 状態を確定させてから結果を届ける（契約表 2）。
            with(id, |e| {
                e.busy = false;
                e.last = Some(if e.unsupported {
                    Err(FlashlightFailure::Unsupported)
                } else {
                    Ok(read)
                });
            });
            applier.apply(move |s| finish(s, req.generation, res, read));
        });
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = LIGHTS.with(|l| l.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(w) = web_sys::window() {
            if let Some((h, _cb)) = entry.interval.take() {
                w.clear_interval_with_handle(h);
            }
            if let Some(cb) = entry.on_pagehide.take() {
                let _ =
                    w.remove_event_listener_with_callback("pagehide", cb.as_ref().unchecked_ref());
            }
            if let (Some(cb), Some(doc)) = (entry.on_visible.take(), w.document()) {
                let _ = doc.remove_event_listener_with_callback(
                    "visibilitychange",
                    cb.as_ref().unchecked_ref(),
                );
            }
        }
        if let Some(s) = entry.stream.take() {
            stop_stream(&s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(s: &str) -> FlashlightFailure {
        FlashlightFailure::Failed(s.to_string())
    }

    #[test]
    fn capture_failure_maps_without_message() {
        assert_eq!(
            failure_from_capture(CaptureFailure::Unsupported),
            FlashlightFailure::Unsupported
        );
        assert_eq!(
            failure_from_capture(CaptureFailure::PermissionDenied),
            failed(PERMISSION_DENIED)
        );
        assert_eq!(
            failure_from_capture(CaptureFailure::Other("secret".into())),
            failed(SET_FAILED)
        );
    }

    #[test]
    fn only_torch_true_is_supported() {
        assert!(capability_outcome(Some(true)).is_ok());
        for t in [Some(false), None] {
            assert_eq!(capability_outcome(t), Err(FlashlightFailure::Unsupported));
        }
    }

    #[test]
    fn read_state_never_reuses_previous_value() {
        assert_eq!(read_state(false, false, Some(true)), Ok(Some(false)));
        assert_eq!(read_state(true, true, Some(true)), Ok(Some(false)));
        assert_eq!(read_state(true, false, Some(true)), Ok(Some(true)));
        assert_eq!(read_state(true, false, Some(false)), Ok(Some(false)));
        assert_eq!(read_state(true, false, None), Err(failed(READ_ERROR)));
    }

    #[test]
    fn confirm_step_orders_error_deadline_match() {
        assert_eq!(confirm_step(0.0, Ok(Some(false)), true), None);
        assert_eq!(
            confirm_step(100.0, Ok(Some(true)), true),
            Some((Ok(()), Some(true)))
        );
        // 期限を過ぎてから届いた点灯は成功にしない。
        let late = confirm_step(CONFIRM_TIMEOUT_MS, Ok(Some(true)), true).unwrap();
        assert_eq!(late.0, Err(failed(NOT_CONFIRMED_REASON)));
        let err = confirm_step(0.0, Err(failed("x")), true).unwrap();
        assert_eq!((err.0, err.1), (Err(failed("x")), None));
    }
}
