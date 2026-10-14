//! ツールチップウィジェットの状態・イベント処理（TASK-8.3-S56 / REQ-8、Issue #151）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の 56/60。
//! ツールチップの状態・イベント処理（S56 #151）を担当する。
//! [`super::icon_widget::IconWidget`]（S38、#133）/ [`super::image_widget::ImageWidget`]
//! （S35、#130）が採った判断（既存 main の公開 API のみに依存する自己完結実装を新設し、
//! `tooltip::Tooltip<S>` との型統合は別作業に委ねる）を踏襲し、[`Widget<S>`] を実装する
//! 自己完結のツールチップウィジェット型 [`TooltipWidget`] を本 issue で新設する（型名を
//! `TooltipWidget`、ファイル名を `tooltip_widget.rs` とし、S55 が導入予定の
//! `tooltip::Tooltip<S>` との将来的な衝突を避ける）。
//!
//! ## 可視性状態機械（本 issue の核心）
//!
//! [`TooltipVisibility`] は `Hidden → PendingShow（表示遅延カウント中）→ Visible` の
//! 一方向遷移と、任意状態からの `Hidden` への即時復帰で構成する。
//!
//! - `PointerEntered`（hover 開始）: `Hidden → PendingShow { remaining_secs: show_delay_secs }`
//!   （構築時に設定する表示遅延秒数だけ待ってから表示する）
//! - `FocusGained`: `Hidden → Visible`（**即時表示**。WAI-ARIA のツールチップ慣例に合わせ、
//!   キーボード操作では表示遅延を挟まない。REQ-9 接続点）
//! - `PointerExited` / `FocusLost`: いずれの状態からも `Hidden` へ即時遷移する
//!   （`PendingShow` 中のキャンセルを含む）
//! - `set_disabled(true)`: `Hidden` へ強制遷移し、以降の入力・`advance` を無視する
//!   （[`InteractionState::set_disabled`] の意味論に整合）
//! - [`TooltipWidget::advance`]（`elapsed_secs` 引数）: `PendingShow` の残り時間を
//!   減算し、0 以下で `Visible` へ遷移させる。[`super::progress::ProgressWidget::advance`]
//!   （S32、#127）の契約を踏襲し、壁時計をウィジェット内に持たずフレーム駆動で
//!   呼び出し元が経過秒を渡す。負値・非有限値（NaN/inf）は no-op とする
//!   （OWASP A04 対策、安全側の判断）。
//!
//! ツールチップの可視性はアプリ状態 `S` に属さないウィジェット内部状態であるため、
//! `apply_input` は [`InteractionState::apply`] へ委譲しつつ上記の可視性遷移を駆動するのみで、
//! `state::Event` へのブリッジは行わない（`&mut S` を公開しない AI 型安全 DSL 方針に
//! 整合。[`super::checkbox::CheckboxWidget`] がトグル確定を `Event` へブリッジしたのとは
//! 対照的な設計判断）。
//!
//! `sync_from_state` はツールチップ本文を [`ReadFn<S, String>`] 経由で表示用キャッシュ
//! `cached_text` へ反映する制御型パターンを [`super::icon_widget::IconWidget`] と同様に
//! 採用し、静的テキスト版（[`TooltipWidget::new_static`]）と状態連動版
//! （[`TooltipWidget::new_dynamic`]）の 2 系統を提供する。`hit_test` は常に `false`
//! （ツールチップ overlay 自体は非インタラクティブ。ホバー判定はホスト側ウィジェットの
//! 領域で行われ、`InteractionInput` として本 widget へ流入する契約）。`paint` は
//! `Visible` のときのみ `PaintCommand::Text` で `cached_text` を出力し、`Hidden` /
//! `PendingShow` では空 `Vec` を返す。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠）:
//! (a) S55（#150）マージ後の型統合（`tooltip::Tooltip<S>` との統合・`Screen` ビルダー配線）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S57 / #152）、
//! (c) ツールチップの配置計算（アンカー・画面端回避、S55 の基本描画スコープ）、
//! (d) プラットフォームレイヤからの実タイマー・実ポインタ配線。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。検証は `Widget`
//! メソッドの直接呼び出しで行う。

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// [`TooltipWidget::new_static`] / [`TooltipWidget::new_dynamic`] が指定しない場合の
/// 既定表示遅延秒数（ホバー開始から `Visible` へ遷移するまでの秒数）。
///
/// 非有限値・負値の指定に対するフォールバック先としても使う
/// （[`normalize_show_delay`] 参照、OWASP A04 対策）。
const DEFAULT_SHOW_DELAY_SECS: f32 = 0.5;

