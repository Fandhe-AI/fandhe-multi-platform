//! ObjC 実装のコンパイル。コンパイルとリンクの手順は `fandhe-platform-apple`（feature `build`）の helper に集約している。
//! framework の宣言は `Cargo.toml` の `[package.metadata.fandhe.ios]`（macOS は `.macos`）と一致させる。
//!
//! macOS の ScreenCaptureKit（12.3+、`SCRecordingOutput` は 15+）は実行体の `MACOSX_DEPLOYMENT_TARGET`（11.0）より
//! 新しいため**弱リンク**にする。iOS は ReplayKit（iOS 14+ の API は `.m` 側で `@available` ガード）で、
//! ScreenCaptureKit の弱リンクは持ち込まない。弱リンクの `rustc-link-arg` は出した package 自身の成果物にしか効かないため、
//! 最終成果物（`fandhe-gallery`）へは `export_link_args`（`Cargo.toml` の `links`）経由で届ける。

use fandhe_platform_apple::build::{
    compile_objc, export_link_args, target_is_ios, target_is_macos, weak_link_framework,
};

fn main() {
    if target_is_ios() {
        compile_objc(
            "apple/fandhe_apple_screen_capture.m",
            &["AVFoundation", "ReplayKit"],
        );
    } else if target_is_macos() {
        compile_objc("apple/fandhe_apple_screen_capture.m", &["AVFoundation"]);
        weak_link_framework("ScreenCaptureKit");
    }
    export_link_args();
}
