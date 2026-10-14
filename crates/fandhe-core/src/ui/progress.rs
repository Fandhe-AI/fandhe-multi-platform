//! プログレス表示ウィジェット `ProgressBar<S>` / `Spinner<S>`（TASK-8.3-S31 / REQ-8、
//! Issue #126）と、その状態・イベント処理を担う `ProgressWidget<S>` /
//! `ProgressMode`（TASK-8.3-S32 / REQ-8、Issue #127）。
//!
//! `ProgressBar` / `Spinner` は、ウィジェット基盤（`Widget<S>` トレイト・
//! `InteractionState`・ヒットテスト・レイアウト・`WidgetHarness`・`WidgetCatalog`）と、
//! 同型の先行ウィジェット [`super::checkbox::Checkbox`]（Issue #114）・
//! [`super::toggle_switch::ToggleSwitch`]（Issue #120）が確立したパターンを踏襲し、
//! `Widget<S>` を実装する具体型 [`ProgressBar`]（determinate）・[`Spinner`]
//! （indeterminate）として実装している。
//!
//! **`ProgressBar` / `Spinner` と `ProgressWidget` の型分離について**:
//! `checkbox::CheckboxWidget` と同じ判断で `Widget<S>` を実装する自己完結の最小型
//! [`ProgressWidget`] / [`ProgressMode`] を独立に実装している。`ProgressBar` /
//! `Spinner` と `ProgressWidget` は名前が異なり実装も独立しているため、統合は行わず
//! 両立させている（`label::Label` / `label::LabelWidget`、`checkbox::Checkbox` /
//! `checkbox::CheckboxWidget` と同じ判断。統合作業自体は別 issue のスコープ、
//! `.claude/rules/out-of-scope-tracking.md` 準拠）。ファイル名も `checkbox.rs` /
//! `label.rs` の前例に倣い `progress.rs` へ両者を共存させる。
//!
//! - **`ProgressBar<S>`**: 進捗値（0.0..=1.0）の SSOT はアプリ状態 `S`。ウィジェットは
//!   [`crate::state::ReadFn`] 経由の [`Widget::sync_from_state`] で表示用キャッシュ
//!   `cached_fraction: f32` を更新するのみで、中間編集状態を持たない
//!   （`Checkbox` / `ToggleSwitch` と同じ構造上の理由）。**`WriteFn` は持たない**
//!   （プログレスは表示専用でユーザー編集対象でないため。`Checkbox::bound` /
//!   `ToggleSwitch::bound` が `set` を先行確保したのは後続工程でトグル配線が
//!   あったためであり、プログレスには該当しない — 安全側の最小 API とする）。
//! - **`Spinner<S>`**: indeterminate であり進捗値を持たない。状態バインディングも
//!   持たず、`sync_from_state` は no-op とする。型パラメータ `S` はビルダー API
//!   （`Screen<S>::spinner`）との整合のためのマーカーであり、`PhantomData<fn(&S)>`
//!   で表現する（`S` の所有権・drop 順序に影響を与えない不変マーカー）。
//! - **`ProgressWidget<S>` / `ProgressMode`**（S32、本モジュールの状態・イベント処理
//!   実装）: determinate モードは `state::ReadFn<S, f32>` 経由の `sync_from_state` のみが
//!   表示用キャッシュ `cached_fraction` を更新する制御型（`[0.0, 1.0]` クランプ・
//!   非有限値の `0.0` フォールバック）、indeterminate モードはアニメーション位相を
//!   [`ProgressWidget::advance`]（`elapsed_secs` 引数）で前進させる（壁時計は
//!   プラットフォームレイヤが駆動、本クレートは純粋関数のみ提供。負値・非有限値は
//!   no-op、OWASP A04 対策）。`ProgressBar` / `Spinner` 同様、非インタラクティブ要素
//!   であり `hit_test` は常に `false`。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す — `Checkbox` S19 / `ToggleSwitch` S25 と
//!   同じ判断）。`ProgressBar` の描画列は `[Rect（トラック）, （fraction > 0.0 のときのみ）
//!   Rect（フィル）, Text(進捗率文字列 例 "42%"), （label 非空時）Text(label)]`、`Spinner`
//!   は `[Rect（プレースホルダ枠）, Text(回転マーク定数)]`、`ProgressWidget` は
//!   determinate/indeterminate いずれも `[Rect（トラック）, Rect（フィル／走行セグメント）,
//!   Text(パーセント表記)]` とする。値・位相の差分はいずれもコマンド列上の `Text` 側で
//!   表現する（`f32` から内部生成し外部入力文字列を経由しない、OWASP A03 対策）。
//! - `hit_test` はいずれも常に `false`（表示専用。[`super::label::Label`] と同じ判断）。
//! - `accessibility_role` はいずれも [`AccessibilityRole::ProgressIndicator`]
//!   （バー・スピナー共用。accesskit `Role::ProgressIndicator` / SwiftUI `ProgressView`
//!   が双方を包含するため専用ロールを分けない）。
//! - `Debug` / `Clone` は手動実装する（`Checkbox` / `ToggleSwitch` と同じく、状態型 `S` を
//!   所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
//!   不要な境界を避ける）。進捗値・ラベルは非機密のため `Debug` にそのまま含める。
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
//! (a) `ProgressBar` / `Spinner` と `ProgressWidget` / `ProgressMode` の型統合、
//! (b) ~~`WidgetCatalog` 登録・テスト網羅拡充（S33 #128）~~ — S33（Issue #128）で完了
//! （`catalog.rs` の `progress_bar` / `spinner` エントリ、`tests/progress_widget.rs`
//! 参照）、(c) フィル矩形の実寸（進捗率に応じた幅）描画（`PaintCommand` の幾何情報拡張後の
//! 別作業）、(d) reconcile パス（`&mut` を `Node::Widget` へ届ける経路）未配線の既知契約
//! （上記参照、S3 系の別作業）。

