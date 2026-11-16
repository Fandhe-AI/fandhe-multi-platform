//! photo-library（アプリが作った PNG を 1 枚、写真ライブラリへ保存する）の機能 crate（REQ-21 / TASK-21.4、Issue #1332）。
//! iOS・macOS は `PHPhotoLibrary`（add-only 認可）、Android は `MediaStore`、Windows は Pictures フォルダへのファイル書き込み。
//! アルバムの作成・一覧・削除は扱わない。呼び出し元は `fandhe-gallery` の画面登録（`photo_save_effect`）。
//! 検査済みの PNG（`PngImage`）は壁紙の設定（wallpaper-set）も使うため `fandhe-device` に残り、契約から再公開する。
//!
//! 守る範囲: 保存確定以外を「保存しました」と見せないこと、完了を 1 回だけ反映すること、失敗時の作りかけファイルの片付け、
//! 入力不正を OS へ渡さないこと。守らない範囲: 複数プロセスからの同時保存、保存後の利用者による編集・削除、
//! iCloud 写真や OneDrive リダイレクトの同期完了（詳細は `contract` の doc）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_photo_library()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::PhotoLibraryError;
