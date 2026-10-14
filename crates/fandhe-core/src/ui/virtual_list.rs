//! 可視範囲算出（TASK-5.5-S2 / REQ-5、Issue #85）。
//!
//! 親 issue #83（リスト仮想化・再利用機構実装）の分解 sub-issue（2/5）。
//! S1（`docs/architecture/list-virtualization-architecture.md`、#84）が定めた
//! §3.2〜3.3 の契約（算出式・境界条件表・型シグネチャ）をそのまま実装する。
//!
//! [`compute_visible_range`] はスクロール位置・ビューポート寸法・項目数という
//! 数値のみを入力にとる純粋関数であり、状態 `S` にもプラットフォーム（egui 等の
//! 描画基盤）にも依存しない（S1 設計文書 §3.1 のレイヤ分界判断）。呼び出し文脈は
//! プラットフォームレイヤ（`fandhe-egui-proxy` では `egui::ScrollArea`）が
//! ビューポート実寸・現在のスクロールオフセットを取得し、本関数へ渡して
//! 描画すべき index 範囲を得る想定であり、`fandhe-egui-proxy::render_screen` への
//! 実配線は Issue #487（親 issue #83 の残課題）で完了した
//! （`crates/fandhe-egui-proxy/src/lib.rs` の `render_virtual_list` 参照）。
//!
//! **呼び出し側の前提（S5 / `scroll` モジュールとの契約）**: `scroll_offset` は
//! 呼び出し側が [`crate::clamp_scroll_offset`] で事前にクランプ済みであることを
//! 推奨する。本関数自身も末尾超過・負値・非有限値を安全側の範囲へ丸めるため panic
//! はしないが、`clamp_scroll_offset` を経由しない生の `ScrollArea` オフセットを
//! 直接渡すと「末尾超過時に空範囲を返す」（クランプ済みなら最終ページを返す設計と
//! 異なる）挙動になり得る。`render_virtual_list`（Issue #487）は両関数を
//! この順序（`clamp_scroll_offset` → `compute_visible_range`）で組み合わせて呼ぶ。
//!
//! 実装は S4（性能確認テスト、#87）が契約固定のために自己完結実装していた
//! テスト内ヘルパー（`tests/virtual_list_perf.rs`）をそのまま移植したものであり、
//! 同テストは本モジュールの呼び出しへ置き換え済み（S2 マージ後の置き換え、
//! 同ファイルの関数コメント参照）。
//!
//! [`grid_item_range`] は Grid 軸（`Axis::Grid { columns }`）専用の付随変換で、
//! [`compute_visible_range`] が返す「可視行範囲」を「可視項目 index 範囲」へ写す。
//! TV 側・非 TV 側の Grid 描画経路が二重実装していたものを Issue #557 で本モジュールへ
//! 集約した（`fandhe-egui-proxy` の `render_virtual_list_grid` / `tv_focus` 参照）。

/// 可視範囲を表す半開区間。`std::ops::Range<usize>` と同じ意味論を持つが、
/// ウィンドウイング専用の名前を与えて呼び出し側の意図を明確にする
/// （S1 設計文書 §3.3）。
pub type VisibleRange = std::ops::Range<usize>;

/// スクロール位置・ビューポート寸法・項目数から、描画すべき項目の index 範囲を
/// 算出する純粋関数。副作用を持たず、状態 `S` にもプラットフォームにも依存しない。
///
/// 算出式（S1 設計文書 §3.2）:
///
/// ```text
/// first = floor(scroll_offset / item_extent)
/// visible_count = ceil(viewport_extent / item_extent) + 1
/// ```
///
/// overscan（前後バッファ `overscan` 項目）を加えた
/// `[first.saturating_sub(overscan), first + visible_count + overscan)` を
/// 項目総数 `len` でクランプした範囲を最終的な可視範囲として返す。
///
/// `item_extent <= 0.0` や `len == 0` はゼロ除算・空範囲として安全側に扱い、
/// panic しない（境界条件表は S1 設計文書 §3.2、`coding-rust.md` の
/// エラーハンドリング方針を参照）。負値・`NaN`・無限大の入力も同様に安全側の
/// 値へ丸め、`f32 → usize` 変換は `saturating_*` 系の演算で行う。
///
/// # Examples
///
/// ```
/// use fandhe_core::ui::compute_visible_range;
///
/// // viewport 600px・item_extent 40px・overscan 3、先頭付近（offset 0）。
/// let range = compute_visible_range(600.0, 0.0, 40.0, 300, 3);
/// assert_eq!(range.start, 0);
/// assert!(range.end <= 300);
/// ```
pub fn compute_visible_range(
    viewport_extent: f32,
    scroll_offset: f32,
    item_extent: f32,
    len: usize,
    overscan: usize,
) -> VisibleRange {
    // `item_extent` が非有限（`NaN` / 無限大）または `0.0` 以下の場合はゼロ除算・
    // 不定値の伝播を避け空範囲を返す（`scroll.rs::total_content_extent` と同じ
    // 「非有限は無効値」という規約に揃える。`NaN <= 0.0` は false のため、
    // `is_finite()` チェックを併用しないと NaN が素通りしてしまう点に注意）。
    if len == 0 || !item_extent.is_finite() || item_extent <= 0.0 {
        return 0..0;
    }

    let offset = scroll_offset.max(0.0);
    let first_visible = (offset / item_extent).floor();
    // 負値・NaN 由来の異常値を弾き、`usize` へ安全に変換する。
    let first_visible = if first_visible.is_finite() && first_visible >= 0.0 {
        first_visible as usize
    } else {
        0
    };

    let visible_count = (viewport_extent / item_extent).ceil() + 1.0;
    let visible_count = if visible_count.is_finite() && visible_count >= 0.0 {
        visible_count as usize
    } else {
        len
    };

    let start = first_visible.saturating_sub(overscan);
    let end = first_visible
        .saturating_add(visible_count)
        .saturating_add(overscan)
        .min(len);
    let start = start.min(end);

    start..end
}

