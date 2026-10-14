# ウィジェット共通トレイト・ライフサイクル設計

## 1. 目的・位置づけ

本文書は #95（TASK-8.3「ウィジェットセット新規開発」、REQ-8、推定工数 30 日）の
60 sub-issue 分解のうち最初の 1 件である #96（TASK-8.3-S1「ウィジェット共通トレイト・
ライフサイクル設計」）の成果物であり、**設計文書 1 本のみを成果物とし、コード変更は
行わない**（TASK-4.4-S1・#79、TASK-5.5-S1・#84 のパターンを踏襲）。

トレイト・ライフサイクルモデルの Rust 実装は S2 以降のスコープとし、本文書は
S2〜S60 が実装する契約（シグネチャ案・状態遷移・境界条件）を確定させる。

### spec 対応

- **REQ-8**（`docs/spec/04-requirements.md` L114-124）: 「`wgpu`（描画）・`taffy`
  （レイアウト）・`cosmic-text`（テキスト）・`winit`（ウィンドウ/入力）・`accesskit`
  （アクセシビリティ橋渡し）を基盤として採用し、その上にウィジェットセット・
  デザイントークン・IME 統合・ストア提出パイプラインを新規開発する」。本文書は
  このうち「ウィジェットセット」領域の設計を担う。
- **TASK-8.3**（`docs/spec/05-tasks.md` L267-272）: 成果物「ウィジェットセット一式」、
  推定工数 30 日、前提タスク TASK-1.1、担当「Claude Code（分野: コア Rust）」。
  M3 所属（同 L629: TASK-8.3〜8.5 は TASK-1.1 完了後に相互並行実施可能な最長系列）。
- **REQ-13 / NFR-9**（`docs/spec/04-requirements.md` L171 以降・L256・L268）: 状態
  アクセスは `fn(&S) -> T` / `fn(&mut S, T)` の関数ポインタに限定し、クロージャに
  よる環境キャプチャを型システムで拒否する（PoC-6 実測: 安全網検知率 100%）。本設計は
  この制約を宣言層（`Screen<S>` / `Node<S>`）に維持したまま、ウィジェット実装層を
  新設する二層モデルを採る（§4.1）。
- **REQ-2**（同 L42 以降）: 共通コア（`fandhe-core`）とプラットフォーム最適化レイヤーの
  二層アーキテクチャ、依存方向は最適化レイヤー → コアの一方向。本設計のトレイトは
  `fandhe-core` に置き、`wgpu`/`taffy`/`cosmic-text` 等の具体実装には依存しない
  純粋な契約として定義する。
- **REQ-9**（同 L125-135）: `accesskit` を通じた OS 標準アクセシビリティ API への UI
  ツリー公開（macOS 達成済み、iOS/Android 未検証、Web は構造的制約あり）。本設計の
  共通トレイトはアクセシビリティロールの接続点を持つ（§4.3）。

### #96〜#155 の分担表（S1〜S6 が共通基盤、S7〜S60 が個別ウィジェット）

`gh api repos/Fandhe-AI/fandhe-multi-platform/issues/95/sub_issues` で取得した実際の
60 件の子 issue（#96〜#155）に基づく。

| sub-issue | 内容 | 本文書との関係 |
|-----------|------|----------------|
| #96（本文書・S1） | ウィジェット共通トレイト・**ライフサイクル設計** | 本文書がその成果物 |
| #97（S2） | 共通スタイリング・状態（hover/pressed/disabled/focus）基盤実装 | 本文書 §4.1・§4.4 のインタラクション状態モデルを実装 |
| #98（S3） | イベントディスパッチ・ヒットテスト基盤実装 | 本文書 §4.3 のイベント処理契約・`WidgetId` 方式を実装 |
| #99（S4） | レイアウトプロトコル（サイズ計測・配置）基盤実装 | 本文書 §4.3 の計測・配置契約（`taffy` 導入含む）を実装 |
| #100（S5） | ウィジェットテストハーネス整備 | 本文書 §4.4 のライフサイクル状態機械をテスト観点化 |
| #101（S6） | ウィジェットカタログ（サンプルギャラリー）土台実装 | 本文書のトレイトを実装する個別ウィジェット（S7〜S60）の一覧管理 |
| #102〜#155（S7〜S60、ボタン〜カード・セパレータ 18 種 × 3 工程） | 個別ウィジェットの API 設計・状態/イベント処理・テスト＆カタログ登録 | 本文書 §4.3 のトレイトシグネチャを実装対象として引き継ぐ |

