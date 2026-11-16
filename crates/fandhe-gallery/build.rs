//! Apple（iOS / macOS）のリンク指定を `fandhe-egui-proxy` と Swift 薄層・弱リンクを持つ機能 crate から受け取って出し直す。
//!
//! `cargo:rustc-link-arg` は出した package 自身の成果物にしか効かない。最終成果物（bin・cdylib・
//! テスト実行体）は本 crate が作るため、egui-proxy の `build.rs` が `links` メタデータとして公開した
//! 弱リンク・`-rpath`・clang_rt・`Info.plist` 埋め込みをここで出し直す。Swift の薄層・弱リンクを持つ機能 crate
//! （image-generation・live-activity・on-device-llm・screen-capture・translation）も同じ形で公開しており、feature `device` が有効なときだけ受け取る
//! （`fandhe-platform-apple` の `build` モジュール doc 参照）。Apple 以外では何もしない。

fn main() {
    fandhe_platform_apple::build::import_link_args("fandhe_egui_proxy");
    // 機能 crate は feature `device` の依存（optional）。無効なら公開されないので受け取らない。
    if std::env::var_os("CARGO_FEATURE_DEVICE").is_some() {
        fandhe_platform_apple::build::import_link_args("fandhe_cap_image_generation");
        fandhe_platform_apple::build::import_link_args("fandhe_cap_live_activity");
        fandhe_platform_apple::build::import_link_args("fandhe_cap_on_device_llm");
        fandhe_platform_apple::build::import_link_args("fandhe_cap_screen_capture");
        fandhe_platform_apple::build::import_link_args("fandhe_cap_translation");
    }
}
