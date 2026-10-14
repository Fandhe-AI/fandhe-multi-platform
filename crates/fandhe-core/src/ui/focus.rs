//! Android TV 等のリモコン操作向けフォーカス移動・再解決コア（TASK-17.2 / REQ-17）。
//!
//! `docs/architecture/tv-focus-navigation-architecture.md`（§3・§4.4・§5）の設計を
//! 実装化したもの。共通コアはレイアウト・実描画矩形を一切持たない
//! （`responsibility-boundary.md` §3）ため、本モジュールが提供するのは
//!
//! - フォーカス可能ノード（`Node::Button` / `Node::IndexedButton`、§3.1）の
//!   ツリー順（DFS・挿入順、§3.2）に基づく近傍探索・軸解決（§4.4「軸解決アルゴリズム」）
//! - 現在フォーカス中の 1 件（[`FocusEntry`]）のみを保持し、`id`（`WidgetId`）比較で
//!   構造変化を自動検知・復旧する自己更新型 API（[`FocusRing::rebuild`]、§4.4）
//!
//! までであり、`VirtualList` の描画窓（`VisibleRange`）の保持・更新（reveal 契約、
//! §4.6）と、決定ボタン押下時の実発火（[`super::event::activate_focus_at`]、§5）は
//! いずれも本モジュールの外側（プラットフォーム統合レイヤ／`event` モジュール）の
//! 責務であり、`FocusRing` はどちらも行わない。
//!
//! 呼び出し文脈: プラットフォームレイヤ（Android TV）が方向キー入力を受けるたびに
//! [`FocusRing::move_focus`] を、状態 `S` 更新後（または画面表示直後）に
//! [`FocusRing::rebuild`] を呼ぶ想定。両メソッドとも `Screen<S>` / `state: &S` を
//! 都度受け取り、`Node::List::expand` 等の都度評価の設計を踏襲する（レイアウト・
//! 描画状態は共通コア側で保持しない）。

use std::collections::{HashMap, VecDeque};

use super::event::resolve_node_at;
use super::node::{Axis, Node};
use super::screen::Screen;
use super::virtual_list::VisibleRange;
use super::widget::WidgetId;

/// 第 2 段（有界全域探索）が `VirtualList` を探索する際の窓マージン件数
/// （`docs/architecture/tv-focus-navigation-architecture.md` §4.4「探索窓ヒント」）。
///
/// 値は本書が固定しない実装裁量（同節「K の具体値は本書では固定せず TASK-17.2 の
/// 実測で決定する」）。TV 向け `VirtualList` は 1 画面あたり数十件程度の可視項目を
/// 想定しており、スクロール中の 1 ステップ分の項目数を吸収できる余裕を持たせつつ
/// 探索コストを抑える初期値として `8` を選んだ。実機・実データでの体感遅延計測後に
/// 調整する前提の値である（7 節「対象外」に記録された実測ベースの再検討事項）。
const VIRTUAL_LIST_SEARCH_MARGIN: usize = 8;

/// 初回解決・第 3 段（先頭フォールバック）が `VirtualList` を先頭から走査する際の
/// 上限件数（設計文書 §4.4「先頭フォールバック」・§7「フォーカス可能項目の索引化・
/// スキップ探索の上限設定」）。この走査は探索窓ヒントを用いない（[`VIRTUAL_LIST_SEARCH_MARGIN`]
/// とは独立の定数）。
///
/// codex-review P1 指摘（フォーカス可能項目が存在しない・末尾にしかない大規模
/// `VirtualList` で `0..len(state)` を無制限に走査し、仮想化契約〔`item()` 呼び出しが
/// `len` に比例しない〕を破る）を受けて導入した。値は本書 §7 が実測ベースの再検討
/// 事項として先送りしていたものを、この不変条件（有界化）の観点で最小限確定した
/// 初期値である。TV 向け画面が 1 画面あたり想定する可視項目数（数十件程度）の
/// 数倍を確保しつつ、数万件規模のリストでの不定コストを避ける値として `256` を
/// 選んだ。実機・実データでの体感遅延計測後に調整する前提の値である。
const FALLBACK_SCAN_LIMIT: usize = 256;

/// 観測キャッシュ（[`ObservedCache`]）の既定上限件数
/// （設計文書 §4.4「観測キャッシュと再捕捉保証の層別化」）。
///
/// verify-on-use 方式（使用時に必ず path 再解決 + `id` 比較で検証する）のため
/// 無効化管理を必要とせず、超過時は古い記録から破棄するだけで正しさに影響しない
/// （破棄された記録は第 2 段以降の探索が代替する）。TV 向け画面が一度に表示する
/// フォーカス可能項目数（数十件程度を想定）に余裕を持たせつつ無制限の増大を
/// 避ける初期値として `64` を選んだ。実測で調整する前提の値である。
const OBSERVED_CACHE_CAPACITY: usize = 64;

/// 呼び出し元（プラットフォームレイヤ）が第 2 段（有界全域探索）へ渡す探索窓ヒント
/// （設計文書 §4.4「探索窓ヒント」）。`VirtualList` コンテナの path（`Vec<usize>`）から
/// そのコンテナの現在の可視範囲 [`VisibleRange`] への読み取り専用マッピングであり、
/// `FocusRing` は状態として保持せず、[`FocusRing::rebuild`] の呼び出しのたびに
/// 引数として受け取るのみである（`state: &S` を都度受け取る既存設計と同型、
/// 設計文書 §4.6「reveal 契約」と矛盾しない）。
pub type WindowHints = HashMap<Vec<usize>, VisibleRange>;

/// 現在フォーカス中のノードを「path + 安定識別子」の組で保持する
/// （設計文書 §4.4）。`path` は「ツリー上の位置」を指すだけで「論理的に同一の
/// 項目」を指し続ける保証はない（§3.2）。`id` は `rebuild` / `activate_focus_at`
/// （`super::event`、§5）の双方で「今その path にある葉ノードは本当に前回
/// フォーカスしていた論理項目と同一か」を検証する一次情報として使う。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusEntry {
    /// フォーカス中ノードの path（§3.2 の探索順序上の位置）。
    pub path: Vec<usize>,
    /// `path` 解決時点で読み取ったノードの `WidgetId`（§3.1）。
    pub id: WidgetId,
}

/// 方向キーの 4 方向（設計文書 §4.4）。4 方向とも「直近の祖先コンテナの `Axis` が
/// この方向を扱うか」という単一規則で解決する（§4.2「4 巡目レビューでの指摘」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Up,
    Down,
    Left,
    Right,
}

/// `WidgetId → 最後に観測した path` の有界マップ（観測キャッシュ、設計文書 §4.4
/// 「観測キャッシュと再捕捉保証の層別化」）。
///
/// `FocusRing` が近傍探索・path 解決・再解決で `Button`/`IndexedButton` を評価する
/// たびに、その副作用として記録する（維持コストゼロ。どのみち発生する項目評価への
/// 相乗りであり、キャッシュのために追加の `item()` 呼び出し・走査は行わない）。
/// verify-on-use 方式のため無効化管理を持たず、超過時は最も古い記録（FIFO）から
/// 破棄する。破棄されても第 2 段（有界全域探索）以降が動くだけで正しさに影響しない。
#[derive(Debug, Default, Clone)]
struct ObservedCache {
    /// 破棄順（挿入順）を FIFO で管理する。`paths` とキー集合を常に一致させる。
    order: VecDeque<WidgetId>,
    paths: HashMap<WidgetId, Vec<usize>>,
}

impl ObservedCache {
    /// `id` の目撃記録を `path` で更新する（新規なら追加、既存なら上書き）。
    /// 上限超過時は最も古い記録を 1 件破棄する。
    fn record(&mut self, id: WidgetId, path: &[usize]) {
        if !self.paths.contains_key(&id) {
            if self.order.len() >= OBSERVED_CACHE_CAPACITY {
                if let Some(evicted) = self.order.pop_front() {
                    self.paths.remove(&evicted);
                }
            }
            self.order.push_back(id);
        }
        self.paths.insert(id, path.to_vec());
    }

    /// `id` の最後の目撃 path を返す（見つからなければ `None`）。
    fn lookup(&self, id: WidgetId) -> Option<Vec<usize>> {
        self.paths.get(&id).cloned()
    }

    /// stale と判明した記録を破棄する（verify-on-use で不一致が判明した場合に呼ぶ）。
    /// `order`（`VecDeque`）内の探索は上限が小さい定数（[`OBSERVED_CACHE_CAPACITY`]）
    /// のため線形探索でも許容する。
    fn discard(&mut self, id: WidgetId) {
        self.paths.remove(&id);
        if let Some(pos) = self.order.iter().position(|&x| x == id) {
            self.order.remove(pos);
        }
    }
}

/// 現在フォーカス中のノードを保持し、方向キー入力の解決・状態変化後の自動復旧を
/// 行う自己更新型のフォーカス状態（設計文書 §4.4）。
///
/// `Screen<S>` / 状態 `S` を所有せず、[`Self::rebuild`] / [`Self::move_focus`] の
/// 呼び出しごとに `&Screen<S>` / `&S` を受け取って都度解決する（`Node::List::expand`
/// 等の都度評価の設計を踏襲、`render.rs` と同じ思想）。`VirtualList` の描画窓
/// （`VisibleRange`）は保持・変更しない（設計文書 §4.6「可視範囲の追随（reveal）契約」、
/// プラットフォーム統合レイヤの責務）。
#[derive(Debug, Default, Clone)]
pub struct FocusRing {
    current: Option<FocusEntry>,
    observed: ObservedCache,
    /// 直近の [`Self::rebuild`] 呼び出しで受け取った [`WindowHints`] のコピー
    /// （codex-review P1 対応: [`Self::move_focus`] の方向探索有界化に使う、
    /// [`find_candidate_in_container`] 参照）。`move_focus` は `Screen<S>` / `S` は
    /// 都度受け取るが `WindowHints` までは受け取らない公開シグネチャ（0.2.0 で
    /// 確定済み・変更不可）のため、`rebuild` の呼び出しのたびにここへ複製しておき
    /// `move_focus` から読み出す。`rebuild` が一度も呼ばれていない場合は空マップ
    /// のままであり、[`find_candidate_in_container`] は「ヒントなし」として現在位置
    /// 中心の固定マージンへフォールバックする。
    window_hints: WindowHints,
    /// 段階探索カーソル（codex-review 9 巡目 P1 対応、詳細は [`SearchCursor`]
    /// のドキュメント参照）。`WindowBounded`（[`ContainerSearch`]）で打ち切った
    /// 探索の再開位置を、同一コンテナ・同一方向・同一フォーカスのまま連打された
    /// 場合に限り引き継ぐ。
    search_cursor: Option<SearchCursor>,
}

