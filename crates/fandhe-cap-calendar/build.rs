//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};

fn main() {
    if target_is_ios() || target_is_macos() {
        // iOS は `EKEventEditViewController`（EventKitUI と UIKit）、macOS は EventKit だけをリンクする。
        let frameworks: &[&str] = if target_is_ios() {
            &["EventKit", "EventKitUI", "Foundation", "UIKit"]
        } else {
            &["EventKit", "Foundation"]
        };
        compile_objc("apple/fandhe_apple_calendar.m", frameworks);
    }
}
