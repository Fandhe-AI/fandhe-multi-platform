//! `Node::RgbaImage`（実画素の画像ノード、Issue #5）の egui 描画。
//!
//! `lib.rs` の `render_node`（と `tv_focus.rs` の同名関数）から呼ばれる。画素は
//! `fandhe_core::RgbaPixels` が構築時に検証済み（長さ一致・寸法上限）のため、長さ不一致で
//! panic する `ColorImage::from_rgba_unmultiplied` へ安全に渡せる。
//!
//! `Screen` は毎フレーム作り直されるため、テクスチャは画面ツリー上の位置（`path`）を
//! キーに egui の一時データへ置き、画素が前フレームと同じ（`Arc` のポインタ一致で
//! 安価に判定される）間は再利用して毎フレームのアップロードを避ける。
//! ponytail: path ごとに 1 枚を保持するだけで、画面を離れても次に上書きされるまで残る。
//! 数は path の種類で上限があるため GC は持たない（問題になったら足す）。

use fandhe_core::RgbaPixels;

/// 画素を実描画する。`alt` は AccessKit の Image ノードのラベルになる。
pub(crate) fn show(ui: &mut egui::Ui, pixels: &RgbaPixels, alt: &str, path: &[usize]) {
    let id = egui::Id::new(("fandhe-rgba-image", path));
    let cached: Option<(RgbaPixels, egui::TextureHandle)> = ui.ctx().data(|d| d.get_temp(id));
    let texture = match cached {
        Some((old, tex)) if &old == pixels => tex,
        _ => {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [pixels.width() as usize, pixels.height() as usize],
                pixels.rgba(),
            );
            let tex =
                ui.ctx()
                    .load_texture("fandhe-rgba-image", image, egui::TextureOptions::LINEAR);
            ui.ctx()
                .data_mut(|d| d.insert_temp(id, (pixels.clone(), tex.clone())));
            tex
        }
    };
    ui.add(
        egui::Image::new(&texture)
            .alt_text(alt)
            .max_width(ui.available_width())
            .max_height(320.0),
    );
}
