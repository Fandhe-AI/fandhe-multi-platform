//! ラジオボタン（グループ）ウィジェットの状態・イベント処理実装
//! （TASK-8.3-S23 / REQ-8、Issue #118）。
//!
//! `TextFieldWidget`（#109）・`Checkbox`（#115）と同じ判断（既存 main の公開 API のみに
//! 依存する自己完結実装とし、`radio_button::RadioButton` との型統合は別作業に委ねる）を
//! 踏襲し、[`Widget<S>`] を実装する最小のラジオボタングループ型
//! [`RadioGroupWidget`] として実装する（`measure` / `paint` は暫定実装）。
//!
//! **命名の衝突回避**（`.claude/rules/out-of-scope-tracking.md` 隣接の設計判断）:
//! `radio_button::RadioButton<S>` との file-add 衝突を避けるため、本ファイルは
//! `radio_group.rs`、型名は `RadioGroupWidget<S>` / `RadioGroupInput` とする
//! （`TextInput`/`TextFieldWidget` が採った分離と同じ判断）。`radio_button::RadioButton`
//! との型統合は行わない。
//!
//! ラジオボタンはチェックボックスと異なり**グループ内排他選択**が本質のため、
//! チェックボックスのような単一トグル型ではなく、選択肢一覧を丸ごと保持する
//! グループ単位のウィジェットとして実装する。以下を本 issue の設計判断とする。
//!
//! - 排他選択の SSOT はアプリ状態 `S`（選択インデックス、`Option<usize>`）に置く
//!   （設計文書 §3 二層モデル）。ウィジェットは表示キャッシュ（`cached_selected`）
//!   のみを保持し、`&mut S` には触れない。編集バッファ・dirty 管理は不要
//!   （`bool`/`index` に中間編集状態がないため `TextFieldWidget` の stale
//!   blur-commit 問題は構造的に発生しない、`Checkbox` S20 と同じ判断）。
//! - 選択確定（ポインタ・キーボードいずれも）は既存の
//!   [`crate::state::Event::ButtonClicked`]（選択肢ラベル）を返すのみとし、widget 層は
//!   `&mut S` に一切触れない（`TextFieldWidget::commit_if_changed` /
//!   `PointerDispatcher` と同じブリッジ設計。状態書き換えの単一ソースを
//!   `StateStore::dispatch` に保つ）。専用バリアント `Event::RadioSelected` の新設・
//!   `WidgetId` ベースディスパッチ統合は本 issue のスコープ外とする
//!   （PR 本文に記録、`.claude/rules/out-of-scope-tracking.md` 準拠）。
//! - 範囲外インデックス（`options.len()` 以上）は `None` へ安全側でクランプし、
//!   panic させない（`.claude/rules/coding-rust.md` のエラーハンドリング方針）。
//! - 選択肢は縦積みの行（1 行高さの暫定近似）として扱い、[`RadioGroupWidget::option_at`]
//!   で座標から行を解決する。press した選択肢を `pressed_option` で追跡し、
//!   **同一選択肢上での release のみ**確定イベントを発火する（press → 領域外へ
//!   exit → release では発火しない。`InteractionState` の意味論とは独立した
//!   グループ固有の状態として管理する）。
//! - キーボード操作は生キーコード・文字列を扱わず、意味づけ済みの列挙
//!   [`RadioGroupInput`] のみで構成する（`TextFieldInput` / `widget::InteractionInput`
//!   と同じ設計判断で OWASP A03 対策を型レベルで担保する、`.claude/rules/security.md`
//!   準拠）。`FocusNext` / `FocusPrev`（矢印キー相当）はグループ内 roving focus を
//!   ラップアラウンドで移動し **selection-follows-focus**（移動先の選択イベントを
//!   即座に発火、WAI-ARIA ラジオグループの標準的な操作感）とする。`Select`（Space
//!   相当）はフォーカス中選択肢の確定イベントを発火する。`focused`（グループが
//!   フォーカスを保持）かつ非 disabled のときのみ受け付け、空の選択肢リストでは
//!   常に `None` を返す（panic させない）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: reconcile パス
//! （`&mut self` を `Node::Widget` へ届ける経路）は S3 以降も未確立であり、本モジュールの
//! `sync_from_state` は現時点でユニットテストからのみ駆動される。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! `Event::RadioSelected` 専用バリアントの新設・`WidgetId` ベースディスパッチ統合、
//! S22（#117）マージ後の型統合、実描画確認（フォーカスリング等の視覚表現）。

