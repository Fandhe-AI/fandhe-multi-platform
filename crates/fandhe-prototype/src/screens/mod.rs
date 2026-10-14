//! 各画面のビルダー関数を集約するモジュール。
//!
//! [`crate::nav::current_screen`] がルーティング（[`crate::state::Route`]）に応じて
//! 本モジュールの関数を呼び分ける。各画面の実装は個別の sub-issue が分担する
//! （各ファイル冒頭のコメントを参照）。

pub mod detail;
pub mod list;
pub mod settings;

pub use detail::detail_screen;
pub use list::list_screen;
pub use settings::settings_screen;
