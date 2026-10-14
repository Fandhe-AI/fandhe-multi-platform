//! Phase 11 の拡充ループ（親 #1135）型サンプル「画面の録画・キャプチャ」の Web 経路（Issue #1338）。
//!
//! `fandhe_core::screens::screen_capture`（端末依存ゼロの状態機械）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `navigator.mediaDevices.getDisplayMedia()`（映像のみ）+ `MediaRecorder` で録画して
//! 結果を書き戻す。構造は `mic_record.rs` と同型（Session / `ACTIVE` / 世代照合 / `release`）。
//!
//! Web は `fandhe-device` の `ScreenCapturer` トレイトを通らない（`mic_record.rs` と同じ理由）。
//! ただし結果の不変条件（0 バイト・0 ミリ秒は失敗）だけは
//! [`fandhe_device::screen_capture::build_capture_result`] を SSOT として共有する。
//! 映像のバイト列は読まない。使うのは `Blob.size` だけ（契約の「映像は読み込まない」）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 1 マウント内の順序と世代の照合、0 バイト・0 ミリ秒を成功にしないこと、
//!   全終了経路でのトラック・タイマー・ハンドラの解放、毎回の同意、
//!   映像の中身とソース名（トラックのラベル）をログに出さないこと
//! - 守らない: 映像の中身、共有選択 UI の見た目と選ばれたソースの種類、
//!   `NotAllowedError` の内訳（利用者のキャンセル・システム拒否・Permissions-Policy は
//!   すべてキャンセル扱い）、複数タブ、非表示タブでのタイマー間引きによる表示の遅れ、
//!   bfcache からの再開、iOS Safari（API が無いため `Unsupported`）
//!
//! # 既知の天井
//!
//! `mount_with_effect` は状態から `Screen` を組み直さないため、検証エントリでは「停止」
//! ボタンが出ない（`mic_record.rs` と同じ）。録画は 10 秒の自動停止か、ブラウザの
//! 「共有を停止」（映像トラックの `ended`。契約上は成功）で終わる。
//!
//! # ジェスチャー
//!
//! `getDisplayMedia()` は transient user activation を要する。effect はクリックハンドラの
//! 再描画直後に同期で呼ばれるため、await を挟まず同期で呼ぶ。
//!
//! # ログ（security.md A09）
//!
//! 例外は `name` だけを読む。`message`・トラックのラベル・映像の中身は出力も保存もしない。

use fandhe_core::screens::screen_capture::{CaptureOutcome, CaptureSummary};
use fandhe_device::screen_capture::build_capture_result;

#[cfg_attr(not(target_arch = "wasm32"), allow(unused_imports))]
use super::mic_record::resolve_mime;

/// 種別を特定できない失敗の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CAPTURE_FAILED: &str = "録画に失敗しました";
/// `pagehide` で録画中に破棄したときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const INTERRUPTED: &str = "録画が中断されました";

/// 例外の `name` → 結果への変換純関数（native からテストできる）。
///
/// `cancel_requested`（共有選択中に停止が押された）は例外の種類に関わらず `Cancelled`。
/// `NotAllowedError` は共有選択を閉じた・拒否した場合でキャンセル（契約の三値）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(
    name: Option<&str>,
    cancel_requested: bool,
) -> Result<CaptureOutcome, String> {
    if cancel_requested {
        return Ok(CaptureOutcome::Cancelled);
    }
    match name {
        Some("NotAllowedError") | Some("PermissionDeniedError") => Ok(CaptureOutcome::Cancelled),
        Some("InvalidStateError") => {
            Err(fandhe_core::screens::screen_capture::NEEDS_USER_GESTURE_TEXT.to_string())
        }
        Some("NotSupportedError") | Some("TypeError") => Ok(CaptureOutcome::Unsupported),
        _ => Err(CAPTURE_FAILED.to_string()),
    }
}

