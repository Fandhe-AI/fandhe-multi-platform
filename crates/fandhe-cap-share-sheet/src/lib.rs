//! share-sheet（OS の共有シートへ文字列と URL を渡す）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、#1302）。
//!
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs`（`share_text_effect`）と、入力検証（`ShareRequest`）を共有する
//! `fandhe-web-dom`（Web 実装本体は `fandhe-web-dom` に残る）。macOS（`NSSharingServicePicker`）・iOS
//! （`UIActivityViewController`）・Android（`Intent.createChooser`、`ShareSheetHelper` を `ShareSheetLifecycle` が束ねる）・
//! Windows（`DataTransferManager`）が実装し、他は `Unsupported`。結果は OS のコールバックが `RequestSlot` へ置き、
//! effect が毎フレーム拾う pull 型。Windows は「共有 UI を閉じた」イベントが無いため、フォーカスの復帰を
//! `fandhe_runtime::HostView::focused` で観測してキャンセルを推定する（egui を知らない）。
//! 守る範囲は 1 プロセス内の要求順序・失敗を成功と見せないこと・Android の受信器の解除。
//! 守らない範囲は共有 UI 表示中の画面の再生成（後から届く結果は捨てる）・共有先アプリの実際の送信結果・
//! Windows のフォーカス推定のずれ。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `platform_share()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ShareSheetError;
