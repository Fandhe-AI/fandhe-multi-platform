//! リストのスクロール幾何ヘルパー（TASK-5.5-S5 / REQ-5、Issue #88）。
//!
//! 親 issue #83（リスト仮想化・再利用機構実装）の分解 sub-issue（5/5）。
//! S1（可視範囲算出・ウィンドウイング設計、#84）が定めた設計文書
//! （`docs/architecture/list-virtualization-architecture.md` §3.5）は、S5 の責務を
//! 「総コンテンツ高さ `len * item_extent` を `egui::ScrollArea` へ明示し、可視範囲のみ
//! 描画してもスクロールバーの長さ・位置が実項目数と整合すること」と定めている。
//!
//! `recycle.rs`（S3・#86）と同じ判断（計画書 2 章のフォールバック）に従い、
//! `Node::VirtualList` / `compute_visible_range`（S2・#85）の型・API に
//! 一切依存しない自己完結の純粋関数として実装している。
//!
//! 呼び出し文脈: プラットフォームレイヤ（`fandhe-egui-proxy`）が毎フレーム
//! [`total_content_extent`]（`ScrollArea` へ渡す総高さ）と
//! [`clamp_scroll_offset`]（`ScrollArea` から得たオフセットの防御的クランプ）を呼ぶ。
//! `render_screen` 側の `egui::ScrollArea::show_viewport` 統合は Issue #487（親 issue #83
//! の残課題）で完了した（`crates/fandhe-egui-proxy/src/lib.rs` の `render_virtual_list`
//! 参照。両関数を `clamp_scroll_offset` → `compute_visible_range` の順で呼ぶ契約は
//! `virtual_list.rs` モジュール doc も参照）。
//!
//! いずれの関数も `#![forbid(unsafe_code)]`（fandhe-core 既定方針）のもとで実装し、
//! 非有限値（`NaN` / 無限大）・負値・オーバーフローを安全側の値へ丸めることで
//! panic を排除する（panic を API 契約にしない、coding-rust.md のエラーハンドリング方針）。

/// 項目数 `len` と 1 項目あたりの高さ `item_extent` から、リストの総コンテンツ高さを
/// 算出する。
///
/// `egui::ScrollArea` へこの値を明示することで、可視範囲のみを描画しても
/// スクロールバーの長さ・位置が実項目数（300 項目規模を含む）と整合する
/// （設計文書 §3.5、REQ-5 受け入れ基準）。
///
/// # 安全側の丸め
///
/// - `len == 0` は総高さ `0.0` を返す（空リスト）。
/// - `item_extent` が非有限（`NaN` / 無限大）または `0.0` 以下の場合、
///   スクロールバーを不正な縮尺で表示しないよう総高さ `0.0` を返す
///   （S1 設計文書 §3.2 の境界条件表「`item_extent = 0`」を踏襲）。
/// - `len * item_extent` が `f32` の範囲を超えて無限大になり得る場合（項目数・
///   項目高さが極端に大きい入力）、`f32::MAX` へ飽和させ、`NaN`/無限大を
///   `egui::ScrollArea` へそのまま渡して未定義の見た目になることを避ける。
pub fn total_content_extent(len: usize, item_extent: f32) -> f32 {
    if len == 0 || !item_extent.is_finite() || item_extent <= 0.0 {
        return 0.0;
    }

    // usize → f32 の変換は 2^24 を超える項目数で丸めが生じ得るが、300 項目規模の
    // 対象範囲では実害がなく、`as` によるサチュレーティングキャストで panic しない
    // （Rust 1.45 以降の `as` 変換セマンティクスに従う）。
    let len_as_extent = len as f32;
    let total = len_as_extent * item_extent;

    if total.is_finite() { total } else { f32::MAX }
}

/// スクロールオフセット `offset` を、総コンテンツ高さ `total_extent` と
/// ビューポート実寸 `viewport_extent` から求まる有効範囲 `[0, max_offset]` へ
/// クランプする。
///
/// `max_offset` は `(total_extent - viewport_extent)` を `0.0` 未満にならないよう
/// クランプした値。ビューポートが総コンテンツ高さ以上の場合（`viewport_extent >=
/// total_extent`）は `max_offset = 0.0` となり、常に `0.0` を返す
/// （設計文書 §3.2 境界条件表「viewport がコンテンツ全高より大きい」に対応）。
///
/// # 安全側の丸め
///
/// - `offset` が非有限（`NaN` / 無限大）の場合は `0.0` を返す（先頭へフォールバック）。
/// - `total_extent` / `viewport_extent` が非有限または負値の場合、`0.0` として扱う
///   （不正な入力に対しても `NaN` を伝播させない）。
/// - 負のオフセット（浮動小数点の丸め等に由来）は `0.0` にクランプする。
/// - 末尾超過のオフセットは `max_offset` にクランプする。
pub fn clamp_scroll_offset(offset: f32, total_extent: f32, viewport_extent: f32) -> f32 {
    let total_extent = normalize_non_negative_finite(total_extent);
    let viewport_extent = normalize_non_negative_finite(viewport_extent);
    let max_offset = (total_extent - viewport_extent).max(0.0);

    if !offset.is_finite() {
        return 0.0;
    }

    // `max_offset` はここまでで必ず有限・非負であるため、`f32::clamp` が
    // 要求する `min <= max` の前提（`0.0 <= max_offset`）を満たし panic しない。
    offset.clamp(0.0, max_offset)
}

