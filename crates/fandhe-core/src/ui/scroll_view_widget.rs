//! スクロールビューウィジェットの状態・イベント処理（TASK-8.3-S41 / REQ-8、Issue #136）。
//!
//! [`super::text_field::TextFieldWidget`]（#109）・[`super::image_widget::ImageWidget`]
//! （#130）・[`super::icon_widget::IconWidget`]（#133）と同じ判断（既存 main の
//! 公開 API のみに依存する自己完結実装とし、`scroll_view::ScrollView` との型統合は
//! 別作業に委ねる）を踏襲し、[`Widget<S>`] を実装する自己完結のスクロールビュー
//! ウィジェット型 [`ScrollViewWidget`] として実装する。
//!
//! 型名・ファイル名は `scroll_view::ScrollView` との衝突を避けるため
//! `ScrollViewWidget` / `scroll_view_widget.rs` とする（`image_widget::ImageWidget` /
//! `icon_widget::IconWidget` と同じ命名判断）。`scroll_view::ScrollView` との型統合は
//! 行わない。
//!
//! ## 状態・イベント処理
//!
//! - **状態はウィジェットローカル**: スクロールオフセット・ビューポート/コンテンツ実寸・
//!   ドラッグ状態はアプリ状態 `S` ではなくウィジェット内部状態として保持する
//!   （`state::Event` はスクロール専用バリアントを持たず、`Event::ButtonClicked` /
//!   `Event::TextFieldChanged` の 2 種のみであり、既存全ウィジェットが行っている
//!   ブリッジのみで済ませる判断を踏襲。新設はスコープ外）。そのため
//!   [`Widget::sync_from_state`] は no-op とする（本モジュール §「Widget<S> 実装」参照）。
//! - **クランプの単一情報源**: オフセットの安全域クランプは [`super::scroll::clamp_scroll_offset`]
//!   （TASK-5.5-S5 / Issue #88）へ委譲し、二重実装しない（既存 SSOT の再利用）。
//!   非有限値（`NaN`/無限大）・負値は同ヘルパーの防御ロジックにより安全側（`0.0` または
//!   `[0, max_offset]` 内）へ丸まる。
//! - **意味づけ済み入力 [`ScrollViewInput`]**: ホイール・トラックパッド（`ScrollBy`）、
//!   プログラム的スクロール（`ScrollTo`）、キーボード相当（`LineUp`/`LineDown`/`PageUp`/
//!   `PageDown`/`Home`/`End`）、ドラッグパン（`DragStarted`/`DragMoved`/`DragEnded`）の
//!   意味づけ済み列挙のみで構成し、生キーコード・生文字列を一切持たない
//!   （`ToggleSwitchInput` / `CheckboxInput` と同じ OWASP A03 対策の設計判断）。
//! - **行送り・ページ送り量はビューポート比**: `LineUp`/`LineDown` はビューポート実寸の
//!   [`LINE_STEP_RATIO`] 倍、`PageUp`/`PageDown` は [`PAGE_STEP_RATIO`] 倍を増減する
//!   （ビューポート未通知時は [`DEFAULT_LINE_STEP`] / [`DEFAULT_PAGE_STEP`] へ
//!   フォールバック）。ページ送りを 100% ではなく [`PAGE_STEP_RATIO`]（0.9）にする理由は、
//!   前ページとの視覚的な連続性（コンテキストの一部を残す）を保つ一般的な UX 慣行に従うため。
//! - **`handle_input` は `bool` を返す**（`Option<Event>` ではない）: スクロールはアプリ状態
//!   `S` を変更しないため、既存の `Event` ブリッジ系ウィジェット（`ToggleSwitchInput` 等）と
//!   異なり、オフセットが変化したかどうか（再描画要否判定用）のみを返す設計とする。
//! - **`apply_input`**: hover/press/focus の反映は [`InteractionState::apply`] へそのまま
//!   委譲する（`disabled` 中は同メソッド内部で全入力を無視する契約に従う）。
//! - **`set_viewport_extent` / `set_content_extent`**: プラットフォームレイヤからの実寸通知
//!   API（`ImageWidget::notify_*` と同じ「プラットフォームレイヤ通知」パターン）。
//!   非有限値・負値は `0.0` へ正規化し、更新のたびに必ず [`super::scroll::clamp_scroll_offset`]
//!   でオフセットを再クランプする（コンテンツ縮小時にオフセットがはみ出したままになる
//!   不整合を防ぐ）。
//! - **ドラッグの意味論**: `DragStarted` は基準位置・基準オフセットを記録するのみでオフセット
//!   自体は変えない（`false` を返す）。`DragMoved` は「コンテンツがポインタに追従する」
//!   （タッチパン）の意味論とし、ポインタが正方向へ動く（コンテンツを下へ引く操作）ほど
//!   オフセットは減る（`origin_offset + (origin_position - position)`）。`drag` が `None`
//!   （`DragStarted` を経ていない）の場面での `DragMoved` は安全側に無視して `false` を返す
//!   （開始通知なしの移動通知という不整合な呼び出し列を拒否する、`ImageWidget` の
//!   ロード状態機械が採る遷移制約と同じ設計判断）。`DragEnded` は `drag` を解除するのみ。
//!   `ScrollBy`/`ScrollTo`/`LineUp`/`LineDown`/`PageUp`/`PageDown`/`Home`/`End`
//!   （非ドラッグ経路）と [`Self::set_viewport_extent`]/[`Self::set_content_extent`]
//!   （実寸変更に伴う再クランプ）もいずれも `drag` を必ずクリアする。これらの経路で
//!   `offset` が `origin_offset` と無関係に変わり得るため、クリアしないと次の
//!   `DragMoved` が古い基準位置から再計算してビューポートがジャンプ/固着し得る
//!   （stale drag origin、本 issue で修正）。
//! - **disabled との一体性**: `handle_input` は `interaction.disabled` の間、全入力を無視して
//!   `false` を返す（スクロールはユーザー対話の一種であり、`ImageWidget` のロード通知
//!   （対話ではないため disabled と直交）とは異なる）。[`Self::set_disabled(true)`]
//!   （`Self::set_disabled`）は `drag` も併せてクリアする。disabled 中は `handle_input` が
//!   `DragEnded` を含む全入力を無視するため、クリアしないとドラッグ中に disable した際の
//!   stale なドラッグ状態が残り、再度 enable した時に意図せず古い基準からドラッグが
//!   再開してしまう（disable leaves drag active、本 issue で修正）。
//! - **横スクロールは未対応**: 垂直スクロールオフセットのみを最小スコープとする
//!   （`scroll::clamp_scroll_offset` も単一軸の契約）。横スクロールは需要が生じた時点の
//!   別作業とする（`.claude/rules/out-of-scope-tracking.md` 対象）。
//!
//! ## `Widget<S>` 実装
//!
//! - `sync_from_state`: no-op（上記「状態はウィジェットローカル」参照）。
//! - `measure`: 利用可能な領域をそのまま占有する（スクロールコンテナは与えられた領域を
//!   埋める、という一般的なコンテナ挙動。子コンテンツの計測配線は S42 以降のスコープ）。
//! - `paint`: `[Rect（ビューポート枠）]` を基本とし、コンテンツがビューポートを超えて
//!   いる場合（[`ScrollViewWidget::overflows`]）のみ `[Rect（スクロールバートラック）,
//!   Rect（サム）]` を追加する。`PaintCommand` は `Text(String)` / `Rect` の 2 種のまま
//!   拡張しない（`ToggleSwitch` 等と同じ判断、3 バックエンドのプレースホルダ契約に
//!   波及するため安全側に倒す）。`PaintCommand::Rect` 自体は位置・寸法を持たない列挙子
//!   であり、サムの実際の位置・長さ（オフセット/`max_offset` 比・ビューポート/コンテンツ比）
//!   はプラットフォームレイヤが [`ScrollViewWidget::offset`] / [`ScrollViewWidget::max_offset`]
//!   / [`ScrollViewWidget::viewport_extent`] / [`ScrollViewWidget::content_extent`] を
//!   参照して算出する契約とする。
//! - `accessibility_role`: TASK-8.3-S40（#135）で新設済みの
//!   [`AccessibilityRole::ScrollView`] を返す（`scroll_view::ScrollView` と同じロールを
//!   共用し、`List`（`ListView` が使用）とは区別する）。
//! - `hit_test`: インタラクティブ要素（トラック/サムのドラッグ・フォーカス取得の対象）
//!   として矩形内判定を行う（`Checkbox::hit_test` と同じ半開区間規約）。`disabled` 中は
//!   常に `false`。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。検証は `Widget`
//! メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) S40（#135）マージ後の型統合（`scroll_view::ScrollView` との突き合わせ）、
//! (b) プラットフォームレイヤへの実配線（`fandhe-egui-proxy` の `egui::ScrollArea` 統合）、
//! (c) `WidgetCatalog` 登録・結合テスト網羅拡充（S42 / #137）、
//! (d) 横スクロール・慣性スクロール・複数子コンテンツ、
//! (e) 描画クリッピング（`PaintCommand` 拡張は 3 バックエンド契約に波及するため見送り）、
//! (f) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。

