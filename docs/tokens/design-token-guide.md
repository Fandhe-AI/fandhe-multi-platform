# デザイントークン設計文書・使用ガイド

本文書は TASK-8.4-S24（親: #156 TASK-8.4「デザイントークン基盤実装」、REQ-8 /
M3、24/24 sub-issue）の成果物です。TASK-8.4 全体の設計を総括し、`fandhe-core`
のデザイントークン基盤（`crate::token`）を利用するアプリ開発者・ウィジェット
実装者・AI 型安全 DSL 利用者向けの使用ガイドを提供します。

## 1. 目的・対象読者・位置づけ

TASK-8.4 の成果物は以下のとおり領域別に分散しています。本文書はこれらを俯瞰し、
「どこに何が書いてあるか」と「実際にどう使うか」をまとめる橋渡し役です。

| 文書 | 種別 | 内容 |
|------|------|------|
| `docs/architecture/design-token-architecture.md`（S1、#157） | 設計 | スキーマ・型設計（カテゴリ体系・命名規則・値モデル・Rust 型案） |
| `docs/architecture/token-resolution-architecture.md`（S2、#158） | 設計 | 解決エンジン設計（参照構文・解決順序・エラー分類・境界条件） |
| `docs/architecture/theme-switching-architecture.md`（S4、#160） | 設計 | テーマ切替設計（`ThemeVariant` / `Theme` / 切替 API 案） |
| `docs/tokens/design-token-reference.md`（S20、#176） | 自動生成 | コード上のトークン定義から生成する網羅的なリファレンス。**手動編集禁止** |
| `docs/tokens/design-token-guide.md`（本文書、S24、#180） | 手書きガイド | 全体設計の総括 + 使用ガイド |

対象読者:

- **ウィジェット実装者**: `ui/` 配下で新規ウィジェットにトークンを配線する開発者
- **アプリ開発者**: `TokenResolver` を組み立ててアプリへ適用する開発者
- **AI 型安全 DSL 利用者**: `TypedTokenRef<K>` 経由でコンパイル時にカテゴリ検査
  されたトークン参照を書く開発者（REQ-13 整合）

トークン基盤の実装は `crates/fandhe-core/src/token/` に閉じており、二層
アーキテクチャ（REQ-2）上は共通コアに属します。特定プラットフォームの描画
バックエンドには依存しません。

## 2. 全体アーキテクチャ総括

### 2.1 モジュール構成

`crates/fandhe-core/src/token/` 配下の各モジュールと責務は次のとおりです
（詳細な役割・他モジュールとの契約は `token/mod.rs` のクレートドキュメントを
一次情報として参照してください）。

| モジュール | 責務 | 対応 sub-issue |
|-----------|------|----------------|
| `schema` | `TokenPath` / `TokenRef` / `TokenValue` / `TokenSet` / `TokenCategory` 等のスキーマ型 | S1（#157）/ S3（#159）が新設 |
| `error` | 型付きエラー `TokenError` | S3 が新設、以降のカテゴリ追加ごとに拡張 |
| `resolver` | `TokenResolver`（ベース → テーマ差分 → エイリアスの順で解決するエンジン本体） | S2（#158）設計、S3（#159）実装 |
| `color` | 色トークンの値型 `ColorValue` とパレット（`color::palette::*`） | S6（#162） |
| `semantic_color` | 役割別セマンティックカラー（`SemanticColorRole`）からパレット参照へのエイリアス構築・解決 | S7（#163） |
| `typography` | タイポグラフィ値型 `TypographyValue` / `FontWeightValue` / `FontStyleValue` と既定スケール | S8（#164）/ S9（#165） |
| `spacing` | スペーシング値型 `SpacingValue` とスケール定数 | S10（#166） |
| `dimension` | サイズ・角丸（radius）・高さ（elevation）の値型 `DimensionValue` / `ElevationValue` と既定スケール | S11（#167） |
| `border` / `shadow` | ボーダー・シャドウ値型と既定トークンセット・色参照解決ヘルパー | S12（#168） |
| `animation` | duration・アニメーション値型とスケール定数・プリセット | S13（#169） |
| `platform_theme` | プラットフォーム別既定テーマ（iOS HIG / Material Design 3 baseline） | S16（#172） |
| `typed` | AI 型安全 DSL 向け型付きトークン参照 `TypedTokenRef<K>`（sealed トレイト `TokenKind` 経由） | S19（#175） |
| `file`（feature `token-file`） | 外部トークン定義ファイル（JSON・DTCG 部分準拠）の読み込み・シリアライズ | S17（#173） |
| `docgen` | コード上のトークン定義から Markdown リファレンスを生成する純粋なデータ変換 | S20（#176） |

