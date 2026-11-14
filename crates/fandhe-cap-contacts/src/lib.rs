//! 連絡先選択の機能 crate（REQ-21 / TASK-21.4、Issue #1330、手順は `docs/operations/capability-crate-migration.md`）。
//!
//! OS の連絡先ピッカーで利用者に 1 件を選ばせ、表示名だけを受け取る（連絡先の一覧取得・書き込み・名前以外の項目は扱わない）。
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、Android は `android`）を
//! 1 つに集める。macOS・iOS・Android は OS コールバックの結果を pull 型の bridge（`RequestSlot`）で受け、Web の実体は
//! `fandhe-web-dom`（契約の `validate_contact_name` だけ共有）、それ以外は `Unsupported` 固定。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`contact_pick_effect`）と、Activity 再生成時の
//! `reset_pending_tracking`（Android）。入口 [`platform_contacts`] は画面の状態型に依存しない。画面の状態型を参照してよいのは
//! `driver` だけ。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（キャンセルと失敗を区別し、古い世代の結果は捨てる）・
//! 名前（個人情報）をログへ出さないこと。守らない範囲: 連絡先アプリ側の挙動、再生成をまたいだ保留中の要求の復元（破棄する契約）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::ContactsError;
