//! macOS 向け動画の撮影実装（親 #169、Issue #1071）。`device/video_capture.rs` の macOS 経路（`platform`）から
//! [`implementation`] が呼ばれ、`macos/fandhe_macos_video_capture.m`（AppKit の撮影パネル +
//! `AVCaptureMovieFileOutput`。`build.rs` が feature `device` 有効時にコンパイルする）を `unsafe extern "C"` で
//! 呼ぶ。`unsafe` はこの FFI 宣言と受け口に閉じ、各所に `// SAFETY:` を書く（coding-rust.md の FFI 例外規定）。
//! 契約の SSOT は `fandhe_device::video_capture`。
//!
//! 世代照合・二重完了の破棄・表示記録のない成功値の拒否は effect ドライバの `bridge` が担い、ここは
//! 撮影パネルの表示依頼・結果の受け取り・数値表の写像だけをする。契約表との対応:
//! - macOS は前面判定・表示前の中断を持たないため kind 4・5・6 は使わない。
//! - 表示直前に `try_begin_present` で現在の要求かを確かめ、古い要求は表示しない（2）。
//! - 完了は `.m` がパネルを畳んで置き場を消した後に 1 回だけ渡す。macOS は既存のポーリング
//!   （`request_repaint_after`）で反映されるため、受け口で再描画は要求しない（4）。
//! - 非対応・権限拒否・0 バイト・メタデータ不可・未知の kind は成功に畳まない（6）。
//!   数値表の写像は `fandhe_device::video_capture::result_from_code` を使い、再実装しない。
//!   取り消しは失敗にしない（10）。
//! - 一時ファイルは要求ごとの mkdtemp（0700）の置き場だけ。消せなければ `Internal` にして次回に消し直す（9）。
//!   パスと OS のエラー文は Rust へ渡さず、ログにも出さない（8）。
//! - 期限・タイマーを持たない（3）。新しい権限・依存は足さない（映像のみでマイクは使わない）。
//!
//! 守らない範囲: パネルの見た目・画質・形式、カメラの選択（OS の既定）、他アプリとの同時利用、
//! 撮影中のプロセス死、複数プロセス、閉じた通知が来ない場合の復旧。

use std::sync::Arc;

use fandhe_device::video_capture::{VideoCaptureFailure, VideoCapturer};

#[cfg(target_os = "macos")]
use fandhe_device::video_capture::result_from_code;
#[cfg(not(target_os = "macos"))]
use fandhe_device::video_capture::UnsupportedVideoCapturer;

// SAFETY: `macos/fandhe_macos_video_capture.m` が定義する C ABI。契約は `.m` ファイル冒頭コメントに集約する。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_video_capture_start(request_id: u64, max_duration_secs: u32) -> i32;
}

/// 同期の受理コードを写す。0 = 受理、それ以外は `Internal`。
// ホストでは単体テストだけが呼ぶ。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn start_result(code: i32) -> Result<(), VideoCaptureFailure> {
    match code {
        0 => Ok(()),
        _ => Err(VideoCaptureFailure::Internal),
    }
}

#[cfg(target_os = "macos")]
struct MacVideoCapturer;

#[cfg(target_os = "macos")]
impl VideoCapturer for MacVideoCapturer {
    #[allow(unsafe_code)]
    fn capture(&self, request_id: u64, max_duration_secs: u32) -> Result<(), VideoCaptureFailure> {
        // SAFETY: 引数は値渡しの整数のみでポインタを渡さない。`.m` は専用 queue へ積んですぐ戻る。
        let code = unsafe { fandhe_macos_video_capture_start(request_id, max_duration_secs) };
        start_result(code)
    }
}

/// macOS 用の実装を返す。ホストのテストビルド（macOS 以外）では dispatch が呼ばれないため `Unsupported` を返す。
pub(crate) fn implementation() -> Arc<dyn VideoCapturer> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(MacVideoCapturer)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(UnsupportedVideoCapturer)
    }
}

/// `.m` がパネルを出す直前に呼ぶ。1 = 表示してよい、0 = 表示しない（古い要求）。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_video_capture_try_begin_present(request_id: u64) -> i32 {
    i32::from(super::video_capture::try_begin_present(request_id))
}

/// `.m` が結果を 1 回だけ渡す受け口。引数は値渡しのみ。kind の意味は `.m` 冒頭と `result_from_code` の表に一致する。
#[cfg(target_os = "macos")]
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

    // macOS ホストでは実際にパネルを開いてしまうため、macOS 以外に限る。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn host_implementation_never_succeeds() {
        assert_eq!(
            implementation().capture(1, 30),
            Err(VideoCaptureFailure::Unsupported)
        );
    }

    #[test]
    fn receiver_mapping_does_not_fake_success() {
        assert_eq!(
            result_from_code(0, 0, 1000, 1920, 1080, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(7, 0, 0, 0, 0, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(8, 0, 0, 0, 0, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(9, 0, 0, 0, 0, 0),
            Err(VideoCaptureFailure::Internal)
        );
        assert_eq!(
            result_from_code(2, 0, 0, 0, 0, 0),
            Err(VideoCaptureFailure::Unsupported)
        );
        assert_eq!(
            result_from_code(3, 0, 0, 0, 0, 0),
            Err(VideoCaptureFailure::PermissionDenied)
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
