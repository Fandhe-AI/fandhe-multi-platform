//! iOS 向けカメラのライブ映像実装（Issue #813、親 #168）。`ios/fandhe_ios_camera_preview_stream.m`
//! （`AVCaptureSession` + `AVCaptureVideoDataOutput`。`build.rs` が iOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `extern "C"` 3 関数で呼び、`.m` からの通知・フレームは `#[no_mangle]` の受け口 3 つで
//! 受ける。`device/camera_preview_stream.rs` の iOS 経路（`platform_camera_preview`）が [`implementation`] を
//! 呼ぶ。dispatch・共通コア画面・driver・trait 契約（`fandhe_device::camera_preview_stream`）は #812 の
//! 最終形のまま変えない。
//!
//! # 流れ
//!
//! 1. `start`: driver の `register` で `id` を得て `.m` の start を呼ぶ。許可の要求・構成・開始は `.m` が
//!    session queue で非同期に行い、結果は OS の通知（開始・中断・実行時エラー・背景遷移）で確定する。
//!    タイマーや `startRunning` 直後の状態読みで推測しない（開始・失敗の確定は通知に一本化）。受理されなければ `unregister` して `Err` を返す（登録を残さない）
//! 2. `.m` の通知は `fandhe_rust_camera_preview_stream_event`（コードの表は `event_from_code`）、
//!    フレームは `fandhe_rust_camera_preview_stream_frame` が受け、`deliver_event` / `deliver_frame` で届ける。
//!    届いた先で driver が `QueueSink::wake` → `crate::ios::request_repaint` を呼ぶ
//! 3. ズームの範囲検証（`zoom_target`）は `fandhe_rust_camera_preview_stream_zoom_target` の 1 か所で行う。
//!    `.m` は範囲を OS から読み、目標値を Rust に決めてもらってから `videoZoomFactor` へ設定する
//! 4. 停止（session の drop）: `unregister` → `.m` の stop（OS 停止・delegate の終了待ち・解放を完了して戻る）
//!
//! 縮小は [`frame_from_raw`] が `downscale_packed`（BGRA）で行う。ホストでテストできるよう純関数は
//! 常時コンパイルし、FFI は `cfg(ios)` に閉じる。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 失敗・非対応・権限拒否・カメラなし（シミュレータ）を成功へ畳まない、同期の失敗で登録を残さない、
//!   停止・失敗・背景の全経路で入出力と購読を解放する、解放後のフレームを捨てる、画素を保存・送信・ログ出力しない
//! - 守らない: フレームの間隔、前面・背面の切り替え、露出・焦点、複数画面での同時利用、
//!   他アプリとの取り合いで OS が止めた後の自動の再開（iOS の中断の終了を除く）、映像の向き
//!
//! # 既知の天井（実機確認 #808 で確かめる）
//!
//! - 32BGRA が出力の対応形式に含まれるか、preset 640x480 が通るか
//! - 中断の理由が届く順、`startRunning` の同期ブロック中に画面を離れたときの停止の待ち
//! - シミュレータにはカメラが無く、開始失敗（カメラなし）になる

use fandhe_device::camera_preview_stream::{
    downscale_packed, zoom_target, PackedLayout, PreviewFrame, PreviewStartError, StartFailure,
    ZoomFailure, ZoomStep,
};
#[cfg(target_os = "ios")]
use fandhe_device::camera_preview_stream::{CameraPreview, PreviewSession};

/// `.m` の start / set_zoom の受理コード。0 だけが受理。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const ACCEPTED: i32 = 0;

/// 目標倍率の判定結果コード（0 成功 / 1 非対応 / 2 失敗）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const ZOOM_OK: i32 = 0;
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const ZOOM_UNSUPPORTED: i32 = 1;
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const ZOOM_FAILED: i32 = 2;

/// `.m` の start の戻り値を契約へ写す（0 受理 / 他 受理されない）。`Ok` へ畳まない。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn start_result(code: i32) -> Result<(), PreviewStartError> {
    if code == ACCEPTED {
        Ok(())
    } else {
        Err(PreviewStartError::Failed(StartFailure::Other))
    }
}

/// `.m` の set_zoom の戻り値を契約へ写す（0 受理 / 他 失敗）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn zoom_accept_result(code: i32) -> Result<(), ZoomFailure> {
    if code == ACCEPTED {
        Ok(())
    } else {
        Err(ZoomFailure::Failed)
    }
}

/// `.m` が OS から読んだ範囲 `min`〜`max` と段 `step`（1 / 2 倍）から目標倍率を決める。
/// 戻りは `(コード, 目標値)`。未知の段は失敗。範囲の検証は `zoom_target` の 1 か所に保つ
/// （丸めの前に検証する。契約表 5）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn zoom_target_code(step: i32, min: f32, max: f32) -> (i32, f32) {
    let step = match step {
        1 => ZoomStep::One,
        2 => ZoomStep::Two,
        _ => return (ZOOM_FAILED, 0.0),
    };
    match zoom_target(step, Some((min, max))) {
        Ok(v) => (ZOOM_OK, v),
        Err(ZoomFailure::Unsupported) => (ZOOM_UNSUPPORTED, 0.0),
        Err(ZoomFailure::Failed) => (ZOOM_FAILED, 0.0),
    }
}

