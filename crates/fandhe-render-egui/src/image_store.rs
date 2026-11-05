//! `ImageWidget` の実画素を egui のテクスチャとして描く側の窓口（Issue #1103、写真選択ほか）。
//!
//! 画素の保管所そのものはレンダラ非依存の `fandhe_runtime::image_store` にあり、effect ドライバ
//! （`fandhe-egui-proxy` の写真選択、機能 crate の書類スキャン等）が [`put`] / [`clear`] で置く。
//! 本モジュールは `lib.rs` の `Node::Widget` 分岐が [`texture`] で識別子が一致するときだけ
//! テクスチャを引くための、GPU へ上げる処理（通し番号が変わったときだけ再アップロード）を持つ。
//! 識別子が一致しない画素は描かない（古い結果が新しい表示へ混ざらない）。

use std::sync::Mutex;

pub use fandhe_runtime::image_store::{clear, put};

/// GPU へ上げ済みのテクスチャと、その元になった保管所の通し番号。
struct Cached {
    serial: u64,
    source: String,
    texture: egui::TextureHandle,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// `source` と識別子が一致するときだけテクスチャを返す（通し番号が変わったら GPU へ上げ直す）。
/// 一致しないときは `None` を返し、保持中のテクスチャは捨てない。
pub(crate) fn texture(ctx: &egui::Context, source: &str) -> Option<egui::TextureHandle> {
    let (serial, pixels) = fandhe_runtime::image_store::get(source)?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache
        .as_ref()
        .filter(|c| c.serial == serial && c.source == source)
    {
        return Some(c.texture.clone());
    }
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [pixels.width() as usize, pixels.height() as usize],
        pixels.rgba(),
    );
    let texture = ctx.load_texture(
        format!("fandhe-image-{source}"),
        image,
        egui::TextureOptions::LINEAR,
    );
    *cache = Some(Cached {
        serial,
        source: source.to_string(),
        texture: texture.clone(),
    });
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::RgbaPixels;

    fn img() -> RgbaPixels {
        RgbaPixels::new(2, 2, vec![255u8; 16]).unwrap()
    }

    /// static の 1 枠を共有するため、1 テストにまとめて並列実行の干渉を避ける。
    #[test]
    fn texture_only_for_matching_source_and_cleared_after_clear() {
        let ctx = egui::Context::default();
        clear();
        assert!(texture(&ctx, "photo:1").is_none());

        put("photo:1".into(), img());
        assert!(texture(&ctx, "photo:2").is_none(), "識別子が違えば描かない");
        assert!(texture(&ctx, "photo:1").is_some());
        assert!(texture(&ctx, "photo:1").is_some(), "2 回目も同じ画像");

        put("photo:2".into(), img());
        assert!(
            texture(&ctx, "photo:1").is_none(),
            "置き換えで前の識別子は無効"
        );
        assert!(texture(&ctx, "photo:2").is_some());

        clear();
        assert!(texture(&ctx, "photo:2").is_none());
    }
}
