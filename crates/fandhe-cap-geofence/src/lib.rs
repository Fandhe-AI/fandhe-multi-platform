//! geofence（地点と半径を 1 件だけ登録し、現在地が円の内外どちらかと出入りを判定する）の機能 crate
//! （REQ-21 / TASK-21.4、Issue #1332）。OS の領域監視 API は使わず、`fandhe-cap-location` の継続取得に乗せて
//! Rust 側で判定する（オーナー判断 10/4）。呼び出し元は `fandhe-gallery` の画面登録（`geofence_effect`）。
//!
//! 守る範囲: 失敗・未取得を「中にいる」「登録済み」に見せないこと、判定と状態更新を同じロックの内側で行うこと。
//! 守らない範囲: 境界付近の揺れによる出入りの繰り返し（ヒステリシス無し）、背景での通知、Fix の鮮度判定。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_geofence()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

#![forbid(unsafe_code)]

mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::GeofenceError;
