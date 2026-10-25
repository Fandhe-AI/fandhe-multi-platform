//! Android 向けカメラのライブ映像実装（Issue #814、親 #168。共通コア・契約・effect ドライバは #812）。
//! `device/camera_preview_stream.rs::platform_camera_preview` の android 経路から UI スレッドで呼ばれ、
//! Java の `CameraPreviewHelper`（Camera2 + `ImageReader`）経由で背面カメラ（無ければ先頭）の映像を受け、
//! 最新の 1 枚を `downscale_yuv420` で長辺 320 px 以下へ縮めてドライバの受け口へ渡す。契約は
//! `fandhe_device::camera_preview_stream`。
//!
//! # 結果コード（数値表の SSOT は Rust の `event_from_code`、定数の並びは Java 側 `CameraPreviewHelper`）
//!
//! - `MainActivity.cameraPreviewStart(long)` の戻り値: 0 = 受け付けた（開くのは非同期）/ 他 = 失敗
//! - `MainActivity.cameraPreviewZoomRange(long)` の戻り値: `null` = 失敗 / 長さ 0 = 非対応 /
//!   長さ 2 = `{min, max}`。それ以外の長さは失敗
//! - `MainActivity.cameraPreviewSetZoom(long, float)` の戻り値: 0 = 受け付けた / 1 = 非対応 / 他 = 失敗。
//!   適用は Java のカメラスレッドで非同期に行い、適用の失敗はイベント 6 で返る
//! - `nativeOnCameraPreviewEvent(id, code, detail)` は `event_from_code` の表のとおり
//! - `nativeOnCameraPreviewFrame(...)` は `YUV_420_888` の 3 面と stride・OS から読んだズーム値を受ける
//!
//! # 方式: id 照合 + 解放の集約（契約表 1・2・3・9・11）
//!
//! ドライバは OS 実装へ id を渡さないため、本実装が [`register`](super::camera_preview_stream::register)
//! で自分用の id を取り、Java へ渡して数える。通知は id を添えて `deliver_*` へ流れ、登録表に無い id
//! （停止後・Activity 再生成前の古い id）は捨てられる（id はプロセス内で単調増加）。
//! 停止（`AndroidSession` の drop）は「登録解除 → Java の停止」の順で、Java の解放完了は待たない。
//! Rust は OS のバッファを JNI 呼び出しの外へ持ち出さず、遅れて届いた呼び出しは登録表が捨てる。
//! 待つとカメラスレッドとの間でデッドロックの余地が生まれる。開始・停止・ズーム・解放は Java 側の
//! 1 本のカメラスレッドに直列化する。ロック取得順は Java → Rust の一方向で、Java は native を
//! 自分のロックの外で呼ぶ（`ACTIVE` を握ったまま呼ばれる `set_zoom` から Rust のロックを取る
//! コールバックは起きない）。
//!
//! # OS を正にする範囲
//!
//! 開始・失敗・停止は OS のコールバックで確定し、期限タイマーは張らない。ズーム値はフレームごとに
//! `CaptureResult.CONTROL_ZOOM_RATIO` から読んだ値を運ぶ（API 30 未満・取れないときは `None`）。
//!
//! # 他 OS との差
//!
//! Android は「中断」（code 1）を送らない。他アプリに取られた等で OS が止めたら終端（実行中の失敗）
//! にし、自動では再開しない。背景へ移ると `onPause` で停止する（許可ダイアログ中を除く）。
//!
//! 守る範囲: 1 プロセス内の順序、失敗・非対応を成功へ畳まないこと、全終了経路でのカメラ解放、
//! 解放後の通知の破棄。守らない範囲: フレームの間隔・時刻、前面・背面の切り替え、端末の向きへの
//! 追従、OS が止めた後の自動再開、複数プロセス、OS 側の非同期確定との時間差による一時的な食い違い。
//! id・画素・例外の内容・OS のエラー文言はログ・エラーに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `cameraPreviewStart` を経た後でしか呼ばれず、
//! feature `device` が無効のビルドではその経路に届かない（sleep_lock_events と同じ根拠）。

use std::sync::Arc;

use fandhe_device::camera_preview_stream::CameraPreview;
#[cfg(not(target_os = "android"))]
use fandhe_device::camera_preview_stream::UnsupportedCameraPreview;
use fandhe_device::camera_preview_stream::{
    checked_zoom, downscale_yuv420, PreviewFrame, ZoomFailure,
};
#[cfg(target_os = "android")]
use fandhe_device::camera_preview_stream::{
    event_from_code, zoom_target, PreviewSession, PreviewSink, PreviewStartError, StartFailure,
    ZoomStep,
};

/// `cameraPreviewStart` の成功コード（SSOT は Java 側 `CameraPreviewHelper.START_OK`）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
const START_OK: i32 = 0;

