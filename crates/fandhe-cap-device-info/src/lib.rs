//! device-info（端末・OS 情報: OS 名と版・機種・メモリ量・空き容量）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! 画面ボタンの押下ごとに 1 回だけ同期で読む（macOS・Windows・iOS・Android。Web は `fandhe-web-dom`）。
//! `fandhe-gallery` の画面登録から effect `device_info_show_effect` が呼ばれる。
//! 文字列整形 `clean_text` は多数の機能が使うため `fandhe-device` に残し、`contract` から再公開する。
//!
//! 守る範囲: 不明・0 バイト・失敗の区別、失敗を成功値へ畳まないこと、個体識別子を取らないこと。
//! 守らない範囲: OS 間の表記の一致、空き容量の厳密さ、読み取り後の変化の反映（再読込まで）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_device_info()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::DeviceInfoError;
