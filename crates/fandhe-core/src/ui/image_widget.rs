//! 画像ウィジェットの状態・イベント処理実装（TASK-8.3-S35 / REQ-8 / REQ-9 / REQ-13、
//! Issue #130）。
//!
//! [`super::text_field::TextFieldWidget`]（#109）・[`super::checkbox::CheckboxWidget`]
//! （#115）・[`super::radio_group::RadioGroupWidget`]（#118）と同じ判断（既存 main の
//! 公開 API のみに依存する自己完結実装とし、`image::Image` との型統合は別作業に
//! 委ねる）を踏襲し、[`Widget<S>`] を実装する自己完結の画像ウィジェット型
//! [`ImageWidget`] として実装する（`measure` / `paint` は `image::Image` 相当の暫定実装）。
//!
//! 型名は `image::Image` / `image::ImageSource` との衝突を避けるため
//! `ImageWidget` / `ImageWidgetSource` とする（`checkbox::Checkbox` /
//! `checkbox::CheckboxWidget` と同じ命名判断）。`image::Image` との型統合は行わない。
//!
//! ## 状態・イベント処理
//!
//! - 画像ソースの SSOT はアプリ状態 `S`（[`ImageWidgetSource::Bound`]）または構築時の
//!   固定値（[`ImageWidgetSource::Static`]）。ウィジェットは [`crate::state::ReadFn`]
//!   経由の [`Widget::sync_from_state`] で表示用キャッシュ `cached_source` を更新する
//!   （`Label` と同じ構造上の理由）。画像ソース識別子は不透明文字列として保持するのみで、
//!   `fandhe-core` は実デコード・I/O を一切行わない（ヘッドレス、OWASP A03 対策）。
//! - [`ImageLoadPhase`]: ロード状態機械（未開始 / ロード中 / 完了 / 失敗）。実デコード・
//!   I/O を担うプラットフォームレイヤが `notify_load_started` / `notify_load_succeeded` /
//!   `notify_load_failed` を通じて遷移を通知する（`fandhe-core` 自体は I/O しない契約を
//!   維持する）。
//! - **ソース照合**: 通知の引数 `source` が現在の `cached_source` と一致しない場合は
//!   無視して `false` を返す。非同期ロードの順序逆転・ソース差し替え後に旧ソースの結果が
//!   届くケースで、古い結果が新しい表示へ適用される整合性不備（OWASP A08 相当）を
//!   型・実行時の両面で遮断する。
//! - **遷移制約**: `notify_load_started` は任意の phase から `Loading` へ遷移可（リトライ・
//!   再ロードを許容する）。`notify_load_succeeded` / `notify_load_failed` は `Loading`
//!   からのみそれぞれ `Loaded` / `Failed` へ遷移し、それ以外の phase では無視して `false`
//!   を返す（開始通知なしの完了通知という不整合な呼び出し列を拒否する）。
//! - **disabled との直交性**: disabled はユーザー対話の無効化であり、ロードは対話ではない
//!   ため disabled 中も通知は受理する（[`InteractionState`] とは独立）。
//! - [`Widget::sync_from_state`] で読み取った新ソースが `cached_source` と異なる場合、
//!   `load_phase` を `Idle` へリセットする（旧ソースの `Loaded` / `Failed` を新ソースへ
//!   持ち越さない）。同一ソースなら phase を維持する。
//! - `paint` は `[Rect, Text(alt)]` のまま変えない（`PaintCommand` は拡張しない）。phase の
//!   視覚表現（スピナー・エラーアイコン等）はプラットフォームレイヤが [`ImageWidget::load_phase`]
//!   を参照して行う契約とする。**`Failed` でもソース識別子を `Text` へ出さない**
//!   （OWASP A03、`Label` / S34 想定と同じ安全判断）。
//! - `hit_test` は常に `false`（表示専用、`Label::hit_test` と同じ判断）。タップ可能画像等の
//!   対話イベントは見送る（out-of-scope-tracking.md 対象）。
//!
//! **実描画の経路**（Issue #1103）: `fandhe-egui-proxy`（`Node::Widget` の `ImageWidget`
//! 分岐）と `fandhe-web-dom`（`convert_image_widget`）は [`Widget::as_any`] で本型へ
//! downcast し、クローンを `sync_from_state` してから `source_text()` を読んで描く。
//! 下記の「未配線」は `render_headless` / `to_snapshot_node` の話で、この 2 経路は含まない。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。検証は `Widget`
//! メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) phase の視覚表現（スピナー・エラー表示等の描画差別化） — `PaintCommand` 拡張が
//! 必要なためプラットフォームレイヤ / 後続タスク、
//! (b) タップ可能画像等の対話イベント — 表示専用設計を維持、
//! (c) `WidgetCatalog` 登録・テスト網羅拡充・S34 マージ後の型統合（S36 / #131）、
//! (d) 実デコード・I/O・SSRF/パストラバーサル検証 — プラットフォームレイヤ、
//! (e) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};

