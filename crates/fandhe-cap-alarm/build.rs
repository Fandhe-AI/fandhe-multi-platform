//! iOS の Swift 薄層 `ios/fandhe_ios_alarm.swift` のコンパイル（Issue #1077）。コンパイルとリンクの手順は
//! `fandhe-platform-apple`（feature `build`）の helper に集約している。
//!
//! AlarmKit（iOS 26 以降）は Swift 専用 API で ObjC から呼べないため、`cc` ではなく swiftc を使い、古い OS でも起動できるよう
//! 弱リンクにする。Apple ツール（`xcrun`）が無いホストでは警告を出して静的ライブラリの生成をスキップする。
//! `FANDHE_REQUIRE_ALARM=1`（本番・配布ビルド）ではスキップせず panic する。
//!
//! swiftc が出す弱リンク・`-rpath` などの `rustc-link-arg` は、この crate の成果物にしか効かない。最終成果物
//! （`fandhe-gallery`）へは `export_link_args`（`Cargo.toml` の `links`）経由で届ける。

use fandhe_platform_apple::build::{SwiftLib, compile_swift, export_link_args, target_is_ios};

fn main() {
    if target_is_ios() {
        compile_swift(&SwiftLib {
            srcs: &["ios/fandhe_ios_alarm.swift"],
            lib: "fandhe_ios_alarm",
            module: "FandheIosAlarmBridge",
            framework: "AlarmKit",
            weak: true,
            require_env: "FANDHE_REQUIRE_ALARM",
            tag: "alarm",
        });
    }
    export_link_args();
}
