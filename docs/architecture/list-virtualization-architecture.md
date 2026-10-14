# リスト仮想化（可視範囲算出・ウィンドウイング）アーキテクチャ設計

## 1. 目的・位置づけ

本文書は #83（TASK-5.5「リスト仮想化・再利用機構実装（300 項目規模）」）の 1/5 sub-issue
である #84（TASK-5.5-S1「可視範囲算出・ウィンドウイング設計」）の成果物であり、
**設計文書 1 本のみを成果物とし、コード変更は行わない**（TASK-4.4-S1・#79 のパターンを踏襲）。

### spec 対応

- **REQ-5**（`docs/spec/04-requirements.md` L80-94）の受け入れ基準:
  「300 項目規模のリストスクロール時、immediate mode（毎フレーム全項目再描画）に
  起因する性能劣化がないことを、リスト仮想化・再利用機構の実装により確認する」
  （L89）。同 REQ-5 のフレームレート基準（L87: 95 パーセンタイルで 16.6ms 以内）を
  300 項目規模でも脅かさないことが最終ゴール。
- **TASK-5.5**（`docs/spec/05-tasks.md` L205-210）: 成果物は「リスト仮想化モジュール、
  性能確認テスト」、推定工数 2.5 日、前提タスクは TASK-1.1、M3 所属（Must、条件 6:
  L643「リスト仮想化は Must」）。
- **M3 完了基準**（`docs/spec/06-roadmap.md` L60 以降）: 3 プラットフォーム統合・基盤
  クレート新規開発マイルストーンの一部としてリスト仮想化の実装完了を含む。

### #83〜#88 の分担表

| sub-issue | 内容 | 本文書との関係 |
|-----------|------|----------------|
| #84（本文書・S1） | 可視範囲算出・ウィンドウイング**設計** | 本文書がその成果物 |
| #85（S2） | 仮想化リストのコア実装 | 本文書 §3.2〜3.3 のシグネチャ案を実装 |
| #86（S3） | 項目再利用機構の実装 | 本文書 §3.5 の ID 安定性方針に従う |
| #87（S4） | 300 項目規模の性能確認テスト実装 | 本文書 §3.5 の検証観点をテストケース化 |
| #88（S5） | スクロール挙動の調整・検証 | 本文書 §3.5 のスクロールバー整合方針に従う |

S2〜S5 は本文書の型シグネチャ・境界条件・テスト観点を前提に着手できる粒度を目標とする。

## 2. 現状分析

本設計が前提とする（仮想化導入前の）リスト描画経路は以下のとおり。

1. **`Node::List`**（`crates/fandhe-core/src/ui/node.rs` L75-81）: 子ノード一覧を
   状態 `S` から都度生成する関数ポインタ `expand: ReadFn<S, Vec<Node<S>>>` を保持する
   だけのバリアント。1 段展開は `Node::expand_children`（L90-95）が担う。
2. **`Screen::visit_expanded`**（`crates/fandhe-core/src/ui/screen.rs` L174-189）: 全
   ノードを挿入順に走査し、`List` に遭遇するたびに `expand(state)` を呼んで
   **子ノード全件を毎回 `Vec` として新規生成**し、深さ優先で再帰的に訪問する。
   ネストした `List` にも対応するが、可視範囲という概念自体が存在しない。
3. **`fandhe-egui-proxy::render_screen`**（`crates/fandhe-egui-proxy/src/lib.rs`
   L64-109）: `visit_expanded` の結果を **毎フレーム全ノードぶん** egui ウィジェットへ
   マッピングする（`Node::Text` → `ui.label`、`Node::TextField` → `ui.text_edit_singleline`
   等）。`ScrollArea` 等のビューポート概念には未対応。

### 300 項目規模でのコスト

上記の経路をそのまま 300 項目のリストに適用すると、毎フレーム:

- `expand(state)` が 300 要素の `Vec<Node<S>>` を新規アロケートする（immediate mode の
  典型的コスト）。
