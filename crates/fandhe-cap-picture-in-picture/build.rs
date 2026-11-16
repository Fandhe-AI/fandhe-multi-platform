//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。
//! iOS と macOS は同じ `.m` を共有する（UI 用 framework だけが違う: iOS は UIKit、macOS は AppKit）。
//! macOS 12.0+ / iOS 15.0+ の API は `.m` 側の `@available` で守る。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos, ui_framework};

fn main() {
    if target_is_ios() || target_is_macos() {
        compile_objc(
            "apple/fandhe_apple_picture_in_picture.m",
            &[
                ui_framework(),
                "AVFoundation",
                "AVKit",
                "CoreMedia",
                "CoreVideo",
                "Foundation",
                "QuartzCore",
            ],
        );
    }
}
