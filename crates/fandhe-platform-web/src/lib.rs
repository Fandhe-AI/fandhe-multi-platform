//! fandhe-multi-platform の Web 共通層（TASK-21.4 / Issue #1185、REQ-21）。
//!
//! 端末機能の Web 実装が共有する部品を、DOM レンダラ（`fandhe-web-dom`）から切り出した crate。
//! `fandhe-platform-android` / `fandhe-platform-apple` と同じ位置づけで、機能 crate（`fandhe-cap-*`）が
//! 依存できるよう workspace 内の他 crate（`fandhe-core` を含む）に依存しない。
//!
//! # 公開するもの
//!
//! - 機能向け（`wasm32`）: [`Applier`]・[`WebEffect`]（状態の書き戻し）、[`is_live`]・[`root_of`]
//!   （マウントの生存確認）、[`on_mount_release`]（破棄時の解放の登録）、[`query_permission`]
//! - レンダラ向け（`wasm32`）: [`register_mount`]・`Applier::new`
//! - 全ターゲット: [`query_param_bytes`]（URL クエリの復号、純関数）
//!
//! # `web-sys` の feature
//!
//! 本 crate は自分が使う分だけ宣言する。機能 crate は自分が使う feature を自分の `Cargo.toml`
//! （`wasm32` の target 依存）に書く。Cargo が和集合にするため、共通層へ集める必要はない。
//!
//! # 機能を crate へ移すとき（解放フック）
//!
//! 破棄時の `release_mount` は、機能 crate が effect の入口で `on_mount_release("<id>", release_mount)`
//! を呼んで登録する。未移行の機能は `fandhe-web-dom` の `mount::release_features` がまとめて登録する
//! ため、機能を移すときはその機能の行を `release_features` から消す。
//!
//! # 守る範囲
//!
//! 1 ページ内の単一スレッド（`wasm32`）。複数タブ・複数ページ間の整合は範囲外。
//!
//! `#![forbid(unsafe_code)]`: `wasm-bindgen` の `Closure` は safe API で、本 crate は `unsafe` を要求しない。

#![forbid(unsafe_code)]

mod query;
pub use query::query_param_bytes;

#[cfg(any(target_arch = "wasm32", test))]
mod release_hooks;

#[cfg(target_arch = "wasm32")]
mod mount;
#[cfg(target_arch = "wasm32")]
mod permission;

#[cfg(target_arch = "wasm32")]
pub use mount::{
    Applier, RenderTrigger, WebEffect, is_live, on_mount_release, register_mount, root_of,
};
#[cfg(target_arch = "wasm32")]
pub use permission::query_permission;
