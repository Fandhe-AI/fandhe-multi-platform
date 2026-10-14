//! デザイントークン（参照・エイリアス解決）モジュール（TASK-8.4-S3、#159）。
//!
//! `docs/architecture/token-resolution-architecture.md`（TASK-8.4-S2、#158）の
//! 設計を実装契約として引き継ぐ。二層アーキテクチャ（REQ-2）上は共通コアに閉じ、
//! 特定プラットフォームの描画バックエンド等には依存しない純粋なデータ変換として
//! 実装する。
//!
//! - `schema`: `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、
//!   #157）§4.3・§5.2 が確定したスキーマ型（`TokenPath` / `TokenRef` /
//!   `TokenValue` / `TokenSet` 等）。`TokenValue::Color` は `color::ColorValue`
//!   を保持する（TASK-8.4-S6、#162）。
//! - `color`: 色トークンの専用値型 `ColorValue` とベースパレット
//!   （`color::palette::*`、TASK-8.4-S6、#162）。設計文書 §4.1 の割り当て表が
//!   定める色カテゴリの分割のうち、パレット（基礎色）を担う。役割別エイリアス
//!   （セマンティックカラー）は S7（#163）のスコープ。
//! - `error`: 型付きエラー `TokenError`（設計文書 §4.2 の 4 分類 + `InvalidPath` /
//!   `InvalidColor` / `InvalidTypography` / `InvalidSpacing` / `InvalidBorder` /
//!   `InvalidShadow` の追加分類）。
//! - `resolver`: `TokenResolver`（ベース → テーマ差分 → エイリアス解決の順序で
//!   解決するエンジン本体、設計文書 §3.3・§4・§5）。
//! - `typography`: タイポグラフィトークンの専用値型 `TypographyValue` /
//!   `FontWeightValue` / `FontStyleValue`（フォントファミリ・サイズ:
//!   TASK-8.4-S8、#164、行間・ウェイト・スタイル: TASK-8.4-S9、#165）と
//!   既定スケール（`typography::family::*` / `typography::scale::*`）。
//!   設計文書 §4.1 の割り当て表が定める typography カテゴリの全フィールドを
//!   `TypographyValue` として統合する（S8/S9 は各々別フィールド群を検証する）。
//! - `dimension`: サイズ・角丸（radius）・高さ（elevation）トークンの専用値型
//!   `DimensionValue`（`SizeValue` / `RadiusValue` の共有実体）・
//!   `ElevationValue`（TASK-8.4-S11、#167）と既定スケール
//!   （`dimension::size_scale::*` / `dimension::radius_scale::*` /
//!   `dimension::elevation_level::*`）。設計文書 §4.1 の割り当て表が定める
//!   size / radius / elevation の 3 カテゴリを実装する。spacing（S10、#166）は
//!   `DimensionValue` を再利用せず独自の `SpacingValue`（`spacing.rs`）を使う
//!   （設計文書 §5.2、`spacing.rs` モジュール doc 参照）。
//!
//! 他 sub-issue との契約（設計文書 §6 参照）:
//! - S4/S5（#160/#161、テーマ切替）: `theme::ThemeVariant` / `theme::Theme` の
//!   2 型と `TokenResolver::with_themes` / `set_active_theme` /
//!   `theme_generation` の切替 API を実装済み（S5、#161）。既存の
//!   `TokenResolver::new` の `theme_overrides` 引数・`set_theme_overrides` +
//!   `resolve_all` による全件無効化・再計算契約は低レベル API として存続する
//!   （`resolver.rs` モジュール doc 参照）。
//! - S12（#168、border / shadow）: `dimension::ElevationValue` の階層値と
//!   shadow の陰影表現（ぼかし・色・オフセット）を合成する。本モジュールは
//!   階層値の受け渡しのみを担う。
//! - S17（#173、外部定義ファイル読み込み）: 参照文字列のパース・`TokenSet` の
//!   構築を担う。本モジュールは構造化済みデータ（`TokenSet`）のみを受け取る。
//! - S18（#174、バリデーション）: `TokenError` の集約・報告を担う。
//!   S17 → S18 → 本モジュールというデータフロー契約（§5.3）を前提とする。
//!   `validation::validate_token_sets` / `TokenResolver::validated` として実装。
//! - S19（#175、AI 型安全 DSL からのトークン参照 API）: `typed::TypedTokenRef<K>` /
//!   `typed::TokenKind` を実装し、`TokenResolver::resolve`（`resolve_typed` 経由）を
//!   内部で呼び出す型安全な公開 API を提供する。
//! - S22（#178）: `set_theme_overrides` + `resolve_all` による切替・
//!   `platform_dark_overrides` の適用の観点に加え、`ThemeVariant` /
//!   `theme_generation` に依存する観点（同一テーマ再切替の冪等性・
//!   `ThemePreference::System` の伝搬）も `tests/theme_switching.rs` で
//!   固定化している（`theme::ThemeVariant` / `theme::Theme`、S5、#161）。
//!
//! - `semantic_color`: S7（#163、色トークン: セマンティックカラー対応）が実装した、
//!   役割別セマンティックカラー（`SemanticColorRole`）からパレット参照への
//!   エイリアス構築・型付き解決 API。既定マッピングの具体値は S16（#172）が、
//!   テーマ差分適用は S5（#161）の `theme::Theme` / `set_active_theme` が担う契約
//!   （詳細は同モジュール doc 参照）。
//! - `spacing`: スペーシングトークンの専用値型 `SpacingValue` とスケール定数
//!   （`spacing::scale::*`、TASK-8.4-S10、#166）。設計文書 §4.1 の割り当て表が
//!   定める spacing カテゴリを単独で担う。ウィジェット・レイアウト（`ui/`
//!   配下）への配線は S14/S15 のスコープ。公開再エクスポートは
//!   `spacing_scale`（`scale as spacing_scale`）へ改名する。`color::palette` の
//!   ような無改名の前例と異なり、S11（#167）が size / radius / elevation 用に
//!   同名の `scale` モジュールを追加し得るため、`token::` 直下での衝突を避ける。
//! - `border`: S12（TASK-8.4-S12、#168）が実装した、ボーダートークンの専用値型
//!   `BorderValue` / `BorderStyleValue` と既定トークンセット（`border_token_set`）・
//!   色参照の解決ヘルパー（`resolve_border_color`）。
//! - `shadow`: S12（TASK-8.4-S12、#168）が実装した、シャドウトークンの専用値型
//!   `ShadowValue` と既定トークンセット（`shadow_token_set`）・色参照の解決ヘルパー
//!   （`resolve_shadow_color`）。elevation カテゴリとの合成は S11/S16 のスコープ。
//! - `animation`: アニメーション・duration トークンの専用値型 `DurationValue` /
//!   `EasingValue` / `AnimationValue` とスケール定数・プリセット
//!   （`animation::scale::*` / `animation::presets::*`、TASK-8.4-S13、#169）。
//!   設計文書 §4.1 の割り当て表が定める animation / duration カテゴリを担う。
//!   ウィジェット・レイアウトへの配線は S14/S15、プラットフォーム準拠の具体値
//!   調整は S16 のスコープ。`spacing` と同じ理由で公開再エクスポート時に
//!   `scale`（→ `duration_scale`）・`presets`（→ `animation_presets`）へ改名する。
//! - `platform_theme`: S16（TASK-8.4-S16、#172）が実装した、プラットフォーム別
//!   既定テーマ（iOS HIG / Material Design 3 baseline 準拠の具体値）。
//!   `ThemePlatform` 列挙・`platform_base_token_set` / `platform_dark_overrides`
//!   を提供する。既存の各カテゴリ既定値（`color` / `typography` / `spacing` /
//!   `dimension` / `border` / `shadow` / `animation`）を合成した上に、
//!   プラットフォーム準拠の具体値を後勝ちで重ねる「合成レイヤ」であり、新規の
//!   カテゴリ・スキーマ変更は行わない。低レベル関数
//!   （`platform_base_token_set` / `platform_dark_overrides`）は引き続き
//!   関数名でライト/ダークを区別する形で存続しつつ、S5（#161）が追加した
//!   `platform_theme(platform, variant) -> Theme` ブリッジ関数が
//!   `theme::ThemeVariant` / `theme::Theme` との統合窓口を提供する。
//! - `validation`: S18（TASK-8.4-S18、#174）が実装した、`TokenSet`（ベース +
//!   テーマ差分）全体の事前検証ゲートウェイ（`validate_token_sets`）と、起点
//!   トークンパスとエラーを関連付けた決定的順序の集約レポート型
//!   （`TokenValidationIssue` / `TokenValidationReport`）。設計文書 §5.3 の
//!   「S17 → S18 → 解決エンジン」データフロー契約を、`resolver::TokenResolver::validated`
//!   経由で型レベルの導線として提供する。値そのものの妥当性は各値型の構築時
//!   検証（S6〜S13）が既に保証済みのため再検証しない（責務分担は同モジュール
//!   doc 参照）。
//!
//! TASK-8.4-S15（#171）で「寸法トークンと `ui::layout::style::LayoutStyle`
//! （taffy レイアウトスタイル）との接続」（設計文書 §6 未解決事項）を完了し、
//! `ui/stack_grid.rs` / `ui/card.rs` / `ui/layout/style.rs` へ spacing / size /
//! radius トークンの配線を追加した。基本ウィジェット（button / label 等）・
//! `StateStyles` / `PaintCommand` への配線は S14（#170）が別途担う。
//!
//! - `theme`: S5（TASK-8.4-S5、#161）が実装した、テーマ切替機構のデータ型
//!   `ThemeVariant`（ライト/ダークの閉じた列挙）・`Theme`（ベース `TokenSet` に
//!   対する 1 テーマ分の差分オーバーレイ）・`ThemePreference`（ユーザー/OS
//!   設定意図の 3 値）。`docs/architecture/theme-switching-architecture.md`
//!   （S4、#160）§2〜§4 を実装契約として引き継ぐ。切替 API 本体
//!   （`with_themes` / `active_theme` / `set_active_theme` / `theme_generation`）
//!   は `resolver` モジュールへ実装する（型とエンジンの責務分離、同文書 §3.1）。
//! - `typed`: S19（TASK-8.4-S19、#175）が実装した、カテゴリを型パラメータへ
//!   焼き込む型付きトークン参照 `TypedTokenRef<K>`（sealed トレイト `TokenKind`
//!   経由）。`ColorTokenRef` 等カテゴリ 10 種分の型エイリアスを提供し、
//!   `ui::style_tokens` の `with_*_typed` ビルダー・`semantic_color::typed_ref` の
//!   実体となる。
//! - `file`（feature `token-file`、既定 off）: TASK-8.4-S17（#173）が実装した、
//!   外部トークン定義ファイル（JSON・DTCG 部分準拠）の読み込み・シリアライズ
//!   境界。参照文字列（`{group.token}`）のパースは本モジュールに集約し、
//!   `schema` / `resolver` へは構造化済みの `TokenRef` のみを渡す契約
//!   （`token-resolution-architecture.md` §3.1）。読み込んだ `TokenSet` は
//!   S18（#174）のバリデーションを経てから `TokenResolver` へ渡す前提であり、
//!   本モジュールは resolver へ直結する API を持たない（§5.3 のデータフロー契約）。
//! - `docgen`: TASK-8.4-S20（#176）が実装した、コード上のトークン定義から
//!   Markdown リファレンス（`docs/tokens/design-token-reference.md`）を生成する
//!   純粋なデータ変換（ファイル I/O は持たない）。書き出しは
//!   `examples/generate_token_docs.rs`、生成物の鮮度検証は
//!   `tests/token_docgen.rs` が担う。設計文書 §4 のスキーマ・命名規則を
//!   `TokenCategory::ALL`（`schema.rs`）経由で機械可読化する。
//!   S24（#180、使用ガイド）が生成物を参照先とする契約。

