//! 機能 crate の `[package.metadata.fandhe]` 宣言の形式・検証・統合（REQ-21 / TASK-21.2、Issue #1160）。
//!
//! - `declaration` モジュール - 宣言の型と検証規則。不正な宣言はここで拒否する
//! - `merge` モジュール - 複数機能の宣言を 1 つへ統合する規則。`cargo fandhe gen`（#1176）が入力に使う
//! - [`check_workspace`] - `cargo metadata` で解決した全 crate の宣言を検証して統合まで通す
//!
//! 形式の仕様は `docs/architecture/capability-declaration.md`。
//! 値は後段で XML・plist・Gradle へ展開されるため、エスケープは生成側の責務として、
//! ここでは識別子の形式と制御文字を拒否する。

#![forbid(unsafe_code)]

pub mod declaration;
pub mod merge;
mod workspace;

pub use declaration::{check_paths, parse, validate, Declaration, Problem};
pub use merge::{merge, Merged};
pub use workspace::check_workspace;
