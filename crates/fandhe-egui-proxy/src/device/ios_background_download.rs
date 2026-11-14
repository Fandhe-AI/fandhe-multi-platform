//! iOS 向けバックグラウンドダウンロード実装（Issue #391。親 #100 の b-ios）。
//! `device/background_download.rs::implementation` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_background_download`（`apple/fandhe_apple_background_download.m`、background `NSURLSession`）を返す。
//! session を作れなければ `Unsupported`（成功を装うダミーにはしない）。Linux ホストの `cargo test` では
//! 共用実装が無いため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - 転送は OS の別プロセスが行い、アプリが suspend されても続く。OS が転送完了でアプリを背景起動すると、
//!   共通層（`fandhe-platform-apple` の `ios/fandhe_ios_app_delegate.m`、Issue #1325）の application delegate が
//!   `application:handleEventsForBackgroundURLSession:completionHandler:` を受け、identifier が download の
//!   ものなら [`register`] で登録した `.m` の `fandhe_apple_bgdl_handle_events` へ渡す。そこで同じ identifier の
//!   session を作り直して保留中のイベントを delegate へ配らせ、配り終えたら completion handler を 1 回だけ呼ぶ。
//! - 利用者がマルチタスク画面から終了すると OS が転送を取り消す（アプリ側では防げない）。
//! - `UIBackgroundModes` は足さない（background session に不要。security.md A05）。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが状態を変えた直後、`apple_background_download` の `imp::repaint()` が
//! `crate::ios::request_repaint` を呼び次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序、世代による古い通知の破棄、嘘の成功の禁止、背景起動の完了通知を 1 回だけ返すこと。
//! 守らない: 起動直後の再接続（非同期）を待たない開始（その間の開始は前の転送の置き換えになる）、
//! 背景で失敗した理由と前回の完了表示（再起動で失われる）、複数プロセス。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::background_download::{BackgroundDownload, UnsupportedBackgroundDownload};

/// iOS の実装を返す（macOS と共用）。session を作れなければ `Unsupported`。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn BackgroundDownload> {
    match super::apple_background_download::instance() {
        Some(d) => d,
        None => Arc::new(UnsupportedBackgroundDownload),
    }
}

// SAFETY: `apple/fandhe_apple_background_download.m`（`build.rs` が iOS でコンパイル・リンクする）が定義する C ABI。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    /// 背景 URLSession の完了通知の受け手を共通層へ登録する（`apple/fandhe_apple_background_download.m`）。
    /// main thread から呼ぶ。冪等。
    fn fandhe_apple_bgdl_register_events();
}

/// 背景ダウンロードの完了通知（`handleEventsForBackgroundURLSession`）の受け手を共通層へ登録する。
/// 最終成果物の `main` が `UIApplicationMain` より前に main thread から呼ぶ。relaunch の完了通知は
/// 起動と同じターンで届くため、間に合わないと完了通知を取り逃がす。冪等。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub fn register() {
    // SAFETY: 引数なしの C 関数。main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_apple_bgdl_register_events() }
}