/// 非有限（`NaN` / 無限大）または負の値を `0.0` へ丸める。
/// [`clamp_scroll_offset`] が `total_extent` / `viewport_extent` の両入力に対して
/// 同一の防御ルールを適用するための共通ヘルパー。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- total_content_extent ---

    #[test]
    fn total_content_extent_returns_zero_for_empty_list() {
        assert_eq!(total_content_extent(0, 24.0), 0.0);
    }

    #[test]
    fn total_content_extent_multiplies_len_by_item_extent() {
        // 300 項目規模（REQ-5 の性能確認基準）でも単純な乗算で求まることを確認する。
        assert_eq!(total_content_extent(300, 24.0), 300.0 * 24.0);
    }

    #[test]
    fn total_content_extent_is_zero_when_item_extent_is_zero() {
        assert_eq!(total_content_extent(300, 0.0), 0.0);
    }

    #[test]
    fn total_content_extent_is_zero_when_item_extent_is_negative() {
        assert_eq!(total_content_extent(300, -1.0), 0.0);
    }

    #[test]
    fn total_content_extent_is_zero_when_item_extent_is_nan() {
        let result = total_content_extent(300, f32::NAN);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn total_content_extent_saturates_instead_of_producing_infinity() {
        // len・item_extent がいずれも極端に大きく、乗算結果が f32 の範囲を超える場合。
        let result = total_content_extent(usize::MAX, f32::MAX);
        assert!(result.is_finite(), "無限大を返さず飽和するはず");
        assert_eq!(result, f32::MAX);
    }

    // --- clamp_scroll_offset ---

    #[test]
    fn clamp_scroll_offset_clamps_negative_offset_to_zero() {
        assert_eq!(clamp_scroll_offset(-10.0, 1000.0, 200.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_passes_through_offset_within_range() {
        assert_eq!(clamp_scroll_offset(300.0, 1000.0, 200.0), 300.0);
    }

    #[test]
    fn clamp_scroll_offset_clamps_offset_past_end_to_max_offset() {
        // 総高さ 1000、ビューポート 200 の場合、末尾は 800。
        assert_eq!(clamp_scroll_offset(5000.0, 1000.0, 200.0), 800.0);
    }

    #[test]
    fn clamp_scroll_offset_is_zero_when_viewport_exceeds_total_extent() {
        // viewport がコンテンツ全高より大きい場合（S1 設計文書 §3.2 の境界条件）。
        assert_eq!(clamp_scroll_offset(50.0, 100.0, 500.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_is_zero_for_empty_list() {
        // total_extent = 0（空リスト）の場合、オフセットは常に 0 に落ちる。
        assert_eq!(clamp_scroll_offset(10.0, 0.0, 200.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_returns_zero_for_nan_offset() {
        let result = clamp_scroll_offset(f32::NAN, 1000.0, 200.0);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn clamp_scroll_offset_returns_zero_for_infinite_offset() {
        // 非有限（無限大）のオフセットはいずれの符号でも安全側の 0.0 に丸める
        // （`is_finite()` チェックが正負両方の無限大を弾く契約の確認）。
        assert_eq!(clamp_scroll_offset(f32::INFINITY, 1000.0, 200.0), 0.0);
        assert_eq!(clamp_scroll_offset(f32::NEG_INFINITY, 1000.0, 200.0), 0.0);
    }

    #[test]
    fn clamp_scroll_offset_treats_non_finite_total_extent_as_zero() {
        let result = clamp_scroll_offset(10.0, f32::NAN, 200.0);
        assert!(result.is_finite());
        assert_eq!(result, 0.0);
    }

    #[test]
    fn clamp_scroll_offset_treats_negative_viewport_extent_as_zero() {
        // ビューポート実寸が負値（呼び出し側の不正入力）でも panic せず、
        // total_extent 全体を上限として扱う。
        let result = clamp_scroll_offset(10.0, 100.0, -50.0);
        assert!(result.is_finite());
        assert_eq!(result, 10.0);
    }

    #[test]
    fn clamp_scroll_offset_never_panics_across_boundary_condition_table() {
        // S1 設計文書 §3.2 の境界条件表を横断的に総当たりし、いずれも panic せず
        // 有限値を返すことを検証する（`f32::clamp` の `min <= max` 前提が崩れないこと）。
        let offsets = [
            -100.0,
            0.0,
            50.0,
            10_000.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        let totals = [0.0, 100.0, f32::NAN, -10.0, f32::INFINITY];
        let viewports = [0.0, 50.0, 500.0, f32::NAN, -5.0];

        for &offset in &offsets {
            for &total in &totals {
                for &viewport in &viewports {
                    let result = clamp_scroll_offset(offset, total, viewport);
                    assert!(
                        result.is_finite(),
                        "offset={offset}, total={total}, viewport={viewport} で非有限値が返った"
                    );
                    assert!(result >= 0.0, "クランプ結果が負値になった");
                }
            }
        }
    }
}
