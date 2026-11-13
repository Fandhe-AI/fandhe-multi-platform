//! カメラ撮影（camera-capture）の機能 crate（REQ-21 / TASK-21.3、Issue #1170）。
//!
//! 契約（[`Camera`]・[`Image`]・[`CameraError`]・[`DummyCamera`]）、OS 別の実装（Android: JNI 経由で
//! `CameraCaptureHelper`、iOS: `ios/fandhe_ios_camera.m`、それ以外: ダミー）、画面の状態機械
//! （`fandhe_core::screens::camera_capture`）へ橋渡しする effect ドライバ
//! （[`camera_capture_effect`]）、機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める。
//!
//! 依存方向は本 crate → `fandhe-core` / `fandhe-runtime` / `fandhe-platform-*` の一方向で、
//! egui・`fandhe-egui-proxy`・`fandhe-device` には依存しない。呼び出し元は画面カタログ
//! （`fandhe-gallery` の `screens/registry.rs` の `effect "camera_capture"` 行）で、
//! `ProxyApp::update` のフレーム末尾から毎フレーム [`camera_capture_effect`] が呼ばれる。
//! 画面の状態機械は `fandhe-web-dom` も使うため `fandhe-core` に残している
//! （`docs/architecture/consumer-app-structure.md` §8）。
//!
//! # Activity 再生成と完了の配送（#881 の判断）
//!
//! [`spawn_capture`] のワーカースレッドは要求元の `ProxyApp` が持つ `CompletionSender` を clone
//! して持ち出す（push 型）。Android で Activity が再生成される（`configChanges` で止められない
//! 再生成。回転や uiMode / locale は抑止済み）と `ProxyApp` ごと作り直されるため、持ち出し済み
//! sender は旧チャンネルを指したまま実送信は `let _ =` で無視される。
//!
//! 同型の課題を pull 型で解消した機能（ファイル選択、#871）もあるが、camera は push 型を維持する。
//! 理由: Java `CameraCaptureHelper` の復元処理は `pendingCaptureRequestId` を復元し、即座に
//! `nativeOnCameraResult(id, …, 7)` を送って撮影を中断決着させる。これにより新 `ProxyApp` の
//! 画面は常に `CapturePhase::Idle` から始まり、`finish_capture` は `Capturing` 以外の phase を
//! 無視するため、旧 sender への送信が届いても no-op で実害は無い。pull 型へ寄せて中断センチネル
//! （status 7 相当）まで復元させると、再生成直後の画面に「失敗しました」を出す改悪になる。

// unsafe は Android の JNI エクスポート 1 関数と iOS の FFI 呼び出しに限る（それぞれ `# Safety` /
// `// SAFETY:` を付ける）。それ以外のターゲットでは使えないようにする（coding-rust.md）。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(any(target_os = "android", test))]
mod mailbox;

use std::sync::Arc;

use fandhe_core::screens::camera_capture::{
    CameraCaptureState, CaptureFailure, CapturePhase, Thumbnail, cancel_capture, finish_capture,
    mark_capturing, thumbnail_from_rgba,
};
use fandhe_runtime::CompletionSender;

/// 画像 1 枚あたりの RGBA バイト数上限（メモリ枯渇防止・オーバーフロー対策、security.md A05）。
/// Java 側 `CameraCaptureHelper.MAX_IMAGE_BYTES` と同値（片側を正にして手で合わせている既存の複製）。
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// デスクトップ向けダミーカメラの撮影サイズ（[`DummyCamera`] へ渡す）。
/// Android・iOS は実カメラのため対象外。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DUMMY_WIDTH: u32 = 160;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DUMMY_HEIGHT: u32 = 120;

/// 撮影結果の画像。`rgba.len()` は常に `width * height * 4`（RGBA・行優先・パディングなし）。
#[derive(Debug, Clone)]
pub struct Image {
    /// 画像の幅（ピクセル）。
    pub width: u32,
    /// 画像の高さ（ピクセル）。
    pub height: u32,
    /// RGBA・行優先・パディングなしの生バイト列（長さは `width * height * 4`）。
    pub rgba: Vec<u8>,
}