- `visit_expanded` が 300 個の `Node` を再帰訪問する。
- `render_screen` が 300 個の egui ウィジェット（`label` / `text_edit_singleline` /
  `button`）を生成・レイアウト・描画する。

画面に実際に映るのは通常数十項目程度であり、残り大部分は「見えないのに毎フレーム
生成・描画されるコスト」となる。これが REQ-5 のフレームタイム基準（95 パーセンタイルで
16.6ms 以内）を 300 項目規模で脅かす主要因であり、TASK-5.5 が解消すべき対象である。

## 3. 設計判断（結論）

### 3.1 レイヤ分界

`coding-rust.md` の「共通コアからプラットフォーム固有クレートへ依存しない
（依存方向は最適化レイヤ → コアの一方向）」を維持する。

| 責務 | 配置レイヤ | 理由 |
|------|-----------|------|
| 可視範囲算出（`compute_visible_range`） | `fandhe-core`（依存ゼロ・`#![forbid(unsafe_code)]` の純粋関数） | スクロール位置・ビューポート寸法・項目数という数値のみを入力にとる算術であり、egui 等の描画基盤に依存する理由がない。SwiftUI ブリッジ・DOM レンダラ（`fandhe-web` 等）でも同一契約を再利用するため、コア側の純粋関数として契約を固定する |
| スクロールオフセット・ビューポート実寸の取得 | プラットフォームレイヤ（`fandhe-egui-proxy` では `egui::ScrollArea`） | スクロール位置はネイティブ UI ツールキットの状態であり、コアが `&S` から取得できるものではない |
| 実際のウィジェット生成・描画 | プラットフォームレイヤ | `render_screen` が担う既存責務を維持し、コアは「どの範囲を描くべきか」のみを返す |

依存方向はプロキシ（`fandhe-egui-proxy`）→ コア（`fandhe-core`）の一方向を維持し、
コア側に egui 型は一切登場させない。

### 3.2 可視範囲算出アルゴリズム

固定項目高さ（`item_extent`）前提の O(1) 算出を主方式とする（可変高さは
§5 の未解決事項へ送る）。

```text
first = floor(scroll_offset / item_extent)
visible_count = ceil(viewport_extent / item_extent) + 1
```

overscan（前後バッファ `overscan` 項目）を加えた
`[first.saturating_sub(overscan), first + visible_count + overscan)` を
項目総数 `len` で **クランプ**した範囲を最終的な可視範囲とする。

#### 境界条件表（S2 のユニットテストにそのまま引き継ぐ想定）

| ケース | 入力例 | 期待される振る舞い |
|--------|--------|---------------------|
| 空リスト | `len = 0` | 空範囲（`0..0`）を返す。panic しない |
| 先頭 | `scroll_offset = 0` | `first = 0` から開始する範囲 |
| 末尾超過 | `scroll_offset` がコンテンツ全高を超える | 終端を `len` でクランプし、範囲が `len` を超えない |
| 負のオフセット | `scroll_offset < 0`（浮動小数点の丸め等に由来） | `first` を `0` にクランプする（負値を渡さない契約でも防御的に扱う） |
| viewport がコンテンツ全高より大きい | `viewport_extent > len * item_extent` | 可視範囲は `0..len` 全体になる（overscan を足しても `len` でクランプ） |
| `item_extent = 0` | ゼロ除算になり得る入力 | ゼロ除算を回避し、安全側（例: 空範囲または `0..len` の固定挙動）を返す。**panic を API 契約にしない**（`coding-rust.md` のエラーハンドリング方針） |
| overscan がリスト端を超える | `overscan` が `first` や `len - (first + visible_count)` より大きい | 開始・終了とも `0..len` の範囲内にクランプされる |

整数変換（`f32`/`f64` の座標系 ⇔ `usize` の index）は `saturating_*` 系の演算で行い、
オーバーフロー・アンダーフローによる panic を避ける。

### 3.3 ウィンドウイング API 契約（S2 実装対象のシグネチャ案）