use std::marker::PhantomData;

use crate::state::ReadFn;

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// プログレスバーのトラック（背景の帯）の幅（論理ピクセル相当）。
const BAR_WIDTH: f32 = 160.0;

/// プログレスバーのトラックの高さ（論理ピクセル相当）。
const BAR_HEIGHT: f32 = 8.0;

/// バーとラベルの間隔（論理ピクセル相当）。
const BAR_LABEL_GAP: f32 = 8.0;

/// スピナーの正方形サイズ（論理ピクセル相当）。
const SPINNER_SIZE: f32 = 16.0;

/// スピナーの回転を表す簡易マーク（`PaintCommand::Text` へ渡すプレースホルダ）。
///
/// `PaintCommand` を拡張しない方針（モジュール doc 参照）のため、実際の回転位相・
/// フレーム駆動は持たず固定文字列を返す（S32 #127 のスコープ）。
const SPINNER_MARK: &str = "\u{27F3}";

const TRACK_WIDTH: f32 = 120.0;
/// トラック矩形の近似高さ（論理ピクセル相当のプレースホルダ定数）。
const TRACK_HEIGHT: f32 = 8.0;

/// 0.0..=1.0 の範囲外・非有限値（NaN / ±∞）を安全側へ正規化する（`ProgressBar` 用）。
///
/// `f32::clamp` は NaN を伝播する（パニックはしないが NaN のまま返る）ため、
/// 先に `is_finite` で弾いてから `clamp` する順序が必須（clamp を先に呼ぶと
/// NaN が生き残る）。異常値がパーセント文字列生成（`as u32` キャスト）・描画層へ
/// 伝播しないようにする信頼境界（OWASP A03、`.claude/rules/security.md` 準拠）。
fn normalize_fraction(value: f32) -> f32 {
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

pub struct ProgressBar<S> {
    id: WidgetId,
    label: String,
    /// アプリ状態 `S` から進捗値（0.0..=1.0 想定）を読み取る関数ポインタ
    /// （REQ-13、環境キャプチャ不可）。
    get: ReadFn<S, f32>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（[`Self::fraction`] が返す値）。
    /// [`normalize_fraction`] により常に `0.0..=1.0` の有限値に正規化済み。
    cached_fraction: f32,
    /// hover/pressed/disabled/focus。プログレスは表示専用のため実際の入力連動は
    /// 持たないが、他ウィジェットとの `Widget<S>` 契約統一のため保持する。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for ProgressBar<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressBar")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("cached_fraction", &self.cached_fraction)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for ProgressBar<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            get: self.get,
            cached_fraction: self.cached_fraction,
            interaction: self.interaction,
        }
    }
}

