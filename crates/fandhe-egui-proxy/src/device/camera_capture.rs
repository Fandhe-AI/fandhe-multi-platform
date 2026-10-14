//! Phase 8（端末機能の型検証、Issue #749）: `fandhe-device` の `Camera` トレイトを
//! `fandhe-core` 側の画面状態（`fandhe_core::screens::camera_capture`）へ橋渡しする
//! effect ドライバ（`crate::app::EffectFn` 契約、`screens.rs` のマクロ doc 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる（`device.rs` /
//! `lib.rs` の `mod` 宣言参照）。`fandhe-core` は `fandhe-device` を参照しない（依存方向は
//! プロキシ → コアの一方向、coding-rust.md 準拠）ため、両者の型を橋渡しするのは本クレート
//! 側の責務にする（`crate::device::camera_capture::to_capture_failure` 参照）。
//!
//! `screens/registry.rs` の `effect "camera_capture" => ..., crate::device::
//! camera_capture::camera_capture_effect,` 行から [`camera_capture_effect`] が
//! `ProxyApp::update` のフレーム末尾で毎フレーム呼ばれる（`app.rs` の `EffectFn` doc
//! 参照）。desktop（macOS/Windows/Linux）・Android は `fandhe_device::camera::
//! DummyCamera`（固定の市松模様画像）を撮影対象にする。iOS は実カメラ
//! （[`crate::device::ios::IosCamera`]、Issue #751）。Android の実カメラ配線は #750。
//!
//! # Activity 再生成と完了の配送（#881 の判断）
//!
//! [`spawn_capture`] のワーカースレッドは要求元の `ProxyApp` が持つ
//! `CompletionSender` を clone して持ち出す（push 型）。Android で Activity が
//! 再生成される（`configChanges` 外の uiMode / locale 変更。回転は抑止済み）と
//! `ProxyApp` ごと作り直されるため、持ち出し済み sender は旧チャンネルを
//! 指したまま実送信は `let _ =` で無視される（下記 doc 参照）。
//!
//! `crate::device::file_pick`（#871）はこれと同型の課題を pull 型 `bridge`
//! （sender も generation も持たない）で解消しているが、camera はそちらへ寄せず
//! push 型を維持すると判断した。理由: Java `MainActivity` の `onCreate` は
//! `pendingCaptureRequestId` を復元し、即座に `nativeOnCameraResult(id, …, 7)`
//! を送って撮影を中断決着させる（`platforms/android/app/.../MainActivity.java`
//! の `pendingCaptureInterrupted` 復元処理、PR #790 codex-review / Cursor Bugbot
//! 是正）。これにより新 `ProxyApp` の画面は常に [`CapturePhase::Idle`] から
//! 始まり、[`fandhe_core::screens::camera_capture::finish_capture`] は
//! `Capturing` 以外の phase を無視するため、旧 sender への送信が届いたとしても
//! no-op で実害は無い。pull 型へ寄せて中断センチネル（status 7 相当）まで
//! 復元させると、再生成直後の画面に「失敗しました」を出す改悪になる。
//! 影響が無い以上、camera を pull 型へ移すコストに見合う利得は無いと判断した。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::camera_capture::{
    cancel_capture, finish_capture, mark_capturing, thumbnail_from_rgba, CameraCaptureState,
    CaptureFailure, CapturePhase, Thumbnail,
};
use fandhe_device::camera::Camera;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use fandhe_device::camera::DummyCamera;
use fandhe_device::{DeviceError, Image};

use crate::app::CompletionSender;

/// デスクトップ向けダミーカメラの撮影サイズ（`fandhe_device::camera::DummyCamera` へ渡す）。
/// Android（[`crate::device::android::AndroidCamera`]、Issue #750）・iOS
/// （[`crate::device::ios::IosCamera`]、Issue #751）は実カメラのため対象外。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DUMMY_WIDTH: u32 = 160;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DUMMY_HEIGHT: u32 = 120;

/// [`CaptureMailbox`] の内部状態。単一飛行（同時に 1 件の撮影のみ）を強制する。
/// `Pending`/`Ready` は要求 ID（`u64`）を保持し、`deliver` が古い要求への遅延コールバックを
/// 新しい要求の結果と取り違えないようにする（codex-review 指摘、PR #790。下記
/// `CaptureMailbox` doc 参照）。
enum MailboxSlot {
    /// 撮影要求なし。
    Idle,
    /// 撮影要求（ID 付き）を出し、コールバックを待っている。
    Pending(u64),
    /// コールバックが届いた（`wait` が消費すると `Idle` へ戻る）。
    Ready(u64, Result<Image, DeviceError>),
}