/// 表示遅延秒数を正規化する（非有限値・負値は既定値へフォールバック）。
///
/// `0.0` は「遅延なし即時表示」として有効な値のため許容する。数値健全性の確保
/// （OWASP A04 相当）のため、後続の [`TooltipVisibility::advance`] が NaN 伝播・
/// 無限ループを起こさないよう構築時点で無害化する
/// （`icon_widget::normalize_icon_size` と同じ判断）。
fn normalize_show_delay(show_delay_secs: f32) -> f32 {
    if show_delay_secs.is_finite() && show_delay_secs >= 0.0 {
        show_delay_secs
    } else {
        DEFAULT_SHOW_DELAY_SECS
    }
}

/// [`Widget::measure`] が返す 1 次元分の値を非負・有限値へ健全化する
/// （Review 指摘対応: `f32::max` は NaN を無害化するが `+f32::INFINITY` はそのまま
/// 素通りするため、`is_finite` を明示的に確認して置き換える。OWASP A04 対策）。
fn normalize_measurement_dimension(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// ツールチップの可視性状態（本 issue の中核。モジュール doc の遷移規則を参照）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TooltipVisibility {
    /// 非表示（初期状態）。
    Hidden,
    /// ホバー開始済み・表示遅延の残り時間をカウント中。
    PendingShow {
        /// 表示までの残り秒数（`0.0` 以下で `Visible` へ遷移、モジュール doc 参照）。
        remaining_secs: f32,
    },
    /// 表示中。
    Visible,
}

impl TooltipVisibility {
    /// `elapsed_secs` だけ時間を前進させ、`PendingShow` の残り時間を減算する。
    ///
    /// - `Hidden` / `Visible` では意味を持たないため no-op（`ProgressWidget::advance` が
    ///   `Determinate` モードで no-op とするのと同型の判断）。
    /// - `elapsed_secs` が負値・非有限値（NaN/inf）の場合は no-op（安全側、OWASP A04 対策）。
    /// - 残り時間が `elapsed_secs` 以下になった場合は `Visible` へ遷移する（負の残り時間を
    ///   保持しない、飽和的な遷移）。
    fn advance(self, elapsed_secs: f32) -> Self {
        let TooltipVisibility::PendingShow { remaining_secs } = self else {
            return self;
        };
        if !elapsed_secs.is_finite() || elapsed_secs < 0.0 {
            return self;
        }
        let remaining = remaining_secs - elapsed_secs;
        if remaining <= 0.0 {
            TooltipVisibility::Visible
        } else {
            TooltipVisibility::PendingShow {
                remaining_secs: remaining,
            }
        }
    }
}

/// [`TooltipWidget`] のツールチップ本文指定方法（[`super::icon_widget::IconWidgetSource`] /
/// [`super::label::LabelSource`] の前例を踏襲）。
///
/// REQ-13（NFR-9）の方針に従い、状態束縛はクロージャ（環境キャプチャ）を許さず
/// 関数ポインタ（[`ReadFn<S, String>`]、実体は `fn(&S) -> String`）に限定する。
pub enum TooltipTextSource<S> {
    /// 状態 `S` に依存しない固定のツールチップ本文。
    Static(String),
    /// アプリ状態 `S` から本文を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    /// [`Widget::sync_from_state`] で `cached_text` を更新する。
    Bound(ReadFn<S, String>),
}