use super::scroll::clamp_scroll_offset;
use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// ビューポート実寸が未通知（`0.0`）の場合に `LineUp` / `LineDown` が使う既定の行送り量
/// （論理ピクセル相当のプレースホルダ定数）。
const DEFAULT_LINE_STEP: f32 = 20.0;

/// ビューポート実寸が未通知（`0.0`）の場合に `PageUp` / `PageDown` が使う既定のページ送り量。
const DEFAULT_PAGE_STEP: f32 = 200.0;

/// `LineUp` / `LineDown` の行送り量を導出する、ビューポート実寸に対する比率。
const LINE_STEP_RATIO: f32 = 0.1;

/// `PageUp` / `PageDown` のページ送り量を導出する、ビューポート実寸に対する比率。
/// `1.0` ではなく `0.9` とする理由はモジュール doc 参照（前ページとの視覚的連続性）。
const PAGE_STEP_RATIO: f32 = 0.9;

/// 非有限値（`NaN`/無限大）または負値を `0.0` へ丸める。
///
/// [`super::scroll::clamp_scroll_offset`] 内部の同名ロジックはモジュール非公開のため
/// 再利用できず、本モジュール独自のガードとして複製する（クランプの中核アルゴリズム
/// 自体は複製せず [`clamp_scroll_offset`] へ委譲する、モジュール doc 参照）。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// 非有限値（`NaN`/無限大）を `0.0` へ丸める（符号は保持する）。
///
/// `ScrollBy` の `delta` や `DragMoved` の `position` は負値も正当な入力（逆方向スクロール・
/// 負座標系のポインタ位置）であるため、[`normalize_non_negative_finite`] とは異なり
/// 符号を落とさない。
fn normalize_finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// [`ScrollViewWidget`] への意味づけ済み入力（本 issue の核心）。
///
/// 生キーコード・生文字列を一切持たない列挙のみで構成し、OWASP A03（インジェクション）
/// 対策を型レベルで担保する（`ToggleSwitchInput` / `CheckboxInput` と同型の設計判断、
/// `.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollViewInput {
    /// ホイール・トラックパッド相当の相対スクロール（正方向は下方向スクロールとする）。
    ScrollBy {
        /// 加算するオフセット量（論理ピクセル相当）。非有限値は `0.0` として扱う。
        delta: f32,
    },
    /// プログラム的スクロール（絶対オフセット指定）。
    ScrollTo {
        /// 設定先の絶対オフセット。非有限値は `0.0` として扱う（範囲外は安全域へクランプ）。
        offset: f32,
    },
    /// キーボードの上方向行送り相当。
    LineUp,
    /// キーボードの下方向行送り相当。
    LineDown,
    /// キーボードのページ上送り相当。
    PageUp,
    /// キーボードのページ下送り相当。
    PageDown,
    /// 先頭（オフセット `0.0`）へ移動する。
    Home,
    /// 末尾（`max_offset`）へ移動する。
    End,
    /// ドラッグパン開始（基準位置を記録する）。
    DragStarted {
        /// ドラッグ開始時のポインタ位置（論理ピクセル相当の任意座標系）。
        position: f32,
    },
    /// ドラッグパン中の移動。
    DragMoved {
        /// 現在のポインタ位置（`DragStarted` と同じ座標系）。
        position: f32,
    },
    /// ドラッグパン終了。
    DragEnded,
}