ウィジェット・レイアウトへの配線（`ui/` 配下）は S14（#170、基本ウィジェット）
・S15（#171、taffy レイアウトスタイルとの接続）が別途担っています。本ガイドの
「§3.6 ウィジェット・レイアウトへの適用」で使い方のみ扱います。

### 2.2 sub-issue ⇔ 成果物対応表（S1〜S24）

| # | sub-issue | Issue | 成果物 |
|---|-----------|-------|--------|
| 1 | スキーマ・型設計 | #157 | `docs/architecture/design-token-architecture.md` |
| 2 | 解決エンジン設計 | #158 | `docs/architecture/token-resolution-architecture.md` |
| 3 | 解決エンジン実装 | #159 | `token::schema` / `token::error` / `token::resolver` |
| 4 | テーマ切替設計 | #160 | `docs/architecture/theme-switching-architecture.md` |
| 5 | テーマ切替実装 | #161 | `token::theme`（`ThemeVariant` / `Theme` / `ThemePreference`）・`TokenResolver::with_themes` / `set_active_theme` / `theme_generation` |
| 6 | 色トークン: パレット | #162 | `token::color` |
| 7 | 色トークン: セマンティックカラー | #163 | `token::semantic_color` |
| 8 | タイポグラフィ: ファミリ・サイズ | #164 | `token::typography`（一部） |
| 9 | タイポグラフィ: 行間・ウェイト・スタイル | #165 | `token::typography`（残余フィールド） |
| 10 | スペーシング | #166 | `token::spacing` |
| 11 | サイズ・角丸・高さ | #167 | `token::dimension` |
| 12 | ボーダー・シャドウ | #168 | `token::border` / `token::shadow` |
| 13 | duration・アニメーション | #169 | `token::animation` |
| 14 | 基本ウィジェットへの配線 | #170 | `ui::style_tokens` |
| 15 | レイアウトスタイルとの接続 | #171 | `ui::stack_grid` / `ui::card` / `ui/layout/style.rs` |
| 16 | プラットフォーム別既定テーマ | #172 | `token::platform_theme` |
| 17 | 外部定義ファイル読み込み | #173 | `token::file`（feature `token-file`） |
| 18 | バリデーション・エラー報告 | #174 | `token::validation` |
| 19 | AI 型安全 DSL からのトークン参照 API | #175 | `token::typed` |
| 20 | ドキュメント自動生成 | #176 | `token::docgen` / `docs/tokens/design-token-reference.md` |
| 21 | S3〜S20 の結合テスト観点整理 | #177 | `tests/token_*.rs` |
| 22 | テーマ切替のユニットテスト | #178 | `tests/theme_switching.rs`（観点 4・5 を含む） |
| 23 | カタログデモへのテーマ適用 | #179 | `ui::theme_demo`（`ThemeVariant` / `set_active_theme` 経由） |
| 24 | 設計文書・使用ガイド作成 | #180 | 本文書 |

### 2.3 spec 整合性