/// `with_intrinsic_size` が非有限値・非正値を無害化する際にフォールバックする
/// 既定の固有幅（論理ピクセル相当のプレースホルダ定数）。
const DEFAULT_INTRINSIC_WIDTH: f32 = 64.0;

/// `with_intrinsic_size` が非有限値・非正値を無害化する際にフォールバックする
/// 既定の固有高さ。`DEFAULT_INTRINSIC_WIDTH` と同じくプレースホルダ。
const DEFAULT_INTRINSIC_HEIGHT: f32 = 64.0;

/// 画像のロード状態機械（本 issue の核心）。
///
/// 実デコード・I/O を担うプラットフォームレイヤからの通知（[`ImageWidget::notify_load_started`]
/// 等）でのみ遷移する。`fandhe-core` 自体はいずれの状態でも実ロードを行わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLoadPhase {
    /// ロード未開始（構築直後・ソース変更直後の初期状態）。
    Idle,
    /// プラットフォームレイヤがロード開始を通知済み。
    Loading,
    /// ロード完了（実表示はプラットフォームレイヤの責務。`fandhe-core` は状態のみ保持）。
    Loaded,
    /// ロード失敗（フォールバック表示は alt テキスト、REQ-9。ソース識別子は `paint` へ
    /// 一切出さない）。
    Failed,
}

/// [`ImageWidget`] の画像ソース指定方法（[`super::label::LabelSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum ImageWidgetSource<S> {
    /// 固定のアセット識別子（パス・URL 等の不透明文字列。`fandhe-core` は I/O しない）。
    Static(String),
    /// アプリ状態 `S` から識別子を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    /// [`Widget::sync_from_state`] で `cached_source` を更新する。
    Bound(ReadFn<S, String>),
}

// `ImageWidgetSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の `S: Debug` / `S: Clone`
// 境界を回避する（`LabelSource` と同じ設計判断）。
impl<S> std::fmt::Debug for ImageWidgetSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageWidgetSource::Static(source) => f.debug_tuple("Static").field(source).finish(),
            ImageWidgetSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for ImageWidgetSource<S> {
    fn clone(&self) -> Self {
        match self {
            ImageWidgetSource::Static(source) => ImageWidgetSource::Static(source.clone()),
            ImageWidgetSource::Bound(get) => ImageWidgetSource::Bound(*get),
        }
    }
}

/// 固有サイズ（幅・高さ）を正規化する（非有限値・非正値は既定プレースホルダ寸法へ
/// フォールバック）。
///
/// 数値健全性の確保（OWASP A04 相当）のため、後続の `measure` がアスペクト比計算で
/// ゼロ除算・NaN 伝播を起こさないよう構築時点で無害化する（`slider::normalize_range`
/// と同じ判断）。
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

