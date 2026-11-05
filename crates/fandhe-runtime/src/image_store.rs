//! `ImageWidget` の実画素を描画側へ届ける 1 枠の保管所（レンダラ非依存）。
//!
//! `fandhe-core` の `ImageWidget` は不透明なソース識別子しか持たない。端末機能の effect ドライバ
//! （写真選択・書類スキャン等）が、復号確認済みの画素を識別子付きで [`put`] し、レンダラ実装
//! （`fandhe-render-egui` の `image_store::texture` 等）が [`get`] で識別子が一致するときだけ引いて描く。
//! 識別子が一致しない画素は描かせない（古い結果が新しい表示へ混ざらない）。
//!
//! 画素は `fandhe_core::RgbaPixels`（検証済み・`Arc` 保持）で持つ。GPU へ上げる処理は
//! レンダラ側の責務で、本モジュールは egui を参照しない。
//!
//! ponytail: 1 画面で 1 枚まで（`static` の 1 枠）。CPU 側の画素は枠が持ち続ける（最大で約 16 MiB。
//! 写真選択の表示用複製は辺 1024 以下で約 4 MiB）。複数枚を同時に出す画面ができたら
//! 識別子をキーにしたマップへ上げる。

use std::sync::Mutex;

use fandhe_core::RgbaPixels;

struct Entry {
    /// [`put`] のたびに増える。レンダラが「同じ画素か」を識別子の再利用と区別して判定するために使う。
    serial: u64,
    source: String,
    pixels: RgbaPixels,
}

static SLOT: Mutex<(u64, Option<Entry>)> = Mutex::new((0, None));

fn lock() -> std::sync::MutexGuard<'static, (u64, Option<Entry>)> {
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

/// 識別子つきで画素を置く。前の画像は置き換えで解放される。
pub fn put(source: String, pixels: RgbaPixels) {
    let mut g = lock();
    g.0 += 1;
    let serial = g.0;
    g.1 = Some(Entry {
        serial,
        source,
        pixels,
    });
}

/// [`clear`] のたびに呼ばれる、レンダラ側の GPU 資源を捨てるフック（レンダラが 1 つ登録する）。
static ON_CLEAR: Mutex<Option<fn()>> = Mutex::new(None);

/// [`clear`] で呼ぶフックを登録する。レンダラが GPU へ上げた分を、画像ウィジェットが
/// 描かれなくなっても確実に解放するために使う。
pub fn set_on_clear(hook: fn()) {
    *ON_CLEAR.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}

/// 保持中の画像を解放する。登録済みのフックでレンダラ側のテクスチャも捨てさせる。
pub fn clear() {
    lock().1 = None;
    // 保管所のロックを手放してから呼ぶ（フックが `get` を呼んでも詰まらない）。
    let hook = *ON_CLEAR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = hook {
        h();
    }
}

/// `source` と識別子が一致するときだけ `(置いた通し番号, 画素)` を返す。
pub fn get(source: &str) -> Option<(u64, RgbaPixels)> {
    lock()
        .1
        .as_ref()
        .filter(|e| e.source == source)
        .map(|e| (e.serial, e.pixels.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px() -> RgbaPixels {
        RgbaPixels::new(2, 2, vec![255u8; 16]).unwrap()
    }

    /// static の 1 枠を共有するため、1 テストにまとめて並列実行の干渉を避ける。
    #[test]
    fn get_only_for_matching_source_and_serial_changes_on_put() {
        clear();
        assert!(get("a").is_none());

        put("a".into(), px());
        assert!(get("b").is_none(), "識別子が違えば返さない");
        let (s1, p) = get("a").unwrap();
        assert_eq!((p.width(), p.height()), (2, 2));

        put("a".into(), px());
        let (s2, _) = get("a").unwrap();
        assert_ne!(s1, s2, "同じ識別子でも置き直せば通し番号が変わる");

        put("b".into(), px());
        assert!(get("a").is_none(), "置き換えで前の識別子は無効");
        clear();
        assert!(get("b").is_none());
    }
}
