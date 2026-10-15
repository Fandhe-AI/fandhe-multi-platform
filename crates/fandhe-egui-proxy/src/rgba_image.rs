//! `Node::RgbaImage`（実画素の画像ノード、Issue #5）の egui 描画。
//!
//! `lib.rs` の `render_node`（と `tv_focus.rs` の同名関数）から呼ばれる。画素は
//! `fandhe_core::RgbaPixels` が構築時に検証済み（長さ一致・寸法上限）のため、長さ不一致で
//! panic する `ColorImage::from_rgba_unmultiplied` へ安全に渡せる。
//!
//! `Screen` は毎フレーム作り直されるため、テクスチャは画面ツリー上の位置（`path`）を
//! キーに egui の一時データ（[`Cache`] 1 個）へ置き、画素が前フレームと同じ（`Arc` の
//! ポインタ一致で安価に判定される）間は再利用して毎フレームのアップロードを避ける。
//! 前フレームで描画されなかったエントリは [`begin_frame`]（`render_screen_with_recyclers`
//! が毎フレーム冒頭で呼ぶ）が捨てる。仮想リストでスクロールアウトした項目や離脱した画面の
//! 画素・GPU テクスチャは 1 フレーム後に解放され、メモリは可視項目数で有界になる。

use std::collections::HashMap;

use fandhe_core::RgbaPixels;

/// path キーごとの (最終描画パス, 画素, テクスチャ)。
type Cache = HashMap<egui::Id, (u64, RgbaPixels, egui::TextureHandle)>;

fn cache_id() -> egui::Id {
    egui::Id::new("fandhe-rgba-image-cache")
}

/// フレーム冒頭で、直前のパスで描画されなかったエントリを解放する。
pub(crate) fn begin_frame(ctx: &egui::Context) {
    let now = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        if let Some(cache) = d.get_temp::<Cache>(cache_id()) {
            let mut cache = cache;
            cache.retain(|_, (seen, _, _)| seen.saturating_add(1) >= now);
            d.insert_temp(cache_id(), cache);
        }
    });
}

/// テスト用: キャッシュ中のエントリ数。
#[cfg(test)]
pub(crate) fn cached_len(ctx: &egui::Context) -> usize {
    ctx.data(|d| d.get_temp::<Cache>(cache_id()))
        .map_or(0, |c| c.len())
}

/// 画素を実描画する。`alt` は AccessKit の Image ノードのラベルになる。
pub(crate) fn show(ui: &mut egui::Ui, pixels: &RgbaPixels, alt: &str, path: &[usize]) {
    let ctx = ui.ctx().clone();
    let now = ctx.cumulative_pass_nr();
    let id = egui::Id::new(("fandhe-rgba-image", path));
    let mut cache: Cache = ctx.data(|d| d.get_temp(cache_id())).unwrap_or_default();
    let texture = match cache.get(&id) {
        Some((_, old, tex)) if old == pixels => tex.clone(),
        _ => {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [pixels.width() as usize, pixels.height() as usize],
                pixels.rgba(),
            );
            ctx.load_texture("fandhe-rgba-image", image, egui::TextureOptions::LINEAR)
        }
    };
    cache.insert(id, (now, pixels.clone(), texture.clone()));
    ctx.data_mut(|d| d.insert_temp(cache_id(), cache));
    ui.add(
        egui::Image::new(&texture)
            .alt_text(alt)
            .max_width(ui.available_width())
            .max_height(320.0),
    );
}
