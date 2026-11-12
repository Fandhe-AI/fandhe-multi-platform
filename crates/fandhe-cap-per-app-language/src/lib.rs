//! per-app-language の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。アプリ別の言語設定（OS が持つこのアプリの言語）を 1 回読む。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`per_app_language_show_effect`）。iOS・macOS は `Bundle.main.preferredLocalizations`、
//! Android は `PerAppLanguageHelper`（`LocaleManager`）を JNI で呼ぶ。守る範囲は嘘の値を出さないこと・押下ごとに OS へ聞くこと、
//! 守らない範囲は読み取り後の変更の反映・アプリ自身の文言の切り替え。Android の `<application android:localeConfig>` と
//! `res/xml/locale_config.xml` は宣言で表せないため、アプリ側に残す。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_per_app_language()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos", test))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::PerAppLanguageError;
