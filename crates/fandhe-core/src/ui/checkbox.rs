//! チェックボックスウィジェット `Checkbox<S>`（TASK-8.3-S19 / REQ-8、Issue #114）と、
//! その状態・イベント処理を担う `CheckboxWidget<S>`（TASK-8.3-S20 / REQ-8、Issue #115）。
//!
//! `Checkbox<S>` は `Label`（#105）・`TextInput`（#108）が確立したパターンを踏襲し、
//! 共通トレイト [`super::widget::Widget`] を実装する具体ウィジェット型である。
//!
//! **`Checkbox` / `CheckboxWidget` の型分離について**: `CheckboxWidget` は
//! `text_field::TextFieldWidget` と同じ判断で `Widget<S>` を実装する自己完結の最小型
//! として独立に実装されている。`Checkbox` と `CheckboxWidget` は名前が異なり実装も
//! 独立しているため、統合は行わず両立させている（`label::Label` / `label::LabelWidget`、
//! `text_input::TextInput` / `text_field::TextFieldWidget` と同じ判断。統合作業自体は
//! 別 issue のスコープ）。アクセシビリティロールは `AccessibilityRole::CheckBox` の
//! 表記へ揃え、`CheckboxWidget` 側の独自バリアント案（`Checkbox`）は採用しない。
//!
//! `CheckboxWidget` はテキストフィールドと異なり **制御型（controlled）** の
//! bool 状態のみを扱う。以下を [`CheckboxWidget`] の設計判断とする。
//!
//! - チェック状態の SSOT はアプリ状態 `S`。ウィジェットは [`crate::state::ReadFn`] 経由の
//!   [`CheckboxWidget::sync_from_state`] で表示用キャッシュ `checked: bool` を更新するのみで、
//!   テキストフィールドのような編集バッファ・dirty 管理は不要（bool に中間編集状態が
//!   ないため、`text_field::TextFieldWidget` の stale blur-commit 問題は構造的に
//!   発生しない）。
//! - トグル確定（クリック・キーボード活性化）は **既存の [`crate::state::Event::ButtonClicked`]
//!   を返す**（[`super::pointer::PointerDispatcher`] がクリック確定に用いる現行の
//!   唯一のブリッジと同一）。宣言層はレガシー `.button(label, toggle_handler)`
//!   （`fn(&mut S)` が bool を反転させる）で受け、`StateStore::dispatch` を経由して
//!   状態へ反映後、`sync_from_state` で新値が還流する。
//!   専用バリアント `Event::CheckboxToggled` の新設は、書き込み先となる宣言層ノード
//!   （`state::WriteFn<S, bool>` 保持）が存在せず `Node<S>` enum への variant 追加が
//!   設計文書 §4.1 の温存方針に反するため、**スコープ外**とする
//!   （`WidgetId` ベースディスパッチ確立後の別作業、`.claude/rules/out-of-scope-tracking.md`
//!   準拠。PR 本文に記録する）。
//! - ポインタ確定は `PointerReleased` を [`InteractionState::apply`] へ適用する**前**に
//!   `pressed && hovered` だったかで判定する（press→領域外へ exit→release では発火しない。
//!   `InteractionState` の「`PointerExited` は pressed も解除する」という既存の意味論を
//!   そのまま利用し、独自のドラッグアウト判定を再実装しない）。
//! - キーボード確定（[`CheckboxInput::Toggle`]、Space/Enter 相当）は `focused` かつ
//!   非 `disabled` のときのみ発火する。
//! - disabled 中は全入力を無視する（`InteractionState` の意味論をそのまま継承し、
//!   独自の入力遮断ロジックを再実装しない）。編集バッファを持たないため、
//!   `text_field::TextFieldWidget::set_disabled` のような disable 前 commit は不要で、
//!   `InteractionState::set_disabled` へそのまま委譲する。
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
//! (a) `Event::CheckboxToggled` 専用バリアント + `WidgetId` ベースディスパッチ統合、
//! (b) `Checkbox` / `CheckboxWidget` の型統合、(c) 三状態（indeterminate）対応、
//! (d) reconcile パス（`&mut` を `Node::Widget` へ届ける経路）未配線の既知契約。
//!
//! (e) `WidgetCatalog` への登録・テスト網羅拡充は TASK-8.3-S21（Issue #116）で完了した。
//! `checkbox` エントリを `catalog.rs` の `Input` カテゴリへ登録し（`Screen::checkbox`
//! 経由で `Checkbox<S>` を配置）、クレート外部からの結合テスト
//! （`tests/checkbox_widget.rs`）を新設した。

