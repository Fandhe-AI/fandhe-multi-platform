//! 型サンプル「動画の撮影」の Web 経路（親 #169、Issue #824）。
//!
//! `fandhe_core::screens::video_capture_show`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `<input type="file" accept="video/*" capture="environment">`（HTML Media Capture）を
//! [`crate::file_picker::drive`]（写真選択・ファイル選択と共用）で開く。選ばれた `File` の
//! `size` と、object URL を入れた `<video>` の `loadedmetadata`（`duration` /
//! `videoWidth` / `videoHeight`）から長さ・寸法・サイズを出す。
//! 呼び出し元は `demo.rs`（`mount_video_capture_show_screen`）、解放の契機は
//! `mount.rs::sweep_mounts`（`release_mount`）。
//!
//! Web は `fandhe-device` の `VideoCapturer` トレイトを通らない（`file_picker.rs` と同じ分担）。
//! 検査の SSOT は [`fandhe_device::video_capture::VideoClip::from_os`] で、複製しない。
//! モバイルのブラウザはカメラアプリを開き、デスクトップのブラウザは `capture` を無視して通常の
//! ファイル選択になるため、結果は常に「受け取った動画」（`ClipOrigin::Received`）として出す。
//! `MediaRecorder` で自前に録る方式は、新しい許可と撮影画面が要るため使わない。
//!
//! # 結果の写像（契約表 10 の三値）
//!
//! - `File` あり・メタデータ読み取り成功: 成功
//! - `change` でファイルなし、または `cancel`: 取り消し（失敗にしない）
//! - `File.size` が 0: `Empty`
//! - `<video>` の `error`、`videoWidth` / `videoHeight` が 0（動画でない・壊れた動画）: `Unreadable`
//! - `<input>` 生成・リスナー登録の失敗: `Unsupported`
//! - 画面の破棄で中断: 反映しない（`Applier::apply` が破棄済みのマウントでは捨てる）
//!
//! `duration` は有限かつ正のときだけ長さとして出す。NaN（メディアなし）・Infinity（長さ不明）・
//! 0 以下は「長さを読めません」。範囲の検証は整数へ変換する前に行う。回転は 0 として渡す。
//! ブラウザが回転を当てた寸法を返す前提は未確認で、実ブラウザ確認（#819）で縦撮りを見る。
//!
//! # 資源の解放
//!
//! object URL・`<video>` の `src`・リスナーは、成功・失敗・取り消し・画面の破棄のすべての経路で
//! 解放する。`Closure` は作らず `forget()` もしない（イベントは `Promise` の `resolve` /
//! `reject` に直接つなぐ）。動画の中身は `ArrayBuffer` へ読まない（巨大なファイルでもメモリを
//! 確保しない）。ファイル名・URL・例外の `message` は `console` にも画面にも出さない。
//!
//! # 期限を持たない理由と守る範囲
//!
//! 撮れたかどうかはブラウザの `change` / `cancel` だけで決め、タイマーでは推測しない。
//! モバイルはカメラアプリを開くとページが隠れるのが正常な流れなので、`visibilitychange` /
//! `pagehide` でも取り消さない。
//! 守る: 1 マウント内の要求と完了の順序（`finish_capture` の世代照合）、失敗を成功へ畳まないこと、
//! 資源の解放。守らない: `cancel` 非対応の古いブラウザで通知が届かない場合（画面は撮影中の
//! まま）、撮影中のブラウザのプロセス死、動画の中身・画質・形式、複数タブ。

use fandhe_core::screens::video_capture_show::{
    finish_capture, ClipOrigin, CoreFailure, CoreOutcome, ShownClip, VideoCaptureShowState,
};
use fandhe_device::video_capture::{VideoCaptureFailure, VideoClip};

/// `<input>` の `accept`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const ACCEPT: &str = "video/*";
/// `<input>` の `capture`（背面カメラ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CAPTURE: &str = "environment";

