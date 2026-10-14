# デザイントークン定義スキーマ・型設計

## 1. 目的・位置づけ

本文書は #156（TASK-8.4「デザイントークン基盤実装」、REQ-8、推定工数 12 日）の
24 sub-issue 分解のうち最初の 1 件である #157（TASK-8.4-S1「トークン定義スキーマ・
型設計」）の成果物であり、**設計文書 1 本のみを成果物とし、コード変更は行わない**。

precedent は以下のとおりで、いずれも「S1 は設計、S2 以降が実装」という分離パターンを
踏襲している。

- TASK-8.3-S1（#96 → commit 3e13f41 `feat(core): TASK-8.3-S1 ウィジェット共通トレイト・
  ライフサイクル設計`）: `docs/architecture/widget-architecture.md` を設計文書のみで
  納品し、S2（#97）が「初めて Rust コード化するモジュール」として実装した
- TASK-4.4-S1（#79）・TASK-5.5-S1（#84）も同パターン
- TASK-8.4 の分解自体でも S2「解決エンジン**設計**」（#158）→ S3「同**実装**」（#159）、
  S4「テーマ切替**設計**」（#160）→ S5「同**実装**」（#161）と設計/実装が分離されており、
  S1（#157）は設計フェーズに属する
- #157 の受け入れ条件は「『トークン定義スキーマ・型設計』が完了し、親 issue #156 の
  受け入れ条件に寄与する」という契約・設計の完了を問う文面であり、具体的なコード成果物
  （`crates/fandhe-core/src/tokens/` の追加等）を名指ししていない。実装は #158 以降に
  委ねる

### spec 対応

- **REQ-8**（`docs/spec/04-requirements.md` L114）: 「`wgpu`（描画）・`taffy`
  （レイアウト）・`cosmic-text`（テキスト）・`winit`（ウィンドウ/入力）・`accesskit`
  （アクセシビリティ橋渡し）を基盤として採用し、その上にウィジェットセット・
  デザイントークン・IME 統合・ストア提出パイプラインを新規開発する」。本文書は
  このうち「デザイントークン」領域の設計を担う。
- **TASK-8.4**（`docs/spec/05-tasks.md` L274-279）: 成果物「デザイントークン基盤
  モジュール」、推定工数 12 日、前提タスク TASK-1.1、担当「Claude Code（分野:
  コア Rust）」。
- **REQ-2**（同 L42 以降）: 共通コア（`fandhe-core`）とプラットフォーム最適化レイヤーの
  二層アーキテクチャ、依存方向は最適化レイヤー → コアの一方向。本設計のトークン型は
  `fandhe-core` に置き、プラットフォーム固有クレートへは依存しない。
- **REQ-13**（同 L171 以降）: 状態アクセスは `fn(&S) -> T` / `fn(&mut S, T)` の
  関数ポインタに限定し、クロージャによる環境キャプチャを型システムで拒否する。
  本設計が提案するトークン関連 API（アクセサ・解決関数のシグネチャ案）はいずれも
  クロージャを取らない設計とし、この制約を維持する（§5.3）。

### #157〜#180 の分担表

`gh api repos/Fandhe-AI/fandhe-multi-platform/issues/156/sub_issues` で取得した実際の
24 件の子 issue（#157〜#180）に基づく。

| sub-issue | 内容 | 本文書との関係 |
|-----------|------|----------------|
| #157（本文書・S1） | トークン定義スキーマ・**型設計** | 本文書がその成果物 |
| #158（S2） | トークン解決エンジン（参照・エイリアス）**設計** | 本文書 §4.4 の参照・エイリアス意味論を前提に設計する |
| #159（S3） | トークン解決エンジン**実装** | #158 の設計を実装。本文書 §5 の `TokenRef` / `TokenSet` 型を使う |
| #160（S4） | テーマ切替機構（ライト/ダーク）**設計** | 本文書 §5.2 の `Theme` プレースホルダを前提に詳細設計する |
| #161（S5） | テーマ切替機構**実装** | #160 の設計を実装 |
| #162（S6） | 色トークン: パレット定義・型実装 | 本文書 §4.1 の color カテゴリ・§5.2 の `ColorValue` を実装 |
| #163（S7） | 色トークン: セマンティックカラー対応 | 本文書 §4.4 のエイリアス意味論（セマンティック → パレット参照）を実装 |
| #164（S8） | タイポグラフィトークン: フォントファミリ・サイズ定義 | 本文書 §4.1 の typography カテゴリ・§5.2 の `TypographyValue` を実装 |
| #165（S9） | タイポグラフィトークン: 行間・ウェイト・スタイル対応 | 同上、`TypographyValue` の残余フィールドを実装 |
| #166（S10） | スペーシングトークン実装 | 本文書 §4.1 の spacing カテゴリ・§5.2 の `SpacingValue` を実装 |
| #167（S11） | サイズ・radius・elevation トークン実装 | 本文書 §4.1 の size/radius/elevation カテゴリを実装 |
| #168（S12） | ボーダー・シャドウトークン実装 | 本文書 §4.1 の border/shadow カテゴリを実装 |
| #169（S13） | アニメーション・duration トークン実装 | 本文書 §4.1 の animation/duration カテゴリを実装 |
| #170（S14） | 基本ウィジェットへのトークン適用配線 | 本文書 §5.4 の `StateStyles<TokenRef>` 接続点・`PaintCommand` 拡張方針を実装 |
| #171（S15） | コンテナ・レイアウトへのトークン適用配線 | 本文書 §5.4 の接続点を `layout` 系（`taffy` スタイル）へ配線 |
| #172（S16） | プラットフォーム別既定テーマ（iOS/Android 準拠値）定義 | 本文書 §5.2 の `Theme` に対する具体値セットを定義 |
| #173（S17） | トークンのシリアライズ・外部定義ファイル読み込み | 本文書 §6 未解決事項（serde 導入可否）を判断・実装 |
| #174（S18） | トークンのバリデーション・エラー報告 | 本文書 §5.2 のキー安全性方針・§6 のバリデーション未確定事項を実装 |
| #175（S19） | AI 型安全 DSL からのトークン参照 API | 本文書 §5.3 の型付きアクセサ方針を実装 |
| #176（S20） | トークンドキュメント自動生成 | 本文書 §4 のスキーマ・命名規則を機械可読化して文書生成する |
| #177（S21） | ユニットテスト（解決エンジン） | #158/#159 の実装に対するテストを追加 |
| #178（S22） | ユニットテスト（テーマ切替・適用） | #160/#161/#170/#171 の実装に対するテストを追加 |
| #179（S23） | サンプル・カタログへのテーマ適用デモ | `widget_gallery` 等の既存サンプルへトークン適用例を追加 |
| #180（S24） | 設計文書・使用ガイド作成 | 本文書を土台に利用者向けガイドを作成 |

S2〜S13 は本文書の §4（スキーマ）・§5（型設計）の該当カテゴリを前提に着手できる粒度を
目標とする。

## 2. 現状分析

既存のスタイリング関連資産（本設計が接続すべき既存契約）は以下のとおり
（`crates/fandhe-core/src/ui/widget.rs`、TASK-8.3-S2 / #97）。

- **`VisualState`**（同ファイル L277 付近）: `Idle` / `Hovered` / `Pressed` /
  `Disabled` の 4 状態を持つ列挙。ウィジェットの一時的 UI 状態から見た目解決に使う
  優先順位付き状態を表す。
- **`StateStyles<T>`**（同 L295 付近）: `base: T` と状態別オーバーライド
  （`hovered` / `pressed` / `disabled`）を持つ汎用構造体。`resolve(&self, visual:
  VisualState) -> &T` で「状態 → 見た目の値」を解決する。`T` はジェネリクスで
  「見た目の値」の型を未確定のまま受けており、トークン型の差し込み先として
  設計済みである（コメントに明記あり: 「状態 → 見た目を解決する共通スタイリング
  基盤」）。
- **`PaintCommand`**（同 L67 付近）: `Text(String)` / `Rect` の 2 バリアントの
  プレースホルダ列挙。色・フォント等の具体値は未定義。「ヘッドレス環境でも生成可能な
  列とする（wgpu 等の具体描画 API には依存しない）」という制約を持つ。
- **`docs/architecture/widget-architecture.md` §8「スコープ外」**: 「デザイントークン
  （TASK-8.4）・IME 統合（TASK-8.5）との統合（接続点の言及に留める）」と明記済み —
  本文書がその接続点を確定させる番である。
- **`crates/fandhe-core/src/ui/layout/style.rs`**: `taffy` ベースのレイアウトスタイル
  （寸法・配置）を扱うモジュール。寸法系トークン（spacing / size）がここへ接続する
  可能性を持つが、詳細配線は S15（#171）のスコープとする。

`fandhe-core` は `#![forbid(unsafe_code)]` 原則・プラットフォーム非依存（依存方向:
最適化レイヤ → コアの一方向）・`taffy` のみランタイム依存（`Cargo.toml` 確認済み）。
トークン型もこの制約下で設計する。

