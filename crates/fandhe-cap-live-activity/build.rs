//! Swift 薄層のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]` と一致させる。
//!
//! `ActivityKit` の `Activity<Attributes>` は Swift 専用 API で ObjC から呼べないため、`cc` ではなく swiftc を使う。
//! Apple ツール（`xcrun`）が無いホスト（Linux の型検査経路。`docs/operations/ios-build-pipeline.md`）では警告を出して
//! 静的ライブラリの生成をスキップする。この場合 `cargo check` は通るが、リンクはシンボル未解決で失敗する。
//! `FANDHE_REQUIRE_LIVE_ACTIVITY=1`（本番・配布ビルド）ではスキップせず panic する。swiftc 自体の失敗は常に panic。
//!
//! swiftc が出す弱リンク・`-rpath` などの `rustc-link-arg` は、この crate の成果物にしか効かない。最終成果物
//! （`fandhe-gallery`）へは `export_link_args`（`Cargo.toml` の `links`）経由で届ける。

use fandhe_platform_apple::build::{SwiftLib, compile_swift, export_link_args, target_is_ios};

fn main() {
    if target_is_ios() {
        compile_swift(&SwiftLib {
            srcs: &[
                "ios/fandhe_live_activity_attributes.swift",
                "ios/fandhe_ios_live_activity.swift",
            ],
            lib: "fandhe_ios_live_activity",
            module: "FandheLiveActivityBridge",
            framework: "ActivityKit",
            weak: false,
            require_env: "FANDHE_REQUIRE_LIVE_ACTIVITY",
            tag: "live_activity",
        });
    }
    export_link_args();
}