/// 開始の結果コードの判定。未知の値は失敗で、成功へは倒さない。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_start(code: i32) -> bool {
    code == START_OK
}

/// `cameraPreviewZoomRange` の戻り値を範囲へ写像する。`None` = null（失敗）、長さ 0 = 非対応。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_zoom_range(raw: Option<&[f32]>) -> Result<(f32, f32), ZoomFailure> {
    match raw {
        None => Err(ZoomFailure::Failed),
        Some([]) => Err(ZoomFailure::Unsupported),
        Some([min, max]) => Ok((*min, *max)),
        Some(_) => Err(ZoomFailure::Failed),
    }
}

/// `cameraPreviewSetZoom` の戻り値を写像する。0 = 受け付けた / 1 = 非対応 / 他 = 失敗。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode_set_zoom(code: i32) -> Result<(), ZoomFailure> {
    match code {
        0 => Ok(()),
        1 => Err(ZoomFailure::Unsupported),
        _ => Err(ZoomFailure::Failed),
    }
}

/// JNI から届いた `YUV_420_888` の 3 面を縮小済みの [`PreviewFrame`] にする。
/// 負の寸法・stride、`downscale_yuv420` が拒んだフレームは `None`（捨てる。数えず、成功にも失敗にも畳まない）。
/// ズームは非有限・非正なら `None`。
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn frame_from_planes(
    width: i32,
    height: i32,
    y: &[u8],
    y_row_stride: i32,
    u: &[u8],
    v: &[u8],
    uv_row_stride: i32,
    uv_pixel_stride: i32,
    zoom: f32,
) -> Option<PreviewFrame> {
    let width = u32::try_from(width).ok()?;
    let height = u32::try_from(height).ok()?;
    let frame = downscale_yuv420(
        width,
        height,
        y,
        usize::try_from(y_row_stride).ok()?,
        u,
        v,
        usize::try_from(uv_row_stride).ok()?,
        usize::try_from(uv_pixel_stride).ok()?,
    )?;
    Some(PreviewFrame {
        source_width: width,
        source_height: height,
        frame,
        zoom: checked_zoom(zoom),
    })
}

/// 実行中の映像。drop で登録を外してから Java の停止を呼ぶ。
#[cfg(target_os = "android")]
struct AndroidSession {
    id: u64,
}

#[cfg(target_os = "android")]
impl Drop for AndroidSession {
    fn drop(&mut self) {
        // 先に登録を外し、以後の遅れたフレーム・通知を捨てさせてから Java を止める。
        super::camera_preview_stream::unregister(self.id);
        // Java の onDestroy が先に片付けている場合や知らない id は何もしない（冪等）。失敗は無視してよい。
        let _ = call_activity(Call::Stop(self.id));
    }
}

#[cfg(target_os = "android")]
impl PreviewSession for AndroidSession {
    fn set_zoom(&self, step: ZoomStep) -> Result<(), ZoomFailure> {
        let range = match call_activity(Call::ZoomRange(self.id)) {
            Some(Ret::Floats(r)) => r,
            _ => return Err(ZoomFailure::Failed),
        };
        let range = decode_zoom_range(range.as_deref())?;
        let target = zoom_target(step, Some(range))?;
        match call_activity(Call::SetZoom(self.id, target)) {
            Some(Ret::Int(code)) => decode_set_zoom(code),
            _ => Err(ZoomFailure::Failed),
        }
    }
}

#[cfg(target_os = "android")]
struct AndroidCameraPreview;

#[cfg(target_os = "android")]
impl CameraPreview for AndroidCameraPreview {
    fn start(
        &self,
        sink: Arc<dyn PreviewSink>,
    ) -> Result<Box<dyn PreviewSession>, PreviewStartError> {
        let id = super::camera_preview_stream::register(sink);
        match call_activity(Call::Start(id)) {
            Some(Ret::Int(code)) if decode_start(code) => Ok(Box::new(AndroidSession { id })),
            _ => {
                // 失敗したら登録表にも Java 側にも何も残さない。
                super::camera_preview_stream::unregister(id);
                let _ = call_activity(Call::Stop(id));
                Err(PreviewStartError::Failed(StartFailure::Other))
            }
        }
    }
}

/// Java へ渡す呼び出しの種類。
#[cfg(target_os = "android")]
enum Call {
    Start(u64),
    Stop(u64),
    ZoomRange(u64),
    SetZoom(u64, f32),
}

/// Java の戻り値。
#[cfg(target_os = "android")]
enum Ret {
    Int(i32),
    /// `float[]`。`None` = null。
    Floats(Option<Vec<f32>>),
}