- **REQ-8**（デザイントークン基盤、M3）: 本モジュール一式がその実装本体です。
- **REQ-2**（二層アーキテクチャ）: `token::` は共通コア（`fandhe-core`）に閉じ、
  プラットフォーム固有クレートへ依存しません。プラットフォーム別の具体値
  （`platform_theme`）はあくまで「共通コア内の合成レイヤ」であり、外部依存の
  逆転（最適化レイヤ → コアの一方向）を破りません。
- **REQ-13**（AI 型安全 DSL・クロージャ拒否）: `TypedTokenRef<K>` はクロージャを
  引数に取るコンストラクタ・メソッドを一切持ちません（`typed.rs` モジュール
  doc 参照）。カテゴリの取り違えはコンパイルエラーとして検出されます
  （`tests/token_typed_compile_fail.rs`）。

## 3. 使用ガイド

### 3.1 トークンの参照と命名規則

トークンパスは `category.group.name` 形式のドット区切り階層パスです
（例: `color.primary`、`spacing.md`、`duration.normal`）。生の文字列は
解決エンジンの境界を越えて受け渡さず、必ず `TokenPath` / `TokenRef` という
パース済みの構造化データへ変換してから扱います（OWASP A03 対策の起点、
`token::schema` モジュール doc 参照）。

```rust
use fandhe_core::{TokenPath, TokenRef};

// ドット区切り文字列から構築する（許可文字集合・`..` セグメント拒否を検証）。
let path = TokenPath::from_dotted("color.palette.brand-a")?;
let token_ref = TokenRef::new(path);
```

`TokenPath::new` はセグメント列（`Vec<String>`）から直接構築することもできます。
許可文字集合は英数字・`-`・`_` のみで、空セグメント・`..` セグメントは
`TokenError::InvalidPath` として拒否されます。

### 3.2 解決エンジンの使い方

`TokenResolver` はベーストークンセットとアクティブテーマの差分から構築し、
「ベース → テーマ差分 → エイリアス」の順序で解決します。

```rust
use fandhe_core::{TokenResolver, TokenSet, palette_token_set};

let base: TokenSet = palette_token_set()?;
let theme_overrides = TokenSet::new(); // ライトテーマ相当（差分なし）
let mut resolver = TokenResolver::new(base, theme_overrides);

// アプリ起動時に一括解決してキャッシュへ反映する。
resolver.resolve_all().map_err(|errors| {
    // 一部キーが失敗しても成功分はキャッシュへ反映済み。全エラーを集約して返す。
    errors
})?;

// 個別のトークン参照を解決する（キャッシュにあればそれを返す）。
let value = resolver.resolve(&token_ref)?;
```

エラー時に既定値へ自動フォールバックしない契約のため、呼び出し元は必ず
`Result` を処理してください（`coding-rust.md` の panic 禁止方針）。

エイリアスチェーンは循環検出（`TokenError::CircularReference`）と最大深さ
`MAX_ALIAS_DEPTH`（32、`TokenError::MaxDepthExceeded`）の両方で打ち切られ、
明示的ループ（再帰なし）で実装されています。参照元と参照先でカテゴリが
一致しない場合は `TokenError::TypeMismatch` になります。

### 3.3 テーマの選択・切替

`ThemePlatform` はプラットフォーム別の既定テーマ（iOS HIG / Material Design 3
baseline 準拠の具体値）を提供します。

```rust
use fandhe_core::{ThemePlatform, platform_base_token_set, platform_dark_overrides};

let base = platform_base_token_set(ThemePlatform::Ios)?;
let dark_overrides = platform_dark_overrides(ThemePlatform::Ios)?;

// ダークテーマを適用する場合は dark_overrides を theme_overrides として渡す。
let mut resolver = TokenResolver::new(base, dark_overrides);
resolver.resolve_all().map_err(|_errors| { /* ... */ })?;
```

