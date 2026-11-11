//! アプリ・端末の識別子（device-identifier）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 契約（`contract`: [`DeviceIdentifier`]・[`DeviceId`]・[`IdentifierRead`]・[`UnsupportedDeviceIdentifier`]）・
//! 失敗の種類（`error`: [`DeviceIdentifierError`]）・effect ドライバ（`driver`:
//! [`device_identifier_show_effect`]）・OS 実装（iOS の `UIDevice.identifierForVendor`、Android の
//! `Settings.Secure.ANDROID_ID`）・機能宣言（`Cargo.toml`）を 1 つに集める。呼び出し元は画面カタログ
//! （`fandhe-gallery` の `screens/registry.rs` の `effect "device_identifier_show"` 行）。
//! 依存方向は本 crate → `fandhe-core` / `fandhe-runtime` / `fandhe-platform-android` の一方向で、
//! egui・`fandhe-egui-proxy`・`fandhe-device` には依存しない。
//!
//! 守る範囲 / 守らない範囲、値の扱い（全体を取り出さない・ログに出さない）は `contract` のモジュール doc を正とする。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_device_identifier()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は iOS の FFI 呼び出し（`ios.rs`）に限り、`// SAFETY:` を付ける。iOS だけ deny へ緩める。Android は JNI を
// `fandhe-platform-android` の `with_activity_class` に任せるため unsafe を持たない。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::DeviceIdentifierError;