`fandhe-core` に新モジュール `ui/virtual_list.rs`（案）を追加し、以下の型・関数を置く。

```rust
/// 可視範囲を表す半開区間。`std::ops::Range<usize>` と同じ意味論を持つが、
/// ウィンドウイング専用の名前を与えて呼び出し側の意図を明確にする。
pub type VisibleRange = std::ops::Range<usize>;

/// スクロール位置・ビューポート寸法・項目数から、描画すべき項目の index 範囲を
/// 算出する純粋関数。副作用を持たず、状態 `S` にもプラットフォームにも依存しない。
///
/// `item_extent <= 0.0` や `len == 0` はゼロ除算・空範囲として安全側に扱い、
/// panic しない（境界条件表は本設計文書 §3.2 を参照）。
pub fn compute_visible_range(
    viewport_extent: f32,
    scroll_offset: f32,
    item_extent: f32,
    len: usize,
    overscan: usize,
) -> VisibleRange {
    // 実装は S2（#85）が担う。本シグネチャが契約。
    unimplemented!()
}
```

既存 `Node::List`（全展開）は後方互換のため存置し、index ベースの遅延展開を行う
新バリアントを追加する案を提示する。

```rust
/// index ベースで項目を遅延展開するリスト（TASK-5.5、`Node::List` の仮想化版）。
///
/// `expand` の代わりに `len`（総項目数を返す関数ポインタ）と `item`
/// （index から 1 項目分の `Node<S>` を生成する関数ポインタ）を持つ。
/// 可視範囲 `VisibleRange` に含まれる index のみへ `item` を呼び出すことで、
/// `Node::List::expand` のような「全件展開」を避ける（AI 型安全 DSL の制約
/// —— クロージャ禁止・関数ポインタ限定 —— は `Node::List` と同様に維持する。
/// REQ-13 / PoC-6 踏襲、`crate::state::ReadFn` SSOT 型エイリアスへ統合する）。
VirtualList {
    /// 状態 `S` から総項目数を取得する関数ポインタ。
    len: crate::state::ReadFn<S, usize>,
    /// 状態 `S` と index から 1 項目分の `Node<S>` を生成する関数ポインタ。
    /// クロージャの環境キャプチャを避けるため、素の関数ポインタ
    /// `fn(&S, usize) -> Node<S>` に限定する（`Node::Button::on_click` 等と
    /// 同じ設計判断。関数ポインタであれば `ReadFn` と異なりコンパイル時に
    /// 引数が 2 つ固定される点に注意し、S2 実装時に型エイリアス化するか
    /// 生の `fn` ポインタのまま据え置くかを決定する）。
    item: fn(&S, usize) -> Node<S>,
    /// 1 項目あたりの高さ（固定項目高さ前提、§3.2 参照）。
    item_extent: f32,
},
```

`visit_expanded` との統合方針: 可視範囲を外部（プラットフォームレイヤ）から渡す
走査 API を新設する（例: `Screen::visit_windowed(&self, state: &S, range: &VisibleRange,
f: impl FnMut(&Node<S>))`）。`VirtualList` に遭遇した場合のみ `range` を用いて
`item(state, index)` を呼び、それ以外の既存バリアント（`Text` / `TextField` /
`Button` / `List`）は `visit_expanded` と同じ全走査を維持する（`VirtualList` を
使わない既存画面の挙動を変えない後方互換性を保つ）。

**`fandhe-core`（S2、Issue #85）の型**: 上記の契約どおり `fandhe-core` は
`ui/virtual_list.rs`（`VisibleRange` / `compute_visible_range`）・`Node::VirtualList`
（`len` / `item` / `item_extent`）・`Screen::virtual_list()` / `Screen::visit_windowed()`
を提供する。`visit_expanded` / `Node::expand_children` は `VirtualList` を
`0..len(state)` の全項目へ展開する（`render_headless` / `fandhe-swiftui-bridge::codec` /
`fandhe-web-dom::dom_tree` 等の既存走査経路が項目を消さない意味論的後方互換を優先し、
性能経路は `visit_windowed` に限定する設計判断）。`item` の型表記は
`crate::state::ReadIndexedFn<S, T> = fn(&S, usize) -> T`（SSOT 型エイリアス）を使う。
S3（`ListRecycler`、#86）・S5（`total_content_extent` / `clamp_scroll_offset`、#88）との
統合、および `fandhe-egui-proxy::render_screen` の `egui::ScrollArea::show_viewport`
実配線は §3.5 で述べる。

