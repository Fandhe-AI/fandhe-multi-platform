//! macOS の ObjC 実装（`macos/fandhe_macos_share_receive.m`）をコンパイルする。
//!
//! 呼び出し元は `src/macos.rs`（C ABI 経由）。コンパイルとリンクの手順は
//! `fandhe-platform-apple`（feature `build`）の helper に任せる。macOS 以外では何もしない。

use fandhe_platform_apple::build::{compile_objc, target_is_macos};

fn main() {
    if target_is_macos() {
        compile_objc(
            "macos/fandhe_macos_share_receive.m",
            &["AppKit", "Foundation"],
        );
    }
}