テーマを切り替える際は、低レベル API として `TokenResolver::set_theme_overrides`
でアクティブテーマの上書きマップを直接差し替え、続けて `resolve_all` を
呼び出してキャッシュを再充填する経路が引き続き使えます（`set_theme_overrides`
単体ではキャッシュを空にするのみで再計算は行いません）。

```rust
resolver.set_theme_overrides(new_theme_overrides);
resolver.resolve_all().map_err(|_errors| { /* ... */ })?;
```

より高レベルな契約として、`ThemeVariant` / `Theme` / `ThemePreference`
（S5、#161、テーマ切替機構実装）が `TokenResolver::with_themes` /
`set_active_theme` / `theme_generation` を提供します。ライト/ダーク両方の
`Theme` を保持したまま `ThemeVariant` 単位で切り替えられ、同一バリアントへの
再切替は no-op（冪等）で `theme_generation` を進めません。

```rust
use fandhe_core::{ThemePlatform, ThemeVariant, TokenResolver, platform_base_token_set, platform_theme};

let base = platform_base_token_set(ThemePlatform::Ios)?;
let light = platform_theme(ThemePlatform::Ios, ThemeVariant::Light)?;
let dark = platform_theme(ThemePlatform::Ios, ThemeVariant::Dark)?;

let mut resolver = TokenResolver::with_themes(base, light, dark, ThemeVariant::Light);

// OS 設定変更の通知等、プラットフォームレイヤが解決済みの ThemeVariant を渡す。
resolver.set_active_theme(ThemeVariant::Dark).map_err(|_errors| { /* ... */ })?;
assert_eq!(resolver.active_theme(), ThemeVariant::Dark);
```

`ThemePreference::System` は OS 判定結果（プラットフォームレイヤが解決済みの
`ThemeVariant`）を `resolved_with` で受け取る純関数であり、コア自身は OS API・
環境変数を読みません（REQ-2 の一方向依存、`docs/architecture/theme-switching-architecture.md`
§2.4 参照）。

### 3.4 型安全 API（AI 型安全 DSL からの参照）

`TypedTokenRef<K>` はカテゴリを型パラメータへ焼き込んだトークン参照です。
カテゴリの異なる参照を取り違えるとコンパイルエラーになります。

```rust
use fandhe_core::{ColorTokenRef, TypedTokenRef};

let color_ref: ColorTokenRef = TypedTokenRef::from_dotted("color.palette.brand-a")?;
let value = color_ref.resolve(&resolver)?; // 型は ColorValue
```

主なカテゴリ別型エイリアス: `ColorTokenRef` / `TypographyTokenRef` /
`SpacingTokenRef` / `SizeTokenRef` / `RadiusTokenRef` / `ElevationTokenRef` /
`BorderTokenRef` / `ShadowTokenRef` / `DurationTokenRef` / `AnimationTokenRef`
（`TokenCategory::Dimension` / `Text` は S1 以前の暫定遺産であり型安全 API の
対象外）。

`TypedTokenRef<K>` はクロージャを引数に取るコンストラクタ・メソッドを持ちません
（REQ-13 整合）。実際のカテゴリ不一致は `resolve` 呼び出し時に実行時検査され、
`TokenError::TypeMismatch` として報告されます（構築時点で防げるのは「型付き
参照同士の取り違え」のみで、無型 `TokenRef::from_ref` で持ち上げた場合の
中身の取り違えは実行時契約のままです）。

`TokenResolver::resolve_typed` も同じ解決ロジックへの薄い委譲として提供されて
います（`resolver.resolve_typed(&color_ref)` は `color_ref.resolve(&resolver)`
と等価）。

### 3.5 外部トークン定義ファイル（feature `token-file`）

`token-file` feature（既定 off）を有効にすると、DTCG 部分準拠の JSON から
`TokenSet` を読み込めます。

```rust
#[cfg(feature = "token-file")]
use fandhe_core::{load_token_set_file, parse_token_set_json};
#[cfg(feature = "token-file")]
use std::path::Path;

#[cfg(feature = "token-file")]
let token_set = load_token_set_file(Path::new("./tokens.json"))?;
```

