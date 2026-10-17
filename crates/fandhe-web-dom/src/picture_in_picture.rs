//! 型サンプル「ピクチャ・イン・ピクチャ」の Web 経路（親 #97、Issue #367）。
//!
//! `fandhe_core::screens::pip_show`（端末依存ゼロ）の状態機械を
//! [`super::mount::mount_with_effect`] の effect として直接駆動する。`take_command` で要求を観測し
//! Picture-in-Picture API へ渡し、`apply_update` / `set_preview` / `expire` で結果を返す。
//! `fandhe-device` の trait は通らない（他の Web モジュールと同じ。`fandhe-egui-proxy` の
//! wasm canvas 経路は対象外）。色表・期限は `fandhe_device::picture_in_picture` と共有する。
//!
//! # 方式
//!
//! - 映像源は実行時に生成する単色 canvas（`captureStream()`）を `<video>` に入れたもの。
//!   `<video>` は mount root の再構築で消えないよう `document.body` 直下へ不可視で置く
//! - PiP API は web-sys では unstable のため `Reflect` + `Function` で呼ぶ（`fullscreen_immersive` と同じ）。
//!   prefixed API（`webkitSetPresentationMode`）と Document PiP は使わない
//! - `requestPictureInPicture` は利用者のクリックから同期に連なる effect 内で呼ぶ
//!   （`await` 後だと transient activation を失う）。Promise は待つだけ
//! - 状態は `enterpictureinpicture` / `leavepictureinpicture` を真実とし、Promise の解決も同じ
//!   確定経路（保留中の要求 1 件を一度だけ確定）を通す。`leave` は失敗ではなく `Exited`
//! - 非対応（`pictureInPictureEnabled` が真でない・`requestPictureInPicture` / `captureStream` が無い）は
//!   `Unsupported`
//! - 準備・開始・終了の 3 遷移すべてに `START_DEADLINE` を掛ける（起点は呼び出しの直前）。
//!   経過時間は単調時計 `performance.now()` で測る
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の操作順と世代の照合、非対応・拒否を小窓中と見せないこと、
//! リスナー・タイマー・ストリーム・`<video>` の解放、`pagehide` で小窓を閉じること。
//! 例外の `message` は転送しない（security.md A09）。
//! 守らない: 複数タブ・複数マウント、画面内プレビューと小窓の色替えの時刻ずれ、小窓の位置・大きさ、
//! 期限後にブラウザが遅れて切り替えた場合の失敗文言、Document PiP、音声。

use fandhe_core::screens::pip_show::PipShowFailure;
use std::time::Duration;

/// 拒否・例外時の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REJECTED: &str = "ブラウザが小窓への切り替えを受け付けませんでした";

/// `DOMException.name` を画面側の失敗種別へ写す。`message` は使わない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_name(name: &str) -> PipShowFailure {
    match name {
        "NotAllowedError" => PipShowFailure::NotAllowed,
        "NotSupportedError" => PipShowFailure::Unsupported,
        _ => PipShowFailure::Failed(REJECTED.to_string()),
    }
}

/// 準備の可否。3 条件がすべて揃ったときだけ `true`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide_prepare(enabled: bool, has_request: bool, has_capture: bool) -> bool {
    enabled && has_request && has_capture
}

/// 準備開始からの経過ミリ秒に対する色。負・NaN は 0 として扱う（時計の巻き戻し対策）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn color_at(elapsed_ms: f64) -> [u8; 3] {
    let ms = if elapsed_ms.is_finite() && elapsed_ms > 0.0 {
        elapsed_ms as u64
    } else {
        0
    };
    fandhe_device::picture_in_picture::frame_color(Duration::from_millis(ms))
}

#[cfg(target_arch = "wasm32")]
pub use web::pip_show_effect;