use crate::state::{Event, ReadFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数（`Checkbox` /
/// `TextFieldWidget` と同じ判断）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 選択肢 1 行の高さ近似（論理ピクセル相当）。`option_at` の行解決にも使う。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// マーカー（丸印）自体の一辺の長さ近似（論理ピクセル相当）。
const MARK_SIZE: f32 = 16.0;

/// マーカーとラベルの間隔（論理ピクセル相当）。
const MARK_LABEL_GAP: f32 = 8.0;

/// 選択済みを表す描画テキスト（`PaintCommand::Text` へ渡す簡易マーク）。
///
/// `PaintCommand` を拡張しない方針（3 バックエンドのプレースホルダ契約に波及するため
/// 安全側に倒す、`Checkbox` と同じ判断）のため、選択有無をテキスト内容の差分として
/// 表現する。
const SELECTED_MARK: &str = "\u{25C9}";

/// 未選択を表す描画テキスト。
const UNSELECTED_MARK: &str = "\u{25CB}";

/// 範囲外インデックスを安全側で `None` にクランプする。
///
/// `options.len()` 以上のインデックスは無効値として扱い、panic させない
/// （`.claude/rules/coding-rust.md` のエラーハンドリング方針）。
fn clamp_index(index: Option<usize>, len: usize) -> Option<usize> {
    index.filter(|&i| i < len)
}

/// ラジオボタングループへのキーボード入力を表す列挙（本 issue の核心）。
///
/// 外部由来の生キーコード・文字列を一切扱わず、意味づけ済みの列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する
/// （`TextFieldInput` / `widget::InteractionInput` と同じ設計判断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioGroupInput {
    /// 次の選択肢へ roving focus をラップアラウンドで移動する（矢印キー相当）。
    /// selection-follows-focus のため、移動先の選択イベントも即座に発火する。
    FocusNext,
    /// 前の選択肢へ roving focus をラップアラウンドで移動する（矢印キー相当）。
    /// `FocusNext` と同じく selection-follows-focus を伴う。
    FocusPrev,
    /// フォーカス中の選択肢を確定する（Space 相当）。
    Select,
}

/// ラジオボタングループウィジェット（TASK-8.3-S23 で新設する自己完結の実装）。
///
/// `Debug` / `Clone` は手動実装する（`Checkbox` / `TextFieldWidget` と同じく、状態型 `S`
/// を所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。選択肢ラベル・選択インデックスは機微情報に該当しないため、
/// `TextFieldWidget` のような値マスクは行わず `Debug` にそのまま含める（`Checkbox` と
/// 同じ判断）。
pub struct RadioGroupWidget<S> {
    id: WidgetId,
    /// 選択肢ラベル列。`Event::ButtonClicked { label }` へのブリッジに使う
    /// キーであり、宣言層と共有する（`WidgetId` とは独立し、状態ディスパッチ側の
    /// 識別に用いる、`TextFieldWidget::label` と同じ設計）。
    options: Vec<String>,
    /// アプリ状態 `S` から選択インデックスを読み取る関数ポインタ（`None` の場合は
    /// 静的な初期値のみを保持し、`sync_from_state` は no-op になる。REQ-13 の
    /// 関数ポインタ限定方針に従い、`ReadFn` のみを介して `S` に触れる）。
    get: Option<ReadFn<S, Option<usize>>>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`selected()` が返す値）。
    /// 範囲外インデックスは構築時・同期時の両方で `None` へクランプする。
    cached_selected: Option<usize>,
    /// roving focus の現在位置（グループが `focused` の間のみ意味を持つ）。
    /// `InteractionState::focused` とは直交する、グループ固有の一時的 UI 状態。
    focused_index: Option<usize>,
    /// ポインタ press 中の選択肢インデックス（同一選択肢上での release のみ
    /// 確定イベントを発火するための追跡状態、モジュール doc 参照）。
    pressed_option: Option<usize>,
    /// hover/pressed(全体)/disabled/focus。個別選択肢の press 追跡は `pressed_option`
    /// が別途担う。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for RadioGroupWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadioGroupWidget")
            .field("id", &self.id)
            .field("options", &self.options)
            .field("cached_selected", &self.cached_selected)
            .field("focused_index", &self.focused_index)
            .field("pressed_option", &self.pressed_option)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for RadioGroupWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            options: self.options.clone(),
            get: self.get,
            cached_selected: self.cached_selected,
            focused_index: self.focused_index,
            pressed_option: self.pressed_option,
            interaction: self.interaction,
        }
    }
}

