//! カード・セパレータウィジェット `Card<S>` / `Separator<S>`
//! （TASK-8.3-S58/S59 / REQ-8、Issue #153/#154）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の分解
//! sub-issue（58/59/60）。カード・セパレータの状態・イベント処理（#154）を担い、
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、先行ウィジェットが確立した
//! 以下のパターンを組み合わせて [`Card`] / [`Separator`] を新設する。
//!
//! - 複数子コンテナ（[`super::stack_grid::Stack`] S43）: `Card` は `children:
//!   Vec<Box<dyn Widget<S>>>` を縦（`Stack::direction == Column` 相当）に積む
//!   グルーピングコンテナであり、`sync_from_state` は全子へ委譲するのみで
//!   `S` へ直接触れない。数値健全化ヘルパー（`normalize_non_negative` /
//!   `clamp_extent` / `MAX_CONTENT_EXTENT`）も `stack_grid` と同じ安全側判断を
//!   モジュールローカルに再定義する（`stack_grid` からは非公開のため）。
//! - 非インタラクティブ要素（[`super::progress::ProgressBar`] S31 /
//!   [`super::tooltip::Tooltip`] S55）: `Separator` は装飾専用であり
//!   `hit_test` は常に `false` を返す。
//! - 寸法プレースホルダ定数（[`super::dialog::Dialog::PANEL_PADDING`] 相当の
//!   考え方）: `Separator::thickness` は既定 `1.0` の論理ピクセル相当値。
//!
//! ## `Card<S>`
//!
//! - `padding`（構築時に非有限値・負値を `0.0` へ丸める）を持つグルーピング
//!   コンテナ。`measure` は子を縦に積んだ集計（`Stack::measure` の Column
//!   相当）に `padding * 2.0` を加算する。子へ渡す制約は `available` から
//!   `padding * 2.0` を差し引いた内側の利用可能領域とする。
//! - `paint` は `[Rect（カード背景・枠）]` に続けて子の `paint` 結果を宣言順に
//!   連結する。`padding` 分のオフセットは `PaintCommand` に座標が表現できない
//!   ため描画へは反映せず、`measure` の寸法計算のみに使う（`Stack` / `Grid` と
//!   同じく `PaintCommand` を拡張しない安全側判断、モジュール doc 参照）。子の
//!   再測定に渡す制約は、割り当てられた `size` から `padding * 2.0` を差し引
//!   いた内側の利用可能領域とし、`measure` が `available` から導く内側領域と
//!   同じ式（[`inner_extent`]）を用いる。水平方向に伸縮する `Separator` 等の
//!   フレキシブルな子が `measure` と `paint` とで異なる幅を報告しないための
//!   修正（Bugbot 指摘: PR #397 レビュー）。
//! - `hit_test` は自身の矩形包含判定（`disabled` 時は `false`）。S58 の挙動を
//!   変更しない（タップ可否で `hit_test` を切り替えると、既存の
//!   `tests/card_separator_widget.rs` の前提が崩れるため。タップ
//!   不可カードでも領域自体は変わらず存在する、という意味論を維持する）。
//! - `accessibility_role` は既存の [`AccessibilityRole::Group`]（TASK-8.3-S43）を
//!   共用する。レイアウト系コンテナへ個別ロールを増やさない判断を
//!   踏襲し、タップ可能化してもロールを昇格しない（専用バリアントは新設しない）。
//! - **状態・イベント処理（TASK-8.3-S59 / #154）**: [`CardInput`]
//!   （`Activate` のみ）+ [`Card::apply_input`] / [`Card::handle_input`] を新設する。
//!   `Button`（S8）/ `ToggleSwitch` と同一の押下確定意味論を踏襲する。
//!   - `apply_input`: 押下確定は `InteractionState::apply` へ入力を適用する
//!     **前**の `pressed && hovered` を見て `PointerReleased` で判定する
//!     （press-and-drag-away では発火しない）。`tap_label` を持つタップカード
//!     のときのみ [`crate::state::Event::ButtonClicked`] を返す。`tap_label` が
//!     `None` の非タップカードでも `InteractionState` 自体への追従（hover/press/
//!     focus の視覚状態反映）は行う。disabled 中は `InteractionState::apply` が
//!     入力を無視するため発火しない。
//!   - `handle_input`: `disabled || !focused || tap_label.is_none()` のいずれか
//!     で `None`。`Activate` はフォーカス保持中のタップカードのみ発火する
//!     （`Button::handle_input` と同じ「フォーカス保持中のみ操作可能」判断）。
//!   - イベントは既存の `Event::ButtonClicked { label }` へブリッジするのみで、
//!     専用バリアント（例: `Event::CardTapped`）は新設しない（Dialog の dismiss
//!     ラベル・Menu と同じ判断。widget 層から `&mut S` へ一切触れず、状態書き
//!     込みを `StateStore::dispatch` の関数ポインタ経路に限定する AI 型安全
//!     DSL の中核契約を維持する）。
//!   - [`Card::visual_state`] は [`InteractionState::visual_state`] への委譲
//!     （`Disabled > Pressed > Hovered > Idle`）。hover エレベーション等の
//!     見た目解決は TASK-8.4 デザイントークン側が本アクセサを使う想定。
//!
//! ## `Separator<S>`
//!
//! - 水平・垂直の向き（[`SeparatorOrientation`]）と太さ `thickness`（既定
//!   `1.0`、非有限値・非正値は既定値へフォールバック）を持つ非インタラクティブ
//!   な区切り線。型パラメータ `S` は `Widget<S>` 契約への適合のためのみ保持し
//!   （`PhantomData<fn() -> S>`）、状態を束縛しない。
//! - [`super::layout::FlexDirection`]（`Stack` が使う主軸方向）の流用は見送る。
//!   `FlexDirection::Row`/`Column` はコンテナの子並べ方向という別の意味論を
//!   持ち、セパレータの「線がどちらを向くか」という意味と紐付けると読み手を
//!   混乱させるため、専用の [`SeparatorOrientation`] を新設する。
//! - `measure`: `Horizontal` は `(available.width の正規化値, thickness)`、
//!   `Vertical` は `(thickness, available.height の正規化値)` を返す。
//! - `paint`: `[Rect]` のみ（自前の `Text` を出さない）。
//! - `hit_test`: 常に `false`（[`super::progress::ProgressBar`] /
//!   [`super::progress::Spinner`] と同じ非インタラクティブ契約）。
//! - `accessibility_role`: [`AccessibilityRole::Separator`]（S58 で新設、
//!   `widget.rs` 参照）。
//! - **機能変更なし**: `Separator` は装飾専用の非インタラク
//!   ティブ契約（`hit_test` 常時 `false`・`sync_from_state` no-op）を維持し、
//!   `apply_input` / `handle_input` を持たない（[`super::progress::ProgressBar`]
//!   / [`super::progress::Spinner`] と同じ「状態・イベント処理を持たない装飾
//!   専用ウィジェット」という設計判断）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self`
//! を要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。
//! この契約を変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に
//! 記録する）:
//! (a) `Event::CardTapped` 等の専用イベントバリアント新設・`WidgetId` ベースの
//! ディスパッチ統合（全インタラクティブウィジェット共通の既知課題。
//! `Event::ButtonClicked` ブリッジで統一する現状の設計を維持）、
//! (b) 角丸・影・エレベーション等の視覚スタイル実体（`Card::visual_state` が
//! 返す [`VisualState`] を消費する具体トークンは TASK-8.4 のスコープ）、
//! (c) ヘッダー・フッター等のカード内スロット構造、
//! (d) `PointerDispatcher` からの実ルーティング配線・プラットフォームレイヤの
//! 実ポインタ接続（既存契約を変更しない）、
//! (e) 子ウィジェットへのフォーカス走査（`Stack`/`Grid` S44 の `FocusNext` 系）:
//! カードは `Stack` を内包し得るグルーピングであり、走査責務は `Stack` 側に
//! 既存のため重複実装しない、
//! (f) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。
//!
//! TASK-8.4-S15（REQ-8 / REQ-2、#171）で `crate::token`（デザイントークン基盤、
//! TASK-8.4）が確定した寸法トークンとの配線を追加した。`Card::with_padding_token`
//! は [`crate::token::SpacingValue`]（余白は spacing カテゴリ、設計文書 §4.1
//! 割り当て表準拠）、`Separator::with_thickness_token` は
//! [`crate::token::SizeValue`]（線の太さはコントロール寸法に近いため size
//! カテゴリを採用。§4.1 に区切り線専用の割り当ては無く、既存カテゴリのうち
//! 意味的に近い size を選ぶ判断）を受け取り、既存の生 `f32` 版ビルダーへ
//! 委譲する。