/// 契約上の失敗を画面の失敗種別へ写す。`fandhe-egui-proxy` に同じ表があるが、
/// 本クレートは egui-proxy に依存できない（依存方向）ため複製する。
fn to_core_failure(f: VideoCaptureFailure) -> CoreFailure {
    match f {
        VideoCaptureFailure::Unsupported => CoreFailure::Unsupported,
        VideoCaptureFailure::PermissionDenied => CoreFailure::PermissionDenied,
        VideoCaptureFailure::NoCameraApp => CoreFailure::NoCameraApp,
        VideoCaptureFailure::NotForeground => CoreFailure::NotForeground,
        VideoCaptureFailure::Interrupted => CoreFailure::Interrupted,
        VideoCaptureFailure::Empty => CoreFailure::Empty,
        VideoCaptureFailure::Unreadable => CoreFailure::Unreadable,
        VideoCaptureFailure::Internal => CoreFailure::Internal,
    }
}

/// ブラウザから読んだ値を検査して画面へ渡す形にする。
///
/// 範囲の検証（有限・正）は整数へ変換する前に行う。`size` が数値でない・0 以下は `Empty`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn clip_from_browser(
    size: f64,
    duration_secs: f64,
    width: u32,
    height: u32,
) -> Result<ShownClip, CoreFailure> {
    if !size.is_finite() || size <= 0.0 {
        return Err(CoreFailure::Empty);
    }
    // `as` は飽和変換（巨大値は u64::MAX）。0 バイトでないことは上で確認済み。
    let size_bytes = size as u64;
    let duration_ms = (duration_secs.is_finite() && duration_secs > 0.0)
        .then(|| (duration_secs * 1000.0).round() as u64);
    let clip =
        VideoClip::from_os(size_bytes, duration_ms, width, height, 0).map_err(to_core_failure)?;
    Ok(ShownClip {
        size_bytes: clip.size_bytes(),
        duration_ms: clip.duration_ms(),
        width: clip.width(),
        height: clip.height(),
        origin: ClipOrigin::Received,
    })
}

