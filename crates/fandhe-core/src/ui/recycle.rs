//! リスト項目再利用機構（TASK-5.5-S3 / REQ-5、Issue #86）。
//!
//! 可視範囲算出（`virtual_list`、#84・#85）の型・API には依存しない自己完結モジュール
//! として実装する。可視ウィンドウは `std::ops::Range<usize>` を引数で受け取る
//! 汎用 API とし、呼び出し側（`fandhe-egui-proxy`）からの配線で統合する。
//!
//! `Node::List::expand`（[`crate::ui::Node::expand_children`]）は走査のたびに
//! 全項目分の `Vec<Node<S>>` を再構築する都度評価設計であり、300 項目規模の
//! リストではフレームごとに全項目分のノード生成（`String` アロケーション含む）が
//! 発生し、スクロール時の性能劣化要因となる（REQ-5 受け入れ基準「immediate mode に
//! 起因する性能劣化がないこと」）。本モジュールが提供する [`ListRecycler`] は、
//! 可視ウィンドウ内の各項目インデックスに対応する `Node<S>` をスロットに保持し、
//! フレーム間・スクロール間で構築済みノードを再利用することで、新規構築を
//! 「新たに可視域へ入った項目」または「状態変化時」のみに限定する。
//!
//! 呼び出し文脈: プラットフォームレイヤ／仮想化リストコア（S2 マージ後）が
//! 毎フレーム [`ListRecycler::acquire`] を呼び出し、可視ウィンドウと
//! [`crate::state::StateStore::version`] を渡す想定。本クレートはプラットフォーム
//! 固有クレートへ依存しないため（coding-rust.md）、実際の呼び出し配線は
//! プラットフォームレイヤ側の責務とする。
//!
//! Issue #487（親 issue #83 の残課題）で `fandhe-egui-proxy::render_screen` への
//! `egui::ScrollArea::show_viewport` + `compute_visible_range` 配線は完了し、
//! Issue #490 で本型（フレーム間のノード再利用キャッシュ）の呼び出し配線も完了した。
//! `ListRecycler<S>` はフレームをまたいで `Node<S>` を保持する必要がある一方、
//! `render_screen<S>` は `S: 'static` を要求しないため egui 側の永続領域
//! （`Ui::memory` 等、`'static` 前提）へそのまま保持できない（`Widget<S>` への
//! `Send + Sync` 追加という、より大きな破壊的変更を伴う）。そのため
//! `fandhe-egui-proxy::VirtualListRecyclers<S>` が呼び出し側（`ProxyApp` /
//! `PrototypeApp` 等のアプリ構造体）のフィールドとしてフレームをまたいで保持し、
//! `render_screen_with_recyclers` へ明示的に渡す設計を採る
//! （`crates/fandhe-egui-proxy/src/recyclers.rs` モジュール doc 参照）。

use std::collections::BTreeMap;
use std::ops::Range;

use crate::ui::Node;

/// 可視ウィンドウ内の項目ノードをフレーム間で再利用するプール（TASK-5.5-S3 / REQ-5）。
///
/// - **再利用条件**: 直前の呼び出しと状態バージョンが同一の項目は、キャッシュ済みの
///   `Node<S>` をそのまま返し、`expand_item` を呼ばない。
/// - **無効化条件**: 状態バージョン（[`crate::state::StateStore::version`]）が進んだら
///   全キャッシュを無効化して再構築する（正しさ優先の安全側設計。項目単位の差分
///   無効化は S4（#87）の計測結果を見て後続改善として検討する）。
/// - **有界性**: キャッシュスロット数は可視ウィンドウ幅に有界であり、ウィンドウ外へ
///   出た項目のスロットは解放する（項目総数に比例したメモリ増加を防ぎ、A05/資源枯渇の
///   観点でも安全）。
/// - **計測**: [`ListRecycler::built_count`] / [`ListRecycler::reused_count`] により、
///   再構築が新規可視分のみに限定されていることをテスト・S4 の性能確認テストから
///   検証できる。
///
/// `#![forbid(unsafe_code)]`（fandhe-core 既定方針）のもとで実装し、範囲演算は
/// 飽和演算を用いて panic を排除する（panic を API 契約にしない、coding-rust.md）。
pub struct ListRecycler<S> {
    /// 直近の `acquire` 呼び出し時点の状態バージョン。`None` は未初期化（初回呼び出し前）。
    version: Option<u64>,
    /// インデックス → 構築済みノード。ウィンドウ幅に有界（ウィンドウ外は都度解放）。
    slots: BTreeMap<usize, Node<S>>,
    /// 累計の新規構築回数（`expand_item` 呼び出し回数）。統計専用で描画には使わない。
    built: u64,
    /// 累計の再利用回数（キャッシュヒット回数）。統計専用で描画には使わない。
    reused: u64,
}