use std::marker::PhantomData;

use crate::state::Event;

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// カードへのキーボード入力を表す列挙（TASK-8.3-S59、`Card<S>` の核心）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する
/// （`button::ButtonInput` / `checkbox::CheckboxInput` と同じ設計判断、
/// `.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardInput {
    /// 押下を確定する（Space/Enter 相当）。
    Activate,
}

/// 集計値（子測定の縦積み合計等）を緩和・クランプする際の上限
/// （論理ピクセル相当のプレースホルダ定数）。[`super::stack_grid::MAX_CONTENT_EXTENT`]
/// と同じ安全側判断（`f32::MAX` は子側の乗算・加算でオーバーフローし `inf` が
/// 伝播しうるため、実用上十分大きい有限値に留める。OWASP A04 相当）。
/// `stack_grid` からは非公開のためモジュールごとに定数を持つ（値は同一）。
const MAX_CONTENT_EXTENT: f32 = 1_048_576.0;

/// セパレータの既定の太さ（論理ピクセル相当）。非有限値・非正値の
/// `with_thickness` 呼び出し時のフォールバック先でもある。
const DEFAULT_SEPARATOR_THICKNESS: f32 = 1.0;

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// [`super::stack_grid::normalize_non_negative`] と同じ安全側の丸め方針を
/// padding・available 成分の正規化に適用するための小さなヘルパー。
fn normalize_non_negative(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// 合計値を [`MAX_CONTENT_EXTENT`] で上限クランプする（inf 伝播・オーバー
/// フローした集計値がそのまま子孫へ伝わるのを防ぐ、`Stack` / `Grid` と同型の
/// 安全側処理）。
fn clamp_extent(value: f32) -> f32 {
    if value.is_finite() {
        value.min(MAX_CONTENT_EXTENT)
    } else {
        MAX_CONTENT_EXTENT
    }
}

/// 外側の利用可能領域（`available` または割り当て済み `size` の一成分）から
/// `padding_total`（`padding * 2.0`）を差し引いた内側の利用可能領域を返す。
/// 減算結果が負値になる場合（`padding` が外側領域を上回る極端な入力）は
/// `0.0` へ丸める（[`normalize_non_negative`] と同じ安全側判断）。
///
/// [`Card::measure`] と [`Card::paint`] の双方がこの関数を経由して子への
/// 制約を導出することで、両フェーズで内側領域の計算式を一致させる
/// （Bugbot 指摘: PR #397 レビュー、モジュール doc 参照）。
fn inner_extent(outer: f32, padding_total: f32) -> f32 {
    normalize_non_negative(outer - padding_total)
}

/// カードのグルーピングコンテナウィジェット（TASK-8.3-S58、`Widget<S>` 実装型）。
///
/// 本ウィジェットセット初のカード型であり、`children` を縦（`Stack::direction ==
/// Column` 相当）に積み、周囲に `padding` を確保する。`Debug` / `Clone` は手動
/// 実装する（[`super::stack_grid::Stack`] と同じく、状態型 `S` を所有していない
/// ため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を
/// 避ける。`Clone` は各子の [`Widget::clone_box`] 経由で複製する）。
pub struct Card<S> {
    id: WidgetId,
    children: Vec<Box<dyn Widget<S>>>,
    /// カード内側の余白（論理ピクセル相当）。非有限値・負値は [`Self::with_padding`]
    /// が構築時に `0.0` へ丸める（[`normalize_non_negative`] 参照）。
    padding: f32,
    interaction: InteractionState,
    /// タップ確定時に発火する `Event::ButtonClicked` のラベル。`None` は非
    /// タップカード（S58 までの既定挙動）を表す（[`Self::with_tap_label`] 参照）。
    /// タイトル等から自動導出せず、`Dialog` の dismiss ラベルと同じく構築者が
    /// 明示指定する設計とする（TASK-8.3-S59）。
    tap_label: Option<String>,
}

impl<S> std::fmt::Debug for Card<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Card")
            .field("id", &self.id)
            .field("children", &self.children)
            .field("padding", &self.padding)
            .field("interaction", &self.interaction)
            .field("tap_label", &self.tap_label)
            .finish()
    }
}

