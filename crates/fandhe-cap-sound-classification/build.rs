//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};

fn main() {
    if target_is_ios() || target_is_macos() {
        // SoundAnalysis は macOS 10.15・iOS 13 からあるため強リンクでよい（内蔵分類器は `.m` の `@available` で守る）。
        let frameworks: &[&str] = if target_is_ios() {
            &["AVFoundation", "Foundation", "SoundAnalysis", "UIKit"]
        } else {
            &["AVFoundation", "Foundation", "SoundAnalysis"]
        };
        compile_objc("apple/fandhe_apple_sound_classification.m", frameworks);
    }
}