// `TooltipTextSource<S>` は `String` と関数ポインタ（`Copy`）のみで構成されるため、
// 状態 `S` を所有しない手動 `Debug` / `Clone` 実装で `derive` の `S: Debug` / `S: Clone`
// 境界を回避する（`IconWidgetSource` と同じ設計判断）。
impl<S> std::fmt::Debug for TooltipTextSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TooltipTextSource::Static(text) => f.debug_tuple("Static").field(text).finish(),
            TooltipTextSource::Bound(_) => f.debug_tuple("Bound").field(&"<fn ptr>").finish(),
        }
    }
}

impl<S> Clone for TooltipTextSource<S> {
    fn clone(&self) -> Self {
        match self {
            TooltipTextSource::Static(text) => TooltipTextSource::Static(text.clone()),
            TooltipTextSource::Bound(get) => TooltipTextSource::Bound(*get),
        }
    }
}

/// ツールチップウィジェット（TASK-8.3-S56、`Widget<S>` 実装型、可視性状態機械 +
/// 状態・イベント処理）。
///
/// `Debug` / `Clone` は手動実装する（状態型 `S` を所有していないため `derive` マクロが
/// 自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける、`IconWidget` と
/// 同じ判断）。
pub struct TooltipWidget<S> {
    id: WidgetId,
    source: TooltipTextSource<S>,
    /// [`Widget::sync_from_state`] が更新する表示用キャッシュ（現在のツールチップ本文）。
    cached_text: String,
    /// ホバー開始から `Visible` へ遷移するまでの表示遅延秒数（構築時に
    /// [`normalize_show_delay`] で正規化済み）。
    show_delay_secs: f32,
    /// 現在の可視性状態（モジュール doc の遷移規則を参照）。
    visibility: TooltipVisibility,
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for TooltipWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TooltipWidget")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("cached_text", &self.cached_text)
            .field("show_delay_secs", &self.show_delay_secs)
            .field("visibility", &self.visibility)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for TooltipWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            source: self.source.clone(),
            cached_text: self.cached_text.clone(),
            show_delay_secs: self.show_delay_secs,
            visibility: self.visibility,
            interaction: self.interaction,
        }
    }
}