S2〜S6 は本文書の型シグネチャ・状態遷移・境界条件を前提に着手できる粒度を目標とする。

## 2. 現状分析

現在の UI 経路は以下のとおり。

1. **`Node<S>`**（`crates/fandhe-core/src/ui/node.rs`）: `Text` / `TextField` /
   `Button` / `List` の 4 バリアントを持つ閉じた enum。状態アクセスは `get: ReadFn<S, T>`
   （`fn(&S) -> T`）・`set: fn(&mut S, T)`・`on_click: fn(&mut S)` のいずれも素の
   関数ポインタに限定される（クロージャの環境キャプチャを型で拒否、
   `tests/compile_fail.rs` で固定化済み）。`Debug`/`Clone` は `derive` ではなく
   手動実装（状態型 `S` への不要な境界を避けるため）。
2. **`Screen<S>`**（`crates/fandhe-core/src/ui/screen.rs`）: `.text()` / `.text_field()` /
   `.button()` / `.list()` のビルダーメソッドで `Node<S>` 列を構築し、
   `visit_expanded` で走査する。`List` は都度 `expand(state)` を呼び直す
   **immediate mode 的評価**（毎回全件を `Vec` として再生成）。
3. **`StateStore<S>`**（`crates/fandhe-core/src/state/store.rs`）: `Event`
   （`ButtonClicked` / `TextFieldChanged`）を受け取り `dispatch` する。発火のたびに
   `version: u64` をインクリメントする仕組みは既にあるが、**ノードの特定は
   `label: String` の一致検索**（`event::click_button` 参照）に依存している。
4. **`RendererLifecycle`**（`crates/fandhe-core/src/ui/render_mode.rs`、REQ-4 /
   TASK-4.4-S3）: `activate` / `deactivate` の 2 メソッドを持つ先行のライフサイクル
   抽象。ただしこれは「**画面単位の出力方式**（Canvas / DOM）の起動・停止」を担う
   `ModeSwitcher` 専用の抽象であり、個々のウィジェット実体のマウント/更新/破棄とは
   粒度・責務が異なる。本設計の「ウィジェットライフサイクル」とは別物として命名・
   責務を区別する（§4.4 末尾）。
5. **`ListRecycler<S>`**（`crates/fandhe-core/src/ui/recycle.rs`、TASK-5.5-S3）: 可視
   ウィンドウ内の項目ノードをインデックスキーでプールし、状態バージョン
   （`StateStore::version`）が変わらない限り再構築せず再利用する retained な
   再利用機構。本設計が導入する「宣言 → mount → update（reconcile）」のうち
   update フェーズの先行実装例として整合を取る対象。

### 18 種のウィジェットへ拡張する際の課題

現行設計をそのまま 18 種（ボタン〜カード・セパレータ）へ拡張しようとすると、
以下の 3 点が障害になる。

- **閉じた enum**: `Node<S>` へバリアントを追加し続けると、`match` 網羅性の破壊が
  18 箇所以上へ伝播し、`Debug`/`Clone` の手動実装・`visit_expanded` 等の走査コードが
  肥大化する。ウィジェットごとに専用のフィールド（`Slider` の `min`/`max`/`step`、
  `Checkbox` の `checked` 等）を enum バリアントに直接持たせる設計は、共通の
  インタラクション状態（hover/pressed/disabled/focus）をバリアントごとに重複定義
  させることになる。
- **`label` 文字列によるイベント対象特定**: `event::click_button` は
  「同一ラベルのボタンが複数存在する場合は先頭の 1 件のみ発火する」という
  PoC-6 準拠の割り切りに依存している。ウィジェット数が増えるとラベル衝突の
  可能性が増え、外部から渡された文字列（例: ユーザー入力を含む動的ラベル）を
  ディスパッチキーとして解釈する経路はインジェクション観点（OWASP A03、
  `security.md`）でも堅牢性を欠く。