## 3. 対象ファイル・変更箇所

| ファイル | 種別 | 内容 |
|---------|------|------|
| `docs/architecture/design-token-architecture.md` | **新規** | トークン定義スキーマ・型設計の設計文書（本 issue の唯一の成果物） |

コード（`crates/`）・CLAUDE.md・spec（読み取り専用）は変更しない。

## 4. トークンスキーマ設計

### 4.1 カテゴリ体系

`TokenCategory`（§5.2）が定義する 10 バリアント（color / typography / spacing /
size / radius / elevation / border / shadow / animation / duration）を、S6〜S13
の 8 件の実装 sub-issue へ以下のとおり割り当てる（カテゴリ数と sub-issue 数は
1:1 ではない。color は S6/S7 の 2 issue に分割し、size/radius/elevation は
S11 の 1 issue にまとめる）。

| カテゴリ | 内容 | 対応 sub-issue |
|---------|------|----------------|
| `color` | パレット（基礎色）+ セマンティックカラー（役割別エイリアス） | S6（#162）/ S7（#163） |
| `typography` | フォントファミリ・サイズ / 行間・ウェイト・スタイル | S8（#164）/ S9（#165） |
| `spacing` | 余白・間隔 | S10（#166） |
| `size` / `radius` / `elevation` | サイズ・角丸・高さ（z 軸的な階層表現） | S11（#167） |
| `border` / `shadow` | 枠線・影 | S12（#168） |
| `animation` / `duration` | アニメーション・時間長 | S13（#169） |

