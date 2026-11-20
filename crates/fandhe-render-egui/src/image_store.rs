//! `ImageWidget` の実画素を egui のテクスチャとして描く側の窓口（Issue #1103、写真選択ほか）。
//!
//! 画素の保管所そのものはレンダラ非依存の `fandhe_runtime::image_store` にあり、effect ドライバ
//! （機能 crate の写真選択・書類スキャン等。置き場は `fandhe_runtime::image_store`）が [`put`] / [`clear`] で置く。
//! 本モジュールは `lib.rs` の `Node::Widget` 分岐が [`texture`] で識別子が一致するときだけ
//! テクスチャを引くための、GPU へ上げる処理（通し番号が変わったときだけ再アップロード）を持つ。
//! 識別子が一致しない画素は描かない（古い結果が新しい表示へ混ざらない）。

use std::collections::HashMap;
use std::sync::Mutex;

pub use fandhe_runtime::image_store::{begin_frame, clear, put, remove, remove_prefix, PutResult};

/// GPU へ上げ済みのテクスチャと、その元になった保管所の通し番号。
struct Cached {
    serial: u64,
    texture: egui::TextureHandle,
}

/// 識別子ごとのテクスチャ。保管所から捨てられた識別子は [`drop_cached`]（フック）が同時に捨てるため、
/// 保管所の容量上限がそのまま GPU 側の上限にもなる。
static CACHE: Mutex<Option<HashMap<String, Cached>>> = Mutex::new(None);

/// 保管所が画像を捨てるたびに呼ばれ、その識別子の GPU 側テクスチャを捨てる（画像が描かれなくなっても解放される）。
fn drop_cached(source: &str) {
    if let Some(map) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(source);
    }
}

/// `source` と識別子が一致するときだけテクスチャを返す（通し番号が変わったら GPU へ上げ直す）。
/// 保管所に無いときは `None` を返し、その識別子の古いテクスチャ登録があれば捨てる。
pub(crate) fn texture(ctx: &egui::Context, source: &str) -> Option<egui::TextureHandle> {
    fandhe_runtime::image_store::set_on_remove(drop_cached);
    texture_with(ctx, source, fandhe_runtime::image_store::get)
}

/// [`texture`] の本体。`get` を差し替えられる（保管所から消えた後の登録の掃除をテストするため）。
///
/// **ロックの順は `CACHE` → 保管所**。`CACHE` を持ったまま `get` を呼ぶので、`get` と登録の間に別スレッドの
/// `remove` が入っても、保管所に無い識別子のテクスチャが残らない（`remove` のフックは保管所のロックを手放してから
/// `drop_cached` を呼び、`CACHE` が空くのを待つ）。保管所のロックを持ったまま `CACHE` を取る経路は無い。
fn texture_with(
    ctx: &egui::Context,
    source: &str,
    get: impl FnOnce(&str) -> Option<(u64, fandhe_core::RgbaPixels)>,
) -> Option<egui::TextureHandle> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    let Some((serial, pixels)) = get(source) else {
        map.remove(source);
        return None;
    };
    if let Some(c) = map.get(source).filter(|c| c.serial == serial) {
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
    map.insert(
        source.to_string(),
        Cached {
            serial,
            texture: texture.clone(),
        },
    );
    Some(texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::RgbaPixels;

    fn img() -> RgbaPixels {
        RgbaPixels::new(2, 2, vec![255u8; 16]).unwrap()
    }

    fn cached() -> usize {
        CACHE.lock().unwrap().as_ref().map_or(0, |m| m.len())
    }

    /// static を共有するため、1 テストにまとめて並列実行の干渉を避ける。
    #[test]
    fn textures_per_source_and_dropped_together_with_the_store() {
        let ctx = egui::Context::default();
        clear();
        assert!(texture(&ctx, "user:1").is_none());

        let _ = put("user:1".into(), img());
        let _ = put("user:2".into(), img());
        assert!(texture(&ctx, "user:3").is_none(), "識別子が違えば描かない");
        let t1 = texture(&ctx, "user:1").unwrap();
        let t2 = texture(&ctx, "user:2").unwrap();
        assert_ne!(t1.id(), t2.id(), "識別子ごとに別のテクスチャ");
        assert_eq!(
            texture(&ctx, "user:1").unwrap().id(),
            t1.id(),
            "2 回目は上げ直さない"
        );
        assert_eq!(cached(), 2);

        let _ = put("user:1".into(), img());
        assert_ne!(
            texture(&ctx, "user:1").unwrap().id(),
            t1.id(),
            "置き直しは上げ直す"
        );

        assert!(remove("user:2"));
        assert!(texture(&ctx, "user:2").is_none());
        assert_eq!(cached(), 1, "保管所から捨てた識別子のテクスチャも捨てる");

        clear();
        assert!(texture(&ctx, "user:1").is_none());
        assert_eq!(cached(), 0, "clear で GPU 側も捨てる");

        // 保管所に無くなった識別子の登録は、`get` が `None` を返した時点で捨てる（競合で取りこぼしたフックの保険）。
        let _ = put("race:1".into(), img());
        assert!(texture(&ctx, "race:1").is_some());
        assert_eq!(cached(), 1);
        assert!(texture_with(&ctx, "race:1", |_| None).is_none());
        assert_eq!(cached(), 0, "get が None なら登録も消える");

        // 容量超過の追い出しでもフックが呼ばれ、GPU 側のテクスチャが減る。
        let _ = put("evict:0".into(), img());
        assert!(texture(&ctx, "evict:0").is_some());
        assert_eq!(cached(), 1);
        let big = |c: u8| RgbaPixels::new(2048, 2048, vec![c; 2048 * 2048 * 4]).unwrap();
        for k in 0..5 {
            let _ = put(format!("evict:big:{k}"), big(k));
        }
        assert!(
            fandhe_runtime::image_store::get("evict:0").is_none(),
            "最古は追い出される"
        );
        assert_eq!(cached(), 0, "追い出しでフックが呼ばれ、テクスチャも捨てる");
        clear();
    }
}
