//! iOS 向け住所の座標変換実装の骨格（Issue #691 a、実体は #692 b-ios）。
//! `device/geocoding_search.rs` の iOS 経路から呼ばれる。現時点は `Unsupported` を返すスタブで、
//! ダミーの成功へは倒さない。
//!
//! b-ios への約束:
//!
//! - `implementation()` が `apple_geocoding_search::AppleForwardGeocoder` を返すだけにする
//!   （macOS と共用。`apple/fandhe_apple_geocoding_search.m` は `build.rs` の iOS 分岐から
//!   コンパイル済みで、`build.rs` は触らない）。dispatch・画面・文言も触らない
//! - iOS 26 未満は ObjC 側の `@available` で `Unsupported`（非推奨の `CLGeocoder` へは倒さない）
//! - 使い始めたら `apple_geocoding_search.rs` 先頭の `allow(dead_code)`（iOS 向け）を外してよい
//! - 背景にいる間は completion の配送が遅れ得る。その場合は Rust 側の期限（`Timeout`）で失敗が
//!   確定し、遅れて届いた結果は世代照合で捨てられる
//!
//! 契約表 4（再描画）: 結果は `RequestSlot::deliver` で置く pull 型で、Rust の共有状態へ書き込む
//! OS コールバックは無い。待機中はドライバの `poll_and_finish` が毎フレーム再描画を予約するため
//! `crate::ios::request_repaint` を別に呼ぶ必要はない。
//! 守る範囲・守らない範囲は `fandhe_device::geocoding_search` の doc を正とする。

use std::sync::Arc;

use fandhe_device::geocoding_search::{ForwardGeocoder, UnsupportedForwardGeocoder};

/// iOS の `ForwardGeocoder` 実装を返す（骨格: `Unsupported` 固定）。
pub(crate) fn implementation() -> Arc<dyn ForwardGeocoder> {
    Arc::new(UnsupportedForwardGeocoder)
}
