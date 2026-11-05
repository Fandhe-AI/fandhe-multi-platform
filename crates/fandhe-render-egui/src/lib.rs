//! fandhe-render-egui: `fandhe-core` の宣言を egui で実描画し、各 OS 向けの起動までを担うレンダラ。
//!
//! `Screen<S>` / `Node<S>` の宣言的 UI を egui ウィジェットへ写し（[`render_screen`]・
//! [`EguiRenderer`]）、`StateStore` を保持して毎フレーム駆動するアプリ本体（[`app::ProxyApp`]）と、
//! デスクトップ・iOS・Android・Web の起動口（[`launch`]）を持つ。
//!
//! 依存方向は `fandhe-render-egui → fandhe-runtime → fandhe-core` の一方向
//! （`docs/architecture/consumer-app-structure.md` §3・§7、`.claude/rules/coding-rust.md`）。
//! eframe はこのクレートの中だけで使い、公開 API に eframe の型を出さない
//! （`eframe::App`・`CreationContext`・`NativeOptions`・`Frame`・`eframe::Result`）。
//! 端末機能（保存・カメラ・OS 連携）は持たない。端末機能の配線は旧構造の `fandhe-egui-proxy` が
//! 担い、終了・破棄の解放処理だけは [`app::set_teardown_hooks`] で登録して呼ぶ。
//!
//! 全ターゲットで `#![forbid(unsafe_code)]` を維持する。`android_main` の `#[no_mangle]` や
//! iOS・Windows の FFI は `fandhe-egui-proxy` 側に置く（coding-rust.md の FFI 例外規定）。
//!
//! 仕様との対応: REQ-21 / TASK-21.2（Issue #1164）。
#![forbid(unsafe_code)]

pub mod accessibility;
pub mod app;
pub mod font;
// 旧構造（`fandhe-egui-proxy` の端末機能の配線）専用の暫定の口。#1194 で撤去する。
#[doc(hidden)]
pub mod image_store;
pub mod launch;
pub mod paint_command;
pub mod recyclers;
mod render;
#[doc(hidden)]
pub mod rgba_image;
pub mod text_role;

pub use app::{Completion, CompletionSender, EffectFn, ProxyApp, ProxyAppDyn};
pub use launch::{EguiApp, LaunchContext, LaunchError};
pub use recyclers::VirtualListRecyclers;
pub use render::{
    dispatch_back_if_requested, render_screen, render_screen_with_recyclers, save_color_image,
    EguiRenderer,
};
