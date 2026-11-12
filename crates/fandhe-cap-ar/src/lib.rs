//! 機能 crate「AR 表示」（ar）。契約（`contract`）・effect ドライバ（`driver`）・iOS の実装（`ios`＝ARKit の FFI、
//! `ios/*.m`）・機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）を 1 つに集める
//! （REQ-21 / TASK-21.4、Issue #1187。移行の手順は `docs/operations/capability-crate-migration.md`）。
//!
//! 呼び出し元: 利用側（`fandhe-gallery` の `screens/registry.rs`）が [`ar_show_effect`] を画面
//! （`fandhe_core::screens::ar_show`）の effect として登録する。Web の実装は `fandhe-web-dom` に残り、
//! OS に触れない純関数・定数（[`checked_reading`]・[`READ_INTERVAL`] など）だけを本 crate の契約から使う。
//! カメラ映像の縮小（`camera_preview_stream`）と待ちの追跡（`compass`）は他機能も使う純粋なコードのため
//! `fandhe-device` に残し、そこへ依存する。egui には依存しない。
//!
//! 守る範囲と守らない範囲は [`ArTracking`] の契約（`contract.rs`）を参照。

// unsafe は iOS の FFI（`ios`）だけ。それ以外の OS では禁止する。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

mod contract;
mod driver;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
// `ArTracking::read_frame` の戻り値型。利用側が `fandhe-device` に直接依存せず実装できるよう再公開する。
pub use fandhe_device::camera_preview_stream::ScaledFrame;