impl<S> Clone for Card<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            children: self.children.iter().map(|c| c.clone_box()).collect(),
            padding: self.padding,
            interaction: self.interaction,
            tap_label: self.tap_label.clone(),
        }
    }
}

impl<S: 'static> Card<S> {
    /// 子を持たないカードを構築する。既定の padding は `0.0`。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            children: Vec::new(),
            padding: 0.0,
            interaction: InteractionState::default(),
            tap_label: None,
        }
    }

    /// 子を末尾に追加するビルダーメソッド（[`super::stack_grid::Stack::with_child`]
    /// と同型）。
    pub fn with_child(mut self, child: impl Widget<S> + 'static) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// 内側の余白を設定するビルダーメソッド。非有限値（NaN/inf）・負値は
    /// `0.0` へ丸める（[`normalize_non_negative`] 参照）。
    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = normalize_non_negative(padding);
        self
    }

    /// 内側の余白をスペーシングトークン（[`crate::token::SpacingValue`]）から
    /// 設定するビルダーメソッド（TASK-8.4-S15、#171）。内部では
    /// [`Self::with_padding`] へ委譲する（トークン値は構築時点で有限・非負が
    /// 検証済みのため、[`normalize_non_negative`] は防御の多重化に留まる）。
    pub fn with_padding_token(self, padding: crate::token::SpacingValue) -> Self {
        self.with_padding(padding.value())
    }

    /// 保持している子の数（テスト・観測用）。
    pub fn children_len(&self) -> usize {
        self.children.len()
    }

    /// 現在の padding 値（テスト・観測用）。
    pub fn padding(&self) -> f32 {
        self.padding
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// カードをタップ可能にするビルダーメソッド（TASK-8.3-S59 の核心）。
    ///
    /// `label` は押下確定時に発火する `Event::ButtonClicked { label }` の値。
    /// タイトル等から自動導出せず明示指定する（`Dialog` の dismiss ラベルと
    /// 同型の判断、モジュール doc 参照）。
    pub fn with_tap_label(mut self, label: impl Into<String>) -> Self {
        self.tap_label = Some(label.into());
        self
    }

    /// 現在のタップラベル（テスト・観測用）。`None` は非タップカードを表す。
    pub fn tap_label(&self) -> Option<&str> {
        self.tap_label.as_deref()
    }

    /// タップ可能なカードかどうか（[`Self::tap_label`] が `Some` か）。
    pub fn is_tappable(&self) -> bool {
        self.tap_label.is_some()
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`]
    /// への委譲。`button::Button::visual_state` と同型）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// ポインタ・フォーカス入力を適用する（TASK-8.3-S59 の核心）。
    ///
    /// 押下確定の判定は [`InteractionState::apply`] へ入力を適用する**前**の
    /// `pressed && hovered` を見て行う。`PointerReleased` 適用前に判定することで、
    /// press → 領域外へ exit（`pressed` が解除される）→ release という経路では
    /// 発火しない（press-and-drag-away の意味論、`button::Button::apply_input` と
    /// 同一）。`tap_label` が `Some`（タップカード）のときのみ確定イベントを
    /// 返す。`tap_label` が `None` の非タップカードでも `InteractionState` への
    /// 追従自体（hover/press/focus の視覚状態反映）は常に行う。disabled 中は
    /// `InteractionState::apply` が入力そのものを無視するため、`pressed` が
    /// 立たず押下確定も発火しない。
    pub fn apply_input(&mut self, input: InteractionInput) -> Option<Event> {
        let should_confirm = matches!(input, InteractionInput::PointerReleased)
            && self.interaction.pressed
            && self.interaction.hovered;

        self.interaction.apply(input);

        if !should_confirm {
            return None;
        }

        self.tap_label
            .clone()
            .map(|label| Event::ButtonClicked { label })
    }

    /// キーボード入力を処理する（TASK-8.3-S59 の核心）。
    ///
    /// disabled 中・非フォーカス中・非タップカードのいずれかであれば `None`
    /// を返す（`button::Button::handle_input` と同じ「フォーカス保持中のみ
    /// 操作可能」判断に、タップ可否のガードを加えたもの）。
    pub fn handle_input(&mut self, input: CardInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        match input {
            CardInput::Activate => self
                .tap_label
                .clone()
                .map(|label| Event::ButtonClicked { label }),
        }
    }
}

impl<S: 'static> Widget<S> for Card<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// reconcile を全子へ伝播する（[`super::stack_grid::Stack::sync_from_state`]
    /// と同型）。カード自身は `S` へ直接触れず、委譲のみを行う。
    fn sync_from_state(&mut self, state: &S) {
        for child in &mut self.children {
            child.sync_from_state(state);
        }
    }

    /// 子を縦に積んだ自然寸法（`Stack::measure` の Column 相当: 高さ = 子の
    /// 高さ合計、幅 = 子の幅の最大値）に `padding * 2.0` を加算して返す。
    /// `available` の非有限値・負値成分は `0.0` へ正規化し、集計値は
    /// [`MAX_CONTENT_EXTENT`] でクランプする（`Stack::measure` と同じ安全側
    /// 判断）。空 `children` は `padding * 2.0` のみを返す。
    ///
    /// 子へ渡す制約は `available` から `padding * 2.0` を差し引いた内側の
    /// 利用可能領域（[`Self::inner_extent`]）とする。[`Self::paint`] が子の
    /// 再測定に用いる制約（カードへ実際に割り当てられた `size` から同じ式で
    /// 導く内側領域）と揃えることで、水平方向に伸縮する `Separator` のような
    /// フレキシブルな子が `measure` と `paint` とで異なる幅を報告しないよう
    /// にする（Bugbot 指摘: PR #397 レビュー、L209-262 の食い違い）。
    fn measure(&self, available: Measurement) -> Measurement {
        let available = Measurement {
            width: normalize_non_negative(available.width),
            height: normalize_non_negative(available.height),
        };
        let padding_total = self.padding * 2.0;

        if self.children.is_empty() {
            return Measurement {
                width: clamp_extent(padding_total),
                height: clamp_extent(padding_total),
            };
        }

        let inner_available = Measurement {
            width: inner_extent(available.width, padding_total),
            height: inner_extent(available.height, padding_total),
        };

        let mut height_sum = 0.0f32;
        let mut width_max = 0.0f32;
        for child in &self.children {
            let measured = child.measure(inner_available);
            height_sum += normalize_non_negative(measured.height);
            width_max = width_max.max(normalize_non_negative(measured.width));
        }

        Measurement {
            width: clamp_extent(width_max + padding_total),
            height: clamp_extent(height_sum + padding_total),
        }
    }

    /// `[Rect（カード背景・枠）]` に続けて、子の縦積み配置（`Stack::paint` の
    /// Column 相当）による `paint` 結果を宣言順に連結する。`padding` 分の
    /// オフセットは `PaintCommand` に座標が表現できないため描画へは反映せず、
    /// [`Self::measure`] の寸法計算のみに使う（モジュール doc 参照）。
    ///
    /// 子の再測定に渡す制約は、割り当てられた `size` から `padding * 2.0` を
    /// 差し引いた内側の利用可能領域（[`Self::measure`] が `available` から
    /// 導いた内側領域と同じ式、[`inner_extent`] 参照）とする。ここを `size`
    /// そのまま（padding 未考慮）にすると、`measure` 時より `paint` 時の方が
    /// 制約が `padding * 2.0` 分大きくなり、伸縮するフレキシブルな子の幅が
    /// 両フェーズで食い違う（Bugbot 指摘、モジュール doc 参照）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];

        if self.children.is_empty() {
            return commands;
        }

        let padding_total = self.padding * 2.0;
        let child_constraint = Measurement {
            width: inner_extent(normalize_non_negative(size.width), padding_total),
            height: inner_extent(normalize_non_negative(size.height), padding_total),
        };
        let mut offset = 0.0f32;
        for child in &self.children {
            let measured = child.measure(child_constraint);
            let child_origin = (origin.0, origin.1 + offset);
            commands.extend(child.paint(child_origin, measured));
            offset = clamp_extent(offset + normalize_non_negative(measured.height));
        }

        commands
    }

    /// 自身の矩形包含判定。`disabled` の間は常に `false` を返す
    /// （[`super::checkbox::Checkbox::hit_test`] と同じ半開区間 `[origin,
    /// origin + size)` の判定規約）。カード自体をタップ可能にする実配線は
    /// S59（#154）のスコープ（モジュール doc 参照）。
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
        AccessibilityRole::Group
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }
}

/// [`Separator`] の向き（モジュール doc 参照。[`super::layout::FlexDirection`]
/// の流用は主軸の意味論が紛らわしいため見送り、専用の enum として新設する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeparatorOrientation {
    /// 水平な区切り線（既定）。縦に並ぶコンテンツの間を区切る。
    #[default]
    Horizontal,
    /// 垂直な区切り線。横に並ぶコンテンツの間を区切る。
    Vertical,
}

