//! `ImageWidget` の実画素を egui へ届ける 1 枠の保管所（Issue #1103、写真選択）。
//!
//! `fandhe-core` の `ImageWidget` は不透明なソース識別子しか持たない（実デコードを
//! しない契約）。effect ドライバ（`device/photo_pick.rs`）が復号確認済みの画素を
//! [`put`] で識別子付きで置き、`lib.rs` の `Node::Widget` 分岐が [`texture`] で識別子が
//! 一致するときだけテクスチャを引いて描く。識別子が一致しない画素は描かない
//! （古い結果が新しい表示へ混ざらない、`ImageWidget` のソース照合と同じ判断）。
//!
//! ponytail: 1 画面で 1 枚まで（`static` の 1 枠）。複数枚を同時に出す画面ができたら
//! 識別子をキーにしたマップへ上げる。

use std::sync::Mutex;

struct Entry {
    source: String,
    /// 未アップロードの画素。初回の [`texture`] で GPU へ上げて空にする。
    pending: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
}

static SLOT: Mutex<Option<Entry>> = Mutex::new(None);

/// 識別子つきで画素を置く。前の画像（画素・テクスチャ）は置き換えで解放される。
#[cfg_attr(
    not(any(test, all(feature = "device", not(target_arch = "wasm32")))),
    allow(dead_code)
)]
pub(crate) fn put(source: String, image: egui::ColorImage) {
    if let Ok(mut slot) = SLOT.lock() {
        *slot = Some(Entry {
            source,
            pending: Some(image),
            texture: None,
        });
    }
}

/// 保持中の画像を解放する。
#[cfg_attr(
    not(any(test, all(feature = "device", not(target_arch = "wasm32")))),
    allow(dead_code)
)]
pub(crate) fn clear() {
    if let Ok(mut slot) = SLOT.lock() {
        *slot = None;
    }
}

/// `source` と識別子が一致するときだけテクスチャを返す（初回に GPU へ上げる）。
/// ロックが poison していたら `None`（描かない）。
pub(crate) fn texture(ctx: &egui::Context, source: &str) -> Option<egui::TextureHandle> {
    let mut slot = SLOT.lock().ok()?;
    let entry = slot.as_mut().filter(|e| e.source == source)?;
    if entry.texture.is_none() {
        let image = entry.pending.take()?;
        entry.texture = Some(ctx.load_texture(
            format!("fandhe-image-{source}"),
            image,
            egui::TextureOptions::LINEAR,
        ));
    }
    entry.texture.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> egui::ColorImage {
        egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255; 16])
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