/// [`FocusRing::search_cursor`] が保持する段階探索の再開状態（codex-review 9 巡目
/// P1 対応）。
///
/// 背景: `WindowBounded`（[`ContainerSearch`]）は「窓の外に候補が残るかもしれない
/// が、この 1 回の入力ではそこまで探索しない」という有界化の結果であり、
/// フォーカス自体は移動しない。566bc97 時点のコメントは「reveal 後の再入力で
/// 前進する」としていたが、reveal はフォーカス移動（＝描画側の追随）を前提に
/// 発火するため、フォーカスが動かない限り reveal も `WindowHints` も進まず、
/// 同じ入力を繰り返しても同じ探索窓・同じ打ち切り位置が再現するだけだった
/// （非フォーカス項目が [`VIRTUAL_LIST_SEARCH_MARGIN`] 幅より連続する
/// `VirtualList` では到達可能な項目へ永久に移動できない、9 巡目 codex-review
/// 指摘）。
///
/// 対応方針: `FocusRing` 側に「どこまで調べ終えたか」を覚えておく探索カーソルを
/// 持たせ、同一コンテナ・同一方向への入力が連続する限り、打ち切り位置の続きから
/// 探索窓を再構成する。フォーカス・画面は段階探索の途中では動かない（視覚的な
/// フィードバックはない）が、1 入力あたりの走査量は従来どおり
/// `VIRTUAL_LIST_SEARCH_MARGIN` 相当で有界のまま、連打を重ねることで任意の
/// 距離まで前進できる（各入力が O(margin) を保ったまま合計 O(距離) で到達する
/// 償却的な段階探索）。到達時（[`ContainerSearch::Found`]）または コンテナの
/// 実端（[`ContainerSearch::Exhausted`]）に達した時点でカーソルを破棄する。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchCursor {
    /// 探索対象コンテナの path（[`find_candidate_in_container`] の
    /// `container_prefix` と同一空間）。
    container_prefix: Vec<usize>,
    /// カーソルが有効な方向。方向が変われば無条件で無効化する。
    direction: FocusDirection,
    /// 次回探索で最初に検証する index（前回 [`ContainerSearch::WindowBounded`]
    /// で未検証のまま打ち切った index）。
    resume_index: usize,
    /// カーソル生成時点でフォーカスしていたノードの `id`。`move_focus` 呼び出し
    /// 時・`rebuild` 呼び出し後に現在のフォーカス `id` と比較し、不一致なら
    /// 「フォーカスが動いた／再捕捉で別項目に切り替わった」とみなして無効化する
    /// （段階探索中はフォーカス自体が動かない設計のため、一致していれば同じ
    /// 探索の続きとみなせる）。
    anchor_id: WidgetId,
    /// カーソル生成時点でフォーカスしていたノードの `path`（codex-review should
    /// 指摘対応）。`id` 一致のみでは不十分: `rebuild` の三段探索（第 1〜2 段）は
    /// 「同じ `id` の項目を別の path で再捕捉する」ことを正規の挙動として許容する
    /// （リスト並べ替え等で基底データの index が変わるケース）。段階探索中に
    /// そうした再捕捉が起きると、`resume_index` は古い index 空間のまま次の
    /// `move_focus` へ引き継がれ、並べ替え後の別コンテナ・別位置から探索が
    /// 再開してしまう（意図しないジャンプ・逆行・到達不能）。「段階探索中は
    /// フォーカスの path も不変」という設計前提を検証に落とし込むため、`id` と
    /// `path` の両方が一致する場合のみカーソルを有効とみなす。
    anchor_path: Vec<usize>,
}

impl FocusRing {
    /// 空のフォーカス状態を作る（`current == None`）。
    pub fn new() -> Self {
        Self::default()
    }

    /// `self.current` を更新し、[`SearchCursor`] のアンカー（`anchor_id` と
    /// `anchor_path` の両方）と整合しなくなった場合はカーソルを破棄する
    /// （[`rebuild`](Self::rebuild) の複数の早期 return 経路すべてで同じ無効化
    /// 判定を共有するためのヘルパー）。`id` のみの一致判定では、`rebuild` の
    /// 三段探索が同一 `id` を別 path で再捕捉した場合（リスト並べ替え等）を
    /// 見逃し、`resume_index` を古い index 空間のまま使い回してしまう
    /// （[`SearchCursor::anchor_path`] のドキュメント参照）。
    fn set_current(&mut self, entry: Option<FocusEntry>) {
        if let Some(cursor) = &self.search_cursor {
            let anchor_still_valid = entry
                .as_ref()
                .is_some_and(|e| e.id == cursor.anchor_id && e.path == cursor.anchor_path);
            if !anchor_still_valid {
                self.search_cursor = None;
            }
        }
        self.current = entry;
    }

    /// 現在フォーカス中のノードを返す（フォーカス可能ノードが 1 つも存在しない
    /// 場合、または未初期化の場合は `None`）。
    pub fn current(&self) -> Option<&FocusEntry> {
        self.current.as_ref()
    }

    /// `self.current` を `id` 比較で自動検証し、必要なら復旧する（設計文書 §4.4
    /// 「再解決アルゴリズム」）。呼び出し元の構造変化申告に依存しない自己更新型
    /// API であり、`current` と観測キャッシュは同一インスタンス上で呼び出しを
    /// またいで引き継がれる（10 巡目改訂で構築型から変更した理由も同節参照）。
    ///
    /// `window_hints` は第 2 段（有界全域探索）が `VirtualList` の探索窓を定める
    /// ための読み取り専用ヒント（[`WindowHints`] 参照）。
    pub fn rebuild<S>(&mut self, screen: &Screen<S>, state: &S, window_hints: &WindowHints) {
        // `move_focus` が方向探索の有界化に使えるよう複製しておく（`window_hints`
        // フィールドのコメント参照、codex-review P1 対応）。
        self.window_hints = window_hints.clone();

        let Some(entry) = self.current.take() else {
            // 1. 初回呼び出し: ツリー先頭から最初のフォーカス可能ノードを探索する
            //    （最初の一致で早期終了する走査、設計文書 §4.4 第 3 段と同一規則）。
            let resolved = find_first_focusable(screen, state, &mut self.observed);
            self.set_current(resolved);
            return;
        };

        // 2. 前回の path を 1 回だけ解決し、id が一致すればそのまま採用する
        //    （O(path の深さ) のみの主経路）。
        if let Some(node) = resolve_node_at(screen, &entry.path, state) {
            if let Some(id) = focusable_id(&node) {
                if id == entry.id {
                    self.observed.record(id, &entry.path);
                    self.set_current(Some(entry));
                    return;
                }
            }
        }

        // 3. id 不一致（または path 範囲外・非葉ノード）を検知した場合の三段探索。
        let target = entry.id;

        // 第 1 段: 観測キャッシュ照会（verify-on-use）。
        if let Some(cached_path) = self.observed.lookup(target) {
            if let Some(node) = resolve_node_at(screen, &cached_path, state) {
                if let Some(id) = focusable_id(&node) {
                    if id == target {
                        self.observed.record(id, &cached_path);
                        self.set_current(Some(FocusEntry {
                            path: cached_path,
                            id,
                        }));
                        return;
                    }
                }
            }
            // 記録が古く不一致だった場合は捨てて第 2 段へ進む（外れても正しさに
            // 影響しない、設計文書 §4.4「観測キャッシュと再捕捉保証の層別化」）。
            self.observed.discard(target);
        }

        // 第 2 段: 有界全域探索（`List` は全展開、`VirtualList` は探索窓のみ）。
        if let Some(found) = find_by_id(screen, state, target, window_hints, &mut self.observed) {
            self.set_current(Some(found));
            return;
        }

        // 第 3 段: 先頭フォールバック（項目自体が削除された場合。初回解決と同一規則）。
        let resolved = find_first_focusable(screen, state, &mut self.observed);
        self.set_current(resolved);
    }

    /// 方向キー入力を解決し、`self.current` を移動先へ更新する（設計文書 §4.4
    /// 「軸解決アルゴリズム」）。移動できた場合は `true`、境界到達・候補不在で
    /// no-op に確定した場合は `false` を返す。
    ///
    /// `self.current` が `None`（フォーカス可能ノードが存在しない画面）の場合は
    /// 何もせず `false` を返す。呼び出し前に [`Self::rebuild`] を呼び、`current`
    /// を最新化しておく想定（設計文書 §4.4「軸解決アルゴリズム」冒頭の前提）。
    ///
    /// `VirtualList` コンテナ内の移動探索は直近の `rebuild` で受け取った
    /// [`WindowHints`]（[`Self::window_hints`] フィールド）と固定マージンで
    /// 有界化する（codex-review P1 対応: 巨大 `VirtualList` で現在位置以降が
    /// 非フォーカス要素だと 1 回の方向キー入力で全項目を生成しうる仮想化契約違反の
    /// 是正、[`find_candidate_in_container`] 参照）。探索窓の外までは移動できず
    /// 移動失敗（フォーカス維持）として扱う。
    ///
    /// 窓境界で打ち切った探索は [`Self::search_cursor`]（[`SearchCursor`]）に
    /// 再開位置を記録し、同一コンテナ・同一方向への入力が連続する限り続きから
    /// 探索する（codex-review 9 巡目 P1 対応、[`SearchCursor`] のドキュメント
    /// 参照）。段階探索の途中はフォーカス・画面とも動かないが、1 入力あたりの
    /// 走査量は引き続き有界のまま、連打を重ねることで到達可能な項目へ最終的に
    /// 移動できる。
    pub fn move_focus<S>(
        &mut self,
        screen: &Screen<S>,
        state: &S,
        direction: FocusDirection,
    ) -> bool {
        let Some(entry) = self.current.clone() else {
            return false;
        };
        let anchor_id = entry.id;
        let path = entry.path;
        if path.is_empty() {
            return false;
        }

        // 段階探索カーソルは「方向が変わった」「フォーカスが（前回の rebuild 等で）
        // 別項目へ切り替わった、または同一 id のまま別 path へ再捕捉された」の
        // いずれかで即座に無効化する（`SearchCursor` ドキュメントの reset 条件）。
        // 以降のコンテナ単位のマッチングは `container_prefix` の一致のみで
        // 判定すればよい。
        if let Some(cursor) = &self.search_cursor {
            if cursor.direction != direction
                || cursor.anchor_id != anchor_id
                || cursor.anchor_path != path
            {
                self.search_cursor = None;
            }
        }

        // `t` は「直近祖先コンテナ」の深さを表す。`t == 0` はトップレベル
        // （`Screen` の挿入順、暗黙の `Axis::Vertical`）を意味する（設計文書
        // §4.4「コンテナと階層」）。祖先コンテナへエスケープするたびに 1 つ減らす。
        let mut t = path.len();
        loop {
            if t == 0 {
                // トップレベルまで遡っても候補が見つからない: エスケープの終端、no-op。
                return false;
            }
            let container_prefix = &path[..t - 1];
            let child_index = path[t - 1];

            let Some((axis, len)) = container_axis_len(screen, state, container_prefix) else {
                // 構造不整合（中間ノードが Button に化けた等）: 安全側で no-op。
                return false;
            };

            if axis_step(axis, direction).is_some() {
                // このコンテナ・この方向に対応する段階探索カーソルがあれば、
                // その再開位置から続きを探す（`child_index` 起点の毎回同じ窓へ
                // 戻ってしまうのを避ける、`SearchCursor` ドキュメント参照）。
                let resume_index = match &self.search_cursor {
                    Some(cursor) if cursor.container_prefix == container_prefix => {
                        Some(cursor.resume_index)
                    }
                    _ => None,
                };

                match find_candidate_in_container(
                    screen,
                    state,
                    container_prefix,
                    axis,
                    direction,
                    child_index,
                    resume_index,
                    len,
                    &self.window_hints,
                    &mut self.observed,
                ) {
                    ContainerSearch::Found(new_entry) => {
                        self.search_cursor = None;
                        self.current = Some(new_entry);
                        return true;
                    }
                    ContainerSearch::WindowBounded { next_index } => {
                        // 探索窓の境界で打ち切った: 先に候補がいる可能性が残るため
                        // 祖先コンテナへエスケープせず、フォーカス維持のまま
                        // 移動失敗として確定する（Cursor Bugbot High 指摘対応、
                        // `ContainerSearch` のドキュメント参照）。次回同じ
                        // コンテナ・同じ方向への入力が続けばここから再開する
                        // （`SearchCursor` ドキュメント参照）。
                        self.search_cursor = Some(SearchCursor {
                            container_prefix: container_prefix.to_vec(),
                            direction,
                            resume_index: next_index,
                            anchor_id,
                            anchor_path: path.clone(),
                        });
                        return false;
                    }
                    ContainerSearch::Exhausted => {
                        // コンテナの端（実端）まで走査して候補がなかった: 従来どおり
                        // 祖先コンテナへのエスケープ条件（下へ続く）。このコンテナに
                        // 対する段階探索カーソルはもう意味を持たないため破棄する。
                        if matches!(
                            &self.search_cursor,
                            Some(cursor) if cursor.container_prefix == container_prefix
                        ) {
                            self.search_cursor = None;
                        }
                    }
                }
            }

            // 自コンテナがこの方向を扱わない、またはコンテナの端まで扱って候補が
            // 存在しなかった: 双方とも祖先コンテナへのエスケープ条件（設計文書
            // §4.4 の 5 巡目改訂で統一済み）。
            t -= 1;
        }
    }
}

/// `node` がフォーカス可能（`Button` / `IndexedButton`、§3.1）であれば `id` を返す。
fn focusable_id<S>(node: &Node<S>) -> Option<WidgetId> {
    match node {
        Node::Button { id, .. } | Node::IndexedButton { id, .. } => Some(*id),
        _ => None,
    }
}

