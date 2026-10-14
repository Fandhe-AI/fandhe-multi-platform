//! fandhe-multi-platform の共通コアクレート。
//!
//! iOS / Android / watchOS / visionOS / macOS / Windows / Linux / Web の各プラットフォーム
//! 統合レイヤ（`fandhe-platform-*` 等）から参照される、プラットフォーム非依存の
//! 宣言的 UI DSL・状態管理・AI 型安全 DSL を提供する。依存方向は最適化レイヤ → 本クレート
//! の一方向とし、本クレートはプラットフォーム固有クレートへ依存しない
//! （`.claude/rules/coding-rust.md` 準拠）。
//!
//! # モジュール構成
//!
//! - `ui`（REQ-1 / REQ-5 / REQ-8）: 宣言的 UI DSL の中核。`Screen<S>` / `Node<S>` の
//!   コア型・ツリー構造・走査、ウィジェット共通トレイト `Widget<S>` とビルトイン
//!   ウィジェットセット、ヒットテスト・ポインタディスパッチ、仮想化リスト、画面遷移
//!   スタックを提供する
//! - `state`（REQ-13 / TASK-13.1）: 状態アクセス基盤。`ReadFn<S, T>` / `WriteFn<S, T>` /
//!   `Event` / `StateStore<S>` を提供する
//! - `token`（REQ-8 / TASK-8.4）: デザイントークン基盤。トークン定義スキーマ・テーマ切替・
//!   型付き参照 API を提供する
//! - `screens`（Issue #653）: AI が生成した画面モジュールの置き場（`ui/mod.rs` には
//!   手を入れない）
//! - `tsv`（親 #979、Issue #980）: TSV（タブ区切り値）の純粋関数パーサ。端末依存ゼロで、
//!   myIme 設定画面のユーザー辞書読み込みや `screens::tsv_import` 型サンプルから使う
//!
//! # 横断的な契約
//!
//! 各モジュールの契約は個々のモジュール doc に SSOT があるが、複数モジュールをまたぐため
//! ここにしか書けないものを以下にまとめる。
//!
//! - 状態アクセスは `ReadFn` / `WriteFn`（関数ポインタ）限定でクロージャを拒否する
//!   （REQ-13 / NFR-9、PoC-6 踏襲）。クロージャ環境キャプチャがコンパイル段階で拒否
//!   されることは `tests/compile_fail.rs`（trybuild）で固定化している
//! - `Widget<S>` 実装（widget 層）は `&mut S` に直接触れず、`state::Event` を返す
//!   だけのブリッジとして実装する（依存方向: widget 層 → `state`）
//! - `Node::Widget` は egui-proxy / `render_headless` では描画未配線であり、AX ノードの
//!   公開のみを行う
//! - `ui::RenderMode` は Web 以外のプラットフォームレイヤが無視してよい
//! - `token` の値解決はベース → テーマ差分 → エイリアス解決の順序で行い
//!   （`token::TokenResolver`）、`validate_token_sets` を経た結果のみが
//!   `TokenResolver::validated` で構築できる。`TypedTokenRef` によりトークンカテゴリの
//!   取り違えをコンパイル時に検出する（REQ-13）
//! - `token` の公開 API はクレートルートへ再エクスポートする。`token-file`
//!   feature（外部定義ファイル読み込み）は既定で無効
#![forbid(unsafe_code)]

/// `docs/ai/SCHEMA.md`（AI 向け公開 API メタ情報、REQ-13 / TASK-13.2）内の Rust コード例を
/// doctest としてコンパイルし、公開 API の変更で SCHEMA.md が陳腐化したことを `cargo test` で
/// 検知する（Issue #641）。SCHEMA.md は AI がコードを書く際にこのファイルを読む前提のため、
/// ここが壊れる = AI 生成コードがビルドできなくなる、を意味する。
#[cfg(doctest)]
#[doc = include_str!("../../../docs/ai/SCHEMA.md")]
mod schema_md {}

pub mod screens;
pub mod state;
pub mod token;
pub mod tsv;
pub mod ui;

