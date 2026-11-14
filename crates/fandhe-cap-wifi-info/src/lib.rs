//! wifi-info（今つないでいる Wi-Fi の名前と電波強度、OS が持つ直近のスキャン結果の上位数件）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS は `CoreWLAN`、Windows は WLAN API、Android は `WifiManager` で読む。
//! iOS・Web は対象外。`fandhe-gallery` の画面登録から effect `wifi_info_show_effect` が呼ばれる。
//! Windows の電源判定 `radio_off` は wifi-connect と共有するため `fandhe-device` に残し、ここから再公開する。
//!
//! 守る範囲: 嘘の成功を出さないこと（0 件と失敗の区別）、資源を漏らさないこと、SSID を件数以外で出力しないこと。
//! 守らない範囲: スキャン結果の鮮度と dBm の揺れ、読んだ後の変化、複数プロセスからの同時読み取り、非 UTF-8 の SSID。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_wifi_info()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "macos", target_os = "windows")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "macos", target_os = "windows"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::WifiInfoError;