/// `Node::List` / `Node::VirtualList` の分岐を正規化した列挙ビュー（#537、旧
/// `container_axis_len` / `container_child` / `container_frame_with_window` /
/// `find_candidate_in_container` の window_bound 判定に散在していた同型 match の
/// 集約先）。[`container_kind`] が生成する。
///
/// `Virtual` バリアントは `item()`（1 項目分の `Node<S>` 生成関数）を**呼び出さない**
/// （仮想化契約: `item()` 評価は呼び出し側が窓内の index に限定して行う）。関数
/// ポインタそのものを保持するだけであり、呼び出し側は `len` で範囲を確認した上で
/// 必要な index だけ `item(state, idx)` を呼ぶ。
enum ContainerKind<S> {
    /// `List`: `expand(state)` を 1 回だけ評価した展開済みの子と軸。
    Slice { axis: Axis, children: Vec<Node<S>> },
    /// `VirtualList`: 総数（`len(state)` 評価済み）と遅延アクセサ（`item()` 未評価）。
    Virtual {
        axis: Axis,
        len: usize,
        item: crate::state::ReadIndexedFn<S, Node<S>>,
    },
}

/// `node` を [`ContainerKind`] へ正規化する。`node` が `List`/`VirtualList` 以外
/// （構造不整合、または `Button` 等の葉ノード）の場合は `None`。
///
/// `List` は `expand(state)` をここで 1 回だけ評価する（呼び出し元が同じ内容を
/// 二重評価しないよう、以降は返された `children` を使い回すこと）。`VirtualList`
/// は `len(state)` のみ評価し、`item()` はここでは一切呼ばない（型ドキュメント参照）。
fn container_kind<S>(node: &Node<S>, state: &S) -> Option<ContainerKind<S>> {
    match node {
        Node::List { expand, axis } => Some(ContainerKind::Slice {
            axis: *axis,
            children: expand(state),
        }),
        Node::VirtualList {
            len, item, axis, ..
        } => Some(ContainerKind::Virtual {
            axis: *axis,
            len: len(state),
            item: *item,
        }),
        _ => None,
    }
}

/// `container_prefix` が指すコンテナの `Axis` と項目数を返す。`container_prefix` が
/// 空（トップレベル）の場合は暗黙の `Axis::Vertical` とし、項目数は `Screen` の
/// トップレベルノード数を返す（設計文書 §4.4「コンテナと階層」の `t = 0` 特例）。
/// コンテナが `List` / `VirtualList` 以外を指す場合（構造不整合）は `None`。
fn container_axis_len<S>(
    screen: &Screen<S>,
    state: &S,
    container_prefix: &[usize],
) -> Option<(Axis, usize)> {
    if container_prefix.is_empty() {
        return Some((Axis::Vertical, screen.len()));
    }
    let node = resolve_node_at(screen, container_prefix, state)?;
    match container_kind(&node, state)? {
        ContainerKind::Slice { axis, children } => Some((axis, children.len())),
        ContainerKind::Virtual { axis, len, .. } => Some((axis, len)),
    }
}

/// `container_prefix` が指すコンテナの `index` 番目の子ノードを取得する
/// （`container_axis_len` と対になる取得ヘルパー）。
fn container_child<S>(
    screen: &Screen<S>,
    state: &S,
    container_prefix: &[usize],
    index: usize,
) -> Option<Node<S>> {
    if container_prefix.is_empty() {
        return screen.iter().nth(index).cloned();
    }
    let node = resolve_node_at(screen, container_prefix, state)?;
    match container_kind(&node, state)? {
        ContainerKind::Slice { children, .. } => children.into_iter().nth(index),
        ContainerKind::Virtual { len, item, .. } => {
            if index >= len {
                None
            } else {
                Some(item(state, index))
            }
        }
    }
}

/// 自コンテナが `direction` を扱う場合、隣接 index への差分を返す（設計文書 §4.4
/// 「コンテナ単位の解決表」）。`Left`/`Right` は `Grid` も含め常に ±1（`Grid` は
/// 別途行番号制約を課す、[`find_candidate_in_container`] 参照）、`Up`/`Down` は
/// `Grid { columns }` のみ ±`columns`。扱わない組み合わせは `None`
/// （祖先コンテナへのエスケープ条件、`move_focus` 参照）。
///
/// `columns` は開発者が渡す任意の `usize` であり、`columns.max(1) as isize` の
/// キャストは `columns > isize::MAX` で折り返す（`usize::MAX` → `-1` で Up/Down が
/// 反転する、`isize::MAX + 1` → `isize::MIN` で単項マイナスが panic する、いずれも
/// codex-review P1 指摘）。`isize::try_from` で検証し、表現できない場合は
/// 「この方向は扱わない」と同じ `None`（祖先コンテナへのエスケープ）で安全側に倒す。
fn axis_step(axis: Axis, direction: FocusDirection) -> Option<isize> {
    match (axis, direction) {
        (Axis::Vertical, FocusDirection::Up) => Some(-1),
        (Axis::Vertical, FocusDirection::Down) => Some(1),
        (Axis::Vertical, FocusDirection::Left | FocusDirection::Right) => None,
        (Axis::Horizontal, FocusDirection::Left) => Some(-1),
        (Axis::Horizontal, FocusDirection::Right) => Some(1),
        (Axis::Horizontal, FocusDirection::Up | FocusDirection::Down) => None,
        (Axis::Grid { .. }, FocusDirection::Left) => Some(-1),
        (Axis::Grid { .. }, FocusDirection::Right) => Some(1),
        (Axis::Grid { columns }, FocusDirection::Up) => {
            isize::try_from(columns.max(1)).ok().map(|c| -c)
        }
        (Axis::Grid { columns }, FocusDirection::Down) => isize::try_from(columns.max(1)).ok(),
    }
}

/// `move_focus` の方向探索が `VirtualList` コンテナ内で許容する index 範囲を求める
/// （codex-review P1 対応: `find_candidate_in_container` が候補発見まで `len` 件を
/// 無制限に反復し、各 index で `item()` を呼ぶ仮想化契約違反の是正）。
///
/// [`virtual_list_search_window`]（第 2 段・再捕捉用途）と基本方式は同じ「ヒント ±
/// マージン」だが、既定値・ヒント不採用の扱いが異なる: 再捕捉は「ヒントが無ければ
/// 先頭ページ」という契約（[`virtual_list_search_window`]参照）で問題ないのに対し、
/// 方向移動は常に `start_index`（現在フォーカス中の index）を含む窓でなければ
/// 実用にならない。
///
/// `step_magnitude` は呼び出し元（[`find_candidate_in_container`]）が
/// `axis_step` の絶対値（1 ステップで進む index 差分）を渡す。`Axis::Grid` の
/// Up/Down は 1 行分の移動が `columns` 単位で index をまたぐため、item 数固定の
/// マージンでは `columns` が大きいと最初の 1 行移動でも窓外に出て失敗する
/// （Cursor Bugbot High 指摘）。マージンを「item 数」ではなく「ステップ数（既定
/// [`VIRTUAL_LIST_SEARCH_MARGIN`]）× ステップ幅（`step_magnitude`）」で決めることで、
/// Grid でも数ステップ分の余裕を確保しつつ、`step_magnitude` は `columns`（有限）
/// で頭打ちのため有界性は維持する。
fn direction_move_window(
    container_path: &[usize],
    start_index: usize,
    total: usize,
    window_hints: &WindowHints,
    step_magnitude: usize,
) -> VisibleRange {
    let margin = VIRTUAL_LIST_SEARCH_MARGIN.saturating_mul(step_magnitude.max(1));
    if let Some(hint) = window_hints.get(container_path) {
        let hint_start = hint.start.saturating_sub(margin);
        let hint_end = hint.end.saturating_add(margin);
        if start_index >= hint_start && start_index <= hint_end {
            let clamped_end = hint_end.min(total);
            let clamped_start = hint_start.min(clamped_end);
            return clamped_start..clamped_end;
        }
    }
    let start = start_index.saturating_sub(margin);
    let end = start_index.saturating_add(margin).saturating_add(1);
    let clamped_end = end.min(total);
    let clamped_start = start.min(clamped_end);
    clamped_start..clamped_end
}

/// [`find_candidate_in_container`] の探索結果（Cursor Bugbot High 指摘対応:
/// 「探索窓の境界で打ち切った（窓の先にまだ候補がいる可能性が残る）」ケースと
/// 「コンテナの端まで走査したが候補が存在しなかった」ケースを区別する）。
///
/// 前者（[`Self::WindowBounded`]）を後者と同一視して祖先コンテナへエスケープすると、
/// まだ項目が残っているのに隣の兄弟コンテナへフォーカスが飛んでしまう
/// （「窓外は候補なし = フォーカス維持」という契約と矛盾する）。
/// 呼び出し元（[`FocusRing::move_focus`]）は [`Self::WindowBounded`] を受け取った
/// 場合、祖先コンテナへ遡らず `move_focus` 全体を失敗（フォーカス維持）で終える。
///
/// 併せて「窓の先のどこまで調べ終えたか」（`next_index`）を返す。9 巡目
/// codex-review 指摘への対応として、`FocusRing::search_cursor`（[`SearchCursor`]）
/// がこれを次回探索の再開位置として引き継ぎ、段階的に前進できるようにする
/// （フォーカス自体は動かないため reveal に頼らない設計、`SearchCursor`
/// ドキュメント参照）。
enum ContainerSearch {
    /// フォーカス可能な候補が見つかった。
    Found(FocusEntry),
    /// コンテナの端（index 範囲外・`Grid` の行境界）まで走査したが候補がなかった。
    /// 祖先コンテナへのエスケープ対象（従来どおりの挙動）。
    Exhausted,
    /// `VirtualList` の探索窓の外に達したため打ち切った。窓の先にまだ候補が
    /// いる可能性が残るため、エスケープしてはならない。`next_index` は未検証の
    /// まま打ち切った最初の index（次回の再開位置）。
    WindowBounded { next_index: usize },
}

