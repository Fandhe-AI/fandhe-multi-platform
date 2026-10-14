//! 画像ウィジェット `Image<S>`（TASK-8.3-S34 / REQ-8 / REQ-9 / REQ-13、Issue #129）。
//!
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、同型の先行ウィジェット
//! [`super::label::Label`]（#105）・[`super::slider::Slider`]（#123）が確立した
//! パターンを踏襲し、`Widget<S>` を実装する具体型 [`Image`] が基本描画
//! （sync_from_state / measure / paint / hit_test / accessibility_role）を実装する。
//!
//! 設計文書 `docs/architecture/widget-architecture.md` §4.1 の方針に従い、
//! `Node<S>` enum は温存し `Node::Widget(Box<dyn Widget<S>>)` 経由でツリーへ組み込む。
//!
//! - 画像ソースの SSOT はアプリ状態 `S`（[`ImageSource::Bound`]）または構築時の
//!   固定値（[`ImageSource::Static`]）。ウィジェットは [`crate::state::ReadFn`]
//!   経由の [`Widget::sync_from_state`] で表示用キャッシュ `cached_source` を
//!   更新するのみで、中間編集状態を持たない（`Label` と同じ構造上の理由）。
//! - 画像ソース識別子（パス・URL 等）は不透明文字列として保持するのみで、`fandhe-core`
//!   は実デコード・テクスチャアップロード・ファイル I/O を一切行わない（ヘッドレス、
//!   OWASP A03 対策・`.claude/rules/security.md`）。実ロードはプラットフォームレイヤ・
//!   後続タスクのスコープとする。
//! - [`super::widget::AccessibilityRole::Image`] を本ウィジェット専用のロールとして
//!   `widget.rs` へ加法的に追加する（`accesskit::Role::Image` へ対応予定）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す — `Checkbox` / `ToggleSwitch` /
//!   `Slider` と同じ判断）。描画列は `[Rect（画像プレースホルダ枠）, Text(alt)]` とする。
//!   `alt` は開発者指定の代替テキストであり、ソース識別子（外部入力になり得る）は
//!   `Text` へ一切出さない（ヘッドレス出力へ外部由来文字列を流さない安全側の判断、
//!   OWASP A03 対策）。
//! - 固有サイズ（`intrinsic`）は構築時に数値健全性を担保する: 非有限値（NaN/inf）または
//!   `<= 0.0` の成分は既定プレースホルダ寸法（`DEFAULT_INTRINSIC_WIDTH` /
//!   `DEFAULT_INTRINSIC_HEIGHT`）へフォールバックする（`Slider::normalize_range` と
//!   同型の安全側判断、OWASP A04 相当）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。S3（#98）確立済みの
//! reconcile/sync パスが `Node::Widget` へ実際に `&mut` を届けるまで、上記 3 箇所は
//! `Node::Widget` を状態非依存のプレースホルダとして扱う契約を維持する。検証は
//! `Widget` メソッドの直接呼び出し（＋必要なら [`super::harness::WidgetHarness`]）で行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) 画像の実デコード・テクスチャアップロード・I/O（プラットフォームレイヤ / 後続
//! タスクのスコープ。core はヘッドレス・識別子は不透明文字列扱い。パストラバーサル・
//! SSRF 検証は実 I/O 導入側の責務として引き継ぐ）、
//! (b) ロード中・エラー時の状態遷移とイベント処理（S35）、
//! (c) `WidgetCatalog` 登録・テスト網羅拡充（S36）、
//! (d) content mode（Fill / Stretch 等）の切り替え・角丸等の装飾（Fit 相当の
//! アスペクト比維持縮小のみ実装）、
//! (e) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};

/// `with_intrinsic_size` が非有限値・非正値を無害化する際にフォールバックする
/// 既定の固有幅（論理ピクセル相当のプレースホルダ定数）。
///
/// 実デコード前のプレースホルダ寸法であり、実画像の実寸取得は後続タスク
/// （プラットフォームレイヤ）のスコープ（モジュール doc 参照）。
const DEFAULT_INTRINSIC_WIDTH: f32 = 64.0;