### 3.4 プラットフォーム統合方針

`fandhe-egui-proxy` 側は `egui::ScrollArea`（`show_rows` あるいは `show_viewport`
相当の API）からビューポートの実寸・現在のスクロールオフセットを取得し、コアの
`compute_visible_range` が返した範囲のみを `Screen::visit_windowed` 経由で描画する。

- egui の組み込み仮想化（`ScrollArea::show_rows`）へ丸投げしない理由: 仮想化の
  契約（可視範囲算出アルゴリズム自体）をコア（`fandhe-core`）に持たせることで、
  SwiftUI ブリッジ（`apple-swiftui`、`List` の `onAppear`/`onDisappear` 相当）・
  DOM レンダラ（`fandhe-web`、TASK-4.4 系の仮想スクロール）等の他バックエンドでも
  同一の可視範囲算出契約を再利用できる。egui 依存の実装に埋め込むと、
  プロキシ以外のプラットフォームレイヤが同じ仮想化ロジックを再実装する
  二重管理になり、`coding-rust.md` の「共通コア ⇔ 最適化レイヤ」の分界に反する。
- `egui::ScrollArea` からの入力取得（`ui.available_height()` 相当のビューポート
  実寸、`ScrollArea::show_viewport` が渡すオフセット）は egui API に固有のため
  引き続きプロキシ側の責務とする。API の詳細確認は S2 実装時に
  reference-researcher へ一次情報付きで再委譲する想定（本設計文書は方針のみを
  確定する）。

### 3.5 S3〜S5 への接続点

- **S3（項目再利用機構、#86）**: ウィンドウ移動時に egui widget id が index と
  ともにずれない設計が必要（`egui::Ui::push_id` 等による `id_salt` の付与を想定）。
  `VirtualList::item(state, index)` が返す `Node<S>` 自体は毎回生成されるため、
  「再利用」は egui 側の widget id 安定化（同一 index には同一 id を割り当てる）
  という形で実現する方針を S3 へ引き継ぐ。
- **S4（性能確認テスト、#87）**: 検証観点は「展開ノード数がウィンドウ幅
  （`visible_count + overscan * 2` 程度）に比例し、総項目数 300 に比例しないこと」
  のアサーションと、フレームタイム計測（REQ-5 の 95 パーセンタイル 16.6ms 基準との
  比較）の 2 点。§3.2 の境界条件表をそのままユニットテストケースへ転用できる。