- **インタラクション状態の置き場所が無い**: hover/pressed/disabled/focus のような
  「アプリの永続状態ではないが、UI が一時的に持つ状態」を格納する場所が現行設計に
  存在しない。状態 `S` に含めると、AI 生成コードが触れるべきでない UI 内部詳細が
  アプリ状態へ漏れ出し、REQ-13 の「状態アクセスの単純化」という狙いに反する。

## 3. 前提とする二層モデル

上記の課題を解消するため、本設計は次の二層モデルを前提に置く。

| 層 | 担い手 | 責務 | 状態アクセス制約 |
|----|--------|------|-------------------|
| **宣言層** | AI・アプリ開発者が触る `Screen<S>` ビルダー | 画面構造を宣言する。関数ポインタでアプリ状態 `S` を参照・変更する | REQ-13 の制約（関数ポインタ限定・クロージャ拒否）を維持する |
| **ウィジェット実装層** | フレームワーク内部の retained な実体（本設計のトレイトを実装する型） | 宣言層の情報をもとにマウントされ、hover/pressed 等の一時的インタラクション状態を保持し、計測・配置・描画・イベント処理を行う | アプリ状態 `S` を直接所有しない。宣言層が渡す関数ポインタ経由でのみ `S` に触れる |

この分離により、「hover した」「pressed された」という UI 内部の一時状態は
ウィジェット実装層（本設計のトレイトを実装する型）が保持し、アプリ状態 `S` には
一切含まれない。宣言層の API（`Screen<S>::button()` 等）・関数ポインタ制約は
一切変更しない後方互換な拡張として設計する。

## 4. 設計判断

### 4.1 `Node<S>` との関係: enum 温存 + トレイトオブジェクトの併用

**判断**: `Node<S>` の既存 4 バリアント（`Text` / `TextField` / `Button` / `List`）は
温存し、新規ウィジェット（18 種）は `Node<S>` に enum バリアントを追加するのではなく、
新設する共通トレイト `Widget<S>` を実装する型を `Node::Widget(Box<dyn Widget<S>>)` の
ような 1 バリアントへ集約する案を推奨する。

| 評価軸 | enum バリアント追加を継続 | トレイトオブジェクト化（推奨） |
|--------|---------------------------|-------------------------------|
| 網羅性チェック | `match` が 18 種増えるたびに全走査コードへ伝播。コンパイラの網羅性検査は安全網として働くが、変更コストが線形に増える | 新規ウィジェットの追加が `Node<S>` 側の変更を要さない（`Widget<S>` 実装の追加のみ）。walk 側は `Node::Widget` の 1 分岐のみで済む |
| object safety | 該当なし | `Widget<S>` のメソッドはすべて `&self` / `&mut self` 経由（ジェネリック関数を持たない）とすることで object safety を満たす設計にする（§4.3 のシグネチャ案で確認済み） |
| `Clone`/`Debug` 手動実装との整合 | 既存の手動実装パターン（関数ポインタの `*get` 複製、`String` の `.clone()`）を素直に拡張できる | `Box<dyn Widget<S>>` の `Clone` は `dyn Widget<S>: Clone` を要求できない（トレイトオブジェクトは `Self: Sized` を要求する `Clone` を持てない）ため、`Widget<S>` に `fn clone_box(&self) -> Box<dyn Widget<S>>` のようなオブジェクトセーフな複製メソッドを別途持たせる必要がある（§4.3 で明記） |
| 関数ポインタ制約（REQ-13）の維持 | 素直に維持できる | `Widget<S>` のメソッド自体はトレイトメソッドであり、内部で保持する初期化用ハンドラ（例: 各ウィジェット構築時に渡す `fn(&S) -> ...`）は引き続き関数ポインタ限定にできる。制約が及ぶのは「宣言層から渡す初期化用の値」であり、トレイトメソッド自体には及ばない（トレイトメソッドは実装がフレームワーク側にあり、AI 生成コードが直接書く箇所ではないため） |
| 性能（間接呼び出し） | 静的ディスパッチ（enum の `match`） | 動的ディスパッチ（vtable 経由の間接呼び出し）。18 種程度・画面あたりせいぜい数十〜数百ウィジェットの規模では無視できるコストと判断する（§6 未解決事項に性能計測を残す） |

