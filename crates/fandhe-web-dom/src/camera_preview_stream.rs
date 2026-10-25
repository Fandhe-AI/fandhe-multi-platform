//! 型サンプル「カメラのライブ映像・フレーム取得」の Web 経路（親 #168、Issue #815）。
//!
//! `fandhe_core::screens::camera_preview_stream_show`（端末依存ゼロ）の `take_start` / `take_zoom`
//! を [`super::mount::mount_with_effect`] の effect として観測し、`getUserMedia` で得た映像を
//! 非表示の `<video>` に流して、`requestVideoFrameCallback` のたびに縮小キャンバスへ描いた画素を
//! 画面へ書き戻す。呼び出し元は `demo.rs` の `mount_camera_preview_stream_show_screen`。
//!
//! Web は `fandhe-device` の `CameraPreview` トレイト（`fandhe-egui-proxy` の effect ドライバが
//! 使う）を通らない（`camera_capture.rs` と同じ理由）。寸法・縮小・検証の不変条件だけは
//! `fandhe_device::camera_preview_stream` の純関数（`preview_size`・`downscale_packed`・
//! `MAX_SOURCE_SIDE`）を共用する。
//!
//! # OS（ブラウザ）を正にする
//!
//! | 事象 | ブラウザの観測点 | 画面への写像 |
//! |------|------------------|--------------|
//! | 開始要求 | `take_start` を観測し、マウントにセッションが無い | `getUserMedia`（背面を ideal、無ければ既定のカメラ）。期限は張らない |
//! | 開始の確定 | `video.play()` の resolve | `apply_running`（トラックが `muted` なら `apply_interrupted`） |
//! | フレーム | `requestVideoFrameCallback` | 縮小キャンバスへ `drawImage` → `getImageData` → `apply_frame` |
//! | 中断・再開 | トラックの `mute` / `unmute` | `apply_interrupted` / `apply_running` |
//! | 実行中の失敗 | トラックの `ended`（許可の取り消し・取り外し） | `apply_runtime_failed` |
//! | 背景へ移る | 映像開始後の `visibilitychange`（`document.hidden`） | `apply_stopped(Backgrounded)`。戻っても再開しない |
//! | 画面を離れる | `pagehide`（bfcache を含む） | `apply_stopped(LeftScreen)`。`pageshow` でも再開しない |
//! | 利用者の停止 | effect の照合（世代・phase の不一致） | 解放だけ行う（画面は `Stopped(User)` 済み） |
//! | マウント破棄 | `mount::sweep_mounts` → [`release_mount`] | 解放だけ行う |
//!
//! ズームは範囲外（`panTiltZoom` の許可を新しく求めることになるため）。ズーム要求には
//! `Unsupported` を返し、表示値は `apply_frame` の `zoom = None` で「この環境では使えない」になる。
//!
//! # 許可ダイアログと背景遷移の順序
//!
//! `visibilitychange` は**ストリーム取得後のセッションにだけ**効かせる（モバイル Chromium は
//! 許可ダイアログの表示中に `hidden` になるため）。`getUserMedia` の resolve 時に `document.hidden`
//! なら再生せずストリームを保持し（`AwaitVisible`）、次に visible になったときに再生を始める。
//! 待つ間の `pagehide` と「止める」では解放する。タイマーは使わない。
//!
//! # 表示の間引き
//!
//! PNG の data URL 生成を伴うため、フレームごとに行うと負荷が大きい。前回の表示から
//! [`DISPLAY_MIN_INTERVAL_MS`] 未満のフレームは描画と反映を省く（受信数には数える。次のコールバックは
//! 必ず予約する）。表示間隔を決めるだけで、状態の判定には使わない。
//!
//! # フレームの反映でボタンを作り直さない
//!
//! `Applier::apply` は root の子を全て作り直すため、mousedown と mouseup の間に挟まると「止める」の
//! `click` が発火しない（間引きは頻度を下げるだけで防げない）。フレームは
//! `Applier::apply_in_place` で `img` の `src` と案内文だけを書き換え、ボタンの要素を残す。
//! 最初のフレーム（`img` がまだ無い）など構造が変わる更新だけが全再構築になる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の開始・停止・受信の順序（世代照合）、失敗・中断・非対応を映像中に畳まない
//! こと、全経路でのトラック・リスナー・コールバック・クロージャの解放、停止を失敗に見せないこと、
//! 例外の `message`・デバイス名・画素をログに出さないこと。
//! 守らない: 画面に出すフレームの間隔と時刻、前面・背面の切り替え、ズーム、複数タブ・複数マウント
//! の同時利用、背景・bfcache からの自動再開、映像の向き、`mute` を出さないブラウザでの中断検知。
//!
//! # 既知の制約
//!
//! `mount_with_effect` は `Screen` を固定するため、トグルボタンの文言は「映像を始める」のまま変わらず、
//! ズームのボタンも出ない（`mic_record.rs`・`screen_capture.rs` と同じ）。