mod animation;
mod border;
mod color;
mod dimension;
mod docgen;
mod error;
#[cfg(feature = "token-file")]
mod file;
mod platform_theme;
mod resolver;
mod schema;
mod semantic_color;
mod shadow;
mod spacing;
mod theme;
mod typed;
mod typography;
mod validation;

pub use animation::{
    AnimationValue, DurationValue, EasingValue, animation_token_set, duration_token_set,
    presets as animation_presets, scale as duration_scale,
};
pub use border::{BorderStyleValue, BorderValue, border_token_set, resolve_border_color};
pub use color::ColorValue;
pub use color::{palette, palette_token_set};
pub use dimension::{DimensionValue, ElevationValue, RadiusValue, SizeValue};
pub use dimension::{elevation_level, elevation_token_set, radius_scale, radius_token_set};
pub use dimension::{size_scale, size_token_set};
pub use docgen::{TokenDocSection, default_token_doc_sections, generate_token_reference};
pub use error::TokenError;
#[cfg(feature = "token-file")]
pub use file::{
    MAX_PATH_SEGMENTS, MAX_REFERENCE_LEN, MAX_SEGMENT_LEN, MAX_TOKEN_COUNT, MAX_TOKEN_FILE_BYTES,
    load_token_set_file, parse_token_reference, parse_token_set_json, serialize_token_set_json,
};
pub use platform_theme::{
    ThemePlatform, platform_base_token_set, platform_dark_overrides, platform_theme,
};
pub use resolver::{MAX_ALIAS_DEPTH, ResolvedValue, TokenResolver};
pub use schema::{TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};
pub use semantic_color::{
    SemanticColorMapping, SemanticColorRole, resolve_semantic_color, semantic_color_entry,
};
pub use shadow::{ShadowValue, resolve_shadow_color, shadow_token_set};
pub use spacing::{SpacingValue, scale as spacing_scale, spacing_token_set};
pub use theme::{Theme, ThemePreference, ThemeVariant};
pub use typed::{
    AnimationKind, AnimationTokenRef, BorderKind, BorderTokenRef, ColorKind, ColorTokenRef,
    DurationKind, DurationTokenRef, ElevationKind, ElevationTokenRef, RadiusKind, RadiusTokenRef,
    ShadowKind, ShadowTokenRef, SizeKind, SizeTokenRef, SpacingKind, SpacingTokenRef, TokenKind,
    TypedTokenRef, TypographyKind, TypographyTokenRef,
};
pub use typography::{FontStyleValue, FontWeightValue, TypographyValue};
pub use typography::{family, scale, typography_token_set};
pub use validation::{TokenValidationIssue, TokenValidationReport, validate_token_sets};