/// 自コンテナ内（`container_prefix`）で `direction` 方向の候補を探す（設計文書 §4.4
/// 「自コンテナ内で解決する場合」）。`start_index`（現在フォーカス中の index。行境界
/// 判定・ヒント不採用時のフォールバック窓の基準として使う）から `axis_step` の
/// 差分ずつ進めながら、フォーカス可能ノード／フォーカス可能な子孫を持つコンテナが
/// 見つかるまで非フォーカス可能ノードをスキップする。境界（範囲外・`Grid` の
/// 行境界）に達すると [`ContainerSearch::Exhausted`]（祖先コンテナへのエスケープ、
/// 呼び出し元が処理）。
///
/// `resume_index` が `Some` の場合、実際に列挙を始める起点は `start_index` ではなく
/// こちらを使う（[`SearchCursor`] による段階探索の再開、codex-review 9 巡目 P1
/// 対応）。`start_index` を起点に固定したままだと、フォーカスが動かない
/// `WindowBounded` の性質上、毎回まったく同じ探索窓が再構成されて前進しない
/// （`SearchCursor` ドキュメント参照）。行境界判定（`row_constrained`）だけは
/// 「現在フォーカス中の行」を指す `start_index` を基準にし続ける（再開位置は
/// 同じ行の内側にしか進まないため矛盾しない）。
///
/// `container_prefix` が `VirtualList` を指す場合のみ、[`direction_move_window`]
/// が求める窓の外への index 到達を [`ContainerSearch::WindowBounded`] として打ち切る
/// （codex-review P1 対応・566bc97）。`List`（非仮想化）・トップレベル（`Screen`）は
/// 仮想化契約の対象外のため有界化しない（設計文書 §2、モジュール内の他の探索関数と
/// 同じ扱い）。
#[allow(clippy::too_many_arguments)]
fn find_candidate_in_container<S>(
    screen: &Screen<S>,
    state: &S,
    container_prefix: &[usize],
    axis: Axis,
    direction: FocusDirection,
    start_index: usize,
    resume_index: Option<usize>,
    len: usize,
    window_hints: &WindowHints,
    cache: &mut ObservedCache,
) -> ContainerSearch {
    let Some(step) = axis_step(axis, direction) else {
        // 呼び出し元は axis_step が Some の場合のみこの関数を呼ぶため通常到達しない。
        // 安全側で「候補なし・エスケープ可」として扱う。
        return ContainerSearch::Exhausted;
    };
    let columns = match axis {
        Axis::Grid { columns } => columns.max(1),
        _ => 0,
    };
    let row_constrained = matches!(axis, Axis::Grid { .. })
        && matches!(direction, FocusDirection::Left | FocusDirection::Right);

    // 段階探索の再開位置（`resume_index`）があればそれを、なければ現在フォーカス
    // 中の index（`start_index`）を、これから列挙を始める基準にする（関数
    // ドキュメント参照）。窓（`window_bound`）もこの基準を中心に組み直すことで、
    // 前回打ち切った続きへ探索窓ごと前進させる。
    let search_anchor = resume_index.unwrap_or(start_index);

    // `container_prefix` が `VirtualList` を指す場合のみ探索窓を求める（`List`・
    // トップレベルは有界化しない、関数ドキュメント参照）。この解決は 1 回だけ
    // （ループの外）行い、ループ内で毎回 `resolve_node_at` を呼ぶコストは払わない。
    //
    // #537: あえて [`container_kind`] は使わない。ここは「VirtualList か否か」の
    // 判別だけが目的で、`len`/`item` の値は不要（`len` は引数で既に渡っている）。
    // `container_kind` を経由すると `List` の場合に `expand(state)` を評価してしまい
    // （`ContainerKind::Slice` 構築のため）、従来この分岐が `List` に対して一切
    // `expand` を呼んでいなかった評価回数を変えてしまう。よって軽量な `matches!`
    // 判別のみにとどめる。
    let window_bound = if container_prefix.is_empty() {
        None
    } else {
        match resolve_node_at(screen, container_prefix, state) {
            Some(Node::VirtualList { .. }) => Some(direction_move_window(
                container_prefix,
                search_anchor,
                len,
                window_hints,
                step.unsigned_abs(),
            )),
            _ => None,
        }
    };

    // 列挙の開始位置: `resume_index` があればそれ自体を最初に検証する（ループ先頭の
    // `idx += step` を打ち消す形で 1 ステップ手前から始める）。`resume_index` が
    // `None`（段階探索の再開でない通常の移動）の場合は従来どおり `start_index`
    // （現在フォーカス中の index）そのものは検証済み扱いとし、そこから 1 歩進めた
    // 位置を最初に検証する。
    let mut idx = match resume_index {
        Some(resume) => resume as isize - step,
        None => start_index as isize,
    };
    loop {
        idx += step;
        if idx < 0 || idx as usize >= len {
            // 範囲外: 自コンテナ内に候補なし（境界到達、エスケープ対象）。
            return ContainerSearch::Exhausted;
        }
        let idx_usize = idx as usize;
        if row_constrained && idx_usize / columns != start_index / columns {
            // 行番号が変わる（行末での Right・行頭での Left）: 隣の行へ回り込まず
            // 行内に候補なしとして扱う（設計文書 §4.4「Axis::Grid の解決」）。
            return ContainerSearch::Exhausted;
        }
        if let Some(window) = &window_bound {
            if idx_usize < window.start || idx_usize >= window.end {
                // 探索窓の外: 「まだ先に候補がいるかもしれない」ため、コンテナの
                // 端到達（Exhausted）とは区別し WindowBounded を返す
                // （Cursor Bugbot High 指摘対応）。未検証のこの index を次回の
                // 再開位置として呼び出し元（`FocusRing::search_cursor`）へ渡す
                // （codex-review 9 巡目 P1 対応、`SearchCursor` ドキュメント参照）。
                return ContainerSearch::WindowBounded {
                    next_index: idx_usize,
                };
            }
        }

        let mut path = container_prefix.to_vec();
        path.push(idx_usize);
        let Some(child) = container_child(screen, state, container_prefix, idx_usize) else {
            // 構造不整合（想定した index に子が存在しない）: 安全側でエスケープ可とする。
            return ContainerSearch::Exhausted;
        };

        if let Some(id) = focusable_id(&child) {
            cache.record(id, &path);
            return ContainerSearch::Found(FocusEntry { path, id });
        }

        if let Some(initial) = container_frame(&child, path.clone(), state) {
            // ネストしたコンテナ（棚の中の棚等）: ツリー順で辿った最初のフォーカス
            // 可能ノードへ descend する（設計文書 §4.4 の 3.）。反復 DFS
            // （[`traverse_focusable`]）を使う（codex-review P1 対応、詳細は
            // [`FocusStackFrame`] のコメント参照）。この descend 自体は
            // `FALLBACK_SCAN_LIMIT` で有界化された別窓（先頭スキャン）を使うため
            // ここでの WindowBounded 契約とは独立の関心事である。
            if let Some(entry) = traverse_focusable(
                initial,
                state,
                cache,
                |_path, total| 0..total.min(FALLBACK_SCAN_LIMIT),
                |_id| true,
            ) {
                return ContainerSearch::Found(entry);
            }
            // このコンテナに候補がなければ同方向へスキップを継続する。
        }
        // `Text` 等の非フォーカス可能ノードは黙ってスキップを続ける。
    }
}

/// [`traverse_focusable`]（反復 DFS）が使う探索スタックの 1 段（設計文書 §4.4 の
/// 「最初の一致で早期終了する走査」を、再帰ではなくヒープ上の `Vec` スタックで
/// 表現する）。
///
/// codex-review P1 指摘（旧 `first_focusable_in_node` / `find_by_id_in_node` が
/// `List`/`VirtualList` のネストごとに自身を再帰し、ネスト深度に上限がないため
/// 深いツリーでネイティブコールスタックを枯渇させうる。`event::resolve_node_at` が
/// A05 対策として反復処理を採用した方針と不整合）への対応。`resolve_node_at` と
/// 同じ「反復ループで降下し、コールスタックを深さに応じて伸ばさない」方針を
/// コンテナ探索側にも適用したもの。
///
/// あえて深度上限の定数は設けない: `resolve_node_at` 自身も深度上限を持たず
/// （`path` の長さは呼び出し元が構築する値であり、降下ロジックはそれに追随する
/// だけ）、本エンジンも同じ考え方を踏襲する。ネイティブコールスタック（既定 8MB
/// 程度、段あたり数十〜数百バイト消費）に対し、本スタックはヒープ上の `Vec` で
/// ネスト 1 段あたり高々 `path: Vec<usize>` 1 個分（数十バイト）を消費するのみで、
/// 段数に応じてコールスタックが伸びる経路そのものが存在しない。1 入力あたりの
/// 処理量（仮想化契約）は本スタックの深度ではなく `item()` 呼び出し回数で管理し
/// ており（[`FALLBACK_SCAN_LIMIT`]・[`VIRTUAL_LIST_SEARCH_MARGIN`]・第 2 段の
/// 探索窓）、この 2 つの有界化は独立した関心事である。
enum FocusStackFrame<S> {
    /// `Screen` トップレベル、または `List::expand` が返した子ノード列を走査中の段。
    Slice {
        path: Vec<usize>,
        children: Vec<Node<S>>,
        next: usize,
    },
    /// `VirtualList` の窓内 index を走査中の段。`item()` は index を訪れる直前に
    /// 1 回だけ遅延評価する（全展開しない、仮想化契約を維持する）。
    Virtual {
        path: Vec<usize>,
        item: crate::state::ReadIndexedFn<S, Node<S>>,
        next: usize,
        bound: usize,
    },
}

/// フォーカス可能ノード探索の共通エンジン（反復 DFS、[`FocusStackFrame`] 参照）。
/// [`find_first_focusable`]・[`find_by_id`]・[`find_candidate_in_container`] の
/// ネスト descend が共有する（旧 `first_focusable_in_node` / `find_by_id_in_node`
/// の再帰実装を統合、codex-review P1 対応）。
///
/// `initial` は開始時点のスタック（`Screen` トップレベル、または既知のコンテナ
/// ノードの子）。`virtual_window` は `VirtualList` に遭遇するたびに走査する index
/// 範囲を決める（先頭フォールバック用途は `0..len.min(上限)`、第 2 段は探索窓
/// ヒント）。`is_match` が `true` を返した最初のフォーカス可能ノードで打ち切る
/// （最初の一致で早期終了する走査、既存の走査規則を維持）。評価したすべての
/// フォーカス可能ノードは `is_match` の結果によらず観測キャッシュへ記録する
/// （維持コストゼロの相乗り、§4.4「観測キャッシュ」）。
fn traverse_focusable<S>(
    initial: FocusStackFrame<S>,
    state: &S,
    cache: &mut ObservedCache,
    mut virtual_window: impl FnMut(&[usize], usize) -> VisibleRange,
    mut is_match: impl FnMut(WidgetId) -> bool,
) -> Option<FocusEntry> {
    let mut stack = vec![initial];

    while let Some(last) = stack.len().checked_sub(1) {
        let step = match &mut stack[last] {
            FocusStackFrame::Slice {
                path,
                children,
                next,
            } => {
                if *next >= children.len() {
                    None
                } else {
                    let idx = *next;
                    *next += 1;
                    let mut child_path = path.clone();
                    child_path.push(idx);
                    Some((child_path, children[idx].clone()))
                }
            }
            FocusStackFrame::Virtual {
                path,
                item,
                next,
                bound,
            } => {
                if *next >= *bound {
                    None
                } else {
                    let idx = *next;
                    *next += 1;
                    let mut child_path = path.clone();
                    child_path.push(idx);
                    Some((child_path, item(state, idx)))
                }
            }
        };

        let Some((child_path, child)) = step else {
            // この段は使い切った: 祖先段（親コンテナの続き）へ戻る（DFS のバックトラック）。
            stack.pop();
            continue;
        };

        if let Some(id) = focusable_id(&child) {
            cache.record(id, &child_path);
            if is_match(id) {
                return Some(FocusEntry {
                    path: child_path,
                    id,
                });
            }
        }

        if let Some(frame) =
            container_frame_with_window(&child, child_path, state, &mut virtual_window)
        {
            stack.push(frame);
        }
    }

    None
}

/// `node` が `List`/`VirtualList` であれば、その子を走査する [`FocusStackFrame`] を
/// 組み立てる（`node` 自身が非コンテナなら `None`）。`path` は `node` 自身の path。
/// `VirtualList` の窓は `virtual_window` に委譲する（[`traverse_focusable`] 用）。
fn container_frame_with_window<S>(
    node: &Node<S>,
    path: Vec<usize>,
    state: &S,
    virtual_window: &mut impl FnMut(&[usize], usize) -> VisibleRange,
) -> Option<FocusStackFrame<S>> {
    match container_kind(node, state)? {
        ContainerKind::Slice { children, .. } => Some(FocusStackFrame::Slice {
            path,
            children,
            next: 0,
        }),
        ContainerKind::Virtual { len, item, .. } => {
            let range = virtual_window(&path, len);
            Some(FocusStackFrame::Virtual {
                path,
                item,
                next: range.start,
                bound: range.end,
            })
        }
    }
}

/// [`find_candidate_in_container`] のネスト descend 専用の入口。`node`（`List` または
/// `VirtualList` であることが呼び出し元で確定済み）の子を走査する
/// [`FocusStackFrame`] を、先頭 [`FALLBACK_SCAN_LIMIT`] 件のみを窓とする規則
/// （[`find_first_focusable`] と同一。スクロール位置に依存しない決定的な先頭
/// スキャン、設計文書 §4.4「先頭フォールバック」）で組み立てる。
fn container_frame<S>(node: &Node<S>, path: Vec<usize>, state: &S) -> Option<FocusStackFrame<S>> {
    let mut window = |_path: &[usize], total: usize| 0..total.min(FALLBACK_SCAN_LIMIT);
    container_frame_with_window(node, path, state, &mut window)
}

/// ツリー先頭（`Screen` の挿入順）から最初のフォーカス可能ノードを探す
/// （[`traverse_focusable`] の入口。初回 `rebuild` と第 3 段フォールバックが
/// 共有する走査、設計文書 §4.4）。
///
/// `VirtualList` は先頭 [`FALLBACK_SCAN_LIMIT`] 件のみへ `item()` を呼ぶ有界化を行う
/// （codex-review P1 対応: フォーカス可能項目が存在しない・末尾にしかない大規模リストで
/// `0..len(state)` を無制限走査し仮想化契約を破ることの是正）。この走査は探索窓ヒントを
/// 用いない（設計文書 §4.4「先頭フォールバック」の 11・12 巡目改訂が確定した「着地点を
/// スクロール位置に依存させない決定的な先頭スキャン」という契約を維持するため、
/// [`VIRTUAL_LIST_SEARCH_MARGIN`]・`WindowHints` は第 2 段〔`find_by_id`〕専用のまま
/// とする）。上限内に候補がなければ `None`（フォーカスなし、`move_focus`/`decide` は
/// no-op になるだけで安全）。`List`（非仮想化）は従来どおり全展開する（仮想化契約の
/// 対象外、設計文書 §2）。
fn find_first_focusable<S>(
    screen: &Screen<S>,
    state: &S,
    cache: &mut ObservedCache,
) -> Option<FocusEntry> {
    let initial = FocusStackFrame::Slice {
        path: Vec::new(),
        children: screen.iter().cloned().collect(),
        next: 0,
    };
    traverse_focusable(
        initial,
        state,
        cache,
        |_path, total| 0..total.min(FALLBACK_SCAN_LIMIT),
        |_id| true,
    )
}

