//! `Node::VirtualList` の可視項目ノードをフレームをまたいで保持するプール
//! （Issue #490 / 親 issue #83 の残課題、#487 で完了した `ScrollArea::show_viewport`
//! 配線の仕上げ）。
//!
//! `fandhe_core::ui::ListRecycler<S>` は可視ウィンドウ内の `Node<S>` をフレーム間で
//! 再利用するキャッシュだが、フレームをまたいで保持する主体が必要になる。
//! `render_screen<S>` は `S: 'static` を要求しない設計（`lib.rs` モジュール doc 参照）
//! であり、egui 側の永続領域（`Ui::memory` / `Context::data`）は `'static + Any +
//! Clone + Send + Sync` を要求するネイティブ実装のため、`Widget<S>` へ
//! `Send + Sync` を追加するという `fandhe-core` 側のより大きな破壊的変更なしには
//! egui 側へ `ListRecycler<S>` を保持できない。そこで本モジュールは egui の永続領域を
//! 使わず、呼び出し側（[`crate::app::ProxyApp`] / [`crate::app::PrototypeApp`] /
//! 結合テストハーネス）がフィールドとして保持し、[`crate::render_screen_with_recyclers`]
//! へ明示的に渡す設計を採る（egui 永続領域案は不採用。out-of-scope-tracking.md 対象の
//! 再検討事項として記録済み）。
//!
//! **前提契約**: 画面は状態の純関数であること（同一
//! [`fandhe_core::StateStore::version`] → 同一画面）。`path`（ツリー上の位置）を
//! キーにプールを再利用するため、version が不変のままツリー構造だけが変わるケースは
//! 契約違反として扱う（`ListRecycler::acquire` 側の version 無効化が「状態変化時は
//! 全件再構築」を保証するため、通常の dispatch 経由の状態変化では stale ノードは
//! 混入しない）。
//!
//! **ネスト対応（take/restore パターン）**: `ListRecycler::acquire` は内部スロットへの
//! 参照 `Vec<&Node<S>>` を返すため、子ノード描画の再帰中に同じプールマップを
//! 可変借用すると借用チェッカーに拒否される。そこで [`VirtualListRecyclers::take`] で
//! 対象 `path` の `ListRecycler` をマップから取り出し（所有権を得る）、
//! `acquire` と項目描画を終えてから [`VirtualListRecyclers::restore`] でマップへ戻す。
//! 取り出されている間は当該 `path` のプールがマップから消えているだけなので、
//! ネストした別 `path` の再帰呼び出しは独立してマップにアクセスできる。
//!
//! **メモリ有界性（A05 対策）**: [`VirtualListRecyclers::begin_frame`] は前フレームで
//! 実際に使われたプール一式を「前フレーム分」として退避し、新しいフレームの
//! プール集合を空から開始する。フレーム中に `take` されなかった前フレームのプールは
//! フレーム境界（次の `begin_frame` 呼び出し時）で破棄される。これによりプール数は
//! 「直近フレームで実際に描画された `VirtualList` 数」に有界となり、画面遷移を
//! 繰り返してもメモリが単調増加しない。

use std::collections::BTreeMap;

use fandhe_core::ui::ListRecycler;

/// 画面ツリー上の `VirtualList` ごとに [`ListRecycler<S>`] をフレームをまたいで
/// 保持するプール（Issue #490）。キーは `path`（ツリー上の兄弟内 index 列。
/// `render_node` / `render_virtual_list` の `path` 引数と同じ表現）。
///
/// 呼び出し元（`ProxyApp` / `PrototypeApp` / 結合テストの `build_ui_state` クロージャ）が
/// フィールドとして保持し、フレームごとに [`begin_frame`][Self::begin_frame] →
/// （描画中に [`take`][Self::take] / [`restore`][Self::restore] を必要な `path` の数だけ
/// 呼ぶ）という手順で使う契約。
pub struct VirtualListRecyclers<S> {
    /// このフレームで `take` → `restore` を経て確定したプール一式。
    /// 次回 `begin_frame` で `previous` へ退避される。
    current: BTreeMap<Vec<usize>, ListRecycler<S>>,
    /// 前フレームのプール一式。今フレームの `take` で消費されなかった分は
    /// 次の `begin_frame` 呼び出し時に破棄される（メモリ有界性）。
    previous: BTreeMap<Vec<usize>, ListRecycler<S>>,
}

