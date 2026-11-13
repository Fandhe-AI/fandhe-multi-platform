//! image-generation の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。OS の画像生成画面（Image Playground）を出し、
//! 利用者が確定した 1 枚を受け取る。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`image_generation_show_effect`）。画素・プロンプト・OS のエラー文は
//! ログに出さない。守る範囲は 1 プロセス内の要求の順序・成功 / キャンセル / 失敗の区別・表示開始前の期限（`driver` の doc を参照）、
//! 守らない範囲は生成画面を開いた後の利用者の操作の長さ。macOS・iOS が実装し、他の OS は `Unsupported`（ダミーの成功へ倒さない）。
//!
//! 契約（`contract`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。失敗の種類は契約の `ImageGenerationFailure` で、
//! 機能 crate 独自のエラー型は持たない。画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_image_generation()` は
//! 状態型に依存しないため、フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
#[cfg(target_os = "ios")]
mod ios;

pub use contract::*;
pub use driver::*;