/// 選択の完了を画面へ反映する。世代の照合は `finish_capture` に任せる。
///
/// `Ok(None)` は取り消し、`Err(_)` は `<input>` の生成・リスナー登録の失敗（`Unsupported`）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn finish_from_pick(
    s: &mut VideoCaptureShowState,
    generation: u64,
    r: Result<Option<Result<ShownClip, CoreFailure>>, String>,
) {
    let outcome = match r {
        Ok(None) => Ok(CoreOutcome::Cancelled),
        Ok(Some(Ok(clip))) => Ok(CoreOutcome::Captured(clip)),
        Ok(Some(Err(f))) => Err(f),
        Err(_) => Err(CoreFailure::Unsupported),
    };
    finish_capture(s, generation, outcome);
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;
#[cfg(target_arch = "wasm32")]
pub use web::video_capture_show_effect;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::video_capture_show::{
        mark_running, take_request, CoreFailure, ShownClip, VideoCaptureShowState,
    };
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{File, HtmlVideoElement, Url};

    use super::{clip_from_browser, finish_from_pick, ACCEPT, CAPTURE};
    use crate::file_picker::{drive, ConvertFuture, WebPickHooks};
    use crate::mount::Applier;

    /// メタデータ待ちの間だけ持つ資源。
    struct Probe {
        video: HtmlVideoElement,
        url: String,
        reject: js_sys::Function,
    }

    thread_local! {
        /// 読み取り中の `<video>` と object URL を [`Applier::mount_id`] ごとに保持する。
        /// 全経路で [`release_probe`] を通して解放する。
        static PROBES: RefCell<HashMap<u64, Probe>> = RefCell::new(HashMap::new());
    }

    /// 資源を解放する（`src` を外す → `load()` で読み込みを止める → URL を revoke）。
    /// 取り出した `Probe` を返す（待ちを終わらせたい呼び出し側が `reject` を使う）。
    fn release_probe(mount_id: u64) -> Option<Probe> {
        let probe = PROBES.with(|cell| cell.borrow_mut().remove(&mount_id))?;
        let _ = probe.video.remove_attribute("src");
        probe.video.load();
        let _ = Url::revoke_object_url(&probe.url);
        Some(probe)
    }

    /// 破棄されたマウントの資源を解放する。`mount` 層から呼ばれる。
    /// 待ちは `reject` で確実に終わらせる（走りっぱなしの future を残さない）。
    /// `<input>` 側は `file_picker::release_mount` が解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(probe) = release_probe(mount_id) {
            let _ = probe.reject.call0(&JsValue::NULL);
        }
    }

    /// `File` のメタデータを読む。環境エラー以外は `Ok(inner)` を返し、失敗の種別は `inner` で運ぶ。
    async fn probe(mount_id: u64, file: File) -> Result<Result<ShownClip, CoreFailure>, String> {
        let size = file.size();
        if !size.is_finite() || size <= 0.0 {
            // 0 バイトでは URL を作らない。
            return Ok(clip_from_browser(size, f64::NAN, 0, 0));
        }
        let unreadable = || Ok(Err(CoreFailure::Unreadable));
        // 開始前にマウントが破棄されていたら URL を作らない（結果は apply で捨てられる）。
        if !crate::mount::is_live(mount_id) {
            return unreadable();
        }
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return Err("document".to_string());
        };
        let Ok(video) = document
            .create_element("video")
            .and_then(|e| e.dyn_into::<HtmlVideoElement>().map_err(JsValue::from))
        else {
            return Err("video".to_string());
        };
        video.set_muted(true);
        video.set_preload("metadata");
        let Ok(url) = Url::create_object_url_with_blob(&file) else {
            return unreadable();
        };
        // 同じマウントの前回分が残っていれば先に解放する。
        release_probe(mount_id);

        let once = web_sys::AddEventListenerOptions::new();
        once.set_once(true);
        let mut executor = |resolve: js_sys::Function, reject: js_sys::Function| {
            let ok = video
                .add_event_listener_with_callback_and_add_event_listener_options(
                    "loadedmetadata",
                    &resolve,
                    &once,
                )
                .and_then(|()| {
                    video.add_event_listener_with_callback_and_add_event_listener_options(
                        "error", &reject, &once,
                    )
                });
            PROBES.with(|cell| {
                cell.borrow_mut().insert(
                    mount_id,
                    Probe {
                        video: video.clone(),
                        url: url.clone(),
                        reject: reject.clone(),
                    },
                );
            });
            if ok.is_err() {
                let _ = reject.call0(&JsValue::NULL);
                return;
            }
            video.set_src(&url);
            // iOS Safari は `preload` を無視するため明示的に読み込みを始める。
            video.load();
        };
        let promise = js_sys::Promise::new(&mut executor);
        let loaded = JsFuture::from(promise).await;

        // 成功・失敗どちらでも、読み終えてから解放する。
        let duration = video.duration();
        let (w, h) = (video.video_width(), video.video_height());
        release_probe(mount_id);
        match loaded {
            Ok(_) => Ok(clip_from_browser(size, duration, w, h)),
            Err(_) => unreadable(),
        }
    }

    fn probe_boxed(mount_id: u64, file: File) -> ConvertFuture<Result<ShownClip, CoreFailure>> {
        Box::pin(probe(mount_id, file))
    }

    /// 動画の撮影画面用の [`WebPickHooks`]。
    const VIDEO_CAPTURE_HOOKS: WebPickHooks<VideoCaptureShowState, Result<ShownClip, CoreFailure>> =
        WebPickHooks {
            accept: Some(ACCEPT),
            capture: Some(CAPTURE),
            take_request,
            mark_picking: mark_running,
            convert: probe_boxed,
            finish: finish_from_pick,
        };

    /// `mount_with_effect` へ渡す effect。動画の撮影画面の要求を処理する（Issue #824）。
    pub fn video_capture_show_effect(
        state: &VideoCaptureShowState,
        applier: &Applier<VideoCaptureShowState>,
    ) {
        drive(state, applier, VIDEO_CAPTURE_HOOKS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::video_capture_show::{
        error_text, mark_running, request_capture, Phase,
    };

    fn running() -> (VideoCaptureShowState, u64) {
        let mut s = VideoCaptureShowState::default();
        request_capture(&mut s);
        mark_running(&mut s);
        let g = s.generation;
        (s, g)
    }

    #[test]
    fn zero_or_invalid_size_is_empty() {
        assert_eq!(clip_from_browser(0.0, 1.0, 1, 1), Err(CoreFailure::Empty));
        assert_eq!(
            clip_from_browser(f64::NAN, 1.0, 1, 1),
            Err(CoreFailure::Empty)
        );
        assert_eq!(clip_from_browser(-1.0, 1.0, 1, 1), Err(CoreFailure::Empty));
    }

    #[test]
    fn zero_dimension_is_unreadable() {
        assert_eq!(
            clip_from_browser(10.0, 1.0, 0, 240),
            Err(CoreFailure::Unreadable)
        );
        assert_eq!(
            clip_from_browser(10.0, 1.0, 320, 0),
            Err(CoreFailure::Unreadable)
        );
    }

    #[test]
    fn unusable_duration_becomes_none() {
        for d in [f64::NAN, f64::INFINITY, 0.0, -3.0] {
            let c = clip_from_browser(10.0, d, 320, 240).unwrap();
            assert_eq!(c.duration_ms, None, "{d}");
        }
    }

    #[test]
    fn duration_is_rounded_to_millis() {
        assert_eq!(
            clip_from_browser(10.0, 2.0, 320, 240).unwrap().duration_ms,
            Some(2000)
        );
        assert_eq!(
            clip_from_browser(10.0, 1.2346, 320, 240)
                .unwrap()
                .duration_ms,
            Some(1235)
        );
    }

    #[test]
    fn success_is_received_origin() {
        let c = clip_from_browser(4096.0, 2.0, 320, 240).unwrap();
        assert_eq!(c.origin, ClipOrigin::Received);
        assert_eq!((c.size_bytes, c.width, c.height), (4096, 320, 240));
    }

    #[test]
    fn huge_size_saturates_without_failing() {
        let c = clip_from_browser(f64::MAX, 1.0, 1, 1).unwrap();
        assert_eq!(c.size_bytes, u64::MAX);
    }

    #[test]
    fn cancel_is_not_a_failure() {
        let (mut s, g) = running();
        finish_from_pick(&mut s, g, Ok(None));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn environment_error_is_unsupported() {
        let (mut s, g) = running();
        finish_from_pick(&mut s, g, Err("x".to_string()));
        assert_eq!(s.result, Some(Err(CoreFailure::Unsupported)));
    }

    #[test]
    fn unreadable_is_a_failure() {
        let (mut s, g) = running();
        finish_from_pick(&mut s, g, Ok(Some(Err(CoreFailure::Unreadable))));
        assert_eq!(s.result, Some(Err(CoreFailure::Unreadable)));
        assert_ne!(error_text(&s), "");
    }

    #[test]
    fn captured_is_reflected() {
        let (mut s, g) = running();
        let clip = clip_from_browser(10.0, 1.0, 2, 3).unwrap();
        finish_from_pick(&mut s, g, Ok(Some(Ok(clip.clone()))));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Captured(clip))));
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn stale_generation_and_non_running_are_ignored() {
        let (mut s, g) = running();
        finish_from_pick(&mut s, g + 1, Ok(None));
        assert_eq!(s.result, None);
        assert_eq!(s.phase, Phase::Running);
        let mut idle = VideoCaptureShowState::default();
        finish_from_pick(&mut idle, 0, Ok(None));
        assert_eq!(idle.result, None);
    }

    #[test]
    fn failure_mapping_covers_all() {
        use VideoCaptureFailure as F;
        let pairs = [
            (F::Unsupported, CoreFailure::Unsupported),
            (F::PermissionDenied, CoreFailure::PermissionDenied),
            (F::NoCameraApp, CoreFailure::NoCameraApp),
            (F::NotForeground, CoreFailure::NotForeground),
            (F::Interrupted, CoreFailure::Interrupted),
            (F::Empty, CoreFailure::Empty),
            (F::Unreadable, CoreFailure::Unreadable),
            (F::Internal, CoreFailure::Internal),
        ];
        for (f, c) in pairs {
            assert_eq!(to_core_failure(f), c);
        }
    }
}
