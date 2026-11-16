//! quick-settings-tile（OS のシステム UI に置いた 1 操作のボタンの押下回数を読んで画面に出す）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。macOS は App Group の回数ファイルを読み、Android は `TileService` の
//! 保存先を JNI で読む。`fandhe-gallery` の画面登録から effect `quick_settings_tile_show_effect` が呼ばれる。
//! macOS の対応可否（26 以降）は `fandhe-cap-device-info` で OS の版を読んで判定する。
//!
//! 守る範囲: 1 押下 = +1、保存失敗を数えたことにしない、未押下（0）と読み出し失敗の区別、嘘の成功の禁止。
//! 守らない範囲: ポーリング間隔ぶんの表示の遅れ、プロセス終了で失敗件数が消えること、複数プロセス。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_quick_settings_tile()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "macos", test))]
mod apple;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::QuickSettingsTileError;