/// `MainActivity.cameraPreviewStart(long)` / `cameraPreviewStop(long)` / `cameraPreviewZoomRange(long)` /
/// `cameraPreviewSetZoom(long, float)` を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_sleep_lock_events.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(call: Call) -> Option<Ret> {
    use super::android::JniCallFailed;
    use jni::objects::{JFloatArray, JValue};
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<Ret, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        match call {
            Call::Start(id) => Ok(Ret::Int(
                env.call_method(
                    &activity,
                    jni::jni_str!("cameraPreviewStart"),
                    jni::jni_sig!((long) -> int),
                    &[JValue::Long(id as jni::sys::jlong)],
                )?
                .i()?,
            )),
            Call::Stop(id) => {
                env.call_method(
                    &activity,
                    jni::jni_str!("cameraPreviewStop"),
                    jni::jni_sig!((long) -> ()),
                    &[JValue::Long(id as jni::sys::jlong)],
                )?;
                Ok(Ret::Int(0))
            }
            Call::ZoomRange(id) => {
                let ret = env.call_method(
                    &activity,
                    jni::jni_str!("cameraPreviewZoomRange"),
                    jni::jni_sig!((long) -> float[]),
                    &[JValue::Long(id as jni::sys::jlong)],
                )?;
                let arr = env.cast_local::<JFloatArray>(ret.l()?)?;
                if arr.is_null() {
                    return Ok(Ret::Floats(None));
                }
                // 長さを検証してから読む。0 と 2 以外は `decode_zoom_range` が失敗にする。
                let len = arr.len(env)?;
                if len > 2 {
                    return Ok(Ret::Floats(Some(vec![0.0; 3])));
                }
                let mut v = vec![0f32; len];
                arr.get_region(env, 0, &mut v)?;
                Ok(Ret::Floats(Some(v)))
            }
            Call::SetZoom(id, ratio) => Ok(Ret::Int(
                env.call_method(
                    &activity,
                    jni::jni_str!("cameraPreviewSetZoom"),
                    jni::jni_sig!((long, float) -> int),
                    &[JValue::Long(id as jni::sys::jlong), JValue::Float(ratio)],
                )?
                .i()?,
            )),
        }
    })
    .ok()
}

/// `CameraPreviewHelper.nativeOnCameraPreviewEvent`（Java 側）から呼ばれる JNI エクスポート。
/// 数値は `event_from_code` の表。登録表に無い id（停止後・再生成前の古い id）の通知は捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため
/// 他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_CameraPreviewHelper_nativeOnCameraPreviewEvent<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    code: jni::sys::jint,
    detail: jni::sys::jint,
) {
    super::camera_preview_stream::deliver_event(id as u64, event_from_code(code, detail));
}

/// `CameraPreviewHelper.nativeOnCameraPreviewFrame`（Java 側）から呼ばれる JNI エクスポート。
/// 3 面の direct `ByteBuffer` を読んで縮小し、ドライバへ渡す。`Image` は Java 側でこの呼び出しが
/// 戻るまで close されない。不正なフレームは捨てる。
/// 各面は Java 側が `slice()` で position 位置から remaining 分に切り出したもので、capacity が実データ長。
/// 必要長に満たない面は `frame_from_planes`（`downscale_yuv420`）が変換の前に弾く。
///
/// # Safety
/// `unsafe` は direct buffer から slice を作る 1 箇所に限る（根拠は本文の SAFETY）。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名で、他の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code, clippy::too_many_arguments)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_CameraPreviewHelper_nativeOnCameraPreviewFrame<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    id: jni::sys::jlong,
    width: jni::sys::jint,
    height: jni::sys::jint,
    y: jni::objects::JByteBuffer<'caller>,
    y_row_stride: jni::sys::jint,
    u: jni::objects::JByteBuffer<'caller>,
    v: jni::objects::JByteBuffer<'caller>,
    uv_row_stride: jni::sys::jint,
    uv_pixel_stride: jni::sys::jint,
    zoom: jni::sys::jfloat,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        if y.is_null() || u.is_null() || v.is_null() {
            return Ok(());
        }
        // direct でないバッファはアドレスが取れず Err になり、捨てる。
        let (Ok(py), Ok(pu), Ok(pv)) = (
            env.get_direct_buffer_address(&y),
            env.get_direct_buffer_address(&u),
            env.get_direct_buffer_address(&v),
        ) else {
            return Ok(());
        };
        let (Ok(ly), Ok(lu), Ok(lv)) = (
            env.get_direct_buffer_capacity(&y),
            env.get_direct_buffer_capacity(&u),
            env.get_direct_buffer_capacity(&v),
        ) else {
            return Ok(());
        };
        if py.is_null() || pu.is_null() || pv.is_null() {
            return Ok(());
        }
        // SAFETY: アドレスと容量は direct な `ByteBuffer` のもので、Java 側の `Image` はこの native 呼び出しが
        // 戻るまで close されず、`ImageReader` の解放もフレーム受信と同じカメラスレッドに直列化されているため、
        // 3 面とも本関数の間は有効。slice は `frame_from_planes` が縮小済みの `Vec` を作った後に捨て、
        // 関数の外へ持ち出さない。
        let frame = unsafe {
            frame_from_planes(
                width,
                height,
                std::slice::from_raw_parts(py, ly),
                y_row_stride,
                std::slice::from_raw_parts(pu, lu),
                std::slice::from_raw_parts(pv, lv),
                uv_row_stride,
                uv_pixel_stride,
                zoom,
            )
        };
        if let Some(frame) = frame {
            super::camera_preview_stream::deliver_frame(id as u64, frame);
        }
        Ok(())
    });
    let _ = outcome.into_outcome();
}

