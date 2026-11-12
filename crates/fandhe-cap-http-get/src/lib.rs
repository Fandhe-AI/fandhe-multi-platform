//! http-get（HTTP 取得・GET 限定）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、Issue #1186）。
//!
//! 契約（`contract`: [`Http`]・[`UreqHttp`]・[`parse_items`]）・失敗の種類（`error`: [`HttpGetError`]）・
//! effect ドライバ（`driver`: [`http_fetch_effect`]、native のみ）・Web 実装（`web`: ブラウザの `fetch`、wasm32 のみ）・
//! 機能宣言（`Cargo.toml`）を 1 つに集める。呼び出し元は画面カタログ（`fandhe-gallery` の
//! `screens/registry.rs` の `effect "http_fetch"` 行）と、DOM レンダラ `fandhe-web-dom` のデモ（`web::http_fetch_effect`）。
//! 取得先の検証（`fandhe_device::http::check_url`）は背景ダウンロード・アップロードと共有のため `fandhe-device` に残る。
//!
//! 守る範囲: https と、ループバック・私設 IPv4 宛ての http だけを許し、リダイレクトを追わず、プロキシを使わず、
//! 本文上限・期限・件数上限を超えさせないこと（失敗を成功に畳まない）。守らない範囲: 取得先サーバーの正しさ、
//! 平文 http 先の盗聴（開発・実機検証用の固定 URL 前提）。
//! 画面の状態型を参照してよいのは `driver` と `web` だけ。

#![forbid(unsafe_code)]

mod contract;
#[cfg(not(target_arch = "wasm32"))]
mod driver;
mod error;
mod web;

pub use contract::*;
#[cfg(not(target_arch = "wasm32"))]
pub use driver::*;
pub use error::HttpGetError;
#[cfg(target_arch = "wasm32")]
pub use web::http_fetch_effect;