impl<S> ProgressBar<S> {
    /// 状態 `S` に束縛したプログレスバーを作る（ビルダー形式の起点）。
    ///
    /// `get` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`]）と代入互換の
    /// 関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで拒否する）。
    /// `WriteFn` を持たない理由はモジュール doc 参照。
    pub fn bound(id: WidgetId, label: impl Into<String>, get: ReadFn<S, f32>) -> Self {
        Self {
            id,
            label: label.into(),
            get,
            cached_fraction: 0.0,
            interaction: InteractionState::default(),
        }
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// `cached_fraction`（直近の [`Widget::sync_from_state`] が読み取り正規化した進捗値、
    /// 常に `0.0..=1.0` の有限値）を返す。
    pub fn fraction(&self) -> f32 {
        self.cached_fraction
    }
}

impl<S: 'static> Widget<S> for ProgressBar<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から進捗値を読み取り、[`normalize_fraction`] で正規化した上で
    /// `cached_fraction` を更新する（reconcile フェーズ）。未配線の契約はモジュール doc
    /// を参照。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_fraction = normalize_fraction((self.get)(state));
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = バー幅 + バー・ラベル間ギャップ + ラベル幅を `available.width` でクランプする。
    /// 高さ = バー高さと行高の大きい方を `available.height` でクランプする。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (BAR_WIDTH + BAR_LABEL_GAP + label_width).min(available.width);
        let height = BAR_HEIGHT.max(APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// `[Rect（トラック）, （fraction > 0.0 のときのみ）Rect（フィル）,
    /// Text(進捗率文字列), （label 非空時）Text(label)]` の描画プリミティブ列を返す。
    ///
    /// ヘッドレス環境でも進捗値がコマンド列の差分（フィル Rect の有無・パーセント
    /// 文字列）として検証できる構成とし、`PaintCommand` 自体は拡張しない
    /// （モジュール doc 参照）。パーセント文字列生成は `cached_fraction` が
    /// [`normalize_fraction`] 済みのため `as u32` キャストが安全に成立する。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];

        if self.cached_fraction > 0.0 {
            commands.push(PaintCommand::Rect);
        }

        let percent = (self.cached_fraction * 100.0).round() as u32;
        commands.push(PaintCommand::Text(format!("{percent}%")));

        if !self.label.is_empty() {
            commands.push(PaintCommand::Text(self.label.clone()));
        }

        commands
    }

    /// 常に `false` を返す（表示専用ウィジェット、[`super::label::Label`] と同じ判断）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ProgressIndicator
    }

    /// ラベル文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
    /// 空文字列の場合はアクセシブルネームなし（`None`）として扱う。
    fn accessibility_label(&self) -> Option<String> {
        if self.label.is_empty() {
            None
        } else {
            Some(self.label.clone())
        }
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Display カテゴリの既定マッピング
    /// （`style_tokens::display_style_refs`）を用い、`paint()` と同じ構造を保った
    /// まま色・タイポグラフィのみを解決済み具体値へ差し替える。解決失敗時は
    /// `paint()` の無スタイル列へフォールバックする。
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

/// スピナーウィジェット（TASK-8.3-S31、`Widget<S>` 実装型、indeterminate）。
///
/// 状態バインディングを持たず、`sync_from_state` は no-op とする（回転位相・
/// active 連動などの状態・イベント処理は S32 #127 のスコープ、モジュール doc 参照）。
/// `Debug` / `Clone` は手動実装する（モジュール doc 参照）。
pub struct Spinner<S> {
    id: WidgetId,
    /// hover/pressed/disabled/focus。プログレスは表示専用のため実際の入力連動は
    /// 持たないが、他ウィジェットとの `Widget<S>` 契約統一のため保持する。
    interaction: InteractionState,
    /// `Screen<S>::spinner` ビルダー API との型整合のためのマーカー。状態を
    /// 所有しないため `PhantomData<fn(&S)>`（`PhantomData<S>` ではなく関数ポインタ形）で
    /// 表現し、`S` の所有権・drop 順序・auto-trait 推論への影響を避ける。
    _state: PhantomData<fn(&S)>,
}

impl<S> std::fmt::Debug for Spinner<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spinner")
            .field("id", &self.id)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Spinner<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            interaction: self.interaction,
            _state: PhantomData,
        }
    }
}

impl<S> Spinner<S> {
    /// 状態バインディングを持たないスピナーを作る（ビルダー形式の起点）。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            interaction: InteractionState::default(),
            _state: PhantomData,
        }
    }
}

impl<S: 'static> Widget<S> for Spinner<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// 状態バインディングを持たないため no-op（モジュール doc 参照）。
    fn sync_from_state(&mut self, _state: &S) {}

    /// 正方形近似で必要寸法を返す（`SPINNER_SIZE` を `available` でクランプする）。
    fn measure(&self, available: Measurement) -> Measurement {
        Measurement {
            width: SPINNER_SIZE.min(available.width),
            height: SPINNER_SIZE.min(available.height),
        }
    }

    /// `[Rect（プレースホルダ枠）, Text(回転マーク定数)]` の描画プリミティブ列を返す。
    ///
    /// 実際の回転アニメーションは持たず固定文字列を返す（S32 #127 のスコープ、
    /// モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![
            PaintCommand::Rect,
            PaintCommand::Text(SPINNER_MARK.to_string()),
        ]
    }

    /// 常に `false` を返す（表示専用ウィジェット、[`super::label::Label`] と同じ判断）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ProgressIndicator
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`ProgressBar::paint_styled`] と
    /// 同型（Display カテゴリの既定マッピングを用い、構造を保ったまま色・
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