**推奨理由**: 18 種という規模と将来の追加可能性（デザイントークン統合 TASK-8.4、
IME 統合 TASK-8.5 後にさらにウィジェットが増える可能性）を踏まえると、`match`
網羅性の伝播コストが動的ディスパッチのコストを上回ると判断する。既存 4 バリアント
（`Text`/`TextField`/`Button`/`List`）は移行コスト（S2〜S60 の並行実装との衝突回避）
の観点から**温存**し、新規ウィジェットのみ `Node::Widget` 経由でトレイトオブジェクト化
する段階的移行とする。

**代替案・トレードオフ**: enum バリアント継続案・完全トレイトオブジェクト化案
（既存 4 バリアントも含めて `Node::Widget` へ統合）は §6 で扱う。

**`Debug` 手動実装との整合（未対応の穴の解消）**: `node.rs` L98-111 の
`impl<S> std::fmt::Debug for Node<S>` は全バリアントを網羅する手動実装であり、
`Node::Widget(Box<dyn Widget<S>>)` を追加すると同じ問題が `Debug` にも起きる
（`dyn Widget<S>` は `Self: Sized` を要求する標準の `Debug` derive を持てない）。
`clone_box`（§4.3）と同様の対処として、`Widget<S>` を **`std::fmt::Debug` を
スーパートレイトに持つ**設計にする（`pub trait Widget<S>: std::fmt::Debug`）。
`std::fmt::Debug` はオブジェクトセーフであり、`Box<dyn Widget<S>>` は
`dyn Widget<S>: Debug` を経由してそのまま `{:?}` 整形できるため、専用の
`debug_name` メソッドを別途持たせる必要はない。`Node<S>` の手動 `Debug` 実装は
`Node::Widget(widget) => f.debug_tuple("Widget").field(widget).finish()` の形で
拡張する（S2 が実装時に反映する）。

### 4.2 `WidgetId`: label 文字列一致からの脱却

**判断**: ウィジェット実装層の各実体に、宣言時に確定する識別子 `WidgetId` を
持たせる。`WidgetId` は「同一画面内でのツリー上の位置」（親からの子インデックス列を
`u64` へ折り畳んだ値等）から導出する安定な値とし、**表示文言（ラベル）とは独立した値**
にする。文字列キー（宣言側が明示的に指定する `&'static str` 等）をハッシュ化して
`WidgetId` にする方式は、本設計のスコープでは**採用しない**。文字列同士のハッシュ
衝突リスクを持ち込むと、§2 で問題視した「ラベル一致によるディスパッチ対象の
曖昧化」と同種の課題を再導入することになるため。位置ベースの導出方法の具体化
（子インデックス列 → `u64` への折り畳みアルゴリズム）は S2〜S3 のスコープとする。

- **現状の課題（§2）への対応**: `event::click_button` の「ラベル一致・先頭 1 件」
  という割り切りを、`WidgetId` によるツリー位置ベースの一意特定へ置き換える
  方針を S3 へ引き継ぐ。ラベルは表示文言としてのみ使い、ディスパッチキーとして
  外部由来の文字列を経路に含めない（OWASP A03 対策、§7）。
- **後方互換**: 既存 4 バリアント（`Button`/`TextField`）の `label` フィールドは
  表示文言として残し、`event::click_button` のラベル一致 API は既存挙動を壊さない
  ため当面併存させる。新設する `Widget<S>` 経由のディスパッチ（S3）は `WidgetId`
  方式に一本化する。
- **`ListRecycler`（`recycle.rs`）との整合**: 同モジュールは「インデックス
  （`usize`）をキーにノードを再利用する」設計を既に持つ。`WidgetId` はこの
  先行実装の「ツリー位置ベースのキー」という考え方を、リスト項目だけでなく
  ウィジェット全般へ一般化したものと位置づける。

### 4.3 共通トレイト `Widget<S>` のシグネチャ案（S2〜S4 が実装する契約）

`fandhe-core` に新モジュール `ui/widget.rs`（案）を追加し、以下の型・トレイトを置く。
本シグネチャは現行 `Screen<S>` / `Node<S>` / `crate::state::ReadFn` と型レベルで
矛盾しないことを確認済み（`node.rs` の `ReadFn<S, T>` 定義・関数ポインタ限定方針を
踏襲）。