impl<S> TooltipWidget<S> {
    /// 状態 `S` に依存しない固定のツールチップ本文を持つツールチップを構築する。
    ///
    /// `cached_text` は構築直後から `text` で初期化されるため、`sync_from_state`
    /// 呼び出し前でも `text()` が期待どおりの本文を返す（`IconWidget::fixed` と同じ
    /// 意味論）。可視性は `Hidden`（初期状態）から始まる。
    pub fn new_static(id: WidgetId, text: impl Into<String>, show_delay_secs: f32) -> Self {
        let text = text.into();
        Self {
            id,
            cached_text: text.clone(),
            source: TooltipTextSource::Static(text),
            show_delay_secs: normalize_show_delay(show_delay_secs),
            visibility: TooltipVisibility::Hidden,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に束縛されたツールチップ本文を持つツールチップを構築する。
    ///
    /// `cached_text` は構築直後は空文字列であり、初回 `sync_from_state` 呼び出しまで
    /// 表示に反映されない（reconcile フェーズの意味論、`IconWidget::bound` と同じ判断）。
    pub fn new_dynamic(id: WidgetId, get: ReadFn<S, String>, show_delay_secs: f32) -> Self {
        Self {
            id,
            source: TooltipTextSource::Bound(get),
            cached_text: String::new(),
            show_delay_secs: normalize_show_delay(show_delay_secs),
            visibility: TooltipVisibility::Hidden,
            interaction: InteractionState::default(),
        }
    }

    /// 現在の表示キャッシュ値（ツールチップ本文）を返す（テスト・観測用）。
    pub fn text(&self) -> &str {
        &self.cached_text
    }

    /// 表示遅延秒数（構築時に正規化済み）を返す。
    pub fn show_delay_secs(&self) -> f32 {
        self.show_delay_secs
    }

    /// 現在の可視性状態を返す。
    pub fn visibility(&self) -> TooltipVisibility {
        self.visibility
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える（[`InteractionState::set_disabled`] への委譲）。
    ///
    /// `true` の場合は可視性を `Hidden` へ強制遷移させる（モジュール doc 参照。
    /// 無効化されたホスト要素のツールチップが表示され続ける不整合を防ぐ）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.visibility = TooltipVisibility::Hidden;
        }
    }

    /// ポインタ・フォーカス入力を適用し、可視性状態機械を駆動する
    /// （[`InteractionState::apply`] への委譲 + モジュール doc の遷移規則）。
    ///
    /// disabled 中は [`InteractionState::apply`] が入力を無視するため
    /// [`InteractionState::disabled`] を確認したうえで可視性遷移も合わせて抑止する
    /// （`IconWidget::apply_input` と同型の判断。可視性遷移だけが disabled を
    /// すり抜けて進行する不整合を防ぐ）。
    ///
    /// `show_delay_secs` が `0.0`（遅延なし即時表示指定）の場合は `PointerEntered` を
    /// `advance` を待たず直接 `Visible` へ遷移させる（`0.0` 以下の残り時間を
    /// `PendingShow` のまま保持しない、[`Self::advance`] の飽和的遷移と整合する判断）。
    ///
    /// hover（`hovered`）と focus（`focused`）は独立した入力系統であり、ホスト要素は
    /// 両方を同時に満たしうる（例: キーボードでフォーカスした要素にマウスも乗せる）。
    /// このため `PointerExited` / `FocusLost` は自分側の入力のみを見て即 `Hidden` にはせず、
    /// [`InteractionState::apply`] 適用後の**もう一方の状態**（`interaction.focused` /
    /// `interaction.hovered`）が残っている場合は `Hidden` へ遷移させない（WAI-ARIA の
    /// 「hover または focus のいずれかが有効な間は表示を維持する」慣例に整合。モジュール
    /// doc 参照）。同様に `PointerEntered` は既に `Visible`（`FocusGained` 由来）の場合は
    /// `PendingShow` へ格下げしない（表示済みツールチップが hover 開始だけで一時的に消える
    /// 不整合を防ぐ）。
    ///
    /// また `PendingShow` 中に `PointerEntered` を重複受信しても `remaining_secs` を
    /// `show_delay_secs` へリセットしない（Review 指摘対応: プラットフォームレイヤから
    /// 重複した `PointerEntered` が飛来し続けた場合に表示が無期限に遅延する事態を防ぐ、
    /// 安全側の判断）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
        if self.interaction.disabled {
            return;
        }
        self.visibility = match input {
            InteractionInput::PointerEntered if self.visibility == TooltipVisibility::Visible => {
                // 既に Visible（FocusGained 由来等）であれば hover 開始で格下げしない。
                self.visibility
            }
            InteractionInput::PointerEntered
                if matches!(self.visibility, TooltipVisibility::PendingShow { .. }) =>
            {
                // 既に PendingShow（表示遅延カウント中）であれば重複した PointerEntered で
                // remaining_secs をリセットしない（無期限遅延の防止、モジュール doc 参照）。
                self.visibility
            }
            InteractionInput::PointerEntered if self.show_delay_secs <= 0.0 => {
                TooltipVisibility::Visible
            }
            InteractionInput::PointerEntered => TooltipVisibility::PendingShow {
                remaining_secs: self.show_delay_secs,
            },
            InteractionInput::FocusGained => TooltipVisibility::Visible,
            InteractionInput::PointerExited if self.interaction.focused => {
                // focus がまだ有効なら hover 解除だけでは隠さない。
                self.visibility
            }
            InteractionInput::FocusLost if self.interaction.hovered => {
                // hover がまだ有効なら focus 解除だけでは隠さない。
                self.visibility
            }
            InteractionInput::PointerExited | InteractionInput::FocusLost => {
                TooltipVisibility::Hidden
            }
            // 押下・解放はツールチップの可視性遷移に関与しない
            // （ホスト要素側の責務、モジュール doc 参照）。
            InteractionInput::PointerPressed | InteractionInput::PointerReleased => self.visibility,
        };
    }

