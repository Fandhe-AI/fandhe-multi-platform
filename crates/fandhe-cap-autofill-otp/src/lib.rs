//! autofill-otp の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1334）。入力欄へ「ワンタイムコード」のヒントを付ける。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`autofill_otp_effect`）と、Web の `fandhe-web-dom`（契約の純関数・定数だけを共有）。
//! OS 実装は持たず、描画層の `fandhe_runtime::HostView::set_text_input_hint` へ委譲する（C ABI は持ち込まない）。
//! 橋渡しがある iOS だけが成功し、他は `Unsupported`。守る範囲は世代照合・値を外へ出さないこと・未対応を成功と表示しないこと、
//! 守らない範囲は OS が候補を出すか・複数タブ（`contract` の doc を参照）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_autofill_otp()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

#![forbid(unsafe_code)]

mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::AutofillOtpError;