```rust
/// ウィジェット実体を画面ツリー上で一意に特定する識別子（TASK-8.3-S1）。
///
/// `label` のような表示文言とは独立させ、イベントディスパッチ（S3）・
/// マウント/reconcile（S1 本文書 §4.4）のキーとして用いる。
/// ツリー上の位置から導出する安定な値であり、ユーザー入力由来の文字列を
/// 直接キーにしない（OWASP A03 対策、`security.md`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetId(u64);

/// レイアウトエンジン（S4、`taffy` 導入見込み）へ渡す計測結果のプレースホルダ型。
/// `taffy::geometry::Size<f32>` 等への対応は S4 が確定する（本文書はプレースホルダ
/// までを契約とする）。
pub struct Measurement {
    pub width: f32,
    pub height: f32,
}

/// 描画プリミティブへの変換結果を表すプレースホルダ型。
/// `render_headless`（TASK-1.1-S2）との両立を前提に、ヘッドレス環境でも
/// 生成可能な列とする（wgpu 等の具体描画 API には依存しない）。
pub enum PaintCommand {
    /// テキスト描画（内容・位置は各ウィジェット実装が確定する）。
    Text(String),
    /// 矩形描画（背景・枠線等、詳細は S4/S2 が確定する）。
    Rect,
}

/// アクセシビリティロール（REQ-9 / accesskit 接続点）。
/// `accesskit::Role` への 1:1 マッピングを想定するが、`fandhe-core` は
/// `accesskit` に直接依存しない（依存方向規約、coding-rust.md）ため、
/// 独自の列挙として定義し、プラットフォームレイヤ側で `accesskit::Role` へ変換する。
///
/// 変換実装は `fandhe-egui-proxy::accessibility`（`to_accesskit_role` /
/// `widget_ax_node`、TASK-9.1-S2）が担う。ターゲット非依存の純粋関数として提供し、
/// egui/eframe の "accesskit" feature 有効化状況とは独立に全ターゲットで
/// コンパイル・テスト可能とする（同 feature に依存する `ctx.accesskit_node_builder`
/// への実配線は `render_screen` 側で macOS 限定（`#[cfg(target_os = "macos")]`）に
/// 閉じる、TASK-9.1-S1 が有効化する eframe 側の feature が前提）。
pub enum AccessibilityRole {
    Button,
    TextInput,
    List,
    // 18 種のウィジェットに応じて S2〜S60 が拡張する。
}

/// ウィジェットが一時的に持つインタラクション状態（S2 が実装）。
/// アプリ状態 `S` には含めない（§3 二層モデル）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionState {
    pub hovered: bool,
    pub pressed: bool,
    pub disabled: bool,
    pub focused: bool,
}

/// ウィジェット実装層の共通契約（TASK-8.3-S1 本設計の核心）。
///
/// `Node::Widget(Box<dyn Widget<S>>)`（§4.1）が保持する実体が実装する。
/// メソッドはすべて `&self`/`&mut self` 経由（ジェネリック引数を取らない）に
/// 限定し、object safety を満たす。`std::fmt::Debug` をスーパートレイトに持つ
/// ことで、`Node<S>` の手動 `Debug` 実装（`node.rs`）を「その場で読める」形で
/// 拡張できるようにする（§4.1「`Debug` 手動実装との整合」参照。`Debug` は
/// オブジェクトセーフなためこの制約を課しても object safety は保たれる）。
pub trait Widget<S>: std::fmt::Debug {
    /// このウィジェット実体のツリー上の識別子。
    fn id(&self) -> WidgetId;

    /// アプリ状態 `S` から表示に必要な値を読み取り、内部表示用キャッシュを
    /// 更新する（reconcile フェーズ、§4.4）。宣言層から渡された関数ポインタ
    /// （`fn(&S) -> T` 相当、REQ-13 制約下）のみを介して `S` に触れる。
    fn sync_from_state(&mut self, state: &S);

    /// レイアウトエンジン（S4）から与えられた制約のもとで自身の必要寸法を返す。
    fn measure(&self, available: Measurement) -> Measurement;

