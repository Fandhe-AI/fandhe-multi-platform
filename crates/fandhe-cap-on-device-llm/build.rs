//! Swift 薄層のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//!
//! `FoundationModels` は macOS 26 / iOS 26 以降の Swift 専用 API で ObjC から呼べないため、`cc` ではなく swiftc を使い、
//! 古い OS でも起動できるよう弱リンクにする。Apple ツール（`xcrun`）が無いホストでは警告を出して静的ライブラリの生成を
//! スキップする。`FANDHE_REQUIRE_ON_DEVICE_LLM=1`（本番・配布ビルド）ではスキップせず panic する。
//!
//! swiftc が出す弱リンク・`-rpath` などの `rustc-link-arg` は、この crate の成果物にしか効かない。最終成果物
//! （`fandhe-gallery`）へは `export_link_args`（`Cargo.toml` の `links`）経由で届ける。

use fandhe_platform_apple::build::{
    SwiftLib, compile_swift, export_link_args, target_is_ios, target_is_macos,
};

fn main() {
    if target_is_ios() || target_is_macos() {
        compile_swift(&SwiftLib {
            srcs: &["apple/fandhe_apple_on_device_llm.swift"],
            lib: "fandhe_apple_on_device_llm",
            module: "FandheOnDeviceLlmBridge",
            framework: "FoundationModels",
            weak: true,
            require_env: "FANDHE_REQUIRE_ON_DEVICE_LLM",
            tag: "on_device_llm",
        });
    }
    export_link_args();
}
