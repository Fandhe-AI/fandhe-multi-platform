//! map-view（決まった地点の地図の静止画を 1 枚取得して RGBA 画像で返す）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! 実装は iOS・macOS の MapKit（`MKMapSnapshotter`、`apple.rs` と `apple/fandhe_apple_map_view.m`）のみで、他 OS は `Unsupported`。ライブ地図・ズーム・注釈は扱わない。
//! 呼び出し元は `fandhe-gallery` の画面登録（`registry.rs` の `effect` 区画）が登録する effect ドライバ（`driver`）。
//!
//! 守る範囲: 画像なし・0 寸法・要求サイズ違いを成功にしないこと、結果は 1 回きりで古い世代の完了を捨てること、期限（15 秒、単調時計）、資源の全経路での解放、座標をログへ出さないこと。
//! 守らない範囲: タイルの内容・鮮度、Apple 側のレート制限、複数プロセス、背景にいる間の取得。詳細は `contract.rs` の契約を参照。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_map_view()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::MapViewError;