/// 撮影の失敗。文言は `fandhe_device::DeviceError` の同名バリアントと同じにしてあり、
/// `CaptureFailure::Other` に出る表示を移行前から変えない。第 1 引数は内部識別子のみ
/// （端末固有情報は含めない、security.md A09）。
#[derive(Debug, Clone, thiserror::Error)]
pub enum CameraError {
    /// 端末側の権限設定でアクセスが拒否された。
    #[error("権限が拒否されました: {0}")]
    PermissionDenied(&'static str),
    /// 現在のプラットフォーム・端末では提供できない（カメラ非搭載・カメラアプリなし等）。
    #[error("このプラットフォームでは未対応です: {0}")]
    Unsupported(&'static str),
    /// 利用者が撮影 UI を能動的にキャンセルした（正常系）。
    #[error("操作がキャンセルされました: {0}")]
    Cancelled(&'static str),
    /// 画像サイズが許容範囲外（0 または上限超過、`width * height * 4` のオーバーフロー）。
    #[error("画像サイズが不正または大きすぎます: {0}x{1}")]
    InvalidImageSize(u32, u32),
    /// 機能自体は使えるが、今回の呼び出しが一時的な内部要因で失敗した。
    /// `Unsupported`（機種として非対応）と区別する。
    #[error("内部エラーが発生しました: {0}")]
    Internal(&'static str),
}

/// 撮影の契約。実装はスレッド間で共有されうる。
pub trait Camera: Send + Sync {
    /// 1 枚撮影して [`Image`] を返す。サイズが `MAX_IMAGE_BYTES` を超える、または
    /// `width * height` がオーバーフローする場合は [`CameraError::InvalidImageSize`]
    /// を返す（前提条件なし・失敗は回復可能なエラーとして返す契約）。
    fn capture(&self) -> Result<Image, CameraError>;
}

/// 固定の格子パターン RGBA を返すダミー実装（desktop 向け、実カメラ非搭載環境の動作確認用）。
pub struct DummyCamera {
    /// 撮影する画像の幅（ピクセル）。
    pub width: u32,
    /// 撮影する画像の高さ（ピクセル）。
    pub height: u32,
}

impl Camera for DummyCamera {
    fn capture(&self) -> Result<Image, CameraError> {
        // 幅・高さのいずれかが 0 だと積が 0 になり後続の上限チェックを素通りしてしまう
        // （0 x 大きな高さ等でピクセルループが u32::MAX 回近く回りかねない）。
        // `InvalidImageSize` はゼロ寸法も対象と rustdoc に明記しているため、
        // 乗算前に弾いて実装とドキュメントを一致させる。
        if self.width == 0 || self.height == 0 {
            return Err(CameraError::InvalidImageSize(self.width, self.height));
        }
        // width * height * 4 は u32 のままだとオーバーフローしうる
        // （例: 65536 x 65536 は release で確保量が暴走し、debug では panic する）。
        // checked_mul + 上限検証で過大要求を回復可能なエラーに変換する。
        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(CameraError::InvalidImageSize(self.width, self.height))?;
        let byte_len = pixel_count
            .checked_mul(4)
            .filter(|&len| len <= MAX_IMAGE_BYTES)
            .ok_or(CameraError::InvalidImageSize(self.width, self.height))?;
        let mut rgba = Vec::with_capacity(byte_len);
        for y in 0..self.height {
            for x in 0..self.width {
                // 8px 格子の市松模様（実カメラでないことが見て分かるダミー画像）。
                let on = ((x / 8) + (y / 8)) % 2 == 0;
                let v = if on { 220 } else { 40 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Ok(Image {
            width: self.width,
            height: self.height,
            rgba,
        })
    }
}

/// 実行対象プラットフォームに応じたカメラ実装を返す（[`camera_capture_effect`] から
/// 撮影開始時のみ呼ばれる）。Android・iOS は実カメラ、それ以外（デスクトップ）は [`DummyCamera`]。
/// 状態型に依存しない入口（consumer-app-structure.md §8.2）。フレームワークの外のアプリも、自分の状態型の effect から使える。
pub fn platform_camera_capture() -> Arc<dyn Camera> {
    #[cfg(target_os = "android")]
    {
        Arc::new(android::AndroidCamera::new())
    }
    #[cfg(target_os = "ios")]
    {
        Arc::new(ios::IosCamera)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        Arc::new(DummyCamera {
            width: DUMMY_WIDTH,
            height: DUMMY_HEIGHT,
        })
    }
}

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
    fn dummy_rgba_length_matches_dimensions() {
        let camera = DummyCamera {
            width: 4,
            height: 3,
        };
        let image = camera.capture().expect("capture は成功する");
        assert_eq!(image.rgba.len(), 4 * 3 * 4);
    }

    #[test]
    fn dummy_oversized_dimensions_return_error_instead_of_overflow() {
        // 65536 x 65536 は u32 乗算がオーバーフローする組み合わせ（回帰テスト）。
        let camera = DummyCamera {
            width: 65536,
            height: 65536,
        };
        assert!(matches!(
            camera.capture(),
            Err(CameraError::InvalidImageSize(65536, 65536))
        ));
    }

    #[test]
    fn dummy_zero_width_or_height_returns_error_instead_of_empty_image() {
        let zero_width = DummyCamera {
            width: 0,
            height: 1,
        };
        assert!(matches!(
            zero_width.capture(),
            Err(CameraError::InvalidImageSize(0, 1))
        ));
        let zero_height = DummyCamera {
            width: 1,
            height: 0,
        };
        assert!(matches!(
            zero_height.capture(),
            Err(CameraError::InvalidImageSize(1, 0))
        ));
    }

    #[test]
    fn error_messages_match_device_error() {
        // `CaptureFailure::Other` に出る文言を移行前（fandhe-device）から変えない。
        assert_eq!(
            CameraError::InvalidImageSize(1, 2).to_string(),
            "画像サイズが不正または大きすぎます: 1x2"
        );
        assert_eq!(
            CameraError::Internal("camera bridge").to_string(),
            "内部エラーが発生しました: camera bridge"
        );
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