    /// 確定した配置矩形をもとに、描画プリミティブ列を生成する。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand>;

    /// ヒットテスト・イベント配送（S3）の対象になるかを判定する。
    /// 実際のディスパッチは `StateStore::dispatch` 相当の呼び出し元が
    /// `WidgetId` 一致で行う想定（§4.2）。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool;

    /// このウィジェットのアクセシビリティロール（REQ-9 接続点）。
    fn accessibility_role(&self) -> AccessibilityRole;

    /// 現在のインタラクション状態（S2 が更新・参照）。
    fn interaction_state(&self) -> InteractionState;

    /// オブジェクトセーフな複製（§4.1: `dyn Widget<S>` は標準の `Clone` を
    /// 実装できないための代替）。
    fn clone_box(&self) -> Box<dyn Widget<S>>;
}
```

`unimplemented!()` を含む具体実装は行わない（本文書はシグネチャ契約のみを
確定させ、実装は S2〜S60 が担う）。トレイト定義自体に `unsafe` は一切登場せず、
`#![forbid(unsafe_code)]`（`fandhe-core` 既定方針）と矛盾しない。

### 4.4 ライフサイクル状態機械

宣言 → mount → update（reconcile）→ layout → paint → event → unmount の
7 状態を経る状態機械として定義する。

```mermaid
stateDiagram-v2
    [*] --> Declared: Screen<S> ビルダーで宣言（都度評価）
    Declared --> Mounted: 初回描画走査時に WidgetId で実体を新規生成
    Mounted --> Updated: sync_from_state（reconcile: 既存実体を WidgetId で照合・再利用）
    Updated --> Updated: 状態変化のたびに再度 sync_from_state
    Updated --> Layout: measure 呼び出し（S4 のレイアウトプロトコル）
    Layout --> Paint: 確定した配置矩形で paint 呼び出し
    Paint --> Event: hit_test / イベントディスパッチ（S3）
    Event --> Updated: イベント発火 → StateStore::dispatch → version 増加 → 次フレームで再度 sync_from_state
    Event --> Unmounted: 宣言側から当該 WidgetId が消える（画面遷移・List 項目の可視範囲外離脱等）
    Unmounted --> [*]
```

- **宣言（Declared）**: `Screen<S>` ビルダーによる都度評価。現行 `Node::List::expand`
  と同様、この段階では実体（ウィジェット実装層）は生成されない。
- **mount**: 描画走査（`visit_expanded` 相当）が `WidgetId` に対応する実体を
  持たない場合に新規生成する。`ListRecycler`（`recycle.rs`）の「キャッシュミス時に
  `expand_item` で新規構築する」という既存挙動と同じ考え方を全ウィジェットへ
  一般化したもの。
- **update（reconcile）**: `WidgetId` で宣言と既存実体を照合し、一致すれば
  `sync_from_state` のみを呼んで実体を再利用する（`ListRecycler` の再利用条件
  ─ 状態バージョン不変なら再構築しない ─ と同じ判断軸を踏襲）。
- **layout**: `measure` を呼ぶ。S4 のレイアウトプロトコル（`taffy` 導入見込み、
  REQ-8）が実際の呼び出し元になる。
- **paint**: 確定した配置矩形をもとに `paint` を呼び、描画プリミティブ列を得る。
  `render_headless`（TASK-1.1-S2）との両立を前提とする（§4.3 `PaintCommand`）。
- **event**: ヒットテスト・ディスパッチ（S3）。発火時は `StateStore::dispatch` を
  経由し、`version` が増加する（既存 `store.rs` の仕組みをそのまま利用）。
  イベント処理後は再び update フェーズへ戻り、次フレームで `sync_from_state` が
  呼ばれる。
- **unmount**: 宣言側（`Screen<S>` の走査結果）から当該 `WidgetId` が消えたとき
  （画面遷移、`List`/`VirtualList` の項目が可視範囲外へ出た等）に実体を破棄する。
  `ListRecycler` の「ウィンドウ外へ出た項目のスロットを解放する」という既存の
  有界性設計と同じ考え方を踏襲する。

**呼び出し元**: mount/update/layout/paint はプラットフォームレイヤの描画走査
（`fandhe-egui-proxy::render_screen` 相当）が毎フレーム駆動する。event は
プラットフォームレイヤの入力ハンドリングが `StateStore::dispatch` を経由して駆動する。