    /// `elapsed_secs` だけ時間を前進させ、`PendingShow` の残り時間を減算する
    /// （`TooltipVisibility` 内部の遷移ロジックへの委譲、モジュール doc 参照）。
    ///
    /// フレーム駆動の契約であり、壁時計をウィジェット内に持たない
    /// （`ProgressWidget::advance` と同じ判断）。disabled 中は `Hidden` に固定されて
    /// いるため呼び出しても no-op（`advance` は `Hidden` を素通りするのみ）。
    pub fn advance(&mut self, elapsed_secs: f32) {
        self.visibility = self.visibility.advance(elapsed_secs);
    }
}

impl<S: 'static> Widget<S> for TooltipWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からツールチップ本文を読み取り `cached_text` を更新する
    /// （reconcile フェーズ）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        match &self.source {
            TooltipTextSource::Static(text) => {
                // 固定本文は状態に依存しないため不変。呼び出し自体は許容し、
                // reconcile ループの呼び出し規約（全ウィジェット一律呼び出し）を崩さない。
                self.cached_text = text.clone();
            }
            TooltipTextSource::Bound(get) => {
                self.cached_text = get(state);
            }
        }
    }

    /// ツールチップは overlay 配置のためレイアウトエンジンからの制約をそのまま
    /// 消費せず、`available` を非負・有限値にクランプした値をそのまま返す（配置計算・
    /// テキスト量に応じた最小サイズ算出自体は S55 の基本描画スコープであり、
    /// ここでは負値・非有限値（NaN/inf）混入を防ぐ健全化のみを行う。モジュール doc 参照）。
    fn measure(&self, available: Measurement) -> Measurement {
        Measurement {
            width: normalize_measurement_dimension(available.width),
            height: normalize_measurement_dimension(available.height),
        }
    }

    /// `Visible` のときのみ `[Text(cached_text)]` を返す。`Hidden` / `PendingShow` では
    /// 空 `Vec` を返す（モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        match self.visibility {
            TooltipVisibility::Visible => vec![PaintCommand::Text(self.cached_text.clone())],
            TooltipVisibility::Hidden | TooltipVisibility::PendingShow { .. } => Vec::new(),
        }
    }

    /// ツールチップ overlay 自体は常に `false`（非インタラクティブ）。ホバー判定は
    /// ホスト側ウィジェットの領域で行われ、`InteractionInput` として本 widget へ
    /// 流入する契約（モジュール doc 参照）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    /// 新設の `AccessibilityRole::Tooltip` を返す（`widget.rs` モジュール doc 参照）。
    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Tooltip
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AppState {
        hint: String,
    }

    fn read_hint(state: &AppState) -> String {
        state.hint.clone()
    }

    fn measurement(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    // (a) 構築・sync_from_state による cached_text 更新

    #[test]
    fn new_static_returns_text_immediately_without_sync() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(1), "保存します", 0.5);

        assert_eq!(tooltip.text(), "保存します");
        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    #[test]
    fn new_dynamic_is_empty_before_first_sync() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_dynamic(WidgetId::from_raw(2), read_hint, 0.5);

        assert_eq!(tooltip.text(), "");
    }

    #[test]
    fn sync_from_state_reflects_bound_state_across_updates() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_dynamic(WidgetId::from_raw(3), read_hint, 0.5);

        tooltip.sync_from_state(&AppState {
            hint: "元に戻す".to_string(),
        });
        assert_eq!(tooltip.text(), "元に戻す");

        tooltip.sync_from_state(&AppState {
            hint: "やり直す".to_string(),
        });
        assert_eq!(tooltip.text(), "やり直す");
    }

    #[test]
    fn sync_from_state_keeps_static_text_unchanged() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(4), "保存します", 0.5);

        tooltip.sync_from_state(&AppState {
            hint: "無視される".to_string(),
        });

        assert_eq!(tooltip.text(), "保存します");
    }

    // (b) hover 開始 → PendingShow → advance 累積で Visible へ遷移

    #[test]
    fn pointer_entered_transitions_to_pending_show() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(5), "ヒント", 0.5);

        tooltip.apply_input(InteractionInput::PointerEntered);

        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.5
            }
        );
    }

    #[test]
    fn advance_accumulates_and_transitions_to_visible() {
        // 0.5 / 0.25 は二進浮動小数点で正確に表現できる値のため、減算の丸め誤差を
        // 気にせず等価比較できる（浮動小数点の精度に依存しないテスト設計）。
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(6), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);

        tooltip.advance(0.25);
        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.25
            }
        );

        tooltip.advance(0.25);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    #[test]
    fn advance_transitions_to_visible_when_overshooting_remaining() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(7), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);

        tooltip.advance(10.0);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    // (c) 遅延満了前の PointerExited でキャンセルされ Hidden へ戻る

    #[test]
    fn pointer_exited_cancels_pending_show() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(8), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);
        tooltip.advance(0.1);

        tooltip.apply_input(InteractionInput::PointerExited);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    #[test]
    fn pointer_exited_hides_visible_tooltip_shown_by_hover_alone() {
        // hover のみで表示されたツールチップ（focus は無関係）は PointerExited で Hidden になる。
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(9), "ヒント", 0.0);
        tooltip.apply_input(InteractionInput::PointerEntered);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        tooltip.apply_input(InteractionInput::PointerExited);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    #[test]
    fn pointer_exited_does_not_hide_focus_only_visible_tooltip() {
        // Review 指摘対応: FocusGained のみで表示中の場合、hover を伴わない
        // PointerExited（例: 別要素からのポインタ通過等）で誤って隠さない。
        // focused=true が維持されている限り Visible のまま。
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(9), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        tooltip.apply_input(InteractionInput::PointerExited);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
        assert!(tooltip.interaction.focused);
        assert!(!tooltip.interaction.hovered);
    }

    // (c-2) hover と focus が併存する場合、片方の解除だけでは Hidden にならない
    // （Review 指摘対応: hovered / focused を独立に扱わないことによる誤動作の再発防止）

    #[test]
    fn focus_gained_then_pointer_entered_stays_visible_without_downgrade() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(20), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        // 表示中に同じ要素へ hover 開始しても、PendingShow へ格下げしない。
        tooltip.apply_input(InteractionInput::PointerEntered);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    #[test]
    fn pointer_exited_does_not_hide_while_still_focused() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(21), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        tooltip.apply_input(InteractionInput::PointerEntered);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        // まだ focus が有効なため、hover 解除だけでは Hidden にならない。
        tooltip.apply_input(InteractionInput::PointerExited);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
        assert!(!tooltip.interaction.hovered);
        assert!(tooltip.interaction.focused);
    }

    #[test]
    fn focus_lost_does_not_hide_while_still_hovered() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(22), "ヒント", 0.0);
        tooltip.apply_input(InteractionInput::PointerEntered);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
        tooltip.apply_input(InteractionInput::FocusGained);

        // まだ hover が有効なため、focus 解除だけでは Hidden にならない。
        tooltip.apply_input(InteractionInput::FocusLost);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
        assert!(tooltip.interaction.hovered);
        assert!(!tooltip.interaction.focused);
    }

    #[test]
    fn pointer_exited_then_focus_lost_hides_after_both_cleared() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(23), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        tooltip.apply_input(InteractionInput::PointerEntered);
        tooltip.apply_input(InteractionInput::PointerExited);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        // 残っていた focus も解除されると、ようやく Hidden へ遷移する。
        tooltip.apply_input(InteractionInput::FocusLost);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    // (d) FocusGained で即時 Visible、FocusLost で Hidden

    #[test]
    fn focus_gained_shows_immediately_without_delay() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(10), "ヒント", 5.0);

        tooltip.apply_input(InteractionInput::FocusGained);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    #[test]
    fn focus_lost_hides_tooltip() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(11), "ヒント", 0.0);
        tooltip.apply_input(InteractionInput::FocusGained);

        tooltip.apply_input(InteractionInput::FocusLost);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    // (e) set_disabled(true) で表示中でも Hidden へ強制遷移し、以降の入力・advance を無視

    #[test]
    fn set_disabled_forces_hidden_even_when_visible() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(12), "ヒント", 0.0);
        tooltip.apply_input(InteractionInput::FocusGained);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        tooltip.set_disabled(true);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
        assert!(tooltip.interaction_state().disabled);
    }

    #[test]
    fn disabled_ignores_subsequent_input_and_advance() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(13), "ヒント", 0.5);
        tooltip.set_disabled(true);

        tooltip.apply_input(InteractionInput::PointerEntered);
        tooltip.apply_input(InteractionInput::FocusGained);
        tooltip.advance(1.0);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
    }

    #[test]
    fn re_enabling_does_not_auto_show_tooltip() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(14), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        tooltip.set_disabled(true);

        tooltip.set_disabled(false);

        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);
        assert!(!tooltip.interaction_state().disabled);
    }

    // (f) advance の負値・NaN/inf が no-op

    #[test]
    fn advance_is_no_op_for_negative_elapsed() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(15), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);

        tooltip.advance(-1.0);

        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.5
            }
        );
    }

    #[test]
    fn advance_is_no_op_for_non_finite_elapsed() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(16), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);

        tooltip.advance(f32::NAN);
        tooltip.advance(f32::INFINITY);
        tooltip.advance(f32::NEG_INFINITY);

        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.5
            }
        );
    }

    #[test]
    fn advance_is_no_op_for_hidden_or_visible_states() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(17), "ヒント", 0.5);

        tooltip.advance(0.3);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Hidden);

        tooltip.apply_input(InteractionInput::FocusGained);
        tooltip.advance(0.3);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    // (g) show_delay_secs の正規化（非有限値・負値のフォールバック、0.0 は有効値）

    #[test]
    fn show_delay_normalizes_invalid_values_to_default() {
        let tooltip_nan: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(18), "ヒント", f32::NAN);
        let tooltip_negative: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(19), "ヒント", -1.0);

        assert_eq!(tooltip_nan.show_delay_secs(), DEFAULT_SHOW_DELAY_SECS);
        assert_eq!(tooltip_negative.show_delay_secs(), DEFAULT_SHOW_DELAY_SECS);
    }

    #[test]
    fn show_delay_zero_is_valid_and_preserved() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(20), "ヒント", 0.0);

        assert_eq!(tooltip.show_delay_secs(), 0.0);

        let mut tooltip = tooltip;
        tooltip.apply_input(InteractionInput::PointerEntered);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }

    // (h) sync_from_state が cached_text を更新し、paint は Visible 時のみ Text を出力

    #[test]
    fn paint_is_empty_when_hidden_or_pending() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(21), "ヒント", 0.5);

        assert_eq!(
            tooltip.paint((0.0, 0.0), measurement(100.0, 20.0)),
            Vec::new()
        );

        tooltip.apply_input(InteractionInput::PointerEntered);
        assert_eq!(
            tooltip.paint((0.0, 0.0), measurement(100.0, 20.0)),
            Vec::new()
        );
    }

    #[test]
    fn paint_outputs_text_only_when_visible() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(22), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);

        let commands = tooltip.paint((0.0, 0.0), measurement(100.0, 20.0));

        assert_eq!(commands, vec![PaintCommand::Text("ヒント".to_string())]);
    }

    #[test]
    fn paint_reflects_bound_text_only_when_visible() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_dynamic(WidgetId::from_raw(23), read_hint, 0.0);
        tooltip.sync_from_state(&AppState {
            hint: "現在の状態".to_string(),
        });
        tooltip.apply_input(InteractionInput::FocusGained);

        let commands = tooltip.paint((0.0, 0.0), measurement(100.0, 20.0));

        assert_eq!(commands, vec![PaintCommand::Text("現在の状態".to_string())]);
    }

    // (i) hit_test が常に false

    #[test]
    fn hit_test_is_always_false_even_when_visible() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(24), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::FocusGained);
        let size = measurement(100.0, 20.0);

        assert!(!tooltip.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!tooltip.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    // (j) 付随の Widget<S> 契約（id / accessibility_role / clone_box / object safety）

    #[test]
    fn accessibility_role_is_tooltip() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(25), "ヒント", 0.5);
        assert_eq!(tooltip.accessibility_role(), AccessibilityRole::Tooltip);
    }

    #[test]
    fn clone_box_preserves_visibility_and_text() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_dynamic(WidgetId::from_raw(26), read_hint, 0.0);
        tooltip.sync_from_state(&AppState {
            hint: "共有中".to_string(),
        });
        tooltip.apply_input(InteractionInput::FocusGained);

        let boxed: Box<dyn Widget<AppState>> = Box::new(tooltip);
        let cloned = boxed.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(26));
        assert_eq!(
            cloned.paint((0.0, 0.0), measurement(100.0, 20.0)),
            vec![PaintCommand::Text("共有中".to_string())]
        );
    }

    #[test]
    fn debug_output_does_not_panic() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(27), "ヒント", 0.5);
        let output = format!("{tooltip:?}");
        assert!(output.contains("TooltipWidget"));
    }

    #[test]
    fn tooltip_widget_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<AppState>> = Box::new(TooltipWidget::new_static(
            WidgetId::from_raw(28),
            "ヒント",
            0.5,
        ));
        assert_eq!(widget.accessibility_role(), AccessibilityRole::Tooltip);
        assert!(!widget.interaction_state().disabled);
    }

    #[test]
    fn tooltip_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TooltipWidget<AppState>>();
    }

    // (j-2) Review 指摘対応: PendingShow 中の重複 PointerEntered は remaining_secs を
    // リセットしない（無期限遅延の防止）

    #[test]
    fn duplicate_pointer_entered_during_pending_show_does_not_reset_remaining_secs() {
        // 0.5 / 0.25 は二進浮動小数点で正確に表現できる値のため、減算の丸め誤差を
        // 気にせず等価比較できる（既存テストと同じ設計判断）。
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(30), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);
        tooltip.advance(0.25);
        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.25
            }
        );

        // 重複した PointerEntered（プラットフォームレイヤからの再送等）を受けても、
        // 既に経過した残り時間はリセットされない。
        tooltip.apply_input(InteractionInput::PointerEntered);

        assert_eq!(
            tooltip.visibility(),
            TooltipVisibility::PendingShow {
                remaining_secs: 0.25
            }
        );
    }

    // (j-3) Review 指摘対応: measure() が +infinity を 0.0 へ健全化する
    // （f32::max は NaN のみ無害化し inf は素通りするため、is_finite を明示確認）

    #[test]
    fn measure_normalizes_non_finite_available_to_zero() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(31), "ヒント", 0.5);

        let measured = tooltip.measure(measurement(f32::INFINITY, f32::NAN));

        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, 0.0);
    }

    #[test]
    fn measure_clamps_negative_available_to_zero() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(32), "ヒント", 0.5);

        let measured = tooltip.measure(measurement(-10.0, -5.0));

        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, 0.0);
    }

    #[test]
    fn measure_preserves_finite_non_negative_available() {
        let tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(33), "ヒント", 0.5);

        let measured = tooltip.measure(measurement(100.0, 20.0));

        assert_eq!(measured.width, 100.0);
        assert_eq!(measured.height, 20.0);
    }

    // (k) press/release は可視性遷移に関与しない

    #[test]
    fn pointer_pressed_and_released_do_not_affect_visibility() {
        let mut tooltip: TooltipWidget<AppState> =
            TooltipWidget::new_static(WidgetId::from_raw(29), "ヒント", 0.5);
        tooltip.apply_input(InteractionInput::PointerEntered);
        tooltip.advance(0.5);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        tooltip.apply_input(InteractionInput::PointerPressed);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);

        tooltip.apply_input(InteractionInput::PointerReleased);
        assert_eq!(tooltip.visibility(), TooltipVisibility::Visible);
    }
}