impl<S> VirtualListRecyclers<S> {
    /// 空のプール集合を生成する。
    pub fn new() -> Self {
        Self {
            current: BTreeMap::new(),
            previous: BTreeMap::new(),
        }
    }

    /// フレーム開始時に呼ぶ: 直前フレームで確定した `current` を `previous` へ退避し、
    /// 新しいフレームの `current` を空から開始する。
    ///
    /// [`render_screen_with_recyclers`][crate::render_screen_with_recyclers] が
    /// 走査開始前に 1 度だけ呼ぶ。`previous` に残っていた（= 前回 `begin_frame` 以降
    /// 一度も `take` されなかった）プールはここで破棄される
    /// （ルート遷移等で描画されなくなった `VirtualList` のプールを解放し、
    /// プール数を「直近フレームで実際に描画された `VirtualList` 数」に有界化する）。
    pub fn begin_frame(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    /// `path` に対応する [`ListRecycler<S>`] の所有権を取得する。
    ///
    /// `current`（今フレームで既に `restore` 済みの分。同一 `path` を 1 フレーム中に
    /// 複数回 `take` するケースは通常発生しないが安全側で両方探索する）→ `previous`
    /// （前フレームからの継続分）の順に探索し、どちらにも無ければ
    /// [`ListRecycler::new`] で新規生成する（初出の `VirtualList`、または直前フレームで
    /// 一度も描画されなかった `path` の場合）。
    ///
    /// 返した値は呼び出し側が `acquire` と項目描画を終えた後、[`restore`][Self::restore]
    /// で必ずマップへ戻す契約（戻さないとそのフレーム限りで当該プールが失われ、
    /// 次フレームで全件再構築になる）。
    pub fn take(&mut self, path: &[usize]) -> ListRecycler<S> {
        if let Some(recycler) = self.current.remove(path) {
            return recycler;
        }
        if let Some(recycler) = self.previous.remove(path) {
            return recycler;
        }
        ListRecycler::new()
    }

    /// [`take`][Self::take] で取り出した `recycler` を `path` のキーで `current` へ戻す。
    pub fn restore(&mut self, path: &[usize], recycler: ListRecycler<S>) {
        self.current.insert(path.to_vec(), recycler);
    }

    /// 現在 `current` に確定しているプール数（= 直近フレームで実際に描画された
    /// `VirtualList` の数）。メモリ有界性の検証・デバッグ用途。
    pub fn pool_count(&self) -> usize {
        self.current.len()
    }

    /// `current` の全プールを合算した累計新規構築回数
    /// （[`ListRecycler::built_count`] の総和）。結合テスト・デバッグ用途。
    pub fn total_built_count(&self) -> u64 {
        self.current.values().map(ListRecycler::built_count).sum()
    }

    /// `current` の全プールを合算した累計再利用回数
    /// （[`ListRecycler::reused_count`] の総和）。結合テスト・デバッグ用途。
    pub fn total_reused_count(&self) -> u64 {
        self.current.values().map(ListRecycler::reused_count).sum()
    }
}

impl<S> Default for VirtualListRecyclers<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::ui::Node;

    /// テスト専用の項目展開関数（`recycle.rs` のテストと同じ形）。
    fn expand_item(_state: &(), index: usize) -> Node<()> {
        Node::Text(format!("item-{index}"))
    }

    /// `take` → `acquire` → `restore` の一往復で、同一 `path` を次フレームでも
    /// `take` すればキャッシュが引き継がれている（フレーム間再利用の基本契約）。
    #[test]
    fn take_and_restore_preserves_recycler_state_across_frames() {
        let mut recyclers: VirtualListRecyclers<()> = VirtualListRecyclers::new();
        let path = vec![0usize];
        let state = ();

        recyclers.begin_frame();
        let mut recycler = recyclers.take(&path);
        recycler.acquire(0..10, 1, &state, expand_item);
        assert_eq!(recycler.built_count(), 10);
        recyclers.restore(&path, recycler);
        assert_eq!(recyclers.total_built_count(), 10);
        assert_eq!(recyclers.pool_count(), 1);

        // 次フレーム: 同一 version・同一 window で再度 take → acquire すると全件再利用される。
        recyclers.begin_frame();
        let mut recycler = recyclers.take(&path);
        assert_eq!(
            recycler.built_count(),
            10,
            "前フレームの構築済みノードが引き継がれているはず"
        );
        let nodes = recycler.acquire(0..10, 1, &state, expand_item);
        assert_eq!(nodes.len(), 10);
        assert_eq!(
            recycler.built_count(),
            10,
            "フレームをまたいでも再構築は発生しないはず"
        );
        assert_eq!(recycler.reused_count(), 10);
        recyclers.restore(&path, recycler);
    }