カテゴリはトップレベルの名前空間として扱い、S14〜S16 が消費する `TokenSet`
（§5.2）はこの 10 カテゴリの合成として構成される。

### 4.2 命名規則

`category.group.name` 形式の階層命名とする（例: `color.primary`、
`color.palette.blue-500`、`spacing.md`、`typography.body.size`）。

- `category` は §4.1（`TokenCategory`、§5.2）の 10 種のいずれか
- `group` はカテゴリ内の意味的なまとまり（例: color の `palette` / セマンティック
  役割名、typography の用途別スケール名）
- `name` は個別トークン名。ハイフン区切りの小文字とする（例: `blue-500`）

参考として W3C Design Tokens Community Group（DTCG）形式のドット区切り階層命名
（`$type` / `$value` によるトークン定義）との対応関係を意識した命名とする。
DTCG 形式（JSON）そのものの採用可否は外部ファイル読み込み（S17、#173）の
スコープであり、本文書は命名規則の互換性のみを確認する。

### 4.3 値モデル

各カテゴリの値型と単位の扱いは以下のとおり（詳細な Rust 型は §5.2 で定義）。

| カテゴリ | 値の性質 | 単位・表現 |
|---------|---------|-----------|
| color | RGBA | 8bit 整数チャンネル（`0..=255`）を正とし、f32 正規化値（`0.0..=1.0`）への変換 API を併せ持つ。`wgpu` 等の描画バックエンドは f32 系を要求することが多いため |
| typography | フォントファミリ名（文字列）・サイズ・行間・ウェイト・スタイル | サイズ・行間は論理ピクセル相当の f32、ウェイトは `100..=900` 相当の数値域、スタイルは normal/italic 等の閉じた列挙 |
| spacing / size / radius | 寸法 | 論理ピクセル相当の f32（`geometry.rs` の `Point` / `Size` と単位系を揃える） |
| elevation | 階層順位 + 影の強さの示唆 | 整数の階層値（0 から始まる段階）。実際の陰影表現は shadow カテゴリと合成する |
| border | 幅・スタイル・色参照 | 幅は f32、スタイルは閉じた列挙、色は `color` カテゴリへの `TokenRef`（§4.4） |
| shadow | オフセット・ぼかし半径・色参照 | オフセット/ぼかしは f32、色は `color` カテゴリへの `TokenRef` |
| animation / duration | 時間長・イージング | 時間はミリ秒相当の f32（またはミリ秒整数）、イージングは閉じた列挙（曲線名） |

