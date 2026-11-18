//! カタログ専用の検証アプリ（Issue #1200 で `fandhe-egui-proxy` から移した）。
//!
//! 利用者向けの部品ではなく、カタログの入口（`select.rs`）と計測専用の入口（`main.rs`・`lib.rs`）だけが使う。
//!
//! - [`apps`]: プロトタイプ（`FANDHE_PROXY_APP=prototype`）・TV フォーカス（`tv-focus`）の検証アプリ
//! - [`tv_focus`]・[`tv_sample`]: TV のフォーカス移動（TASK-17.2 / REQ-17）。全ターゲットでコンパイルし、デスクトップの矢印キーでも同じ経路を動かせる
//! - `frame_metrics`（feature `frame-metrics`）・`cold_start`（feature `cold-start-probe`）: 計測専用の入口

pub mod apps;
#[cfg(feature = "cold-start-probe")]
pub mod cold_start;
// Web の計測は `web/index.html` の `?measure=1`（TASK-4.5）を使うため wasm32 は対象外。
#[cfg(all(feature = "frame-metrics", not(target_arch = "wasm32")))]
pub mod frame_metrics;
pub mod tv_focus;
pub mod tv_sample;

pub use apps::{PrototypeApp, TvFocusApp};
