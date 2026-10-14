//! トグルスイッチウィジェット `ToggleSwitch<S>`（TASK-8.3-S25 / REQ-8、Issue #120）。
//!
//! 親 issue #95（TASK-8.3、60 sub-issue）の 25/60。トグルスイッチの 3 工程
//! （S25: API 設計・基本描画 → S26: 状態・イベント処理 → S27: テスト・カタログ登録）の
//! うち第 1 工程であり、ウィジェット基盤（`Widget<S>` トレイト・`InteractionState`・
//! ヒットテスト・レイアウト・`WidgetHarness`・`WidgetCatalog`）と、同型の先行ウィジェット
//! [`super::checkbox::Checkbox`]（S19、Issue #114）が確立したパターンを踏襲し、
//! `Widget<S>` を実装する具体型 [`ToggleSwitch`] を新設して基本描画（measure / paint /
//! hit_test / accessibility_role）まで実装する。
//!
//! - チェック状態と同じく、on/off 状態の SSOT はアプリ状態 `S`。ウィジェットは
//!   [`crate::state::ReadFn`] 経由の [`Widget::sync_from_state`] で表示用キャッシュ
//!   `cached_on: bool` を更新するのみで、中間編集状態を持たない
//!   （`Checkbox` と同じ構造上の理由）。
//! - `set: WriteFn<S, bool>` はイベント配線（S26）に備えたシグネチャ確保のみで、
//!   呼び出し経路（トグルディスパッチ）は実装しない（`Checkbox::bound` /
//!   `Checkbox::write_fn` と同じ判断）。
//! - [`super::widget::AccessibilityRole::Switch`] を本ウィジェット専用のロールとして
//!   `widget.rs` へ加法的に追加する（`accesskit::Role::Switch` へ対応予定）。
//!   `AccessibilityRole::CheckBox` を流用しない理由は、プラットフォーム側の
//!   アクセシビリティロール（SwiftUI `Toggle` / accesskit `Switch`）と 1:1 対応させる
//!   ためである。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す — `Checkbox` S19 と同じ判断）。描画列は
//!   `[Rect（トラック）, Rect（サム）, （on 時のみ）Text(ON マーク定数), Text(label)]` とし、
//!   ヘッドレス環境でも on/off がコマンド列の差分として検証できる構成にする。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。reconcile/sync パスが
//! `Node::Widget` へ実際に `&mut` を届けるまで、上記 3 箇所は
//! `Node::Widget` を状態非依存のプレースホルダとして扱う契約を維持する。検証は
//! `Widget` メソッドの直接呼び出し（＋必要なら [`super::harness::WidgetHarness`]）で行う。
//!
//! TASK-8.3-S26（REQ-8、Issue #121）: トグル確定・キーボード活性化・disabled/focus
//! 連動を実装する。[`super::checkbox::CheckboxWidget`]（Issue #115）のように別型を
//! 新設するのではなく、既存 [`ToggleSwitch<S>`] へ直接メソッドを追加する
//! （label/text_field/checkbox で発生した「同型ウィジェットが 2 つ存在する」問題を
//! 繰り返さないための判断）。
//!
//! - トグル確定（ポインタ・キーボード活性化）は既存の [`crate::state::Event::ButtonClicked`]
//!   へブリッジするのみで、widget 層から `&mut S` へ直接触れない（`CheckboxWidget` と
//!   同じブリッジ設計）。専用イベントバリアント `Event::ToggleSwitchChanged` の新設は
//!   `Node<S>` enum への variant 追加が設計文書 §4.1 の温存方針に反するためスコープ外
//!   とする（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）。
//! - ポインタ確定は [`InteractionState::apply`] へ入力を適用する**前**の
//!   `pressed && hovered` で判定する（press → 領域外 exit → release では発火しない、
//!   `CheckboxWidget::apply_input` と同一の意味論）。
//! - キーボード活性化は新設 [`ToggleSwitchInput`]（Space/Enter 相当）で表現し、
//!   生キーコード・文字列を扱わず意味づけ済み列挙のみで構成する（OWASP A03 対策、
//!   `CheckboxInput` と同型）。`focused` かつ非 `disabled` のときのみ発火する。
//! - disabled は [`InteractionState::set_disabled`] へ委譲するのみ（bool に中間編集
//!   状態がなく blur-commit 不要、`CheckboxWidget::set_disabled` と同じ）。
//!
//! **S27（Issue #122）で実施済み**: `WidgetCatalog` への `toggle_switch` エントリ登録
//! （`catalog.rs`）・結合テストの網羅拡充（`tests/toggle_switch_widget.rs`）。S27 の
//! テストは `interaction_state()` 等 S25 の API のみを対象とし、S26 の
//! `apply_input` / `handle_input` / `ToggleSwitchInput` のイベント配線検証は
//! Issue #121（S26）側のユニットテストが担う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) `Event::ToggleSwitchChanged` 専用バリアント新設 + `WidgetId` ベースディスパッチ統合、
//! (b) アニメーション（サムのスライド遷移）、(c) reconcile パス未配線の既知契約
//! （上記参照）。

