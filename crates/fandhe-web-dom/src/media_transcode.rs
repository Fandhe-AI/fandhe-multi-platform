//! 型サンプル「動画・音声の変換」の Web 経路（親 #91、Issue #315）。
//!
//! `fandhe_core::screens::media_transcode`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`take_request` で要求を観測し、実行時に作った 5 秒・モノラル・48 kHz の PCM
//! （`fandhe_device::media_playback::demo_pcm`）を WebCodecs の `AudioEncoder` で符号化し、
//! `AudioDecoder` で復号し直して検証する。`MediaTranscode` trait（`Send + Sync`）は `JsValue` を
//! 持てないため通らない。WebCodecs は web-sys の不安定 API（`web_sys_unstable_apis` が要る）なので、
//! ビルドフラグを増やさず `js_sys::Reflect` で呼ぶ。`fandhe-egui-proxy` の wasm canvas 経路は
//! 対象外（従来どおり `Unsupported`）。mux は作らないため、出力はコンテナなしの AAC か Opus になる。
//!
//! # 結果の種類
//!
//! - 成功: 出力が 0 バイトでなく、復号し直せて、復号したフレーム数から求めた長さが入力 ±200 ms の中
//! - 失敗: 非対応（secure context でない・`AudioEncoder` / `AudioDecoder` が無い・どちらの形式も
//!   符号化と復号の両方に対応しない）・期限切れ・出力が空・復号できない・長さの不一致・その他
//! - 中断: `pagehide`。失敗とは別で、`pageshow` で戻っても自動では再開しない。
//!   `visibilitychange` では中断しない（変換は短いため）
//!
//! # 期限
//!
//! 起点は effect での要求観測時の `performance.now()`（単調時計）。`TRANSCODE_DEADLINE` の
//! `setTimeout` で codec を閉じて `Timeout` を確定する。期限の後に届いた成功も `Timeout` にする。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（世代照合）、失敗・期限切れ・非対応を成功へ畳まないこと、
//! codec・`AudioData`・タイマー・リスナーの解放。
//! 守らない: ブラウザごとのコーデックの有無・ビットレート、複数タブ・複数マウント間の一致、
//! egui-proxy の wasm canvas 経路。例外の `message` は画面にも console にも出さない（security.md A09）。

use fandhe_core::screens::media_transcode::{TranscodeFailure, TranscodeFormat, TranscodeOutput};
use fandhe_device::media_transcode::{
    check_duration, input_duration, SAMPLE_RATE, TRANSCODE_DEADLINE,
};

/// 変換失敗の固定文言（`fandhe-egui-proxy` の `to_failure` と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED: &str = "変換できませんでした";

/// 試すコーデックの順（AAC → Opus）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CODEC_CANDIDATES: [(&str, TranscodeFormat); 2] = [
    ("mp4a.40.2", TranscodeFormat::AacRaw),
    ("opus", TranscodeFormat::OpusRaw),
];

/// 候補ごとの（encoder 対応, decoder 対応）から、両方対応の最初の添字を選ぶ。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn pick_codec(support: &[(bool, bool)]) -> Option<usize> {
    support.iter().position(|&(e, d)| e && d)
}

/// 符号化・復号の集計から結果を決める。空 → 復号不可 → 長さの不一致の順（嘘の成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome(
    format: TranscodeFormat,
    samples: usize,
    out_bytes: u64,
    frames: u64,
    decode_failed: bool,
) -> Result<TranscodeOutput, TranscodeFailure> {
    if out_bytes == 0 {
        return Err(TranscodeFailure::Empty);
    }
    if decode_failed || frames == 0 {
        return Err(TranscodeFailure::Unreadable);
    }
    let input = input_duration(samples, SAMPLE_RATE);
    let output = input_duration(frames as usize, SAMPLE_RATE);
    check_duration(output, input).map_err(|_| TranscodeFailure::Mismatch)?;
    Ok(TranscodeOutput {
        format,
        input_bytes: 44 + 2 * samples as u64,
        input_ms: input.as_millis() as u64,
        output_bytes: out_bytes,
        output_ms: output.as_millis() as u64,
    })
}

/// 開始から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    now_ms - started_ms >= TRANSCODE_DEADLINE.as_secs_f64() * 1000.0
}