use fandhe_core::screens::camera_preview_stream_show::{FailureView, FrameView, PreviewPhase};
use fandhe_core::ui::RgbaPixels;
use fandhe_device::camera_preview_stream::{
    downscale_packed, preview_size, PackedLayout, MAX_SOURCE_SIDE,
};

/// 画面へ出すフレームの最小間隔（ミリ秒、単調時計）。実ブラウザで見直す仮置き値（#808）。
pub const DISPLAY_MIN_INTERVAL_MS: f64 = 200.0;

/// `getUserMedia` の例外 `name` を開始失敗の種別へ写す。`message` は読まない（security.md A09）。
/// 非対応（`SecurityError` 等）は呼び出し側が API の有無で `apply_unsupported` にするため、ここでは
/// `Other` に畳む。
pub fn failure_from_error_name(name: Option<&str>) -> FailureView {
    match name {
        Some("NotAllowedError" | "PermissionDeniedError") => FailureView::PermissionDenied,
        Some("NotFoundError" | "OverconstrainedError" | "DevicesNotFoundError") => {
            FailureView::NoCamera
        }
        Some("NotReadableError" | "TrackStartError") => FailureView::InUse,
        _ => FailureView::Other,
    }
}

/// 元寸法から縮小キャンバスの寸法を決める。検証は丸め（縮小）の前に行い、範囲外は `None`。
pub fn canvas_size(video_w: u32, video_h: u32) -> Option<(u32, u32)> {
    let ok = |v: u32| (1..=MAX_SOURCE_SIDE).contains(&v);
    if !ok(video_w) || !ok(video_h) {
        return None;
    }
    let (w, h) = preview_size(video_w, video_h);
    (w > 0 && h > 0).then_some((w, h))
}

/// `getImageData` の RGBA から画面用のフレームを作る。寸法 0・長さ不一致・上限超過は `None`
/// （嘘の成功にしない）。
pub fn frame_view_from_image_data(
    src_w: u32,
    src_h: u32,
    canvas_w: u32,
    canvas_h: u32,
    data: &[u8],
) -> Option<FrameView> {
    if !(1..=MAX_SOURCE_SIDE).contains(&src_w) || !(1..=MAX_SOURCE_SIDE).contains(&src_h) {
        return None;
    }
    let stride = (canvas_w as usize).checked_mul(4)?;
    let scaled = downscale_packed(canvas_w, canvas_h, stride, PackedLayout::Rgba8, data)?;
    let pixels = RgbaPixels::new(scaled.width, scaled.height, scaled.rgba)?;
    Some(FrameView {
        pixels,
        source_width: src_w,
        source_height: src_h,
    })
}

/// 画面へ出すべきか。前回から [`DISPLAY_MIN_INTERVAL_MS`] 以上空いたとき、または初回。
/// 非有限の値・時刻の逆行は「出す」側に倒し、止まりっぱなしにしない。
pub fn should_display(now_ms: f64, last_ms: Option<f64>) -> bool {
    let Some(last) = last_ms else {
        return true;
    };
    if !now_ms.is_finite() || !last.is_finite() || now_ms < last {
        return true;
    }
    now_ms - last >= DISPLAY_MIN_INTERVAL_MS
}

/// セッションを維持してよいか。phase が進行中で、世代が画面の現在の要求と一致するときだけ真。
pub fn keep_session(phase: PreviewPhase, state_gen: u64, session_gen: u64) -> bool {
    state_gen == session_gen
        && matches!(
            phase,
            PreviewPhase::Starting | PreviewPhase::Live | PreviewPhase::Interrupted
        )
}

/// フレームの到着を DOM へどう反映するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePatch {
    /// `Live`: 映像の要素だけを書き換える（要素が無ければ呼び出し側が全再構築へ倒す）。
    Rewrite,
    /// `Starting`・`Interrupted`: 状態はフレームで変わらず DOM は直前の描画と一致する。
    /// 何も書かない（全再構築すると停止ボタンが作り直されクリックを取り逃がす）。
    Keep,
    /// それ以外（終端等）: 全再構築に任せる。
    Rebuild,
}

