//! サイズ計測プロトコル（設計文書 `docs/architecture/widget-architecture.md` §4.3 の
//! `measure` 契約を具体化したもの）。
//!
//! [`Measure`] は object-safe なトレイトとして定義しており、`ui::layout::LayoutTree` は
//! `Box<dyn Measure>` をリーフノードのコンテキストとして保持する。ウィジェット実装層
//! （`Widget<S>` トレイト、S2 / Issue #97）とはまだ未接続であり、
//! `Widget<S>::measure` はこのトレイトの実装として `LayoutTree` へ接続される想定
//! （out-of-scope-tracking.md 対象）。

use super::geometry::LayoutSize;

/// レイアウトエンジンがリーフノードへ与える利用可能スペース（1 軸分）。
///
/// taffy の `AvailableSpace` と同じ 3 状態を持つが、taffy の型を公開 API に
/// 露出させない方針（§4.3）に従い `fandhe-core` 所有の型として定義する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AvailableSpace {
    /// 指定ポイント数までの確定サイズが利用可能。
    Definite(f32),
    /// 利用可能サイズは不定で、内容の最小サイズ制約下でレイアウトする。
    MinContent,
    /// 利用可能サイズは不定で、内容の最大サイズ制約下でレイアウトする。
    MaxContent,
}

/// リーフノードの計測（[`Measure::measure`]）に渡す幅・高さ 2 軸分の制約。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutConstraints {
    /// 幅方向の利用可能スペース。
    pub width: AvailableSpace,
    /// 高さ方向の利用可能スペース。
    pub height: AvailableSpace,
}

/// リーフノード（テキスト・画像等、子を持たない末端ウィジェット）のサイズ計測契約。
///
/// `LayoutTree::new_leaf_with_measure` で登録したリーフノードについて、
/// `LayoutTree::compute_layout` の内部から呼び出される。呼び出し元はレイアウトエンジン
/// （taffy をラップした `LayoutTree`）であり、実装側（将来の `Widget<S>::measure`）は
/// 純粋関数として振る舞うことを期待される（副作用を持たせない）。
pub trait Measure: std::fmt::Debug {
    /// 与えられた制約のもとでのノードのサイズを計算する。
    fn measure(&self, constraints: LayoutConstraints) -> LayoutSize;
}
