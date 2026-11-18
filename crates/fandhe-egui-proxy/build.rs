//! iOS / macOS 向け ObjC・Swift 薄層（`ios/*`・`apple/*`・`macos/*`）のビルドスクリプト。
//!
//! 位置づけ: 端末機能の ObjC 実装を `fandhe-egui-proxy` がまとめてコンパイル・リンクする入口。
//! コンパイルとリンクの手順そのものは `fandhe-platform-apple`（feature `build`）の helper に集約しており、
//! 本ファイルは「どの `.m` / Swift をどの framework でビルドするか」の一覧だけを持つ
//! （機能 crate への分離が進めば、各機能の `build.rs` が同じ helper を使う。TASK-21.2 / Issue #1168）。
//!
//! - `CARGO_CFG_TARGET_OS` が `ios` / `macos` 以外（Windows・Linux・wasm32・Android）では即座に return する
//!   no-op で、既存ビルドへの影響はない。
//! - UIScene 化シム（Issue #27）は `fandhe-platform-apple` へ移した（Issue #1292。レンダラ非依存のため）。
//!   `ios/fandhe_ios_text_input.m`（Issue #795）は `src/ios_text_input.rs` が外れる計測ビルド
//!   （`cold-start-probe`・`frame-metrics`）ではコンパイルしない（Issue #19）。winit の補完で起動経路の
//!   責務のため、本 crate に残す。
//! - 端末機能の ObjC は機能 crate（`fandhe-cap-*`）の `build.rs` が持つ。`src/device/` は撤去した（Issue #1355）。
//! - framework は `.m` / Swift ごとに自己完結して宣言する（他の単位のリンクに頼らない）。
//! - macOS の `Info.plist` 埋め込み（`-sectcreate`）は最終実行体の設定のため、`fandhe-gallery` の `build.rs` が
//!   生成物から行う（#1194）。
//! - Swift 薄層の弱リンク・`-rpath`・clang_rt は `rustc-link-arg` では依存元へ伝わらない。
//!   最終成果物は `fandhe-gallery` へ移ったため、`links` メタデータで転送し、gallery の `build.rs` が
//!   出し直す（`fandhe-platform-apple` の `build` モジュール doc 参照）。

use fandhe_platform_apple::build::{compile_objc, export_link_args, target_is_ios};

fn main() {
    build();
    // 最終成果物（`fandhe-gallery`）へ Apple のリンク指定を転送する（`Cargo.toml` の `links`）。
    export_link_args();
}

fn build() {
    if !target_is_ios() {
        return;
    }

    // 計測専用ビルドでは `src/ios_text_input.rs`（`#[no_mangle]` の受け口）が `lib.rs` の
    // cfg で外れるため、呼び出し側の `.m` も同じ条件で外す（Issue #19）。
    let measure_build = std::env::var("CARGO_FEATURE_COLD_START_PROBE").is_ok()
        || std::env::var("CARGO_FEATURE_FRAME_METRICS").is_ok();
    if !measure_build {
        compile_objc("ios/fandhe_ios_text_input.m", &["Foundation", "UIKit"]);
    }

    // 端末機能の ObjC（写真選択・文字認識・バーコード読み取りを含む）は機能 crate（`fandhe-cap-*`）の `build.rs` が持つ（Issue #1355）。
}
