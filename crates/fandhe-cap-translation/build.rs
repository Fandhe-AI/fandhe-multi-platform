//! Swift 薄層のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//!
//! `Translation` は macOS 15 / iOS 18 以降の Swift 専用 API で ObjC から呼べないため、`cc` ではなく swiftc を使い、
//! 古い OS でも起動できるよう弱リンクにする。Apple ツール（`xcrun`）が無いホストでは警告を出して静的ライブラリの生成を
//! スキップする。`FANDHE_REQUIRE_TRANSLATION=1`（本番・配布ビルド）ではスキップせず panic する。
//!
//! swiftc が出す弱リンク・`-rpath` などの `rustc-link-arg` は、この crate の成果物にしか効かない。最終成果物
//! （`fandhe-gallery`）へは `export_link_args`（`Cargo.toml` の `links`）経由で届ける。

use fandhe_platform_apple::build::{
    SwiftLib, compile_swift, export_link_args, target_is_ios, target_is_macos,
};

fn main() {
    if target_is_ios() || target_is_macos() {
        compile_swift(&SwiftLib {
            srcs: &["apple/fandhe_apple_translation.swift"],
            lib: "fandhe_apple_translation",
            module: "FandheTranslationBridge",
            framework: "Translation",
            weak: true,
            require_env: "FANDHE_REQUIRE_TRANSLATION",
            tag: "translation",
        });
    }
    export_link_args();
}