/// `VirtualList` コンテナ（`container_path` が指す）の探索窓を求める（設計文書 §4.4
/// 「探索窓ヒント」）。ヒントが存在すれば `hint.start.saturating_sub(K)..(hint.end + K)`
/// （`total` でクランプ）、存在しなければ先頭 `K` 件を既定窓とする。
fn virtual_list_search_window(
    container_path: &[usize],
    total: usize,
    window_hints: &WindowHints,
) -> VisibleRange {
    let base = window_hints.get(container_path).cloned();
    let range = match base {
        Some(hint) => {
            let start = hint.start.saturating_sub(VIRTUAL_LIST_SEARCH_MARGIN);
            let end = hint.end.saturating_add(VIRTUAL_LIST_SEARCH_MARGIN);
            start..end
        }
        None => 0..VIRTUAL_LIST_SEARCH_MARGIN,
    };
    let clamped_end = range.end.min(total);
    let clamped_start = range.start.min(clamped_end);
    clamped_start..clamped_end
}

/// 第 2 段（有界全域探索）: `target` と同じ `id` を持つ葉ノードを探す（設計文書
/// §4.4「id 不一致検知時の三段探索」、[`traverse_focusable`] の入口）。`List` は
/// 全展開（仮想化契約の対象外、§2）、`VirtualList` は [`virtual_list_search_window`]
/// の窓内のみへ `item()` を呼ぶ有界化を行う。評価したすべての `Button`/
/// `IndexedButton` を観測キャッシュへ記録する。
fn find_by_id<S>(
    screen: &Screen<S>,
    state: &S,
    target: WidgetId,
    window_hints: &WindowHints,
    cache: &mut ObservedCache,
) -> Option<FocusEntry> {
    let initial = FocusStackFrame::Slice {
        path: Vec::new(),
        children: screen.iter().cloned().collect(),
        next: 0,
    };
    traverse_focusable(
        initial,
        state,
        cache,
        |path, total| virtual_list_search_window(path, total, window_hints),
        |id| id == target,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WidgetId;

    // ------------------------------------------------------------------
    // 軸解決アルゴリズムのテスト（設計文書 §4.4「机上検証」のツリー例を再現）。
    // ------------------------------------------------------------------

    /// テスト専用の状態型。軸解決テストでは状態非依存の固定ツリーのみを使うため
    /// フィールドは持たない。
    struct AxisTreeState;

    fn axis_no_op(_state: &mut AxisTreeState) {}

    /// Shelf1 = L1（Axis::Horizontal）: ButtonA(id1) ButtonB(id2) ButtonC(id3)。
    fn shelf1_expand(_state: &AxisTreeState) -> Vec<Node<AxisTreeState>> {
        vec![
            Node::Button {
                label: "A".to_string(),
                on_click: axis_no_op,
                id: WidgetId::from_raw(1),
            },
            Node::Button {
                label: "B".to_string(),
                on_click: axis_no_op,
                id: WidgetId::from_raw(2),
            },
            Node::Button {
                label: "C".to_string(),
                on_click: axis_no_op,
                id: WidgetId::from_raw(3),
            },
        ]
    }

    /// Shelf2 = L2（Axis::Horizontal）: ButtonD(id4) ButtonE(id5)。
    fn shelf2_expand(_state: &AxisTreeState) -> Vec<Node<AxisTreeState>> {
        vec![
            Node::Button {
                label: "D".to_string(),
                on_click: axis_no_op,
                id: WidgetId::from_raw(4),
            },
            Node::Button {
                label: "E".to_string(),
                on_click: axis_no_op,
                id: WidgetId::from_raw(5),
            },
        ]
    }

    /// Grid = L3（Axis::Grid { columns: 3 }、len=7）: 行 0=[0,1,2] 行 1=[3,4,5] 行 2=[6]。
    fn grid_len(_state: &AxisTreeState) -> usize {
        7
    }

    fn grid_no_op(_state: &mut AxisTreeState, _index: usize) {}

    fn grid_item(_state: &AxisTreeState, index: usize) -> Node<AxisTreeState> {
        Node::IndexedButton {
            label: format!("G{index}"),
            index,
            on_click: grid_no_op,
            id: WidgetId::from_raw(10 + index as u64),
        }
    }

    /// L0（Axis::Vertical、トップレベル唯一のノード）: Shelf1, Shelf2, Grid。
    fn l0_expand(_state: &AxisTreeState) -> Vec<Node<AxisTreeState>> {
        vec![
            Node::List {
                expand: shelf1_expand,
                axis: Axis::Horizontal,
            },
            Node::List {
                expand: shelf2_expand,
                axis: Axis::Horizontal,
            },
            Node::VirtualList {
                len: grid_len,
                item: grid_item,
                item_extent: 40.0,
                axis: Axis::Grid { columns: 3 },
            },
        ]
    }

    fn axis_tree() -> Screen<AxisTreeState> {
        Screen::new("axis-tree").list_with_axis(l0_expand, Axis::Vertical)
    }

    fn ring_at(path: &[usize], id: u64) -> FocusRing {
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: path.to_vec(),
            id: WidgetId::from_raw(id),
        });
        ring
    }

    /// 机上検証 #1: ButtonA [0,0,0] Right → ButtonB [0,0,1]（棚内の隣接移動）。
    #[test]
    fn move_focus_within_horizontal_shelf() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 0], 1);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Right);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 0, 1]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(2));
    }

    /// 机上検証 #2: ButtonC [0,0,2] Right → no-op（棚境界、L0 は Right 非対応）。
    #[test]
    fn move_focus_stops_at_shelf_boundary_right() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 2], 3);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Right);

        assert!(!moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 0, 2]);
    }

    /// 机上検証 #3: ButtonA [0,0,0] Down → ButtonD [0,1,0]（棚から出て次の棚の
    /// 先頭へ。Cursor Bugbot 指摘「棚内で Up/Down と Left/Right が重複する」の解消）。
    #[test]
    fn move_focus_escapes_shelf_to_next_shelf_on_down() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 0], 1);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 1, 0]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(4));
    }

    /// 机上検証 #5: Grid idx2（行0列2） Right → no-op（行境界、隣の行へ回り込まない）。
    #[test]
    fn move_focus_grid_right_stops_at_row_boundary() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 2, 2], 12);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Right);

        assert!(!moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 2, 2]);
    }

    /// 机上検証 #6: Grid idx4（行1列1） Right → Grid idx5（同一行内）。
    #[test]
    fn move_focus_grid_right_within_row() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 2, 4], 14);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Right);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 2, 5]);
    }

    /// 机上検証 #7: Grid idx6（行2列0、最終行） Up → Grid idx3（行間ジャンプ、
    /// Grid 最終行が欠けている構成でも `index - columns` の範囲内であれば移動できる）。
    #[test]
    fn move_focus_grid_up_jumps_row_with_incomplete_last_row() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 2, 6], 16);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Up);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 2, 3]);
    }

    /// 机上検証 #8: Grid idx1（行0列1、最上行） Up → ButtonD [0,1,0]（grid 最上行から
    /// 棚へ戻る。Cursor Bugbot High 指摘「Grid becomes a focus trap」の解消）。
    #[test]
    fn move_focus_grid_escapes_to_shelf_on_up_from_top_row() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 2, 1], 11);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Up);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 1, 0]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(4));
    }

    /// 机上検証 #9: Grid idx4 Down → no-op（Grid の最終行より下はなく、L0 も
    /// Grid が最後の子のためエスケープ先がない。トップレベルまで遡っても
    /// 候補が無い場合の no-op 確定）。
    #[test]
    fn move_focus_grid_down_no_candidate_below_last_row() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 2, 4], 14);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(!moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 2, 4]);
    }

    /// 机上検証 #10: ButtonA [0,0,0] Up → no-op（先頭シェルフより上はない、複数段の
    /// エスケープを経てトップレベルで no-op 確定）。
    #[test]
    fn move_focus_up_from_first_shelf_is_no_op() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 0], 1);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Up);

        assert!(!moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 0, 0]);
    }

    /// 机上検証 #11: ButtonA [0,0,0] Left → no-op（棚境界、L0 も Left 非対応）。
    #[test]
    fn move_focus_left_from_first_button_is_no_op() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 0], 1);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Left);

        assert!(!moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 0, 0]);
    }

    /// `current` が `None` の場合、`move_focus` は何もせず `false` を返す。
    #[test]
    fn move_focus_is_no_op_when_current_is_none() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = FocusRing::new();

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(!moved);
        assert!(ring.current().is_none());
    }

    // ------------------------------------------------------------------
    // `rebuild` の初回解決・自己更新セマンティクスのテスト。
    // ------------------------------------------------------------------

    /// 初回 `rebuild`（`current == None`）はツリー先頭の最初のフォーカス可能ノード
    /// （ButtonA）を採用する。
    #[test]
    fn rebuild_initial_resolution_picks_first_focusable_node() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = FocusRing::new();

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert_eq!(ring.current().unwrap().path, vec![0, 0, 0]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(1));
    }

    /// 構造変化がない場合、`rebuild` を複数回呼んでも `current` は変わらない
    /// （自己更新型 API が同一インスタンス上で `current` を引き継ぐことの確認）。
    #[test]
    fn rebuild_is_idempotent_without_structural_change() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 1, 0], 4);

        ring.rebuild(&screen, &state, &WindowHints::new());
        let after_first = ring.current().cloned().unwrap();
        ring.rebuild(&screen, &state, &WindowHints::new());
        let after_second = ring.current().cloned().unwrap();

        assert_eq!(after_first, after_second);
        assert_eq!(after_first.path, vec![0, 1, 0]);
    }

    // ------------------------------------------------------------------
    // 三段探索（id 不一致検知時の再捕捉）のテスト。動的な `List` を持つ状態を使う。
    // ------------------------------------------------------------------

    /// 挿入・削除・並べ替えが可能な `List` 1 本のみを持つテスト用状態。
    #[derive(Default)]
    struct DynamicListState {
        items: Vec<u64>,
    }

    fn dyn_no_op(_state: &mut DynamicListState) {}

    fn dyn_expand(state: &DynamicListState) -> Vec<Node<DynamicListState>> {
        state
            .items
            .iter()
            .map(|&id| Node::Button {
                label: format!("item-{id}"),
                on_click: dyn_no_op,
                id: WidgetId::from_raw(id),
            })
            .collect()
    }

    fn dyn_tree() -> Screen<DynamicListState> {
        Screen::new("dyn-tree").list_with_axis(dyn_expand, Axis::Vertical)
    }

    /// 並べ替え（id=1 が index 0 → index 1 へ移動）後の `rebuild` が、第 2 段
    /// （有界全域探索、`List` は全展開）で同一 `WidgetId` を再捕捉できることを検証する
    /// （設計文書 §4.4「机上検証（コンテナ並べ替え時の rebuild 再捕捉）」相当）。
    #[test]
    fn rebuild_recovers_via_full_search_after_reorder() {
        let mut state = DynamicListState {
            items: vec![1, 2, 3],
        };
        let screen = dyn_tree();
        let mut ring = FocusRing::new();
        ring.rebuild(&screen, &state, &WindowHints::new());
        assert_eq!(ring.current().unwrap().path, vec![0, 0]);

        // id=1 が index 0 → index 1 へ移動する（並べ替え）。
        state.items = vec![3, 1, 2];

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(1));
        assert_eq!(ring.current().unwrap().path, vec![0, 1]);
    }

    /// 観測キャッシュに正しい記録が残っている場合、`rebuild` は第 1 段（キャッシュ
    /// 照会）だけで復旧できることを検証する（第 2 段の全域探索を経由しなくても
    /// 解決できる、キャッシュヒットの確認）。
    #[test]
    fn rebuild_recovers_via_observed_cache_hit() {
        let state = DynamicListState {
            items: vec![1, 2, 3],
        };
        let screen = dyn_tree();
        let mut ring = FocusRing::new();
        // 通常の解決で id=2 の観測記録（path=[0,1]）をキャッシュへ積む。
        ring.rebuild(&screen, &state, &WindowHints::new());
        ring.move_focus(&screen, &state, FocusDirection::Down);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(2));

        // `current.path` だけを意図的に誤らせる（キャッシュ側は正しい記録を保持）。
        ring.current = Some(FocusEntry {
            path: vec![99],
            id: WidgetId::from_raw(2),
        });

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(2));
        assert_eq!(ring.current().unwrap().path, vec![0, 1]);
    }

    /// 観測キャッシュの記録が古くなっている（stale）場合、`rebuild` は
    /// verify-on-use で棄却し第 2 段（全域探索）へフォールバックすることを検証する。
    #[test]
    fn rebuild_discards_stale_cache_and_falls_back_to_full_search() {
        let mut state = DynamicListState {
            items: vec![1, 2, 3],
        };
        let screen = dyn_tree();
        let mut ring = FocusRing::new();
        ring.rebuild(&screen, &state, &WindowHints::new());
        ring.move_focus(&screen, &state, FocusDirection::Down);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(2));
        // この時点でキャッシュには id=2 → path=[0,1] が記録されている。

        // id=2 を削除し、id=4 を挿入する（キャッシュの path=[0,1] は id=4 を指す
        // ようになり stale になる）。
        state.items = vec![1, 4, 3];
        ring.current = Some(FocusEntry {
            path: vec![0, 1],
            id: WidgetId::from_raw(2),
        });

        ring.rebuild(&screen, &state, &WindowHints::new());

        // id=2 はもうツリーに存在しないため、第 3 段フォールバックでツリー先頭
        // （id=1）へ着地する。
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(1));
        assert_eq!(ring.current().unwrap().path, vec![0, 0]);
    }

    /// 項目自体が削除され、ツリー全体を走査しても見つからない場合は第 3 段
    /// （先頭フォールバック）が働くことを検証する。
    #[test]
    fn rebuild_falls_back_to_first_focusable_when_item_deleted() {
        let mut state = DynamicListState {
            items: vec![1, 2, 3],
        };
        let screen = dyn_tree();
        let mut ring = ring_from_dyn(&[0, 1], 2);
        ring.rebuild(&screen, &state, &WindowHints::new());
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(2));

        // id=2 を削除する（もうどこにも存在しない）。
        state.items = vec![1, 3];

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(1));
        assert_eq!(ring.current().unwrap().path, vec![0, 0]);
    }

    fn ring_from_dyn(path: &[usize], id: u64) -> FocusRing {
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: path.to_vec(),
            id: WidgetId::from_raw(id),
        });
        ring
    }

    // ------------------------------------------------------------------
    // `VirtualList` の探索窓（第 2 段）・item() 呼び出し回数のテスト。
    // ------------------------------------------------------------------

    /// index に応じて `id` が変化しうる（`offset` フィールドで擬似的に「項目の
    /// 論理位置がずれた」状態を作る）`VirtualList` 単体のテスト用状態。
    #[derive(Default)]
    struct VirtualState {
        offset: i64,
        item_call_count: std::cell::Cell<usize>,
    }

    fn v_len(_state: &VirtualState) -> usize {
        100
    }

    fn v_no_op(_state: &mut VirtualState, _index: usize) {}

    fn v_item(state: &VirtualState, index: usize) -> Node<VirtualState> {
        state.item_call_count.set(state.item_call_count.get() + 1);
        let id = (index as i64 + state.offset) as u64;
        Node::IndexedButton {
            label: format!("v{index}"),
            index,
            on_click: v_no_op,
            id: WidgetId::from_raw(id),
        }
    }

    fn v_tree() -> Screen<VirtualState> {
        Screen::new("virtual-tree").virtual_list_with_axis(v_len, v_item, 40.0, Axis::Vertical)
    }

    /// 初回 `rebuild` は最初の 1 件（index=0）のみを評価して終了する
    /// （早期終了走査、`item()` 呼び出し回数が近傍に留まることの確認、設計文書
    /// 8 節 確認事項 8 の単体レベル検証）。
    #[test]
    fn rebuild_initial_resolution_calls_item_minimally() {
        let state = VirtualState::default();
        let screen = v_tree();
        let mut ring = FocusRing::new();

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert_eq!(ring.current().unwrap().path, vec![0, 0]);
        assert_eq!(
            state.item_call_count.get(),
            1,
            "初回解決は最初の一致で早期終了するため item() は 1 回のみ呼ばれるはず"
        );
    }

    /// 探索窓ヒント（± マージン K）の範囲内に移動先がある場合、第 2 段が
    /// `VirtualList` の該当項目を再捕捉できることを検証する。あわせて `item()`
    /// 呼び出し回数が全長（100 件）に比例しない（近傍に留まる）ことも確認する。
    #[test]
    fn rebuild_recovers_virtual_list_item_within_window_hint() {
        let mut state = VirtualState::default();
        let screen = v_tree();

        // id=10 の項目（index=10）へフォーカスを合わせる。
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![0, 10],
            id: WidgetId::from_raw(10),
        });

        // 論理位置が 5 件分ずれる（id(index) = index - 5）。id=10 は今度は
        // index=15 に存在する。
        state.offset = -5;
        state.item_call_count.set(0);

        let mut hints = WindowHints::new();
        // ヒントはずれる前の可視範囲（index 10..11）のみを示す。マージン K
        // （既定 8）を足すことで index=15 まで届く窓になる。
        hints.insert(vec![0], 10..11);

        ring.rebuild(&screen, &state, &hints);

        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(10));
        assert_eq!(ring.current().unwrap().path, vec![0, 15]);
        assert!(
            state.item_call_count.get() < 30,
            "item() 呼び出し回数は探索窓（近傍）に留まるはず（実測: {}）",
            state.item_call_count.get()
        );
    }

    /// 移動先が探索窓（ヒント ± K）の外に出た場合、第 2 段では再捕捉できず
    /// 第 3 段（先頭フォールバック）に落ちることを検証する（設計文書 §4.4
    /// 「再捕捉保証の層別化」: 窓外の移動は保証しない）。
    #[test]
    fn rebuild_falls_back_when_virtual_list_move_exceeds_window() {
        let mut state = VirtualState::default();
        let screen = v_tree();

        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![0, 10],
            id: WidgetId::from_raw(10),
        });

        // 50 件分ずれる（id=10 は index=60 に存在し、ヒント周辺の窓には入らない）。
        state.offset = -50;

        let mut hints = WindowHints::new();
        hints.insert(vec![0], 10..11);

        ring.rebuild(&screen, &state, &hints);

        // 先頭（index=0、id = 0 - 50 は u64 キャストで再解釈されるため、ここでは
        // 「id=10 ではない」ことのみを厳密に確認する。
        assert_eq!(ring.current().unwrap().path, vec![0, 0]);
        assert_ne!(ring.current().unwrap().id, WidgetId::from_raw(10));
    }

    // ------------------------------------------------------------------
    // `first_focusable_in_node` の有界化（codex-review P1 対応）のテスト。
    // ------------------------------------------------------------------

    /// 先頭 [`FALLBACK_SCAN_LIMIT`] 件を超えた位置にしかフォーカス可能項目が
    /// 存在しない大規模 `VirtualList` 単体のテスト用状態（`Text` を非フォーカス可能
    /// 項目として大量に並べ、`item_call_count` で `item()` 呼び出し回数を検証する）。
    #[derive(Default)]
    struct BeyondLimitState {
        item_call_count: std::cell::Cell<usize>,
    }

    /// リスト全長は `FALLBACK_SCAN_LIMIT` の 10 倍規模を想定した大規模リストとする。
    fn beyond_limit_len(_state: &BeyondLimitState) -> usize {
        FALLBACK_SCAN_LIMIT * 10
    }

    /// index が `FALLBACK_SCAN_LIMIT` 未満の間は非フォーカス可能な `Text`、それ以降に
    /// のみフォーカス可能な `IndexedButton` を置く（上限外にしか候補がない状況の再現）。
    fn beyond_limit_no_op(_state: &mut BeyondLimitState, _index: usize) {}

    fn beyond_limit_item(state: &BeyondLimitState, index: usize) -> Node<BeyondLimitState> {
        state.item_call_count.set(state.item_call_count.get() + 1);
        if index < FALLBACK_SCAN_LIMIT {
            Node::Text(format!("heading-{index}"))
        } else {
            Node::IndexedButton {
                label: format!("item-{index}"),
                index,
                on_click: beyond_limit_no_op,
                id: WidgetId::from_raw(index as u64),
            }
        }
    }

    fn beyond_limit_tree() -> Screen<BeyondLimitState> {
        Screen::new("beyond-limit-tree").virtual_list_with_axis(
            beyond_limit_len,
            beyond_limit_item,
            40.0,
            Axis::Vertical,
        )
    }

    /// 上限を超えた位置にしか候補がない場合、初回 `rebuild` は `current = None`
    /// のまま確定し（フォーカス可能ノード不在時の既存契約と同じ扱い）、`item()` は
    /// 上限件数のみ呼ばれてリスト全長（`FALLBACK_SCAN_LIMIT * 10`）には比例しない
    /// ことを検証する（有界化の主張そのものの確認）。
    #[test]
    fn first_focusable_stops_at_scan_limit_and_returns_none_beyond_it() {
        let state = BeyondLimitState::default();
        let screen = beyond_limit_tree();
        let mut ring = FocusRing::new();

        ring.rebuild(&screen, &state, &WindowHints::new());

        assert!(
            ring.current().is_none(),
            "上限外にしか候補がない場合はフォーカスなしで確定するはず"
        );
        assert_eq!(
            state.item_call_count.get(),
            FALLBACK_SCAN_LIMIT,
            "item() 呼び出し回数は走査上限に留まり、リスト全長には比例しないはず"
        );
    }

    /// `move_focus` が `false` を返す no-op であり、`current` が `None` のままで
    /// panic しないことも確認する（上限超過時の呼び出し元 API 安全性の確認）。
    #[test]
    fn move_focus_is_safe_no_op_when_no_focusable_found_within_limit() {
        let state = BeyondLimitState::default();
        let screen = beyond_limit_tree();
        let mut ring = FocusRing::new();
        ring.rebuild(&screen, &state, &WindowHints::new());
        assert!(ring.current().is_none());

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(!moved);
        assert!(ring.current().is_none());
    }

    // ------------------------------------------------------------------
    // `Axis::Grid { columns: usize::MAX }` の isize 変換安全性のテスト
    // （codex-review P1 対応: `columns as isize` の折り返し・panic の是正）。
    // ------------------------------------------------------------------

    /// `columns` に `usize::MAX` を渡しても `axis_step` が折り返し値
    /// （`usize::MAX as isize == -1`）を返さず `None`（安全側のエスケープ扱い）で
    /// あることを確認する（Up/Down の方向反転を防ぐ）。
    #[test]
    fn axis_step_grid_up_down_with_max_columns_does_not_wrap() {
        let axis = Axis::Grid {
            columns: usize::MAX,
        };

        assert_eq!(axis_step(axis, FocusDirection::Up), None);
        assert_eq!(axis_step(axis, FocusDirection::Down), None);
    }

    /// `axis_step` が `None` を返す組み合わせでは `find_candidate_in_container` が
    /// 呼ばれず、`Axis::Grid { columns: usize::MAX }` を含むツリーでも `move_focus`
    /// の呼び出しが panic せず no-op になることを結合レベルで確認する。
    #[test]
    fn move_focus_grid_with_max_columns_is_safe_no_op() {
        struct MaxColumnsState;
        fn no_op(_state: &mut MaxColumnsState, _index: usize) {}
        fn item(_state: &MaxColumnsState, index: usize) -> Node<MaxColumnsState> {
            Node::IndexedButton {
                label: format!("g{index}"),
                index,
                on_click: no_op,
                id: WidgetId::from_raw(index as u64),
            }
        }
        fn len(_state: &MaxColumnsState) -> usize {
            3
        }
        let screen = Screen::new("max-columns-grid").virtual_list_with_axis(
            len,
            item,
            40.0,
            Axis::Grid {
                columns: usize::MAX,
            },
        );
        let state = MaxColumnsState;
        let mut ring = ring_from_max_columns(&[0, 1], 1);

        let up = ring.move_focus(&screen, &state, FocusDirection::Up);
        let down = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(!up, "columns が isize で表現できない場合は安全に no-op");
        assert!(!down, "columns が isize で表現できない場合は安全に no-op");
        assert_eq!(ring.current().unwrap().path, vec![0, 1]);
    }

    fn ring_from_max_columns(path: &[usize], id: u64) -> FocusRing {
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: path.to_vec(),
            id: WidgetId::from_raw(id),
        });
        ring
    }

    // ------------------------------------------------------------------
    // codex-review 7 巡目 P1 対応: 方向移動の有界化（`find_candidate_in_container`）・
    // 反復 DFS への変更（`traverse_focusable`）のテスト。
    // ------------------------------------------------------------------

    /// 全長 100 万件・全項目非フォーカス可能な `VirtualList` 単体のテスト用状態
    /// （現在位置以降に候補が存在しない状況の再現、`item()` 呼び出し回数を検証する）。
    #[derive(Default)]
    struct HugeVirtualState {
        item_call_count: std::cell::Cell<usize>,
    }

    fn huge_len(_state: &HugeVirtualState) -> usize {
        1_000_000
    }

    fn huge_item(state: &HugeVirtualState, index: usize) -> Node<HugeVirtualState> {
        state.item_call_count.set(state.item_call_count.get() + 1);
        Node::Text(format!("row-{index}"))
    }

    fn huge_tree() -> Screen<HugeVirtualState> {
        Screen::new("huge-tree").virtual_list_with_axis(huge_len, huge_item, 40.0, Axis::Vertical)
    }

    /// 巨大 `VirtualList`（len=10^6）で現在位置以降に候補が存在しない場合、方向移動
    /// 1 回の `item()` 呼び出し回数が探索窓（既定マージン近傍）に留まり、リスト全長
    /// には比例しないことを検証する（codex-review 7 巡目 P1 指摘の主張そのものの確認）。
    #[test]
    fn move_focus_bounds_item_calls_in_huge_virtual_list_with_no_focusable_neighbor() {
        let state = HugeVirtualState::default();
        let screen = huge_tree();
        let mut ring = FocusRing::new();
        // 現在位置を明示的にセットする（`rebuild` を経由せず、`window_hints` が
        // 空のまま `move_focus` を呼ぶケース＝最悪ケースの検証）。
        ring.current = Some(FocusEntry {
            path: vec![0, 500_000],
            id: WidgetId::from_raw(1),
        });

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(
            !moved,
            "探索窓の外までは移動できず、移動失敗（フォーカス維持）として扱われるはず"
        );
        assert!(
            state.item_call_count.get() < 100,
            "item() 呼び出し回数は探索窓（既定マージン近傍）に留まり、\
             リスト全長（100 万件）には比例しないはず（実測: {}）",
            state.item_call_count.get()
        );
    }

    /// `rebuild` が受け取った `WindowHints` を `move_focus` が引き継ぐことを検証する
    /// （`FocusRing::window_hints` フィールド、有界化の窓を実際の可視範囲に追随させる）。
    #[test]
    fn move_focus_uses_window_hint_recorded_by_rebuild() {
        let state = HugeVirtualState::default();
        let screen = huge_tree();
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![0, 500_000],
            id: WidgetId::from_raw(1),
        });

        let mut hints = WindowHints::new();
        // 実際の可視範囲は現在位置からかけ離れている想定（スクロール直後等）。
        hints.insert(vec![0], 900_000..900_010);
        ring.rebuild(&screen, &state, &hints);
        // id 不一致（`current` の id=1 はどこにも存在しない）で `rebuild` は
        // 先頭フォールバックへ落ちるため、ここでは `rebuild` 呼び出しの副作用
        // （`window_hints` の記録）のみを利用し、`current` は改めて明示的に戻す。
        ring.current = Some(FocusEntry {
            path: vec![0, 500_000],
            id: WidgetId::from_raw(1),
        });
        state.item_call_count.set(0);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(
            !moved,
            "記録されたヒント窓（900_000..900_010 ± マージン）にも現在位置\
             （500_000）近傍にも候補は無いため移動失敗のはず"
        );
        assert!(
            state.item_call_count.get() < 100,
            "item() 呼び出し回数は窓（現在位置近傍とヒントの和集合 ± マージン）に\
             留まるはず（実測: {}）",
            state.item_call_count.get()
        );
    }

    /// 深さ 10^4 段の `List` ネスト（各段が子 1 件のみを持つ）を持つテスト用状態。
    /// `remaining`（`Cell`）を `expand` 呼び出しのたびに 1 ずつ減らし、0 になった
    /// 段で葉の `Button` を生成する（素の関数ポインタ 1 本でネスト構造を組み立てる
    /// ためのテスト専用の仕掛け。`docs/spec` の DSL 契約とは無関係）。
    #[derive(Default)]
    struct DeepNestState {
        remaining: std::cell::Cell<usize>,
    }

    fn deep_no_op(_state: &mut DeepNestState) {}

    fn deep_expand(state: &DeepNestState) -> Vec<Node<DeepNestState>> {
        let remaining = state.remaining.get();
        if remaining == 0 {
            vec![Node::Button {
                label: "leaf".to_string(),
                on_click: deep_no_op,
                id: WidgetId::from_raw(1),
            }]
        } else {
            state.remaining.set(remaining - 1);
            vec![Node::List {
                expand: deep_expand,
                axis: Axis::Vertical,
            }]
        }
    }

    const DEEP_NEST_DEPTH: usize = 10_000;

    fn deep_tree() -> Screen<DeepNestState> {
        Screen::new("deep-tree").list_with_axis(deep_expand, Axis::Vertical)
    }

    /// 深さ 10^4 段の `List` ネストに対し `rebuild`（内部で最初のフォーカス可能ノード
    /// 探索＝旧 `first_focusable_in_node` 相当）・`move_focus` を呼んでも、反復 DFS
    /// （[`traverse_focusable`]）を使うためネイティブコールスタックを消費せず、
    /// panic（スタックオーバーフロー）せず完了することを検証する（codex-review 7
    /// 巡目 P1 対応）。
    #[test]
    fn rebuild_and_move_focus_complete_without_stack_overflow_on_deep_nesting() {
        // `path` の深さは「トップレベル選択 1 段 + `expand` 呼び出し回数
        // （`remaining..=0` で `remaining + 1` 回）」。`DEEP_NEST_DEPTH` と
        // 一致させるため `remaining = DEEP_NEST_DEPTH - 2` から始める。
        let state = DeepNestState {
            remaining: std::cell::Cell::new(DEEP_NEST_DEPTH - 2),
        };
        let screen = deep_tree();
        let mut ring = FocusRing::new();

        ring.rebuild(&screen, &state, &WindowHints::new());

        let entry = ring.current().expect("最深部の Button が見つかるはず");
        assert_eq!(
            entry.path.len(),
            DEEP_NEST_DEPTH,
            "path の深さはネスト段数と一致するはず"
        );
        assert_eq!(entry.id, WidgetId::from_raw(1));

        // 兄弟が存在しない構造のため移動先はなく no-op になるが、ここでの主眼は
        // 「途中の祖先コンテナへのエスケープ探索が深いネストを反復で辿り切り、
        // panic せず完了すること」であり、戻り値は問わない。
        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);
        assert!(!moved);
    }

    // ------------------------------------------------------------------
    // Cursor Bugbot High 指摘 2 件（PR #539）の回帰テスト。
    // ------------------------------------------------------------------

    /// `columns` が探索マージン（既定 8）を超える `Axis::Grid` の `VirtualList` 単体の
    /// テスト用状態。全項目をフォーカス可能にし、`item()` 呼び出し回数を記録する。
    #[derive(Default)]
    struct WideGridState {
        item_call_count: std::cell::Cell<usize>,
    }

    const WIDE_GRID_COLUMNS: usize = 12;
    const WIDE_GRID_LEN: usize = WIDE_GRID_COLUMNS * 2;

    fn wide_grid_len(_state: &WideGridState) -> usize {
        WIDE_GRID_LEN
    }

    fn wide_grid_no_op(_state: &mut WideGridState, _index: usize) {}

    fn wide_grid_item(state: &WideGridState, index: usize) -> Node<WideGridState> {
        state.item_call_count.set(state.item_call_count.get() + 1);
        Node::IndexedButton {
            label: format!("g{index}"),
            index,
            on_click: wide_grid_no_op,
            id: WidgetId::from_raw(index as u64),
        }
    }

    fn wide_grid_tree() -> Screen<WideGridState> {
        Screen::new("wide-grid-tree").virtual_list_with_axis(
            wide_grid_len,
            wide_grid_item,
            40.0,
            Axis::Grid {
                columns: WIDE_GRID_COLUMNS,
            },
        )
    }

    /// Bugbot High 指摘 1: `columns`（12）が固定マージン（既定 8）を超える Grid で、
    /// 行 1 の先頭（index=12）から Up 1 回で行 0 の先頭（index=0）へ移動できることを
    /// 検証する。マージンを「item 数」ではなく「ステップ数 × ステップ幅
    /// （`columns`）」で決める（[`direction_move_window`]）修正がないと、固定
    /// マージン 8 の窓（`4..21` 相当）に index=0 が入らず失敗する。
    #[test]
    fn move_focus_grid_up_succeeds_when_columns_exceed_fixed_margin() {
        let state = WideGridState::default();
        let screen = wide_grid_tree();
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![0, WIDE_GRID_COLUMNS],
            id: WidgetId::from_raw(WIDE_GRID_COLUMNS as u64),
        });

        let moved = ring.move_focus(&screen, &state, FocusDirection::Up);

        assert!(
            moved,
            "columns > 既定マージンでも 1 行上への移動は成功するはず"
        );
        assert_eq!(ring.current().unwrap().path, vec![0, 0]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(0));
    }

    /// Bugbot High 指摘 2: 探索窓の境界で打ち切った（`ContainerSearch::WindowBounded`）
    /// 場合、コンテナの端まで走査して候補がなかった場合（`Exhausted`）と異なり
    /// 祖先コンテナへエスケープしないことを検証する。
    ///
    /// トップレベルに [ShelfA（先頭）, 巨大 VirtualList（全項目非フォーカス可能・
    /// 現在位置は窓外にしか候補がない）, ShelfB（末尾）] を配置する。現在位置を
    /// 巨大リストの中央付近に置いて Down すると、窓境界で打ち切られる
    /// （`WindowBounded`）。修正前はこれを `Exhausted` と同一視して祖先（トップ
    /// レベル）へエスケープし、次の兄弟である ShelfB の先頭ボタンへ飛んでしまう。
    /// 修正後は `move_focus` が `false` を返し、`current` は巨大リスト内の元の
    /// 位置のまま維持される。
    #[derive(Default)]
    struct WindowEscapeState;

    fn window_escape_no_op(_state: &mut WindowEscapeState) {}

    fn window_escape_shelf_a(_state: &WindowEscapeState) -> Vec<Node<WindowEscapeState>> {
        vec![Node::Button {
            label: "A".to_string(),
            on_click: window_escape_no_op,
            id: WidgetId::from_raw(1),
        }]
    }

    fn window_escape_shelf_b(_state: &WindowEscapeState) -> Vec<Node<WindowEscapeState>> {
        vec![Node::Button {
            label: "B".to_string(),
            on_click: window_escape_no_op,
            id: WidgetId::from_raw(2),
        }]
    }

    fn window_escape_middle_len(_state: &WindowEscapeState) -> usize {
        1_000_000
    }

    fn window_escape_middle_item(
        _state: &WindowEscapeState,
        index: usize,
    ) -> Node<WindowEscapeState> {
        Node::Text(format!("row-{index}"))
    }

    fn window_escape_l0(_state: &WindowEscapeState) -> Vec<Node<WindowEscapeState>> {
        vec![
            Node::List {
                expand: window_escape_shelf_a,
                axis: Axis::Horizontal,
            },
            Node::VirtualList {
                len: window_escape_middle_len,
                item: window_escape_middle_item,
                item_extent: 40.0,
                axis: Axis::Vertical,
            },
            Node::List {
                expand: window_escape_shelf_b,
                axis: Axis::Horizontal,
            },
        ]
    }

    fn window_escape_tree() -> Screen<WindowEscapeState> {
        Screen::new("window-escape-tree").list_with_axis(window_escape_l0, Axis::Vertical)
    }

    #[test]
    fn move_focus_window_bounded_does_not_escape_to_sibling_container() {
        let state = WindowEscapeState;
        let screen = window_escape_tree();
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![1, 500_000],
            id: WidgetId::from_raw(999),
        });

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(
            !moved,
            "探索窓の境界で打ち切った場合は祖先コンテナへエスケープせず移動失敗のはず"
        );
        assert_eq!(
            ring.current().unwrap().path,
            vec![1, 500_000],
            "ShelfB（次の兄弟コンテナ）へフォーカスが飛んではならない"
        );
    }

    /// 対照実験: コンテナの端（`Exhausted`）まで走査して候補がなかった場合は
    /// 従来どおり祖先コンテナへエスケープすることを確認する（既存挙動の回帰確認、
    /// 机上検証 #3 相当のツリーを再利用）。
    #[test]
    fn move_focus_exhausted_container_still_escapes_to_next_sibling() {
        let screen = axis_tree();
        let state = AxisTreeState;
        let mut ring = ring_at(&[0, 0, 0], 1);

        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);

        assert!(moved);
        assert_eq!(ring.current().unwrap().path, vec![0, 1, 0]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(4));
    }

    // --- codex-review 9 巡目 P1 対応: 段階探索カーソル（`SearchCursor`）の検証 ---
    //
    // 巨大 `VirtualList`（len=10^6）の中央付近に「現在フォーカス位置（index）」と
    // 「そこから `STAGGERED_TARGET_OFFSET` 件先の到達可能項目」のみを `Button` とし、
    // 間を非フォーカス可能な `Text` で埋めたツリーを使う。オフセットは既定マージン
    // （`VIRTUAL_LIST_SEARCH_MARGIN` = 8）を大きく超えるため、`WindowBounded` の
    // 段階探索カーソルが機能しない限り、同方向の入力をいくら繰り返しても到達
    // できない（`move_focus_window_bounded_does_not_escape_to_sibling_container`
    // が示す「窓外は永久にフォーカス維持」という 566bc97 時点の挙動そのもの）。

    #[derive(Default)]
    struct StaggeredVirtualState {
        item_call_count: std::cell::Cell<usize>,
        /// 段階探索の途中で基底データが並べ替わったことを模す（should 指摘対応
        /// テスト専用）。0 なら従来どおり `STAGGERED_CURRENT_INDEX` /
        /// `STAGGERED_CURRENT_INDEX + STAGGERED_TARGET_OFFSET` に固定 button が
        /// 置かれる。非 0 にすると両 index がまとめてこの分だけ後方へずれる
        /// （挿入等で基底配列がシフトし、同じ id の項目が別 index で
        /// 再捕捉される状況の再現）。
        shift: std::cell::Cell<usize>,
    }

    const STAGGERED_CURRENT_INDEX: usize = 500_000;
    const STAGGERED_TARGET_OFFSET: usize = 50;

    fn staggered_no_op(_state: &mut StaggeredVirtualState) {}

    fn staggered_len(_state: &StaggeredVirtualState) -> usize {
        1_000_000
    }

    fn staggered_item(state: &StaggeredVirtualState, index: usize) -> Node<StaggeredVirtualState> {
        state.item_call_count.set(state.item_call_count.get() + 1);
        let current = STAGGERED_CURRENT_INDEX + state.shift.get();
        if index == current {
            Node::Button {
                label: "current".to_string(),
                on_click: staggered_no_op,
                id: WidgetId::from_raw(1),
            }
        } else if index == current + STAGGERED_TARGET_OFFSET {
            Node::Button {
                label: "target".to_string(),
                on_click: staggered_no_op,
                id: WidgetId::from_raw(42),
            }
        } else {
            Node::Text(format!("row-{index}"))
        }
    }

    fn staggered_tree() -> Screen<StaggeredVirtualState> {
        Screen::new("staggered-tree").virtual_list_with_axis(
            staggered_len,
            staggered_item,
            40.0,
            Axis::Vertical,
        )
    }

    fn staggered_ring() -> FocusRing {
        let mut ring = FocusRing::new();
        ring.current = Some(FocusEntry {
            path: vec![0, STAGGERED_CURRENT_INDEX],
            id: WidgetId::from_raw(1),
        });
        ring
    }

    /// マージン超の距離にしか候補がない `VirtualList` で、同方向の入力を繰り返すと
    /// 段階探索カーソルにより最終的に到達可能項目へフォーカスが移ることを検証する
    /// （codex-review 9 巡目 P1 指摘そのものの確認）。
    #[test]
    fn move_focus_staged_search_reaches_target_beyond_margin_via_repeated_input() {
        let state = StaggeredVirtualState::default();
        let screen = staggered_tree();
        let mut ring = staggered_ring();

        let mut reached = false;
        // 1 回あたりの前進量はおよそ margin + 1（既定 9）のため、50 件先へは
        // 6 回程度で到達するはず。余裕を見て 12 回まで許容する。
        for _ in 0..12 {
            if ring.move_focus(&screen, &state, FocusDirection::Down) {
                reached = true;
                break;
            }
        }

        assert!(
            reached,
            "段階探索カーソルにより既定マージンを超えた距離でも最終的に到達できるはず"
        );
        assert_eq!(
            ring.current().unwrap().path,
            vec![0, STAGGERED_CURRENT_INDEX + STAGGERED_TARGET_OFFSET]
        );
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(42));
    }

    /// 段階探索の各回でも、1 入力あたりの `item()` 呼び出し回数が引き続き有界
    /// （探索窓 1 枚分程度）であることを検証する。段階探索が「打ち切り位置からの
    /// 続き」を正しく窓化できていない場合、毎回同じ窓を舐め直す・あるいは残り
    /// 全域を舐めるといった劣化が起こりうる。
    #[test]
    fn move_focus_staged_search_keeps_item_calls_bounded_per_input() {
        let state = StaggeredVirtualState::default();
        let screen = staggered_tree();
        let mut ring = staggered_ring();

        let mut reached = false;
        for _ in 0..12 {
            state.item_call_count.set(0);
            let moved = ring.move_focus(&screen, &state, FocusDirection::Down);
            assert!(
                state.item_call_count.get() < 100,
                "1 入力あたりの item() 呼び出し回数は引き続き有界のはず（実測: {}）",
                state.item_call_count.get()
            );
            if moved {
                reached = true;
                break;
            }
        }
        assert!(reached, "有界のまま最終的に到達できるはず（前提の確認）");
    }

    /// 方向が変わった場合、段階探索カーソルがリセットされ、逆方向の探索が
    /// カーソルを引き継がず正しく現在位置基準で始まることを検証する。
    #[test]
    fn move_focus_staged_search_resets_cursor_on_direction_change() {
        let state = StaggeredVirtualState::default();
        let screen = staggered_tree();
        let mut ring = staggered_ring();

        let moved_down = ring.move_focus(&screen, &state, FocusDirection::Down);
        assert!(
            !moved_down,
            "1 回の Down では既定マージン超のため探索窓の境界で打ち切られるはず"
        );
        let cursor_after_down = ring
            .search_cursor
            .clone()
            .expect("窓打ち切り後は段階探索カーソルが記録されるはず");
        assert_eq!(cursor_after_down.direction, FocusDirection::Down);

        let moved_up = ring.move_focus(&screen, &state, FocusDirection::Up);
        assert!(
            !moved_up,
            "Up 方向にも既定マージン内に候補が無いため探索窓の境界で打ち切られるはず"
        );
        let cursor_after_up = ring
            .search_cursor
            .clone()
            .expect("Up 方向でも窓打ち切りによりカーソルが記録されるはず");
        assert_eq!(
            cursor_after_up.direction,
            FocusDirection::Up,
            "方向転換により Down 用カーソルが Up 用へ置き換わっているはず\
             （Down 用カーソルが誤って引き継がれていないことの確認）"
        );
        assert!(
            cursor_after_up.resume_index < STAGGERED_CURRENT_INDEX,
            "Up 方向の再開位置は現在位置より小さい index のはず（Down 方向の\
             再開位置が誤って残っていないことの確認、実測: {}）",
            cursor_after_up.resume_index
        );
    }

    /// TV レイヤの毎フレーム `rebuild` 呼び出しを模し、構造変化のない `rebuild` を
    /// 挟んでも段階探索の進捗（`search_cursor`）が失われないことを検証する。
    /// 失われる場合、`rebuild` のたびに探索窓が現在位置基準へ巻き戻り、
    /// マージン超の距離には永久に到達できない。
    #[test]
    fn move_focus_staged_search_survives_structurally_unchanged_rebuild() {
        let state = StaggeredVirtualState::default();
        let screen = staggered_tree();
        let mut ring = staggered_ring();
        let hints = WindowHints::new();

        let mut reached = false;
        for _ in 0..12 {
            // 構造変化なしの毎フレーム rebuild（TV レイヤの想定呼び出しパターン）。
            ring.rebuild(&screen, &state, &hints);
            if ring.move_focus(&screen, &state, FocusDirection::Down) {
                reached = true;
                break;
            }
        }

        assert!(
            reached,
            "毎フレーム rebuild を挟んでも段階探索の進捗は失われず、\
             最終的に到達できるはず"
        );
        assert_eq!(
            ring.current().unwrap().path,
            vec![0, STAGGERED_CURRENT_INDEX + STAGGERED_TARGET_OFFSET]
        );
    }

    /// codex-review should 指摘の検証: 段階探索の途中で「同一 id・別 path」への
    /// 再捕捉が起きる `rebuild`（基底データの並べ替えを模す）を挟むと、
    /// `id` のみの旧判定では見逃されカーソルが生き残ってしまっていたが、
    /// `path` も検証する新判定ではカーソルが破棄され、並べ替え後の位置から
    /// 正しく探索が再開されることを確認する。
    #[test]
    fn move_focus_staged_search_cursor_discarded_on_same_id_reindex_rebuild() {
        let state = StaggeredVirtualState::default();
        let screen = staggered_tree();
        let mut ring = staggered_ring();

        // 1. 通常どおり窓境界で打ち切り、段階探索カーソルを積む。
        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);
        assert!(
            !moved,
            "既定マージン超のため初回は探索窓の境界で打ち切られるはず"
        );
        assert!(
            ring.search_cursor.is_some(),
            "打ち切り後は段階探索カーソルが記録されているはず（前提）"
        );

        // 2. 基底データが並べ替わり、同じ id=1 の項目が別 index（+1000）で
        //    再捕捉される状況を模す。`rebuild` にはその新位置を指す
        //    `WindowHints` を渡す（プラットフォームレイヤの reveal 契約と同型）。
        const REORDER_SHIFT: usize = 1_000;
        state.shift.set(REORDER_SHIFT);
        let new_current_index = STAGGERED_CURRENT_INDEX + REORDER_SHIFT;
        let new_target_index = new_current_index + STAGGERED_TARGET_OFFSET;
        // ヒント窓は新しい target（+50）まで含む広さにしておく（プラットフォーム
        // レイヤの reveal がスクロール後の可視範囲を都度渡す想定と同型。ヒント窓
        // を故意に旧位置付近へ固定したままにする検証は「reveal が更新されない
        // 場合の据え置き挙動」という別の関心事であり、本テストの対象外）。
        let mut hints = WindowHints::new();
        hints.insert(vec![0], new_current_index..(new_target_index + 1));
        ring.rebuild(&screen, &state, &hints);

        assert_eq!(
            ring.current().unwrap().path,
            vec![0, new_current_index],
            "rebuild は同一 id を新しい index で再捕捉できているはず（前提）"
        );
        assert!(
            ring.search_cursor.is_none(),
            "id は一致していても path が変わった再捕捉では段階探索カーソルを\
             破棄しなければならない（should 指摘の本体）"
        );

        // 3. 破棄後の探索は stale な resume_index を引き継がず、並べ替え後の
        //    現在位置を起点に再開する（もし旧 resume_index を使い回していれば、
        //    もはや存在しない旧位置周辺を探索してしまい、この 1 回の入力では
        //    到達できないはず）。ヒント窓は新 target を含むため、正しく起点を
        //    取り直せていれば 1 回の Down で到達する。
        let moved = ring.move_focus(&screen, &state, FocusDirection::Down);
        assert!(
            moved,
            "カーソル破棄後は並べ替え後の現在位置基準で再開し、\
             新しい target へ到達できるはず"
        );
        assert_eq!(ring.current().unwrap().path, vec![0, new_target_index]);
        assert_eq!(ring.current().unwrap().id, WidgetId::from_raw(42));
    }
}
