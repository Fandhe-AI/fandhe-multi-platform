//! iOS の ObjC 実装（`ios/fandhe_ios_camera.m`）のコンパイル。
//!
//! コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! iOS 以外では何もしない。framework の宣言（`AVFoundation` 等）は `Cargo.toml` の
//! `[package.metadata.fandhe.ios]` と一致させる（cargo-fandhe の突き合わせテストが検証する）。

use fandhe_platform_apple::build::{compile_objc, target_is_ios};

fn main() {
    if target_is_ios() {
        compile_objc(
            "ios/fandhe_ios_camera.m",
            &["AVFoundation", "CoreGraphics", "UIKit"],
        );
    }
}
