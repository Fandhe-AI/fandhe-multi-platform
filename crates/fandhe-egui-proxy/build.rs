//! iOS / macOS 向け ObjC・Swift 薄層（`ios/*`・`apple/*`・`macos/*`）のビルドスクリプト。
//!
//! 位置づけ: 端末機能の ObjC 実装を `fandhe-egui-proxy` がまとめてコンパイル・リンクする入口。
//! コンパイルとリンクの手順そのものは `fandhe-platform-apple`（feature `build`）の helper に集約しており、
//! 本ファイルは「どの `.m` / Swift をどの framework でビルドするか」の一覧だけを持つ
//! （機能 crate への分離が進めば、各機能の `build.rs` が同じ helper を使う。TASK-21.2 / Issue #1168）。
//!
//! - `CARGO_CFG_TARGET_OS` が `ios` / `macos` 以外（Windows・Linux・wasm32・Android）では即座に return する
//!   no-op で、既存ビルドへの影響はない。
//! - UIScene 化シム（Issue #27）は `fandhe-platform-apple` へ移した（Issue #1292。レンダラ非依存のため）。
//!   `ios/fandhe_ios_text_input.m`（Issue #795）は `src/ios_text_input.rs` が外れる計測ビルド
//!   （`cold-start-probe`・`frame-metrics`）ではコンパイルしない（Issue #19）。winit の補完で起動経路の
//!   責務のため、本 crate に残す。
//! - それ以外の端末機能は feature `device`（既定 on）でのみコンパイルする。無効時は呼び出し元の
//!   `src/device/*` がビルドされないため、リンクエラーを避けて素通りする。
//! - framework は `.m` / Swift ごとに自己完結して宣言する（他の単位のリンクに頼らない）。
//!   各 `compile_*` の doc に、対象 OS・呼び出し元・Issue 番号を書く。
//! - macOS の `Info.plist` 埋め込み（`-sectcreate`）は最終実行体の設定。`fandhe-gallery` へ転送する。
//! - Swift 薄層の弱リンク・`-rpath`・clang_rt・`-sectcreate` は `rustc-link-arg` では依存元へ伝わらない。
//!   最終成果物は `fandhe-gallery` へ移ったため、`links` メタデータで転送し、gallery の `build.rs` が
//!   出し直す（`fandhe-platform-apple` の `build` モジュール doc 参照）。

use fandhe_platform_apple::build::{
    bin_link_arg, compile_objc, export_link_args, target_is_ios, target_is_macos,
};

fn main() {
    build();
    // 最終成果物（`fandhe-gallery`）へ Apple のリンク指定を転送する（`Cargo.toml` の `links`）。
    export_link_args();
}

fn build() {
    if target_is_macos() {
        build_macos();
        return;
    }
    if !target_is_ios() {
        return;
    }

    // 計測専用ビルドでは `src/ios_text_input.rs`（`#[no_mangle]` の受け口）が `lib.rs` の
    // cfg で外れるため、呼び出し側の `.m` も同じ条件で外す（Issue #19）。
    let measure_build = std::env::var("CARGO_FEATURE_COLD_START_PROBE").is_ok()
        || std::env::var("CARGO_FEATURE_FRAME_METRICS").is_ok();
    if !measure_build {
        compile_objc("ios/fandhe_ios_text_input.m", &["Foundation", "UIKit"]);
    }

    // feature `device`（既定 on）でのみ `fandhe-device` への依存自体が有効になる
    // （`Cargo.toml` の native 分岐参照）。無効時に ObjC をコンパイルしても
    // 呼び出し元（`src/device/ios.rs`）がビルドされないため、リンクエラーを
    // 避けるべく feature 無効時は素通りする。
    if std::env::var("CARGO_FEATURE_DEVICE").is_err() {
        return;
    }

    // `ios/fandhe_ios_photo_picker.m`（写真選択）。
    compile_objc(
        "ios/fandhe_ios_photo_picker.m",
        &["ImageIO", "PhotosUI", "UIKit", "UniformTypeIdentifiers"],
    );

    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も iOS 分岐と共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_barcode_scan.m`（Issue #557 macOS + #558 iOS）も共用する。
    compile_apple_barcode_scan();
}

/// `apple/fandhe_apple_barcode_scan.m` を `cc` でコンパイルし Vision をリンクする
/// （Issue #557 macOS、iOS は #558 が利用、バーコード・QR 読み取り）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_barcode_scan`）と
/// 同じ OS 集合に保つ。
fn compile_apple_barcode_scan() {
    compile_objc(
        "apple/fandhe_apple_barcode_scan.m",
        &["Foundation", "Vision"],
    );
}

/// `apple/fandhe_apple_image_recognition.m` を `cc` でコンパイルし Vision をリンクする
/// （Issue #1250 macOS、iOS は #1252 が利用、画像の文字認識）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_image_recognition`）と
/// 同じ OS 集合に保つ。
fn compile_apple_image_recognition() {
    compile_objc(
        "apple/fandhe_apple_image_recognition.m",
        &["Foundation", "Vision"],
    );
}

/// macOS ターゲット向けビルド（Issue #889・#897）。iOS と異なり macOS は位置情報
/// （機能 crate `fandhe-cap-location` が持つ）などの `apple/*.m` だけを対象とする。既存の `ios/*.m` 群（文字入力・写真選択）は iOS 専用のため対象外。
///
/// feature `device`（既定 on）でのみ `fandhe-device` への依存自体が有効になり、
/// 呼び出し元の Rust がビルド対象になる（iOS 分岐と同じ判断）。無効時はコンパイルもリンクも行わない。
fn build_macos() {
    if std::env::var("CARGO_FEATURE_DEVICE").is_err() {
        return;
    }

    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_barcode_scan.m`（Issue #557 macOS + #558 iOS）も共用する。
    compile_apple_barcode_scan();

    // 素の実行体（`fandhe-gallery` の bin）へ `macos/Info.plist`
    // （`CFBundleIdentifier` + `NSLocationUsageDescription`）を埋め込む。macOS 10.15+
    // は Info.plist を持たない実行体に CoreLocation の許可プロンプトを一切出さない
    // ため（CoreLocationCLI の既知要件、実装計画 §3.2）。最終実行体は `fandhe-gallery` へ移ったので、
    // 本 crate は bin 専用の指定として記録するだけにし、gallery の `build.rs` が
    // `import_link_args` で `rustc-link-arg-bins` に出し直す（テスト実行体・rlib には影響しない）。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    println!("cargo:rerun-if-changed=macos/Info.plist");
    bin_link_arg(&format!(
        "-Wl,-sectcreate,__TEXT,__info_plist,{manifest_dir}/macos/Info.plist"
    ));
}
