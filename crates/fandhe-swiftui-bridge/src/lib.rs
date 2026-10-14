//! Rust コア（`fandhe-core`）と SwiftUI 描画層を接続するプラットフォーム統合レイヤー
//! （REQ-3 / TASK-3.2、`docs/architecture/swiftui-bridge-architecture.md` が SSOT）。
//!
//! 依存方向は本クレート → `fandhe-core` の一方向（coding-rust.md 準拠）。
//! `fandhe-core` は本クレートへ依存しない。
//!
//! # モジュール構成
//!
//! - [`codec`]: 引数・戻り値の JSON シリアライズ・デシリアライズ（安全コードのみ、
//!   `unsafe` を一切含まない。TASK-3.2-S3 / #60、設計文書 §3.2・§3.1「codec モジュール」）。
//! - [`ffi`]: `extern "C"` エクスポート関数の定義。`unsafe` を閉じ込める唯一のモジュール
//!   （TASK-3.2-S2 / #59、設計文書 §3.1・§3.3「インターフェース草案」）。C ヘッダ
//!   `platforms/apple/FandheSwiftUIBridge/Sources/CFandheBridge/include/fandhe_bridge.h`
//!   と 1 対 1 対応する。
//! - [`error`]: エラーコード enum・詳細メッセージ保持・panic 遮断ヘルパ（安全コードのみ、
//!   `unsafe` を一切含まない。TASK-3.2-S4 / #61、設計文書 §3.3「エラー伝播」・
//!   §3.1「所有権・寿命規約」）。
//!
//! # unsafe 境界
//!
//! クレートルートは forbid 属性（`unsafe_code` を上書き不可で禁止するもの）ではなく、
//! `#![deny(unsafe_code)]` を採用する。forbid はクレート内のどこからも
//! `#[allow(unsafe_code)]` で局所的に上書きできないため、
//! 「`ffi` モジュールのみ `unsafe` を許可し他モジュールは禁止する」という設計上の境界を
//! 表現できない（設計文書 §3.1「unsafe 境界の規約」）。[`ffi`] モジュールのみ
//! `#![allow(unsafe_code)]` で例外的に許可し、`codec` / `error` モジュールは
//! `unsafe` を一切含まない。
#![deny(unsafe_code)]

pub mod codec;
pub mod error;
pub mod ffi;