/// OS バッファ（32BGRA、行パディング付き）を縮めて `PreviewFrame` にする。
/// 寸法 0・上限超・stride 不足・長さ不足は `None`（捨てる）。ズーム値は生のまま渡し、
/// 検証は driver の `checked_zoom` が 1 か所で行う。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn frame_from_raw(
    data: &[u8],
    width: u32,
    height: u32,
    row_stride: usize,
    zoom: Option<f32>,
) -> Option<PreviewFrame> {
    let frame = downscale_packed(width, height, row_stride, PackedLayout::Bgra8, data)?;
    Some(PreviewFrame {
        source_width: width,
        source_height: height,
        frame,
        zoom,
    })
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use super::{
        start_result, zoom_accept_result, CameraPreview, PreviewSession, PreviewStartError,
        ZoomFailure, ZoomStep,
    };
    use crate::device::camera_preview_stream::{register, unregister};
    use fandhe_device::camera_preview_stream::PreviewSink;

    // SAFETY: `ios/fandhe_ios_camera_preview_stream.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_camera_preview_stream_start(id: u64) -> i32;
        fn fandhe_ios_camera_preview_stream_set_zoom(id: u64, step: i32) -> i32;
        fn fandhe_ios_camera_preview_stream_stop(id: u64);
    }

    struct IosCameraPreview;

    impl CameraPreview for IosCameraPreview {
        #[allow(unsafe_code)]
        fn start(
            &self,
            sink: Arc<dyn PreviewSink>,
        ) -> Result<Box<dyn PreviewSession>, PreviewStartError> {
            let id = register(sink);
            // SAFETY: 値渡しの `id` のみ。`.m` は許可・構成・開始を自前の queue で非同期に行い、
            // Rust のポインタは保持しない。
            let code = unsafe { fandhe_ios_camera_preview_stream_start(id) };
            if let Err(e) = start_result(code) {
                // 同期で受理されなかった場合は登録を残さない（OS 資源は `.m` が確保していない）。
                unregister(id);
                return Err(e);
            }
            Ok(Box::new(IosPreviewSession { id }))
        }
    }

    /// 開始した映像 1 件。drop で停止・解放する。
    struct IosPreviewSession {
        id: u64,
    }

    impl PreviewSession for IosPreviewSession {
        #[allow(unsafe_code)]
        fn set_zoom(&self, step: ZoomStep) -> Result<(), ZoomFailure> {
            let step = match step {
                ZoomStep::One => 1,
                ZoomStep::Two => 2,
            };
            // SAFETY: 値渡しのみ。`.m` は `id` を照合して受理し、適用は session queue で非同期に行う。
            zoom_accept_result(unsafe { fandhe_ios_camera_preview_stream_set_zoom(self.id, step) })
        }
    }

    impl Drop for IosPreviewSession {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 登録解除 → OS 停止（契約 doc の順）。以後 `.m` が届ける呼び出しは driver が捨てる。
            unregister(self.id);
            // SAFETY: 値渡しのみ。`.m` は停止・解放・実行中 delegate の終了待ちを完了してから戻る。
            // 呼び出し元は UI スレッドで、`.m` の queue から main へ `dispatch_sync` する経路は無い。
            unsafe { fandhe_ios_camera_preview_stream_stop(self.id) }
        }
    }

    /// iOS の `CameraPreview` 実装を返す（`device/camera_preview_stream.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn CameraPreview> {
        Arc::new(IosCameraPreview)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` の通知を届ける受け口（コードの表は `fandhe_device::camera_preview_stream::event_from_code`）。
/// 任意のスレッドから呼ばれ、panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_camera_preview_stream_event(id: u64, code: i32, detail: i32) {
    use fandhe_device::camera_preview_stream::event_from_code;
    super::camera_preview_stream::deliver_event(id, event_from_code(code, detail));
}

/// `.m` のフレームを縮めて届ける受け口。`data` は `CVPixelBufferLockBaseAddress` 中の先頭で、
/// この呼び出しの間だけ有効。不正な入力は捨てる。任意のスレッド（`.m` の video queue）から呼ばれ、
/// panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_camera_preview_stream_frame(
    id: u64,
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    row_stride: usize,
    zoom: f32,
    has_zoom: bool,
) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: `.m` は `len`（= bytesPerRow * height）バイトの読み取りロック済みバッファを渡す。
    // 本関数の戻りまでしか参照せず、保持しない。
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    if let Some(frame) = frame_from_raw(slice, width, height, row_stride, has_zoom.then_some(zoom))
    {
        super::camera_preview_stream::deliver_frame(id, frame);
    }
}

/// `.m` が OS から読んだズーム範囲と段から目標倍率を決める受け口（検証の SSOT は Rust 側）。
/// 戻りは 0 成功（`out` に目標値）/ 1 非対応 / 2 失敗。`out` が NULL なら失敗。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_camera_preview_stream_zoom_target(
    step: i32,
    min: f32,
    max: f32,
    out: *mut f32,
) -> i32 {
    if out.is_null() {
        return ZOOM_FAILED;
    }
    let (code, value) = zoom_target_code(step, min, max);
    if code == ZOOM_OK {
        // SAFETY: `out` は非 NULL で、`.m` が呼び出しの間有効な `float` を指す。
        unsafe { *out = value };
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::camera_preview_stream::{
        CameraPreview, PreviewEvent, PreviewSink, UnsupportedCameraPreview, MAX_SOURCE_SIDE,
    };
    use std::sync::Arc;

    struct NopSink;
    impl PreviewSink for NopSink {
        fn frame(&self, _: PreviewFrame) {}
        fn event(&self, _: PreviewEvent) {}
    }

    #[test]
    fn host_fallback_never_succeeds() {
        let r = UnsupportedCameraPreview.start(Arc::new(NopSink));
        assert!(matches!(r, Err(PreviewStartError::Unsupported)));
    }

    #[test]
    fn start_result_only_zero_is_ok() {
        assert!(start_result(0).is_ok());
        for code in [1, -1, 2, i32::MAX, i32::MIN] {
            assert!(matches!(
                start_result(code),
                Err(PreviewStartError::Failed(StartFailure::Other))
            ));
        }
    }

    #[test]
    fn zoom_accept_result_only_zero_is_ok() {
        assert!(zoom_accept_result(0).is_ok());
        assert_eq!(zoom_accept_result(1), Err(ZoomFailure::Failed));
        assert_eq!(zoom_accept_result(-1), Err(ZoomFailure::Failed));
    }

    #[test]
    fn zoom_target_code_clamps_and_validates() {
        assert_eq!(zoom_target_code(1, 1.0, 5.0), (ZOOM_OK, 1.0));
        assert_eq!(zoom_target_code(2, 1.0, 5.0), (ZOOM_OK, 2.0));
        // 上限で clamp する。
        assert_eq!(zoom_target_code(2, 1.0, 1.5), (ZOOM_OK, 1.5));
        // 未知の段・NaN・min > max・min <= 0 は失敗。
        assert_eq!(zoom_target_code(0, 1.0, 5.0).0, ZOOM_FAILED);
        assert_eq!(zoom_target_code(3, 1.0, 5.0).0, ZOOM_FAILED);
        assert_eq!(zoom_target_code(1, f32::NAN, 5.0).0, ZOOM_FAILED);
        assert_eq!(zoom_target_code(1, 1.0, f32::INFINITY).0, ZOOM_FAILED);
        assert_eq!(zoom_target_code(1, 3.0, 2.0).0, ZOOM_FAILED);
        assert_eq!(zoom_target_code(1, 0.0, 2.0).0, ZOOM_FAILED);
    }

    fn bgra(w: usize, h: usize, stride: usize, px: [u8; 4]) -> Vec<u8> {
        let mut v = vec![0u8; stride * (h - 1) + w * 4];
        for y in 0..h {
            for x in 0..w {
                v[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&px);
            }
        }
        v
    }

    #[test]
    fn frame_from_raw_reorders_bgra_and_keeps_source_size() {
        // B=10, G=20, R=30, A=255 -> RGBA は 30, 20, 10, 255。
        let data = bgra(4, 2, 20, [10, 20, 30, 255]);
        let f = frame_from_raw(&data, 4, 2, 20, Some(2.5)).expect("frame");
        assert_eq!((f.source_width, f.source_height), (4, 2));
        assert_eq!(&f.frame.rgba[..4], &[30, 20, 10, 255]);
        // ズーム値は丸めず生のまま渡す。
        assert_eq!(f.zoom, Some(2.5));
        assert_eq!(
            frame_from_raw(&data, 4, 2, 20, None).expect("frame").zoom,
            None
        );
    }

    #[test]
    fn frame_from_raw_downscales_640x480() {
        let data = bgra(640, 480, 640 * 4, [0, 0, 0, 255]);
        let f = frame_from_raw(&data, 640, 480, 640 * 4, None).expect("frame");
        assert_eq!((f.frame.width, f.frame.height), (320, 240));
        assert_eq!((f.source_width, f.source_height), (640, 480));
    }

    #[test]
    fn frame_from_raw_rejects_bad_input() {
        let data = bgra(4, 2, 16, [1, 2, 3, 4]);
        // 寸法 0。
        assert!(frame_from_raw(&data, 0, 2, 16, None).is_none());
        assert!(frame_from_raw(&data, 4, 0, 16, None).is_none());
        // stride 不足。
        assert!(frame_from_raw(&data, 4, 2, 8, None).is_none());
        // 長さ不足。
        assert!(frame_from_raw(&data[..data.len() - 1], 4, 2, 16, None).is_none());
        // 上限超（巨大確保を避けて None）。
        assert!(frame_from_raw(&data, MAX_SOURCE_SIDE + 1, 2, 16, None).is_none());
    }
}