**外部入力は信頼できない入力として扱ってください**（OWASP A03 / A08）。
`load_token_set_file` はファイルサイズ（`MAX_TOKEN_FILE_BYTES`、1 MiB）・
トークン件数（`MAX_TOKEN_COUNT`、4096）・パスのセグメント数
（`MAX_PATH_SEGMENTS`、16）・セグメント長（`MAX_SEGMENT_LEN`、64 バイト）・
参照文字列長（`MAX_REFERENCE_LEN`、256 バイト）のそれぞれに上限を設けて
DoS 耐性を確保しています。ただし本モジュールは構文レベルの検証
（JSON 形状・文字集合・上限・値域）のみを行い、未定義参照・循環・型不一致
といった意味レベルの検証は `token::validation`（`validate_token_sets` /
`TokenValidationReport`、#174）のスコープです。`load_token_set_file` /
`parse_token_set_json` のエラーは呼び出し元で処理し、意味レベル検証は
`validate_token_sets` を通してから `TokenResolver` へ渡してください。
信頼できないパス・JSON をそのまま `TokenResolver` へ渡す使い方は推奨しません。
ネットワーク経由のテーマ配信（OTA、Web 限定）を行う場合は別途署名検証が
必須です（本モジュールのスコープ外、`security.md` A08 参照）。

### 3.6 ウィジェット・レイアウトへの適用

基本ウィジェット（`ui::style_tokens`、S14/#170）は `StyleTokenRefs`（背景・
前景・タイポグラフィのトークン参照の束）を `StateStyles<StyleTokenRefs>` へ
乗せることで、状態別（Idle/Hovered/Pressed/Disabled）のスタイルを表現します。

```rust
use fandhe_core::{ColorTokenRef, StyleTokenRefs, TypedTokenRef};

let refs = StyleTokenRefs::new()
    .with_background_typed(TypedTokenRef::from_dotted("color.semantic.surface")?)
    .with_foreground_typed(TypedTokenRef::from_dotted("color.semantic.on-surface")?);
```

型付きビルダー（`with_background_typed` 等）は S19（#175）以降に追加された
加法的な入口であり、既存の無型ビルダー（`with_background` 等）と併存します。
解決失敗（未定義キー・型不一致等）は `Err` を返し、呼び出し元（各ウィジェットの
`paint_styled` オーバーライド）は無スタイル描画列へのフォールバックとして
扱う契約です。

寸法トークン（spacing / size / radius）と `ui::layout::style::LayoutStyle`
（taffy レイアウトスタイル）との接続は S15（#171）が担っており、
`ui/stack_grid.rs` / `ui/card.rs` に配線例があります。

### 3.7 エラーハンドリング

`TokenError` は解決・構築・パースの各段階で発生し得るエラーを型付けしたもの
です（`token::error` モジュール参照）。主な分類:

| 分類 | 発生源 | 意味 |
|------|--------|------|
| `UndefinedReference` | `resolver` | 参照先キーがベース・テーマ差分いずれにも存在しない |
| `TypeMismatch` | `resolver` / `typed` | 参照元と参照先でカテゴリが一致しない |
| `CircularReference` | `resolver` | エイリアスチェーンが循環している |
| `MaxDepthExceeded` | `resolver` | エイリアスチェーンの深さが `MAX_ALIAS_DEPTH`（32）を超えた |
| `InvalidPath` | `schema` | `TokenPath` の構築時検証に失敗した |
| `InvalidColor` / `InvalidTypography` / `InvalidSpacing` / `InvalidDimension` / `InvalidBorder` / `InvalidShadow` / `InvalidDuration` | 各値型 | 専用値型の構築時検証（非有限値・負値等）に失敗した |
| `InvalidReferenceSyntax` / `ParseError` / `FileRead` | `file`（feature `token-file`） | 外部定義ファイルの構文検証・読み込みに失敗した |

