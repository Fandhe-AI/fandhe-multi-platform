//! `Node::RgbaImage`（Issue #5）が egui で描画され、alt が AX の Image ノードとして
//! 公開されることの結合テスト。`ax_tree.rs` と同じく既定レンダラ（wgpu 不要）を使い、
//! AX ツリーが得られる macOS / Android に限定する。

#![cfg(any(target_os = "macos", target_os = "android"))]

use egui_kittest::kittest::Queryable;
use fandhe_core::{RgbaPixels, Screen, StateStore};

struct S;

#[test]
fn rgba_image_is_exposed_as_image_with_alt() {
    let pixels = RgbaPixels::new(2, 2, vec![255u8; 16]).expect("有効な画素");
    let screen: Screen<S> = Screen::new("img").rgba_image(pixels, "撮影結果のサムネイル");
    let mut harness = egui_kittest::Harness::builder().build_ui_state(
        move |ui, store: &mut StateStore<S>| {
            fandhe_egui_proxy::render_screen(ui, &screen, store);
        },
        StateStore::new(S),
    );
    harness.run();
    let _ = harness.get_by_role_and_label(egui::accesskit::Role::Image, "撮影結果のサムネイル");
}