- **S5（スクロール挙動、#88）**: 総コンテンツ高さ `len * item_extent` を
  `egui::ScrollArea` へ明示することでスクロールバーの見た目上の長さ・位置を
  実項目数と整合させる（可視範囲だけを描画してもスクロールバーが正しい縮尺で
  表示される前提）。

  **`crates/fandhe-core/src/ui/scroll.rs` の設計**: `total_content_extent` /
  `clamp_scroll_offset` は、本文書 §3.4 の依存方向規約（プロキシ → コア）に従い、
  `Node::VirtualList` の型に依存しない自己完結の純粋関数として提供する。

  - `total_content_extent(len: usize, item_extent: f32) -> f32`: 総コンテンツ高さの算出。
    `item_extent` が非有限・`0.0` 以下の場合や `len * item_extent` が `f32` の範囲を
    超える場合は、`NaN`/無限大を `egui::ScrollArea` へ渡さないよう安全側（`0.0` または
    `f32::MAX`）へ丸める（§3.2 境界条件表を踏襲、panic しない契約）。
  - `clamp_scroll_offset(offset: f32, total_extent: f32, viewport_extent: f32) -> f32`:
    `egui::ScrollArea::show_viewport` から得たオフセットを `[0, max(total_extent -
    viewport_extent, 0)]` へ防御的にクランプする。負値・`NaN`・無限大・
    「viewport がコンテンツ全高より大きい」（§3.2）のいずれも安全側の値へ丸める。
  - **統合側の責務**: `Node::VirtualList` / `compute_visible_range` / `Screen::visit_windowed`
    がウィンドウ幅の算出とツリー走査を担い、`fandhe-egui-proxy::render_screen` が
    `egui::ScrollArea::show_viewport` の配線とウィンドウ移動時の widget id 安定化
    （`push_id(index)`、S3 §3.5 方針）を担う。`scroll.rs` の純粋関数はこれらから
    幾何演算のみを切り出して呼び出される。

  **REQ-5 受け入れ基準・M3 完了基準との整合**（親 issue #83 の受け入れ条件）: REQ-5
  （`docs/spec/04-requirements.md` L89）の受け入れ基準は「300 項目規模のリストスクロール時、
  immediate mode に起因する性能劣化がないことを、リスト仮想化・再利用機構の実装により
  確認する」であり、`scroll.rs` はそのうち「スクロールバーの縮尺・位置整合」という
  挙動面の契約を幾何演算として固定する。性能面の確認（フレームタイム計測）は S4（#87）の
  責務であり、S5 は重複実装しない。M3 完了基準（`docs/spec/06-roadmap.md` L107 付近、
  リスト仮想化の実装完了）に対しては、S4（#87）が性能面、S5 が幾何契約という責務分担で
  TASK-5.5 全体の完了に寄与する。

## 4. トレードオフ・代替案

| 案 | 内容 | 不採用理由 |
|----|------|-----------|
| egui `show_rows` 完全依存案 | 可視範囲算出・ウィンドウイングを egui 側の API にすべて委譲する | §3.4 のとおり、契約がプロキシ（egui 依存）に閉じ込められ、SwiftUI・DOM 等の他バックエンドで再利用できない。`coding-rust.md` の依存方向規約（プロキシ → コア）にも反する |
| 全展開維持＋描画スキップのみ案 | `expand`/`visit_expanded` は現状のまま全展開し、`render_screen` 側で可視範囲外のウィジェット生成のみスキップする | `expand(state)` の `Vec<Node<S>>` 全件アロケーションのコスト（300 項目分のヒープ確保）が毎フレーム残るため、REQ-5 の性能目標達成に不十分。ノード生成そのものを可視範囲に絞る必要がある |
| 可変項目高さを最初から本設計に含める案 | 項目ごとに異なる高さを許容する可視範囲算出 | 累積高さテーブルの構築・更新コストが加わり、TASK-5.5 の推定工数（2.5 日）内での確定が難しい。固定高さ前提で先に REQ-5 の 300 項目規模の目標を達成し、可変高さは §5 の将来課題として切り出す判断とした |

## 5. 未解決事項・将来課題

- **可変項目高さ対応**: 本設計の対象外。必要になった場合は
  `.claude/rules/out-of-scope-tracking.md` のフロー（既存 Issue 確認 → ユーザー承認 →
  起票）に従い、ユーザー承認後に別 Issue として起票する。
- **ネストした `VirtualList`**: `Node::List` のネスト（`visit_expanded` の再帰展開）と
  異なり、本設計では `VirtualList` のネストの可視範囲算出（親のウィンドウ内で
  さらに子のウィンドウを算出する場合の座標系変換）を扱っていない。S2 実装時に
  スコープ内かどうかを再確認する。
- **書き込みアクセサ統合**: `Node::List::expand` が `ReadFn`（読み取り専用）である
  のと同様、`VirtualList::len` は読み取り専用の設計とした。書き込みアクセサ
  （S2 #30 系）との統合は、`Node::TextField::set` の既存据え置き判断
  （`node.rs` コメント参照）と同じ理由で本設計のスコープから除外する。