/// プログレス表示の意味づけ済みモード列挙（`ProgressWidget` 用）。
///
/// 生の真偽値ではなく列挙にすることで、呼び出し側の分岐が意図を失わないようにする
/// （OWASP A03「意味づけ済み入力のみで分岐する」方針、モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressMode {
    /// 進捗値が既知のバー表示（`cached_fraction` を参照する）。
    Determinate,
    /// 進捗値が不明なスピナー・走行バー表示（`phase` を参照する）。
    Indeterminate,
}

/// プログレス表示ウィジェット（TASK-8.3-S32 で新設する自己完結の最小実装）。
///
/// `Debug` / `Clone` は `LabelWidget` / `Slider` と同じ理由（状態 `S` を所有しない
/// フィールドのみで構成されるため、`derive` が付与する不要な `S: Debug` /
/// `S: Clone` 境界を避ける）で手動実装する。
pub struct ProgressWidget<S> {
    id: WidgetId,
    mode: ProgressMode,
    /// 状態 `S` から進捗値（`[0.0, 1.0]` 想定）を読み取る関数ポインタ
    /// （REQ-13 / NFR-9、クロージャ不可）。`Indeterminate` モードでは `None` のまま。
    value_source: Option<ReadFn<S, f32>>,
    /// 直近の `sync_from_state` で確定した表示用キャッシュ値。常に `[0.0, 1.0]` へ
    /// クランプ済み（非有限値は `0.0` へフォールバック済み）。
    cached_fraction: f32,
    /// indeterminate アニメーションの現在位相。`[0.0, 1.0)` の範囲で循環する。
    phase: f32,
    interaction: InteractionState,
}