### 4.4 参照・エイリアスの意味論

トークンはトークンを参照できる。スキーマ上は「あるトークンの値が、別のトークンの
キーへの参照である」という表現を持つ（例: セマンティックカラー `color.primary` が
パレット `color.palette.blue-500` を参照する）。

- 参照はキー（§4.2 の階層命名）による間接参照とし、値のコピーではない
- 参照先は同一カテゴリ内に限定しない（例: border の色フィールドが color
  カテゴリを参照する、§4.3）が、参照先の値型はフィールドが要求する型と一致する
  ことをスキーマ上の制約とする（例: 色を要求するフィールドは color カテゴリの
  トークンのみを参照できる）
- 循環参照は許容しない意味論とする。ただし**解決アルゴリズム・循環検出の実装は
  S2（#158）/ S3（#159）のスコープ**とし、本文書はスキーマ表現（「参照は許される」
  「循環は禁止」という契約）の確定までに留める

## 5. Rust 型設計

S3（#159）/ S5（#161）/ S6〜S13（#162〜#169）が実装するシグネチャ案。

### 5.1 モジュール配置案

`crates/fandhe-core/src/tokens/` を新設する。`crates/fandhe-core/src/lib.rs`
（L431-432）は現時点で `pub mod state;` / `pub mod ui;` の 2 モジュールのみを
公開しており、`tokens` は `state` / `ui` と並ぶ第 3 のトップレベルモジュールと
位置づける。

依存方向は `ui` → `tokens` の一方向とする（`ui` 側の `StateStyles<T>` が
`T = TokenRef` としてトークン型を参照するが、`tokens` 側は `ui` の型を知らない）。
プラットフォームレイヤ・AI 型安全 DSL（S19、#175）もこの `tokens` モジュールを
直接参照できる想定とする。

```
crates/fandhe-core/src/tokens/
├── mod.rs        # tokens モジュール入口・公開 API の再エクスポート
├── key.rs        # TokenKey（階層名の型安全表現）
├── value.rs       # TokenValue（カテゴリ別値の閉じた enum）・カテゴリ別値型
├── reference.rs   # TokenRef（エイリアス参照）
├── set.rs         # TokenSet（1 テーマ分の集合）
└── theme.rs        # Theme（ライト/ダーク切替の単位、詳細設計は S4/#160）
```

（実際のファイル分割は S3 以降の実装判断に委ねる。本文書はモジュール境界の
方針のみを確定する。）

### 5.2 コア型案