use crate::state::{Event, ReadFn, WidgetValue, WriteFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数であり、
/// `measure` の折り返し・複数行はスコープ外（`Checkbox` と同じ既知の制約）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// トグルスイッチのトラック（背景の帯）の幅（論理ピクセル相当）。
const TRACK_WIDTH: f32 = 36.0;

/// トグルスイッチのトラックの高さ（論理ピクセル相当）。
const TRACK_HEIGHT: f32 = 20.0;

/// トラックとラベルの間隔（論理ピクセル相当）。
const TRACK_LABEL_GAP: f32 = 8.0;

/// on 状態を表す描画テキスト（`PaintCommand::Text` へ渡す簡易マーク）。
///
/// `PaintCommand` を拡張しない方針（モジュール doc 参照）のため、on/off の有無を
/// テキスト内容の差分として表現する（`Checkbox::CHECK_MARK` と同じ判断）。
const ON_MARK: &str = "ON";

/// トグルスイッチへのキーボード入力を表す列挙（TASK-8.3-S26、`ToggleSwitch<S>` の核心）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`checkbox::CheckboxInput` /
/// `widget::InteractionInput` と同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleSwitchInput {
    /// on/off のトグルを確定する（Space/Enter 相当）。
    Toggle,
}

/// トグルスイッチウィジェット（TASK-8.3-S25、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Checkbox` と同じく、状態型 `S` を所有していない
/// ため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける）。
/// on/off（`bool`）・ラベルは秘密情報に該当しないため、`Debug` にそのまま含める。
pub struct ToggleSwitch<S> {
    id: WidgetId,
    label: String,
    /// アプリ状態 `S` から on/off を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    get: ReadFn<S, bool>,
    /// トグル結果を状態 `S` へ反映する関数ポインタ。実際のイベント配線は
    /// 後続工程（S26）が別途提供する。
    set: WriteFn<S, bool>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`is_on()` が返す値）。
    cached_on: bool,
    /// hover/pressed/disabled/focus。実際のトグル配線は後続工程（S26）のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for ToggleSwitch<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToggleSwitch")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("cached_on", &self.cached_on)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for ToggleSwitch<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            get: self.get,
            set: self.set,
            cached_on: self.cached_on,
            interaction: self.interaction,
        }
    }
}

impl<S> ToggleSwitch<S> {
    /// 状態 `S` に束縛したトグルスイッチを作る（ビルダー形式の起点）。
    ///
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`] / [`WriteFn`]）
    /// と代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。`set` をコンストラクタに含める理由は、後続工程で
    /// ビルダーシグネチャを破壊変更しないためであり、トグル自体の書き込み・イベント
    /// ディスパッチは後続工程（S26）のスコープとする（`Checkbox::bound` と同じ判断）。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        get: ReadFn<S, bool>,
        set: WriteFn<S, bool>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            get,
            set,
            cached_on: false,
            interaction: InteractionState::default(),
        }
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// `cached_on`（直近の [`Widget::sync_from_state`] が読み取った on/off）を返す。
    pub fn is_on(&self) -> bool {
        self.cached_on
    }

    /// イベント配線が使う書き込み経路を返す。
    ///
    /// この関数ポインタを呼び出す経路（トグルディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる
    /// （`Checkbox::write_fn` と同型）。
    pub fn write_fn(&self) -> WriteFn<S, bool> {
        self.set
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// on/off（bool）には中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `text_field::TextFieldWidget::set_disabled` のような blur-commit は不要で、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む
    /// （`checkbox::CheckboxWidget::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する（TASK-8.3-S26 の核心）。
    ///
    /// トグル確定の判定は [`InteractionState::apply`] へ入力を適用する**前**の
    /// `pressed && hovered` を見て行う。`PointerReleased` 適用前に判定することで、
    /// press → 領域外へ exit（`pressed` が解除される）→ release という経路では
    /// 発火しない（press-and-drag-away の意味論、`checkbox::CheckboxWidget::apply_input` と
    /// 同じく `InteractionState` の既存意味論をそのまま利用する設計）。disabled 中は
    /// `InteractionState::apply` が入力そのものを無視するため、`pressed` が立たず
    /// トグル確定も発火しない。
    pub fn apply_input(&mut self, input: InteractionInput) -> Option<Event> {
        let should_confirm = matches!(input, InteractionInput::PointerReleased)
            && self.interaction.pressed
            && self.interaction.hovered;

        self.interaction.apply(input);

        if should_confirm {
            return Some(Event::ButtonClicked {
                label: self.label.clone(),
            });
        }
        None
    }