pub use state::{
    Event, ReadAccessor, ReadFn, ReadIndexedFn, StateStore, WidgetValue, WriteAccessor, WriteFn,
};
pub use token::{
    AnimationKind, AnimationTokenRef, AnimationValue, BorderKind, BorderStyleValue, BorderTokenRef,
    BorderValue, ColorKind, ColorTokenRef, ColorValue, DimensionValue, DurationKind,
    DurationTokenRef, DurationValue, EasingValue, ElevationKind, ElevationTokenRef, ElevationValue,
    FontStyleValue, FontWeightValue, MAX_ALIAS_DEPTH, RadiusKind, RadiusTokenRef, RadiusValue,
    ResolvedValue, SemanticColorMapping, SemanticColorRole, ShadowKind, ShadowTokenRef,
    ShadowValue, SizeKind, SizeTokenRef, SizeValue, SpacingKind, SpacingTokenRef, SpacingValue,
    Theme, ThemePlatform, ThemePreference, ThemeVariant, TokenCategory, TokenDefinition,
    TokenDocSection, TokenError, TokenKind, TokenPath, TokenRef, TokenResolver, TokenSet,
    TokenValidationIssue, TokenValidationReport, TokenValue, TypedTokenRef, TypographyKind,
    TypographyTokenRef, TypographyValue, animation_presets, animation_token_set, border_token_set,
    default_token_doc_sections, duration_scale, duration_token_set, elevation_level,
    elevation_token_set, family, generate_token_reference, palette, palette_token_set,
    platform_base_token_set, platform_dark_overrides, platform_theme, radius_scale,
    radius_token_set, resolve_border_color, resolve_semantic_color, resolve_shadow_color, scale,
    semantic_color_entry, shadow_token_set, size_scale, size_token_set, spacing_scale,
    spacing_token_set, typography_token_set, validate_token_sets,
};
#[cfg(feature = "token-file")]
pub use token::{
    MAX_PATH_SEGMENTS, MAX_REFERENCE_LEN, MAX_SEGMENT_LEN, MAX_TOKEN_COUNT, MAX_TOKEN_FILE_BYTES,
    load_token_set_file, parse_token_reference, parse_token_set_json, serialize_token_set_json,
};
pub use ui::{
    AccessibilityRole, Axis, Button, ButtonInput, CanvasWidget, Card, CardInput, CatalogCategory,
    CatalogEntry, CatalogError, Checkbox, CheckboxInput, CheckboxWidget, Dialog, DialogHitRegion,
    DialogInput, DialogWidget, FocusDirection, FocusEntry, FocusRing, GalleryState, Grid,
    GridInput, GridWidget, HitRegion, HitTarget, Icon, IconSource, IconWidget, IconWidgetSource,
    Image, ImageLoadPhase, ImageSource, ImageWidget, ImageWidgetSource, ImeEvent, ImePurpose,
    ImeRequest, ImeSessionId, InteractionInput, InteractionState, Label, LabelSource, LabelWidget,
    LifecyclePhase, ListItemSource, ListView, ListViewInput, Measurement, Menu, MenuHitRegion,
    MenuInput, ModeSwitcher, Node, PaintCommand, Point, PointerDispatcher, PointerEvent,
    PointerEventKind, ProgressBar, ProgressMode, ProgressWidget, RadioButton, RadioGroupInput,
    RadioGroupWidget, Rect, RenderMode, RendererLifecycle, ResolvedWidgetStyle, SampleState,
    Screen, ScreenStack, ScrollAxis, ScrollView, ScrollViewInput, ScrollViewWidget, Separator,
    SeparatorOrientation, Size, Slider, SliderInput, Spinner, Stack, StackAxis, StackInput,
    StackWidget, StateStyles, StyleTokenRefs, TabItem, Tabs, TextFieldInput, TextFieldWidget,
    TextInput, TextRole, ToggleSwitch, ToggleSwitchInput, Tooltip, TooltipSource,
    TooltipTextSource, TooltipVisibility, TooltipWidget, VisibleRange, VisualState, Widget,
    WidgetCatalog, WidgetHarness, WidgetId, WindowHints, action_style_refs, activate_focus_at,
    clamp_scroll_offset, click_button, click_button_at, compute_visible_range, demo_resolver,
    display_style_refs, grid_item_range, hit_test, input_style_refs, render_headless, request_back,
    resolve_widget_style, sample_screen, set_widget_value_at, style_paint_commands,
    themed_catalog_lines, themed_sample_lines, themed_style_lines, total_content_extent,
};