/// 画像ウィジェット（TASK-8.3-S35、`Widget<S>` 実装型、自己完結の暫定基本描画 +
/// 状態・イベント処理）。
///
/// `Debug` / `Clone` は手動実装する（状態型 `S` を所有していないため `derive` マクロが
/// 自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける）。ソース識別子・
/// 代替テキスト・寸法・ロード状態は秘密情報に該当しないため、`Debug` にそのまま含める。
pub struct ImageWidget<S> {
    id: WidgetId,
    source: ImageWidgetSource<S>,
    /// 代替テキスト（REQ-9 アクセシビリティ、`paint` の `Text` 出力に使用）。
    /// 開発者指定の文字列であり、ソース識別子（外部入力になり得る）とは独立させる。
    alt: String,
    /// 固有サイズ（幅・高さ）。構築時に [`normalize_intrinsic_size`] で正規化済み。
    intrinsic: Measurement,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在のソース識別子）。
    cached_source: String,
    /// ロード状態機械（本 issue の核心）。プラットフォームレイヤからの通知でのみ遷移する。
    load_phase: ImageLoadPhase,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for ImageWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageWidget")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("alt", &self.alt)
            .field("intrinsic", &self.intrinsic)
            .field("cached_source", &self.cached_source)
            .field("load_phase", &self.load_phase)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for ImageWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            alt: self.alt.clone(),
            intrinsic: self.intrinsic,
            cached_source: self.cached_source.clone(),
            load_phase: self.load_phase,
            interaction: self.interaction,
        }
    }
}