/// マウント破棄時に小窓・映像・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::pip_show::{
        apply_update, expire, mark_command_taken, prepare, set_preview, take_command, PipCommand,
        PipShowFailure, PipShowPhase, PipShowState, PipShowUpdate,
    };
    use fandhe_device::picture_in_picture::{
        FRAME_HEIGHT, FRAME_WIDTH, POLL_INTERVAL, START_DEADLINE,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{CanvasRenderingContext2d, EventTarget, HtmlCanvasElement, HtmlVideoElement};

    use super::{color_at, decide_prepare, failure_from_name, REJECTED};
    use crate::mount::{is_live, Applier};

    type State = PipShowState;
    type Listener = (EventTarget, &'static str, Closure<dyn FnMut()>);

    /// 不可視 `<video>` の style。1px・透明・クリックを奪わない。
    const VIDEO_STYLE: &str =
        "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none";

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Prepare,
        Start,
        Stop,
    }

    struct Pending {
        generation: u64,
        kind: Kind,
        timer: (i32, Closure<dyn FnMut()>),
    }

    #[derive(Default)]
    struct Entry {
        /// 発送済みの世代（同じ世代を二度渡さない）。
        started: Option<u64>,
        pending: Option<Pending>,
        video: Option<HtmlVideoElement>,
        stream: Option<web_sys::MediaStream>,
        interval: Option<(i32, Closure<dyn FnMut()>)>,
        /// `<video>` に付けたリスナー。`teardown_media` で外す。
        media_listeners: Vec<Listener>,
        /// window の `pagehide`。マウント破棄で外す。
        page_listeners: Vec<Listener>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        ENTRIES.with(|e| e.borrow_mut().get_mut(&id).map(f))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn call(obj: &JsValue, method: &str) -> Result<JsValue, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        f.call0(obj)
    }

    fn name_of(err: &JsValue) -> String {
        get(err, "name").as_string().unwrap_or_default()
    }

    fn rejected() -> PipShowFailure {
        PipShowFailure::Failed(REJECTED.to_string())
    }

    fn document() -> Option<web_sys::Document> {
        web_sys::window()?.document()
    }

    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    fn remove_all(listeners: Vec<Listener>) {
        for (target, name, cb) in listeners {
            let _ = target.remove_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
        }
    }

    /// `target` に `name` のリスナーを付け、`teardown` で外せるよう Entry に預ける。
    fn listen(
        id: u64,
        page: bool,
        target: EventTarget,
        name: &'static str,
        cb: Closure<dyn FnMut()>,
    ) {
        if target
            .add_event_listener_with_callback(name, cb.as_ref().unchecked_ref())
            .is_err()
        {
            return;
        }
        with(id, |e| {
            let v = if page {
                &mut e.page_listeners
            } else {
                &mut e.media_listeners
            };
            v.push((target, name, cb));
        });
    }

    /// 自分の `<video>` が小窓中なら閉じる（best effort。結果は `leavepictureinpicture` で受ける）。
    fn exit_if_ours(id: u64) {
        if is_ours(id) {
            if let Some(doc) = document() {
                let _ = call(&JsValue::from(doc), "exitPictureInPicture");
            }
        }
    }

    fn is_ours(id: u64) -> bool {
        let Some(doc) = document() else {
            return false;
        };
        let cur = get(&JsValue::from(doc), "pictureInPictureElement");
        with(id, |e| {
            e.video
                .as_ref()
                .is_some_and(|v| cur.is_object() && cur == JsValue::from(v.clone()))
        })
        .unwrap_or(false)
    }

    /// 映像資源（小窓・タイマー・ストリーム・`<video>`・リスナー）を解放する。
    /// 失敗経路・期限切れ・マウント破棄の全てがここを通る（契約表 9）。Entry 自体は残す。
    fn teardown_media(id: u64) {
        exit_if_ours(id);
        let (pending, interval, video, stream, listeners) = match with(id, |e| {
            (
                e.pending.take(),
                e.interval.take(),
                e.video.take(),
                e.stream.take(),
                std::mem::take(&mut e.media_listeners),
            )
        }) {
            Some(t) => t,
            None => return,
        };
        let win = web_sys::window();
        if let Some(w) = &win {
            if let Some(p) = &pending {
                w.clear_timeout_with_handle(p.timer.0);
            }
            if let Some((h, _)) = &interval {
                w.clear_interval_with_handle(*h);
            }
        }
        remove_all(listeners);
        if let Some(s) = stream {
            stream_stop(&s);
        }
        if let Some(v) = video {
            v.set_src_object(None);
            v.remove();
        }
    }

    /// 保留中の要求が `kind` なら取り出してタイマーを止める。違えば `None`（二重確定の防止）。
    fn take_pending(id: u64, kind: Kind) -> Option<u64> {
        let p = with(id, |e| match &e.pending {
            Some(p) if p.kind == kind => e.pending.take(),
            _ => None,
        })??;
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(p.timer.0);
        }
        Some(p.generation)
    }

    /// 保留中の要求が `kind` かつ世代 `gen` のままか。`await` 再開後に古い世代の完了通知を捨てる判定（契約表 2）。
    /// 単一スレッドなので、この判定の直後に `settle` しても割り込まれない。
    fn is_current(id: u64, kind: Kind, gen: u64) -> bool {
        with(
            id,
            |e| matches!(&e.pending, Some(p) if p.kind == kind && p.generation == gen),
        )
        .unwrap_or(false)
    }

    /// 保留中の `kind` の要求を確定する。タイマーを止めてから状態を確定する（契約表 2・9）。
    /// 失敗なら映像資源も解放する。
    fn settle(applier: &Applier<State>, id: u64, kind: Kind, update: PipShowUpdate) {
        let Some(gen) = take_pending(id, kind) else {
            return;
        };
        let failed = matches!(update, PipShowUpdate::Failed(_));
        applier.apply(move |s| apply_update(s, gen, update));
        if failed {
            teardown_media(id);
        }
    }

    /// 期限を予約する。起点は呼び出しの直前。切れたら `expire` で失敗にし、小窓を閉じて資源を解放する。
    fn arm(applier: &Applier<State>, id: u64, gen: u64, kind: Kind) -> bool {
        let a = applier.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let Some(g) = take_pending(id, kind) else {
                return;
            };
            a.apply(move |s| expire(s, g));
            teardown_media(id);
        });
        let handle = web_sys::window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                START_DEADLINE.as_millis() as i32,
            )
            .ok()
        });
        let Some(handle) = handle else {
            return false;
        };
        with(id, |e| {
            e.pending = Some(Pending {
                generation: gen,
                kind,
                timer: (handle, cb),
            })
        })
        .is_some()
    }

    /// 要求を失敗にして解放する（発送前の失敗用）。
    fn fail_now(applier: &Applier<State>, id: u64, gen: u64, f: PipShowFailure) {
        applier.apply(move |s| apply_update(s, gen, PipShowUpdate::Failed(f)));
        teardown_media(id);
    }

    /// マウントごとに 1 度だけ Entry・`pagehide` を登録し、画面を開いたときの自動準備を要求する。
    fn install(applier: &Applier<State>, id: u64) {
        let fresh = ENTRIES.with(|e| match e.borrow_mut().entry(id) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Entry::default());
                true
            }
            _ => false,
        });
        if !fresh {
            return;
        }
        if let Some(win) = web_sys::window() {
            let a = applier.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                exit_if_ours(id);
                a.apply(|s| {
                    let g = s.generation;
                    apply_update(s, g, PipShowUpdate::Phase(PipShowPhase::Exited));
                });
            });
            listen(id, true, win.into(), "pagehide", cb);
        }
        applier.apply(prepare);
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない（再描画ループ防止）。
    pub fn pip_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        install(applier, id);
        let Some((cmd, gen)) = take_command(state) else {
            return;
        };
        let fresh = with(id, |e| {
            let fresh = e.started != Some(gen);
            e.started = Some(gen);
            fresh
        })
        .unwrap_or(false);
        if !fresh {
            return;
        }
        applier.apply(move |s| mark_command_taken(s, gen));
        match cmd {
            PipCommand::Prepare => do_prepare(applier, id, gen),
            PipCommand::Start => do_start(applier, id, gen),
            PipCommand::Stop => do_stop(applier, id, gen),
        }
    }

    fn do_prepare(applier: &Applier<State>, id: u64, gen: u64) {
        // 前回の失敗で残った資源があれば先に解放する。
        teardown_media(id);
        let Some(doc) = document() else {
            return fail_now(applier, id, gen, PipShowFailure::Unsupported);
        };
        let canvas = doc
            .create_element("canvas")
            .ok()
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok());
        let video = doc
            .create_element("video")
            .ok()
            .and_then(|e| e.dyn_into::<HtmlVideoElement>().ok());
        let (Some(canvas), Some(video)) = (canvas, video) else {
            return fail_now(applier, id, gen, PipShowFailure::Unsupported);
        };
        let enabled =
            get(&JsValue::from(doc.clone()), "pictureInPictureEnabled").as_bool() == Some(true);
        let has_request = get(&video, "requestPictureInPicture").is_function();
        let has_capture = get(&canvas, "captureStream").is_function();
        if !decide_prepare(enabled, has_request, has_capture) {
            return fail_now(applier, id, gen, PipShowFailure::Unsupported);
        }
        canvas.set_width(FRAME_WIDTH);
        canvas.set_height(FRAME_HEIGHT);
        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok());
        let stream = canvas.capture_stream().ok();
        let (Some(ctx), Some(stream), Some(body)) = (ctx, stream, doc.body()) else {
            return fail_now(applier, id, gen, rejected());
        };
        if !arm(applier, id, gen, Kind::Prepare) {
            return fail_now(applier, id, gen, rejected());
        }
        let t0 = now_ms();
        paint_and_report(applier, &ctx, color_at(0.0));

        video.set_muted(true);
        let _ = video.set_attribute("playsinline", "");
        let _ = video.set_attribute("aria-hidden", "true");
        let _ = video.set_attribute("style", VIDEO_STYLE);
        video.set_src_object(Some(&stream));
        if body.append_child(&video).is_err() {
            video.set_src_object(None);
            stream_stop(&stream);
            return fail_now(applier, id, gen, rejected());
        }
        with(id, |e| {
            e.video = Some(video.clone());
            e.stream = Some(stream);
        });

        let et: EventTarget = video.clone().into();
        let a = applier.clone();
        let on_meta = Closure::<dyn FnMut()>::new(move || {
            settle(
                &a,
                id,
                Kind::Prepare,
                PipShowUpdate::Phase(PipShowPhase::Ready),
            )
        });
        listen(id, false, et.clone(), "loadedmetadata", on_meta);
        let a = applier.clone();
        let on_enter = Closure::<dyn FnMut()>::new(move || {
            settle(
                &a,
                id,
                Kind::Start,
                PipShowUpdate::Phase(PipShowPhase::Active),
            )
        });
        listen(id, false, et.clone(), "enterpictureinpicture", on_enter);
        let a = applier.clone();
        let on_leave = Closure::<dyn FnMut()>::new(move || {
            // 遷移中でも小窓中でも失敗ではなく「戻った」。保留中の Start / Stop は確定させる。
            for k in [Kind::Start, Kind::Stop] {
                let _ = take_pending(id, k);
            }
            a.apply(|s| {
                let g = s.generation;
                apply_update(s, g, PipShowUpdate::Phase(PipShowPhase::Exited));
            });
        });
        listen(id, false, et, "leavepictureinpicture", on_leave);

        // 色の巡回。ticks は POLL_INTERVAL、色が変わったときだけ塗り直して画面へ送る。
        let a = applier.clone();
        let mut last = color_at(0.0);
        let tick = Closure::<dyn FnMut()>::new(move || {
            let c = color_at(now_ms() - t0);
            if c != last {
                last = c;
                paint_and_report(&a, &ctx, c);
            }
        });
        let h = web_sys::window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                tick.as_ref().unchecked_ref(),
                POLL_INTERVAL.as_millis() as i32,
            )
            .ok()
        });
        match h {
            Some(h) => {
                with(id, |e| e.interval = Some((h, tick)));
            }
            None => {
                return settle(
                    applier,
                    id,
                    Kind::Prepare,
                    PipShowUpdate::Failed(rejected()),
                )
            }
        }

        // loadedmetadata が来ない環境向けに play() の解決でも Ready にする（二重確定は take_pending が防ぐ）。
        if let Ok(p) = video.play() {
            let a = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let res = JsFuture::from(p).await;
                if !is_live(id) || !is_current(id, Kind::Prepare, gen) {
                    return;
                }
                let upd = match res {
                    Ok(_) => PipShowUpdate::Phase(PipShowPhase::Ready),
                    Err(_) => PipShowUpdate::Failed(rejected()),
                };
                settle(&a, id, Kind::Prepare, upd);
            });
        } else {
            settle(
                applier,
                id,
                Kind::Prepare,
                PipShowUpdate::Failed(rejected()),
            );
        }
    }

    fn stream_stop(s: &web_sys::MediaStream) {
        for t in s.get_tracks().iter() {
            if let Ok(t) = t.dyn_into::<web_sys::MediaStreamTrack>() {
                t.stop();
            }
        }
    }

    fn paint_and_report(applier: &Applier<State>, ctx: &CanvasRenderingContext2d, c: [u8; 3]) {
        ctx.set_fill_style_str(&format!("rgb({},{},{})", c[0], c[1], c[2]));
        ctx.fill_rect(0.0, 0.0, FRAME_WIDTH as f64, FRAME_HEIGHT as f64);
        applier.apply(move |s| set_preview(s, c));
    }

    fn do_start(applier: &Applier<State>, id: u64, gen: u64) {
        let Some(video) = with(id, |e| e.video.clone()).flatten() else {
            return fail_now(applier, id, gen, rejected());
        };
        if !arm(applier, id, gen, Kind::Start) {
            return fail_now(applier, id, gen, rejected());
        }
        // transient activation を保つため、ここ（同期）で呼ぶ。
        match call(&video, "requestPictureInPicture") {
            Ok(r) => watch(applier, id, gen, Kind::Start, r),
            Err(e) => settle(
                applier,
                id,
                Kind::Start,
                PipShowUpdate::Failed(failure_from_name(&name_of(&e))),
            ),
        }
    }

    fn do_stop(applier: &Applier<State>, id: u64, gen: u64) {
        if !is_ours(id) {
            // 既に小窓ではない。何も待たず「戻った」にする。
            applier
                .apply(move |s| apply_update(s, gen, PipShowUpdate::Phase(PipShowPhase::Exited)));
            return;
        }
        if !arm(applier, id, gen, Kind::Stop) {
            return fail_now(applier, id, gen, rejected());
        }
        let doc = document().map(JsValue::from).unwrap_or(JsValue::UNDEFINED);
        match call(&doc, "exitPictureInPicture") {
            Ok(r) => watch(applier, id, gen, Kind::Stop, r),
            Err(e) => settle(
                applier,
                id,
                Kind::Stop,
                PipShowUpdate::Failed(failure_from_name(&name_of(&e))),
            ),
        }
    }

    /// Promise の結果を保留中の `kind` の確定へ流す。イベントが先に確定済みなら `settle` が捨てる。
    fn watch(applier: &Applier<State>, id: u64, gen: u64, kind: Kind, ret: JsValue) {
        let Ok(p) = ret.dyn_into::<Promise>() else {
            return;
        };
        let a = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let res = JsFuture::from(p).await;
            // 再開後も同じマウント・同じ世代かを確かめる（契約表 2）。
            if !is_live(id) || !is_current(id, kind, gen) {
                return;
            }
            let upd = match res {
                Ok(_) if kind == Kind::Start => PipShowUpdate::Phase(PipShowPhase::Active),
                Ok(_) => PipShowUpdate::Phase(PipShowPhase::Exited),
                Err(e) => PipShowUpdate::Failed(failure_from_name(&name_of(&e))),
            };
            settle(&a, id, kind, upd);
        });
    }

    pub(crate) fn release_mount(mount_id: u64) {
        teardown_media(mount_id);
        if let Some(entry) = ENTRIES.with(|e| e.borrow_mut().remove(&mount_id)) {
            remove_all(entry.page_listeners);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_mapping() {
        assert_eq!(
            failure_from_name("NotAllowedError"),
            PipShowFailure::NotAllowed
        );
        assert_eq!(
            failure_from_name("NotSupportedError"),
            PipShowFailure::Unsupported
        );
        assert_eq!(
            failure_from_name("InvalidStateError"),
            PipShowFailure::Failed(REJECTED.to_string())
        );
        assert_eq!(
            failure_from_name(""),
            PipShowFailure::Failed(REJECTED.to_string())
        );
    }

    #[test]
    fn prepare_needs_all_three() {
        assert!(decide_prepare(true, true, true));
        assert!(!decide_prepare(false, true, true));
        assert!(!decide_prepare(true, false, true));
        assert!(!decide_prepare(true, true, false));
    }

    #[test]
    fn color_follows_interval_and_survives_bad_clock() {
        use fandhe_device::picture_in_picture::FRAME_COLORS;
        assert_eq!(color_at(0.0), FRAME_COLORS[0]);
        assert_eq!(color_at(999.0), FRAME_COLORS[0]);
        assert_eq!(color_at(1000.0), FRAME_COLORS[1]);
        assert_eq!(color_at(5000.0), FRAME_COLORS[0]);
        assert_eq!(color_at(-5.0), FRAME_COLORS[0]);
        assert_eq!(color_at(f64::NAN), FRAME_COLORS[0]);
    }
}
