//! iOS の UIScene 化シムのリンク用アンカー（Issue #27）。`ios/fandhe_ios_scene.m` の
//! `FandheSceneDelegate`（Info.plist の `UIApplicationSceneManifest` が名前で参照する）を、
//! リンカに捨てさせないために `main`（`UIApplicationMain` より前）から 1 回呼ばれる。
//! winit 0.30 が scene 非対応な間の補完で、upstream が対応（winit#4723）したら本モジュールごと削除する。

// SAFETY: `ios/fandhe_ios_scene.m`（`build.rs` が iOS で常にコンパイル）が定義する引数なしの C 関数。
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_ios_scene_install();
}

/// `.m` のクラスを参照してリンクに残す。副作用なし・冪等。
#[allow(unsafe_code)]
pub fn install() {
    // SAFETY: 引数・戻り値なしで状態を持たない C 関数。
    unsafe { fandhe_ios_scene_install() }
}