```rust
/// トークンの階層キー（`category.group.name`、§4.2）を型安全に表現する。
///
/// 文字列の実行時解決に閉じず、カテゴリごとの型付きアクセサ（§5.3）を
/// 基本経路とする。外部定義ファイル（S17、#173）からの読み込み時など、
/// 文字列由来のキーを扱う経路は検証付き（S18、#174）に限定する
/// （OWASP A03 対策、`.claude/rules/security.md`）。
pub struct TokenKey {
    category: TokenCategory,
    group: String,
    name: String,
}

/// §4.1 の 10 カテゴリに対応する閉じた列挙（S6〜S13 の 8 件の実装
/// sub-issue には 1:1 対応しない。詳細は §4.1 の割り当て表を参照）。
pub enum TokenCategory {
    Color,
    Typography,
    Spacing,
    Size,
    Radius,
    Elevation,
    Border,
    Shadow,
    Animation,
    Duration,
}

/// カテゴリ別の値を保持する閉じた enum（§4.3 の値モデルに対応）。
pub enum TokenValue {
    Color(ColorValue),
    Typography(TypographyValue),
    Spacing(SpacingValue),
    Size(SizeValue),
    Radius(RadiusValue),
    Elevation(ElevationValue),
    Border(BorderValue),
    Shadow(ShadowValue),
    Animation(AnimationValue),
    Duration(DurationValue),
    /// 値そのものではなく、他トークンへの参照（§4.4）。
    Reference(TokenRef),
}

/// 色トークンの値（8bit チャンネル正、f32 正規化への変換 API を併設、§4.3）。
pub struct ColorValue {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// タイポグラフィトークンの値（S8/S9 が段階的に実装）。
pub struct TypographyValue {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
    pub weight: u16,
    pub style: FontStyleValue,
}

pub enum FontStyleValue {
    Normal,
    Italic,
}

/// 寸法系トークンの共通表現（spacing / size / radius が同型を使う想定）。
pub struct DimensionValue {
    pub value: f32,
}

pub type SpacingValue = DimensionValue;
pub type SizeValue = DimensionValue;
pub type RadiusValue = DimensionValue;

/// 階層順位を表す（§4.3）。
pub struct ElevationValue {
    pub level: u8,
}

/// 枠線トークン（色は `TokenRef` による color カテゴリ参照、§4.4）。
pub struct BorderValue {
    pub width: f32,
    pub style: BorderStyleValue,
    pub color: TokenRef,
}

pub enum BorderStyleValue {
    Solid,
    Dashed,
}

/// 影トークン（色は `TokenRef` による color カテゴリ参照、§4.4）。
pub struct ShadowValue {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: TokenRef,
}

/// アニメーション時間長（S13/#169）。
pub struct DurationValue {
    pub milliseconds: f32,
}

pub struct AnimationValue {
    pub duration: DurationValue,
    pub easing: EasingValue,
}

pub enum EasingValue {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

/// 他トークンへの参照（エイリアス、§4.4）。値のコピーではなくキーによる
/// 間接参照を保持し、解決（値への展開）は S2（#158）/ S3（#159）が担う。
pub struct TokenRef {
    target: TokenKey,
}

/// 1 テーマ分のトークン集合（キー → 値のマッピング）。
///
/// 解決前（`TokenValue::Reference` を含み得る）と解決後（すべて具体値）の
/// 2 状態を型で区別するかどうかは S2/S3 の設計判断に委ねる。本文書は
/// `TokenSet` が「1 テーマ分のトークン全体を束ねる」責務を持つことのみを
/// 確定する。
pub struct TokenSet {
    // フィールド構成（HashMap<TokenKey, TokenValue> 等）は S3（#159）が確定する。
}

/// ライト/ダーク切替の単位（詳細設計は S4、#160）。
///
/// 本文書ではプレースホルダとしての位置づけのみを示す。
pub struct Theme {
    // フィールド構成は S4（#160）が確定する。
}
```

上記はシグネチャ案であり、フィールドの可視性・具体的なメソッド構成は
S3/S5/S6〜S13 の各実装で確定する。

### 5.3 制約の明記

- `#![forbid(unsafe_code)]`（`fandhe-core` 既定方針、`.claude/rules/coding-rust.md`）
  を満たす。`unsafe` を要求する要素を持ち込まない
- プラットフォーム非依存。`wgpu` / `accesskit` 等の具体描画・OS API には依存しない
- 外部クレート依存を追加しない（`serde` 導入可否は S17、#173 で判断する）
- AI 型安全 DSL からの参照（S19、#175）を見据え、文字列キーの実行時解決に閉じず、
  型付きアクセサ（例: `TokenSet::color(&self, key: ColorTokenKey) -> &ColorValue`
  のような、カテゴリごとに閉じた型を引数に取るアクセサ）を用意する方針とする
- REQ-13（関数ポインタ限定方針）との整合: 本文書が提案するアクセサ・解決関数の
  シグネチャはいずれもクロージャを引数に取らない。トークン解決はデータ変換
  （キー → 値）であり、状態アクセスの `fn(&S) -> T` パターンとは別の関心事だが、
  念のため「クロージャを取る API を追加しない」制約を明記する

### 5.4 `StateStyles<T>` への差し込みと `PaintCommand` との接続点

- `StateStyles<T>` は `T = TokenRef`（または `T = TokenKey`）としてインスタンス化
  できる設計とする。ウィジェットの状態別スタイル（例: ボタンの `Idle` / `Hovered` /
  `Pressed` / `Disabled` それぞれの背景色）を `StateStyles<TokenRef>` として表現し、
  `resolve(visual_state)` で得た `TokenRef` を トークン解決エンジン（S2/S3）が
  `ColorValue` 等の具体値へ解決する、という 2 段階の流れになる