impl<S> RadioGroupWidget<S> {
    /// 静的な選択肢・初期選択のみを持つラジオボタングループを構築する
    /// （`sync_from_state` は no-op）。
    pub fn new_static(
        id: WidgetId,
        options: impl IntoIterator<Item = impl Into<String>>,
        initial_selected: Option<usize>,
    ) -> Self {
        let options: Vec<String> = options.into_iter().map(Into::into).collect();
        let cached_selected = clamp_index(initial_selected, options.len());
        Self {
            id,
            options,
            get: None,
            cached_selected,
            focused_index: None,
            pressed_option: None,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` から動的に選択インデックスを読み取るラジオボタングループを構築する。
    pub fn new_dynamic(
        id: WidgetId,
        options: impl IntoIterator<Item = impl Into<String>>,
        initial_selected: Option<usize>,
        get: ReadFn<S, Option<usize>>,
    ) -> Self {
        let options: Vec<String> = options.into_iter().map(Into::into).collect();
        let cached_selected = clamp_index(initial_selected, options.len());
        Self {
            id,
            options,
            get: Some(get),
            cached_selected,
            focused_index: None,
            pressed_option: None,
            interaction: InteractionState::default(),
        }
    }

    /// 選択肢ラベル列を返す。
    pub fn options(&self) -> &[String] {
        &self.options
    }

    /// `cached_selected`（直近の [`Widget::sync_from_state`] が読み取った選択インデックス）
    /// を返す。
    pub fn selected(&self) -> Option<usize> {
        self.cached_selected
    }

    /// roving focus の現在位置を返す（グループが `focused` でない間も直近の
    /// 位置を保持する。次にフォーカスを得たときの復帰位置として使う）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。`true` にした時点で press 追跡もクリアする
    /// （[`InteractionState::set_disabled`] が hover/pressed/focused を解除するのと
    /// 揃え、無効化されたグループが press 中の見た目のまま残る不整合を防ぐ）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.pressed_option = None;
        }
    }

    /// 座標からヒットした選択肢の行インデックスを解決する（縦積み 1 行高さの暫定近似）。
    ///
    /// disabled 中・矩形外・選択肢数を超える行は `None` を返す（`Checkbox::hit_test` と
    /// 同じ半開区間 `[origin, origin + size)` の意味論）。
    pub fn option_at(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<usize> {
        if self.interaction.disabled {
            return None;
        }
        if point.0 < origin.0 || point.0 >= origin.0 + size.width {
            return None;
        }
        if point.1 < origin.1 || point.1 >= origin.1 + size.height {
            return None;
        }
        let relative_y = point.1 - origin.1;
        let index = (relative_y / APPROX_LINE_HEIGHT) as usize;
        clamp_index(Some(index), self.options.len())
    }

    /// ポインタ押下を記録する（本 issue の核心）。
    ///
    /// `option_at` で解決した行を `pressed_option` として追跡し、この時点ではまだ
    /// 確定イベントを発火しない（確定は [`Self::release`] が同一選択肢上でのみ行う）。
    /// あわせて `InteractionState::apply(PointerPressed)` を適用し、
    /// [`Self::visual_state`] が `Pressed` を返せるようにする（`press`/`release` は
    /// `InteractionState` の press 遷移を自前で管理する契約とし、呼び出し側が
    /// `apply_input(PointerPressed)` を二重に呼ぶ必要はない）。
    pub fn press(&mut self, point: (f32, f32), origin: (f32, f32), size: Measurement) {
        self.pressed_option = self.option_at(point, origin, size);
        self.interaction.apply(InteractionInput::PointerPressed);
    }

    /// ポインタ解放を処理し、press 時と同一選択肢上での release のみ確定イベントを
    /// 発火する（本 issue の核心）。
    ///
    /// press → 領域外へ exit（[`Self::cancel_press`]）→ release という経路では
    /// `pressed_option` が既に `None` のため発火しない。press した選択肢と異なる
    /// 選択肢上で release した場合も発火しない（ドラッグして離す操作の安全側の扱い）。
    /// あわせて `InteractionState::apply(PointerReleased)` を適用する（[`Self::press`]
    /// と対称、`Self::press` の doc 参照）。
    ///
    /// 確定イベントを発火する分岐では `focused_index` も確定した選択肢へ同期する
    /// （roving focus とポインタ確定の位置がずれると、直後の `FocusNext` /
    /// `FocusPrev` / `Select` がポインタ操作前のフォーカス位置から動作してしまう
    /// ため。Bugbot 指摘「Pointer select skips focus update」対応）。
    pub fn release(
        &mut self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<Event> {
        let released_at = self.option_at(point, origin, size);
        let pressed = self.pressed_option.take();
        self.interaction.apply(InteractionInput::PointerReleased);
        match (pressed, released_at) {
            (Some(p), Some(r)) if p == r => {
                self.focused_index = Some(r);
                Some(Event::ButtonClicked {
                    label: self.options[r].clone(),
                })
            }
            _ => None,
        }
    }

    /// press 追跡をキャンセルする（ポインタが領域外へ出た場合の意味論。
    /// `InteractionState::apply` の `PointerExited` が hover/press を解除するのと
    /// 揃え、グループ固有の `pressed_option` も同様にクリアする）。
    pub fn cancel_press(&mut self) {
        self.pressed_option = None;
    }

    /// ポインタ出入り・フォーカス移動を適用する（`InteractionState::apply` への委譲）。
    ///
    /// `PointerExited` は `pressed_option` も連動してクリアする（[`Self::cancel_press`]
    /// 参照）。`FocusGained` で `focused_index` が未設定の場合、現在の選択（なければ
    /// 先頭）を roving focus の初期位置とする（WAI-ARIA ラジオグループの標準的な
    /// フォーカス挙動）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);

        if matches!(input, InteractionInput::PointerExited) {
            self.cancel_press();
        }

        if matches!(input, InteractionInput::FocusGained)
            && self.interaction.focused
            && self.focused_index.is_none()
            && !self.options.is_empty()
        {
            self.focused_index = Some(self.cached_selected.unwrap_or(0));
        }
    }

    /// キーボード操作を処理する（本 issue の核心、WAI-ARIA ラジオグループ準拠の
    /// 最小実装）。
    ///
    /// `focused`（グループがフォーカス保持）かつ非 disabled のときのみ受け付ける。
    /// 空の選択肢リストでは常に `None` を返し panic させない。`FocusNext` /
    /// `FocusPrev` はラップアラウンドで roving focus を移動し
    /// selection-follows-focus（移動先の選択イベントを即座に発火）する。
    pub fn handle_input(&mut self, input: RadioGroupInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused || self.options.is_empty() {
            return None;
        }

        match input {
            RadioGroupInput::FocusNext => {
                let next = match self.focused_index {
                    Some(current) => (current + 1) % self.options.len(),
                    None => 0,
                };
                self.focused_index = Some(next);
                Some(Event::ButtonClicked {
                    label: self.options[next].clone(),
                })
            }
            RadioGroupInput::FocusPrev => {
                let prev = match self.focused_index {
                    Some(current) => (current + self.options.len() - 1) % self.options.len(),
                    None => self.options.len() - 1,
                };
                self.focused_index = Some(prev);
                Some(Event::ButtonClicked {
                    label: self.options[prev].clone(),
                })
            }
            RadioGroupInput::Select => {
                let index = self.focused_index?;
                Some(Event::ButtonClicked {
                    label: self.options[index].clone(),
                })
            }
        }
    }
}

impl<S: 'static> Widget<S> for RadioGroupWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から選択インデックスを読み取り `cached_selected` を更新する
    /// （reconcile フェーズ）。範囲外インデックスは `None` へ安全側でクランプする
    /// （モジュール doc 参照）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        let Some(get) = self.get else {
            return;
        };
        let raw = get(state);
        self.cached_selected = clamp_index(raw, self.options.len());
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ
    /// 実装、折り返し・複数行はスコープ外、`Checkbox::measure` と同じ判断）。
    ///
    /// 幅 = マーカー幅 + マーカー・ラベル間ギャップ + 最長ラベル幅 を `available.width`
    /// でクランプする。高さ = 選択肢数 × 行高 を `available.height` でクランプする。
    /// マルチバイトは `chars().count()` 基準の近似（`Checkbox` / `TextFieldWidget` と
    /// 同じ既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let max_label_width = self
            .options
            .iter()
            .map(|label| label.chars().count() as f32 * APPROX_CHAR_WIDTH)
            .fold(0.0_f32, f32::max);
        let width = (MARK_SIZE + MARK_LABEL_GAP + max_label_width).min(available.width);
        let height = ((self.options.len() as f32) * APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// 選択肢ごとに `[Rect（マーカー枠）, Text(選択マーク), Text(label)]` の
    /// 描画プリミティブ列を並べて返す。
    ///
    /// ヘッドレス環境でも選択状態がコマンド列の差分として検証できる構成とし、
    /// `PaintCommand` 自体は拡張しない（`Checkbox` と同じ判断、モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = Vec::with_capacity(self.options.len() * 3);
        for (index, label) in self.options.iter().enumerate() {
            commands.push(PaintCommand::Rect);
            let mark = if self.cached_selected == Some(index) {
                SELECTED_MARK
            } else {
                UNSELECTED_MARK
            };
            commands.push(PaintCommand::Text(mark.to_string()));
            commands.push(PaintCommand::Text(label.clone()));
        }
        commands
    }

    /// グループ全体の矩形包含判定（AABB）。`interaction.disabled` の間は `false` を
    /// 返し、無効化されたグループを入力対象にしない（`Checkbox::hit_test` と同じ
    /// 設計判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、左上を含み
    /// 右下を含まない半開区間（`[origin, origin + size)`）で判定する。個別選択肢の
    /// 行解決は [`Self::option_at`] が別途担う。
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
        AccessibilityRole::RadioGroup
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateStore;
    use crate::ui::Screen;

    struct FormState {
        plan: Option<usize>,
    }

    fn read_plan(state: &FormState) -> Option<usize> {
        state.plan
    }

    fn full_size() -> Measurement {
        Measurement {
            width: 200.0,
            height: 60.0,
        }
    }

    fn focus(group: &mut RadioGroupWidget<FormState>) {
        group.apply_input(InteractionInput::FocusGained);
    }

    // (a) 構築・アクセサ

    #[test]
    fn new_static_constructs_with_options_and_initial_selection() {
        let group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            Some(1),
        );

        assert_eq!(group.options(), &["free", "standard", "premium"]);
        assert_eq!(group.selected(), Some(1));
        assert_eq!(group.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_static_clamps_out_of_range_initial_selection_to_none() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], Some(9));

        assert_eq!(group.selected(), None);
    }

    #[test]
    fn new_dynamic_constructs_with_clamped_initial_selection() {
        let group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard"],
            None,
            read_plan,
        );

        assert_eq!(group.selected(), None);
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_static_group_is_no_op() {
        let mut group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], Some(0));
        let state = FormState { plan: Some(1) };

        group.sync_from_state(&state);

        assert_eq!(group.selected(), Some(0));
    }

    #[test]
    fn sync_from_state_dynamic_group_tracks_state_changes() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
            read_plan,
        );
        let mut state = FormState { plan: Some(2) };

        group.sync_from_state(&state);
        assert_eq!(group.selected(), Some(2));

        state.plan = None;
        group.sync_from_state(&state);
        assert_eq!(group.selected(), None);
    }

    #[test]
    fn sync_from_state_clamps_out_of_range_index_to_none() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard"],
            None,
            read_plan,
        );
        let state = FormState { plan: Some(42) };

        group.sync_from_state(&state);

        assert_eq!(group.selected(), None);
    }

    // (c) ポインタ確定

    #[test]
    fn press_then_release_on_same_option_fires_event() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        group.press((5.0, 25.0), (0.0, 0.0), size);
        let event = group.release((5.0, 25.0), (0.0, 0.0), size);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "standard".to_string(),
            })
        );
    }

    #[test]
    fn release_on_different_option_does_not_fire() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        group.press((5.0, 5.0), (0.0, 0.0), size);
        let event = group.release((5.0, 45.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn drag_away_then_release_does_not_fire() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        group.press((5.0, 5.0), (0.0, 0.0), size);
        group.apply_input(InteractionInput::PointerExited);
        let event = group.release((5.0, 5.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn press_then_release_on_same_option_updates_focused_index() {
        // Bugbot 指摘「Pointer select skips focus update」の回帰テスト。
        // ポインタで選択肢を確定した後、roving focus がその選択肢へ追従していないと
        // 直後の FocusNext/FocusPrev/Select がポインタ操作前の位置から動作してしまう。
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        group.press((5.0, 45.0), (0.0, 0.0), size);
        let event = group.release((5.0, 45.0), (0.0, 0.0), size);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "premium".to_string(),
            })
        );
        assert_eq!(group.focused_index(), Some(2));
    }

    #[test]
    fn press_sets_pressed_visual_state_and_release_clears_it() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        group.press((5.0, 5.0), (0.0, 0.0), size);
        assert_eq!(group.visual_state(), VisualState::Pressed);

        group.release((5.0, 5.0), (0.0, 0.0), size);
        assert_eq!(group.visual_state(), VisualState::Idle);
    }

    #[test]
    fn disabled_group_ignores_press_and_release() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        group.set_disabled(true);
        let size = full_size();

        group.press((5.0, 5.0), (0.0, 0.0), size);
        let event = group.release((5.0, 5.0), (0.0, 0.0), size);

        assert_eq!(event, None);
    }

    #[test]
    fn option_at_resolves_row_by_vertical_position() {
        let group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        let size = full_size();

        assert_eq!(group.option_at((5.0, 5.0), (0.0, 0.0), size), Some(0));
        assert_eq!(group.option_at((5.0, 25.0), (0.0, 0.0), size), Some(1));
        assert_eq!(group.option_at((5.0, 45.0), (0.0, 0.0), size), Some(2));
        assert_eq!(group.option_at((5.0, 100.0), (0.0, 0.0), size), None);
    }

    // (d) キーボード

    #[test]
    fn focus_next_wraps_around_and_selects() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        focus(&mut group);
        group.handle_input(RadioGroupInput::FocusNext);
        group.handle_input(RadioGroupInput::FocusNext);

        let event = group.handle_input(RadioGroupInput::FocusNext);

        assert_eq!(group.focused_index(), Some(0));
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "free".to_string(),
            })
        );
    }

    #[test]
    fn focus_prev_wraps_around_and_selects() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        focus(&mut group);

        let event = group.handle_input(RadioGroupInput::FocusPrev);

        assert_eq!(group.focused_index(), Some(2));
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "premium".to_string(),
            })
        );
    }

    #[test]
    fn select_fires_event_for_focused_option() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        focus(&mut group);
        // focus() は未選択（cached_selected == None）のため roving focus を index 0
        // （"free"）から開始する。FocusNext で index 1（"standard"）へ移動する。
        group.handle_input(RadioGroupInput::FocusNext);

        let event = group.handle_input(RadioGroupInput::Select);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "standard".to_string(),
            })
        );
    }

    #[test]
    fn keyboard_input_ignored_when_not_focused() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );

        let event = group.handle_input(RadioGroupInput::FocusNext);

        assert_eq!(event, None);
        assert_eq!(group.focused_index(), None);
    }

    #[test]
    fn keyboard_input_ignored_when_disabled() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
        );
        focus(&mut group);
        group.set_disabled(true);

        let event = group.handle_input(RadioGroupInput::Select);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_input_on_empty_options_is_safe_none() {
        let mut group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), Vec::<String>::new(), None);
        focus(&mut group);

        assert_eq!(group.handle_input(RadioGroupInput::FocusNext), None);
        assert_eq!(group.handle_input(RadioGroupInput::FocusPrev), None);
        assert_eq!(group.handle_input(RadioGroupInput::Select), None);
    }

    #[test]
    fn focus_gained_defaults_roving_focus_to_current_selection() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            Some(2),
        );

        focus(&mut group);

        assert_eq!(group.focused_index(), Some(2));
    }

    // (e) 排他性の StateStore 結合往復

    #[test]
    fn state_store_dispatch_round_trip_enforces_exclusive_selection() {
        fn select_free(state: &mut FormState) {
            state.plan = Some(0);
        }
        fn select_standard(state: &mut FormState) {
            state.plan = Some(1);
        }
        fn select_premium(state: &mut FormState) {
            state.plan = Some(2);
        }

        let screen: Screen<FormState> = Screen::new("form")
            .button(WidgetId::from_raw(101), "free", select_free)
            .button(WidgetId::from_raw(102), "standard", select_standard)
            .button(WidgetId::from_raw(103), "premium", select_premium);
        let mut store = StateStore::new(FormState { plan: Some(0) });
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard", "premium"],
            None,
            read_plan,
        );
        group.sync_from_state(store.state());
        assert_eq!(group.selected(), Some(0));

        let size = full_size();
        group.press((5.0, 25.0), (0.0, 0.0), size);
        let event = group
            .release((5.0, 25.0), (0.0, 0.0), size)
            .expect("standard 行の press/release は Some を返す");
        let handled = store.dispatch(&screen, &event);
        assert!(handled);

        group.sync_from_state(store.state());

        assert_eq!(store.state().plan, Some(1));
        assert_eq!(group.selected(), Some(1));
    }

    // (f) hit_test / Debug / clone_box / measure

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], None);

        assert!(group.hit_test((5.0, 5.0), (0.0, 0.0), full_size()));
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], None);
        let size = full_size();

        assert!(!group.hit_test((size.width, size.height), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled() {
        let mut group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], None);
        group.set_disabled(true);

        assert!(!group.hit_test((5.0, 5.0), (0.0, 0.0), full_size()));
    }

    #[test]
    fn accessibility_role_is_radio_group() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["free", "standard"], None);

        assert_eq!(group.accessibility_role(), AccessibilityRole::RadioGroup);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard"],
            Some(0),
            read_plan,
        );
        let state = FormState { plan: Some(1) };
        group.sync_from_state(&state);

        let mut cloned = group.clone_box();
        let other_state = FormState { plan: Some(0) };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(group.selected(), Some(1));
        let cloned_commands = cloned.paint((0.0, 0.0), full_size());
        assert!(cloned_commands.contains(&PaintCommand::Text(UNSELECTED_MARK.to_string())));
    }

    #[test]
    fn debug_output_contains_cached_selected_and_options() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard"],
            None,
            read_plan,
        );
        let state = FormState { plan: Some(1) };
        group.sync_from_state(&state);

        let output = format!("{group:?}");

        assert!(output.contains("cached_selected"));
        assert!(output.contains("Some(1)"));
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(1), ["あ"], None);

        let measurement = group.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = MARK_SIZE + MARK_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, APPROX_LINE_HEIGHT);
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let group: RadioGroupWidget<FormState> = RadioGroupWidget::new_static(
            WidgetId::from_raw(1),
            ["非常に長いラベルです非常に長いラベルです", "短い"],
            None,
        );

        let measurement = group.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn paint_marks_selected_option_and_lists_all_labels() {
        let mut group: RadioGroupWidget<FormState> = RadioGroupWidget::new_dynamic(
            WidgetId::from_raw(1),
            ["free", "standard"],
            None,
            read_plan,
        );
        let state = FormState { plan: Some(1) };
        group.sync_from_state(&state);

        let commands = group.paint((0.0, 0.0), full_size());

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text(UNSELECTED_MARK.to_string()),
                PaintCommand::Text("free".to_string()),
                PaintCommand::Rect,
                PaintCommand::Text(SELECTED_MARK.to_string()),
                PaintCommand::Text("standard".to_string()),
            ]
        );
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let group: RadioGroupWidget<FormState> =
            RadioGroupWidget::new_static(WidgetId::from_raw(200), ["free", "standard"], Some(0));
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert_eq!(
            group.paint_styled((0.0, 0.0), size, &empty_resolver),
            group.paint((0.0, 0.0), size)
        );
    }
}
