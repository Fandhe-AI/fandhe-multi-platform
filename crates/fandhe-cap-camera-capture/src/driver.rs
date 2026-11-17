//! `fandhe_core::screens::camera_capture` の effect ドライバ（camera-capture 機能 crate の `driver`）。
//!
//! 画面の `CapturePhase::Requested` を観測して [`Camera`] で撮影し、`CompletionSender` 経由で
//! `mark_capturing` / `finish_capture` を送る。呼び出し元は `fandhe-gallery` の `screens/registry.rs` の
//! `effect "camera_capture"` 行。画面の状態型を参照してよいのは本モジュールだけ
//! （契約・OS 実装は `crate` 直下・`android` / `ios`。`consumer-app-structure.md` §8.1）。

use std::sync::Arc;

use fandhe_core::screens::camera_capture::{
    CameraCaptureState, CaptureFailure, CapturePhase, Thumbnail, cancel_capture, finish_capture,
    mark_capturing, thumbnail_from_rgba,
};
use fandhe_runtime::CompletionSender;

use crate::{Camera, CameraError, platform_camera_capture};
#[cfg(test)]
use crate::{DummyCamera, Image};

/// [`CameraError`] を `fandhe_core::screens::camera_capture::CaptureFailure` へ変換する。
/// `fandhe-core` へ内部構造を漏らさず、`Other` の場合のみ `Display` の文言を転送する
/// （security.md A09 準拠）。
fn to_capture_failure(e: CameraError) -> CaptureFailure {
    match e {
        CameraError::PermissionDenied(_) => CaptureFailure::PermissionDenied,
        CameraError::Unsupported(_) => CaptureFailure::Unsupported,
        other => CaptureFailure::Other(other.to_string()),
    }
}

/// [`Camera::capture`] を呼び、成功時は [`thumbnail_from_rgba`] で長辺 160px の実画素へ縮小する。
/// 間引きに失敗した場合（[`Image`] の寸法・バイト長が矛盾する不正データ）は
/// `CaptureFailure::Other` として扱う（契約違反があっても panic しない防御。coding-rust.md）。
pub fn capture_thumbnail(camera: &dyn Camera) -> Result<Thumbnail, CaptureFailure> {
    let image = camera.capture().map_err(to_capture_failure)?;
    thumbnail_from_rgba(image.width, image.height, &image.rgba)
        .ok_or_else(|| CaptureFailure::Other("画像データが不正です".to_string()))
}

/// `Requested` のときのみ撮影を起動する。`mark_capturing` を同期送信してから
/// 別スレッドで [`Camera::capture`] を実行し、完了時に `finish_capture` を送信する
/// （`DummyCamera::capture` 自体は同期だが、Android/iOS の実カメラが非同期コールバックのため、
/// 経路を統一する設計判断）。
///
/// 送信失敗（受信側 `ProxyApp` がドロップ済み）は `let _ =` で無視する
/// （panic を API 契約にしない、coding-rust.md 準拠）。Android の Activity 再生成時も
/// 同じ経路で無視される（モジュール doc「Activity 再生成と完了の配送（#881 の判断）」参照）。
fn spawn_capture(
    state: &CameraCaptureState,
    sender: &CompletionSender<CameraCaptureState>,
    camera: Arc<dyn Camera>,
) {
    if !matches!(state.phase, CapturePhase::Requested) {
        return;
    }
    let _ = sender.send(Box::new(mark_capturing));

    let sender = sender.clone();
    std::thread::spawn(move || {
        // `CameraError::Cancelled` は利用者による正常な操作（`UIImagePickerController`
        // のキャンセル等）であり、`to_capture_failure` の catch-all で `Other` に丸めて
        // `Failed` として表示すると内部識別子入りの失敗表示になってしまう。ここで先に分岐し、
        // `Capturing → Idle` へ静かに戻す。
        match camera.capture() {
            Err(CameraError::Cancelled(_)) => {
                let _ = sender.send(Box::new(cancel_capture));
            }
            outcome => {
                let result = outcome.map_err(to_capture_failure).and_then(|image| {
                    thumbnail_from_rgba(image.width, image.height, &image.rgba)
                        .ok_or_else(|| CaptureFailure::Other("画像データが不正です".to_string()))
                });
                let _ = sender.send(Box::new(move |state: &mut CameraCaptureState| {
                    finish_capture(state, result);
                }));
            }
        }
    });
}