いずれも `panic` せず `Result` で返ります。フォールバック方針（既定値への
置き換え等）は呼び出し元が決定する契約であり、本基盤側は自動フォールバックを
行いません。

### 3.8 リファレンスの再生成手順と鮮度テスト

`docs/tokens/design-token-reference.md` はコード上のトークン定義から機械的に
生成される網羅的なリファレンスです。**手動編集しないでください**。

```sh
# 再生成（コード変更後・カテゴリ追加後に実行する）
cargo run -p fandhe-core --example generate_token_docs

# 鮮度検証（コミット済みファイルが現在のコードと一致することを CI で強制する）
cargo test -p fandhe-core --test token_docgen
```

`examples/generate_token_docs.rs` は `token::docgen` の純粋な生成ロジック
（ファイル I/O なし）を呼び出し、固定パス（`CARGO_MANIFEST_DIR` 起点、外部
入力を経由しない）へ書き出す唯一の窓口です。`tests/token_docgen.rs` は同じ
セクション構築関数（`default_token_doc_sections`）を共有するため、生成物と
テストは常に同じ内容になります（OWASP A08「整合性の不備」対策）。

## 4. 拡張ガイド（新カテゴリ・新トークン追加時の手順）

新しいトークンカテゴリを追加する場合は、以下の順序で変更してください
（既存カテゴリの追加時の実例は `token::border` / `token::shadow`（S12）・
`token::animation`（S13）を参照）。

1. **値型の追加**: 新規モジュール（例 `token/foo.rs`）に専用値型
   （`FooValue`）を定義する。構築時検証を行い、`TokenError` に対応する分類
   （`InvalidFoo` 等）を追加する。
2. **スキーマへの登録**: `schema::TokenCategory` に新バリアントを追加し、
   `TokenCategory::ALL` 配列を更新する（`all_covers_every_variant_exactly_once`
   テストが要素数・重複なしを検証する）。`schema::TokenValue` にも対応する
   バリアントを追加する。
3. **既定セットの用意**: `foo_token_set()` のような既定トークンセット構築
   関数を用意する（他カテゴリの `*_token_set()` に倣う）。
4. **型安全 API への登録**: 型安全 DSL から参照させる場合は `token::typed` に
   `define_token_kind!` マクロでマーカー型を追加し、`FooTokenRef` 型
   エイリアスを公開する。
5. **docgen セクション登録**: `docgen::default_token_doc_sections` へ新カテゴリの
   セクションを追加する。
6. **鮮度テストの再実行**: `cargo run -p fandhe-core --example
   generate_token_docs` で `docs/tokens/design-token-reference.md` を再生成し、
   `cargo test -p fandhe-core --test token_docgen` が通ることを確認してから
   コミットする。
7. **`mod.rs` のクレートドキュメント更新**: 追加した sub-issue の契約・他
   モジュールとの接続点を `token/mod.rs` の `//!` コメントへ追記する
   （`code-comment-style.md` 準拠）。

## 参考: 関連ファイル一覧

- 実装本体: `crates/fandhe-core/src/token/`
- 結合テスト: `crates/fandhe-core/tests/token_resolver.rs` /
  `token_typed_ref.rs` / `token_widget_wiring.rs` / `token_docgen.rs` /
  `token_file.rs`
- コンパイル失敗テスト（型安全性の固定化）:
  `crates/fandhe-core/tests/token_typed_compile_fail.rs`
- リファレンス生成 example: `crates/fandhe-core/examples/generate_token_docs.rs`
- 設計文書: `docs/architecture/design-token-architecture.md` /
  `docs/architecture/token-resolution-architecture.md` /
  `docs/architecture/theme-switching-architecture.md`
- 自動生成リファレンス: `docs/tokens/design-token-reference.md`