**既存 `RendererLifecycle`（`render_mode.rs`）との区別**: `RendererLifecycle` は
「画面単位の出力方式（Canvas/DOM）が起動しているか」という**レンダラ全体**の
2 状態（活性/非活性）を扱う抽象であり、粒度が異なる。本設計の 7 状態機械は
**個々のウィジェット実体**のライフサイクルを扱う。命名衝突を避けるため、本設計の
トレイトには `activate`/`deactivate` という語を用いず、`mount`/`sync_from_state`/
`unmount` という別語彙を採用する。

## 5. S2〜S6 への接続点

- **S2（スタイリング状態）**: `InteractionState`（§4.3）の hover/pressed/disabled/focus
  各フィールドを、入力イベント（ポインタ移動・押下等）に応じてどう更新するかを
  実装する。`Widget<S>::interaction_state` / `sync_from_state` を拡張点とする。
- **S3（イベントディスパッチ・ヒットテスト）**: `WidgetId`（§4.2）による対象特定と
  `Widget<S>::hit_test`（§4.3）を用いたヒットテストを実装する。`StateStore::dispatch`
  との配線（`version` 増加後の update フェーズ復帰、§4.4）を行う。
- **S4（レイアウトプロトコル）**: `Measurement`（§4.3）のプレースホルダ型を
  `taffy` の型（`taffy::geometry::Size<f32>` 等、REQ-8）へ具体化し、
  `Widget<S>::measure` の呼び出し元（レイアウトツリー構築・制約解決）を実装する。
- **S5（テストハーネス）**: §4.4 の 7 状態機械（宣言 → mount → update → layout →
  paint → event → unmount）と、`ListRecycler` 同様の再利用条件（状態バージョン
  不変なら reconcile のみ・変化があれば `sync_from_state` を呼ぶ）をテストケース化する。
- **S6（ウィジェットカタログ）**: `Widget<S>` を実装する具体型（S7〜S60、18 種）を
  列挙・登録する仕組みを実装する。`AccessibilityRole`（§4.3）の一覧が
  カタログのメタ情報として使える。
- **S7〜S60（個別ウィジェット、18 種 × 3 工程）**: 各ウィジェットは `Widget<S>`
  トレイト（§4.3）を実装する具体型として設計・実装・テストされる。宣言層 API
  （`Screen<S>::button()` 相当のビルダーメソッド）は各ウィジェットの sub-issue が
  個別に追加する。

## 6. トレードオフ・代替案

| 案 | 内容 | 不採用理由 |
|----|------|-----------|
| enum バリアント継続案 | 18 種すべてを `Node<S>` の enum バリアントとして追加する | §4.1 のとおり、`match` 網羅性の伝播コストが 18 種規模で線形に増え、共通のインタラクション状態（hover/pressed 等）をバリアントごとに重複定義することになる。静的ディスパッチによる性能は維持できるが、保守コストが動的ディスパッチのコストを上回ると判断した |
| 完全トレイトオブジェクト化案 | 既存 4 バリアント（`Text`/`TextField`/`Button`/`List`）も含めて `Node::Widget` へ統合する | 並行実行中の他 sub-issue（S2〜S60）・既存 `Screen<S>` ビルダー API・`event::click_button`（label 一致）・`ListRecycler`（index キー）との互換性を保つ移行コストが大きく、TASK-8.3-S1 単体（設計文書のみ、コード変更なし）のスコープを超える。既存 4 バリアントは温存し、新規ウィジェットのみトレイトオブジェクト化する段階的移行を採用した |
| `WidgetId` を `label: String` のまま維持する案 | 識別子を新設せず、既存のラベル一致検索を 18 種へそのまま拡張する | §2 のとおり、ラベル衝突可能性の増大とディスパッチキーへの外部由来文字列混入（OWASP A03）の懸念があり、18 種規模への拡張には不十分と判断した |

## 7. セキュリティ考慮（OWASP Top 10 観点）

本 issue はドキュメントのみの変更でありランタイムコード・依存・CI 設定に触れないため、
直接の攻撃面は増えない。設計文書の内容として以下を明記する。

