//! 型サンプル「動画の変換」の Web 経路（親 #340、Issue #1018）。
//!
//! `fandhe_core::screens::video_transcode`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`take_request` で要求を観測し、実行時に作った 1280×720・15 fps・30 フレーム
//! （`fandhe_device::video_transcode::fill_source_frame_bgra`）を WebCodecs の `VideoEncoder` で 640×360 へ
//! 符号化し、`VideoDecoder` で復号し直して `check_decoded` で検証する。縮小は符号化器に任せる
//! （仕様の MUST scale）。`VideoTranscode` trait（`Send + Sync`）は `JsValue` を持てないため通らない。
//! WebCodecs は web-sys の不安定 API なので `js_sys::Reflect` で呼ぶ（音声版 [`super::media_transcode`] と同じ）。
//! mux は作らないため、出力はコンテナなしの符号化チャンクになる。`fandhe-egui-proxy` の wasm canvas 経路は
//! 対象外（従来どおり `Unsupported`）。
//!
//! # 結果の種類
//!
//! - 成功: チャンクが 0 でなく、復号し直せて、表示寸法が 640×360 で元より小さく、長さが元 ±200 ms の中
//! - 失敗: 非対応（secure context でない・`VideoEncoder` / `VideoDecoder` / `VideoFrame` が無い・どの候補も
//!   符号化と復号の両方に対応しない）・期限切れ・出力が空・復号できない・寸法や長さの不一致・その他
//! - 中断: `pagehide`。失敗とは別で、`pageshow` で戻っても自動では再開しない
//!
//! # 候補の順
//!
//! `avc1.42001f`（H.264）→ `vp09.00.10.08`（VP9）→ `vp8`。H.264 を持たないビルドのブラウザでも試せるよう
//! VP9 / VP8 へ落とす。画面に出す codec は固定ラベルだけで、ブラウザ由来の文字列は出さない。
//!
//! # 期限
//!
//! 起点は effect での要求観測時の `performance.now()`（単調時計）。`TRANSCODE_DEADLINE` の
//! `setTimeout` で codec を閉じて `Timeout` を確定する。期限の後に届いた成功も `Timeout` にする。
//!
//! # 解放
//!
//! `VideoFrame`（入力・復号出力）は使い終えた直後に `close()` する。codec・タイマー・リスナーは
//! `teardown` / `settle` が全経路で解く。`close()` は同期で一時ファイルも無いため、Android / iOS の
//! ような「解放の未確認」状態（`RELEASE_PENDING`）は Web では起きない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（世代照合）、失敗・期限切れ・非対応を成功へ畳まないこと、
//! codec・`VideoFrame`・タイマー・リスナーの解放。
//! 守らない: ブラウザごとのコーデックの有無・ビットレート、複数タブ・複数マウント間の一致、
//! egui-proxy の wasm canvas 経路。例外の `message` と出力のバイト列は画面にも console にも出さない
//! （security.md A09）。

use fandhe_core::screens::video_transcode::{
    VideoContainer, VideoTranscodeFailure, VideoTranscodeOutput, GENERIC_FAILED_MESSAGE,
};
use fandhe_device::video_transcode::{
    check_decoded, source_duration, DecodedFrame, SOURCE_HEIGHT, SOURCE_WIDTH, TARGET_HEIGHT,
    TARGET_WIDTH, TRANSCODE_DEADLINE,
};
use fandhe_device::DeviceError;

/// 試す codec の順（`isConfigSupported` に渡す文字列, 画面に出すラベル）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CODEC_CANDIDATES: [(&str, &str); 3] = [
    ("avc1.42001f", "avc1"),
    ("vp09.00.10.08", "vp09"),
    ("vp8", "vp8"),
];

/// `check_decoded` のエラーを失敗種別へ写す（`fandhe-egui-proxy` の `to_failure` と同じ写像）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_failure(e: DeviceError) -> VideoTranscodeFailure {
    use fandhe_device::video_transcode::{EMPTY, MISMATCH, NOT_SMALLER, UNREADABLE};
    match e {
        DeviceError::Internal(EMPTY) => VideoTranscodeFailure::Empty,
        DeviceError::Internal(UNREADABLE) => VideoTranscodeFailure::Unreadable,
        DeviceError::Internal(NOT_SMALLER) => VideoTranscodeFailure::NotSmaller,
        DeviceError::Internal(MISMATCH) => VideoTranscodeFailure::Mismatch,
        DeviceError::Unsupported(_) => VideoTranscodeFailure::Unsupported,
        _ => VideoTranscodeFailure::Failed(GENERIC_FAILED_MESSAGE.to_string()),
    }
}