/// 非インタラクティブな区切り線ウィジェット（TASK-8.3-S58、`Widget<S>` 実装型）。
///
/// 型パラメータ `S` は `Widget<S>` 契約への適合のためのみ保持し
/// （`PhantomData<fn() -> S>`）、状態束縛は持たない。`Debug` / `Clone` は
/// 手動実装する（`PhantomData<fn() -> S>` は `S: Debug` / `S: Clone` を要求
/// しないため本来 `derive` でも問題ないが、他ウィジェットとの実装方針統一の
/// ため手動実装に揃える）。
pub struct Separator<S> {
    id: WidgetId,
    orientation: SeparatorOrientation,
    /// 線の太さ（論理ピクセル相当）。既定 [`DEFAULT_SEPARATOR_THICKNESS`]。
    /// 非有限値・非正値は [`Self::with_thickness`] が既定値へフォールバック
    /// させる。
    thickness: f32,
    interaction: InteractionState,
    _state: PhantomData<fn() -> S>,
}

impl<S> std::fmt::Debug for Separator<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Separator")
            .field("id", &self.id)
            .field("orientation", &self.orientation)
            .field("thickness", &self.thickness)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Separator<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            orientation: self.orientation,
            thickness: self.thickness,
            interaction: self.interaction,
            _state: PhantomData,
        }
    }
}

