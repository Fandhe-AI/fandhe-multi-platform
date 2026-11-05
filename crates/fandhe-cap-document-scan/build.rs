//! iOS のときだけ `ios/fandhe_ios_document_scan.m`（VisionKit の `VNDocumentCameraViewController`）を
//! コンパイルしてリンクする。手順は `fandhe-platform-apple` の helper に集約している。
//! iOS 以外（macOS・Android・Windows・Linux・wasm32）では何もしない。

use fandhe_platform_apple::build::{compile_objc, target_is_ios};

fn main() {
    if target_is_ios() {
        compile_objc(
            "ios/fandhe_ios_document_scan.m",
            &["AVFoundation", "UIKit", "VisionKit"],
        );
    }
}