/// `Blob.size`・mime・録画時間 → 結果の純関数。検証は `build_capture_result` の SSOT に任せ、
/// 内部識別子は画面へ出さず固定文言へ畳む。NaN・負値・非有限のサイズは 0 扱い。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_blob(
    size: f64,
    mime: String,
    duration_ms: u64,
) -> Result<CaptureOutcome, String> {
    let size_bytes = if size.is_finite() && size > 0.0 {
        size as u64
    } else {
        0
    };
    let r = build_capture_result(size_bytes, mime, duration_ms)
        .map_err(|_| CAPTURE_FAILED.to_string())?;
    Ok(CaptureOutcome::Captured(CaptureSummary {
        size_bytes: r.size_bytes,
        mime: r.mime,
        duration_ms: r.duration_ms,
    }))
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::screen_capture::{
        finish_capture, mark_recording, mark_stopping, take_start_request, take_stop_request, tick,
        CaptureOutcome, ScreenCaptureState, MAX_CAPTURE_MS,
    };
    use js_sys::{Array, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{MediaRecorder, MediaStream, MediaStreamTrack, RecordingState};

    use super::{
        outcome_from_blob, outcome_from_error_name, resolve_mime, CAPTURE_FAILED, INTERRUPTED,
    };
    use crate::mount::Applier;

    /// 1 回の録画セッション。全終了経路が [`release`] を通して解放する。
    struct Session {
        applier: Applier<ScreenCaptureState>,
        generation: u64,
        /// 共有選択中（recorder 未生成）に停止が押された。解決後に `Cancelled` へ畳む。
        cancel_requested: bool,
        stream: Option<MediaStream>,
        recorder: Option<MediaRecorder>,
        chunks: Array,
        /// `performance.now()` の開始時刻。時計の巻き戻しで duration が 0 になるのを避ける。
        started_at_ms: Option<f64>,
        timeout_id: Option<i32>,
        interval_id: Option<i32>,
        // Drop されるまで JS から呼べる状態を保つためだけに保持する（`forget()` はしない）。
        #[allow(dead_code)]
        closures: Vec<Closure<dyn FnMut(JsValue)>>,
        #[allow(dead_code)]
        timers: Vec<Closure<dyn FnMut()>>,
    }

    thread_local! {
        static ACTIVE: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    fn now_ms() -> f64 {
        window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or_else(js_sys::Date::now)
    }

    fn stop_tracks(stream: &MediaStream) {
        for track in stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.set_onended(None);
                track.stop();
            }
        }
    }

    fn error_name(err: &JsValue) -> Option<String> {
        Reflect::get(err, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
    }

    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    fn matches_current(mount_id: u64, generation: u64) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .is_some_and(|s| s.generation == generation)
        })
    }

    fn cancel_requested(mount_id: u64) -> bool {
        ACTIVE.with(|a| {
            a.borrow()
                .get(&mount_id)
                .is_some_and(|s| s.cancel_requested)
        })
    }

    /// セッションを解放する（タイマー解除 → ハンドラ解除 → 全トラック停止で共有表示を消す）。
    fn release(mount_id: u64) {
        let Some(session) = ACTIVE.with(|active| active.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(w) = window() {
            if let Some(id) = session.timeout_id {
                w.clear_timeout_with_handle(id);
            }
            if let Some(id) = session.interval_id {
                w.clear_interval_with_handle(id);
            }
        }
        if let Some(recorder) = &session.recorder {
            recorder.set_ondataavailable(None);
            recorder.set_onstop(None);
            recorder.set_onerror(None);
        }
        if let Some(stream) = &session.stream {
            stop_tracks(stream);
        }
    }

    /// `sweep_mounts` から呼ばれる。マウント破棄済みなので結果は届けず解放だけする。
    pub(crate) fn release_mount(mount_id: u64) {
        release(mount_id);
    }

    /// 世代を再照合して解放し、結果を届ける（状態を確定させてから届ける）。
    fn finish(
        applier: &Applier<ScreenCaptureState>,
        mount_id: u64,
        generation: u64,
        result: Result<CaptureOutcome, String>,
    ) {
        if !matches_current(mount_id, generation) {
            return;
        }
        release(mount_id);
        applier.apply(move |s| finish_capture(s, generation, result));
    }

    fn fail(applier: &Applier<ScreenCaptureState>, mount_id: u64, generation: u64) {
        finish(
            applier,
            mount_id,
            generation,
            Err(CAPTURE_FAILED.to_string()),
        );
    }

    /// まだ Session に載せていないストリームを止めてから確定する。
    fn finish_with_stream(
        applier: &Applier<ScreenCaptureState>,
        mount_id: u64,
        generation: u64,
        stream: &MediaStream,
        result: Result<CaptureOutcome, String>,
    ) {
        stop_tracks(stream);
        finish(applier, mount_id, generation, result);
    }

    /// `state() == Recording` のときだけ `stop()` する（二重停止は無害）。
    fn stop_recorder(mount_id: u64) {
        let recorder = ACTIVE.with(|a| a.borrow().get(&mount_id).and_then(|s| s.recorder.clone()));
        if let Some(recorder) = recorder {
            if recorder.state() == RecordingState::Recording {
                let _ = recorder.stop();
            }
        }
    }

    /// `onstop` の継続。Closure 自身の中で drop しないよう `spawn_local` へ逃がす。
    fn handle_stop(mount_id: u64, generation: u64, applier: Applier<ScreenCaptureState>) {
        wasm_bindgen_futures::spawn_local(async move {
            if !matches_current(mount_id, generation) || !crate::mount::is_live(mount_id) {
                return;
            }
            let (chunks, recorder_mime, started) = ACTIVE.with(|a| {
                let b = a.borrow();
                let s = b.get(&mount_id);
                (
                    s.map(|s| s.chunks.clone()),
                    s.and_then(|s| s.recorder.as_ref()).map(|r| r.mime_type()),
                    s.and_then(|s| s.started_at_ms),
                )
            });
            let (Some(chunks), Some(recorder_mime), Some(started)) =
                (chunks, recorder_mime, started)
            else {
                fail(&applier, mount_id, generation);
                return;
            };
            let Ok(blob) = web_sys::Blob::new_with_blob_sequence(&chunks) else {
                fail(&applier, mount_id, generation);
                return;
            };
            let duration_ms = (now_ms() - started).max(0.0) as u64;
            let mime = resolve_mime(&recorder_mime, &blob.type_());
            let result = outcome_from_blob(blob.size(), mime, duration_ms);
            finish(&applier, mount_id, generation, result);
        });
    }

    fn handle_error(mount_id: u64, generation: u64, applier: Applier<ScreenCaptureState>) {
        wasm_bindgen_futures::spawn_local(async move {
            fail(&applier, mount_id, generation);
        });
    }

    /// 共有が許可されたあとの続き（recorder の構築・配線・`start()`・タイマー）。
    fn start_recorder(
        mount_id: u64,
        generation: u64,
        applier: &Applier<ScreenCaptureState>,
        stream: MediaStream,
    ) {
        if cancel_requested(mount_id) {
            finish_with_stream(
                applier,
                mount_id,
                generation,
                &stream,
                Ok(CaptureOutcome::Cancelled),
            );
            return;
        }
        let Ok(recorder) = MediaRecorder::new_with_media_stream(&stream) else {
            finish_with_stream(
                applier,
                mount_id,
                generation,
                &stream,
                Ok(CaptureOutcome::Unsupported),
            );
            return;
        };

        let chunks = Array::new();
        let chunks_h = chunks.clone();
        let on_data = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            if let Ok(event) = event.dyn_into::<web_sys::BlobEvent>() {
                if let Some(blob) = event.data() {
                    chunks_h.push(&blob);
                }
            }
        });
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));

        let a = applier.clone();
        let on_stop = Closure::<dyn FnMut(JsValue)>::new(move |_e: JsValue| {
            handle_stop(mount_id, generation, a.clone());
        });
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        let a = applier.clone();
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |_e: JsValue| {
            handle_error(mount_id, generation, a.clone());
        });
        recorder.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        // ブラウザの「共有を停止」。契約上は成功なので通常の停止経路へつなぐ。
        let mut closures = vec![on_data, on_stop, on_error];
        for track in stream.get_video_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                let on_ended = Closure::<dyn FnMut(JsValue)>::new(move |_e: JsValue| {
                    if matches_current(mount_id, generation) {
                        stop_recorder(mount_id);
                    }
                });
                track.set_onended(Some(on_ended.as_ref().unchecked_ref()));
                closures.push(on_ended);
            }
        }

        if recorder.start().is_err() {
            recorder.set_ondataavailable(None);
            recorder.set_onstop(None);
            recorder.set_onerror(None);
            finish_with_stream(
                applier,
                mount_id,
                generation,
                &stream,
                Err(CAPTURE_FAILED.to_string()),
            );
            return;
        }

        let started_at_ms = now_ms();

        let timeout_closure = Closure::<dyn FnMut()>::new(move || {
            if matches_current(mount_id, generation) {
                stop_recorder(mount_id);
            }
        });
        let timeout_id = window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                timeout_closure.as_ref().unchecked_ref(),
                MAX_CAPTURE_MS as i32,
            )
            .ok()
        });

        let applier_tick = applier.clone();
        let interval_closure = Closure::<dyn FnMut()>::new(move || {
            if !matches_current(mount_id, generation) {
                return;
            }
            let elapsed = (now_ms() - started_at_ms).max(0.0) as u64;
            applier_tick.apply(move |s| {
                let dt = elapsed.saturating_sub(s.elapsed_ms);
                tick(s, dt);
            });
        });
        let interval_id = window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                interval_closure.as_ref().unchecked_ref(),
                1_000,
            )
            .ok()
        });

        ACTIVE.with(|active| {
            if let Some(session) = active.borrow_mut().get_mut(&mount_id) {
                session.stream = Some(stream);
                session.recorder = Some(recorder);
                session.chunks = chunks;
                session.started_at_ms = Some(started_at_ms);
                session.timeout_id = timeout_id;
                session.interval_id = interval_id;
                session.closures = closures;
                session.timers = vec![timeout_closure, interval_closure];
            }
        });
    }

    /// 全 Session を破棄し「中断」で確定する（bfcache から戻っても再開しない）。
    fn discard_all_sessions() {
        let entries: Vec<(u64, u64, Applier<ScreenCaptureState>)> = ACTIVE.with(|a| {
            a.borrow()
                .iter()
                .map(|(id, s)| (*id, s.generation, s.applier.clone()))
                .collect()
        });
        for (mount_id, generation, applier) in entries {
            release(mount_id);
            applier.apply(move |s| finish_capture(s, generation, Err(INTERRUPTED.to_string())));
        }
    }

    fn register_pagehide_once() -> bool {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(w) = window() else {
                return false;
            };
            let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
                discard_all_sessions();
            });
            if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
                .is_err()
            {
                return false;
            }
            *cell.borrow_mut() = Some(closure);
            true
        })
    }

    /// `mount_with_effect` へ渡す effect。開始・停止要求を処理する。
    pub fn screen_capture_effect(
        state: &ScreenCaptureState,
        applier: &Applier<ScreenCaptureState>,
    ) {
        let mount_id = applier.mount_id();

        if let Some(request) = take_start_request(state) {
            if ACTIVE.with(|a| a.borrow().contains_key(&mount_id)) {
                return;
            }
            let generation = request.generation;
            applier.apply(mark_recording);
            ACTIVE.with(|a| {
                a.borrow_mut().insert(
                    mount_id,
                    Session {
                        applier: applier.clone(),
                        generation,
                        cancel_requested: false,
                        stream: None,
                        recorder: None,
                        chunks: Array::new(),
                        started_at_ms: None,
                        timeout_id: None,
                        interval_id: None,
                        closures: Vec::new(),
                        timers: Vec::new(),
                    },
                );
            });

            // 登録できなければ離脱後の録画開始を防げないため fail-closed。
            if !register_pagehide_once() {
                fail(applier, mount_id, generation);
                return;
            }

            let unsupported = |applier: &Applier<ScreenCaptureState>| {
                finish(
                    applier,
                    mount_id,
                    generation,
                    Ok(CaptureOutcome::Unsupported),
                );
            };
            let Some(w) = window() else {
                unsupported(applier);
                return;
            };
            if !w.is_secure_context() {
                unsupported(applier);
                return;
            }
            let Ok(media_devices) = w.navigator().media_devices() else {
                unsupported(applier);
                return;
            };
            // 型に現れない JS 側の実態を境界で検証する（security.md A03）。
            let global: JsValue = js_sys::global().into();
            if !has_function(media_devices.as_ref(), "getDisplayMedia")
                || !has_function(&global, "MediaRecorder")
            {
                unsupported(applier);
                return;
            }

            // ユーザー操作の内側に収めるため、await を挟まず同期で呼ぶ。
            let promise = match media_devices.get_display_media() {
                Ok(p) => p,
                Err(err) => {
                    let result = outcome_from_error_name(error_name(&err).as_deref(), false);
                    finish(applier, mount_id, generation, result);
                    return;
                }
            };

            let applier = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match JsFuture::from(promise).await {
                    Ok(value) => {
                        let stream = value.dyn_into::<MediaStream>().ok();
                        if !matches_current(mount_id, generation)
                            || !crate::mount::is_live(mount_id)
                        {
                            // 破棄済み。取得したストリームは即止める（共有表示を残さない）。
                            if let Some(stream) = stream {
                                stop_tracks(&stream);
                            }
                            return;
                        }
                        match stream {
                            Some(stream) => start_recorder(mount_id, generation, &applier, stream),
                            None => fail(&applier, mount_id, generation),
                        }
                    }
                    Err(err) => {
                        if !matches_current(mount_id, generation) {
                            return;
                        }
                        let result = outcome_from_error_name(
                            error_name(&err).as_deref(),
                            cancel_requested(mount_id),
                        );
                        finish(&applier, mount_id, generation, result);
                    }
                }
            });
            return;
        }

        if let Some(request) = take_stop_request(state) {
            applier.apply(mark_stopping);
            let has_recorder = ACTIVE.with(|a| {
                let mut b = a.borrow_mut();
                let Some(s) = b.get_mut(&mount_id) else {
                    return false;
                };
                if s.generation != request.generation {
                    return false;
                }
                if s.recorder.is_none() {
                    s.cancel_requested = true;
                    return false;
                }
                true
            });
            if has_recorder {
                stop_recorder(mount_id);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;
#[cfg(target_arch = "wasm32")]
pub use web::screen_capture_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_requested_wins() {
        assert_eq!(
            outcome_from_error_name(Some("AbortError"), true),
            Ok(CaptureOutcome::Cancelled)
        );
    }

    #[test]
    fn error_names_map_to_three_values() {
        assert_eq!(
            outcome_from_error_name(Some("NotAllowedError"), false),
            Ok(CaptureOutcome::Cancelled)
        );
        assert!(outcome_from_error_name(Some("InvalidStateError"), false).is_err());
        assert_eq!(
            outcome_from_error_name(Some("NotSupportedError"), false),
            Ok(CaptureOutcome::Unsupported)
        );
        assert_eq!(
            outcome_from_error_name(Some("TypeError"), false),
            Ok(CaptureOutcome::Unsupported)
        );
        assert_eq!(
            outcome_from_error_name(Some("Whatever"), false),
            Err(CAPTURE_FAILED.to_string())
        );
        assert_eq!(
            outcome_from_error_name(None, false),
            Err(CAPTURE_FAILED.to_string())
        );
    }

    #[test]
    fn zero_nan_negative_and_zero_duration_fail() {
        for size in [0.0, f64::NAN, -1.0, f64::INFINITY] {
            // INFINITY は非有限として 0 扱い。
            assert_eq!(
                outcome_from_blob(size, "video/webm".into(), 1000),
                Err(CAPTURE_FAILED.to_string())
            );
        }
        assert_eq!(
            outcome_from_blob(10.0, "video/webm".into(), 0),
            Err(CAPTURE_FAILED.to_string())
        );
    }

    #[test]
    fn valid_blob_is_captured() {
        assert_eq!(
            outcome_from_blob(1234.0, "video/webm".into(), 5000),
            Ok(CaptureOutcome::Captured(CaptureSummary {
                size_bytes: 1234,
                mime: "video/webm".into(),
                duration_ms: 5000
            }))
        );
    }
}