/// 非同期カメラコールバック（Android の JNI・iOS の delegate、#750/#751）と、
/// `Camera::capture` を同期的に呼ぶ worker スレッド（`spawn_capture` 参照）の間を
/// 仲介する受け渡し窓口。ターゲット非依存（ホスト上のユニットテストで検証できる）。
///
/// 単一飛行: `begin` が `None` を返した場合、呼び出し元は新規の撮影を開始せず
/// エラーを返す契約（`device/android.rs` の `AndroidCamera::capture` 参照）。
///
/// 要求 ID: `begin` が返す ID を呼び出し元が JNI 往復（`device/android.rs` の
/// `requestCameraCapture`/`nativeOnCameraResult` 引数）に乗せて運び、`cancel`/`deliver`/`wait`
/// はすべて同じ ID を要求する。これにより「要求 A が `wait` タイムアウトで `Idle` に
/// 戻った後、要求 B が `begin` してから A の遅延コールバックが届く」順序でも、
/// `deliver` は ID 不一致で無視され、B が A の古い結果を受け取らない
/// （codex-review 指摘、PR #790）。
pub struct CaptureMailbox {
    slot: Mutex<MailboxSlot>,
    condvar: Condvar,
    next_id: AtomicU64,
}

impl CaptureMailbox {
    /// 空の mailbox（`Idle`）を構築する。`static` 初期化子から呼べるよう `const fn` にする
    /// （`device/android.rs` の `CAPTURE_MAILBOX` 参照）。
    pub const fn new() -> Self {
        Self {
            slot: Mutex::new(MailboxSlot::Idle),
            condvar: Condvar::new(),
            next_id: AtomicU64::new(0),
        }
    }

    /// `Idle` なら新しい要求 ID を発行して `Pending` へ遷移し、その ID を返す。
    /// 既に進行中なら `None`
    /// （ponytail: 待ち行列化はしない。撮影が同時多重に要る場合は Vec 化して拡張）。
    pub fn begin(&self) -> Option<u64> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Idle) {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            *slot = MailboxSlot::Pending(id);
            Some(id)
        } else {
            None
        }
    }

    /// `begin` 後、JNI 呼び出し自体が失敗するなど、コールバックが来ないと分かった時点で
    /// `Pending(id)` を `Idle` へ戻す（`deliver` を経由しない早期中断）。`id` が現在の
    /// `Pending` と一致しない場合は無視する（別要求のスロットを誤って畳まない）。
    pub fn cancel(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
            *slot = MailboxSlot::Idle;
        }
    }

    /// コールバックスレッドから結果を届ける。`Pending(id)` と ID が一致する場合のみ受理する。
    /// 一致しない（重複コールバック・タイムアウト後の遅延到着で `id` が既に古い）場合は
    /// 無視する（`wait` 側が既にタイムアウトで `Idle` に戻し、別要求が `begin` 済みの
    /// 可能性があるため）。
    pub fn deliver(&self, id: u64, result: Result<Image, DeviceError>) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
            *slot = MailboxSlot::Ready(id, result);
            self.condvar.notify_all();
        }
    }

    /// `begin` を呼んだスレッドが、その ID の結果を待つ。`timeout` 内に届かない場合は
    /// スロットを `Idle` へ戻し（固着防止）、`DeviceError::Unsupported` を返す
    /// （`fandhe-device` の `DeviceError` に専用の Timeout バリアントを追加する変更は
    /// 本 Issue のスコープ外（fandhe-device は変更しない方針）のため、意味が近い
    /// 既存バリアントを再利用する）。
    pub fn wait(&self, id: u64, timeout: Duration) -> Result<Image, DeviceError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(*slot, MailboxSlot::Ready(ready_id, _) if ready_id == id) {
                let ready = std::mem::replace(&mut *slot, MailboxSlot::Idle);
                return match ready {
                    MailboxSlot::Ready(_, result) => result,
                    _ => unreachable!("直前に Ready であることを確認済み"),
                };
            }
            let now = Instant::now();
            if now >= deadline {
                // 自分（id）の Pending のみ Idle へ戻す。ID 不一致（既に別要求が
                // 進行中）ならここでは何もしない（他要求のスロットを畳まない）。
                if matches!(*slot, MailboxSlot::Pending(pending_id) if pending_id == id) {
                    *slot = MailboxSlot::Idle;
                }
                return Err(DeviceError::Unsupported("camera_timeout"));
            }
            let (guard, _timeout_result) = self
                .condvar
                .wait_timeout(slot, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            slot = guard;
        }
    }
}