    /// `begin_frame` で `take` されなかった前フレームのプールは破棄される
    /// （メモリ有界性: ルート遷移等で描画されなくなった `path` のプールを解放する）。
    #[test]
    fn begin_frame_discards_pools_not_taken_in_previous_frame() {
        let mut recyclers: VirtualListRecyclers<()> = VirtualListRecyclers::new();
        let path_a = vec![0usize];
        let path_b = vec![1usize];
        let state = ();

        recyclers.begin_frame();
        let mut recycler_a = recyclers.take(&path_a);
        recycler_a.acquire(0..5, 1, &state, expand_item);
        recyclers.restore(&path_a, recycler_a);
        let mut recycler_b = recyclers.take(&path_b);
        recycler_b.acquire(0..5, 1, &state, expand_item);
        recyclers.restore(&path_b, recycler_b);
        assert_eq!(recyclers.pool_count(), 2);

        // 次フレームでは path_a のみ描画（path_b は画面遷移で消えた想定）。
        recyclers.begin_frame();
        let mut recycler_a = recyclers.take(&path_a);
        assert_eq!(recycler_a.built_count(), 5, "path_a は引き継がれているはず");
        recycler_a.acquire(0..5, 1, &state, expand_item);
        recyclers.restore(&path_a, recycler_a);
        assert_eq!(
            recyclers.pool_count(),
            1,
            "path_b を take しなかったフレームでは pool_count は 1 のはず"
        );

        // 3 フレーム目に入ったところで、path_b が「1 フレーム前（2 フレーム目）に
        // 一度も take されなかった前フレーム分」として破棄される
        // （`begin_frame` は「直前の `current`」を `previous` へ退避する契約のため、
        // 2 フレーム目の `previous`（1 フレーム目の current）にはまだ path_b が
        // 残っており、2 フレーム目中に take すれば復元できてしまう。破棄を
        // 確認するには、path_b を運ばなかった 2 フレーム目を経てからの
        // 3 フレーム目まで進める必要がある）。
        recyclers.begin_frame();
        let recycler_b = recyclers.take(&path_b);
        assert_eq!(
            recycler_b.built_count(),
            0,
            "破棄された path_b は新規プールとして返るはず"
        );
    }

    /// 未知の `path` を `take` した場合は新規の空プールが返る。
    #[test]
    fn take_creates_new_recycler_for_unknown_path() {
        let mut recyclers: VirtualListRecyclers<()> = VirtualListRecyclers::new();
        let recycler = recyclers.take(&[42usize]);
        assert_eq!(recycler.built_count(), 0);
        assert_eq!(recycler.slot_count(), 0);
    }

    /// 複数 `path` の統計が `total_built_count` / `total_reused_count` へ合算されることを検証する。
    #[test]
    fn total_counts_sum_across_multiple_pools() {
        let mut recyclers: VirtualListRecyclers<()> = VirtualListRecyclers::new();
        let state = ();

        recyclers.begin_frame();
        let mut recycler_a = recyclers.take(&[0usize]);
        recycler_a.acquire(0..3, 1, &state, expand_item);
        recyclers.restore(&[0usize], recycler_a);

        let mut recycler_b = recyclers.take(&[1usize]);
        recycler_b.acquire(0..7, 1, &state, expand_item);
        recyclers.restore(&[1usize], recycler_b);

        assert_eq!(recyclers.total_built_count(), 10);
        assert_eq!(recyclers.total_reused_count(), 0);
        assert_eq!(recyclers.pool_count(), 2);
    }
}