impl<S> ImageWidget<S> {
    /// 状態 `S` に依存しない固定のアセット識別子を持つ画像を構築する。
    ///
    /// `cached_source` は構築直後から `source` で初期化されるため、
    /// `sync_from_state` 呼び出し前でも `source_text` が期待どおりの識別子を返す
    /// （`Label::fixed` と同じ意味論）。`load_phase` は `Idle` で初期化する。
    pub fn fixed(id: WidgetId, source: impl Into<String>, alt: impl Into<String>) -> Self {
        let source = source.into();
        Self {
            id,
            cached_source: source.clone(),
            source: ImageWidgetSource::Static(source),
            alt: alt.into(),
            intrinsic: Measurement {
                width: DEFAULT_INTRINSIC_WIDTH,
                height: DEFAULT_INTRINSIC_HEIGHT,
            },
            load_phase: ImageLoadPhase::Idle,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたアセット識別子を持つ画像を構築する。
    ///
    /// `cached_source` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`Label::bound` と同じ判断）。
    /// `load_phase` は `Idle` で初期化する。
    pub fn bound(id: WidgetId, get: ReadFn<S, String>, alt: impl Into<String>) -> Self {
        Self {
            id,
            source: ImageWidgetSource::Bound(get),
            alt: alt.into(),
            intrinsic: Measurement {
                width: DEFAULT_INTRINSIC_WIDTH,
                height: DEFAULT_INTRINSIC_HEIGHT,
            },
            cached_source: String::new(),
            load_phase: ImageLoadPhase::Idle,
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

    /// 現在のロード状態を返す（プラットフォームレイヤが phase の視覚表現を選択する際に
    /// 参照する。モジュール doc 参照）。
    pub fn load_phase(&self) -> ImageLoadPhase {
        self.load_phase
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// 画像には中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む
    /// （`Slider::set_disabled` と同じ判断）。ロード通知は disabled と直交するため
    /// 本メソッドは `load_phase` に影響しない（モジュール doc §「disabled との直交性」）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// プラットフォームレイヤからロード開始を通知する（本 issue の核心 API）。
    ///
    /// `source` が現在の `cached_source` と一致しない場合は無視して `false` を返す
    /// （ソース差し替え後の stale 通知を遮断、モジュール doc 参照）。一致する場合は
    /// 任意の phase から `Loading` へ遷移し `true` を返す（リトライ・再ロードを許容）。
    pub fn notify_load_started(&mut self, source: &str) -> bool {
        if source != self.cached_source {
            return false;
        }
        self.load_phase = ImageLoadPhase::Loading;
        true
    }

    /// プラットフォームレイヤからロード成功を通知する。
    ///
    /// `source` が `cached_source` と一致し、かつ現在の phase が `Loading` の場合のみ
    /// `Loaded` へ遷移して `true` を返す。それ以外（ソース不一致・開始通知なしの完了
    /// 通知）は無視して `false` を返す（モジュール doc の遷移制約参照）。
    pub fn notify_load_succeeded(&mut self, source: &str) -> bool {
        if source != self.cached_source || self.load_phase != ImageLoadPhase::Loading {
            return false;
        }
        self.load_phase = ImageLoadPhase::Loaded;
        true
    }

    /// プラットフォームレイヤからロード失敗を通知する。
    ///
    /// 遷移制約は [`Self::notify_load_succeeded`] と同じ（`Loading` からのみ `Failed` へ）。
    pub fn notify_load_failed(&mut self, source: &str) -> bool {
        if source != self.cached_source || self.load_phase != ImageLoadPhase::Loading {
            return false;
        }
        self.load_phase = ImageLoadPhase::Failed;
        true
    }
}

impl<S: 'static> Widget<S> for ImageWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からソース識別子を読み取り `cached_source` を更新する
    /// （reconcile フェーズ）。ソースが変化した場合は `load_phase` を `Idle` へ
    /// リセットする（旧ソースの `Loaded` / `Failed` を新ソースへ持ち越さない、
    /// 本 issue の核心）。同一ソースなら phase を維持する。未配線の契約はモジュール
    /// doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        let new_source = match &self.source {
            ImageWidgetSource::Static(source) => source.clone(),
            ImageWidgetSource::Bound(get) => get(state),
        };
        if new_source != self.cached_source {
            self.load_phase = ImageLoadPhase::Idle;
        }
        self.cached_source = new_source;
    }

    /// 固有サイズをアスペクト比維持で `available` にフィットさせる（縮小のみ、
    /// 拡大はしない — content mode の Fit 相当）。
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
    /// `load_phase` の値に関わらず一定であり、ソース識別子（外部入力になり得る）は
    /// `Text` へ一切出さない（モジュール doc 参照、OWASP A03 対策）。phase の視覚表現
    /// （スピナー・エラーアイコン等）はプラットフォームレイヤの責務とする。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect, PaintCommand::Text(self.alt.clone())]
    }

    /// 画像は表示専用要素であり、常に `false` を返す（`Label::hit_test` と同じ
    /// 安全側の判断）。タップ可能画像等の対話は見送る（モジュール doc 参照）。
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

    /// 描画バックエンド（`fandhe-egui-proxy`・`fandhe-web-dom`）が具象型へ downcast して
    /// `source_text()` を読み、実画素を描くための口（写真選択、Issue #1103）。
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`super::image::Image::paint_styled`]
    /// と同型（Display カテゴリの既定マッピングを用い、構造を保ったまま色・
    /// タイポグラフィのみ差し替える）。
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

    // (a) 構築と各アクセサ・初期 load_phase