/// 符号化・復号の集計から結果を決める。空 → 復号不可 → 寸法・長さの順（嘘の成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome(
    label: &str,
    chunks: usize,
    bytes: u64,
    frames: &[DecodedFrame],
    decode_failed: bool,
) -> Result<VideoTranscodeOutput, VideoTranscodeFailure> {
    let chunks = if bytes == 0 { 0 } else { chunks };
    if chunks > 0 && decode_failed {
        return Err(VideoTranscodeFailure::Unreadable);
    }
    let len = check_decoded(chunks, frames, TARGET_WIDTH, TARGET_HEIGHT).map_err(to_failure)?;
    // check_decoded が非空・全フレーム同寸法を確かめ済み。
    let first = frames[0];
    Ok(VideoTranscodeOutput {
        container: VideoContainer::Raw,
        codec: label.to_string(),
        source_width: SOURCE_WIDTH,
        source_height: SOURCE_HEIGHT,
        source_ms: source_duration().as_millis() as u64,
        output_width: first.width,
        output_height: first.height,
        output_ms: len.as_millis() as u64,
        output_bytes: bytes,
    })
}

/// 復号フレームの表示寸法と時刻を検証して `DecodedFrame` にする。有限でない・負・範囲外は `None`
/// （丸めの前に範囲を確かめる。契約表 5）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decoded_frame(w: f64, h: f64, ts: f64) -> Option<DecodedFrame> {
    let ok = |v: f64, max: f64| v.is_finite() && (0.0..=max).contains(&v);
    // ts は 2^53 まで（f64 で厳密に表せる整数の上限）。
    if ok(w, f64::from(u32::MAX)) && ok(h, f64::from(u32::MAX)) && ok(ts, 9_007_199_254_740_992.0) {
        Some(DecodedFrame {
            width: w as u32,
            height: h as u32,
            timestamp_us: ts as u64,
        })
    } else {
        None
    }
}

/// 開始から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    now_ms - started_ms >= TRANSCODE_DEADLINE.as_secs_f64() * 1000.0
}

