//! Apple（iOS / macOS）のリンク指定を `fandhe-egui-proxy` から受け取って出し直す。
//!
//! `cargo:rustc-link-arg` は出した package 自身の成果物にしか効かない。最終成果物（bin・cdylib・
//! テスト実行体）は本 crate が作るため、egui-proxy の `build.rs` が `links` メタデータとして公開した
//! 弱リンク・`-rpath`・clang_rt・`Info.plist` 埋め込みをここで出し直す
//! （`fandhe-platform-apple` の `build` モジュール doc 参照）。Apple 以外では何もしない。

fn main() {
    fandhe_platform_apple::build::import_link_args("fandhe_egui_proxy");
}