/// Grid 軸（`Axis::Grid { columns }`）における「可視行範囲」を「可視項目 index 範囲」へ
/// 変換する純関数。呼び出し側は [`compute_visible_range`] の戻り値（行単位で
/// 算出した `row_range`）をそのまま `row_range` 引数へ渡す想定であり、両関数を
/// この順序（`compute_visible_range` → `grid_item_range`）で組み合わせて使う契約は
/// モジュール冒頭 doc の呼び出し順契約と同じ位置づけである。
///
/// `row_range.start`/`end` はそれぞれ `columns` 倍することで項目 index に写るが、
/// 行数・列数がともに巨大な場合（`VirtualList` は巨大な論理件数を遅延処理する API で
/// あり、境界条件表・panic を契約にしない方針、`.claude/rules/coding-rust.md` の
/// エラーハンドリング規約）、素の乗算は `usize` を overflow しうる。乗算は
/// `saturating_mul` で飽和させ、さらに項目総数 `total_len` へクランプする
/// （末尾行の端数を取りこぼさないためのクランプでもある）。
///
/// `columns = 0` は panic せず空範囲（`0..0` 相当、`total_len` へのクランプにより
/// 常に `start == end == 0`）を返す。ただし通常の呼び出し側は
/// `rows = total_len.div_ceil(columns)` の算出にも `columns.max(1)` のガードを
/// 必要とするため、このガードは呼び出し側に残す設計であり、本関数のガードは
/// あくまでガードを経由しない直接呼び出しに対する安全網である。
///
/// # Examples
///
/// ```
/// use fandhe_core::ui::grid_item_range;
///
/// // 3 列グリッドで可視行 2..5 → 項目 index 6..15。
/// let range = grid_item_range(&(2..5), 3, 100);
/// assert_eq!(range, 6..15);
/// ```
pub fn grid_item_range(row_range: &VisibleRange, columns: usize, total_len: usize) -> VisibleRange {
    row_range.start.saturating_mul(columns).min(total_len)
        ..row_range.end.saturating_mul(columns).min(total_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 境界条件表（S1 設計文書 §3.2）: 空リストは空範囲を返す。
    #[test]
    fn empty_list_returns_empty_range() {
        assert_eq!(compute_visible_range(600.0, 0.0, 40.0, 0, 3), 0..0);
    }

    /// 境界条件表: 先頭（`scroll_offset = 0`）は `first = 0` から開始する範囲になる。
    #[test]
    fn head_offset_starts_from_zero_before_overscan_clamp() {
        let range = compute_visible_range(600.0, 0.0, 40.0, 300, 3);
        assert_eq!(
            range.start, 0,
            "overscan による saturating_sub でも 0 未満にならない"
        );
    }

    /// 境界条件表: 末尾超過（`scroll_offset` がコンテンツ全高を超える）は終端が `len` で
    /// クランプされる。
    #[test]
    fn tail_overflow_clamps_end_to_len() {
        let range = compute_visible_range(600.0, 100_000.0, 40.0, 300, 3);
        assert!(range.end <= 300, "終端は総項目数 300 を超えてはならない");
        assert_eq!(range.end, 300);
    }

    /// 境界条件表: 負のオフセット（浮動小数点の丸め等に由来）は `first` を `0` に
    /// クランプする。
    #[test]
    fn negative_offset_clamps_first_to_zero() {
        let range = compute_visible_range(600.0, -50.0, 40.0, 300, 3);
        assert_eq!(range.start, 0);
    }

    /// 境界条件表: viewport がコンテンツ全高より大きい場合、可視範囲は `0..len` 全体になる。
    #[test]
    fn viewport_larger_than_content_covers_whole_range() {
        let range = compute_visible_range(100_000.0, 0.0, 40.0, 10, 3);
        assert_eq!(range, 0..10);
    }

    /// 境界条件表: `item_extent = 0` はゼロ除算を回避し、panic せず空範囲を返す。
    #[test]
    fn zero_item_extent_avoids_division_by_zero() {
        let range = compute_visible_range(600.0, 0.0, 0.0, 300, 3);
        assert_eq!(range, 0..0);
    }

    /// 境界条件表: overscan がリスト端を超えても、開始・終了とも `0..len` の範囲内に
    /// クランプされる。
    #[test]
    fn overscan_exceeding_list_bounds_is_clamped_within_len() {
        let range = compute_visible_range(600.0, 0.0, 40.0, 5, 100);
        assert_eq!(range, 0..5);
    }

    /// NaN・無限大の入力でも panic せず有限な範囲を返すことを確認する
    /// （境界条件表には明示されていないが、`total_content_extent` /
    /// `clamp_scroll_offset`（scroll.rs）と同水準の防御を課す）。
    #[test]
    fn non_finite_inputs_never_panic() {
        let range = compute_visible_range(f32::NAN, f32::NAN, 40.0, 300, 3);
        assert!(range.start <= range.end && range.end <= 300);

        let range = compute_visible_range(600.0, f32::INFINITY, 40.0, 300, 3);
        assert!(range.start <= range.end && range.end <= 300);

        let range = compute_visible_range(f32::INFINITY, 0.0, 40.0, 300, 3);
        assert_eq!(range, 0..300);
    }

    /// `item_extent` が `NaN` の場合も安全側の空範囲を返すことを確認する
    /// （`NaN <= 0.0` は false のため `is_finite()` チェックが必須。
    /// `scroll.rs::total_content_extent_is_zero_when_item_extent_is_nan` と
    /// 同じ「非有限な `item_extent` は無効値」という規約に揃える回帰防止）。
    #[test]
    fn nan_item_extent_returns_empty_range() {
        let range = compute_visible_range(600.0, 0.0, f32::NAN, 300, 3);
        assert_eq!(range, 0..0);
    }

    /// 通常のスクロール位置でウィンドウ幅が想定どおりであることを確認する
    /// （`tests/virtual_list_perf.rs` の期待値と同じ入力の回帰確認）。
    #[test]
    fn typical_scroll_position_yields_expected_window() {
        // visible_count = ceil(600/40)+1 = 16、first = 0 → start=0(saturating_sub(3))、
        // end = min(0+16+3, 300) = 19。
        let range = compute_visible_range(600.0, 0.0, 40.0, 300, 3);
        assert_eq!(range, 0..19);
    }

    /// `grid_item_range` 境界条件表: 通常ケースは `row_range * columns` の単純写像になる。
    #[test]
    fn grid_item_range_typical_case_multiplies_by_columns() {
        assert_eq!(grid_item_range(&(2..5), 3, 100), 6..15);
    }

    /// 境界条件表: 末尾の端数行（`rows * columns` が `total_len` を超える場合）は
    /// `total_len` へクランプされる。
    #[test]
    fn grid_item_range_tail_row_remainder_clamps_to_total_len() {
        // columns=3・total_len=11（最終行は 2 項目のみ）。row_range.end=4 は
        // 素の乗算では 12 になるが、11 へクランプされる。
        assert_eq!(grid_item_range(&(0..4), 3, 11), 0..11);
    }

    /// 境界条件表: `columns` が巨大でも `saturating_mul` により `usize` を overflow
    /// せず、結果は `total_len` へ飽和する。
    #[test]
    fn grid_item_range_huge_columns_saturates_without_overflow() {
        assert_eq!(grid_item_range(&(1..2), usize::MAX, 100), 100..100);
        assert_eq!(grid_item_range(&(0..usize::MAX), usize::MAX, 42), 0..42);
    }

    /// 境界条件表: `total_len` 自体が `usize::MAX` の場合、飽和値がクランプで
    /// 切り詰められず `usize::MAX` のまま返る（`.min(total_len)` が実質無効となる
    /// 経路の回帰防止。移設前の `fandhe-egui-proxy` 側テストから引き継いだケース）。
    #[test]
    fn grid_item_range_saturated_value_survives_max_total_len() {
        assert_eq!(
            grid_item_range(&(0..usize::MAX), 2, usize::MAX),
            0..usize::MAX
        );
    }

    /// 境界条件表: 空の行範囲（`0..0`）は空の項目範囲を返す。
    #[test]
    fn grid_item_range_empty_row_range_returns_empty_range() {
        assert_eq!(grid_item_range(&(0..0), 3, 100), 0..0);
    }

    /// 境界条件表: `columns = 0`（ガードを経由しない直接呼び出し）は panic せず
    /// 空範囲を返す。
    #[test]
    fn grid_item_range_zero_columns_returns_empty_range_without_panic() {
        assert_eq!(grid_item_range(&(2..5), 0, 100), 0..0);
    }
}