/// ドラッグパン中の基準状態（本 issue の核心、内部実装詳細）。
///
/// `DragStarted` 時点のポインタ位置・オフセットを記録し、以降の `DragMoved` が
/// 累積誤差なく絶対座標から追従量を算出できるようにする（相対量の逐次加算にすると
/// 浮動小数点誤差が蓄積するため、基準点方式を採る）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct DragState {
    /// `DragStarted` 時点のポインタ位置。
    origin_position: f32,
    /// `DragStarted` 時点のオフセット。
    origin_offset: f32,
}

/// スクロールビューウィジェット（TASK-8.3-S41、`Widget<S>` 実装型、状態・イベント処理）。
///
/// `Debug` / `Clone` は手動実装する（状態型 `S` を保持しないため `derive` マクロが
/// 自動付与する `S: Debug` / `S: Clone` という不要な境界を避ける、`ImageWidget` と
/// 同じ判断）。オフセット・実寸・ドラッグ状態は秘密情報に該当しないため `Debug` に
/// そのまま含める。
pub struct ScrollViewWidget<S> {
    id: WidgetId,
    /// 垂直スクロールオフセット（ウィジェットローカル状態、モジュール doc 参照）。
    offset: f32,
    /// コンテンツの総実寸（プラットフォームレイヤから [`Self::set_content_extent`] で通知）。
    content_extent: f32,
    /// ビューポートの実寸（プラットフォームレイヤから [`Self::set_viewport_extent`] で通知）。
    viewport_extent: f32,
    /// ドラッグパン中の基準状態（非ドラッグ中は `None`）。
    drag: Option<DragState>,
    interaction: InteractionState,
    /// `S` を所有しないため `PhantomData` でジェネリック引数を保持する
    /// （`Widget<S>` を実装するために型パラメータ自体は必要、`ImageWidget` 同様のパターンだが
    /// 本ウィジェットは `S` に触れるフィールドを一切持たないため明示的に `PhantomData` を置く）。
    _state: std::marker::PhantomData<fn(&S)>,
}