/// `with_intrinsic_size` が非有限値・非正値を無害化する際にフォールバックする
/// 既定の固有高さ。`DEFAULT_INTRINSIC_WIDTH` と同じくプレースホルダ。
const DEFAULT_INTRINSIC_HEIGHT: f32 = 64.0;

/// [`Image`] の画像ソース指定方法（[`super::label::LabelSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum ImageSource<S> {
    /// 固定のアセット識別子（パス・URL 等の不透明文字列。`fandhe-core` は I/O しない）。
    Static(String),
    /// アプリ状態 `S` から識別子を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    /// [`Widget::sync_from_state`] で `cached_source` を更新する。
    Bound(ReadFn<S, String>),
}

// `ImageSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の
// `S: Debug` / `S: Clone` 境界を回避する（`LabelSource` と同じ設計判断）。
impl<S> std::fmt::Debug for ImageSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageSource::Static(source) => f.debug_tuple("Static").field(source).finish(),
            ImageSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for ImageSource<S> {
    fn clone(&self) -> Self {
        match self {
            ImageSource::Static(source) => ImageSource::Static(source.clone()),
            ImageSource::Bound(get) => ImageSource::Bound(*get),
        }
    }
}

/// 固有サイズ（幅・高さ）を正規化する（非有限値・非正値は既定プレースホルダ寸法へ
/// フォールバック）。
///
/// 数値健全性の確保（OWASP A04 相当）のため、後続の `measure` がアスペクト比計算で
/// ゼロ除算・NaN 伝播を起こさないよう構築時点で無害化する
/// （`slider::normalize_range` と同じ判断）。
fn normalize_intrinsic_size(width: f32, height: f32) -> (f32, f32) {
    let width = if width.is_finite() && width > 0.0 {
        width
    } else {
        DEFAULT_INTRINSIC_WIDTH
    };
    let height = if height.is_finite() && height > 0.0 {
        height
    } else {
        DEFAULT_INTRINSIC_HEIGHT
    };
    (width, height)
}

/// 画像ウィジェット（TASK-8.3-S34、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Slider` / `Label` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。ソース識別子・代替テキスト・寸法は秘密情報に該当しないため、
/// `Debug` にそのまま含める。
pub struct Image<S> {
    id: WidgetId,
    source: ImageSource<S>,
    /// 代替テキスト（REQ-9 アクセシビリティ、`paint` の `Text` 出力に使用）。
    /// 開発者指定の文字列であり、ソース識別子（外部入力になり得る）とは独立させる。
    alt: String,
    /// 固有サイズ（幅・高さ）。構築時に [`normalize_intrinsic_size`] で正規化済み。
    intrinsic: Measurement,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在のソース識別子）。
    cached_source: String,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Image<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("alt", &self.alt)
            .field("intrinsic", &self.intrinsic)
            .field("cached_source", &self.cached_source)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Image<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            alt: self.alt.clone(),
            intrinsic: self.intrinsic,
            cached_source: self.cached_source.clone(),
            interaction: self.interaction,
        }
    }
}