impl<S: 'static> Separator<S> {
    /// 既定（水平・太さ [`DEFAULT_SEPARATOR_THICKNESS`]）のセパレータを構築する。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            orientation: SeparatorOrientation::default(),
            thickness: DEFAULT_SEPARATOR_THICKNESS,
            interaction: InteractionState::default(),
            _state: PhantomData,
        }
    }

    /// 向きを設定するビルダーメソッド。
    pub fn with_orientation(mut self, orientation: SeparatorOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// 太さを設定するビルダーメソッド。非有限値（NaN/inf）・非正値は既定値
    /// [`DEFAULT_SEPARATOR_THICKNESS`] へフォールバックする（`0.0` へ丸めると
    /// 見えない区切り線になり構築者の意図に反するため、`Stack`/`Grid` の
    /// spacing とは異なりゼロを許容しない安全側判断）。
    pub fn with_thickness(mut self, thickness: f32) -> Self {
        self.thickness = if thickness.is_finite() && thickness > 0.0 {
            thickness
        } else {
            DEFAULT_SEPARATOR_THICKNESS
        };
        self
    }

    /// 太さをサイズトークン（[`crate::token::SizeValue`]）から設定するビルダー
    /// メソッド（TASK-8.4-S15、#171。カテゴリ選定はモジュール doc 参照）。
    /// 内部では [`Self::with_thickness`] へ委譲する（`size.none` 等の `0.0`
    /// トークンは既存契約どおり既定値へフォールバックする）。
    pub fn with_thickness_token(self, thickness: crate::token::SizeValue) -> Self {
        self.with_thickness(thickness.value())
    }

    /// 現在の向き（テスト・観測用）。
    pub fn orientation(&self) -> SeparatorOrientation {
        self.orientation
    }

    /// 現在の太さ（テスト・観測用）。
    pub fn thickness(&self) -> f32 {
        self.thickness
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }
}

