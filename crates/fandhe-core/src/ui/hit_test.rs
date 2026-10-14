//! 座標 → 対象ウィジェット解決（ヒットテスト）基盤（TASK-8.3-S3 / REQ-8）。
//!
//! プラットフォーム統合レイヤ（ポインタ・タップ入力を受け取る側）が、レイアウト確定後の
//! 矩形ツリーと座標から「どのウィジェットがヒットしたか」を解決するための汎用 API。
//! 矩形の算出自体（レイアウトプロトコル、TASK-8.3-S4 / Issue #99）には踏み込まず、
//! 本モジュールは「レイアウト済み矩形ツリーが既に手元にある」前提の走査ロジックのみを担う。
//! [`super::pointer::PointerDispatcher`] が本モジュールの [`hit_test`] を呼び出し側として使う。

use super::geometry::{Point, Rect};

/// レイアウト確定後の 1 ノード分の矩形と、その子リージョン群。
///
/// `key` はウィジェットを一意に特定する識別子（例: `Node::Button::label` と対応させる
/// 呼び出し側の規約）。`key` を持たないリージョン（装飾目的のコンテナ等）は
/// ヒットテスト結果には含めないが、子への走査は継続する（透過的なコンテナとして扱う）。
#[derive(Debug, Clone, PartialEq)]
pub struct HitRegion {
    /// 本リージョンの矩形（親からの相対ではなく、既に画面座標系へ確定済みの矩形を想定）。
    pub bounds: Rect,
    /// ウィジェットキー。`None` はキーを持たない透過コンテナ。
    pub key: Option<String>,
    /// 子リージョン（宣言順。後述のとおり後の要素ほど前面）。
    pub children: Vec<HitRegion>,
}

impl HitRegion {
    /// 子を持たない葉リージョンを作る。
    pub fn leaf(bounds: Rect, key: impl Into<String>) -> Self {
        Self {
            bounds,
            key: Some(key.into()),
            children: Vec::new(),
        }
    }

    /// キーを持たない透過コンテナリージョンを作る。
    pub fn container(bounds: Rect, children: Vec<HitRegion>) -> Self {
        Self {
            bounds,
            key: None,
            children,
        }
    }
}

/// ヒットテストの 1 件分の結果。ヒットしたリージョンの矩形とキーを保持する。
#[derive(Debug, Clone, PartialEq)]
pub struct HitTarget {
    /// ヒットしたリージョンのウィジェットキー。
    pub key: String,
    /// ヒットしたリージョンの矩形。
    pub bounds: Rect,
}

/// `point` を含むリージョンを、最前面（子・宣言順で後）から祖先へ向かう順で返す。
///
/// 宣言的 UI の Z 順規約として「子は親より前面」「兄弟は宣言順で後が前面」を採用する
/// （バブリングイベント配線を想定した順序。最前面のキー付きターゲットが先頭に来る）。
/// `key` を持たないリージョンは結果に含めないが、子の走査は継続する（透過コンテナ）。
/// ヒットなしは空 `Vec` を返す（panic を API 契約にしない、coding-rust.md 準拠）。
pub fn hit_test(region: &HitRegion, point: Point) -> Vec<HitTarget> {
    let mut results = Vec::new();
    collect_hits(region, point, &mut results);
    results
}

/// `region` 以下を再帰走査し、ヒットしたキー付きリージョンを前面優先順で `results` に積む。
///
/// 子は宣言順で後の要素ほど前面のため、走査は逆順（末尾から）で行い、
/// 最前面の子から先に `results` へ積む。最後に自身（親）を積むことで
/// 「最前面の子 → … → 祖先」の順序（バブリング順序と一致）を実現する。
fn collect_hits(region: &HitRegion, point: Point, results: &mut Vec<HitTarget>) {
    if !region.bounds.contains(point) {
        return;
    }
    for child in region.children.iter().rev() {
        collect_hits(child, point, results);
    }
    if let Some(key) = &region.key {
        results.push(HitTarget {
            key: key.clone(),
            bounds: region.bounds,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::geometry::{Point, Rect, Size};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(Point::new(x, y), Size::new(w, h))
    }

    #[test]
    fn hit_test_returns_empty_when_no_hit() {
        let region = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "button");
        assert!(hit_test(&region, Point::new(100.0, 100.0)).is_empty());
    }

    #[test]
    fn hit_test_returns_single_leaf_hit() {
        let region = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "button");
        let hits = hit_test(&region, Point::new(5.0, 5.0));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "button");
    }

    #[test]
    fn hit_test_prefers_front_most_overlapping_sibling() {
        // 同一領域に重なる 2 兄弟。宣言順で後（front）が前面優先。
        let back = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "back");
        let front = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "front");
        let parent = HitRegion::container(rect(0.0, 0.0, 10.0, 10.0), vec![back, front]);

        let hits = hit_test(&parent, Point::new(5.0, 5.0));

        assert_eq!(hits[0].key, "front");
        assert_eq!(hits[1].key, "back");
    }

    #[test]
    fn hit_test_returns_child_then_ancestor_in_bubbling_order() {
        let child = HitRegion::leaf(rect(2.0, 2.0, 4.0, 4.0), "child");
        let parent = HitRegion {
            bounds: rect(0.0, 0.0, 10.0, 10.0),
            key: Some("parent".to_string()),
            children: vec![child],
        };

        let hits = hit_test(&parent, Point::new(3.0, 3.0));

        assert_eq!(
            hits.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(),
            vec!["child", "parent"]
        );
    }

    #[test]
    fn hit_test_skips_keyless_container_but_descends_into_children() {
        let child = HitRegion::leaf(rect(0.0, 0.0, 10.0, 10.0), "inner");
        let container = HitRegion::container(rect(0.0, 0.0, 10.0, 10.0), vec![child]);

        let hits = hit_test(&container, Point::new(5.0, 5.0));

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "inner");
    }
}