/// 例外の `name` → 失敗。内部の文言は返さない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> VideoTranscodeFailure {
    match name {
        Some("NotSupportedError") => VideoTranscodeFailure::Unsupported,
        _ => VideoTranscodeFailure::Failed(GENERIC_FAILED_MESSAGE.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::video_transcode_effect;

/// マウント破棄時に codec・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::any::Any;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::video_transcode::{
        finish_transcode, interrupt_transcode, mark_running, take_request, VideoTranscodeFailure,
        VideoTranscodeOutput, VideoTranscodeState, GENERIC_FAILED_MESSAGE,
    };
    use fandhe_device::video_transcode::{
        fill_source_frame_bgra, DecodedFrame, SOURCE_FPS, SOURCE_FRAMES, SOURCE_HEIGHT,
        SOURCE_WIDTH, TARGET_HEIGHT, TARGET_WIDTH, TRANSCODE_DEADLINE,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{decoded_frame, failure_from_error_name, is_late, outcome, CODEC_CANDIDATES};
    use crate::media_transcode::pick_codec;
    use crate::mount::{is_live, Applier};

    type State = VideoTranscodeState;

    /// マウントごとの変換 1 件。`teardown` で解除してから drop する。
    struct Session {
        gen: u64,
        /// 結果（成功・失敗・中断・期限切れ）を確定済みか。以後の通知はすべて捨てる。
        finished: bool,
        timer_id: Option<i32>,
        /// タイマー発火中に drop しないよう、`teardown` まで保持する。
        _timer_cb: Closure<dyn FnMut()>,
        on_pagehide: Closure<dyn FnMut()>,
        encoder: Option<JsValue>,
        decoder: Option<JsValue>,
        /// codec のコールバック。codec を閉じてから drop する。
        _keep: Vec<Box<dyn Any>>,
    }

    /// codec の出力の集計。
    #[derive(Default)]
    struct Acc {
        bytes: u64,
        chunks: Vec<JsValue>,
        config: Option<JsValue>,
        frames: Vec<DecodedFrame>,
        /// 復号フレームの寸法・時刻が範囲外だった。
        bad_frame: bool,
    }

    thread_local! {
        static SESSIONS: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    fn get(o: &JsValue, k: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
    }

    fn obj(fields: &[(&str, JsValue)]) -> JsValue {
        let o = Object::new();
        for (k, v) in fields {
            let _ = Reflect::set(&o, &JsValue::from_str(k), v);
        }
        o.into()
    }

    /// 例外の `name` だけを読む（`message` は使わない）。
    fn err_name(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    fn call(o: &JsValue, m: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(o, m).dyn_into()?;
        let a = Array::new();
        for v in args {
            a.push(v);
        }
        Reflect::apply(&f, o, &a)
    }

    async fn wait(v: Result<JsValue, JsValue>) -> Result<JsValue, JsValue> {
        JsFuture::from(Promise::resolve(&v?)).await
    }

    fn construct(ctor: &JsValue, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = ctor.clone().dyn_into()?;
        let a = Array::new();
        for v in args {
            a.push(v);
        }
        Reflect::construct(&f, &a)
    }

    /// 閉じていなければ閉じる。例外は握りつぶす（後始末のため）。
    fn close_codec(c: &JsValue) {
        if get(c, "state").as_string().as_deref() != Some("closed") {
            let _ = call(c, "close", &[]);
        }
    }

    fn release_codecs(s: &mut Session) {
        for c in [s.encoder.take(), s.decoder.take()].iter().flatten() {
            close_codec(c);
        }
    }

    /// 解除を済ませてから Closure を drop する（契約表 9）。
    fn teardown(mut s: Session) {
        let w = web_sys::window();
        if let (Some(w), Some(id)) = (&w, s.timer_id.take()) {
            w.clear_timeout_with_handle(id);
        }
        release_codecs(&mut s);
        if let Some(w) = w {
            let _ = w.remove_event_listener_with_callback(
                "pagehide",
                s.on_pagehide.as_ref().unchecked_ref(),
            );
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        if let Some(s) = SESSIONS.with(|m| m.borrow_mut().remove(&mount)) {
            teardown(s);
        }
    }

    /// 通知を非同期に書き戻す。
    fn post(applier: &Applier<State>, f: impl FnOnce(&mut State) + 'static) {
        let a = applier.clone();
        spawn_local(async move { a.apply(f) });
    }

    /// マウントが有効で、世代が一致し、未確定のときだけ `f` を実行する（契約表 2）。
    fn with_live<R>(mount: u64, g: u64, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        if !is_live(mount) {
            return None;
        }
        SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            m.get_mut(&mount)
                .filter(|s| s.gen == g && !s.finished)
                .map(f)
        })
    }

    /// 確定の共通処理。確定済みにし、タイマーを止め、codec を閉じてから `after` を書き戻す。
    fn settle(
        applier: &Applier<State>,
        mount: u64,
        g: u64,
        after: impl FnOnce(&mut State) + 'static,
    ) {
        let hit = with_live(mount, g, |s| {
            s.finished = true;
            if let (Some(w), Some(id)) = (web_sys::window(), s.timer_id.take()) {
                w.clear_timeout_with_handle(id);
            }
            release_codecs(s);
        });
        if hit.is_some() {
            post(applier, after);
            // 実行中の Closure（codec・タイマーのコールバック）から呼ばれうるため、
            // 抜けた後（次のマイクロタスク）に pagehide 解除・コールバックと集計の破棄を行う（契約表 3・9）。
            spawn_local(async move {
                let done = SESSIONS.with(|m| {
                    let mut m = m.borrow_mut();
                    if m.get(&mount).is_some_and(|s| s.gen == g && s.finished) {
                        m.remove(&mount)
                    } else {
                        None
                    }
                });
                if let Some(s) = done {
                    teardown(s);
                }
            });
        }
    }

    fn conclude(
        applier: &Applier<State>,
        mount: u64,
        g: u64,
        r: Result<VideoTranscodeOutput, VideoTranscodeFailure>,
    ) {
        settle(applier, mount, g, move |s| finish_transcode(s, g, r));
    }

    /// 内部の打ち切り理由。`Gone` は要求がもう現在のものでないこと（何も書き戻さない）。
    enum Stop {
        Gone,
        Fail(VideoTranscodeFailure),
    }

    fn alive(mount: u64, g: u64) -> Result<(), Stop> {
        with_live(mount, g, |_| ()).ok_or(Stop::Gone)
    }

    fn js_fail(e: JsValue) -> Stop {
        Stop::Fail(failure_from_error_name(err_name(&e).as_deref()))
    }

    fn fail(f: VideoTranscodeFailure) -> Stop {
        Stop::Fail(f)
    }

    fn noop_error() -> Closure<dyn FnMut(JsValue)> {
        Closure::new(|_| {})
    }

    fn global(name: &str) -> JsValue {
        web_sys::window().map_or(JsValue::UNDEFINED, |w| get(&w, name))
    }

    async fn is_supported(ctor: &JsValue, cfg: JsValue) -> bool {
        match wait(call(ctor, "isConfigSupported", &[cfg])).await {
            Ok(r) => get(&r, "supported").as_bool() == Some(true),
            Err(_) => false,
        }
    }

    /// 符号化 → 復号 → 判定。各 `await` の後に現在の要求かを確かめる。
    async fn run(mount: u64, g: u64) -> Result<VideoTranscodeOutput, Stop> {
        let acc = Rc::new(RefCell::new(Acc::default()));
        let secure =
            web_sys::window().is_some_and(|w| get(&w, "isSecureContext").as_bool() == Some(true));
        let enc_ctor = global("VideoEncoder");
        let dec_ctor = global("VideoDecoder");
        let frame_ctor = global("VideoFrame");
        if !secure
            || !enc_ctor.is_function()
            || !dec_ctor.is_function()
            || !frame_ctor.is_function()
        {
            return Err(fail(VideoTranscodeFailure::Unsupported));
        }
        let mut support = Vec::new();
        for (codec, _) in CODEC_CANDIDATES {
            let e = is_supported(
                &enc_ctor,
                obj(&[
                    ("codec", codec.into()),
                    ("width", TARGET_WIDTH.into()),
                    ("height", TARGET_HEIGHT.into()),
                    ("framerate", SOURCE_FPS.into()),
                ]),
            )
            .await;
            let d = is_supported(
                &dec_ctor,
                obj(&[
                    ("codec", codec.into()),
                    ("codedWidth", TARGET_WIDTH.into()),
                    ("codedHeight", TARGET_HEIGHT.into()),
                ]),
            )
            .await;
            alive(mount, g)?;
            support.push((e, d));
        }
        let idx = pick_codec(&support).ok_or(fail(VideoTranscodeFailure::Unsupported))?;
        let (codec, label) = CODEC_CANDIDATES[idx];

        // 符号化。出力は集計だけ行い、バイト列は読まない（A09）。
        let a = acc.clone();
        let on_out: Closure<dyn FnMut(JsValue, JsValue)> = Closure::new(move |chunk, meta| {
            let mut a = a.borrow_mut();
            let n = get(&chunk, "byteLength").as_f64().unwrap_or(0.0);
            if n.is_finite() && n > 0.0 {
                a.bytes += n as u64;
            }
            a.chunks.push(chunk);
            let c = get(&meta, "decoderConfig");
            if a.config.is_none() && c.is_object() {
                a.config = Some(c);
            }
        });
        let on_err = noop_error();
        let enc = construct(
            &enc_ctor,
            &[obj(&[
                ("output", on_out.as_ref().clone()),
                ("error", on_err.as_ref().clone()),
            ])],
        )
        .map_err(js_fail)?;
        // 作った codec は即 Session へ渡し、以後の解放を teardown / settle に任せる。
        let kept = with_live(mount, g, |s| {
            s.encoder = Some(enc.clone());
            s._keep.push(Box::new(on_out));
            s._keep.push(Box::new(on_err));
        });
        if kept.is_none() {
            close_codec(&enc);
            return Err(Stop::Gone);
        }
        call(
            &enc,
            "configure",
            &[obj(&[
                ("codec", codec.into()),
                ("width", TARGET_WIDTH.into()),
                ("height", TARGET_HEIGHT.into()),
                ("framerate", SOURCE_FPS.into()),
            ])],
        )
        .map_err(js_fail)?;
        // 1280x720 BGRA を 1 本のバッファで使い回す（約 3.6 MB）。
        let stride = SOURCE_WIDTH as usize * 4;
        let mut buf = vec![0u8; stride * SOURCE_HEIGHT as usize];
        let frame_us = 1_000_000.0 / f64::from(SOURCE_FPS);
        for i in 0..SOURCE_FRAMES {
            if !fill_source_frame_bgra(i, SOURCE_WIDTH, SOURCE_HEIGHT, stride, &mut buf) {
                return Err(fail(VideoTranscodeFailure::Failed(
                    GENERIC_FAILED_MESSAGE.to_string(),
                )));
            }
            let init = obj(&[
                ("format", "BGRA".into()),
                ("codedWidth", SOURCE_WIDTH.into()),
                ("codedHeight", SOURCE_HEIGHT.into()),
                ("timestamp", (f64::from(i) * frame_us).floor().into()),
                ("duration", frame_us.floor().into()),
            ]);
            let frame = construct(&frame_ctor, &[Uint8Array::from(&buf[..]).into(), init])
                .map_err(js_fail)?;
            let enc_res = call(&enc, "encode", std::slice::from_ref(&frame));
            // encode の成否にかかわらず入力フレームを解放する（契約表 9）。
            let _ = call(&frame, "close", &[]);
            enc_res.map_err(js_fail)?;
        }
        wait(call(&enc, "flush", &[])).await.map_err(js_fail)?;
        alive(mount, g)?;

        // 復号し直して寸法と長さを測る。
        let (chunks, config, bytes) = {
            let a = acc.borrow();
            (a.chunks.clone(), a.config.clone(), a.bytes)
        };
        if bytes == 0 {
            return outcome(label, chunks.len(), 0, &[], false).map_err(fail);
        }
        let Some(config) = config else {
            return Err(fail(VideoTranscodeFailure::Unreadable));
        };
        let a = acc.clone();
        let on_dec_out: Closure<dyn FnMut(JsValue)> = Closure::new(move |frame: JsValue| {
            let n = |k| get(&frame, k).as_f64().unwrap_or(f64::NAN);
            let f = decoded_frame(n("displayWidth"), n("displayHeight"), n("timestamp"));
            let _ = call(&frame, "close", &[]);
            let mut a = a.borrow_mut();
            match f {
                Some(f) => a.frames.push(f),
                None => a.bad_frame = true,
            }
        });
        let on_dec_err = noop_error();
        let dec = construct(
            &dec_ctor,
            &[obj(&[
                ("output", on_dec_out.as_ref().clone()),
                ("error", on_dec_err.as_ref().clone()),
            ])],
        )
        .map_err(js_fail)?;
        let kept = with_live(mount, g, |s| {
            s.decoder = Some(dec.clone());
            s._keep.push(Box::new(on_dec_out));
            s._keep.push(Box::new(on_dec_err));
        });
        if kept.is_none() {
            close_codec(&dec);
            return Err(Stop::Gone);
        }
        let decoded = async {
            call(&dec, "configure", &[config])?;
            for c in &chunks {
                call(&dec, "decode", std::slice::from_ref(c))?;
            }
            wait(call(&dec, "flush", &[])).await
        }
        .await;
        alive(mount, g)?;
        let (frames, bad) = {
            let a = acc.borrow();
            (a.frames.clone(), a.bad_frame)
        };
        outcome(label, chunks.len(), bytes, &frames, decoded.is_err() || bad).map_err(fail)
    }

    /// 画面の effect。未発送の要求を 1 度だけ発送する（同じ世代を二重に始めない）。
    pub fn video_transcode_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        let Some(g) = take_request(state) else { return };
        if SESSIONS.with(|m| m.borrow().get(&mount).is_some_and(|s| s.gen == g)) {
            return;
        }
        if let Some(old) = SESSIONS.with(|m| m.borrow_mut().remove(&mount)) {
            teardown(old);
        }
        let started = now();
        let (a_t, a_p) = (applier.clone(), applier.clone());
        let timer_cb: Closure<dyn FnMut()> = Closure::new(move || {
            conclude(&a_t, mount, g, Err(VideoTranscodeFailure::Timeout));
        });
        // 中断は失敗ではない。復帰しても自動では再開しない。
        let on_pagehide: Closure<dyn FnMut()> = Closure::new(move || {
            settle(&a_p, mount, g, move |s| interrupt_transcode(s, g));
        });
        let mut timer_id = None;
        if let Some(w) = web_sys::window() {
            // リスナーは要求を発送する前に付ける（契約表 1）。
            let _ = w
                .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref());
            timer_id = w
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    timer_cb.as_ref().unchecked_ref(),
                    TRANSCODE_DEADLINE.as_millis() as i32,
                )
                .ok();
        }
        SESSIONS.with(|m| {
            m.borrow_mut().insert(
                mount,
                Session {
                    gen: g,
                    finished: false,
                    timer_id,
                    _timer_cb: timer_cb,
                    on_pagehide,
                    encoder: None,
                    decoder: None,
                    _keep: Vec::new(),
                },
            );
        });
        applier.apply(mark_running);
        let a = applier.clone();
        spawn_local(async move {
            let r = match run(mount, g).await {
                Err(Stop::Gone) => return,
                Err(Stop::Fail(f)) => Err(f),
                // 期限の後に届いた成功は成功にしない（契約表 3）。
                Ok(_) if is_late(started, now()) => Err(VideoTranscodeFailure::Timeout),
                Ok(o) => Ok(o),
            };
            conclude(&a, mount, g, r);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(ts_ms: u64, w: u32, h: u32) -> DecodedFrame {
        DecodedFrame {
            width: w,
            height: h,
            timestamp_us: ts_ms * 1000,
        }
    }

    /// 15 fps・30 フレームの復号結果。
    fn good(w: u32, h: u32) -> Vec<DecodedFrame> {
        (0..30).map(|i| fr(i * 1000 / 15, w, h)).collect()
    }

    #[test]
    fn outcome_branches() {
        let g = good(640, 360);
        assert_eq!(
            outcome("vp8", 30, 0, &g, false),
            Err(VideoTranscodeFailure::Empty)
        );
        assert_eq!(
            outcome("vp8", 30, 9, &g, true),
            Err(VideoTranscodeFailure::Unreadable)
        );
        assert_eq!(
            outcome("vp8", 30, 9, &[], false),
            Err(VideoTranscodeFailure::Unreadable)
        );
        assert_eq!(
            outcome("vp8", 30, 9, &good(1280, 720), false),
            Err(VideoTranscodeFailure::NotSmaller)
        );
        assert_eq!(
            outcome("vp8", 30, 9, &good(320, 180), false),
            Err(VideoTranscodeFailure::Unreadable)
        );
        let half: Vec<_> = g.iter().take(15).copied().collect();
        assert_eq!(
            outcome("vp8", 30, 9, &half, false),
            Err(VideoTranscodeFailure::Mismatch)
        );
        let o = outcome("vp09", 30, 1234, &g, false).unwrap();
        assert_eq!(o.container, VideoContainer::Raw);
        assert_eq!(o.codec, "vp09");
        assert_eq!(
            (o.source_width, o.source_height, o.source_ms),
            (1280, 720, 2000)
        );
        assert_eq!((o.output_width, o.output_height), (640, 360));
        assert_eq!(o.output_bytes, 1234);
        assert!(o.output_ms.abs_diff(2000) <= 200);
    }

    #[test]
    fn decoded_frame_rejects_bad_values() {
        assert!(decoded_frame(640.0, 360.0, 66_666.0).is_some());
        assert!(decoded_frame(f64::NAN, 360.0, 0.0).is_none());
        assert!(decoded_frame(-1.0, 360.0, 0.0).is_none());
        assert!(decoded_frame(640.0, 5e9, 0.0).is_none());
        assert!(decoded_frame(640.0, 360.0, f64::INFINITY).is_none());
        assert!(decoded_frame(640.0, 360.0, 1e19).is_none());
    }

    #[test]
    fn late_boundary() {
        assert!(!is_late(100.0, 20_099.9));
        assert!(is_late(100.0, 20_100.0));
        assert!(!is_late(100.0, f64::NAN));
    }

    #[test]
    fn error_names_hide_messages() {
        assert_eq!(
            failure_from_error_name(Some("NotSupportedError")),
            VideoTranscodeFailure::Unsupported
        );
        let f = VideoTranscodeFailure::Failed(GENERIC_FAILED_MESSAGE.to_string());
        assert_eq!(failure_from_error_name(Some("X")), f);
        assert_eq!(failure_from_error_name(None), f);
    }
}
