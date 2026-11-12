//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。

use fandhe_platform_apple::build::{compile_objc, target_is_ios, target_is_macos};

fn main() {
    if target_is_ios() {
        compile_objc("ios/fandhe_ios_spell_check.m", &["Foundation", "UIKit"]);
    }
    if target_is_macos() {
        compile_objc(
            "macos/fandhe_macos_spell_check.m",
            &["AppKit", "Foundation"],
        );
    }
}