/// 画面カタログの `effect "camera_capture"` 行から呼ばれる effect ドライバ
/// （`fn(&S, &CompletionSender<S>)`）。[`platform_camera_capture`] が返す実装を撮影対象にする。
pub fn camera_capture_effect(
    state: &CameraCaptureState,
    sender: &CompletionSender<CameraCaptureState>,
) {
    spawn_capture(state, sender, platform_camera_capture());
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    use fandhe_runtime::{Completion, RepaintWaker};

    use super::*;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<CameraCaptureState>,
        Receiver<Completion<CameraCaptureState>>,
    ) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    /// 常に固定結果を返すモック [`Camera`]。
    struct MockCamera(Result<Image, CameraError>);

    impl Camera for MockCamera {
        fn capture(&self) -> Result<Image, CameraError> {
            self.0.clone()
        }
    }

    fn checkerboard_image(width: u32, height: u32) -> Image {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _y in 0..height {
            for x in 0..width {
                let v = if x % 2 == 0 { 255 } else { 0 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    #[test]
    fn capture_thumbnail_success_path() {
        let camera = MockCamera(Ok(checkerboard_image(16, 8)));
        let thumb = capture_thumbnail(&camera).expect("成功する");
        assert_eq!(thumb.width, 16);
        assert_eq!(thumb.height, 8);
    }

    #[test]
    fn capture_thumbnail_maps_errors() {
        let denied = MockCamera(Err(CameraError::PermissionDenied("camera")));
        assert_eq!(
            capture_thumbnail(&denied).unwrap_err(),
            CaptureFailure::PermissionDenied
        );

        let unsupported = MockCamera(Err(CameraError::Unsupported("camera")));
        assert_eq!(
            capture_thumbnail(&unsupported).unwrap_err(),
            CaptureFailure::Unsupported
        );

        let invalid = MockCamera(Err(CameraError::InvalidImageSize(0, 0)));
        assert!(matches!(
            capture_thumbnail(&invalid).unwrap_err(),
            CaptureFailure::Other(_)
        ));
    }

    #[test]
    fn spawn_capture_ignores_non_requested_state() {
        let (sender, rx) = sender_with_channel();
        let state = CameraCaptureState::default(); // Idle
        spawn_capture(
            &state,
            &sender,
            Arc::new(DummyCamera {
                width: 4,
                height: 4,
            }),
        );
        assert!(rx.try_recv().is_err(), "Idle 状態では起動要求を送らない");
    }

    /// `Requested` から始めて `mark_capturing` と終端の完了を順に受け、適用後の状態を返す。
    fn drive(camera: Arc<dyn Camera>) -> CameraCaptureState {
        let (sender, rx) = sender_with_channel();
        let state = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        spawn_capture(&state, &sender, camera);

        let mut applied = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let mark = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("mark_capturing 完了を受信する");
        mark(&mut applied);
        assert_eq!(applied.phase, CapturePhase::Capturing);

        let finish = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("終端の完了を受信する");
        finish(&mut applied);
        applied
    }

    #[test]
    fn spawn_capture_drives_requested_to_captured_via_dummy_camera() {
        let applied = drive(Arc::new(DummyCamera {
            width: 4,
            height: 4,
        }));
        assert!(matches!(applied.phase, CapturePhase::Captured(_)));
    }

    #[test]
    fn spawn_capture_drives_requested_to_failed_on_permission_denied() {
        let applied = drive(Arc::new(MockCamera(Err(CameraError::PermissionDenied(
            "camera",
        )))));
        assert_eq!(
            applied.phase,
            CapturePhase::Failed(CaptureFailure::PermissionDenied)
        );
    }

    /// キャンセルは `Failed` ではなく `Idle` へ戻る
    /// （`CaptureFailure::Other` 経由で内部識別子 "camera" が表示されないことの回帰防止）。
    #[test]
    fn spawn_capture_drives_requested_to_idle_on_cancel() {
        let applied = drive(Arc::new(MockCamera(Err(CameraError::Cancelled("camera")))));
        assert_eq!(applied.phase, CapturePhase::Idle);
    }
}
