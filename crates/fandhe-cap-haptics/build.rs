//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};

fn main() {
    if target_is_ios() || target_is_macos() {
        // macOS は AppKit、iOS は CoreHaptics（`supportsHaptics` 判定）と UIKit をリンクする。
        let frameworks: &[&str] = if target_is_ios() {
            &["CoreHaptics", "Foundation", "UIKit"]
        } else {
            &["AppKit", "Foundation"]
        };
        compile_objc("apple/fandhe_apple_haptics.m", frameworks);
    }
}
