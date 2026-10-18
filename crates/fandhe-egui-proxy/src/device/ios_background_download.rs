//! iOS 向けバックグラウンドダウンロード実装（Issue #391。親 #100 の b-ios）。
//! `device/background_download.rs::implementation` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_background_download`（`apple/fandhe_apple_background_download.m`、background `NSURLSession`）を返す。
//! session を作れなければ `Unsupported`（成功を装うダミーにはしない）。Linux ホストの `cargo test` では
//! 共用実装が無いため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - 転送は OS の別プロセスが行い、アプリが suspend されても続く。OS が転送完了でアプリを背景起動すると、
//!   `ios/fandhe_ios_url_scheme.m` の application delegate が
//!   `application:handleEventsForBackgroundURLSession:completionHandler:` を受け、`.m` の
//!   `fandhe_apple_bgdl_handle_events` へ転送する。そこで同じ identifier の session を作り直して
//!   保留中のイベントを delegate へ配らせ、配り終えたら completion handler を 1 回だけ呼ぶ。
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