/// フェーズからフレーム到着時の DOM 反映方法を決める。
pub fn frame_patch_kind(phase: PreviewPhase) -> FramePatch {
    match phase {
        PreviewPhase::Live => FramePatch::Rewrite,
        PreviewPhase::Starting | PreviewPhase::Interrupted => FramePatch::Keep,
        _ => FramePatch::Rebuild,
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::camera_preview_stream_show::{
        apply_frame, apply_interrupted, apply_running, apply_runtime_failed, apply_start_failed,
        apply_stopped, apply_unsupported, finish_zoom, status_text, take_start, take_zoom,
        CameraPreviewStreamShowState, FailureView, StopView, ZoomFailureView,
    };
    use js_sys::{Function, Object, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream,
        MediaStreamConstraints, MediaStreamTrack,
    };

    use super::{
        canvas_size, failure_from_error_name, frame_patch_kind, frame_view_from_image_data,
        keep_session, should_display, FramePatch,
    };
    use crate::mount::{is_live, Applier};

    type EventClosure = Closure<dyn FnMut(web_sys::Event)>;
    type FrameClosure = Closure<dyn FnMut(f64, JsValue)>;

    /// セッションの段。
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Stage {
        /// `getUserMedia` の Promise 待ち（許可ダイアログを含む）。
        Requesting,
        /// ストリームは得たが `document.hidden` のため再生を待っている。
        AwaitVisible,
        /// 再生中（`play()` 待ちを含む）。
        Playing,
    }

    /// 1 マウント 1 セッション。全終了経路が [`release`] を通して解放する。
    struct Session {
        applier: Applier<CameraPreviewStreamShowState>,
        generation: u64,
        stage: Stage,
        stream: Option<MediaStream>,
        video: Option<HtmlVideoElement>,
        canvas: Option<(HtmlCanvasElement, CanvasRenderingContext2d)>,
        rvfc_handle: Option<f64>,
        rvfc_closure: Option<FrameClosure>,
        track_listeners: Vec<(MediaStreamTrack, &'static str, EventClosure)>,
        received: u64,
        last_zoom_seq: u64,
        last_display_ms: Option<f64>,
    }

    thread_local! {
        static ACTIVE: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
        static PAGEHIDE: RefCell<Option<EventClosure>> = const { RefCell::new(None) };
        static VISIBILITYCHANGE: RefCell<Option<EventClosure>> = const { RefCell::new(None) };
    }

    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    fn is_hidden() -> bool {
        window()
            .and_then(|w| w.document())
            .map(|d| d.hidden())
            .unwrap_or(false)
    }

    fn matches_current(mount_id: u64, generation: u64) -> bool {
        ACTIVE.with(|a| {
            a.borrow()
                .get(&mount_id)
                .is_some_and(|s| s.generation == generation)
        })
    }

    fn error_name(e: &JsValue) -> Option<String> {
        Reflect::get(e, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
    }

    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|v| v.is_function())
            .unwrap_or(false)
    }

    fn stop_stream(stream: &MediaStream) {
        for t in stream.get_tracks().iter() {
            if let Ok(t) = t.dyn_into::<MediaStreamTrack>() {
                t.stop();
            }
        }
    }

    /// 実行中のクロージャを呼び出しの最中に drop しないよう、マイクロタスクまで延ばして捨てる。
    fn defer_drop<T: 'static>(v: T) {
        wasm_bindgen_futures::spawn_local(async move {
            drop(v);
        });
    }

    /// セッションを取り出して解放する。解放の順序: 登録表から外す → rVFC を取り消す →
    /// トラックのリスナーを外す → 再生を止めて `srcObject` を外す → 全トラックを止める →
    /// クロージャの drop を遅らせる。戻り値は通知用の `(applier, generation)`。無ければ `None`
    /// （二重解放を無害化する）。
    fn release(mount_id: u64) -> Option<(Applier<CameraPreviewStreamShowState>, u64)> {
        let session = ACTIVE.with(|a| a.borrow_mut().remove(&mount_id))?;
        if let (Some(video), Some(handle)) = (&session.video, session.rvfc_handle) {
            let v: JsValue = video.clone().into();
            if let Ok(f) = Reflect::get(&v, &JsValue::from_str("cancelVideoFrameCallback")) {
                if let Some(f) = f.dyn_ref::<Function>() {
                    let _ = f.call1(&v, &JsValue::from_f64(handle));
                }
            }
        }
        for (track, ev, closure) in &session.track_listeners {
            let _ = track.remove_event_listener_with_callback(ev, closure.as_ref().unchecked_ref());
        }
        if let Some(video) = &session.video {
            let _ = video.pause();
            video.set_src_object(None);
        }
        if let Some(stream) = &session.stream {
            stop_stream(stream);
        }
        let out = (session.applier.clone(), session.generation);
        defer_drop(session);
        Some(out)
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。マウントは死んでいるので apply しない。
    pub(crate) fn release_mount(mount_id: u64) {
        release(mount_id);
    }

    /// 解放して停止を画面へ届ける（解放が先。届けた後で片付ける順序にしない）。
    fn stop_with(mount_id: u64, why: StopView) {
        if let Some((applier, generation)) = release(mount_id) {
            applier.apply(move |s| apply_stopped(s, generation, why));
        }
    }

    fn fail_with(mount_id: u64, view: FailureView, runtime: bool) {
        if let Some((applier, generation)) = release(mount_id) {
            applier.apply(move |s| {
                if runtime {
                    apply_runtime_failed(s, generation);
                } else {
                    apply_start_failed(s, generation, view);
                }
            });
        }
    }

    fn unsupported(mount_id: u64) {
        if let Some((applier, generation)) = release(mount_id) {
            applier.apply(move |s| apply_unsupported(s, generation));
        }
    }

    fn all_ids() -> Vec<(u64, Stage)> {
        ACTIVE.with(|a| a.borrow().iter().map(|(id, s)| (*id, s.stage)).collect())
    }

    fn register_pagehide_once() -> bool {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(w) = window() else {
                return false;
            };
            let closure = EventClosure::new(|_e: web_sys::Event| {
                for (id, _) in all_ids() {
                    stop_with(id, StopView::LeftScreen);
                }
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

    /// ストリーム取得後にだけ呼ぶ（許可ダイアログ中の `hidden` で正常な許可を潰さないため）。
    fn register_visibilitychange_once() -> bool {
        VISIBILITYCHANGE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(doc) = window().and_then(|w| w.document()) else {
                return false;
            };
            let closure = EventClosure::new(|_e: web_sys::Event| on_visibility());
            if doc
                .add_event_listener_with_callback(
                    "visibilitychange",
                    closure.as_ref().unchecked_ref(),
                )
                .is_err()
            {
                return false;
            }
            *cell.borrow_mut() = Some(closure);
            true
        })
    }

    /// 背景へ移ったら再生中を止め、戻ったら表示待ちを再生する。
    fn on_visibility() {
        let hidden = is_hidden();
        for (id, stage) in all_ids() {
            if hidden && stage == Stage::Playing {
                stop_with(id, StopView::Backgrounded);
            } else if !hidden && stage == Stage::AwaitVisible {
                let taken = ACTIVE.with(|a| {
                    a.borrow_mut().get_mut(&id).and_then(|s| {
                        s.stage = Stage::Playing;
                        s.stream.clone()
                    })
                });
                let generation = ACTIVE.with(|a| a.borrow().get(&id).map(|s| s.generation));
                if let (Some(stream), Some(generation)) = (taken, generation) {
                    begin_playback(id, generation, stream);
                }
            }
        }
    }

    /// 映像トラックのイベントを購読する。`ended` は実行中の失敗、`mute`/`unmute` は中断/再開。
    fn listen_track(mount_id: u64, generation: u64, track: &MediaStreamTrack) {
        let mut added: Vec<(MediaStreamTrack, &'static str, EventClosure)> = Vec::new();
        for ev in ["ended", "mute", "unmute"] {
            let closure = EventClosure::new(move |_e: web_sys::Event| {
                let Some(applier) = ACTIVE.with(|a| {
                    a.borrow()
                        .get(&mount_id)
                        .filter(|s| s.generation == generation)
                        .map(|s| s.applier.clone())
                }) else {
                    return;
                };
                match ev {
                    "ended" => fail_with(mount_id, FailureView::Runtime, true),
                    "mute" => applier.apply(move |s| apply_interrupted(s, generation)),
                    _ => applier.apply(move |s| apply_running(s, generation)),
                }
            });
            if track
                .add_event_listener_with_callback(ev, closure.as_ref().unchecked_ref())
                .is_ok()
            {
                // `MediaStreamTrack::clone` は JS の clone()（別トラック）になるため、参照の複製を明示する。
                added.push((Clone::clone(track), ev, closure));
            }
        }
        ACTIVE.with(|a| {
            if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                if s.generation == generation {
                    s.track_listeners.extend(added);
                }
            }
        });
    }

    /// 再生を始める（`document.hidden` でないときだけ呼ぶ）。`play()` の resolve で開始を確定する。
    fn begin_playback(mount_id: u64, generation: u64, stream: MediaStream) {
        // 先にストリームをセッションへ預ける。以降の早期 return はすべて `release`（`fail_with`）を
        // 通り、トラックの停止はそこへ一本化される（分岐ごとに `stop_stream` を足さない）。
        let stored = ACTIVE.with(|a| {
            if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                if s.generation == generation {
                    s.stage = Stage::Playing;
                    // `stream.clone()` は JS の MediaStream.clone()（トラック複製）になるため
                    // `Clone::clone` で参照の複製を明示する。
                    s.stream = Some(Clone::clone(&stream));
                    return true;
                }
            }
            false
        });
        if !stored {
            stop_stream(&stream);
            return;
        }
        let Some(doc) = window().and_then(|w| w.document()) else {
            return fail_with(mount_id, FailureView::Other, false);
        };
        if !register_visibilitychange_once() {
            return fail_with(mount_id, FailureView::Other, false);
        }
        let Some(video) = doc
            .create_element("video")
            .ok()
            .and_then(|e| e.dyn_into::<HtmlVideoElement>().ok())
        else {
            return fail_with(mount_id, FailureView::Other, false);
        };
        video.set_muted(true);
        let _ = video.set_attribute("playsinline", "");
        video.set_src_object(Some(&stream));
        let video_stored = ACTIVE.with(|a| {
            if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                if s.generation == generation {
                    s.video = Some(video.clone());
                    return true;
                }
            }
            false
        });
        if !video_stored {
            video.set_src_object(None);
            return;
        }
        // 取得から再生の確定までに ended になったトラックは `ended` イベントを取り逃すため、
        // `play()` の前後で状態を直接確認する。購読は開始の確定後（下の非同期部）に始める。
        let already_ended = stream.get_video_tracks().iter().any(|t| {
            t.dyn_into::<MediaStreamTrack>()
                .is_ok_and(|t| t.ready_state() == web_sys::MediaStreamTrackState::Ended)
        });
        if already_ended {
            return fail_with(mount_id, FailureView::Runtime, true);
        }
        let Ok(promise) = video.play() else {
            return fail_with(mount_id, FailureView::Other, false);
        };
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await;
            // 解放や差し替えが済んでいれば何もしない（`pause()` による AbortError もここで捨てる）。
            if !matches_current(mount_id, generation) || !is_live(mount_id) {
                return;
            }
            if result.is_err() {
                return fail_with(mount_id, FailureView::Other, false);
            }
            let tracks: Vec<MediaStreamTrack> = stream
                .get_video_tracks()
                .iter()
                .filter_map(|t| t.dyn_into::<MediaStreamTrack>().ok())
                .collect();
            // play() 待ちの間に ended になった場合も、映像中にせず失敗へ倒す。
            if tracks
                .iter()
                .any(|t| t.ready_state() == web_sys::MediaStreamTrackState::Ended)
            {
                return fail_with(mount_id, FailureView::Runtime, true);
            }
            // 開始は `play()` の resolve で確定する。`mute`/`unmute` の購読はここから始め、
            // 確定前の `unmute` で開始が確定しないようにする（確定の場所を 1 か所にする）。
            // 状態の読み取りと購読の間に await を挟まないため、取りこぼしは起きない。
            for t in &tracks {
                listen_track(mount_id, generation, t);
            }
            let muted = tracks.iter().any(|t| t.muted());
            let Some(applier) = ACTIVE.with(|a| {
                a.borrow()
                    .get(&mount_id)
                    .filter(|s| s.generation == generation)
                    .map(|s| s.applier.clone())
            }) else {
                return;
            };
            if muted {
                applier.apply(move |s| apply_interrupted(s, generation));
            } else {
                applier.apply(move |s| apply_running(s, generation));
            }
            if matches_current(mount_id, generation) {
                schedule_frame(mount_id, generation);
            }
        });
    }

    /// 次の `requestVideoFrameCallback` を予約する。コールバックは初回に 1 度だけ作る。
    fn schedule_frame(mount_id: u64, generation: u64) {
        let prepared = ACTIVE.with(|a| {
            let mut map = a.borrow_mut();
            let s = map.get_mut(&mount_id)?;
            if s.generation != generation {
                return None;
            }
            let video = s.video.clone()?;
            let closure = s.rvfc_closure.get_or_insert_with(|| {
                FrameClosure::new(move |now: f64, _meta: JsValue| {
                    on_frame(mount_id, generation, now);
                })
            });
            let f: Function = closure.as_ref().unchecked_ref::<Function>().clone();
            Some((video, f))
        });
        let Some((video, f)) = prepared else {
            return;
        };
        let v: JsValue = video.into();
        let handle = Reflect::get(&v, &JsValue::from_str("requestVideoFrameCallback"))
            .ok()
            .and_then(|m| m.dyn_into::<Function>().ok())
            .and_then(|m| m.call1(&v, &f).ok())
            .and_then(|h| h.as_f64());
        match handle {
            Some(h) => ACTIVE.with(|a| {
                if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                    if s.generation == generation {
                        s.rvfc_handle = Some(h);
                    }
                }
            }),
            None => fail_with(mount_id, FailureView::Runtime, true),
        }
    }

    /// 縮小キャンバスへ描いて RGBA を読む。キャンバスは寸法が変わったときだけ作り直す。
    fn draw(
        mount_id: u64,
        generation: u64,
        video: &HtmlVideoElement,
        cw: u32,
        ch: u32,
    ) -> Option<Vec<u8>> {
        let existing = ACTIVE.with(|a| {
            a.borrow()
                .get(&mount_id)
                .filter(|s| s.generation == generation)
                .and_then(|s| s.canvas.clone())
        });
        let (canvas, ctx) = match existing {
            Some(c) => c,
            None => {
                let doc = window()?.document()?;
                let canvas: HtmlCanvasElement =
                    doc.create_element("canvas").ok()?.dyn_into().ok()?;
                let opts = Object::new();
                Reflect::set(&opts, &"willReadFrequently".into(), &JsValue::TRUE).ok()?;
                let ctx: CanvasRenderingContext2d = canvas
                    .get_context_with_context_options("2d", &opts)
                    .ok()??
                    .dyn_into()
                    .ok()?;
                ACTIVE.with(|a| {
                    if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                        if s.generation == generation {
                            s.canvas = Some((canvas.clone(), ctx.clone()));
                        }
                    }
                });
                (canvas, ctx)
            }
        };
        if canvas.width() != cw {
            canvas.set_width(cw);
        }
        if canvas.height() != ch {
            canvas.set_height(ch);
        }
        ctx.draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, cw as f64, ch as f64)
            .ok()?;
        let data = ctx.get_image_data(0.0, 0.0, cw as f64, ch as f64).ok()?;
        Some(data.data().0)
    }

    /// フレームの到着を既存の DOM へ直接書く（`img` の `src` と案内文だけ）。ボタンを作り直さない
    /// ので、クリックの途中で要素が差し替わらない。`Live` で映像の要素がまだ無いとき（最初のフレーム
    /// ・フェーズの切り替わり）は `false` を返して全再構築へ倒す。
    fn patch_frame(root: &web_sys::Element, state: &CameraPreviewStreamShowState) -> bool {
        match frame_patch_kind(state.phase) {
            FramePatch::Keep => return true,
            FramePatch::Rebuild => return false,
            FramePatch::Rewrite => {}
        }
        let Some(frame) = &state.frame else {
            return false;
        };
        let (Ok(Some(img)), Ok(Some(status))) = (
            root.query_selector("img.fandhe-image"),
            root.query_selector("p"),
        ) else {
            return false;
        };
        img.set_attribute("src", &crate::png_data_url::png_data_url(&frame.pixels))
            .is_ok()
            && {
                status.set_text_content(Some(&status_text(state)));
                true
            }
    }

    /// `requestVideoFrameCallback` の本体。判断は `ACTIVE` の借用の中で済ませ、借用を落としてから
    /// apply する（apply は同期で再描画と effect を起こすため）。
    fn on_frame(mount_id: u64, generation: u64, now: f64) {
        if !is_live(mount_id) {
            release(mount_id);
            return;
        }
        let snapshot = ACTIVE.with(|a| {
            let map = a.borrow();
            let s = map.get(&mount_id).filter(|s| s.generation == generation)?;
            Some((
                s.video.clone()?,
                s.applier.clone(),
                s.received,
                s.last_display_ms,
            ))
        });
        let Some((video, applier, received, last_display)) = snapshot else {
            return;
        };
        let (sw, sh) = (video.video_width(), video.video_height());
        // 不正な寸法のフレームは数えずに捨てる（嘘の成功にしない）。
        if let Some((cw, ch)) = canvas_size(sw, sh) {
            let received = received + 1;
            let show = should_display(now, last_display);
            ACTIVE.with(|a| {
                if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                    if s.generation == generation {
                        s.received = received;
                        if show {
                            s.last_display_ms = Some(now);
                        }
                    }
                }
            });
            if show {
                let view = draw(mount_id, generation, &video, cw, ch)
                    .and_then(|d| frame_view_from_image_data(sw, sh, cw, ch, &d));
                match view {
                    Some(view) => applier.apply_in_place(
                        move |s| apply_frame(s, generation, received, view, None),
                        patch_frame,
                    ),
                    None => return fail_with(mount_id, FailureView::Runtime, true),
                }
            }
        }
        // 省いたときも必ず次を予約する（apply で解放済みなら予約しない）。
        if matches_current(mount_id, generation) {
            schedule_frame(mount_id, generation);
        }
    }

    /// `getUserMedia` が resolve / reject した後の続き。
    fn on_user_media(mount_id: u64, generation: u64, result: Result<JsValue, JsValue>) {
        let current = matches_current(mount_id, generation) && is_live(mount_id);
        match result {
            Ok(v) => {
                let Ok(stream) = v.dyn_into::<MediaStream>() else {
                    if current {
                        fail_with(mount_id, FailureView::Other, false);
                    }
                    return;
                };
                if !current {
                    // 解放・差し替え済み。得たストリームはその場で止める。
                    stop_stream(&stream);
                    return;
                }
                if is_hidden() {
                    if !register_visibilitychange_once() {
                        stop_stream(&stream);
                        return fail_with(mount_id, FailureView::Other, false);
                    }
                    ACTIVE.with(|a| {
                        if let Some(s) = a.borrow_mut().get_mut(&mount_id) {
                            s.stage = Stage::AwaitVisible;
                            s.stream = Some(Clone::clone(&stream));
                        }
                    });
                } else {
                    begin_playback(mount_id, generation, stream);
                }
            }
            Err(e) => {
                if current {
                    let view = failure_from_error_name(error_name(&e).as_deref());
                    fail_with(mount_id, view, false);
                }
            }
        }
    }

    /// 開始。非対応は API の有無だけで判定する（許可やカメラの有無を事前に問い合わせない）。
    fn start(applier: &Applier<CameraPreviewStreamShowState>, mount_id: u64, generation: u64) {
        ACTIVE.with(|a| {
            a.borrow_mut().insert(
                mount_id,
                Session {
                    applier: applier.clone(),
                    generation,
                    stage: Stage::Requesting,
                    stream: None,
                    video: None,
                    canvas: None,
                    rvfc_handle: None,
                    rvfc_closure: None,
                    track_listeners: Vec::new(),
                    received: 0,
                    last_zoom_seq: 0,
                    last_display_ms: None,
                },
            );
        });
        if !register_pagehide_once() {
            return fail_with(mount_id, FailureView::Other, false);
        }
        let Some(w) = window() else {
            return fail_with(mount_id, FailureView::Other, false);
        };
        if !w.is_secure_context() {
            return unsupported(mount_id);
        }
        let Ok(md) = w.navigator().media_devices() else {
            return unsupported(mount_id);
        };
        let md_value: JsValue = md.clone().into();
        let rvfc_ok = Reflect::get(&w, &JsValue::from_str("HTMLVideoElement"))
            .and_then(|c| Reflect::get(&c, &JsValue::from_str("prototype")))
            .map(|p| has_function(&p, "requestVideoFrameCallback"))
            .unwrap_or(false);
        if !has_function(&md_value, "getUserMedia") || !rvfc_ok {
            return unsupported(mount_id);
        }
        // 背面を ideal にする（無ければ既定のカメラ）。ズーム用の許可（panTiltZoom）は求めない。
        let facing = Object::new();
        let video = Object::new();
        let built = Reflect::set(&facing, &"ideal".into(), &"environment".into()).is_ok()
            && Reflect::set(&video, &"facingMode".into(), &facing).is_ok();
        if !built {
            return fail_with(mount_id, FailureView::Other, false);
        }
        let constraints = MediaStreamConstraints::new();
        constraints.set_video(&video);
        constraints.set_audio(&JsValue::FALSE);
        let promise = match md.get_user_media_with_constraints(&constraints) {
            Ok(p) => p,
            Err(e) => {
                let view = failure_from_error_name(error_name(&e).as_deref());
                return fail_with(mount_id, view, false);
            }
        };
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await;
            on_user_media(mount_id, generation, result);
        });
    }

    /// `mount_with_effect` へ渡す effect。描画のたびに呼ばれるため、処理すべき要求が無ければ何もしない。
    pub fn camera_preview_stream_show_effect(
        state: &CameraPreviewStreamShowState,
        applier: &Applier<CameraPreviewStreamShowState>,
    ) {
        let mount_id = applier.mount_id();
        // 1. 照合: 利用者の停止・世代の差し替えは解放だけ行う。
        let existing = ACTIVE.with(|a| a.borrow().get(&mount_id).map(|s| s.generation));
        if let Some(session_gen) = existing {
            if !keep_session(state.phase, state.generation, session_gen) {
                release(mount_id);
            }
        }
        // 2. 開始: 同じ要求で二重に起動しない。
        if let Some(generation) = take_start(state) {
            let has = ACTIVE.with(|a| a.borrow().contains_key(&mount_id));
            if !has {
                start(applier, mount_id, generation);
            }
        }
        // 3. ズーム: 範囲外。同じ要求に 1 回だけ「使えない」を返す。
        if let Some(req) = take_zoom(state) {
            let fresh = ACTIVE.with(|a| {
                let mut map = a.borrow_mut();
                match map.get_mut(&mount_id) {
                    Some(s) if s.generation == req.generation && s.last_zoom_seq != req.seq => {
                        s.last_zoom_seq = req.seq;
                        true
                    }
                    _ => false,
                }
            });
            if fresh {
                let (generation, seq) = (req.generation, req.seq);
                applier.apply(move |s| {
                    finish_zoom(s, generation, seq, Err(ZoomFailureView::Unsupported))
                });
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::camera_preview_stream_show_effect;

/// マウント破棄時に解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_names_map_to_failures() {
        for (n, want) in [
            (Some("NotAllowedError"), FailureView::PermissionDenied),
            (Some("PermissionDeniedError"), FailureView::PermissionDenied),
            (Some("NotFoundError"), FailureView::NoCamera),
            (Some("OverconstrainedError"), FailureView::NoCamera),
            (Some("DevicesNotFoundError"), FailureView::NoCamera),
            (Some("NotReadableError"), FailureView::InUse),
            (Some("TrackStartError"), FailureView::InUse),
            (Some("AbortError"), FailureView::Other),
            (Some("SecurityError"), FailureView::Other),
            (Some("Whatever"), FailureView::Other),
            (None, FailureView::Other),
        ] {
            assert_eq!(failure_from_error_name(n), want, "{n:?}");
        }
    }

    #[test]
    fn canvas_size_validates_before_scaling() {
        assert_eq!(canvas_size(0, 480), None);
        assert_eq!(canvas_size(640, 0), None);
        assert_eq!(canvas_size(MAX_SOURCE_SIDE + 1, 10), None);
        assert_eq!(canvas_size(1280, 720), Some((320, 180)));
        assert_eq!(canvas_size(200, 100), Some((200, 100)));
    }

    #[test]
    fn frame_view_accepts_exact_and_rejects_invalid() {
        let data = vec![7u8; 4 * 4 * 4];
        let v = frame_view_from_image_data(640, 480, 4, 4, &data).expect("valid");
        assert_eq!((v.source_width, v.source_height), (640, 480));
        assert_eq!(v.pixels.width(), 4);
        assert!(frame_view_from_image_data(640, 480, 4, 4, &data[..10]).is_none());
        assert!(frame_view_from_image_data(640, 480, 0, 4, &data).is_none());
        assert!(frame_view_from_image_data(0, 480, 4, 4, &data).is_none());
        assert!(frame_view_from_image_data(640, 0, 4, 4, &data).is_none());
    }

    #[test]
    fn display_interval_uses_monotonic_values_safely() {
        assert!(should_display(0.0, None));
        assert!(!should_display(100.0, Some(0.0)));
        assert!(should_display(200.0, Some(0.0)));
        assert!(should_display(10.0, Some(500.0)), "逆行は表示側に倒す");
        assert!(should_display(f64::NAN, Some(0.0)));
        assert!(should_display(100.0, Some(f64::NAN)));
    }

    #[test]
    fn interrupted_and_starting_frames_keep_dom() {
        use fandhe_core::screens::camera_preview_stream_show::StopView;
        assert_eq!(frame_patch_kind(PreviewPhase::Live), FramePatch::Rewrite);
        assert_eq!(
            frame_patch_kind(PreviewPhase::Interrupted),
            FramePatch::Keep
        );
        assert_eq!(frame_patch_kind(PreviewPhase::Starting), FramePatch::Keep);
        assert_eq!(
            frame_patch_kind(PreviewPhase::Stopped(StopView::User)),
            FramePatch::Rebuild
        );
    }

    #[test]
    fn session_kept_only_for_active_phase_and_same_generation() {
        use fandhe_core::screens::camera_preview_stream_show::StopView;
        for p in [
            PreviewPhase::Starting,
            PreviewPhase::Live,
            PreviewPhase::Interrupted,
        ] {
            assert!(keep_session(p, 3, 3));
            assert!(!keep_session(p, 4, 3));
        }
        for p in [
            PreviewPhase::Idle,
            PreviewPhase::Stopped(StopView::User),
            PreviewPhase::Failed(FailureView::Other),
        ] {
            assert!(!keep_session(p, 3, 3));
        }
    }
}