impl<S> ListRecycler<S> {
    /// 空のプールを生成する。`version` は未初期化のため、最初の `acquire` は
    /// 必ず全項目を新規構築する（初回フレームにキャッシュヒットは存在しない）。
    pub fn new() -> Self {
        Self {
            version: None,
            slots: BTreeMap::new(),
            built: 0,
            reused: 0,
        }
    }

    /// 可視ウィンドウ `window` 内の各項目インデックスについて、キャッシュ済みノードを
    /// 再利用するか `expand_item` で新規構築し、インデックス昇順で参照を返す。
    ///
    /// - `version` が前回呼び出しと異なる場合、全キャッシュを無効化してから構築する
    ///   （状態変化後に古いノードが返る整合性不備を防ぐ、OWASP A08 観点）。
    /// - ウィンドウ外へ出た項目のスロットは解放する（メモリ有界性）。
    /// - `window` が空、または項目総数を超える範囲でも panic しない
    ///   （`expand_item` へは `window` に含まれるインデックスのみを渡すため、
    ///   項目総数の上限チェックは呼び出し側の責務とする）。
    /// - `expand_item` は環境をキャプチャしないよう関数ポインタに限定する
    ///   （`state::ReadFn` と同じ AI 型安全 DSL 方針、REQ-13 / PoC-6 踏襲）。
    pub fn acquire(
        &mut self,
        window: Range<usize>,
        version: u64,
        state: &S,
        expand_item: fn(&S, usize) -> Node<S>,
    ) -> Vec<&Node<S>> {
        // 状態バージョンが進んだ場合は正しさ優先で全キャッシュを無効化する。
        // 初回呼び出し（version: None）もここで「全件再構築」経路に入る。
        if self.version != Some(version) {
            self.slots.clear();
            self.version = Some(version);
        }

        // ウィンドウ外へ出たスロットを解放する（スロット数をウィンドウ幅に有界化）。
        self.slots
            .retain(|index, _| window.start <= *index && *index < window.end);

        for index in window.clone() {
            match self.slots.entry(index) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(expand_item(state, index));
                    self.built = self.built.saturating_add(1);
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    self.reused = self.reused.saturating_add(1);
                }
            }
        }

        window.filter_map(|index| self.slots.get(&index)).collect()
    }

    /// 保持中の全キャッシュを破棄し、次回 `acquire` を全件再構築させる。
    ///
    /// バージョン管理に依らず明示的な無効化が必要な場合（`expand_item` の実装差し替え等、
    /// 状態バージョンだけでは検知できない変更があった場合）に呼び出し側から使う。
    pub fn invalidate(&mut self) {
        self.slots.clear();
        self.version = None;
    }

    /// 累計の新規構築回数（`expand_item` 呼び出し回数）。S4（#87）の性能確認テストが
    /// 「再構築が新規可視分のみに限定されている」ことを assert するための統計値。
    pub fn built_count(&self) -> u64 {
        self.built
    }

    /// 累計の再利用回数（キャッシュヒット回数）。
    pub fn reused_count(&self) -> u64 {
        self.reused
    }

    /// 現在保持しているスロット数（= 直近ウィンドウ幅に有界なキャッシュ件数）。
    /// メモリ有界性のテスト・デバッグ用途。
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }
}