impl<S: 'static> Widget<S> for Separator<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// 状態非依存の no-op。呼び出し自体は許容し、reconcile ループの呼び出し
    /// 規約（全ウィジェット一律呼び出し）を崩さない（[`super::tooltip::Tooltip`]
    /// の `TooltipSource::Static` 分岐と同じ考え方）。
    fn sync_from_state(&mut self, _state: &S) {}

    /// `Horizontal` は `(available.width の正規化値, thickness)`、`Vertical` は
    /// `(thickness, available.height の正規化値)` を返す。
    fn measure(&self, available: Measurement) -> Measurement {
        let available = Measurement {
            width: normalize_non_negative(available.width),
            height: normalize_non_negative(available.height),
        };
        match self.orientation {
            SeparatorOrientation::Horizontal => Measurement {
                width: available.width,
                height: self.thickness,
            },
            SeparatorOrientation::Vertical => Measurement {
                width: self.thickness,
                height: available.height,
            },
        }
    }

    /// `[Rect]` のみを返す（自前の `Text` を出さない）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        vec![PaintCommand::Rect]
    }

    /// 常に `false`（[`super::progress::ProgressBar`] / [`super::progress::Spinner`]
    /// と同じ非インタラクティブ契約。区切り線は装飾であり操作対象ではない）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Separator
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
    use crate::ui::label::Label;

    struct ProfileState {
        name: String,
    }

    fn get_name(state: &ProfileState) -> String {
        state.name.clone()
    }

    fn label(id: u64) -> Label<ProfileState> {
        Label::bound(WidgetId::from_raw(id), get_name)
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    /// テスト専用プローブウィジェット（`stack_grid` の `ProbeWidget` を流用）。
    /// `measure` は固定寸法、`paint` は自身が呼ばれた origin を `Text` へ
    /// 埋め込むことで、コンテナ側の子配置計算（origin・順序）を直接検証する。
    #[derive(Debug, Clone)]
    struct ProbeWidget {
        id: WidgetId,
        size: Measurement,
        interaction: InteractionState,
    }

    impl ProbeWidget {
        fn new(id: u64, width: f32, height: f32) -> Self {
            Self {
                id: WidgetId::from_raw(id),
                size: Measurement { width, height },
                interaction: InteractionState::default(),
            }
        }
    }

    impl Widget<ProfileState> for ProbeWidget {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, _state: &ProfileState) {}

        fn measure(&self, _available: Measurement) -> Measurement {
            self.size
        }

        fn paint(&self, origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Text(format!(
                "{}:{},{}",
                self.id.raw(),
                origin.0,
                origin.1
            ))]
        }

        fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
            false
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Group
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<ProfileState>> {
            Box::new(self.clone())
        }
    }

    /// テスト専用のフレキシブルプローブウィジェット（[`Separator`] の水平方向
    /// 挙動を模す）。`measure` に渡された `available.width` をそのまま自身の
    /// 幅として報告し、`paint` はその `measure` 呼び出し時に観測した幅を
    /// `Text` へ埋め込む。`Card::measure` と `Card::paint` が子へ渡す制約
    /// （内側の利用可能領域）が一致していることを、`measure` 単独呼び出しと
    /// `paint` 経由の再測定の双方で観測される幅を突き合わせて検証するために
    /// 使う（Bugbot 指摘の回帰テスト、モジュール doc 参照）。
    #[derive(Debug, Clone)]
    struct FlexProbe {
        id: WidgetId,
        height: f32,
        interaction: InteractionState,
    }

    impl FlexProbe {
        fn new(id: u64, height: f32) -> Self {
            Self {
                id: WidgetId::from_raw(id),
                height,
                interaction: InteractionState::default(),
            }
        }
    }

    impl Widget<ProfileState> for FlexProbe {
        fn id(&self) -> WidgetId {
            self.id
        }

        fn sync_from_state(&mut self, _state: &ProfileState) {}

        fn measure(&self, available: Measurement) -> Measurement {
            Measurement {
                width: normalize_non_negative(available.width),
                height: self.height,
            }
        }

        fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
            vec![PaintCommand::Text(format!(
                "{}:{},{}:w={}",
                self.id.raw(),
                origin.0,
                origin.1,
                size.width
            ))]
        }

        fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
            false
        }

        fn accessibility_role(&self) -> AccessibilityRole {
            AccessibilityRole::Group
        }

        fn interaction_state(&self) -> InteractionState {
            self.interaction
        }

        fn clone_box(&self) -> Box<dyn Widget<ProfileState>> {
            Box::new(self.clone())
        }
    }

    // --- Card: 構築とアクセサ ---

    #[test]
    fn card_new_has_zero_padding_and_no_children() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(1));
        assert_eq!(card.padding(), 0.0);
        assert_eq!(card.children_len(), 0);
        assert_eq!(card.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn card_new_interaction_state_is_all_false() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(2));
        let interaction = card.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    #[test]
    fn card_with_padding_clamps_negative_nan_infinite_to_zero() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(3)).with_padding(-5.0);
        assert_eq!(card.padding(), 0.0);

        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(4)).with_padding(f32::NAN);
        assert_eq!(card.padding(), 0.0);

        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(5)).with_padding(f32::INFINITY);
        assert_eq!(card.padding(), 0.0);
    }

    #[test]
    fn card_with_padding_accepts_positive_finite_value() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(6)).with_padding(12.0);
        assert_eq!(card.padding(), 12.0);
    }

    #[test]
    fn card_with_padding_token_matches_raw_f32_equivalent() {
        // TASK-8.4-S15（#171）: トークン型ビルダーと生 f32 ビルダーの同値性検証。
        let padding = crate::token::SpacingValue::new(12.0).expect("正当な値");
        let from_token: Card<ProfileState> =
            Card::new(WidgetId::from_raw(31)).with_padding_token(padding);
        let from_raw: Card<ProfileState> = Card::new(WidgetId::from_raw(31)).with_padding(12.0);
        assert_eq!(from_token.padding(), from_raw.padding());
    }

    // --- Card: sync_from_state の子委譲 ---

    #[test]
    fn card_sync_from_state_propagates_to_all_children() {
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(7))
            .with_child(label(100))
            .with_child(label(101));
        let state = ProfileState {
            name: "太郎".to_string(),
        };
        card.sync_from_state(&state);

        let commands = card.paint((0.0, 0.0), sized(100.0, 100.0));
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0], PaintCommand::Rect);
        assert_eq!(commands[1], PaintCommand::Text("太郎".to_string()));
        assert_eq!(commands[2], PaintCommand::Text("太郎".to_string()));
    }

    // --- Card: measure ---

    #[test]
    fn card_measure_sums_child_heights_and_adds_padding() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(8))
            .with_padding(10.0)
            .with_child(ProbeWidget::new(200, 30.0, 10.0))
            .with_child(ProbeWidget::new(201, 20.0, 40.0));

        let measured = card.measure(sized(200.0, 200.0));
        // 高さ = 10 + 40 + padding * 2(20) = 70、幅 = max(30, 20) + padding * 2(20) = 50。
        assert_eq!(measured.height, 70.0);
        assert_eq!(measured.width, 50.0);
    }

    #[test]
    fn card_measure_empty_children_returns_padding_only() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(9)).with_padding(8.0);
        let measured = card.measure(sized(100.0, 100.0));
        assert_eq!(measured.width, 16.0);
        assert_eq!(measured.height, 16.0);
    }

    #[test]
    fn card_measure_normalizes_non_finite_available_to_zero() {
        let card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(10)).with_child(ProbeWidget::new(202, 10.0, 10.0));
        let measured = card.measure(sized(f32::NAN, f32::INFINITY));
        // ProbeWidget は固定寸法を返すため available 正規化の影響を受けないが、
        // 呼び出し自体が panic しないことを確認する。
        assert_eq!(measured.width, 10.0);
        assert_eq!(measured.height, 10.0);
    }

    #[test]
    fn card_measure_clamps_extent_for_extreme_child_measurement() {
        // 有限だが `MAX_CONTENT_EXTENT` を超える巨大な子測定値は、
        // `normalize_non_negative` では丸められない（有限値のため）が、
        // 最終集計値は `clamp_extent` により上限へ丸められることを検証する。
        let card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(11)).with_child(ProbeWidget::new(203, 10.0, f32::MAX));
        let measured = card.measure(sized(100.0, 100.0));
        assert!(measured.height.is_finite());
        assert_eq!(measured.height, MAX_CONTENT_EXTENT);
    }

    #[test]
    fn card_measure_and_paint_agree_on_flexible_child_width() {
        // Bugbot 指摘（PR #397 レビュー）の回帰テスト: `Card::measure` が子へ
        // 渡す内側の利用可能領域（`available` - `padding * 2.0`）と、
        // `Card::paint` が再測定に渡す内側の利用可能領域（割り当てられた
        // `size` - `padding * 2.0`）が一致し、水平方向に伸縮するフレキシブル
        // な子（`FlexProbe`、`Separator` の挙動を模す）が両フェーズで同じ幅を
        // 報告することを検証する。
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(30))
            .with_padding(10.0)
            .with_child(FlexProbe::new(206, 5.0));

        let available = sized(100.0, 100.0);
        let measured = card.measure(available);
        // 内側の利用可能領域 = 100 - padding*2(20) = 80。
        // card 全体の幅 = 80 + padding*2(20) = 100、高さ = 5 + 20 = 25。
        assert_eq!(measured.width, 100.0);
        assert_eq!(measured.height, 25.0);

        // `measure` が返した寸法をそのままカードへ割り当てて `paint` した際、
        // 子の再測定幅（`w=` として埋め込まれる）が `measure` フェーズと同じ
        // 80.0 になることを確認する（修正前は size(100) がそのまま制約となり
        // `w=100` に食い違っていた）。
        let commands = card.paint((0.0, 0.0), measured);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Rect);
        assert_eq!(commands[1], PaintCommand::Text("206:0,0:w=80".to_string()));
    }

    // --- Card: paint（順序・子なし） ---

    #[test]
    fn card_paint_without_children_is_rect_only() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(12));
        let commands = card.paint((0.0, 0.0), sized(100.0, 100.0));
        assert_eq!(commands, vec![PaintCommand::Rect]);
    }

    #[test]
    fn card_paint_appends_children_after_leading_rect_in_order() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(13))
            .with_child(ProbeWidget::new(204, 10.0, 20.0))
            .with_child(ProbeWidget::new(205, 10.0, 30.0));

        let commands = card.paint((2.0, 3.0), sized(100.0, 100.0));
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0], PaintCommand::Rect);
        assert_eq!(commands[1], PaintCommand::Text("204:2,3".to_string()));
        // 2 番目の子の origin.y = 3 + 20（1 番目の高さ）= 23。
        assert_eq!(commands[2], PaintCommand::Text("205:2,23".to_string()));
    }

    // --- Card: hit_test ---

    #[test]
    fn card_hit_test_returns_true_when_point_inside_rect() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(14));
        let hit = card.hit_test((5.0, 5.0), (0.0, 0.0), sized(10.0, 10.0));
        assert!(hit);
    }

    #[test]
    fn card_hit_test_returns_false_when_point_outside_rect() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(15));
        let hit = card.hit_test((50.0, 50.0), (0.0, 0.0), sized(10.0, 10.0));
        assert!(!hit);
    }

    #[test]
    fn card_hit_test_returns_false_when_disabled() {
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(16));
        card.set_disabled(true);
        let hit = card.hit_test((5.0, 5.0), (0.0, 0.0), sized(10.0, 10.0));
        assert!(!hit);
    }

    // --- Card: accessibility_role・clone_box ---

    #[test]
    fn card_accessibility_role_is_group() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(17));
        assert_eq!(card.accessibility_role(), AccessibilityRole::Group);
    }

    #[test]
    fn card_clone_box_produces_independent_copy_of_children() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(18)).with_child(label(102));
        let cloned = card.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(18));
        assert_eq!(
            cloned.paint((0.0, 0.0), sized(80.0, 20.0)),
            card.paint((0.0, 0.0), sized(80.0, 20.0))
        );
    }

    #[test]
    fn card_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Card<ProfileState>>();
    }

    // --- Card: タップラベル・visual_state（TASK-8.3-S59） ---

    #[test]
    fn card_new_has_no_tap_label_and_is_not_tappable() {
        let card: Card<ProfileState> = Card::new(WidgetId::from_raw(40));
        assert_eq!(card.tap_label(), None);
        assert!(!card.is_tappable());
    }

    #[test]
    fn card_with_tap_label_sets_tap_label_and_is_tappable() {
        let card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(41)).with_tap_label("open_detail");
        assert_eq!(card.tap_label(), Some("open_detail"));
        assert!(card.is_tappable());
    }

    #[test]
    fn card_visual_state_delegates_to_interaction_state_priority() {
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(42));
        assert_eq!(card.visual_state(), VisualState::Idle);

        card.apply_input(InteractionInput::PointerEntered);
        assert_eq!(card.visual_state(), VisualState::Hovered);

        card.apply_input(InteractionInput::PointerPressed);
        assert_eq!(card.visual_state(), VisualState::Pressed);

        card.set_disabled(true);
        assert_eq!(card.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn card_clone_box_preserves_tap_label() {
        let card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(43)).with_tap_label("dup_label");
        let cloned = card.clone();
        assert_eq!(cloned.tap_label(), Some("dup_label"));

        let boxed = card.clone_box();
        // clone_box は `Box<dyn Widget<S>>` を返すため `tap_label` へ直接アクセス
        // できないが、id が保たれることで複製が行われたことを確認する。
        assert_eq!(boxed.id(), card.id());
    }

    // --- Card: apply_input（ポインタ入力、TASK-8.3-S59） ---

    #[test]
    fn card_apply_input_tappable_fires_button_clicked_on_press_and_release_inside() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(44)).with_tap_label("tap_target");

        assert_eq!(card.apply_input(InteractionInput::PointerEntered), None);
        assert_eq!(card.apply_input(InteractionInput::PointerPressed), None);
        let event = card.apply_input(InteractionInput::PointerReleased);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "tap_target".to_string()
            })
        );
    }

    #[test]
    fn card_apply_input_does_not_fire_on_press_and_drag_away() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(45)).with_tap_label("tap_target");

        card.apply_input(InteractionInput::PointerEntered);
        card.apply_input(InteractionInput::PointerPressed);
        card.apply_input(InteractionInput::PointerExited);
        let event = card.apply_input(InteractionInput::PointerReleased);
        assert_eq!(event, None);
    }

    #[test]
    fn card_apply_input_does_not_fire_on_release_without_press() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(46)).with_tap_label("tap_target");

        card.apply_input(InteractionInput::PointerEntered);
        let event = card.apply_input(InteractionInput::PointerReleased);
        assert_eq!(event, None);
    }

    #[test]
    fn card_apply_input_ignores_pointer_input_while_disabled() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(47)).with_tap_label("tap_target");
        card.set_disabled(true);

        card.apply_input(InteractionInput::PointerEntered);
        card.apply_input(InteractionInput::PointerPressed);
        let event = card.apply_input(InteractionInput::PointerReleased);
        assert_eq!(event, None);
        let interaction = card.interaction_state();
        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
    }

    #[test]
    fn card_apply_input_non_tappable_never_fires_but_still_tracks_interaction_state() {
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(48));

        card.apply_input(InteractionInput::PointerEntered);
        card.apply_input(InteractionInput::PointerPressed);
        let event = card.apply_input(InteractionInput::PointerReleased);
        assert_eq!(event, None);

        // 非タップカードでも hover/press の視覚状態追従自体は行われる
        // （モジュール doc 参照）。
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(49));
        card.apply_input(InteractionInput::PointerEntered);
        assert!(card.interaction_state().hovered);
    }

    // --- Card: handle_input（キーボード入力、TASK-8.3-S59） ---

    #[test]
    fn card_handle_input_activate_fires_when_focused_and_tappable() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(50)).with_tap_label("tap_target");
        card.apply_input(InteractionInput::FocusGained);

        let event = card.handle_input(CardInput::Activate);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "tap_target".to_string()
            })
        );
    }

    #[test]
    fn card_handle_input_activate_returns_none_when_not_focused() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(51)).with_tap_label("tap_target");
        let event = card.handle_input(CardInput::Activate);
        assert_eq!(event, None);
    }

    #[test]
    fn card_handle_input_activate_returns_none_when_disabled() {
        let mut card: Card<ProfileState> =
            Card::new(WidgetId::from_raw(52)).with_tap_label("tap_target");
        card.apply_input(InteractionInput::FocusGained);
        card.set_disabled(true);

        let event = card.handle_input(CardInput::Activate);
        assert_eq!(event, None);
    }

    #[test]
    fn card_handle_input_activate_returns_none_when_not_tappable() {
        let mut card: Card<ProfileState> = Card::new(WidgetId::from_raw(53));
        card.apply_input(InteractionInput::FocusGained);

        let event = card.handle_input(CardInput::Activate);
        assert_eq!(event, None);
    }

    // --- Separator: 構築とアクセサ ---

    #[test]
    fn separator_new_has_horizontal_orientation_and_default_thickness() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(19));
        assert_eq!(separator.orientation(), SeparatorOrientation::Horizontal);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);
        assert_eq!(separator.id(), WidgetId::from_raw(19));
    }

    #[test]
    fn separator_with_orientation_overrides_default() {
        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(20)).with_orientation(SeparatorOrientation::Vertical);
        assert_eq!(separator.orientation(), SeparatorOrientation::Vertical);
    }

    #[test]
    fn separator_with_thickness_accepts_positive_finite_value() {
        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(21)).with_thickness(4.0);
        assert_eq!(separator.thickness(), 4.0);
    }

    #[test]
    fn separator_with_thickness_token_matches_raw_f32_equivalent() {
        // TASK-8.4-S15（#171）: トークン型ビルダーと生 f32 ビルダーの同値性検証。
        let thickness = crate::token::SizeValue::new(2.0).expect("正当な値");
        let from_token: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(34)).with_thickness_token(thickness);
        let from_raw: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(34)).with_thickness(2.0);
        assert_eq!(from_token.thickness(), from_raw.thickness());
    }

    #[test]
    fn separator_with_thickness_falls_back_to_default_on_non_finite_or_non_positive() {
        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(22)).with_thickness(-1.0);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);

        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(23)).with_thickness(0.0);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);

        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(24)).with_thickness(f32::NAN);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);

        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(25)).with_thickness(f32::INFINITY);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);
    }

    // --- Separator: sync_from_state（no-op） ---

    #[test]
    fn separator_sync_from_state_is_noop_and_does_not_panic() {
        let mut separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(26));
        let state = ProfileState {
            name: "花子".to_string(),
        };
        separator.sync_from_state(&state);
        assert_eq!(separator.thickness(), DEFAULT_SEPARATOR_THICKNESS);
    }

    // --- Separator: measure ---

    #[test]
    fn separator_horizontal_measure_uses_available_width_and_thickness() {
        let separator: Separator<ProfileState> =
            Separator::new(WidgetId::from_raw(27)).with_thickness(2.0);
        let measured = separator.measure(sized(80.0, 999.0));
        assert_eq!(measured.width, 80.0);
        assert_eq!(measured.height, 2.0);
    }

    #[test]
    fn separator_vertical_measure_uses_available_height_and_thickness() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(28))
            .with_orientation(SeparatorOrientation::Vertical)
            .with_thickness(3.0);
        let measured = separator.measure(sized(999.0, 60.0));
        assert_eq!(measured.width, 3.0);
        assert_eq!(measured.height, 60.0);
    }

    #[test]
    fn separator_measure_normalizes_non_finite_available_to_zero() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(29));
        let measured = separator.measure(sized(f32::NAN, f32::INFINITY));
        assert_eq!(measured.width, 0.0);
        assert_eq!(measured.height, DEFAULT_SEPARATOR_THICKNESS);
    }

    // --- Separator: paint・hit_test ---

    #[test]
    fn separator_paint_is_rect_only() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(30));
        let commands = separator.paint((0.0, 0.0), sized(80.0, 1.0));
        assert_eq!(commands, vec![PaintCommand::Rect]);
    }

    #[test]
    fn separator_hit_test_is_always_false() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(31));
        assert!(!separator.hit_test((5.0, 0.0), (0.0, 0.0), sized(80.0, 1.0)));
    }

    // --- Separator: accessibility_role・clone_box ---

    #[test]
    fn separator_accessibility_role_is_separator() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(32));
        assert_eq!(separator.accessibility_role(), AccessibilityRole::Separator);
    }

    #[test]
    fn separator_clone_box_produces_equivalent_widget() {
        let separator: Separator<ProfileState> = Separator::new(WidgetId::from_raw(33))
            .with_orientation(SeparatorOrientation::Vertical)
            .with_thickness(5.0);
        let cloned = separator.clone_box();
        assert_eq!(cloned.id(), separator.id());
        assert_eq!(
            cloned.paint((0.0, 0.0), sized(80.0, 20.0)),
            separator.paint((0.0, 0.0), sized(80.0, 20.0))
        );
    }

    #[test]
    fn separator_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Separator<ProfileState>>();
    }
}