impl<S> ProgressWidget<S> {
    /// 状態 `S` に束縛された determinate（バー）プログレス表示を構築する。
    ///
    /// `cached_fraction` は最初の `sync_from_state` 呼び出しまで `0.0`
    /// （`LabelWidget::new_dynamic` と異なり、進捗値は「未計測」を意味する既定値として
    /// 妥当なため初期値引数を設けない）。
    pub fn new_determinate(id: WidgetId, value_source: ReadFn<S, f32>) -> Self {
        Self {
            id,
            mode: ProgressMode::Determinate,
            value_source: Some(value_source),
            cached_fraction: 0.0,
            phase: 0.0,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` に依存しない indeterminate（スピナー）プログレス表示を構築する。
    ///
    /// `sync_from_state` は no-op（`value_source` を持たないため）。位相の前進は
    /// `advance` が担う。
    pub fn new_indeterminate(id: WidgetId) -> Self {
        Self {
            id,
            mode: ProgressMode::Indeterminate,
            value_source: None,
            cached_fraction: 0.0,
            phase: 0.0,
            interaction: InteractionState::default(),
        }
    }

    /// 進捗値を `[0.0, 1.0]` へクランプし、非有限値（NaN/inf）は `0.0` へ
    /// フォールバックする（`Slider` S28 の数値健全性判断を踏襲、OWASP A04 対策）。
    fn normalize_fraction(value: f32) -> f32 {
        if !value.is_finite() {
            return 0.0;
        }
        value.clamp(0.0, 1.0)
    }

    /// 現在のモード。
    pub fn mode(&self) -> ProgressMode {
        self.mode
    }

    /// 現在の表示キャッシュ値（`[0.0, 1.0]` へ正規化済み、テスト・観測用）。
    pub fn fraction(&self) -> f32 {
        self.cached_fraction
    }

    /// 現在のアニメーション位相（`[0.0, 1.0)`、テスト・観測用）。
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// indeterminate アニメーションの位相を前進させる。
    ///
    /// 壁時計・vsync をこのクレート内で読まない設計方針（ヘッドレステスト可能性の
    /// 維持、モジュール doc 参照）のため、経過秒数はプラットフォーム統合レイヤが
    /// フレーム駆動で計算し `elapsed_secs` として渡す契約とする。
    ///
    /// - `elapsed_secs` は「1 サイクル/秒」の固定速度を前提とした加算量であり、
    ///   `phase` へそのまま加算する（モジュール doc 参照）。可変速度・周期のスケーリング
    ///   はプラットフォーム統合レイヤの責務とし、本メソッドは関与しない。
    /// - `elapsed_secs` が負値・非有限値（NaN/inf）の場合は no-op（安全側、
    ///   OWASP A04 対策）。
    /// - `Determinate` モードでは意味を持たないため位相を進めない（no-op）。
    /// - 位相は `[0.0, 1.0)` の範囲で循環する（`1.0` 到達時に `0.0` へ巻き戻す）。
    pub fn advance(&mut self, elapsed_secs: f32) {
        if self.mode != ProgressMode::Indeterminate {
            return;
        }
        if !elapsed_secs.is_finite() || elapsed_secs < 0.0 {
            return;
        }
        // `elapsed_secs` が大きい場合（複数サイクル分の加算）でも `[0.0, 1.0)` の
        // 循環不変条件を保つため `rem_euclid` で正規化する。
        let advanced = self.phase + elapsed_secs;
        self.phase = advanced.rem_euclid(1.0);
    }

    /// disabled フラグを切り替える（[`InteractionState::set_disabled`] への委譲）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する（[`InteractionState::apply`] への委譲）。
    ///
    /// プログレス表示は非インタラクティブ要素だが、disabled 中の視覚状態解決
    /// （`visual_state`）に必要な `InteractionState` の意味論を独自実装しないため、
    /// `LabelWidget` と同じく委譲経路を公開する。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への
    /// 委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }
}

impl<S> std::fmt::Debug for ProgressWidget<S> {
    /// `LabelWidget` と同じ理由（derive が `S: Debug` の不要な境界を課すのを避ける）で
    /// 手動実装する。関数ポインタの有無のみ記録し、アドレスは出力しない。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressWidget")
            .field("id", &self.id)
            .field("mode", &self.mode)
            .field("cached_fraction", &self.cached_fraction)
            .field("phase", &self.phase)
            .field("interaction", &self.interaction)
            .field("has_value_source", &self.value_source.is_some())
            .finish()
    }
}

impl<S> Clone for ProgressWidget<S> {
    /// `LabelWidget` と同じ理由で手動実装する（derive が `S: Clone` の不要な境界を
    /// 課すのを避ける。関数ポインタは `Copy` であり複製にコストがかからない）。
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            mode: self.mode,
            value_source: self.value_source,
            cached_fraction: self.cached_fraction,
            phase: self.phase,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Widget<S> for ProgressWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        if let Some(read) = self.value_source {
            self.cached_fraction = Self::normalize_fraction(read(state));
        }
    }

    fn measure(&self, available: Measurement) -> Measurement {
        // 暫定計測（`Slider` と同じプレースホルダ定数方式、正式なレイアウト計算は
        // レイアウトプロトコル・S31 統合後の別作業）。
        Measurement {
            width: TRACK_WIDTH.min(available.width),
            height: TRACK_HEIGHT.min(available.height),
        }
    }

    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        // `PaintCommand::Rect` はジオメトリを持たないユニットバリアントのため、
        // 値・位相の差分をコマンド列の差分として検証可能にするのは `Text` 側の役割
        // （モジュール doc 参照）。両モードとも `Text` の内容を対応する内部状態
        // （`cached_fraction` / `phase`）から生成する。
        match self.mode {
            ProgressMode::Determinate => {
                let percent = (self.cached_fraction * 100.0).round() as i32;
                vec![
                    PaintCommand::Rect,
                    PaintCommand::Rect,
                    PaintCommand::Text(format!("{percent}%")),
                ]
            }
            ProgressMode::Indeterminate => {
                let phase_percent = (self.phase * 100.0).round() as i32;
                vec![
                    PaintCommand::Rect,
                    PaintCommand::Rect,
                    PaintCommand::Text(format!("{phase_percent}%")),
                ]
            }
        }
    }

    /// プログレス表示は非インタラクティブ要素であり、常に `false` を返す
    /// （クリック透過、`LabelWidget` と同じ判断）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ProgressIndicator
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`ProgressBar::paint_styled`] と
    /// 同型（Display カテゴリの既定マッピングを用い、構造を保ったまま色・
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

    struct FormState {
        progress: f32,
    }

    fn get_progress(state: &FormState) -> f32 {
        state.progress
    }

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_and_zero_default() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);

        assert_eq!(bar.label_text(), "ダウンロード");
        assert_eq!(bar.fraction(), 0.0);
        assert_eq!(bar.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn spinner_new_constructs_with_id() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        assert_eq!(spinner.id(), WidgetId::from_raw(2));
    }

    // (b) sync_from_state / normalize_fraction

    #[test]
    fn sync_from_state_updates_cached_fraction_from_get() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState { progress: 0.5 };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 0.5);
    }

    #[test]
    fn sync_from_state_clamps_negative_value_to_zero() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState { progress: -0.3 };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 0.0);
    }

    #[test]
    fn sync_from_state_clamps_value_above_one_to_one() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState { progress: 1.5 };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 1.0);
    }

    #[test]
    fn sync_from_state_normalizes_nan_to_zero() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState { progress: f32::NAN };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 0.0);
    }

    #[test]
    fn sync_from_state_normalizes_positive_infinity_to_zero() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState {
            progress: f32::INFINITY,
        };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 0.0);
    }

    #[test]
    fn sync_from_state_normalizes_negative_infinity_to_zero() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "ダウンロード", get_progress);
        let state = FormState {
            progress: f32::NEG_INFINITY,
        };

        bar.sync_from_state(&state);

        assert_eq!(bar.fraction(), 0.0);
    }

    #[test]
    fn spinner_sync_from_state_is_noop() {
        let mut spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));
        let state = FormState { progress: 0.7 };

        // no-op であることの確認（panic せず完了すること自体が検証対象）。
        spinner.sync_from_state(&state);
    }

    // (c) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "OK", get_progress);

        let measurement = bar.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = BAR_WIDTH + BAR_LABEL_GAP + (2.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, BAR_HEIGHT.max(APPROX_LINE_HEIGHT));
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let bar: ProgressBar<FormState> = ProgressBar::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです非常に長いラベルです",
            get_progress,
        );

        let measurement = bar.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "あ", get_progress);

        let measurement = bar.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = BAR_WIDTH + BAR_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
    }

    #[test]
    fn spinner_measure_clamps_to_available_space() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        let measurement = spinner.measure(Measurement {
            width: 5.0,
            height: 1000.0,
        });

        assert_eq!(measurement.width, 5.0);
        assert_eq!(measurement.height, SPINNER_SIZE);
    }

    // (d) paint

    #[test]
    fn paint_at_zero_percent_omits_fill_rect() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);

        let commands = bar.paint(
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
                PaintCommand::Text("0%".to_string()),
                PaintCommand::Text("進捗".to_string()),
            ]
        );
    }

    #[test]
    fn paint_at_fifty_percent_includes_fill_rect_and_percent_text() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let state = FormState { progress: 0.5 };
        bar.sync_from_state(&state);

        let commands = bar.paint(
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
                PaintCommand::Text("50%".to_string()),
                PaintCommand::Text("進捗".to_string()),
            ]
        );
    }

    #[test]
    fn paint_at_hundred_percent_includes_fill_rect_and_percent_text() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let state = FormState { progress: 1.0 };
        bar.sync_from_state(&state);

        let commands = bar.paint(
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
                PaintCommand::Text("100%".to_string()),
                PaintCommand::Text("進捗".to_string()),
            ]
        );
    }

    #[test]
    fn paint_omits_label_text_when_label_empty() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "", get_progress);

        let commands = bar.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("0%".to_string())]
        );
    }

    #[test]
    fn spinner_paint_returns_rect_and_mark() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        let commands = spinner.paint(
            (0.0, 0.0),
            Measurement {
                width: 16.0,
                height: 16.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text(SPINNER_MARK.to_string())
            ]
        );
    }

    // (e) hit_test / accessibility_role / clone_box / Debug

    #[test]
    fn hit_test_always_returns_false_for_progress_bar() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);

        let hit = bar.hit_test(
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
    fn hit_test_always_returns_false_for_spinner() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        let hit = spinner.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 16.0,
                height: 16.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn accessibility_role_is_progress_indicator_for_both_widgets() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        assert_eq!(
            bar.accessibility_role(),
            AccessibilityRole::ProgressIndicator
        );
        assert_eq!(
            spinner.accessibility_role(),
            AccessibilityRole::ProgressIndicator
        );
    }

    #[test]
    fn accessibility_label_is_progress_bar_label() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);

        assert_eq!(bar.accessibility_label(), Some("進捗".to_string()));
    }

    #[test]
    fn accessibility_label_is_none_for_spinner() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        assert_eq!(spinner.accessibility_label(), None);
    }

    #[test]
    fn progress_bar_clone_box_produces_independent_copy() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let state = FormState { progress: 0.8 };
        bar.sync_from_state(&state);

        let mut cloned = bar.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState { progress: 0.1 };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(bar.fraction(), 0.8);
        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            cloned_commands.contains(&PaintCommand::Text("10%".to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn spinner_clone_box_produces_independent_copy() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        let cloned = spinner.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(2));
    }

    #[test]
    fn progress_bar_debug_output_contains_cached_fraction() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let state = FormState { progress: 0.5 };
        bar.sync_from_state(&state);

        let output = format!("{bar:?}");

        assert!(output.contains("cached_fraction"));
        assert!(output.contains("0.5"));
    }

    #[test]
    fn spinner_debug_output_contains_id() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(2));

        let output = format!("{spinner:?}");

        assert!(output.contains("Spinner"));
    }

    // (f) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(1), "進捗", get_progress);
        let state = FormState { progress: 0.5 };

        bar.sync_from_state(&state);
        let measurement = bar.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = bar.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("50%".to_string()),
                PaintCommand::Text("進捗".to_string()),
            ]
        );
    }

    // (g) Screen ビルダー（`Screen::progress_bar` / `Screen::spinner`）配線の検証

    use crate::ui::{Node, Screen};

    #[test]
    fn screen_progress_bar_and_spinner_push_widget_nodes_with_progress_indicator_role() {
        let screen: Screen<FormState> = Screen::new("進捗画面")
            .progress_bar(WidgetId::from_raw(10), "進捗", get_progress)
            .spinner(WidgetId::from_raw(11));

        assert_eq!(screen.len(), 2);

        let mut roles = Vec::new();
        screen.visit(|node| {
            if let Node::Widget(widget) = node {
                roles.push(widget.accessibility_role());
            }
        });

        assert_eq!(
            roles,
            vec![
                AccessibilityRole::ProgressIndicator,
                AccessibilityRole::ProgressIndicator,
            ]
        );
    }

    // (h) ProgressWidget / ProgressMode（S32 の状態・イベント処理）

    struct DownloadState {
        fraction: f32,
    }

    fn read_fraction(state: &DownloadState) -> f32 {
        state.fraction
    }

    #[test]
    fn determinate_sync_from_state_updates_cached_fraction() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(1), read_fraction);
        assert_eq!(progress.fraction(), 0.0);

        progress.sync_from_state(&DownloadState { fraction: 0.5 });
        assert_eq!(progress.fraction(), 0.5);
    }

    #[test]
    fn determinate_sync_from_state_tracks_state_changes() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(2), read_fraction);

        progress.sync_from_state(&DownloadState { fraction: 0.2 });
        assert_eq!(progress.fraction(), 0.2);

        progress.sync_from_state(&DownloadState { fraction: 0.9 });
        assert_eq!(progress.fraction(), 0.9);
    }

    #[test]
    fn out_of_range_values_are_clamped_to_unit_interval() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(3), read_fraction);

        progress.sync_from_state(&DownloadState { fraction: -1.5 });
        assert_eq!(progress.fraction(), 0.0);

        progress.sync_from_state(&DownloadState { fraction: 3.0 });
        assert_eq!(progress.fraction(), 1.0);
    }

    #[test]
    fn non_finite_values_fall_back_to_zero() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(4), read_fraction);

        progress.sync_from_state(&DownloadState { fraction: f32::NAN });
        assert_eq!(progress.fraction(), 0.0);

        progress.sync_from_state(&DownloadState {
            fraction: f32::INFINITY,
        });
        assert_eq!(progress.fraction(), 0.0);

        progress.sync_from_state(&DownloadState {
            fraction: f32::NEG_INFINITY,
        });
        assert_eq!(progress.fraction(), 0.0);
    }

    #[test]
    fn indeterminate_sync_from_state_is_no_op() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(5));

        progress.sync_from_state(&DownloadState { fraction: 0.7 });

        assert_eq!(progress.fraction(), 0.0);
        assert_eq!(progress.mode(), ProgressMode::Indeterminate);
    }

    #[test]
    fn advance_progresses_phase_for_indeterminate_mode() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(6));

        progress.advance(0.25);
        assert_eq!(progress.phase(), 0.25);

        progress.advance(0.5);
        assert_eq!(progress.phase(), 0.75);
    }

    #[test]
    fn advance_wraps_phase_within_unit_interval() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(7));

        progress.advance(0.9);
        progress.advance(0.9);

        // 0.9 + 0.9 = 1.8 -> 0.8 に巻き戻る。
        assert!((progress.phase() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn advance_is_no_op_for_negative_or_non_finite_elapsed() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(8));

        progress.advance(-1.0);
        assert_eq!(progress.phase(), 0.0);

        progress.advance(f32::NAN);
        assert_eq!(progress.phase(), 0.0);

        progress.advance(f32::INFINITY);
        assert_eq!(progress.phase(), 0.0);
    }

    #[test]
    fn advance_is_no_op_for_determinate_mode() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(9), read_fraction);

        progress.advance(0.5);

        assert_eq!(progress.phase(), 0.0);
    }

    #[test]
    fn hit_test_is_always_false_even_inside_bounds() {
        let progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(10));
        let size = Measurement {
            width: 120.0,
            height: 8.0,
        };

        assert!(!progress.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!progress.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    #[test]
    fn set_disabled_updates_interaction_state_and_visual_state() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(11));

        progress.set_disabled(true);

        assert!(progress.interaction_state().disabled);
        assert_eq!(progress.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn disabled_ignores_pointer_and_focus_input() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(12));
        progress.set_disabled(true);

        progress.apply_input(InteractionInput::PointerEntered);
        progress.apply_input(InteractionInput::PointerPressed);
        progress.apply_input(InteractionInput::FocusGained);

        let interaction = progress.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.focused);
        assert!(interaction.disabled);
    }

    #[test]
    fn accessibility_role_is_progress_indicator() {
        let progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(13));

        assert_eq!(
            progress.accessibility_role(),
            AccessibilityRole::ProgressIndicator
        );
    }

    #[test]
    fn determinate_paint_reflects_cached_fraction_diff() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(14), read_fraction);
        let size = Measurement {
            width: 120.0,
            height: 8.0,
        };

        progress.sync_from_state(&DownloadState { fraction: 0.25 });
        let commands_a = progress.paint((0.0, 0.0), size);
        assert_eq!(
            commands_a,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("25%".to_string())
            ]
        );

        progress.sync_from_state(&DownloadState { fraction: 0.75 });
        let commands_b = progress.paint((0.0, 0.0), size);
        assert_eq!(
            commands_b,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("75%".to_string())
            ]
        );
        assert_ne!(commands_a, commands_b);
    }

    #[test]
    fn indeterminate_paint_reflects_phase_diff() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(15));
        let size = Measurement {
            width: 120.0,
            height: 8.0,
        };

        let commands_a = progress.paint((0.0, 0.0), size);
        assert_eq!(
            commands_a,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("0%".to_string())
            ]
        );

        progress.advance(0.5);
        let commands_b = progress.paint((0.0, 0.0), size);
        assert_eq!(
            commands_b,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("50%".to_string())
            ]
        );
        assert_ne!(commands_a, commands_b);
    }

    #[test]
    fn clone_box_preserves_id_mode_and_cached_values() {
        let mut progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_determinate(WidgetId::from_raw(16), read_fraction);
        progress.sync_from_state(&DownloadState { fraction: 0.42 });

        let boxed: Box<dyn Widget<DownloadState>> = Box::new(progress);
        let cloned = boxed.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(16));
        assert_eq!(
            cloned.accessibility_role(),
            AccessibilityRole::ProgressIndicator
        );
    }

    #[test]
    fn debug_output_does_not_panic() {
        let progress: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(17));
        let output = format!("{progress:?}");
        assert!(output.contains("ProgressWidget"));
    }

    #[test]
    fn progress_widget_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<DownloadState>> =
            Box::new(ProgressWidget::new_indeterminate(WidgetId::from_raw(18)));
        assert!(!widget.interaction_state().disabled);
    }

    /// `Widget<S>: Send + Sync` 境界を `ProgressWidget<S>` が満たすことの静的アサーション
    /// （`Label`/`Slider` と同じ回帰防止パターン、widget.rs 参照）。
    #[test]
    fn progress_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProgressWidget<DownloadState>>();
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約（3 型とも共通）。
    #[test]
    fn progress_bar_paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let bar: ProgressBar<FormState> =
            ProgressBar::bound(WidgetId::from_raw(200), "ダウンロード", get_progress);
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert_eq!(
            bar.paint_styled((0.0, 0.0), size, &empty_resolver),
            bar.paint((0.0, 0.0), size)
        );
    }

    #[test]
    fn spinner_paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let spinner: Spinner<FormState> = Spinner::new(WidgetId::from_raw(201));
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 20.0,
            height: 20.0,
        };

        assert_eq!(
            spinner.paint_styled((0.0, 0.0), size, &empty_resolver),
            spinner.paint((0.0, 0.0), size)
        );
    }

    #[test]
    fn progress_widget_paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let widget: ProgressWidget<DownloadState> =
            ProgressWidget::new_indeterminate(WidgetId::from_raw(202));
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 20.0,
            height: 20.0,
        };

        assert_eq!(
            widget.paint_styled((0.0, 0.0), size, &empty_resolver),
            widget.paint((0.0, 0.0), size)
        );
    }
}
