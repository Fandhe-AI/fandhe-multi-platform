//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。
//! iOS と macOS は同じ `.m` を共有する（`TARGET_OS_IPHONE` で分岐。macOS は UIKit を使わない）。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};

fn main() {
    if target_is_ios() {
        compile_objc(
            "ios/fandhe_ios_camera_preview_stream.m",
            &[
                "AVFoundation",
                "CoreMedia",
                "CoreVideo",
                "Foundation",
                "UIKit",
            ],
        );
    } else if target_is_macos() {
        compile_objc(
            "ios/fandhe_ios_camera_preview_stream.m",
            &["AVFoundation", "CoreMedia", "CoreVideo", "Foundation"],
        );
    }
}