impl<S> Image<S> {
    /// 状態 `S` に依存しない固定のアセット識別子を持つ画像を構築する。
    ///
    /// `cached_source` は構築直後から `source` で初期化されるため、
    /// `sync_from_state` 呼び出し前でも `source_text` が期待どおりの識別子を返す
    /// （`Label::fixed` と同じ意味論）。固有サイズは既定プレースホルダ寸法
    /// （`DEFAULT_INTRINSIC_WIDTH` × `DEFAULT_INTRINSIC_HEIGHT`）で初期化し、
    /// 変更する場合は [`Self::with_intrinsic_size`] を使う。
    pub fn fixed(id: WidgetId, source: impl Into<String>, alt: impl Into<String>) -> Self {
        let source = source.into();
        Self {
            id,
            cached_source: source.clone(),
            source: ImageSource::Static(source),
            alt: alt.into(),
            intrinsic: Measurement {
                width: DEFAULT_INTRINSIC_WIDTH,
                height: DEFAULT_INTRINSIC_HEIGHT,
            },
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたアセット識別子を持つ画像を構築する。
    ///
    /// `cached_source` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`Label::bound` と同じ判断）。
    pub fn bound(id: WidgetId, get: ReadFn<S, String>, alt: impl Into<String>) -> Self {
        Self {
            id,
            source: ImageSource::Bound(get),
            alt: alt.into(),
            intrinsic: Measurement {
                width: DEFAULT_INTRINSIC_WIDTH,
                height: DEFAULT_INTRINSIC_HEIGHT,
            },
            cached_source: String::new(),
            interaction: InteractionState::default(),
        }
    }

    /// 固有サイズ（幅・高さ）を設定するビルダーメソッド。
    ///
    /// 非有限値（NaN/inf）または `<= 0.0` の成分は既定プレースホルダ寸法へ
    /// フォールバックする（[`normalize_intrinsic_size`] 参照、安全側の判断）。
    pub fn with_intrinsic_size(mut self, width: f32, height: f32) -> Self {
        let (width, height) = normalize_intrinsic_size(width, height);
        self.intrinsic = Measurement { width, height };
        self
    }

    /// 現在の表示キャッシュ値（ソース識別子）を返す（テスト・観測用）。
    pub fn source_text(&self) -> &str {
        &self.cached_source
    }

    /// 代替テキストを返す。
    pub fn alt_text(&self) -> &str {
        &self.alt
    }

    /// 固有サイズ（構築時・`with_intrinsic_size` 時点で正規化済み）を返す。
    pub fn intrinsic_size(&self) -> Measurement {
        self.intrinsic
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// 画像には中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む
    /// （`Slider::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }
}

impl<S: 'static> Widget<S> for Image<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からソース識別子を読み取り `cached_source` を更新する
    /// （reconcile フェーズ）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            ImageSource::Static(source) => {
                // 固定識別子は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_source = source.clone();
            }
            ImageSource::Bound(get) => {
                self.cached_source = get(state);
            }
        }
    }

    /// 固有サイズをアスペクト比維持で `available` にフィットさせる（縮小のみ、
    /// 拡大はしない — content mode の Fit 相当、モジュール doc 参照）。
    ///
    /// `available` の幅・高さのいずれかが `0.0` 以下、または固有サイズが縮退
    /// （構築時に正規化済みのため通常発生しないが、念のためゼロ除算を回避する）
    /// している場合は `0.0` へ倒す（安全側の判断、`Slider::ratio` と同型）。
    fn measure(&self, available: Measurement) -> Measurement {
        if available.width <= 0.0 || available.height <= 0.0 {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }
        if self.intrinsic.width <= 0.0 || self.intrinsic.height <= 0.0 {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }

        let width_scale = available.width / self.intrinsic.width;
        let height_scale = available.height / self.intrinsic.height;
        // 縮小のみ許容し、拡大はしない（Fit 相当）。両軸のスケールのうち
        // 小さい方（より制約が厳しい軸）を採用し、かつ `1.0` を超えない。
        let scale = width_scale.min(height_scale).min(1.0);

        Measurement {
            width: self.intrinsic.width * scale,
            height: self.intrinsic.height * scale,
        }
    }

    /// `[Rect（画像プレースホルダ枠）, Text(alt)]` の描画プリミティブ列を返す。
    ///
    /// `alt` は開発者指定の代替テキストであり、ソース識別子（外部入力になり得る）は
    /// `Text` へ一切出さない（モジュール doc 参照、OWASP A03 対策）。`PaintCommand`
    /// 自体は拡張しない。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect, PaintCommand::Text(self.alt.clone())]
    }

    /// 画像は表示専用要素であり、常に `false` を返す（`Label::hit_test` と同じ
    /// 安全側の判断）。タップ可能画像等の対話は後続工程（S35）で再検討する。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Display カテゴリの既定マッピング
    /// （`style_tokens::display_style_refs`）を用い、`paint()` と同じ構造
    /// （`[Rect（画像プレースホルダ枠）, Text(alt)]`）を保ったまま色・タイポグラフィのみを
    /// 解決済み具体値へ差し替える。解決失敗時は `paint()` の無スタイル列へ
    /// フォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::display_style_refs() else {
            return self.paint(origin, size);
        };
        let Ok(resolved) = super::style_tokens::resolve_widget_style(
            resolver,
            &refs,
            self.interaction_state().visual_state(),
        ) else {
            return self.paint(origin, size);
        };
        match super::style_tokens::style_paint_commands(self.paint(origin, size), &resolved) {
            Some(styled) => styled,
            None => self.paint(origin, size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProfileState {
        avatar_url: String,
    }

    fn get_avatar_url(state: &ProfileState) -> String {
        state.avatar_url.clone()
    }

    // (a) 構築と各アクセサ

    #[test]
    fn fixed_returns_source_immediately_without_sync() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(1), "assets/logo.png", "ロゴ");

        assert_eq!(image.source_text(), "assets/logo.png");
        assert_eq!(image.alt_text(), "ロゴ");
        assert_eq!(image.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn bound_is_empty_before_first_sync() {
        let image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(2), get_avatar_url, "アバター");

        assert_eq!(image.source_text(), "");
        assert_eq!(image.alt_text(), "アバター");
    }

    #[test]
    fn default_intrinsic_size_is_placeholder_constant() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(3), "assets/logo.png", "ロゴ");

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    // (b) with_intrinsic_size の正規化（NaN / inf / 0 / 負値 → 既定寸法フォールバック）

    #[test]
    fn with_intrinsic_size_sets_positive_finite_dimensions() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(4), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(200.0, 100.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, 200.0);
        assert_eq!(size.height, 100.0);
    }

    #[test]
    fn with_intrinsic_size_falls_back_when_width_is_nan() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(5), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(f32::NAN, 100.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, 100.0);
    }

    #[test]
    fn with_intrinsic_size_falls_back_when_height_is_infinite() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(6), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(200.0, f32::INFINITY);

        let size = image.intrinsic_size();
        assert_eq!(size.width, 200.0);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    #[test]
    fn with_intrinsic_size_falls_back_when_zero() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(7), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(0.0, 0.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    #[test]
    fn with_intrinsic_size_falls_back_when_negative() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(8), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(-10.0, -20.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    // (c) sync_from_state の Static 不変・Bound の状態追従

    #[test]
    fn sync_from_state_keeps_static_source_unchanged() {
        let mut image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(9), "assets/logo.png", "ロゴ");
        let state = ProfileState {
            avatar_url: "https://example.com/avatar.png".to_string(),
        };

        image.sync_from_state(&state);

        assert_eq!(image.source_text(), "assets/logo.png");
    }

    #[test]
    fn sync_from_state_reflects_bound_state() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(10), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/avatar.png".to_string(),
        };

        image.sync_from_state(&state);

        assert_eq!(image.source_text(), "https://example.com/avatar.png");
    }

    #[test]
    fn sync_from_state_tracks_state_changes_across_calls() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(11), get_avatar_url, "アバター");
        let mut state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };

        image.sync_from_state(&state);
        assert_eq!(image.source_text(), "https://example.com/a.png");

        state.avatar_url = "https://example.com/b.png".to_string();
        image.sync_from_state(&state);
        assert_eq!(image.source_text(), "https://example.com/b.png");
    }

    // (d) measure（アスペクト比維持フィット・十分な余白ではそのまま・縮退時 0.0）

    #[test]
    fn measure_returns_intrinsic_size_when_available_is_large_enough() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(12), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(100.0, 50.0);

        let measurement = image.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        assert_eq!(measurement.width, 100.0);
        assert_eq!(measurement.height, 50.0);
    }

    #[test]
    fn measure_shrinks_to_fit_width_constraint_preserving_aspect_ratio() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(13), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(200.0, 100.0);

        let measurement = image.measure(Measurement {
            width: 50.0,
            height: 1000.0,
        });

        assert_eq!(measurement.width, 50.0);
        assert_eq!(measurement.height, 25.0);
    }

    #[test]
    fn measure_shrinks_to_fit_height_constraint_preserving_aspect_ratio() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(14), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(200.0, 100.0);

        let measurement = image.measure(Measurement {
            width: 1000.0,
            height: 20.0,
        });

        assert_eq!(measurement.width, 40.0);
        assert_eq!(measurement.height, 20.0);
    }

    #[test]
    fn measure_returns_zero_when_available_width_is_zero() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(15), "assets/logo.png", "ロゴ");

        let measurement = image.measure(Measurement {
            width: 0.0,
            height: 100.0,
        });

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn measure_returns_zero_when_available_height_is_negative() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(16), "assets/logo.png", "ロゴ");

        let measurement = image.measure(Measurement {
            width: 100.0,
            height: -1.0,
        });

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    // (e) paint（列・alt 内容検証・ソース文字列が Text に混入しないこと）

    #[test]
    fn paint_returns_rect_and_alt_text() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(17), get_avatar_url, "プロフィール画像");
        let state = ProfileState {
            avatar_url: "https://example.com/secret-token.png".to_string(),
        };
        image.sync_from_state(&state);

        let commands = image.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("プロフィール画像".to_string()),
            ]
        );
    }

    #[test]
    fn paint_never_leaks_source_string_into_text_command() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(18), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/should-not-appear.png".to_string(),
        };
        image.sync_from_state(&state);

        let commands = image.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );

        for command in &commands {
            if let PaintCommand::Text(text) = command {
                assert!(!text.contains("should-not-appear"));
            }
        }
    }

    // (f) hit_test は常に false

    #[test]
    fn hit_test_is_always_false_even_inside_bounds() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(19), "assets/logo.png", "ロゴ");
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert!(!image.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_is_always_false_when_disabled() {
        let mut image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(20), "assets/logo.png", "ロゴ");
        image.set_disabled(true);
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert!(!image.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    // (g) accessibility_role / interaction_state / clone_box

    #[test]
    fn accessibility_role_is_image() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(21), "assets/logo.png", "ロゴ");

        assert_eq!(image.accessibility_role(), AccessibilityRole::Image);
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        use super::super::widget::InteractionInput;

        let mut image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(22), "assets/logo.png", "ロゴ");

        image.interaction.apply(InteractionInput::PointerEntered);
        assert_eq!(image.visual_state(), VisualState::Hovered);

        image.set_disabled(true);
        assert_eq!(image.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(23), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };
        image.sync_from_state(&state);

        let mut cloned = image.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(23));

        let other_state = ProfileState {
            avatar_url: "https://example.com/b.png".to_string(),
        };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(image.source_text(), "https://example.com/a.png");
    }

    #[test]
    fn debug_output_does_not_panic() {
        let image: Image<ProfileState> =
            Image::fixed(WidgetId::from_raw(24), "assets/logo.png", "ロゴ");
        let output = format!("{image:?}");
        assert!(output.contains("Image"));
    }

    #[test]
    fn image_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Image<ProfileState>>();
    }

    // (h) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut image: Image<ProfileState> =
            Image::bound(WidgetId::from_raw(25), get_avatar_url, "プロフィール画像")
                .with_intrinsic_size(200.0, 100.0);
        let state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };

        image.sync_from_state(&state);
        let measurement = image.measure(Measurement {
            width: 50.0,
            height: 1000.0,
        });
        let commands = image.paint((0.0, 0.0), measurement);

        assert_eq!(measurement.width, 50.0);
        assert_eq!(measurement.height, 25.0);
        assert_eq!(image.source_text(), "https://example.com/a.png");
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("プロフィール画像".to_string()),
            ]
        );
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let image: Image<ProfileState> = Image::fixed(
            WidgetId::from_raw(200),
            "https://example.com/a.png",
            "プロフィール画像",
        );
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert_eq!(
            image.paint_styled((0.0, 0.0), size, &empty_resolver),
            image.paint((0.0, 0.0), size)
        );
    }
}