/// Android の `CameraPreview` 実装を返す（`device/camera_preview_stream.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn CameraPreview> {
    Arc::new(AndroidCameraPreview)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> Arc<dyn CameraPreview> {
    Arc::new(UnsupportedCameraPreview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::camera_preview_stream::{PreviewEvent, PreviewSink, PreviewStartError};

    struct NopSink;
    impl PreviewSink for NopSink {
        fn frame(&self, _: PreviewFrame) {}
        fn event(&self, _: PreviewEvent) {}
    }

    #[test]
    fn host_implementation_is_unsupported() {
        let r = implementation().start(Arc::new(NopSink));
        assert!(matches!(r, Err(PreviewStartError::Unsupported)));
    }

    #[test]
    fn decode_start_only_zero_succeeds() {
        assert!(decode_start(0));
        assert!(!decode_start(1));
        assert!(!decode_start(-1));
    }

    #[test]
    fn decode_zoom_range_maps_lengths() {
        assert_eq!(decode_zoom_range(None), Err(ZoomFailure::Failed));
        assert_eq!(decode_zoom_range(Some(&[])), Err(ZoomFailure::Unsupported));
        assert_eq!(decode_zoom_range(Some(&[1.0, 8.0])), Ok((1.0, 8.0)));
        assert_eq!(decode_zoom_range(Some(&[1.0])), Err(ZoomFailure::Failed));
        assert_eq!(
            decode_zoom_range(Some(&[1.0, 2.0, 3.0])),
            Err(ZoomFailure::Failed)
        );
    }

    #[test]
    fn decode_set_zoom_maps_codes() {
        assert_eq!(decode_set_zoom(0), Ok(()));
        assert_eq!(decode_set_zoom(1), Err(ZoomFailure::Unsupported));
        assert_eq!(decode_set_zoom(2), Err(ZoomFailure::Failed));
        assert_eq!(decode_set_zoom(-1), Err(ZoomFailure::Failed));
    }

    fn planes_4x4() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (vec![128; 16], vec![128; 4], vec![128; 4])
    }

    #[test]
    fn frame_from_planes_accepts_valid_frame() {
        let (y, u, v) = planes_4x4();
        let f = frame_from_planes(4, 4, &y, 4, &u, &v, 2, 1, 2.0).expect("valid");
        assert_eq!((f.source_width, f.source_height), (4, 4));
        assert_eq!((f.frame.width, f.frame.height), (4, 4));
        assert_eq!(f.frame.rgba.len(), 4 * 4 * 4);
        assert_eq!(f.zoom, Some(2.0));
    }

    #[test]
    fn frame_from_planes_drops_non_finite_zoom_but_keeps_frame() {
        let (y, u, v) = planes_4x4();
        let f = frame_from_planes(4, 4, &y, 4, &u, &v, 2, 1, f32::NAN).expect("valid");
        assert_eq!(f.zoom, None);
    }

    #[test]
    fn frame_from_planes_rejects_negative_and_short_input() {
        let (y, u, v) = planes_4x4();
        assert!(frame_from_planes(-1, 4, &y, 4, &u, &v, 2, 1, 1.0).is_none());
        assert!(frame_from_planes(4, -4, &y, 4, &u, &v, 2, 1, 1.0).is_none());
        assert!(frame_from_planes(4, 4, &y, -4, &u, &v, 2, 1, 1.0).is_none());
        assert!(frame_from_planes(4, 4, &y, 4, &u, &v, 2, -1, 1.0).is_none());
        // stride が幅に満たない・面が短い・pixel stride が 0。
        assert!(frame_from_planes(4, 4, &y, 3, &u, &v, 2, 1, 1.0).is_none());
        assert!(frame_from_planes(4, 4, &y[..8], 4, &u, &v, 2, 1, 1.0).is_none());
        assert!(frame_from_planes(4, 4, &y, 4, &u, &v, 2, 0, 1.0).is_none());
        assert!(frame_from_planes(0, 4, &y, 4, &u, &v, 2, 1, 1.0).is_none());
    }
}
