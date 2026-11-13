//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。

use fandhe_platform_apple::build::{compile_objc, target_is_ios};

fn main() {
    // `.m` は `TARGET_OS_IPHONE` ガードで中身を囲んでいる。macOS では使わないため iOS のときだけコンパイルする。
    if target_is_ios() {
        compile_objc("apple/fandhe_apple_text_scale.m", &["UIKit"]);
    }
}