    /// キーボード入力を処理する（TASK-8.3-S26 の核心）。
    ///
    /// disabled 中・非フォーカス中はすべての入力を無視する（`checkbox::CheckboxWidget` と
    /// 同じ「フォーカス保持中のみ操作可能」という設計判断）。
    pub fn handle_input(&mut self, input: ToggleSwitchInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        match input {
            ToggleSwitchInput::Toggle => Some(Event::ButtonClicked {
                label: self.label.clone(),
            }),
        }
    }
}

impl<S: 'static> Widget<S> for ToggleSwitch<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から on/off を読み取り `cached_on` を更新する
    /// （reconcile フェーズ）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_on = (self.get)(state);
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = トラック幅 + トラック・ラベル間ギャップ + ラベル幅を `available.width` で
    /// クランプする。高さ = トラック高さと行高の大きい方を `available.height` で
    /// クランプする。マルチバイトは `chars().count()` 基準の近似（`Checkbox` と同じ
    /// 既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (TRACK_WIDTH + TRACK_LABEL_GAP + label_width).min(available.width);
        let height = TRACK_HEIGHT.max(APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// `[Rect（トラック）, Rect（サム）, （on 時のみ）Text(ON マーク), Text(label)]` の
    /// 描画プリミティブ列を返す。
    ///
    /// ヘッドレス環境でも on/off がコマンド列の差分として検証できる構成とし、
    /// `PaintCommand` 自体は拡張しない（モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect, PaintCommand::Rect];

        if self.cached_on {
            commands.push(PaintCommand::Text(ON_MARK.to_string()));
        }
        commands.push(PaintCommand::Text(self.label.clone()));

        commands
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたトグルスイッチを入力対象にしない（安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.interaction.disabled {
            return false;
        }
        point.0 >= origin.0
            && point.0 < origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Switch
    }

    /// スイッチのラベル文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
    fn accessibility_label(&self) -> Option<String> {
        Some(self.label.clone())
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Input カテゴリの既定マッピング
    /// （`style_tokens::input_style_refs`）を用い、`paint()` と同じ構造を保った
    /// まま色・タイポグラフィのみを解決済み具体値へ差し替える。解決失敗時は
    /// `paint()` の無スタイル列へフォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::input_style_refs() else {
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

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// `WidgetValue::Bool` を `write_fn()` で状態へ書き込む（Issue #805 / 親 #804）。
    /// `Number` / `Index` 等の型不一致は `false` を返し、状態を変更しない
    /// （`Checkbox::apply_value` と同型）。
    fn apply_value(&self, state: &mut S, value: &WidgetValue) -> bool {
        match value {
            WidgetValue::Bool(v) => {
                (self.write_fn())(state, *v);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FormState {
        notifications_on: bool,
    }

    fn get_notifications_on(state: &FormState) -> bool {
        state.notifications_on
    }

    fn set_notifications_on(state: &mut FormState, value: bool) {
        state.notifications_on = value;
    }

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_and_off_default() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        assert_eq!(toggle.label_text(), "通知");
        assert!(!toggle.is_on());
        assert_eq!(toggle.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let write_fn = toggle.write_fn();
        let mut state = FormState {
            notifications_on: false,
        };
        write_fn(&mut state, true);
        assert!(state.notifications_on);
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_updates_cached_on_from_get() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let state = FormState {
            notifications_on: true,
        };

        toggle.sync_from_state(&state);

        assert!(toggle.is_on());
    }

    #[test]
    fn sync_from_state_follows_state_changes_across_calls() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let mut state = FormState {
            notifications_on: true,
        };

        toggle.sync_from_state(&state);
        assert!(toggle.is_on());

        state.notifications_on = false;
        toggle.sync_from_state(&state);
        assert!(!toggle.is_on());
    }

    // (c) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "メモ",
            get_notifications_on,
            set_notifications_on,
        );

        let measurement = toggle.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // トラック + ギャップ + ラベル(2 文字) 分の近似幅。
        let expected_width = TRACK_WIDTH + TRACK_LABEL_GAP + (2.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, TRACK_HEIGHT.max(APPROX_LINE_HEIGHT));
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです非常に長いラベルです",
            get_notifications_on,
            set_notifications_on,
        );

        let measurement = toggle.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "あ",
            get_notifications_on,
            set_notifications_on,
        );

        let measurement = toggle.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // "あ" は 1 文字（3 バイト）。バイト数基準なら幅がずれてしまう回帰の防止。
        let expected_width = TRACK_WIDTH + TRACK_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
    }

    // (d) paint

    #[test]
    fn paint_returns_track_thumb_on_mark_and_label_when_on() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let state = FormState {
            notifications_on: true,
        };
        toggle.sync_from_state(&state);

        let commands = toggle.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text(ON_MARK.to_string()),
                PaintCommand::Text("通知".to_string()),
            ]
        );
    }

    #[test]
    fn paint_omits_on_mark_when_off() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let commands = toggle.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("通知".to_string()),
            ]
        );
    }

    // (e) hit_test / accessibility_role / clone_box / Debug

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let hit = toggle.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(hit);
    }

    #[test]
    fn hit_test_returns_false_when_point_outside_rect() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let hit = toggle.hit_test(
            (500.0, 500.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        // `Rect::contains` の半開区間仕様（[origin, origin + size)）に合わせ、
        // 右端・下端ちょうどの点は非ヒットとする（隣接ウィジェットとの境界重複を避ける）。
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let hit = toggle.hit_test(
            (100.0, 20.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_for_zero_size_at_origin() {
        // 0 サイズの矩形は半開区間では常に非ヒットになる。
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let hit = toggle.hit_test(
            (0.0, 0.0),
            (0.0, 0.0),
            Measurement {
                width: 0.0,
                height: 0.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_point_inside_rect() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        toggle.interaction.set_disabled(true);

        let hit = toggle.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    // (e-1) interaction_state（TASK-8.3-S27 / Issue #122 でのテスト拡充。`RadioButton`
    // S24 / Issue #119 の先例と同じ観点）

    #[test]
    fn interaction_state_defaults_to_not_disabled() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        assert!(!toggle.interaction_state().disabled);
    }

    #[test]
    fn interaction_state_reflects_disabled_flag() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        toggle.interaction.set_disabled(true);

        assert!(toggle.interaction_state().disabled);
    }

    #[test]
    fn accessibility_role_is_switch() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        assert_eq!(toggle.accessibility_role(), AccessibilityRole::Switch);
    }

    #[test]
    fn accessibility_label_is_toggle_label() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        assert_eq!(toggle.accessibility_label(), Some("通知".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let state = FormState {
            notifications_on: true,
        };
        toggle.sync_from_state(&state);

        let mut cloned = toggle.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState {
            notifications_on: false,
        };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert!(toggle.is_on());
        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            !cloned_commands.contains(&PaintCommand::Text(ON_MARK.to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn debug_output_contains_cached_on() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let state = FormState {
            notifications_on: true,
        };
        toggle.sync_from_state(&state);

        let output = format!("{toggle:?}");

        assert!(output.contains("cached_on"));
        assert!(output.contains("true"));
    }

    // (f) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let state = FormState {
            notifications_on: true,
        };

        toggle.sync_from_state(&state);
        let measurement = toggle.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = toggle.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text(ON_MARK.to_string()),
                PaintCommand::Text("通知".to_string()),
            ]
        );
    }

    // (g) 状態・イベント処理（TASK-8.3-S26）

    use crate::state::StateStore;
    use crate::ui::Screen;

    fn toggle_notifications_on(state: &mut FormState) {
        state.notifications_on = !state.notifications_on;
    }

    fn focus(toggle: &mut ToggleSwitch<FormState>) {
        toggle.apply_input(InteractionInput::FocusGained);
    }

    #[test]
    fn pointer_press_and_release_inside_confirms_toggle() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(201),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        toggle.apply_input(InteractionInput::PointerEntered);
        toggle.apply_input(InteractionInput::PointerPressed);
        let event = toggle.apply_input(InteractionInput::PointerReleased);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "通知".to_string(),
            })
        );
    }

    #[test]
    fn pointer_press_and_drag_away_does_not_confirm_toggle() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(202),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        toggle.apply_input(InteractionInput::PointerEntered);
        toggle.apply_input(InteractionInput::PointerPressed);
        toggle.apply_input(InteractionInput::PointerExited);
        let event = toggle.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn release_without_prior_press_does_not_confirm_toggle() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(203),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        toggle.apply_input(InteractionInput::PointerEntered);
        let event = toggle.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_toggle_confirms_when_focused() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(204),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        focus(&mut toggle);

        let event = toggle.handle_input(ToggleSwitchInput::Toggle);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "通知".to_string(),
            })
        );
    }

    #[test]
    fn keyboard_toggle_ignored_when_not_focused() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(205),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );

        let event = toggle.handle_input(ToggleSwitchInput::Toggle);

        assert_eq!(event, None);
    }

    #[test]
    fn disabled_toggle_switch_ignores_all_input() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(206),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        focus(&mut toggle);
        toggle.set_disabled(true);

        assert!(!toggle.interaction_state().focused);

        toggle.apply_input(InteractionInput::PointerEntered);
        toggle.apply_input(InteractionInput::PointerPressed);
        let pointer_event = toggle.apply_input(InteractionInput::PointerReleased);
        let keyboard_event = toggle.handle_input(ToggleSwitchInput::Toggle);

        assert_eq!(pointer_event, None);
        assert_eq!(keyboard_event, None);
    }

    #[test]
    fn hit_test_returns_false_when_disabled_after_set_disabled() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(207),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        toggle.set_disabled(true);

        let hit = toggle.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(208),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        toggle.apply_input(InteractionInput::PointerEntered);

        assert_eq!(toggle.visual_state(), VisualState::Hovered);

        toggle.set_disabled(true);
        assert_eq!(toggle.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn state_store_dispatch_toggles_state_via_legacy_button_bridge() {
        // 宣言層はレガシー `.button(label, toggle_handler)` で受け、
        // `StateStore::dispatch` を経由して状態へ反映後、`sync_from_state` で
        // 新値が還流することを確認する（モジュール doc の設計判断の E2E 検証。
        // `checkbox::CheckboxWidget` の同名テストと同型）。
        let screen: Screen<FormState> =
            Screen::new("form").button(WidgetId::from_raw(1), "通知", toggle_notifications_on);
        let mut store = StateStore::new(FormState {
            notifications_on: false,
        });
        let mut toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(209),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        focus(&mut toggle);

        let event = toggle
            .handle_input(ToggleSwitchInput::Toggle)
            .expect("フォーカス中の Toggle は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert!(store.state().notifications_on);
        assert_eq!(store.version(), 1);

        toggle.sync_from_state(store.state());
        assert!(toggle.is_on());

        // もう 1 回で元へ戻る（トグル往復）。
        let event = toggle
            .handle_input(ToggleSwitchInput::Toggle)
            .expect("フォーカス中の Toggle は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert!(!store.state().notifications_on);
        assert_eq!(store.version(), 2);

        toggle.sync_from_state(store.state());
        assert!(!toggle.is_on());
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let toggle: ToggleSwitch<FormState> = ToggleSwitch::bound(
            WidgetId::from_raw(200),
            "通知",
            get_notifications_on,
            set_notifications_on,
        );
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert_eq!(
            toggle.paint_styled((0.0, 0.0), size, &empty_resolver),
            toggle.paint((0.0, 0.0), size)
        );
    }
}
