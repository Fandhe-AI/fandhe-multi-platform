//! TASK-2.2（REQ-2 / M3）向けプロトタイプアプリの共通コードクレート。
//!
//! 親 issue #47（TASK-2.2）は、共通コードベース比率（PoC-1 実測 70/126 行 ≈ 55.6%）を
//! 3〜5 画面規模のアプリで再計測するためのプロトタイプ実装である。本クレートは
//! その「共通コード」側（分子・分母双方に算入される部分）を独立させたもので、
//! `fandhe-core`（TASK-1.1）が提供する `Screen<S>` / `Node<S>` DSL のみに依存する。
//! プラットフォーム側（`fandhe-egui-proxy` 等）から起動するために追加する差分は
//! 「プラットフォーム別コード」（分母のみに算入）として本クレートの外側に置く方針
//! （設計の詳細は `docs/architecture/prototype-app-design.md` を参照）。
//!
//! 画面構成は「タスクメモ」アプリを題材に、一覧・詳細・設定の 3 画面＋ルーティングで
//! 構成する（本 issue #48 = TASK-2.2-S1 のスコープ）。各画面の実装は sub-issue へ分担した:
//! - 一覧画面（[`screens::list`]）: S2 #49
//! - 詳細画面（[`screens::detail`]）: S3 #50
//! - 設定画面（[`screens::settings`]）: S4 #51
//! - 画面遷移ハンドラ本体・状態共有・統合テスト（[`nav`]）: S5 #52（実装済み）
//! - 共通/プラットフォーム別コードの分類・行数計測: S6 #53
//!
//! 画面遷移ハンドラ（`go_to_list` / `go_to_detail` / `go_to_settings` /
//! `select_next` / `select_prev`）は [`nav`] に一本化されており、各画面はこれらの
//! 関数ポインタをそのまま `Screen::button` へ渡す（画面モジュール内に重複定義しない）。
#![forbid(unsafe_code)]

pub mod nav;
pub mod screens;
pub mod state;

pub use nav::current_screen;
pub use state::{AppState, Item, Route, Settings};
