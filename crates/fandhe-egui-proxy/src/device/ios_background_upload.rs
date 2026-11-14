//! iOS 向けバックグラウンド送信実装（Issue #836。親 #170 の b-ios）。
//! `device/background_upload.rs::implementation` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_background_upload`（`apple/fandhe_apple_background_upload.m`、background `NSURLSession`）を返す。
//! session を作れなければ `Unsupported`（成功を装うダミーにはしない）。
//!
//! # iOS 固有の挙動
//!
//! - 転送は OS が行い、アプリが suspend されても続く。OS が転送完了でアプリを背景起動すると、
//!   共通層（`fandhe-platform-apple` の `ios/fandhe_ios_app_delegate.m`、Issue #1325）の application delegate が
//!   `application:handleEventsForBackgroundURLSession:completionHandler:` を受け、identifier が upload の
//!   ものなら [`register`] で登録した `.m` の `fandhe_apple_bgul_handle_events` が同じ identifier の session を
//!   作り直して保留中のイベントを配らせ、配り終えたら completion handler を 1 回だけ呼ぶ。identifier は
//!   download と別の固定値で、共通層が完全一致で振り分ける。
//! - 利用者がマルチタスク画面から終了すると OS が転送を取り消す（アプリ側では防げない）。
//! - `UIBackgroundModes` は足さない（background session に不要。security.md A05）。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが状態を変えた直後、`apple_background_upload` の `fandhe_rust_bgul_signal` が
//! `crate::ios::request_repaint` を呼び次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序、古い通知の破棄、嘘の成功の禁止、背景起動の完了通知を 1 回だけ返すこと。
//! 守らない: リダイレクト先への送信（OS が必ず追う。検出して失敗にする）、アプリが終わっている間に完了した
//! 転送の結果、複数プロセス、`getAllTasks` の completion が来ないこと。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::background_upload::{BackgroundUpload, UnsupportedBackgroundUpload};

/// iOS の実装を返す（macOS と共用）。session を作れなければ `Unsupported`。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn BackgroundUpload> {
    match super::apple_background_upload::instance() {
        Some(d) => d,
        None => Arc::new(UnsupportedBackgroundUpload),
    }
}

// SAFETY: `apple/fandhe_apple_background_upload.m`（`build.rs` が iOS でコンパイル・リンクする）が定義する C ABI。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    /// 背景 URLSession の完了通知の受け手を共通層へ登録する（`apple/fandhe_apple_background_upload.m`）。
    /// main thread から呼ぶ。冪等。
    fn fandhe_apple_bgul_register_events();
}

/// 背景送信の完了通知（`handleEventsForBackgroundURLSession`）の受け手を共通層へ登録する。
/// 最終成果物の `main` が `UIApplicationMain` より前に main thread から呼ぶ。relaunch の完了通知は
/// 起動と同じターンで届くため、間に合わないと完了通知を取り逃がす。冪等。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub fn register() {
    // SAFETY: 引数なしの C 関数。main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_apple_bgul_register_events() }
}
