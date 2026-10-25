//! iOS 向け動画の撮影実装（親 #169、Issue #822）。`device/video_capture.rs` の iOS 経路（`platform`）から
//! [`implementation`] が呼ばれ、`ios/fandhe_ios_video_capture.m`（`UIImagePickerController` の動画モード。
//! `build.rs` が feature `device` 有効時にコンパイルして CoreMedia をリンクする）を `unsafe extern "C"` で
//! 呼ぶ。`unsafe` はこの FFI 宣言と受け口に閉じ、各所に `// SAFETY:` を書く（coding-rust.md の FFI 例外規定）。
//! 契約の SSOT は `fandhe_device::video_capture`。
//!
//! 世代照合・二重完了の破棄・表示記録のない成功値の拒否は effect ドライバの `bridge` が担い、ここは
//! 撮影 UI の表示依頼・結果の受け取り・数値表の写像だけをする。契約表との対応:
//! - 受付時に背景なら `NotForeground`（5）、権限応答時に背景なら `Interrupted`（6）。
//! - 表示直前に `try_begin_present` で現在の要求かを確かめ、古い要求は表示しない（2）。
//! - 完了は `.m` が撮影 UI を閉じた後に 1 回だけ渡す。受け口は結果を渡した直後に
//!   `crate::ios::request_repaint` で次フレームを起こす（4・5）。
//! - 非対応・権限拒否・0 バイト・メタデータ不可・未知の kind は成功に畳まない（6）。
//!   数値表の写像は `fandhe_device::video_capture::result_from_code` を使い、再実装しない。
//!   取り消しは失敗にしない（10）。
//! - 一時ファイルは `.m` が全経路で消す。パスと OS のエラー文は Rust へ渡さず、ログにも出さない（8・9）。
//! - 期限・タイマーを持たない。撮影は利用者の操作時間に依存し、完了は OS の delegate 通知だけで決める（3）。
//!   新しい権限・依存は足さない。
//!
//! 守らない範囲: 提示の completion が届かない場合の救済、録画中の背景遷移の扱い（OS 任せ）、撮影 UI の
//! 表示中に Rust 側から閉じること、複数プロセス、プロセス死後の結果。

use std::sync::Arc;

use fandhe_device::video_capture::{VideoCaptureFailure, VideoCapturer};

#[cfg(target_os = "ios")]
use fandhe_device::video_capture::result_from_code;
#[cfg(not(target_os = "ios"))]
use fandhe_device::video_capture::UnsupportedVideoCapturer;

// SAFETY: `ios/fandhe_ios_video_capture.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_video_capture_start(request_id: u64, max_duration_secs: u32) -> i32;
}

/// 同期の受理コードを写す。0 = 受理、それ以外は `Internal`。
// ホストでは単体テストだけが呼ぶ。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn start_result(code: i32) -> Result<(), VideoCaptureFailure> {
    match code {
        0 => Ok(()),
        _ => Err(VideoCaptureFailure::Internal),
    }
}

#[cfg(target_os = "ios")]
struct IosVideoCapturer;

#[cfg(target_os = "ios")]
impl VideoCapturer for IosVideoCapturer {
    #[allow(unsafe_code)]
    fn capture(&self, request_id: u64, max_duration_secs: u32) -> Result<(), VideoCaptureFailure> {
        // SAFETY: 引数は値渡しの整数のみでポインタを渡さない。`.m` は main queue へ積んで戻る。
        let code = unsafe { fandhe_ios_video_capture_start(request_id, max_duration_secs) };
        start_result(code)
    }
}

/// iOS 用の実装を返す。ホストのテストビルドでは dispatch が呼ばれないため `Unsupported` を返す。
pub(crate) fn implementation() -> Arc<dyn VideoCapturer> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosVideoCapturer)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(UnsupportedVideoCapturer)
    }
}

/// `.m` が撮影 UI を出す直前に呼ぶ。1 = 表示してよい、0 = 表示しない（古い要求）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_video_capture_try_begin_present(request_id: u64) -> i32 {
    i32::from(super::video_capture::try_begin_present(request_id))
}

/// `.m` が結果を 1 回だけ渡す受け口。渡した直後に再描画を要求する（入力がないと egui が止まる）。
/// 引数は値渡しのみ。kind の意味は `.m` 冒頭と `result_from_code` の表に一致する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_video_capture_complete(
    request_id: u64,
    kind: i32,
    size_bytes: u64,
    duration_ms: i64,
    width: i32,
    height: i32,
    rotation_deg: i32,
) {
    super::video_capture::complete_from_os(
        request_id,
        result_from_code(kind, size_bytes, duration_ms, width, height, rotation_deg),
    );
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::video_capture::{result_from_code, VideoCaptureOutcome};

    #[test]
    fn start_result_maps_accept_and_errors() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(VideoCaptureFailure::Internal));
        assert_eq!(start_result(-1), Err(VideoCaptureFailure::Internal));
    }

    #[test]
    fn host_implementation_never_succeeds() {
        assert_eq!(
            implementation().capture(1, 30),
            Err(VideoCaptureFailure::Unsupported)
        );
    }

    #[test]
    fn receiver_mapping_does_not_fake_success() {
        // 0 バイト・寸法 0・不正な回転・未知の kind は成功にならない。
        assert_eq!(
            result_from_code(0, 0, 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(0, 10, 1000, 0, 1080, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(0, 10, 1000, 1920, 1080, 45),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(99, 10, 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Internal)
        );
        assert_eq!(
            result_from_code(1, 0, 0, 0, 0, 0),
            Ok(VideoCaptureOutcome::Cancelled)
        );
    }
}
