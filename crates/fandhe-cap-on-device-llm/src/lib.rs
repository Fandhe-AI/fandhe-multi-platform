//! on-device-llm（端末内 LLM による応答生成） の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//!
//! 呼び出し元は `fandhe-gallery` の画面登録（`on_device_llm_effect`）と、Activity 再生成時に `reset_pending_tracking` を呼ぶ
//! アプリの `android_main`。契約（応答検査・期限・失敗の細分類）は Web 実装（`fandhe-web-dom`、本体は web-dom に残る）と共有する。
//! Apple（iOS / macOS）は Swift 薄層（FoundationModels を弱リンク）で、Android は未実装（Unsupported）。
//! 守る範囲はモデル未取得・拒否・混雑・期限切れ・空応答を成功にしないことと 1 プロセス内の要求順序で、応答の非決定性・
//! 複数プロセス・プロンプトの内容の検証（長さの上限のみ）は守らない。プロンプトと応答はログへ出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_on_device_llm()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::OnDeviceLlmError;