use crate::state::{Event, ReadFn, WidgetValue, WriteFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数であり、
/// `measure` の折り返し・複数行はスコープ外（`TextInput` と同じ設計判断）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// チェックボックスの矩形（ボックス）自体の一辺の長さ（論理ピクセル相当）。
const BOX_SIZE: f32 = 16.0;

/// ボックスとラベルの間隔（論理ピクセル相当）。
const BOX_LABEL_GAP: f32 = 8.0;

/// チェック済みを表す描画テキスト（`PaintCommand::Text` へ渡す簡易マーク）。
///
/// `PaintCommand` を拡張しない方針（3 バックエンドのプレースホルダ契約に波及するため
/// 安全側に倒す）のため、チェック有無をテキスト内容の差分として表現する。
const CHECK_MARK: &str = "\u{2713}";

/// チェックボックスウィジェット（TASK-8.3-S19、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Screen<S>` / `Node<S>` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。チェック有無（`bool`）・ラベルは秘密情報に該当しないため、
/// `TextInput` のような値マスクは行わず `Debug` にそのまま含める。
pub struct Checkbox<S> {
    id: WidgetId,
    label: String,
    /// アプリ状態 `S` からチェック有無を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    get: ReadFn<S, bool>,
    /// トグル結果を状態 `S` へ反映する関数ポインタ。実際のイベント配線は
    /// [`CheckboxWidget`]（S20）が別型として提供する。
    set: WriteFn<S, bool>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`is_checked()` が返す値）。
    cached_checked: bool,
    /// hover/pressed/disabled/focus。実際のトグル配線は [`CheckboxWidget`] のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Checkbox<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkbox")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("cached_checked", &self.cached_checked)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Checkbox<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            get: self.get,
            set: self.set,
            cached_checked: self.cached_checked,
            interaction: self.interaction,
        }
    }
}