    #[test]
    fn fixed_returns_source_immediately_without_sync() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(1), "assets/logo.png", "ロゴ");

        assert_eq!(image.source_text(), "assets/logo.png");
        assert_eq!(image.alt_text(), "ロゴ");
        assert_eq!(image.id(), WidgetId::from_raw(1));
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn bound_is_empty_before_first_sync_and_idle() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(2), get_avatar_url, "アバター");

        assert_eq!(image.source_text(), "");
        assert_eq!(image.alt_text(), "アバター");
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn default_intrinsic_size_is_placeholder_constant() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(3), "assets/logo.png", "ロゴ");

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    // (b) with_intrinsic_size の正規化

    #[test]
    fn with_intrinsic_size_sets_positive_finite_dimensions() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(4), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(200.0, 100.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, 200.0);
        assert_eq!(size.height, 100.0);
    }

    #[test]
    fn with_intrinsic_size_falls_back_when_invalid() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(5), "assets/logo.png", "ロゴ")
                .with_intrinsic_size(f32::NAN, -1.0);

        let size = image.intrinsic_size();
        assert_eq!(size.width, DEFAULT_INTRINSIC_WIDTH);
        assert_eq!(size.height, DEFAULT_INTRINSIC_HEIGHT);
    }

    // (c) sync_from_state（Static 不変・Bound の状態追従・ソース変更時の phase リセット）

    #[test]
    fn sync_from_state_keeps_static_source_unchanged() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(9), "assets/logo.png", "ロゴ");
        let state = ProfileState {
            avatar_url: "https://example.com/avatar.png".to_string(),
        };

        image.sync_from_state(&state);

        assert_eq!(image.source_text(), "assets/logo.png");
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn sync_from_state_reflects_bound_state() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(10), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/avatar.png".to_string(),
        };

        image.sync_from_state(&state);

        assert_eq!(image.source_text(), "https://example.com/avatar.png");
    }

    #[test]
    fn sync_from_state_resets_phase_to_idle_when_source_changes() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(11), get_avatar_url, "アバター");
        let mut state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };
        image.sync_from_state(&state);
        assert!(image.notify_load_started("https://example.com/a.png"));
        assert!(image.notify_load_succeeded("https://example.com/a.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loaded);

        state.avatar_url = "https://example.com/b.png".to_string();
        image.sync_from_state(&state);

        assert_eq!(image.source_text(), "https://example.com/b.png");
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn sync_from_state_keeps_phase_when_source_unchanged() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(12), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };
        // 初回 sync は "" → "https://example.com/a.png" の変化なので Idle のまま
        // （既に Idle のため観測不能）。2 回目の同一ソース sync が本テストの本題。
        image.sync_from_state(&state);
        assert!(image.notify_load_started("https://example.com/a.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loading);

        image.sync_from_state(&state);

        assert_eq!(image.load_phase(), ImageLoadPhase::Loading);
    }

    // (d) notify_load_started / succeeded / failed の遷移制約

    #[test]
    fn notify_load_started_transitions_idle_to_loading() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(13), "assets/logo.png", "ロゴ");
        let mut image = image;

        assert!(image.notify_load_started("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loading);
    }

    #[test]
    fn notify_load_started_allows_retry_from_failed() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(14), "assets/logo.png", "ロゴ");
        assert!(image.notify_load_started("assets/logo.png"));
        assert!(image.notify_load_failed("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Failed);

        assert!(image.notify_load_started("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loading);
    }

    #[test]
    fn notify_load_started_ignores_mismatched_source() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(15), "assets/logo.png", "ロゴ");

        let applied = image.notify_load_started("assets/other.png");

        assert!(!applied);
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn notify_load_succeeded_transitions_loading_to_loaded() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(16), "assets/logo.png", "ロゴ");
        image.notify_load_started("assets/logo.png");

        assert!(image.notify_load_succeeded("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loaded);
    }

    #[test]
    fn notify_load_succeeded_ignored_when_not_loading() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(17), "assets/logo.png", "ロゴ");

        // Idle からの完了通知（開始通知なし）は無視される。
        assert!(!image.notify_load_succeeded("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);

        image.notify_load_started("assets/logo.png");
        image.notify_load_failed("assets/logo.png");
        // Failed からの完了通知も無視される。
        assert!(!image.notify_load_succeeded("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Failed);
    }

    #[test]
    fn notify_load_failed_transitions_loading_to_failed() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(18), "assets/logo.png", "ロゴ");
        image.notify_load_started("assets/logo.png");

        assert!(image.notify_load_failed("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Failed);
    }

    #[test]
    fn notify_load_failed_ignores_stale_source_after_resync() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(19), get_avatar_url, "アバター");
        let mut state = ProfileState {
            avatar_url: "https://example.com/old.png".to_string(),
        };
        image.sync_from_state(&state);
        image.notify_load_started("https://example.com/old.png");

        // ソース差し替え後に旧ソースの失敗通知が届く（非同期順序逆転を模擬）。
        state.avatar_url = "https://example.com/new.png".to_string();
        image.sync_from_state(&state);
        let applied = image.notify_load_failed("https://example.com/old.png");

        assert!(!applied);
        assert_eq!(image.load_phase(), ImageLoadPhase::Idle);
    }

    #[test]
    fn notify_accepted_while_disabled() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(20), "assets/logo.png", "ロゴ");
        image.set_disabled(true);

        assert!(image.notify_load_started("assets/logo.png"));
        assert!(image.notify_load_succeeded("assets/logo.png"));
        assert_eq!(image.load_phase(), ImageLoadPhase::Loaded);
    }

    // (e) paint（load_phase に依らず一定・ソース文字列の非混入）

    #[test]
    fn paint_returns_rect_and_alt_text_regardless_of_phase() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(21), get_avatar_url, "プロフィール画像");
        let state = ProfileState {
            avatar_url: "https://example.com/secret-token.png".to_string(),
        };
        image.sync_from_state(&state);
        image.notify_load_started("https://example.com/secret-token.png");
        image.notify_load_failed("https://example.com/secret-token.png");

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
        for command in &commands {
            if let PaintCommand::Text(text) = command {
                assert!(!text.contains("secret-token"));
            }
        }
    }

    // (f) hit_test は常に false

    #[test]
    fn hit_test_is_always_false_regardless_of_phase() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(22), "assets/logo.png", "ロゴ");
        image.notify_load_started("assets/logo.png");
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };

        assert!(!image.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    // (g) accessibility_role / clone_box（load_phase を含む独立コピー）

    #[test]
    fn accessibility_role_is_image() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(23), "assets/logo.png", "ロゴ");

        assert_eq!(image.accessibility_role(), AccessibilityRole::Image);
    }

    #[test]
    fn clone_box_preserves_load_phase_independently() {
        let mut image: ImageWidget<ProfileState> =
            ImageWidget::bound(WidgetId::from_raw(24), get_avatar_url, "アバター");
        let state = ProfileState {
            avatar_url: "https://example.com/a.png".to_string(),
        };
        image.sync_from_state(&state);
        image.notify_load_started("https://example.com/a.png");
        image.notify_load_succeeded("https://example.com/a.png");
        assert_eq!(image.load_phase(), ImageLoadPhase::Loaded);

        let mut cloned = image.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(24));

        // clone_box は `dyn Widget<S>` を返すため load_phase は直接読めないが、
        // クローン先だけソースを変えて phase をリセットさせ、元へ波及しないことで
        // 独立コピーであることを確認する（`Image::clone_box` と同じ検証手法）。
        let other_state = ProfileState {
            avatar_url: "https://example.com/b.png".to_string(),
        };
        cloned.sync_from_state(&other_state);

        // クローン元は影響を受けない（独立性の検証）。
        assert_eq!(image.load_phase(), ImageLoadPhase::Loaded);
        assert_eq!(image.source_text(), "https://example.com/a.png");
    }

    #[test]
    fn debug_output_does_not_panic() {
        let image: ImageWidget<ProfileState> =
            ImageWidget::fixed(WidgetId::from_raw(25), "assets/logo.png", "ロゴ");
        let output = format!("{image:?}");
        assert!(output.contains("ImageWidget"));
    }

    #[test]
    fn image_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ImageWidget<ProfileState>>();
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let image: ImageWidget<ProfileState> = ImageWidget::fixed(
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
