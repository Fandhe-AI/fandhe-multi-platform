//! 機能 crate の `[package.metadata.fandhe]` 宣言の形式・検証・統合規則と、ネイティブ設定の生成
//! （REQ-21 / TASK-21.2・21.3、Issue #1160・#1176）。
//!
//! - `declaration` モジュール - 宣言の型と検証規則。不正な宣言はここで拒否する
//! - `merge` モジュール - 複数機能の宣言を 1 つへ統合する規則。`native` が入力に使う
//! - [`check_workspace`] - `cargo metadata` で解決した全 crate の宣言を検証する（統合・到達判定はしない）
//! - `reach` モジュール - アプリが依存として解決した機能 crate の到達判定（feature の活性化を自前で辿る）
//! - `native` モジュール - 到達した機能の宣言だけからネイティブ設定（Manifest・Gradle・Info.plist・entitlements、Web は `--os web` で Web App Manifest）を生成する（`cargo fandhe gen`。`gen` は 2024 edition の予約語のため別名）
//! - [`eject`] - 生成した設定ファイルを `<アプリの crate>/eject/` へ書き出す（`cargo fandhe gen --eject`、#1202）。`eject/` のファイルは gen・run・build が生成の代わりに使う
//! - `run` モジュール - アプリの実行・ビルド（`cargo fandhe run|build`、デスクトップ・Web・Android、#1422・#1423）。手順は純粋関数で組み、実行器が順に走らせる
//! - `scope` モジュール - 選ばなかった機能の宣言が成果物に無いことの照合（`cargo fandhe verify-scope`、#1178）
//!
//! 形式の仕様は `docs/architecture/capability-declaration.md`。
//! 値は XML・plist・Gradle へ展開されるため、エスケープは生成側（`native`）の責務とし、
//! 宣言の検証では識別子の形式と制御文字を拒否する。

#![forbid(unsafe_code)]

pub mod declaration;
pub mod merge;
pub mod native;
mod reach;
pub mod run;
mod scope;
mod workspace;

pub use declaration::{check_paths, parse, validate, Declaration, Problem};
pub use merge::{merge, Merged};
pub use native::{
    eject, generate, generate_web, write, Generated, ANDROID_SKIPPED_NOTE, EJECT_NOTE,
};
pub use scope::{verify_scope, ScopeReport};
pub use workspace::check_workspace;