impl Default for CaptureMailbox {
    fn default() -> Self {
        Self::new()
    }
}

/// 実行対象プラットフォームに応じたカメラ実装を返す（`camera_capture_effect` から
/// 撮影開始時のみ呼ばれる）。Android は #750 の `AndroidCamera`、iOS は #751 の
/// `IosCamera`、それ以外（デスクトップ）は引き続き `DummyCamera` を使う。
fn platform_camera() -> Arc<dyn Camera> {
    #[cfg(target_os = "android")]
    {
        Arc::new(crate::device::android::AndroidCamera::new())
    }
    #[cfg(target_os = "ios")]
    {
        Arc::new(crate::device::ios::IosCamera)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        Arc::new(DummyCamera {
            width: DUMMY_WIDTH,
            height: DUMMY_HEIGHT,
        })
    }
}

/// `fandhe_device::DeviceError` を `fandhe_core::screens::camera_capture::CaptureFailure`
/// へ変換する。`DeviceError` の内部構造は `fandhe-core` 側へ漏らさず、`Other` の場合のみ
/// `Display` の文言を転送する（security.md A09 準拠。`device.rs`（#747）の
/// `to_note_storage_error` と同じ判断）。
fn to_capture_failure(e: DeviceError) -> CaptureFailure {
    match e {
        DeviceError::PermissionDenied(_) => CaptureFailure::PermissionDenied,
        DeviceError::Unsupported(_) => CaptureFailure::Unsupported,
        other => CaptureFailure::Other(other.to_string()),
    }
}

/// `Camera::capture` を呼び、成功時は [`thumbnail_from_rgba`] でモザイクへ間引く。
/// 間引きに失敗した場合（`Image` の寸法・バイト長が矛盾する不正データ）は
/// `CaptureFailure::Other` として扱う（`fandhe-device` 側のガードを信頼しつつ、
/// 契約違反があっても panic しない防御。coding-rust.md のエラーハンドリング方針）。
pub fn capture_thumbnail(camera: &dyn Camera) -> Result<Thumbnail, CaptureFailure> {
    let image = camera.capture().map_err(to_capture_failure)?;
    thumbnail_from_rgba(image.width, image.height, &image.rgba)
        .ok_or_else(|| CaptureFailure::Other("画像データが不正です".to_string()))
}