impl<S> Default for ListRecycler<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Node;

    /// テスト専用の項目展開関数。インデックスをラベルへ埋め込むだけの単純な `Text`。
    fn expand_item(_state: &(), index: usize) -> Node<()> {
        Node::Text(format!("item-{index}"))
    }

    /// 同一 version・同一ウィンドウでの再呼び出しは全件再利用され、`built_count` が
    /// 増加しないことを検証する（フレーム間再利用の基本契約）。
    #[test]
    fn acquire_reuses_all_items_on_same_version_and_window() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        let first = recycler.acquire(0..10, 1, &state, expand_item);
        assert_eq!(first.len(), 10);
        assert_eq!(recycler.built_count(), 10);
        assert_eq!(recycler.reused_count(), 0);

        let second = recycler.acquire(0..10, 1, &state, expand_item);
        assert_eq!(second.len(), 10);
        assert_eq!(recycler.built_count(), 10, "再構築は発生しないはず");
        assert_eq!(recycler.reused_count(), 10, "全件が再利用されるはず");
    }

    /// ウィンドウを 1 項目分シフトした場合、新規構築は流入項目のみに限定される
    /// （差分構築の検証）。
    #[test]
    fn acquire_builds_only_newly_visible_item_on_window_shift() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        recycler.acquire(0..10, 1, &state, expand_item);
        assert_eq!(recycler.built_count(), 10);

        // ウィンドウを 1 だけ右にシフト（0..10 → 1..11）。
        // 1..10 の 9 項目は既存キャッシュを再利用し、新規は index=10 のみ。
        recycler.acquire(1..11, 1, &state, expand_item);
        assert_eq!(
            recycler.built_count(),
            11,
            "新規流入 1 件のみ構築されるはず"
        );
        assert_eq!(recycler.reused_count(), 9, "9 件は再利用されるはず");
        assert_eq!(
            recycler.slot_count(),
            10,
            "スロット数はウィンドウ幅を超えないはず"
        );
    }

    /// version 更新後は全件再構築される（キャッシュ無効化の検証）。
    #[test]
    fn acquire_rebuilds_all_items_after_version_change() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        recycler.acquire(0..5, 1, &state, expand_item);
        assert_eq!(recycler.built_count(), 5);

        // version が進んだら、ウィンドウが同一でも全件再構築する（正しさ優先）。
        recycler.acquire(0..5, 2, &state, expand_item);
        assert_eq!(
            recycler.built_count(),
            10,
            "version 変化時は全件再構築されるはず"
        );
        assert_eq!(recycler.reused_count(), 0);
    }

    /// ウィンドウ外へ出たスロットは解放され、スロット数がウィンドウ幅を超えて
    /// 増加しない（メモリ有界性の検証。項目総数に比例したメモリ増加を防ぐ）。
    #[test]
    fn acquire_bounds_slot_count_to_window_width_across_shifts() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        for start in 0..50 {
            recycler.acquire(start..(start + 30), 1, &state, expand_item);
            assert!(
                recycler.slot_count() <= 30,
                "スロット数はウィンドウ幅（30）を超えないはず: start={start}"
            );
        }
    }

    /// 空ウィンドウでも panic せず、構築 0 件・再利用 0 件で返ることを検証する。
    #[test]
    fn acquire_handles_empty_window_without_panic() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        let result = recycler.acquire(5..5, 1, &state, expand_item);
        assert!(result.is_empty());
        assert_eq!(recycler.built_count(), 0);
        assert_eq!(recycler.reused_count(), 0);
    }

    /// 300 項目規模の状態で可視 30 項目のみが構築されることを検証する
    /// （S4（#87）の性能確認テストの前提となる機能検証。REQ-5 の 300 項目規模基準）。
    #[test]
    fn acquire_builds_only_visible_window_out_of_300_items() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        // 300 項目のうち可視ウィンドウは 30 件のみ（index 100..130）。
        let result = recycler.acquire(100..130, 1, &state, expand_item);
        assert_eq!(result.len(), 30);
        assert_eq!(recycler.built_count(), 30, "可視 30 項目のみ構築されるはず");
        assert_eq!(recycler.slot_count(), 30);
    }

    /// `invalidate` 呼び出し後は version が同一でも全件再構築される。
    #[test]
    fn invalidate_forces_full_rebuild_even_with_same_version() {
        let mut recycler: ListRecycler<()> = ListRecycler::new();
        let state = ();

        recycler.acquire(0..5, 1, &state, expand_item);
        assert_eq!(recycler.built_count(), 5);

        recycler.invalidate();
        recycler.acquire(0..5, 1, &state, expand_item);
        assert_eq!(
            recycler.built_count(),
            10,
            "invalidate 後は再構築されるはず"
        );
    }
}