- **A03 インジェクション**: 現行のイベントディスパッチ（`event::click_button`）が
  `label: String` の一致に依存している課題（§2）を明記し、`WidgetId`（§4.2）に
  よる「ツリー位置由来の安定値」へ移行することで、ユーザー入力由来の文字列を
  ディスパッチキーとして解釈する経路を縮小する方針を S3 実装時の検査観点として
  引き継ぐ。
- **メモリ安全性**: 本設計のトレイト・状態機械はいずれも `unsafe` を要求する要素を
  持ち込まない。`#![forbid(unsafe_code)]`（`fandhe-core` 既定方針）を維持する前提を
  §4.3 に明記した。
- **A06 脆弱な依存**: 依存追加は行わない。`taffy`（S4）・`cosmic-text`（テキスト
  描画、S2 以降）・`accesskit`（S3 以降、REQ-9 系）の導入判断は本 issue のスコープ外
  であることを§8 に明記し、導入時の `cargo audit` 確認を未解決事項として残す。
- **A05 セキュリティ設定ミス**: 本 issue では CI・ビルド設定を変更しない。

## 8. スコープ外（明記）

- 共通トレイト・ライフサイクルの Rust 実装（S2〜S6 のスコープ。本 issue は
  §4.3 のシグネチャ案・§4.4 の状態機械までに留める）。
- 個別ウィジェットの API 設計・実装（S7〜S60、18 種 × 3 工程）。
- `taffy` / `cosmic-text` / `accesskit` の依存追加（S4 以降・REQ-9 系タスクの
  スコープ。導入時の `cargo audit` 確認は未解決事項として残す）。
- デザイントークン（TASK-8.4）・IME 統合（TASK-8.5）との統合（接続点の言及に留める）。
- 既存 `Node<S>` 4 バリアント（`Text`/`TextField`/`Button`/`List`）・
  `event::click_button`・`ListRecycler` の実装変更（本設計は温存方針のみを示し、
  実際の統合判断は S2〜S60 の各実装時に個別に行う）。
- 静的ディスパッチ（enum）と動的ディスパッチ（`dyn Widget<S>`）の性能差の実測
  （§6 の判断は定性評価に基づくものであり、実測は S2 以降・性能確認タスクへ委ねる）。

## 9. 未解決事項・将来課題

- **動的ディスパッチの性能実測**: §6 で「18 種程度・画面あたり数十〜数百
  ウィジェット規模では無視できるコスト」と定性評価したが、実測（フレームタイム
  計測）は行っていない。S5（テストハーネス、#100）・個別ウィジェット実装（S7〜S60）
  の性能確認タスクで実測し、無視できない場合は enum バリアント継続案（§6）への
  部分回帰を再検討する。
- **`taffy` / `cosmic-text` / `accesskit` の導入タイミングと `cargo audit`**:
  §4.3 の `Measurement` / `PaintCommand` / `AccessibilityRole` はプレースホルダ型
  であり、これらのクレートを実際に依存へ追加するのは S4（レイアウトプロトコル、
  #99）・S2 以降（テキスト描画）・S3 以降（REQ-9 系、アクセシビリティ橋渡し）の
  スコープとする。依存追加時は `cargo audit` による既知脆弱性確認を行うこと
  （`security.md` A06）。
- **`WidgetId` の具体的な導出アルゴリズム**: §4.2 でツリー位置（子インデックス列）
  由来の `u64` とする方針は確定したが、折り畳みアルゴリズムの具体化・衝突しない
  ことの保証は S2〜S3 のスコープとする。
- **ネストしたウィジェット実装層とツリー位置の対応**: `Node::List` /
  `VirtualList`（TASK-5.5 系）配下に `Node::Widget` が入れ子になる場合の
  `WidgetId` 導出（親のツリー位置 + 子インデックス）は本設計で扱いきれておらず、
  S3（イベントディスパッチ、#98）実装時に再確認する。
- **完全トレイトオブジェクト化への移行判断**: §6 で不採用とした「既存 4 バリアント
  も `Node::Widget` へ統合する案」は、S60 完了時点（18 種すべて `Widget<S>` 実装
  済み）で改めてコスト・ベネフィットを再評価する余地を残す。