impl<S> Checkbox<S> {
    /// 状態 `S` に束縛したチェックボックスを作る（ビルダー形式の起点）。
    ///
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`] / [`WriteFn`]）
    /// と代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。`set` をコンストラクタに含める理由は、後続工程で
    /// ビルダーシグネチャを破壊変更しないためであり、トグル自体の書き込み・イベント
    /// ディスパッチは [`CheckboxWidget`] のスコープとする（`TextInput::bound` と同じ判断）。
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
            cached_checked: false,
            interaction: InteractionState::default(),
        }
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// `cached_checked`（直近の [`Widget::sync_from_state`] が読み取ったチェック有無）を返す。
    pub fn is_checked(&self) -> bool {
        self.cached_checked
    }

    /// イベント配線が使う書き込み経路を返す。
    ///
    /// この関数ポインタを呼び出す経路（トグルディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる
    /// （`TextInput::write_fn` と同型）。
    pub fn write_fn(&self) -> WriteFn<S, bool> {
        self.set
    }
}

impl<S: 'static> Widget<S> for Checkbox<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からチェック有無を読み取り `cached_checked` を更新する
    /// （reconcile フェーズ）。未配線の契約はモジュール doc を参照。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_checked = (self.get)(state);
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = ボックスサイズ + ボックス・ラベル間ギャップ + ラベル幅を `available.width` で
    /// クランプする。高さ = ボックスサイズと行高の大きい方を `available.height` で
    /// クランプする。マルチバイトは `chars().count()` 基準の近似（`TextInput` と同じ
    /// 既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (BOX_SIZE + BOX_LABEL_GAP + label_width).min(available.width);
        let height = BOX_SIZE.max(APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// `[Rect（ボックス）, （checked 時のみ）Text(チェックマーク), Text(label)]` の
    /// 描画プリミティブ列を返す。
    ///
    /// ヘッドレス環境でもチェック有無がコマンド列の差分として検証できる構成とし、
    /// `PaintCommand` 自体は拡張しない（モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];

        if self.cached_checked {
            commands.push(PaintCommand::Text(CHECK_MARK.to_string()));
        }
        commands.push(PaintCommand::Text(self.label.clone()));

        commands
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたチェックボックスを入力対象にしない（安全側の判断）。
    ///
    /// 表示専用の `Label`（常に `false`）と異なり、チェックボックスはトグル対象になるため
    /// 包含判定を実際に返す（`TextInput` と同じ設計判断）。
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
        AccessibilityRole::CheckBox
    }

    /// チェックボックスのラベル文言をアクセシブルネームとして公開する
    /// （TASK-9.1-S2、REQ-9）。
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
    /// （`style_tokens::input_style_refs`）を用い、`paint()` と同じ構造（矩形・
    /// テキストの個数/順序）を保ったまま色・タイポグラフィのみを解決済み具体値へ
    /// 差し替える。解決失敗時は `paint()` の無スタイル列へフォールバックする。
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
    /// `Number` / `Index` 等の型不一致は `false` を返し、状態を変更しない。
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

/// チェックボックスへのキーボード入力を表す列挙（[`CheckboxWidget`] の核心）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`widget::InteractionInput` /
/// `text_field::TextFieldInput` と同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxInput {
    /// チェック状態のトグルを確定する（Space/Enter 相当）。
    Toggle,
}

/// チェックボックスウィジェット（TASK-8.3-S20 で新設する自己完結の最小実装）。
///
/// `measure` / `paint` は [`Checkbox`]（S19）の基本描画実装がスコープであるため、
/// 本型では状態・イベント処理を単体で検証可能にするための暫定実装
/// （[`super::label::LabelWidget`] と同様、文字数に応じた素朴な計測・`PaintCommand` への
/// 変換のみ）を提供する。
pub struct CheckboxWidget<S> {
    id: WidgetId,
    /// `Event::ButtonClicked` へのブリッジに使うラベル（宣言層の `.button(label, ...)` と
    /// 共有するキー。`WidgetId` とは独立し、状態ディスパッチ側の識別に用いる）。
    label: String,
    /// 状態 `S` からチェック状態を読み取るための関数ポインタ（`None` の場合は静的な
    /// 初期値のみを保持し、`sync_from_state` は no-op になる。REQ-13 の関数ポインタ
    /// 限定方針に従い、`ReadFn` のみを介して `S` に触れる）。
    checked_source: Option<ReadFn<S, bool>>,
    /// 直近の `sync_from_state`（または構築時の初期値）で確定したチェック状態。
    checked: bool,
    interaction: InteractionState,
}

impl<S> CheckboxWidget<S> {
    /// 静的な初期値のみを持つチェックボックスを構築する（`sync_from_state` は no-op）。
    pub fn new_static(id: WidgetId, label: impl Into<String>, initial_checked: bool) -> Self {
        Self {
            id,
            label: label.into(),
            checked_source: None,
            checked: initial_checked,
            interaction: InteractionState::default(),
        }
    }

    /// 状態 `S` から動的にチェック状態を読み取るチェックボックスを構築する。
    ///
    /// `initial_checked` は最初の `sync_from_state` 呼び出し前に `checked()` / `paint()` が
    /// 参照する初期表示値（未同期状態でも `false` を返さないための保険。
    /// `super::label::LabelWidget::new_dynamic` と同じ判断）。
    pub fn new_dynamic(
        id: WidgetId,
        label: impl Into<String>,
        initial_checked: bool,
        checked_source: ReadFn<S, bool>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            checked_source: Some(checked_source),
            checked: initial_checked,
            interaction: InteractionState::default(),
        }
    }

    /// `Event::ButtonClicked` の発火対象を特定するラベル。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 現在の表示用チェック状態。
    pub fn checked(&self) -> bool {
        self.checked
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// bool には中間編集状態がなく確定すべき編集バッファも存在しないため、
    /// `text_field::TextFieldWidget::set_disabled` のような blur-commit は不要で、
    /// `InteractionState::set_disabled` へそのまま委譲するのみで済む（モジュール doc 参照）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// ポインタ・フォーカス入力を適用する。
    ///
    /// クリック確定の判定は [`InteractionState::apply`] へ入力を適用する**前**の
    /// `pressed && hovered` を見て行う。`PointerReleased` 適用前に判定することで、
    /// press→領域外へ exit（`pressed` が解除される）→release という経路では
    /// 発火しない（press-and-drag-away の意味論、`text_field` の blur-commit 判定と
    /// 同じく `InteractionState` の既存意味論をそのまま利用する設計）。disabled 中は
    /// `InteractionState::apply` が入力そのものを無視するため、`pressed` が立たず
    /// クリック確定も発火しない。
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

    /// キーボード入力を処理する（本型の核心）。
    ///
    /// disabled 中・非フォーカス中はすべての入力を無視する（`text_field` と同じ
    /// 「フォーカス保持中のみ操作可能」という設計判断）。
    pub fn handle_input(&mut self, input: CheckboxInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        match input {
            CheckboxInput::Toggle => Some(Event::ButtonClicked {
                label: self.label.clone(),
            }),
        }
    }
}

impl<S> std::fmt::Debug for CheckboxWidget<S> {
    /// `label.rs` の `LabelWidget` と同じ理由（derive が `S: Debug` の不要な境界を
    /// 課すのを避けるため）で手動実装する。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckboxWidget")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("checked", &self.checked)
            .field("interaction", &self.interaction)
            .field("has_checked_source", &self.checked_source.is_some())
            .finish()
    }
}

impl<S> Clone for CheckboxWidget<S> {
    /// `label.rs` の `LabelWidget` と同じ理由で手動実装する（derive が `S: Clone` の
    /// 不要な境界を課すのを避ける。関数ポインタは `Copy` であり複製にコストがかからない）。
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            checked_source: self.checked_source,
            checked: self.checked,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Widget<S> for CheckboxWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        let Some(read) = self.checked_source else {
            return;
        };
        self.checked = read(state);
    }

    fn measure(&self, available: Measurement) -> Measurement {
        // 暫定計測（`Checkbox` が正式なチェックボックス寸法へ置き換える想定、
        // `LabelWidget` を踏襲）。「[x] ラベル」/「[ ] ラベル」の文字数近似
        // （先頭 4 文字の状態マーカー分を加算）。
        let approx_width = ((self.label.chars().count() + 4) as f32) * 8.0;
        Measurement {
            width: approx_width.min(available.width),
            height: available.height.min(16.0),
        }
    }

    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        // 暫定描画。チェック状態をテキストマーカーで表現する。
        let marker = if self.checked { "[x]" } else { "[ ]" };
        vec![
            PaintCommand::Rect,
            PaintCommand::Text(format!("{marker} {}", self.label)),
        ]
    }

    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        // 設計判断: チェックボックスはラベルと異なりインタラクティブ要素であり、
        // 矩形内判定を行う（フォーカス取得・クリック確定のためのポインタターゲットとして
        // 機能する）。右端・下端は半開区間 `[origin, origin + size)` として扱う
        // （`Rect::contains` / `hit_test::hit_test` が採用する共通のヒットジオメトリ規約に
        // 揃え、隣接するウィジェットが共有エッジ上の点を両方とも自分のものと
        // 判定してしまう不整合を防ぐ、`text_field::TextFieldWidget::hit_test` と同じ設計）。
        //
        // `interaction.disabled` の間は `false` を返し、無効化されたチェックボックスを
        // ポインタ当たり判定の対象から外す（`text_field::TextFieldWidget::hit_test` と
        // 同じ設計。無効化中でも下に重なる他ウィジェットへのヒットを奪わないため）。
        if self.interaction.disabled {
            return false;
        }
        point.0 >= origin.0
            && point.0 < origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::CheckBox
    }

    /// チェックボックスのラベル文言をアクセシブルネームとして公開する
    /// （TASK-9.1-S2、REQ-9）。
    fn accessibility_label(&self) -> Option<String> {
        Some(self.label.clone())
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。[`Checkbox::paint_styled`] と同型
    /// （Input カテゴリの既定マッピングを用い、構造を保ったまま色・タイポグラフィのみ
    /// 差し替える）。
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

    struct FormState {
        agreed: bool,
    }

    fn get_agreed(state: &FormState) -> bool {
        state.agreed
    }

    fn set_agreed(state: &mut FormState, value: bool) {
        state.agreed = value;
    }

    // --- Checkbox（S19: API 設計・基本描画）---

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_and_unchecked_default() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        assert_eq!(checkbox.label_text(), "同意する");
        assert!(!checkbox.is_checked());
        assert_eq!(checkbox.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let write_fn = checkbox.write_fn();
        let mut state = FormState { agreed: false };
        write_fn(&mut state, true);
        assert!(state.agreed);
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_updates_cached_checked_from_get() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: true };

        checkbox.sync_from_state(&state);

        assert!(checkbox.is_checked());
    }

    #[test]
    fn sync_from_state_follows_state_changes_across_calls() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let mut state = FormState { agreed: true };

        checkbox.sync_from_state(&state);
        assert!(checkbox.is_checked());

        state.agreed = false;
        checkbox.sync_from_state(&state);
        assert!(!checkbox.is_checked());
    }

    // (c) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "メモ", get_agreed, set_agreed);

        let measurement = checkbox.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // ボックス + ギャップ + ラベル(2 文字) 分の近似幅。
        let expected_width = BOX_SIZE + BOX_LABEL_GAP + (2.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, BOX_SIZE.max(APPROX_LINE_HEIGHT));
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let checkbox: Checkbox<FormState> = Checkbox::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです非常に長いラベルです",
            get_agreed,
            set_agreed,
        );

        let measurement = checkbox.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "あ", get_agreed, set_agreed);

        let measurement = checkbox.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // "あ" は 1 文字（3 バイト）。バイト数基準なら幅がずれてしまう回帰の防止。
        let expected_width = BOX_SIZE + BOX_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
    }

    // (d) paint

    #[test]
    fn paint_returns_rect_check_mark_and_label_when_checked() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: true };
        checkbox.sync_from_state(&state);

        let commands = checkbox.paint(
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
                PaintCommand::Text(CHECK_MARK.to_string()),
                PaintCommand::Text("同意する".to_string()),
            ]
        );
    }

    #[test]
    fn paint_omits_check_mark_when_unchecked() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let commands = checkbox.paint(
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
                PaintCommand::Text("同意する".to_string()),
            ]
        );
    }

    // (e) hit_test / accessibility_role / clone_box / Debug

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let hit = checkbox.hit_test(
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
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let hit = checkbox.hit_test(
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
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let hit = checkbox.hit_test(
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
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        let hit = checkbox.hit_test(
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
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        checkbox.interaction.set_disabled(true);

        let hit = checkbox.hit_test(
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
    fn accessibility_role_is_check_box() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        assert_eq!(checkbox.accessibility_role(), AccessibilityRole::CheckBox);
    }

    #[test]
    fn accessibility_label_is_checkbox_label() {
        let checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);

        assert_eq!(checkbox.accessibility_label(), Some("同意する".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: true };
        checkbox.sync_from_state(&state);

        let mut cloned = checkbox.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState { agreed: false };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert!(checkbox.is_checked());
        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            !cloned_commands.contains(&PaintCommand::Text(CHECK_MARK.to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn debug_output_contains_cached_checked() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: true };
        checkbox.sync_from_state(&state);

        let output = format!("{checkbox:?}");

        assert!(output.contains("cached_checked"));
        assert!(output.contains("true"));
    }

    // (f) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut checkbox: Checkbox<FormState> =
            Checkbox::bound(WidgetId::from_raw(1), "同意する", get_agreed, set_agreed);
        let state = FormState { agreed: true };

        checkbox.sync_from_state(&state);
        let measurement = checkbox.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = checkbox.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text(CHECK_MARK.to_string()),
                PaintCommand::Text("同意する".to_string()),
            ]
        );
    }

    // --- CheckboxWidget（S20: 状態・イベント処理）---

    use crate::state::StateStore;
    use crate::ui::Screen;

    fn read_agreed(state: &FormState) -> bool {
        state.agreed
    }

    fn toggle_agreed(state: &mut FormState) {
        state.agreed = !state.agreed;
    }

    fn focus(checkbox: &mut CheckboxWidget<FormState>) {
        checkbox.apply_input(InteractionInput::FocusGained);
    }

    #[test]
    fn new_static_holds_initial_checked_value() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(101), "agree", true);

        assert!(checkbox.checked());
        assert_eq!(checkbox.label(), "agree");
    }

    #[test]
    fn new_dynamic_holds_initial_checked_value_before_sync() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_dynamic(WidgetId::from_raw(102), "agree", false, read_agreed);

        assert!(!checkbox.checked());
    }

    #[test]
    fn sync_from_state_static_checkbox_widget_is_no_op() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(103), "agree", true);
        let state = FormState { agreed: false };

        checkbox.sync_from_state(&state);

        assert!(checkbox.checked());
    }

    #[test]
    fn sync_from_state_dynamic_checkbox_widget_tracks_state() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_dynamic(WidgetId::from_raw(104), "agree", false, read_agreed);
        let state = FormState { agreed: true };

        checkbox.sync_from_state(&state);

        assert!(checkbox.checked());
    }

    #[test]
    fn pointer_press_and_release_inside_confirms_toggle() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(105), "agree", false);

        checkbox.apply_input(InteractionInput::PointerEntered);
        checkbox.apply_input(InteractionInput::PointerPressed);
        let event = checkbox.apply_input(InteractionInput::PointerReleased);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "agree".to_string(),
            })
        );
    }

    #[test]
    fn pointer_press_and_drag_away_does_not_confirm_toggle() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(106), "agree", false);

        checkbox.apply_input(InteractionInput::PointerEntered);
        checkbox.apply_input(InteractionInput::PointerPressed);
        checkbox.apply_input(InteractionInput::PointerExited);
        let event = checkbox.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn release_without_prior_press_does_not_confirm_toggle() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(107), "agree", false);

        checkbox.apply_input(InteractionInput::PointerEntered);
        let event = checkbox.apply_input(InteractionInput::PointerReleased);

        assert_eq!(event, None);
    }

    #[test]
    fn keyboard_toggle_confirms_when_focused() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(108), "agree", false);
        focus(&mut checkbox);

        let event = checkbox.handle_input(CheckboxInput::Toggle);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "agree".to_string(),
            })
        );
    }

    #[test]
    fn keyboard_toggle_ignored_when_not_focused() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(109), "agree", false);

        let event = checkbox.handle_input(CheckboxInput::Toggle);

        assert_eq!(event, None);
    }

    #[test]
    fn disabled_checkbox_widget_ignores_all_input() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(110), "agree", false);
        focus(&mut checkbox);
        checkbox.set_disabled(true);

        assert!(!checkbox.interaction_state().focused);

        checkbox.apply_input(InteractionInput::PointerEntered);
        checkbox.apply_input(InteractionInput::PointerPressed);
        let pointer_event = checkbox.apply_input(InteractionInput::PointerReleased);
        let keyboard_event = checkbox.handle_input(CheckboxInput::Toggle);

        assert_eq!(pointer_event, None);
        assert_eq!(keyboard_event, None);
    }

    #[test]
    fn hit_test_is_true_inside_rect_and_false_outside_for_checkbox_widget() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(111), "agree", false);
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(checkbox.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!checkbox.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_point_inside_rect_for_checkbox_widget() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(112), "agree", false);
        checkbox.set_disabled(true);
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(!checkbox.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_excludes_right_and_bottom_closed_edge_for_checkbox_widget() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(113), "agree", false);
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(!checkbox.hit_test((100.0, 5.0), (0.0, 0.0), size));
        assert!(!checkbox.hit_test((5.0, 20.0), (0.0, 0.0), size));
        assert!(checkbox.hit_test((99.9, 19.9), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_check_box_for_checkbox_widget() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(114), "agree", false);

        assert_eq!(checkbox.accessibility_role(), AccessibilityRole::CheckBox);
    }

    #[test]
    fn accessibility_label_is_label_for_checkbox_widget() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(114), "agree", false);

        assert_eq!(checkbox.accessibility_label(), Some("agree".to_string()));
    }

    #[test]
    fn clone_box_preserves_state_for_checkbox_widget() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(115), "agree", true);

        let cloned: Box<dyn Widget<FormState>> = checkbox.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(115));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 10.0,
            },
        );
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("[x] agree".to_string())
            ]
        );
    }

    #[test]
    fn widget_trait_object_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<FormState>> = Box::new(CheckboxWidget::new_static(
            WidgetId::from_raw(116),
            "agree",
            false,
        ));

        assert_eq!(widget.accessibility_role(), AccessibilityRole::CheckBox);
        assert!(!widget.interaction_state().disabled);
    }

    #[test]
    fn state_store_dispatch_toggles_state_via_legacy_button_bridge() {
        // 宣言層はレガシー `.button(label, toggle_handler)` で受け、
        // `StateStore::dispatch` を経由して状態へ反映後、`sync_from_state` で
        // 新値が還流することを確認する（モジュール doc の設計判断の E2E 検証）。
        let screen: Screen<FormState> =
            Screen::new("form").button(WidgetId::from_raw(1), "agree", toggle_agreed);
        let mut store = StateStore::new(FormState { agreed: false });
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_dynamic(WidgetId::from_raw(117), "agree", false, read_agreed);
        focus(&mut checkbox);

        let event = checkbox
            .handle_input(CheckboxInput::Toggle)
            .expect("フォーカス中の Toggle は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert!(store.state().agreed);
        assert_eq!(store.version(), 1);

        checkbox.sync_from_state(store.state());
        assert!(checkbox.checked());

        // もう 1 回で元へ戻る（トグル往復）。
        let event = checkbox
            .handle_input(CheckboxInput::Toggle)
            .expect("フォーカス中の Toggle は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert!(!store.state().agreed);
        assert_eq!(store.version(), 2);

        checkbox.sync_from_state(store.state());
        assert!(!checkbox.checked());
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        let mut checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(118), "agree", false);
        checkbox.apply_input(InteractionInput::PointerEntered);

        assert_eq!(checkbox.visual_state(), VisualState::Hovered);

        checkbox.set_disabled(true);
        assert_eq!(checkbox.visual_state(), VisualState::Disabled);
    }

    // --- TASK-8.4-S14: paint_styled ---

    fn test_resolver_for_input() -> crate::token::TokenResolver {
        use crate::token::{
            SemanticColorMapping, SemanticColorRole, TokenPath, TokenRef, TokenSet,
        };

        let mut base = crate::token::palette_token_set().expect("パレット構築は成功する");
        base.extend(crate::token::typography_token_set().expect("タイポグラフィ構築は成功する"));
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Surface,
                TokenRef::new(TokenPath::from_dotted("color.palette.gray-100").unwrap()),
            )
            .with(
                SemanticColorRole::Primary,
                TokenRef::new(TokenPath::from_dotted("color.palette.blue-500").unwrap()),
            )
            .merge_into(&mut base)
            .expect("マージできる");
        crate::token::TokenResolver::new(base, TokenSet::new())
    }

    #[test]
    fn checkbox_widget_paint_styled_resolves_input_colors() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(119), "agree", false);
        let resolver = test_resolver_for_input();

        let commands = checkbox.paint_styled(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
            &resolver,
        );

        assert_eq!(
            commands.len(),
            checkbox
                .paint(
                    (0.0, 0.0),
                    Measurement {
                        width: 100.0,
                        height: 20.0,
                    }
                )
                .len()
        );
        assert!(matches!(commands[0], PaintCommand::StyledRect { .. }));
    }

    #[test]
    fn checkbox_widget_paint_styled_falls_back_when_resolver_lacks_tokens() {
        let checkbox: CheckboxWidget<FormState> =
            CheckboxWidget::new_static(WidgetId::from_raw(120), "agree", false);
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );

        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };
        let styled = checkbox.paint_styled((0.0, 0.0), size, &empty_resolver);
        let plain = checkbox.paint((0.0, 0.0), size);
        assert_eq!(styled, plain);
    }
}
