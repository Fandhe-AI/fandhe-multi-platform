//! Apple（iOS / macOS）のリンク指定を `fandhe-egui-proxy` と Swift 薄層・弱リンクを持つ機能 crate から受け取って出し直し、
//! macOS では `cargo fandhe gen` の Info.plist を実行体へ埋め込む。
//!
//! `cargo:rustc-link-arg` は出した package 自身の成果物にしか効かない。最終成果物（bin・cdylib・
//! テスト実行体）は本 crate が作るため、egui-proxy の `build.rs` が `links` メタデータとして公開した
//! 弱リンク・`-rpath`・clang_rt をここで出し直す。Swift の薄層・弱リンクを持つ機能 crate
//! （alarm・image-generation・live-activity・on-device-llm・screen-capture・translation）も同じ形で公開しており、ここで受け取る
//! （`fandhe-platform-apple` の `build` モジュール doc 参照）。Apple 以外では何もしない。
//!
//! macOS の Info.plist（Issue #1194）: 素の実行体（非バンドル）でも TCC の許可プロンプト（位置情報・マイク等）が出るよう、
//! `-sectcreate __TEXT __info_plist` で実行体へ埋め込む（macOS 10.15+ は埋め込みが無いと CoreLocation の許可を出さない）。
//! 中身は機能 crate の宣言と `[package.metadata.fandhe-app.macos]` から毎回生成し、手書きの Info.plist は持たない。
//! iOS・Android の生成物はバンドルを組むスクリプト（`scripts/ios/*`・`scripts/android/*`）が `cargo fandhe gen` で作る。

use std::path::{Path, PathBuf};

fn main() {
    fandhe_platform_apple::build::import_link_args("fandhe_egui_proxy");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_alarm");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_image_generation");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_live_activity");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_on_device_llm");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_screen_capture");
    fandhe_platform_apple::build::import_link_args("fandhe_cap_translation");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        embed_macos_info_plist();
    }
}

/// 生成した macOS の Info.plist を `OUT_DIR` に書き、bin へ埋め込む。宣言・アプリ設定の誤りはビルドを止める
/// （黙って埋め込まないと、許可プロンプトが出ないまま失敗扱いになる）。
fn embed_macos_info_plist() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo が設定する"));
    let generated = cargo_fandhe::generate(Some(&dir.join("Cargo.toml")))
        .unwrap_or_else(|e| panic!("cargo fandhe gen に失敗しました:\n{}", e.join("\n")));
    let plist = generated
        .files
        .get("macos/Info.plist")
        .expect("[package.metadata.fandhe-app.macos] bundle-id があれば必ず出る");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo が設定する")).join("Info.plist");
    std::fs::write(&out, plist).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        out.display()
    );
    // 生成の入力（workspace の各 crate の宣言・アプリ設定・依存の解決）が変わったときだけ作り直す。
    let root = dir.join("../..");
    for f in ["Cargo.toml", "Cargo.lock"] {
        rerun_if_changed(&root.join(f));
    }
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        for e in rd.flatten() {
            rerun_if_changed(&e.path().join("Cargo.toml"));
        }
    }
}

fn rerun_if_changed(p: &Path) {
    if p.is_file() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}
