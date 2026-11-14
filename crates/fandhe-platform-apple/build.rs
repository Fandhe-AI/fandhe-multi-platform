//! UIScene 化シム（`ios/fandhe_ios_scene.m`、Issue #27・#1292）のコンパイル。
//!
//! Info.plist の `UIApplicationSceneManifest` は 1 つで feature・レンダラに関係しないため、iOS では常に
//! コンパイルする。手順は自身の `build` helper を読み込んで使う（他の機能 crate と同じ `compile_objc`）。
//! `ios/fandhe_ios_app_delegate.m`（Issue #1325）も同様に常にコンパイルする。リンカは登録関数を参照する機能が
//! ある成果物にだけ取り込む。iOS 以外では何もしない。

#[path = "src/build.rs"]
#[allow(dead_code)]
mod build;

fn main() {
    if build::target_is_ios() {
        build::compile_objc("ios/fandhe_ios_scene.m", &["Foundation", "UIKit"]);
        // 旧経路の URL・背景転送の完了を機能へ配る delegate（Issue #1325）。
        build::compile_objc("ios/fandhe_ios_app_delegate.m", &["Foundation", "UIKit"]);
    }
}