/// 例外の `name` → 失敗。内部の文言は返さない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> TranscodeFailure {
    match name {
        Some("NotSupportedError") => TranscodeFailure::Unsupported,
        _ => TranscodeFailure::Failed(FAILED.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::media_transcode_effect;

/// マウント破棄時に codec・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::any::Any;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::media_transcode::{
        finish_transcode, interrupt_transcode, mark_running, take_request, TranscodeFailure,
        TranscodeOutput, TranscodeState,
    };
    use fandhe_device::media_playback::demo_pcm;
    use fandhe_device::media_transcode::{SAMPLE_RATE, TRANSCODE_DEADLINE};
    use js_sys::{Array, Function, Int16Array, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{failure_from_error_name, is_late, outcome, pick_codec, CODEC_CANDIDATES};
    use crate::mount::{is_live, Applier};

    type State = TranscodeState;

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
        frames: u64,
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

    fn construct(ctor: &JsValue, init: &JsValue) -> Result<JsValue, JsValue> {
        let f: Function = ctor.clone().dyn_into()?;
        Reflect::construct(&f, &Array::of1(init))
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
        r: Result<TranscodeOutput, TranscodeFailure>,
    ) {
        settle(applier, mount, g, move |s| finish_transcode(s, g, r));
    }

    /// 内部の打ち切り理由。`Gone` は要求がもう現在のものでないこと（何も書き戻さない）。
    enum Stop {
        Gone,
        Fail(TranscodeFailure),
    }

    fn alive(mount: u64, g: u64) -> Result<(), Stop> {
        with_live(mount, g, |_| ()).ok_or(Stop::Gone)
    }

    fn js_fail(e: JsValue) -> Stop {
        Stop::Fail(failure_from_error_name(err_name(&e).as_deref()))
    }

    fn fail(f: TranscodeFailure) -> Stop {
        Stop::Fail(f)
    }

    fn noop_error() -> Closure<dyn FnMut(JsValue)> {
        Closure::new(|_| {})
    }

    fn global(name: &str) -> JsValue {
        web_sys::window().map_or(JsValue::UNDEFINED, |w| get(&w, name))
    }

    async fn is_supported(ctor: &JsValue, codec: &str) -> bool {
        let cfg = obj(&[
            ("codec", codec.into()),
            ("sampleRate", SAMPLE_RATE.into()),
            ("numberOfChannels", 1.into()),
        ]);
        match wait(call(ctor, "isConfigSupported", &[cfg])).await {
            Ok(r) => get(&r, "supported").as_bool() == Some(true),
            Err(_) => false,
        }
    }

    /// 符号化 → 復号 → 判定。各 `await` の後に現在の要求かを確かめる。
    async fn run(mount: u64, g: u64) -> Result<TranscodeOutput, Stop> {
        let acc = Rc::new(RefCell::new(Acc::default()));
        let secure =
            web_sys::window().is_some_and(|w| get(&w, "isSecureContext").as_bool() == Some(true));
        let enc_ctor = global("AudioEncoder");
        let dec_ctor = global("AudioDecoder");
        if !secure || !enc_ctor.is_function() || !dec_ctor.is_function() {
            return Err(fail(TranscodeFailure::Unsupported));
        }
        let mut support = Vec::new();
        for (codec, _) in CODEC_CANDIDATES {
            let e = is_supported(&enc_ctor, codec).await;
            let d = is_supported(&dec_ctor, codec).await;
            alive(mount, g)?;
            support.push((e, d));
        }
        let idx = pick_codec(&support).ok_or(fail(TranscodeFailure::Unsupported))?;
        let (codec, format) = CODEC_CANDIDATES[idx];

        // 符号化。出力は集計だけ行い、バイト列は読まない（A09）。
        let pcm = demo_pcm(SAMPLE_RATE);
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
            &obj(&[
                ("output", on_out.as_ref().clone()),
                ("error", on_err.as_ref().clone()),
            ]),
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
                ("sampleRate", SAMPLE_RATE.into()),
                ("numberOfChannels", 1.into()),
            ])],
        )
        .map_err(js_fail)?;
        let data = obj(&[
            ("format", "s16".into()),
            ("sampleRate", SAMPLE_RATE.into()),
            ("numberOfFrames", (pcm.len() as u32).into()),
            ("numberOfChannels", 1.into()),
            ("timestamp", 0.into()),
            ("data", Int16Array::from(&pcm[..]).into()),
        ]);
        let audio = construct(&global("AudioData"), &data).map_err(js_fail)?;
        let enc_res = call(&enc, "encode", std::slice::from_ref(&audio));
        let _ = call(&audio, "close", &[]);
        enc_res.map_err(js_fail)?;
        wait(call(&enc, "flush", &[])).await.map_err(js_fail)?;
        alive(mount, g)?;

        // 復号し直して長さを測る。
        let (chunks, config, bytes) = {
            let a = acc.borrow();
            (a.chunks.clone(), a.config.clone(), a.bytes)
        };
        if bytes == 0 {
            return outcome(format, pcm.len(), 0, 0, false).map_err(fail);
        }
        let Some(config) = config else {
            return Err(fail(TranscodeFailure::Unreadable));
        };
        let a = acc.clone();
        let on_dec_out: Closure<dyn FnMut(JsValue)> = Closure::new(move |frame: JsValue| {
            let n = get(&frame, "numberOfFrames").as_f64().unwrap_or(0.0);
            if n.is_finite() && n > 0.0 {
                a.borrow_mut().frames += n as u64;
            }
            let _ = call(&frame, "close", &[]);
        });
        let on_dec_err = noop_error();
        let dec = construct(
            &dec_ctor,
            &obj(&[
                ("output", on_dec_out.as_ref().clone()),
                ("error", on_dec_err.as_ref().clone()),
            ]),
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
        let frames = acc.borrow().frames;
        outcome(format, pcm.len(), bytes, frames, decoded.is_err()).map_err(fail)
    }

    /// 画面の effect。未発送の要求を 1 度だけ発送する（同じ世代を二重に始めない）。
    pub fn media_transcode_effect(state: &State, applier: &Applier<State>) {
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
            conclude(&a_t, mount, g, Err(TranscodeFailure::Timeout));
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
                Ok(_) if is_late(started, now()) => Err(TranscodeFailure::Timeout),
                Ok(o) => Ok(o),
            };
            conclude(&a, mount, g, r);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: TranscodeFormat = TranscodeFormat::OpusRaw;
    const N: usize = 240_000;

    #[test]
    fn pick_first_fully_supported() {
        assert_eq!(pick_codec(&[(true, true), (true, true)]), Some(0));
        assert_eq!(pick_codec(&[(true, false), (true, true)]), Some(1));
        assert_eq!(pick_codec(&[(false, true), (true, false)]), None);
    }

    #[test]
    fn outcome_branches() {
        assert_eq!(
            outcome(F, N, 0, 240_000, false),
            Err(TranscodeFailure::Empty)
        );
        assert_eq!(
            outcome(F, N, 10, 0, false),
            Err(TranscodeFailure::Unreadable)
        );
        assert_eq!(
            outcome(F, N, 10, 240_000, true),
            Err(TranscodeFailure::Unreadable)
        );
        // 200 ms ちょうど（9600 フレーム）は成功、201 ms 分ずれると不一致。
        assert!(outcome(F, N, 10, 240_000 - 9_600, false).is_ok());
        assert_eq!(
            outcome(F, N, 10, 240_000 - 9_648, false),
            Err(TranscodeFailure::Mismatch)
        );
        let o = outcome(F, N, 3000, 240_000, false).unwrap();
        assert_eq!(
            (o.input_bytes, o.input_ms, o.output_bytes, o.output_ms),
            (480_044, 5000, 3000, 5000)
        );
    }

    #[test]
    fn late_boundary() {
        assert!(!is_late(100.0, 10_099.0));
        assert!(is_late(100.0, 10_100.0));
        assert!(!is_late(100.0, f64::NAN));
    }

    #[test]
    fn error_names_hide_messages() {
        assert_eq!(
            failure_from_error_name(Some("NotSupportedError")),
            TranscodeFailure::Unsupported
        );
        let f = TranscodeFailure::Failed(FAILED.to_string());
        assert_eq!(failure_from_error_name(Some("X")), f);
        assert_eq!(failure_from_error_name(None), f);
    }
}