impl<S> std::fmt::Debug for ScrollViewWidget<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollViewWidget")
            .field("id", &self.id)
            .field("offset", &self.offset)
            .field("content_extent", &self.content_extent)
            .field("viewport_extent", &self.viewport_extent)
            .field("drag", &self.drag)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for ScrollViewWidget<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            offset: self.offset,
            content_extent: self.content_extent,
            viewport_extent: self.viewport_extent,
            drag: self.drag,
            interaction: self.interaction,
            _state: std::marker::PhantomData,
        }
    }
}

impl<S> ScrollViewWidget<S> {
    /// 新規スクロールビューを構築する（オフセット `0.0`・実寸未通知の初期状態）。
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            offset: 0.0,
            content_extent: 0.0,
            viewport_extent: 0.0,
            drag: None,
            interaction: InteractionState::default(),
            _state: std::marker::PhantomData,
        }
    }

    /// 現在のスクロールオフセットを返す。
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// 現在のコンテンツ総実寸を返す。
    pub fn content_extent(&self) -> f32 {
        self.content_extent
    }

    /// 現在のビューポート実寸を返す。
    pub fn viewport_extent(&self) -> f32 {
        self.viewport_extent
    }

    /// 現在の実寸から求まる最大オフセット（`[0, max_offset]` の上限）を返す。
    ///
    /// [`super::scroll::clamp_scroll_offset`] と同じ導出式
    /// （`(content_extent - viewport_extent).max(0.0)`）を用いる。
    pub fn max_offset(&self) -> f32 {
        (self.content_extent - self.viewport_extent).max(0.0)
    }

    /// コンテンツがビューポートを超えているか（スクロールバー描画要否の判定に使う）。
    pub fn overflows(&self) -> bool {
        self.max_offset() > 0.0
    }

    /// プラットフォームレイヤからビューポート実寸を通知する。
    ///
    /// 非有限値・負値は `0.0` へ正規化し、その後 [`super::scroll::clamp_scroll_offset`]
    /// でオフセットを再クランプする（縮小時のはみ出し防止、モジュール doc 参照）。
    pub fn set_viewport_extent(&mut self, viewport_extent: f32) {
        self.viewport_extent = normalize_non_negative_finite(viewport_extent);
        self.reclamp();
    }

    /// プラットフォームレイヤからコンテンツ総実寸を通知する。
    ///
    /// [`Self::set_viewport_extent`] と同じ正規化・再クランプを行う。
    pub fn set_content_extent(&mut self, content_extent: f32) {
        self.content_extent = normalize_non_negative_finite(content_extent);
        self.reclamp();
    }

    /// 現在の `offset` を現在の実寸に対して再クランプする（内部ヘルパー）。
    ///
    /// 実寸変更は `offset` を `DragMoved` の基準（`origin_offset`）と無関係に動かし得るため、
    /// ここで `drag` を必ずクリアする。クリアしない場合、次の `DragMoved` が
    /// `origin_offset` に旧基準を適用し続け、ビューポートがジャンプ/固着し得る
    /// （Bugbot 指摘: stale drag origin after scroll）。
    fn reclamp(&mut self) {
        self.offset = clamp_scroll_offset(self.offset, self.content_extent, self.viewport_extent);
        self.drag = None;
    }

    /// 行送り量（[`LINE_STEP_RATIO`] 倍、ビューポート未通知時は [`DEFAULT_LINE_STEP`]）。
    fn line_step(&self) -> f32 {
        if self.viewport_extent > 0.0 {
            self.viewport_extent * LINE_STEP_RATIO
        } else {
            DEFAULT_LINE_STEP
        }
    }

    /// ページ送り量（[`PAGE_STEP_RATIO`] 倍、ビューポート未通知時は [`DEFAULT_PAGE_STEP`]）。
    fn page_step(&self) -> f32 {
        if self.viewport_extent > 0.0 {
            self.viewport_extent * PAGE_STEP_RATIO
        } else {
            DEFAULT_PAGE_STEP
        }
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// [`InteractionState::set_disabled`] は hover/press/focus をクリアするが、
    /// `drag`（ウィジェットローカル状態）は関知しないため、ここで明示的にクリアする。
    /// `disabled` 中は [`Self::handle_input`] が `DragEnded` を含む全入力を無視するため、
    /// クリアしないと stale なドラッグ基準が残り、再度 enable した際に途中からの
    /// `DragMoved` が古い基準位置で不整合なオフセットへ飛ぶ可能性がある
    /// （Bugbot 指摘: disable leaves drag active）。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.drag = None;
        }
    }

    /// hover/press/focus の入力を反映する（[`InteractionState::apply`] への委譲）。
    ///
    /// `disabled` 中は同メソッド内部で全入力が無視される契約に従う。
    pub fn apply_input(&mut self, input: InteractionInput) {
        self.interaction.apply(input);
    }

    /// [`ScrollViewInput`] を適用し、オフセットが変化したかを返す（本 issue の核心 API）。
    ///
    /// `disabled` 中は全入力を無視して `false` を返す（モジュール doc 参照、スクロールは
    /// ユーザー対話の一種であるため）。オフセット変更を伴わない入力（`DragStarted` /
    /// `DragEnded` / 不整合な `DragMoved`）は常に `false` を返す。
    pub fn handle_input(&mut self, input: ScrollViewInput) -> bool {
        if self.interaction.disabled {
            return false;
        }

        let previous_offset = self.offset;

        match input {
            ScrollViewInput::ScrollBy { delta } => {
                // wheel/トラックパッド起点の非ドラッグ経路。`drag` 基準（`origin_offset`）が
                // ここでの `offset` 変化と無関係に stale 化するため、次の `DragMoved` が
                // 古い基準を適用しないようクリアする
                // （Bugbot 指摘: stale drag origin after scroll、モジュール doc 参照）。
                self.drag = None;
                let delta = normalize_finite(delta);
                self.offset = clamp_scroll_offset(
                    self.offset + delta,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::ScrollTo { offset } => {
                self.drag = None;
                let offset = normalize_finite(offset);
                self.offset =
                    clamp_scroll_offset(offset, self.content_extent, self.viewport_extent);
            }
            ScrollViewInput::LineUp => {
                self.drag = None;
                let step = self.line_step();
                self.offset = clamp_scroll_offset(
                    self.offset - step,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::LineDown => {
                self.drag = None;
                let step = self.line_step();
                self.offset = clamp_scroll_offset(
                    self.offset + step,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::PageUp => {
                self.drag = None;
                let step = self.page_step();
                self.offset = clamp_scroll_offset(
                    self.offset - step,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::PageDown => {
                self.drag = None;
                let step = self.page_step();
                self.offset = clamp_scroll_offset(
                    self.offset + step,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::Home => {
                self.drag = None;
                self.offset = clamp_scroll_offset(0.0, self.content_extent, self.viewport_extent);
            }
            ScrollViewInput::End => {
                self.drag = None;
                // `content_extent` を渡せば `clamp_scroll_offset` が `max_offset` 以下へ
                // 落とし込むため、`f32::MAX` 等の特別値を使わずに済む
                // （`super::scroll` の既存クランプ契約をそのまま利用する）。
                self.offset = clamp_scroll_offset(
                    self.content_extent,
                    self.content_extent,
                    self.viewport_extent,
                );
            }
            ScrollViewInput::DragStarted { position } => {
                let position = normalize_finite(position);
                self.drag = Some(DragState {
                    origin_position: position,
                    origin_offset: self.offset,
                });
                // 基準位置の記録のみでオフセットは変えない。
                return false;
            }
            ScrollViewInput::DragMoved { position } => {
                let Some(drag) = self.drag else {
                    // 開始通知なしの移動通知は不整合な呼び出し列として無視する
                    // （`ImageWidget` の遷移制約と同じ設計判断、モジュール doc 参照）。
                    return false;
                };
                let position = normalize_finite(position);
                let new_offset = drag.origin_offset + (drag.origin_position - position);
                self.offset =
                    clamp_scroll_offset(new_offset, self.content_extent, self.viewport_extent);
            }
            ScrollViewInput::DragEnded => {
                self.drag = None;
                return false;
            }
        }

        self.offset != previous_offset
    }
}

impl<S: 'static> Widget<S> for ScrollViewWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// no-op（モジュール doc §「状態はウィジェットローカル」参照。オフセットはアプリ状態
    /// `S` の SSOT ではないため、reconcile フェーズで読み取るべき値がない）。
    fn sync_from_state(&mut self, _state: &S) {}

    /// 利用可能な領域をそのまま占有する（コンテナとしての一般的な計測挙動。子コンテンツの
    /// 計測配線は S42 以降のスコープ、モジュール doc 参照）。
    fn measure(&self, available: Measurement) -> Measurement {
        available
    }

    /// `[Rect（ビューポート枠）]` を基本とし、オーバーフロー時のみスクロールバー
    /// （トラック・サム）の `Rect` を追加する（モジュール doc §「Widget<S> 実装」参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];
        if self.overflows() {
            commands.push(PaintCommand::Rect); // トラック
            commands.push(PaintCommand::Rect); // サム
        }
        commands
    }

    /// インタラクティブ要素として矩形内判定を行う（`Checkbox::hit_test` と同じ半開区間
    /// 規約 `[origin, origin + size)`）。`disabled` 中は常に `false`。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.interaction.disabled {
            return false;
        }
        point.0 >= origin.0
            && point.0 < origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height
    }

    /// `scroll_view::ScrollView` と同じ [`AccessibilityRole::ScrollView`] を返す
    /// （モジュール doc 参照。`List` は `ListView` が使用するロールのため流用しない）。
    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ScrollView
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

    struct DummyState;

    fn new_widget() -> ScrollViewWidget<DummyState> {
        let mut widget = ScrollViewWidget::new(WidgetId::from_raw(1));
        widget.set_content_extent(1000.0);
        widget.set_viewport_extent(200.0);
        widget
    }

    // (a) 構築・実寸通知・max_offset/overflows

    #[test]
    fn new_widget_starts_at_zero_offset() {
        let widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(1));
        assert_eq!(widget.offset(), 0.0);
        assert_eq!(widget.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn max_offset_is_content_minus_viewport() {
        let widget = new_widget();
        assert_eq!(widget.max_offset(), 800.0);
        assert!(widget.overflows());
    }

    #[test]
    fn overflows_is_false_when_viewport_covers_content() {
        let mut widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(2));
        widget.set_content_extent(100.0);
        widget.set_viewport_extent(500.0);
        assert_eq!(widget.max_offset(), 0.0);
        assert!(!widget.overflows());
    }

    #[test]
    fn set_content_extent_reclamps_offset_when_content_shrinks() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 800.0 });
        assert_eq!(widget.offset(), 800.0);

        // コンテンツが縮小し、以前のオフセットがはみ出す状況を模擬する。
        widget.set_content_extent(300.0);

        assert_eq!(widget.max_offset(), 100.0);
        assert_eq!(widget.offset(), 100.0);
    }

    #[test]
    fn set_viewport_extent_reclamps_offset_when_viewport_grows() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 800.0 });

        // ビューポートが拡大し、以前のオフセットがはみ出す状況を模擬する。
        widget.set_viewport_extent(600.0);

        assert_eq!(widget.max_offset(), 400.0);
        assert_eq!(widget.offset(), 400.0);
    }

    #[test]
    fn extent_setters_normalize_non_finite_and_negative_values() {
        let mut widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(3));
        widget.set_content_extent(f32::NAN);
        widget.set_viewport_extent(-10.0);
        assert_eq!(widget.content_extent(), 0.0);
        assert_eq!(widget.viewport_extent(), 0.0);
    }

    // (b) ScrollBy / ScrollTo

    #[test]
    fn scroll_by_accumulates_offset() {
        let mut widget = new_widget();
        let changed = widget.handle_input(ScrollViewInput::ScrollBy { delta: 50.0 });
        assert!(changed);
        assert_eq!(widget.offset(), 50.0);

        widget.handle_input(ScrollViewInput::ScrollBy { delta: 30.0 });
        assert_eq!(widget.offset(), 80.0);
    }

    #[test]
    fn scroll_by_clamps_at_top_and_bottom() {
        let mut widget = new_widget();
        let changed = widget.handle_input(ScrollViewInput::ScrollBy { delta: -50.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);

        widget.handle_input(ScrollViewInput::ScrollBy { delta: 10_000.0 });
        assert_eq!(widget.offset(), 800.0);
    }

    #[test]
    fn scroll_to_clamps_out_of_range_offset() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: -100.0 });
        assert_eq!(widget.offset(), 0.0);

        widget.handle_input(ScrollViewInput::ScrollTo { offset: 5_000.0 });
        assert_eq!(widget.offset(), 800.0);
    }

    #[test]
    fn scroll_to_same_offset_returns_false() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 100.0 });
        let changed = widget.handle_input(ScrollViewInput::ScrollTo { offset: 100.0 });
        assert!(!changed);
    }

    // (c) LineUp/LineDown/PageUp/PageDown/Home/End

    #[test]
    fn line_down_then_line_up_returns_to_start() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::LineDown);
        assert_eq!(widget.offset(), 200.0 * LINE_STEP_RATIO);

        widget.handle_input(ScrollViewInput::LineUp);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn page_down_then_page_up_returns_to_start() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::PageDown);
        assert_eq!(widget.offset(), 200.0 * PAGE_STEP_RATIO);

        widget.handle_input(ScrollViewInput::PageUp);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn line_and_page_steps_fall_back_to_defaults_without_viewport() {
        let mut widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(4));
        widget.set_content_extent(10_000.0);
        // viewport_extent は未通知（0.0）のまま。

        widget.handle_input(ScrollViewInput::LineDown);
        assert_eq!(widget.offset(), DEFAULT_LINE_STEP);

        widget.handle_input(ScrollViewInput::PageDown);
        assert_eq!(widget.offset(), DEFAULT_LINE_STEP + DEFAULT_PAGE_STEP);
    }

    #[test]
    fn home_moves_to_zero_and_end_moves_to_max_offset() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::End);
        assert_eq!(widget.offset(), widget.max_offset());
        assert_eq!(widget.offset(), 800.0);

        let changed = widget.handle_input(ScrollViewInput::Home);
        assert!(changed);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn home_at_start_returns_false() {
        let mut widget = new_widget();
        let changed = widget.handle_input(ScrollViewInput::Home);
        assert!(!changed);
    }

    // (d) ドラッグパン

    #[test]
    fn drag_started_does_not_change_offset() {
        let mut widget = new_widget();
        let changed = widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn drag_moved_follows_pointer_direction() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });

        // ポインタが下方向（正方向）へ動く = コンテンツを引き下げる操作 = オフセットは減る。
        // 開始時オフセットが 0 のため、下限クランプで 0 のまま変化なしとなることを確認する。
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 150.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);

        // ポインタが上方向（負方向）へ動く = コンテンツを押し上げる操作 = オフセットは増える。
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 50.0 });
        assert!(changed);
        assert_eq!(widget.offset(), 50.0);
    }

    #[test]
    fn drag_moved_uses_origin_offset_as_base_not_cumulative_delta() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 300.0 });
        widget.handle_input(ScrollViewInput::DragStarted { position: 500.0 });

        widget.handle_input(ScrollViewInput::DragMoved { position: 480.0 });
        assert_eq!(widget.offset(), 320.0);

        // 基準点方式のため、同じ position への複数回の適用でも結果は一致する
        // （相対量の累積加算であれば値がずれてしまうケース）。
        widget.handle_input(ScrollViewInput::DragMoved { position: 480.0 });
        assert_eq!(widget.offset(), 320.0);
    }

    #[test]
    fn drag_moved_without_drag_started_is_ignored() {
        let mut widget = new_widget();
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 50.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn set_disabled_true_clears_active_drag() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });

        widget.set_disabled(true);
        widget.set_disabled(false);

        // disable 中に drag がクリアされていれば、開始通知なしの移動通知として無視される。
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 50.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn scroll_to_between_drag_events_clears_stale_drag_origin() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });

        // ドラッグ中に wheel/keyboard/ScrollTo 相当の非ドラッグ経路が割り込むと、
        // 古いドラッグ基準（origin_offset/origin_position）は無効になる。
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 400.0 });

        // クリアされていれば、開始通知なしの移動通知として無視され offset は変わらない。
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 90.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 400.0);
    }

    #[test]
    fn extent_change_between_drag_events_clears_stale_drag_origin() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });

        // extent の再クランプ（wheel/keyboard 以外の経路）でも drag はクリアされる。
        widget.set_content_extent(2000.0);

        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 50.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);
    }

    #[test]
    fn drag_ended_clears_drag_state_and_does_not_change_offset() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::DragStarted { position: 100.0 });
        let changed = widget.handle_input(ScrollViewInput::DragEnded);
        assert!(!changed);

        // ドラッグ終了後の DragMoved は開始通知なしとして無視される。
        let changed = widget.handle_input(ScrollViewInput::DragMoved { position: 50.0 });
        assert!(!changed);
        assert_eq!(widget.offset(), 0.0);
    }

    // (e) disabled

    #[test]
    fn disabled_ignores_all_scroll_view_inputs() {
        let mut widget = new_widget();
        widget.set_disabled(true);

        let inputs = [
            ScrollViewInput::ScrollBy { delta: 50.0 },
            ScrollViewInput::ScrollTo { offset: 100.0 },
            ScrollViewInput::LineDown,
            ScrollViewInput::LineUp,
            ScrollViewInput::PageDown,
            ScrollViewInput::PageUp,
            ScrollViewInput::Home,
            ScrollViewInput::End,
            ScrollViewInput::DragStarted { position: 0.0 },
            ScrollViewInput::DragMoved { position: 10.0 },
            ScrollViewInput::DragEnded,
        ];
        for input in inputs {
            assert!(!widget.handle_input(input));
        }
        assert_eq!(widget.offset(), 0.0);
    }

    // (f) 数値健全性（NaN/±∞/負値の総当たり）

    #[test]
    fn handle_input_never_panics_across_non_finite_values() {
        let mut widget = new_widget();
        let values = [
            0.0,
            -100.0,
            10_000.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];

        for &value in &values {
            widget.handle_input(ScrollViewInput::ScrollBy { delta: value });
            assert!(widget.offset().is_finite());
            assert!(widget.offset() >= 0.0);

            widget.handle_input(ScrollViewInput::ScrollTo { offset: value });
            assert!(widget.offset().is_finite());
            assert!(widget.offset() >= 0.0);

            widget.handle_input(ScrollViewInput::DragStarted { position: value });
            widget.handle_input(ScrollViewInput::DragMoved { position: value });
            assert!(widget.offset().is_finite());
            assert!(widget.offset() >= 0.0);
            widget.handle_input(ScrollViewInput::DragEnded);
        }
    }

    #[test]
    fn extent_setters_never_produce_non_finite_max_offset() {
        let mut widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(5));
        let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0];

        for &value in &values {
            widget.set_content_extent(value);
            widget.set_viewport_extent(value);
            assert!(widget.max_offset().is_finite());
            assert!(widget.offset().is_finite());
            assert!(widget.offset() >= 0.0);
        }
    }

    // (g) apply_input / visual_state

    #[test]
    fn apply_input_updates_hover_and_visual_state() {
        let mut widget = new_widget();
        assert_eq!(widget.visual_state(), VisualState::Idle);

        widget.apply_input(InteractionInput::PointerEntered);
        assert_eq!(widget.visual_state(), VisualState::Hovered);
    }

    #[test]
    fn apply_input_ignored_while_disabled() {
        let mut widget = new_widget();
        widget.set_disabled(true);
        widget.apply_input(InteractionInput::PointerEntered);
        assert_eq!(widget.visual_state(), VisualState::Disabled);
    }

    // (h) paint / hit_test / accessibility_role

    #[test]
    fn paint_omits_scrollbar_when_not_overflowing() {
        let mut widget: ScrollViewWidget<DummyState> = ScrollViewWidget::new(WidgetId::from_raw(6));
        widget.set_content_extent(100.0);
        widget.set_viewport_extent(500.0);

        let commands = widget.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(commands, vec![PaintCommand::Rect]);
    }

    #[test]
    fn paint_includes_scrollbar_rects_when_overflowing() {
        let widget = new_widget();
        let commands = widget.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 100.0,
            },
        );
        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Rect, PaintCommand::Rect]
        );
    }

    #[test]
    fn hit_test_true_inside_rect_false_outside_and_when_disabled() {
        let mut widget = new_widget();
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(widget.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!widget.hit_test((500.0, 500.0), (0.0, 0.0), size));

        widget.set_disabled(true);
        assert!(!widget.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_scroll_view() {
        let widget = new_widget();
        assert_eq!(widget.accessibility_role(), AccessibilityRole::ScrollView);
    }

    #[test]
    fn measure_returns_available_space() {
        let widget = new_widget();
        let available = Measurement {
            width: 320.0,
            height: 480.0,
        };
        assert_eq!(widget.measure(available), available);
    }

    // (i) sync_from_state no-op / clone_box / debug

    #[test]
    fn sync_from_state_is_noop() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 123.0 });
        widget.sync_from_state(&DummyState);
        assert_eq!(widget.offset(), 123.0);
    }

    #[test]
    fn clone_box_preserves_offset_independently() {
        let mut widget = new_widget();
        widget.handle_input(ScrollViewInput::ScrollTo { offset: 400.0 });

        let mut cloned = widget.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        // クローン先だけ変更し、元へ波及しないことで独立コピーであることを確認する。
        cloned.sync_from_state(&DummyState);
        let _ = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 10.0,
                height: 10.0,
            },
        );

        assert_eq!(widget.offset(), 400.0);
    }

    #[test]
    fn debug_output_does_not_panic() {
        let widget = new_widget();
        let output = format!("{widget:?}");
        assert!(output.contains("ScrollViewWidget"));
    }

    #[test]
    fn scroll_view_widget_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ScrollViewWidget<DummyState>>();
    }
}