- `PaintCommand` は現状 `Text(String)` / `Rect` の 2 バリアントのみで色・フォントの
  具体値を持たない。S14（#170、基本ウィジェットへのトークン適用配線）が
  `PaintCommand` を拡張し、例えば `Rect` に `color: ColorValue` フィールドを
  追加する、または `Text` に `TypographyValue` 相当のフィールドを追加する形で
  トークンの解決結果（具体値）を運ぶ想定とする。`PaintCommand` が保持するのは
  常に解決済みの具体値（`ColorValue` 等）であり、`TokenRef` を直接保持しない
  （描画バックエンドはトークンの存在を知らずに済む設計とする）
- この接続点の確定的な API 形状（`PaintCommand` の具体的なバリアント定義）は
  S14（#170）のスコープとし、本文書は「解決済み具体値を運ぶ」という契約のみを
  確定する

## 6. 未解決事項

- **解決エンジンの詳細**（アルゴリズム・循環検出・エラー表現）: S2（#158）/
  S3（#159）のスコープ
- **テーマモデル詳細**（ライト/ダーク切替の具体的な API・既定テーマの構成）:
  S4（#160）/ S5（#161）のスコープ
- **外部ファイル形式・`serde` 依存の可否**: S17（#173）のスコープ。導入する場合は
  `cargo audit` による既知脆弱性確認を行う（`.claude/rules/security.md` A06）
- **バリデーション仕様**（不正なキー・型不一致・循環参照の検出とエラー報告形式）:
  S18（#174）のスコープ
- **`PaintCommand` の最終的なバリアント形状**: §5.4 で方針のみ確定。具体的な
  フィールド構成は S14（#170）のスコープ
- **寸法トークンと `taffy` レイアウトスタイルとの配線**: `layout/style.rs` との
  接続は S15（#171）のスコープ。本文書は接続の必要性のみを言及する
  → S15（#171）で実装完了（`LayoutStyle::with_gap_token` / `with_padding_token`・
  `Dimension::from_size`、および `ui/stack_grid.rs` / `ui/card.rs` への
  spacing / size トークン配線）
- **プラットフォーム別既定テーマの具体値**（iOS Human Interface Guidelines /
  Material Design 準拠値）: S16（#172）のスコープ

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

- **A03 インジェクション**: トークンキー・外部定義ファイル（S17）由来の文字列を
  検証なしに描画・ログへ展開しない方針を §5.2（`TokenKey` のドキュメンテーション
  コメント）・§5.3（型付きアクセサ方針）に明記した。キーは型安全表現を基本とし、
  実行時文字列解決は検証付き経路（S18）に限定する
- **A05 セキュリティ設定ミス**: 本文書にデバッグ用の実キー・実 URL・内部ホスト名等は
  含めていない。例示（`color.primary`・`spacing.md` 等）はすべて架空値である
- **A06 脆弱な依存**: 本 issue では依存追加を行わない。§6 に S17（serde 等の導入判断）
  時の `cargo audit` 確認を未解決事項として引き継いだ
- **A08 整合性の不備**: 外部トークン定義ファイルの読み込み（S17）は信頼できない
  入力として扱い、バリデーション（S18）を必須とする前提を §6 に明記した。OTA 的な
  テーマ配信を行う場合は署名検証要件（`.claude/rules/security.md` A08）への準拠が
  必要になる旨を付記する
- **秘密情報の混入防止**: 本文書・コミットに認証情報を含めない（create-commit
  スキルのシークレット検査対象）
- **メモリ安全性**: トークン型は `fandhe-core`（`#![forbid(unsafe_code)]`）内で
  完結し、`unsafe` を導入しない設計であることを §5.3 に明記した

## 8. スコープ外（後続 issue へ委譲）

- トークン解決エンジンの設計・実装 → S2（#158）/ S3（#159）
- テーマ切替機構 → S4（#160）/ S5（#161）
- 各カテゴリの型「実装」（Rust コード化）→ S6〜S13（#162〜#169）
- 基本ウィジェット・コンテナへのトークン適用配線 → S14（#170）/ S15（#171）
- プラットフォーム別既定テーマの具体値定義 → S16（#172）
- シリアライズ・外部ファイル・バリデーション → S17（#173）/ S18（#174）
- AI 型安全 DSL からの参照 API → S19（#175）
- トークンドキュメント自動生成・ユニットテスト・サンプル適用・利用ガイド →
  S20〜S24（#176〜#180）
- 実装中に新たなスコープ外事項を検出した場合は `.claude/rules/out-of-scope-tracking.md`
  のフロー（既存 issue 確認 → 承認 → 起票）に載せ、本 PR に混入させない