/// `Requested` のときのみ撮影を起動する。`mark_capturing` を同期送信してから
/// 別スレッドで `Camera::capture` を実行し、完了時に `finish_capture` を送信する
/// （`DummyCamera::capture` 自体は同期だが、Android/iOS の実カメラ（Intent /
/// `UIImagePickerController`、#750/#751）が非同期コールバックのため、経路を
/// 統一する設計判断。HTTP 取得画面 `#748` の `spawn_request` と同型）。
///
/// 送信失敗（受信側 `ProxyApp` がドロップ済み）は `let _ =` で無視する
/// （panic を API 契約にしない、coding-rust.md 準拠）。Android の Activity 再生成時も
/// 同じ経路で無視される（旧 sender への送信が失敗するだけで、Java 側が status 7 で
/// 中断決着済みのため実結果は既に不要、モジュール doc「Activity 再生成と完了の
/// 配送（#881 の判断）」参照）。
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
        // `DeviceError::Cancelled` はユーザーによる正常な操作（`UIImagePickerController`
        // のキャンセル等）であり、`to_capture_failure` の catch-all で `Other` に丸めて
        // `Failed` として表示すると内部識別子（"camera" 等）入りの失敗表示になってしまう
        // （PR #791 レビュー指摘）。ここで先に分岐し、`Capturing → Idle` へ静かに戻す。
        match camera.capture() {
            Err(DeviceError::Cancelled(_)) => {
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

/// レジストリの `effect "camera_capture"` 行から呼ばれる effect ドライバ
/// （`fn(&S, &CompletionSender<S>)`、`crate::app::EffectFn`）。
/// [`platform_camera`] が返す実装（Android/iOS は実カメラ、それ以外は `DummyCamera`）を
/// 撮影対象にする。
pub fn camera_capture_effect(
    state: &CameraCaptureState,
    sender: &CompletionSender<CameraCaptureState>,
) {
    spawn_capture(state, sender, platform_camera());
}

#[cfg(test)]
mod tests {
    use super::*;
    // `DummyCamera` はテスト対象の worker スレッド駆動（`spawn_capture`）を検証する
    // ためのモック撮影対象。本体側の import（L26-27）は desktop/Android のみ cfg
    // されるが、`DummyCamera` 自体は `fandhe-device` 側で全ターゲット cfg なしのため
    // テストは iOS ターゲットでも問題なく参照できる（Issue #882: `mod tests` に
    // cfg を付けず、ここで明示 import する）。
    use fandhe_device::camera::DummyCamera;

    /// 常に固定結果を返すモック `Camera`（GPU 不要、`fandhe-device` の実装に依存しない）。
    struct MockCamera(Result<Image, DeviceError>);

    impl Camera for MockCamera {
        fn capture(&self) -> Result<Image, DeviceError> {
            match &self.0 {
                Ok(image) => Ok(Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba.clone(),
                }),
                Err(e) => Err(clone_device_error(e)),
            }
        }
    }

    /// `DeviceError` は `#[derive(Debug, thiserror::Error)]` のみで `Clone` を持たないため、
    /// テスト内で必要なバリアントのみ手動で複製する（テストコード限定、本体には持ち込まない）。
    fn clone_device_error(e: &DeviceError) -> DeviceError {
        match e {
            DeviceError::PermissionDenied(s) => DeviceError::PermissionDenied(s),
            DeviceError::Unsupported(s) => DeviceError::Unsupported(s),
            DeviceError::InvalidImageSize(w, h) => DeviceError::InvalidImageSize(*w, *h),
            DeviceError::InvalidKey(s) => DeviceError::InvalidKey(s.clone()),
            DeviceError::Io(io) => DeviceError::Io(std::io::Error::new(io.kind(), io.to_string())),
            DeviceError::Http(s) => DeviceError::Http(s.clone()),
            DeviceError::Cancelled(s) => DeviceError::Cancelled(s),
            DeviceError::Internal(s) => DeviceError::Internal(s),
            DeviceError::InvalidInput(s) => DeviceError::InvalidInput(s),
            DeviceError::Disabled(s) => DeviceError::Disabled(s),
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
    fn capture_thumbnail_maps_device_errors() {
        let denied = MockCamera(Err(DeviceError::PermissionDenied("camera")));
        assert_eq!(
            capture_thumbnail(&denied).unwrap_err(),
            CaptureFailure::PermissionDenied
        );

        let unsupported = MockCamera(Err(DeviceError::Unsupported("camera")));
        assert_eq!(
            capture_thumbnail(&unsupported).unwrap_err(),
            CaptureFailure::Unsupported
        );

        let invalid = MockCamera(Err(DeviceError::InvalidImageSize(0, 0)));
        assert!(matches!(
            capture_thumbnail(&invalid).unwrap_err(),
            CaptureFailure::Other(_)
        ));
    }

    #[test]
    fn spawn_capture_ignores_non_requested_state() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
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

    #[test]
    fn spawn_capture_drives_requested_to_captured_via_dummy_camera() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        let state = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        spawn_capture(
            &state,
            &sender,
            Arc::new(DummyCamera {
                width: 4,
                height: 4,
            }),
        );

        let mut applied = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let mark = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("mark_capturing 完了を受信する");
        mark(&mut applied);
        assert_eq!(applied.phase, CapturePhase::Capturing);

        let finish = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("finish_capture 完了を受信する");
        finish(&mut applied);
        assert!(matches!(applied.phase, CapturePhase::Captured(_)));
    }

    #[test]
    fn spawn_capture_drives_requested_to_failed_on_permission_denied() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        let state = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let camera: Arc<dyn Camera> =
            Arc::new(MockCamera(Err(DeviceError::PermissionDenied("camera"))));
        spawn_capture(&state, &sender, camera);

        let mut applied = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let mark = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("mark_capturing 完了を受信する");
        mark(&mut applied);

        let finish = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("finish_capture 完了を受信する");
        finish(&mut applied);
        assert_eq!(
            applied.phase,
            CapturePhase::Failed(CaptureFailure::PermissionDenied)
        );
    }

    /// PR #791 レビュー指摘対応: キャンセルは `Failed` ではなく `Idle` へ戻る
    /// （`CaptureFailure::Other` 経由で内部識別子 "camera" が表示されないことの回帰防止）。
    #[test]
    fn spawn_capture_drives_requested_to_idle_on_cancel() {
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        let state = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let camera: Arc<dyn Camera> = Arc::new(MockCamera(Err(DeviceError::Cancelled("camera"))));
        spawn_capture(&state, &sender, camera);

        let mut applied = CameraCaptureState {
            phase: CapturePhase::Requested,
        };
        let mark = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("mark_capturing 完了を受信する");
        mark(&mut applied);
        assert_eq!(applied.phase, CapturePhase::Capturing);

        let finish = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("cancel_capture 完了を受信する");
        finish(&mut applied);
        assert_eq!(applied.phase, CapturePhase::Idle);
    }

    /// [`CaptureMailbox`]（Issue #750）: `deliver` → `wait` が別スレッド間で値を運ぶこと
    /// を検証する（Android の JNI コールバックスレッドと worker スレッドの関係を模す）。
    #[test]
    fn mailbox_delivers_across_threads() {
        let mailbox = Arc::new(CaptureMailbox::new());
        let id = mailbox.begin().expect("Idle からは begin できる");

        let deliverer = Arc::clone(&mailbox);
        std::thread::spawn(move || {
            deliverer.deliver(
                id,
                Ok(Image {
                    width: 1,
                    height: 1,
                    rgba: vec![0, 0, 0, 255],
                }),
            );
        });

        let image = mailbox
            .wait(id, Duration::from_secs(5))
            .expect("deliver された結果を受け取る");
        assert_eq!(image.width, 1);
    }

    /// `begin` 中（`Pending`）の再 `begin` は単一飛行を守るため `None` を返す。
    #[test]
    fn mailbox_rejects_concurrent_begin() {
        let mailbox = CaptureMailbox::new();
        assert!(mailbox.begin().is_some());
        assert!(mailbox.begin().is_none(), "Pending 中の再 begin は拒否する");
    }

    /// `deliver` が来ないまま `timeout` を超えると `Err` を返し、スロットを `Idle` に
    /// 戻して固着させない（次の `begin` が成功することも合わせて確認する）。
    #[test]
    fn mailbox_wait_times_out_and_resets_to_idle() {
        let mailbox = CaptureMailbox::new();
        let id = mailbox.begin().expect("Idle からは begin できる");
        let result = mailbox.wait(id, Duration::from_millis(10));
        assert!(result.is_err(), "deliver が来ないままタイムアウトする");
        assert!(
            mailbox.begin().is_some(),
            "タイムアウト後は Idle に戻り再度 begin できる"
        );
    }

    /// codex-review 指摘（PR #790）の再現: 要求 A が `wait` タイムアウトで `Idle` に
    /// 戻った後、要求 B が `begin` してから A の遅延コールバックが `deliver` されても、
    /// ID 不一致のため無視され、B は自分の結果が届くまで `wait` で待ち続ける
    /// （A の古い結果を受け取らない）。
    #[test]
    fn mailbox_ignores_stale_deliver_after_timeout() {
        let mailbox = CaptureMailbox::new();
        let id_a = mailbox.begin().expect("A は begin できる");
        let timeout_result = mailbox.wait(id_a, Duration::from_millis(10));
        assert!(timeout_result.is_err(), "A はタイムアウトする");

        let id_b = mailbox
            .begin()
            .expect("A のタイムアウト後 B は begin できる");
        assert_ne!(id_a, id_b, "A と B の要求 ID は異なる");

        // A の遅延コールバックが今ごろ届く（B の Pending 中）。
        mailbox.deliver(
            id_a,
            Ok(Image {
                width: 999,
                height: 999,
                rgba: vec![0, 0, 0, 255],
            }),
        );

        // B 自身の結果を届けると、A の値ではなく B の値を受け取れる。
        mailbox.deliver(
            id_b,
            Ok(Image {
                width: 2,
                height: 2,
                rgba: vec![1, 1, 1, 255],
            }),
        );
        let image = mailbox
            .wait(id_b, Duration::from_secs(1))
            .expect("B の結果を受け取る");
        assert_eq!(
            image.width, 2,
            "A の古い結果ではなく B 自身の結果を受け取る"
        );
    }
}
