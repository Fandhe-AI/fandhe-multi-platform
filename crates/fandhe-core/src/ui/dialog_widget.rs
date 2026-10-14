//! ダイアログ・モーダルウィジェットの状態・イベント処理（TASK-8.3-S47 / REQ-8、Issue #142）。
//!
//! `ui::Dialog`（TASK-8.3-S46 / Issue #141）とは別に、
//! [`super::scroll_view_widget::ScrollViewWidget`] / [`super::icon_widget::IconWidget`]と
//! 同じ判断で、`Widget<S>` を実装する自己完結のダイアログウィジェット型
//! [`DialogWidget`] として実装する。
//!
//! 型名・ファイル名は `dialog::Dialog` との衝突を避けるため
//! `DialogWidget` / `dialog_widget.rs` とする（`scroll_view_widget::ScrollViewWidget` /
//! `icon_widget::IconWidget` と同じ命名判断）。`dialog::Dialog` との型統合は行わない
//! （`.claude/rules/out-of-scope-tracking.md` 対象）。
//!
//! `measure` / `paint` は `Dialog` の正式な基本描画実装がスコープであるため、
//! 本型では状態・イベント処理を単体で検証可能にするための最小の暫定実装
//! （[`super::checkbox::CheckboxWidget`] と同様、`Dialog` と揃えた定数・
//! パネル寸法近似のみ）を提供する。
//!
//! ## 状態・イベント処理
//!
//! - **意味づけ済み入力 [`DialogInput`]**: Esc（[`DialogInput::Dismiss`]）・閉じるボタンの
//!   確定（[`DialogInput::ConfirmClose`]）・フォーカス移動（[`DialogInput::FocusNext`] /
//!   [`DialogInput::FocusPrev`]）の意味づけ済み列挙のみで構成し、生キーコード・生文字列を
//!   一切持たない（`CheckboxInput` / `ScrollViewInput` と同型の OWASP A03 対策、
//!   `.claude/rules/security.md` 準拠）。
//! - **開閉状態の SSOT はアプリ状態 `S`**: [`Self::new_dynamic`] が受け取る
//!   `open_source: ReadFn<S, bool>` 経由の [`Widget::sync_from_state`] のみが表示キャッシュ
//!   `cached_open` を更新する（`Dialog`（S46）・`CheckboxWidget` と同じ制御型の設計判断）。
//! - **書き込み経路 `set: WriteFn<S, bool>`**: 構築時に必須で受け取り [`Self::write_fn`] で
//!   公開する（`Checkbox::write_fn` / `ToggleSwitch::write_fn` と同型のアクセサ）。
//!   `Dialog`（S46）はまだこの書き込み経路を持たないため、本型が単独で保持する
//!   （S48 での型統合時に突き合わせる、モジュール doc 冒頭参照）。SSOT はあくまで
//!   アプリ状態 `S` 側（REQ-13、環境キャプチャ不可の関数ポインタに限定）。
//! - **dismiss 確定は `Event::ButtonClicked` を返す**（`Event` enum への専用バリアント
//!   追加はスコープ外。`CheckboxWidget` / `ToggleSwitch` の legacy button bridge と同じ
//!   運用）。宣言層はレガシー `.button(label, handler)` で受け、`StateStore::dispatch` を
//!   経由して `S` へ反映後、`sync_from_state` で新値が還流する想定。実際の `S` への書き込み
//!   （`write_fn` 経由）はテストで直接呼んで検証する（`write_fn` 自体を `apply_pointer` /
//!   `handle_input` から自動配線しない設計判断、上記と同じ理由）。ブリッジキーは
//!   タイトルから派生させず、専用フィールド [`Self::dismiss_label`] として明示的に保持する
//!   （宣言層の `.button(label, ...)` キーと確実に一致させるための意図的な分離）。
//! - **背景（スクリム）クリックによる dismiss は座標が要る**: [`InteractionInput`] は
//!   ポインタ出入り・押下・解放・フォーカス移動のみで座標を持たないため、
//!   `apply_input(InteractionInput)` 単体では「スクリム上での press→release」を判定
//!   できない（`hovered` はダイアログ全体（modal 時は画面全域）への出入りを示すのみで
//!   パネル内クリックと区別できない）。[`ScrollViewWidget`][super::scroll_view_widget::ScrollViewWidget]
//!   が座標を持つ専用入力型を導入したのと同じ判断で、本型は座標解決済みの領域
//!   [`DialogHitRegion`]（[`Self::hit_region`] が算出）を明示的な引数として受け取る
//!   [`Self::apply_pointer`] を提供する。押下時の領域を `press_region` として記録し、
//!   解放時の領域と両方が `Scrim` かつ `modal` の場合、または両方が `CloseButton`
//!   の場合（modal 可否を問わず、閉じるボタンは常時有効）に dismiss を発火する
//!   （`CheckboxWidget::apply_input` の「press-and-drag-away では発火しない」意味論を、
//!   座標ベースの領域比較で再現。`PointerExited` で `press_region` をクリアする）。
//!   `press_region` は `apply_pointer` / `PointerExited` 以外にも、`sync_from_state`
//!   による開閉クローズ・`set_disabled(true)`・`handle_input` の dismiss 発火（`Dismiss` /
//!   `ConfirmClose`）でも都度クリアする。これらの経路をまたいで直前の `press_region` が
//!   残存すると、後続の `PointerReleased` で意図しない dismiss 再発火や Esc との
//!   二重発火を招くため（バグ修正、PR #389 の指摘対応）。
//! - **`hit_region` の境界**: 非有限（NaN/inf）座標は安全側の [`DialogHitRegion::Outside`]
//!   を返す。パネル・スクリムの寸法自体も [`normalize_non_negative_finite`] で
//!   非有限値・負値を `0.0` へ丸めるため、不正なサイズ通知を受けても panic せず
//!   「常に非ヒット」という安全側の結果に収束する（OWASP A04 相当の数値健全性対策、
//!   `scroll_view_widget::normalize_non_negative_finite` と同じ丸め方針）。
//! - **フォーカストラップ**: `focused_index: Option<usize>` にダイアログ内フォーカス位置を
//!   保持する（`InteractionState::focused` とは直交するウィジェットローカルな一時 UI 状態、
//!   `radio_group::RadioGroupWidget` の roving focus と同じ整理）。フォーカス対象スロットは
//!   `[閉じるボタン]`（常時）＋（[`Self::with_content_slot`] で有効化した場合のみ）
//!   `[コンテンツ]` の最小構成とし、`FocusNext` / `FocusPrev` はラップアラウンドで巡回して
//!   ダイアログ外へフォーカスを逃がさない。`sync_from_state` が開閉遷移（false→true）を
//!   検知したら初期フォーカス（閉じるボタン、スロット `0`）へリセットし、true→false で
//!   `None` へクリアする。
//! - **閉状態・disabled 中はすべての入力を無視**（安全側の判断、`CheckboxWidget` /
//!   `ScrollViewWidget` と同じ）。`hit_test` も `!cached_open` の間は常に `false` を返し、
//!   modal 全域捕捉のジオメトリが閉じたダイアログの背景ヒットを奪わないようにする。
//!
//! ## `Widget<S>` 実装
//!
//! - `measure` / `paint` / `hit_test` は `Dialog`（S46）の確定寸法契約（modal 時は
//!   scrim 全域、非modal 時はパネル矩形のみ）を暫定的に踏襲する（定数値も揃えている）。
//!   ただし本型は子コンテンツ（`Box<dyn Widget<S>>`）を保持せず、
//!   [`Self::with_content_slot`] は「フォーカス巡回対象にコンテンツスロットを含めるか」の
//!   真偽値のみを表す（実際の子ウィジェット合成は S48 以降、`Dialog`（S46）との型統合の
//!   スコープ）。
//! - `sync_from_state`: `open_source` が `Some` の場合のみ `cached_open` を更新する
//!   （`CheckboxWidget::sync_from_state` と同型。`None` の場合は静的初期値のまま no-op）。
//! - `children`（Issue #739）と `content_nodes`（PR #792 codex-review 指摘 A）:
//!   [`Self::with_content`] で設定した `content` スロットは開状態のときだけ
//!   `content(state)` を返す `Node<S>` 列である。表示用に常時この列を取得したい
//!   呼び出し（egui-proxy 側の描画）は [`Self::content_nodes`] を使う。一方
//!   `Widget::children` は走査ツリー（`Node::expand_children` /
//!   `Screen::visit_node_expanded` / `ui::event::resolve_node_at`）双方の入口であり、
//!   ここが子を返すとイベント解決（`resolve_node_at` 経由のクリック判定）も子へ
//!   降下してしまう。そのため `children` は `interaction.disabled` の間は常に空を
//!   返し、`content_nodes` へは委譲しない（「disabled 中はすべての入力を無視する」
//!   契約を子ノードの入力にも及ぼすための実装。子の表示自体は `content_nodes` 経由で
//!   維持される想定 — egui-proxy が `add_enabled_ui(false)` 等で描画するのは表示側の
//!   責務）。上記の [`Self::has_content_slot`]（フォーカス巡回対象に含めるかの
//!   真偽値のみ）とは別物であり、両者は独立して設定できる。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。検証は `Widget`
//! メソッド・[`Self::apply_pointer`] / [`Self::handle_input`] の直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) `WidgetCatalog` 登録・テスト網羅拡充（S48 / #143）、
//! (b) reconcile パス（`&mut` を `Node::Widget` へ届ける経路）未配線の既知契約、
//! (c) プラットフォームレイヤでの実キーコード → `DialogInput` 変換・
//! アクセシビリティ連携（フォーカス可視化）、
//! (d) `Event::ButtonClicked` 専用バリアント化・`WidgetId` ベースディスパッチ統合、
//! (e) アニメーション・`AlertDialog` ロール分岐（S46 記録済み）、
//! (f) S46（#141）マージ後の `dialog::Dialog` との型統合（実子コンテンツ合成を含む）。

use crate::state::{Event, ReadFn, WriteFn};

use super::node::Node;
use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行の高さ近似（論理ピクセル相当）。`dialog::Dialog`（S46）の
/// `APPROX_LINE_HEIGHT` と同じ値を採用し、測定近似の基準を揃える（S48 での型統合を
/// 見据えた判断。cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数）。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// パネル（ダイアログ本体）の固定幅プレースホルダ（論理ピクセル相当）。
/// `dialog::Dialog`（S46）の `PANEL_WIDTH` と同じ値。
const PANEL_WIDTH: f32 = 320.0;

/// タイトル・本文の周囲に確保する余白（論理ピクセル相当）。
/// `dialog::Dialog`（S46）の `PANEL_PADDING` と同じ値。
///
/// TASK-8.4-S15（#171）: `dialog::Dialog` と同じ理由でスペーシングトークンの
/// 既定スケール（`spacing.md` = 16px）から導出するよう差し替えた。値は不変。
const PANEL_PADDING: f32 = crate::token::spacing_scale::MD.value();

/// パネル右上に配置する閉じるボタン領域の一辺（論理ピクセル相当のプレースホルダ）。
/// [`DialogWidget::hit_region`] が `DialogHitRegion::CloseButton` を判定する際の
/// 矩形サイズとして使う。
const CLOSE_BUTTON_SIZE: f32 = 24.0;

/// フォーカス巡回スロットのうち、常時存在する「閉じるボタン」のスロット番号。
const CLOSE_BUTTON_SLOT: usize = 0;

/// 非有限値（`NaN`/無限大）または負値を `0.0` へ丸める。
///
/// [`super::scroll_view_widget`] の同名ヘルパーと同じ丸め方針を採用し、パネル・
/// スクリムの寸法算出で非有限値・負値の伝播を防ぐ（OWASP A04 相当の数値健全性対策）。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// [`DialogWidget`] へのキーボード入力を表す列挙（本 issue の核心）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`CheckboxInput` /
/// `ScrollViewInput` と同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogInput {
    /// Esc キー相当の dismiss。開状態でのみ有効（フォーカス位置は問わない）。
    Dismiss,
    /// 閉じるボタンの確定（Space/Enter 相当）。フォーカストラップが閉じるボタン
    /// スロットを指している間のみ有効。
    ConfirmClose,
    /// Tab 相当。ダイアログ内フォーカスリングを次スロットへラップアラウンドで進める。
    FocusNext,
    /// Shift+Tab 相当。ダイアログ内フォーカスリングを前スロットへラップアラウンドで戻す。
    FocusPrev,
}

/// ポインタ座標がダイアログのどの領域上にあるかを表す列挙（[`DialogWidget::hit_region`]
/// が算出する）。
///
/// [`DialogWidget::apply_pointer`] が背景（スクリム）クリックによる dismiss を
/// 判定するための座標解決結果であり、生の `(f32, f32)` 座標そのものを
/// イベント配線へ持ち込まない（モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogHitRegion {
    /// modal 時にパネル外を覆う背景（スクリム）領域。
    Scrim,
    /// パネル内（閉じるボタン領域を除く）。
    Panel,
    /// パネル右上の閉じるボタン領域。
    CloseButton,
    /// ダイアログの外側、または閉状態・非有限座標等により判定不能。
    Outside,
}

/// ダイアログ・モーダルウィジェット（TASK-8.3-S47 で新設する自己完結の状態・
/// イベント処理型）。
pub struct DialogWidget<S> {
    id: WidgetId,
    title: String,
    body: String,
    /// `Event::ButtonClicked` へのブリッジに使う専用ラベル（タイトルから派生させない
    /// 理由はモジュール doc 参照）。
    dismiss_label: String,
    /// モーダル（背景操作ブロック・スクリム経由の dismiss 対象）か非モーダルかの区分。
    modal: bool,
    /// アプリ状態 `S` から開閉状態を読み取るための関数ポインタ（`None` の場合は
    /// 静的な初期値のみを保持し、`sync_from_state` は no-op になる。`CheckboxWidget`
    /// の `checked_source` と同型）。
    open_source: Option<ReadFn<S, bool>>,
    /// 開閉結果を状態 `S` へ反映する関数ポインタ。[`Self::write_fn`] で公開する
    /// （モジュール doc 参照）。
    set: WriteFn<S, bool>,
    /// 直近の `sync_from_state`（または構築時の初期値）で確定した開閉状態。
    cached_open: bool,
    /// フォーカス巡回にコンテンツスロットを含めるか（[`Self::with_content_slot`]）。
    has_content_slot: bool,
    /// ダイアログ内フォーカス位置（`None` はダイアログが閉じている、またはまだ
    /// 一度も開いていないことを表す）。
    focused_index: Option<usize>,
    /// 直近の `PointerPressed` が発生した領域（`apply_pointer` の press-and-drag-away
    /// 判定に使う。モジュール doc 参照）。
    press_region: Option<DialogHitRegion>,
    /// hover/pressed/disabled/focus。
    interaction: InteractionState,
    /// 開状態のときに走査ツリーへ実際に差し込む子ノード列（[`Self::with_content`]）。
    /// `None` は子なし（[`Widget::children`] 参照、Issue #739）。
    content: Option<ReadFn<S, Vec<Node<S>>>>,
}

impl<S> std::fmt::Debug for DialogWidget<S> {
    /// `checkbox::CheckboxWidget` と同じ理由（derive が `S: Debug` の不要な境界を
    /// 課すのを避けるため）で手動実装する。title / body / dismiss_label は UI 表示文字列
    /// であり秘密情報に該当しないため、`Dialog`（S46）と同様 `Debug` へそのまま含める。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogWidget")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("body", &self.body)
            .field("dismiss_label", &self.dismiss_label)
            .field("modal", &self.modal)
            .field("cached_open", &self.cached_open)
            .field("has_content_slot", &self.has_content_slot)
            .field("focused_index", &self.focused_index)
            .field("press_region", &self.press_region)
            .field("interaction", &self.interaction)
            .field("has_open_source", &self.open_source.is_some())
            .field("has_content", &self.content.is_some())
            .finish()
    }
}

impl<S> Clone for DialogWidget<S> {
    /// `checkbox::CheckboxWidget` と同じ理由で手動実装する（関数ポインタは `Copy` であり
    /// 複製にコストがかからない）。
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            title: self.title.clone(),
            body: self.body.clone(),
            dismiss_label: self.dismiss_label.clone(),
            modal: self.modal,
            open_source: self.open_source,
            set: self.set,
            cached_open: self.cached_open,
            has_content_slot: self.has_content_slot,
            focused_index: self.focused_index,
            press_region: self.press_region,
            interaction: self.interaction,
            content: self.content,
        }
    }
}

impl<S> DialogWidget<S> {
    /// 静的な初期値のみを持つダイアログを構築する（`sync_from_state` は no-op）。
    /// 既定はモーダル・コンテンツスロットなし（[`Self::with_modal`] /
    /// [`Self::with_content_slot`] で変更可能）。
    pub fn new_static(
        id: WidgetId,
        title: impl Into<String>,
        body: impl Into<String>,
        dismiss_label: impl Into<String>,
        initial_open: bool,
        set: WriteFn<S, bool>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            body: body.into(),
            dismiss_label: dismiss_label.into(),
            modal: true,
            open_source: None,
            set,
            cached_open: initial_open,
            has_content_slot: false,
            focused_index: if initial_open {
                Some(CLOSE_BUTTON_SLOT)
            } else {
                None
            },
            press_region: None,
            interaction: InteractionState::default(),
            content: None,
        }
    }

    /// 状態 `S` から動的に開閉状態を読み取るダイアログを構築する。
    ///
    /// `initial_open` は最初の `sync_from_state` 呼び出し前に `is_open()` /
    /// `measure()` / `paint()` が参照する初期表示値（`CheckboxWidget::new_dynamic` と
    /// 同じ保険）。
    pub fn new_dynamic(
        id: WidgetId,
        title: impl Into<String>,
        body: impl Into<String>,
        dismiss_label: impl Into<String>,
        initial_open: bool,
        open_source: ReadFn<S, bool>,
        set: WriteFn<S, bool>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            body: body.into(),
            dismiss_label: dismiss_label.into(),
            modal: true,
            open_source: Some(open_source),
            set,
            cached_open: initial_open,
            has_content_slot: false,
            focused_index: if initial_open {
                Some(CLOSE_BUTTON_SLOT)
            } else {
                None
            },
            press_region: None,
            interaction: InteractionState::default(),
            content: None,
        }
    }

    /// モーダル / 非モーダルを設定するビルダーメソッド（既定は `true`）。
    pub fn with_modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// フォーカス巡回にコンテンツスロットを含めるかを設定するビルダーメソッド
    /// （既定は `false`。実際の子ウィジェット合成は S48 以降のスコープ、モジュール doc
    /// 参照）。
    pub fn with_content_slot(mut self, has_content_slot: bool) -> Self {
        self.has_content_slot = has_content_slot;
        self
    }

    /// 開状態のときに走査ツリーへ差し込む子ノード列を設定するビルダーメソッド
    /// （既定は `None` = 子なし、Issue #739）。[`Widget::children`] の実装本体は
    /// [`Self::has_content_slot`]（フォーカス巡回対象かどうかの真偽値）とは独立に
    /// 動作する。
    pub fn with_content(mut self, content: ReadFn<S, Vec<Node<S>>>) -> Self {
        self.content = Some(content);
        self
    }

    /// 表示文言（タイトル）を返す。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 表示文言（本文）を返す。
    pub fn body(&self) -> &str {
        &self.body
    }

    /// `Event::ButtonClicked` の発火対象を特定する dismiss ラベルを返す。
    pub fn dismiss_label(&self) -> &str {
        &self.dismiss_label
    }

    /// モーダルかどうかを返す。
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// `cached_open`（直近の [`Widget::sync_from_state`] が読み取った開閉状態）を返す。
    pub fn is_open(&self) -> bool {
        self.cached_open
    }

    /// フォーカス巡回にコンテンツスロットを含めるかどうかを返す。
    pub fn has_content_slot(&self) -> bool {
        self.has_content_slot
    }

    /// ダイアログ内フォーカス位置を返す（`None` は閉状態）。
    pub fn focused_index(&self) -> Option<usize> {
        self.focused_index
    }

    /// イベント配線が使う書き込み経路を返す（モジュール doc 参照）。
    pub fn write_fn(&self) -> WriteFn<S, bool> {
        self.set
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// 表示用に子ノード列を返す（PR #792 codex-review 指摘 A）。`disabled` は見ない
    /// （表示は維持し、egui-proxy が `add_enabled_ui(false)` 等で disabled 見た目を
    /// 描く用途）。開閉判定は [`Widget::children`] と同じ基準（`open_source` があれば
    /// それを直接呼び、なければ `cached_open`）。閉状態・`content` 未設定なら空。
    pub fn content_nodes(&self, state: &S) -> Vec<Node<S>> {
        let is_open = match self.open_source {
            Some(read) => read(state),
            None => self.cached_open,
        };
        if !is_open {
            return Vec::new();
        }
        match self.content {
            Some(content) => content(state),
            None => Vec::new(),
        }
    }

    /// disabled フラグを切り替える。`disabled` へ遷移する際は直前の `press_region` も
    /// クリアする（`apply_pointer` / `PointerExited` と同じ理由。モジュール doc の
    /// 「スコープ外事項」参照）。
    pub fn set_disabled(&mut self, disabled: bool) {
        if disabled {
            self.press_region = None;
        }
        self.interaction.set_disabled(disabled);
    }

    /// タイトル・本文の行数から近似したパネル高さ（論理ピクセル相当）。
    /// `dialog::Dialog::approx_panel_height`（S46）と同じ算出式。
    fn approx_panel_height(&self) -> f32 {
        APPROX_LINE_HEIGHT * 2.0 + PANEL_PADDING * 2.0
    }

    /// パネル（ダイアログ本体、scrim を含まない）矩形を `size` へクランプして返す
    /// （`measure` の非 modal 分岐・`hit_test` の非 modal 分岐・`hit_region` が共有する
    /// 単一の計算根拠。`dialog::Dialog::panel_size`（S46）と同じ設計判断）。
    fn panel_size(&self, size: Measurement) -> Measurement {
        let size = Measurement {
            width: normalize_non_negative_finite(size.width),
            height: normalize_non_negative_finite(size.height),
        };
        Measurement {
            width: PANEL_WIDTH.min(size.width),
            height: self.approx_panel_height().min(size.height),
        }
    }

    /// フォーカス巡回のスロット総数（閉じるボタン + コンテンツスロット有無）。
    fn focus_slot_count(&self) -> usize {
        if self.has_content_slot { 2 } else { 1 }
    }

    /// フォーカスリングを `delta` スロット分ラップアラウンドで進める（負値は逆方向）。
    /// 閉状態（`focused_index` が `None`）では no-op とする（`sync_from_state` の
    /// 開閉遷移リセットに委ねる、モジュール doc 参照）。
    fn advance_focus(&mut self, delta: i32) {
        let Some(current) = self.focused_index else {
            return;
        };
        let slot_count = self.focus_slot_count() as i32;
        let next = (current as i32 + delta).rem_euclid(slot_count);
        self.focused_index = Some(next as usize);
    }

    /// ポインタ座標がダイアログのどの領域上にあるかを解決する（モジュール doc 参照）。
    ///
    /// 非有限座標・閉状態は安全側の [`DialogHitRegion::Outside`] を返す。
    /// パネル・スクリムの寸法は [`Self::panel_size`] / [`normalize_non_negative_finite`]
    /// により非有限値・負値を `0.0` へ丸めた上で判定するため、不正なサイズ通知を
    /// 受けても panic せず「常に非ヒット」に収束する。
    pub fn hit_region(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> DialogHitRegion {
        if !self.cached_open {
            return DialogHitRegion::Outside;
        }
        let (px, py) = point;
        if !px.is_finite() || !py.is_finite() {
            return DialogHitRegion::Outside;
        }

        let panel = self.panel_size(size);
        let in_panel = px >= origin.0
            && px < origin.0 + panel.width
            && py >= origin.1
            && py < origin.1 + panel.height;

        if in_panel {
            let close_size = CLOSE_BUTTON_SIZE.min(panel.width).min(panel.height);
            let close_origin_x = origin.0 + panel.width - close_size;
            let in_close_button = px >= close_origin_x
                && px < origin.0 + panel.width
                && py >= origin.1
                && py < origin.1 + close_size;

            return if in_close_button {
                DialogHitRegion::CloseButton
            } else {
                DialogHitRegion::Panel
            };
        }

        if self.modal {
            let scrim = Measurement {
                width: normalize_non_negative_finite(size.width),
                height: normalize_non_negative_finite(size.height),
            };
            let in_scrim = px >= origin.0
                && px < origin.0 + scrim.width
                && py >= origin.1
                && py < origin.1 + scrim.height;
            if in_scrim {
                return DialogHitRegion::Scrim;
            }
        }

        DialogHitRegion::Outside
    }

    /// ポインタ入力を、[`Self::hit_region`] が解決した領域とともに適用する。
    ///
    /// dismiss は次の 2 経路のいずれかで発火する（`PointerPressed` 時の領域
    /// （`press_region`）と `PointerReleased` 時の領域の比較。press-and-drag-away
    /// では発火しない）: (1) 背景（スクリム）クリック — 両方 `Scrim` かつ `modal` の
    /// 場合、(2) 閉じるボタンクリック — 両方 `CloseButton` の場合（modal 可否を問わず
    /// 常時有効）。`PointerExited` は `press_region` をクリアする（モジュール doc 参照）。
    /// 閉状態・disabled 中は入力を無視する（`press_region` もクリアして stale な状態を
    /// 残さない）。
    pub fn apply_pointer(
        &mut self,
        input: InteractionInput,
        region: DialogHitRegion,
    ) -> Option<Event> {
        if !self.cached_open || self.interaction.disabled {
            // 閉状態・disabled 中は「すべての入力を無視する」契約（モジュール doc・
            // handle_input と同じ設計判断）のため、interaction.apply へ委譲しない
            // （Bugbot 指摘対応）。以前はここで interaction.apply(input) を呼んで
            // おり、ダイアログが閉じていても pressed 等へ書き込み得た。press_region
            // のクリアのみ行い、pressed/hovered/focused の後始末は close 遷移側
            // （sync_from_state）と set_disabled 側にそれぞれ委ねる。
            self.press_region = None;
            return None;
        }

        let event = match input {
            InteractionInput::PointerPressed => {
                self.press_region = Some(region);
                None
            }
            InteractionInput::PointerReleased => {
                let scrim_dismiss = self.modal
                    && self.press_region == Some(DialogHitRegion::Scrim)
                    && region == DialogHitRegion::Scrim;
                let close_button_dismiss = self.press_region == Some(DialogHitRegion::CloseButton)
                    && region == DialogHitRegion::CloseButton;
                let should_dismiss = scrim_dismiss || close_button_dismiss;
                self.press_region = None;
                if should_dismiss {
                    Some(Event::ButtonClicked {
                        label: self.dismiss_label.clone(),
                    })
                } else {
                    None
                }
            }
            InteractionInput::PointerExited => {
                self.press_region = None;
                None
            }
            _ => None,
        };

        self.interaction.apply(input);
        event
    }

    /// キーボード入力を処理する（本型の核心）。
    ///
    /// 閉状態・disabled 中はすべての入力を無視する（`CheckboxWidget::handle_input` と
    /// 同じ「フォーカス保持中のみ操作可能」ではなく「開状態かつ有効な間のみ操作可能」
    /// という設計判断。Esc は特定フォーカス位置を要求しないため）。
    pub fn handle_input(&mut self, input: DialogInput) -> Option<Event> {
        if !self.cached_open || self.interaction.disabled {
            return None;
        }

        match input {
            DialogInput::Dismiss => {
                // 直前の scrim プレスが残存したまま Esc で dismiss すると、後続の
                // PointerReleased で二重発火し得るため、ここでもクリアする
                // （モジュール doc「スコープ外事項」参照。`sync_from_state` /
                // `set_disabled(true)` と同じ理由）。
                self.press_region = None;
                Some(Event::ButtonClicked {
                    label: self.dismiss_label.clone(),
                })
            }
            DialogInput::ConfirmClose => {
                if self.focused_index == Some(CLOSE_BUTTON_SLOT) {
                    self.press_region = None;
                    Some(Event::ButtonClicked {
                        label: self.dismiss_label.clone(),
                    })
                } else {
                    None
                }
            }
            DialogInput::FocusNext => {
                self.advance_focus(1);
                None
            }
            DialogInput::FocusPrev => {
                self.advance_focus(-1);
                None
            }
        }
    }
}

impl<S: 'static> Widget<S> for DialogWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から開閉状態を読み取り `cached_open` を更新する（reconcile
    /// フェーズ）。開閉遷移（false→true）を検知したら初期フォーカス（閉じるボタン、
    /// スロット `0`）へリセットし、true→false で `focused_index` をクリアする
    /// （モジュール doc「フォーカストラップ」参照）。`open_source` が `None`
    /// （`new_static` 構築）の場合は no-op（`CheckboxWidget::sync_from_state` と同型）。
    fn sync_from_state(&mut self, state: &S) {
        let Some(read) = self.open_source else {
            return;
        };

        let previous_open = self.cached_open;
        self.cached_open = read(state);

        if !previous_open && self.cached_open {
            self.focused_index = Some(CLOSE_BUTTON_SLOT);
        } else if previous_open && !self.cached_open {
            self.focused_index = None;
            self.press_region = None;
            // close 時点で pressed/hovered/focused を残さない（Bugbot 指摘対応）。
            // Esc 等の同期的な close がマウスダウン中に発生すると hit_test が
            // ダイアログ消失中は false を返すため PointerReleased が届かず、
            // pressed が古い状態のまま残り得る。apply_pointer 側の早期リターン
            // （closed/disabled 中は interaction.apply を呼ばない）と対にして、
            // close の瞬間にここで確実にリセットする。
            self.interaction.hovered = false;
            self.interaction.pressed = false;
            self.interaction.focused = false;
        }
    }

    /// 閉状態はゼロ寸法（存在しないものとして扱う、`dialog::Dialog::measure`（S46）と
    /// 同じ契約）。開状態は modal か否かで異なる（`dialog::Dialog` モジュール doc 参照）:
    /// modal は正規化した `available` をそのまま返し、非 modal は [`Self::panel_size`]
    /// を返す。
    fn measure(&self, available: Measurement) -> Measurement {
        if !self.cached_open {
            return Measurement {
                width: 0.0,
                height: 0.0,
            };
        }

        let available = Measurement {
            width: normalize_non_negative_finite(available.width),
            height: normalize_non_negative_finite(available.height),
        };

        if self.modal {
            available
        } else {
            self.panel_size(available)
        }
    }

    /// 閉状態は空 `Vec`。開状態は `[（modal 時のみ）Rect（スクリム）, Rect（パネル）,
    /// Text(title), Text(body)]` を返す（`dialog::Dialog::paint`（S46）と同じ列。
    /// 本型は子コンテンツを保持しないため `content.paint` の連結は行わない、
    /// モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        if !self.cached_open {
            return Vec::new();
        }

        let mut commands = Vec::new();
        if self.modal {
            commands.push(PaintCommand::Rect);
        }
        commands.push(PaintCommand::Rect);
        commands.push(PaintCommand::Text(self.title.clone()));
        commands.push(PaintCommand::Text(self.body.clone()));

        commands
    }

    /// 閉状態は常に `false`（存在しないものとして扱う）。開状態は modal 時 `size` 全域
    /// （スクリムを含む＝背景クリック捕捉）、非 modal 時は [`Self::panel_size`] の矩形の
    /// みを対象とする。`interaction.disabled` の間は常に `false` を返す
    /// （先行ウィジェットと同じ安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、左上を含み
    /// 右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if !self.cached_open || self.interaction.disabled {
            return false;
        }

        let target = if self.modal {
            size
        } else {
            self.panel_size(size)
        };

        point.0 >= origin.0
            && point.0 < origin.0 + target.width
            && point.1 >= origin.1
            && point.1 < origin.1 + target.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Dialog
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// [`Self::with_content`] で設定した子ノード列を、開状態のときだけ返す。
    /// `sync_from_state` を経ない共有参照走査（`render_headless` 等）でも正しい
    /// 開閉状態で判定できるよう、`open_source` が `Some` ならそれを直接呼んで判定し
    /// （`cached_open` の古い値に頼らない）、`None`（`new_static` 構築）の場合のみ
    /// `cached_open` へフォールバックする（Issue #739）。
    ///
    /// `children` は走査（`Node::expand_children`）・イベント解決
    /// （`ui::event::resolve_node_at`）双方の入口であるため、`interaction.disabled`
    /// の間は常に空を返す（PR #792 codex-review 指摘 A：disabled 中に子ハンドラが
    /// クリックで実行できてしまう既存契約の迂回を塞ぐ）。表示のみを維持したい場合は
    /// [`Self::content_nodes`] を使う。
    fn children(&self, state: &S) -> Vec<Node<S>> {
        if self.interaction.disabled {
            return Vec::new();
        }
        self.content_nodes(state)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FormState {
        dialog_open: bool,
        dismissed: bool,
    }

    fn read_dialog_open(state: &FormState) -> bool {
        state.dialog_open
    }

    fn set_dialog_open(state: &mut FormState, value: bool) {
        state.dialog_open = value;
        if !value {
            state.dismissed = true;
        }
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    fn make_static(initial_open: bool) -> DialogWidget<FormState> {
        DialogWidget::new_static(
            WidgetId::from_raw(1),
            "確認",
            "この操作を実行しますか？",
            "confirm-dialog-dismiss",
            initial_open,
            set_dialog_open,
        )
    }

    fn make_dynamic(initial_open: bool) -> DialogWidget<FormState> {
        DialogWidget::new_dynamic(
            WidgetId::from_raw(2),
            "確認",
            "この操作を実行しますか？",
            "confirm-dialog-dismiss",
            initial_open,
            read_dialog_open,
            set_dialog_open,
        )
    }

    // --- (a) 構築と各アクセサ ---

    #[test]
    fn new_static_holds_initial_values() {
        let dialog = make_static(false);

        assert_eq!(dialog.title(), "確認");
        assert_eq!(dialog.body(), "この操作を実行しますか？");
        assert_eq!(dialog.dismiss_label(), "confirm-dialog-dismiss");
        assert!(dialog.is_modal());
        assert!(!dialog.is_open());
        assert!(!dialog.has_content_slot());
        assert_eq!(dialog.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn with_modal_and_with_content_slot_override_defaults() {
        let dialog = make_static(false).with_modal(false).with_content_slot(true);

        assert!(!dialog.is_modal());
        assert!(dialog.has_content_slot());
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let dialog = make_static(true);
        let write_fn = dialog.write_fn();
        let mut state = FormState {
            dialog_open: true,
            dismissed: false,
        };

        write_fn(&mut state, false);

        assert!(!state.dialog_open);
        assert!(state.dismissed);
    }

    // --- (b) sync_from_state・フォーカスリセット ---

    #[test]
    fn sync_from_state_is_no_op_for_static_dialog() {
        let mut dialog = make_static(false);
        let state = FormState {
            dialog_open: true,
            dismissed: false,
        };

        dialog.sync_from_state(&state);

        assert!(!dialog.is_open());
    }

    #[test]
    fn sync_from_state_tracks_state_for_dynamic_dialog() {
        let mut dialog = make_dynamic(false);
        let state = FormState {
            dialog_open: true,
            dismissed: false,
        };

        dialog.sync_from_state(&state);

        assert!(dialog.is_open());
    }

    #[test]
    fn sync_from_state_resets_focus_to_close_button_on_open_transition() {
        let mut dialog = make_dynamic(false).with_content_slot(true);
        let mut state = FormState {
            dialog_open: false,
            dismissed: false,
        };
        dialog.sync_from_state(&state);
        assert_eq!(dialog.focused_index(), None);

        state.dialog_open = true;
        dialog.sync_from_state(&state);

        assert_eq!(dialog.focused_index(), Some(CLOSE_BUTTON_SLOT));
    }

    #[test]
    fn sync_from_state_clears_focus_on_close_transition() {
        let mut dialog = make_dynamic(true);
        let mut state = FormState {
            dialog_open: true,
            dismissed: false,
        };
        dialog.sync_from_state(&state);
        assert_eq!(dialog.focused_index(), Some(CLOSE_BUTTON_SLOT));

        state.dialog_open = false;
        dialog.sync_from_state(&state);

        assert_eq!(dialog.focused_index(), None);
    }

    // --- (c) measure ---

    #[test]
    fn closed_dialog_measures_to_zero() {
        let dialog = make_static(false);
        let measurement = dialog.measure(sized(800.0, 600.0));

        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn open_modal_dialog_measure_returns_normalized_available() {
        let dialog = make_static(true);
        let measurement = dialog.measure(sized(800.0, 600.0));

        assert_eq!(measurement.width, 800.0);
        assert_eq!(measurement.height, 600.0);
    }

    #[test]
    fn open_non_modal_dialog_measure_returns_panel_size() {
        let dialog = make_static(true).with_modal(false);
        let measurement = dialog.measure(sized(800.0, 600.0));

        assert!(measurement.width < 800.0);
        assert!(measurement.height < 600.0);
    }

    #[test]
    fn measure_handles_non_finite_and_negative_available() {
        let dialog = make_static(true);

        let non_finite = dialog.measure(sized(f32::NAN, f32::INFINITY));
        assert_eq!(non_finite.width, 0.0);
        assert!(non_finite.height.is_finite());

        let negative = dialog.measure(sized(-10.0, -10.0));
        assert_eq!(negative.width, 0.0);
        assert_eq!(negative.height, 0.0);
    }

    // --- (d) paint ---

    #[test]
    fn closed_dialog_paints_nothing() {
        let dialog = make_static(false);
        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));

        assert!(commands.is_empty());
    }

    #[test]
    fn open_modal_dialog_paints_scrim_panel_title_and_body() {
        let dialog = make_static(true);
        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("確認".to_string()),
                PaintCommand::Text("この操作を実行しますか？".to_string()),
            ]
        );
    }

    #[test]
    fn open_non_modal_dialog_paints_without_scrim() {
        let dialog = make_static(true).with_modal(false);
        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("確認".to_string()),
                PaintCommand::Text("この操作を実行しますか？".to_string()),
            ]
        );
    }

    // --- (e) hit_test / hit_region ---

    #[test]
    fn closed_dialog_hit_test_is_always_false() {
        let dialog = make_static(false);
        assert!(!dialog.hit_test((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)));
    }

    #[test]
    fn open_modal_dialog_hit_test_covers_full_size() {
        let dialog = make_static(true);
        let size = sized(800.0, 600.0);

        assert!(dialog.hit_test((0.0, 0.0), (0.0, 0.0), size));
        assert!(dialog.hit_test((799.0, 599.0), (0.0, 0.0), size));
        assert!(!dialog.hit_test((800.0, 600.0), (0.0, 0.0), size));
    }

    #[test]
    fn open_non_modal_dialog_hit_test_is_limited_to_panel_rect() {
        let dialog = make_static(true).with_modal(false);
        let size = sized(800.0, 600.0);

        assert!(dialog.hit_test((10.0, 10.0), (0.0, 0.0), size));
        assert!(!dialog.hit_test((700.0, 500.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_open_and_inside() {
        let mut dialog = make_static(true);
        dialog.set_disabled(true);

        assert!(!dialog.hit_test((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)));
    }

    #[test]
    fn hit_region_resolves_scrim_panel_and_close_button() {
        let dialog = make_static(true);
        let size = sized(800.0, 600.0);
        let panel = dialog.panel_size(size);

        // パネル外（右下遠方）はスクリム。
        assert_eq!(
            dialog.hit_region((700.0, 500.0), (0.0, 0.0), size),
            DialogHitRegion::Scrim
        );
        // パネル内・閉じるボタンを除く領域はパネル。
        assert_eq!(
            dialog.hit_region((10.0, 10.0), (0.0, 0.0), size),
            DialogHitRegion::Panel
        );
        // パネル右上の閉じるボタン領域。
        assert_eq!(
            dialog.hit_region((panel.width - 1.0, 1.0), (0.0, 0.0), size),
            DialogHitRegion::CloseButton
        );
    }

    #[test]
    fn hit_region_is_outside_for_non_modal_beyond_panel() {
        let dialog = make_static(true).with_modal(false);
        let size = sized(800.0, 600.0);

        assert_eq!(
            dialog.hit_region((700.0, 500.0), (0.0, 0.0), size),
            DialogHitRegion::Outside
        );
    }

    #[test]
    fn hit_region_is_outside_when_closed() {
        let dialog = make_static(false);
        assert_eq!(
            dialog.hit_region((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)),
            DialogHitRegion::Outside
        );
    }

    #[test]
    fn hit_region_is_outside_for_non_finite_point() {
        let dialog = make_static(true);
        assert_eq!(
            dialog.hit_region((f32::NAN, 10.0), (0.0, 0.0), sized(800.0, 600.0)),
            DialogHitRegion::Outside
        );
        assert_eq!(
            dialog.hit_region((10.0, f32::INFINITY), (0.0, 0.0), sized(800.0, 600.0)),
            DialogHitRegion::Outside
        );
    }

    // --- (f) apply_pointer（背景クリックによる dismiss） ---

    #[test]
    fn scrim_press_and_release_dismisses_modal_dialog() {
        let mut dialog = make_static(true);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "confirm-dialog-dismiss".to_string(),
            })
        );
    }

    #[test]
    fn press_on_scrim_then_release_on_panel_does_not_dismiss() {
        let mut dialog = make_static(true);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Panel);

        assert_eq!(event, None);
    }

    #[test]
    fn press_on_panel_then_release_on_scrim_does_not_dismiss() {
        let mut dialog = make_static(true);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Panel);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(event, None);
    }

    #[test]
    fn pointer_exited_between_press_and_release_cancels_dismiss() {
        let mut dialog = make_static(true);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        dialog.apply_pointer(InteractionInput::PointerExited, DialogHitRegion::Outside);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(event, None);
    }

    #[test]
    fn non_modal_dialog_scrim_region_never_dismisses() {
        let mut dialog = make_static(true).with_modal(false);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(event, None);
    }

    #[test]
    fn close_button_press_and_release_dismisses_modal_dialog() {
        // PR #389 のバグ修正: 閉じるボタンの press→release が dismiss を発火しない
        // 問題があった（scrim 同士の比較のみを見ていた）。
        let mut dialog = make_static(true);

        dialog.apply_pointer(
            InteractionInput::PointerPressed,
            DialogHitRegion::CloseButton,
        );
        let event = dialog.apply_pointer(
            InteractionInput::PointerReleased,
            DialogHitRegion::CloseButton,
        );

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "confirm-dialog-dismiss".to_string(),
            })
        );
    }

    #[test]
    fn close_button_press_and_release_dismisses_non_modal_dialog() {
        // 閉じるボタンは modal 可否を問わず常時有効（モジュール doc 参照）。
        let mut dialog = make_static(true).with_modal(false);

        dialog.apply_pointer(
            InteractionInput::PointerPressed,
            DialogHitRegion::CloseButton,
        );
        let event = dialog.apply_pointer(
            InteractionInput::PointerReleased,
            DialogHitRegion::CloseButton,
        );

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "confirm-dialog-dismiss".to_string(),
            })
        );
    }

    #[test]
    fn press_on_close_button_then_release_on_panel_does_not_dismiss() {
        let mut dialog = make_static(true);

        dialog.apply_pointer(
            InteractionInput::PointerPressed,
            DialogHitRegion::CloseButton,
        );
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Panel);

        assert_eq!(event, None);
    }

    #[test]
    fn stale_press_region_after_close_transition_does_not_leak_into_reopen() {
        // PR #389 のバグ修正: `sync_from_state` によるクローズ時に `press_region` が
        // 残存すると、再度開いた直後の無関係な PointerReleased で誤って dismiss が
        // 発火し得た。
        let mut dialog = make_dynamic(true);
        let mut state = FormState {
            dialog_open: true,
            dismissed: false,
        };
        dialog.sync_from_state(&state);
        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);

        state.dialog_open = false;
        dialog.sync_from_state(&state);
        state.dialog_open = true;
        dialog.sync_from_state(&state);

        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);
        assert_eq!(event, None);
    }

    #[test]
    fn stale_press_region_is_cleared_when_disabled() {
        let mut dialog = make_static(true);
        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);

        dialog.set_disabled(true);
        dialog.set_disabled(false);

        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);
        assert_eq!(event, None);
    }

    #[test]
    fn stale_press_region_is_cleared_after_keyboard_dismiss() {
        // Esc（`DialogInput::Dismiss`）発火後に scrim の `press_region` が残ったままだと、
        // 続く PointerReleased で二重発火し得た。
        let mut dialog = make_static(true);
        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);

        dialog.handle_input(DialogInput::Dismiss);

        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);
        assert_eq!(event, None);
    }

    #[test]
    fn closed_dialog_ignores_pointer_input() {
        let mut dialog = make_static(false);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(event, None);
    }

    #[test]
    fn disabled_dialog_ignores_pointer_input() {
        let mut dialog = make_static(true);
        dialog.set_disabled(true);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Scrim);
        let event = dialog.apply_pointer(InteractionInput::PointerReleased, DialogHitRegion::Scrim);

        assert_eq!(event, None);
    }

    #[test]
    fn closed_dialog_does_not_mutate_interaction_state() {
        // Bugbot 指摘（PR #389）: 以前は closed/disabled 中も interaction.apply が
        // 呼ばれており、存在しないダイアログに pressed 等をセットし得た。
        let mut dialog = make_static(false);

        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Panel);

        assert!(!dialog.interaction_state().pressed);
        assert!(!dialog.interaction_state().hovered);
    }

    #[test]
    fn dismiss_during_press_clears_stale_interaction_state_on_close() {
        // Bugbot 指摘（PR #389）: Esc（同期的な close）がマウスダウン中に発生すると
        // hit_test はダイアログ消失中は false を返すため PointerReleased が届かず、
        // pressed/hovered/focused が古い状態のまま残り得た。close 遷移
        // （sync_from_state）でこれらをリセットすることを確認する。
        let mut dialog = make_dynamic(true);
        let mut state = FormState {
            dialog_open: true,
            dismissed: false,
        };
        dialog.sync_from_state(&state);

        dialog.apply_pointer(InteractionInput::PointerEntered, DialogHitRegion::Panel);
        dialog.apply_pointer(InteractionInput::PointerPressed, DialogHitRegion::Panel);
        dialog.apply_pointer(InteractionInput::FocusGained, DialogHitRegion::Panel);
        assert!(dialog.interaction_state().pressed);
        assert!(dialog.interaction_state().hovered);
        assert!(dialog.interaction_state().focused);

        // Esc 相当の同期的 close。
        state.dialog_open = false;
        dialog.sync_from_state(&state);

        assert!(!dialog.interaction_state().pressed);
        assert!(!dialog.interaction_state().hovered);
        assert!(!dialog.interaction_state().focused);
    }

    // --- (g) handle_input（キーボード） ---

    #[test]
    fn dismiss_confirms_when_open_regardless_of_focus() {
        let mut dialog = make_static(true);

        let event = dialog.handle_input(DialogInput::Dismiss);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "confirm-dialog-dismiss".to_string(),
            })
        );
    }

    #[test]
    fn dismiss_ignored_when_closed() {
        let mut dialog = make_static(false);
        assert_eq!(dialog.handle_input(DialogInput::Dismiss), None);
    }

    #[test]
    fn dismiss_ignored_when_disabled() {
        let mut dialog = make_static(true);
        dialog.set_disabled(true);
        assert_eq!(dialog.handle_input(DialogInput::Dismiss), None);
    }

    #[test]
    fn confirm_close_fires_only_when_focus_is_on_close_button() {
        let mut dialog = make_static(true).with_content_slot(true);
        assert_eq!(dialog.focused_index(), Some(CLOSE_BUTTON_SLOT));

        let event = dialog.handle_input(DialogInput::ConfirmClose);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "confirm-dialog-dismiss".to_string(),
            })
        );

        dialog.handle_input(DialogInput::FocusNext);
        let event_after_move = dialog.handle_input(DialogInput::ConfirmClose);
        assert_eq!(event_after_move, None);
    }

    #[test]
    fn focus_next_and_prev_wrap_around_with_content_slot() {
        let mut dialog = make_static(true).with_content_slot(true);
        assert_eq!(dialog.focused_index(), Some(0));

        dialog.handle_input(DialogInput::FocusNext);
        assert_eq!(dialog.focused_index(), Some(1));

        dialog.handle_input(DialogInput::FocusNext);
        assert_eq!(dialog.focused_index(), Some(0));

        dialog.handle_input(DialogInput::FocusPrev);
        assert_eq!(dialog.focused_index(), Some(1));
    }

    #[test]
    fn focus_next_is_no_op_without_content_slot() {
        let mut dialog = make_static(true);
        assert_eq!(dialog.focused_index(), Some(0));

        dialog.handle_input(DialogInput::FocusNext);
        assert_eq!(dialog.focused_index(), Some(0));

        dialog.handle_input(DialogInput::FocusPrev);
        assert_eq!(dialog.focused_index(), Some(0));
    }

    #[test]
    fn focus_next_ignored_when_closed() {
        let mut dialog = make_static(false);
        dialog.handle_input(DialogInput::FocusNext);
        assert_eq!(dialog.focused_index(), None);
    }

    // --- (h) accessibility_role / clone_box / Debug / StateStore 連携 ---

    #[test]
    fn accessibility_role_is_dialog() {
        let dialog = make_static(true);
        assert_eq!(dialog.accessibility_role(), AccessibilityRole::Dialog);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let dialog = make_static(true).with_content_slot(true);
        let cloned: Box<dyn Widget<FormState>> = dialog.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(1));
        assert_eq!(cloned.accessibility_role(), AccessibilityRole::Dialog);
    }

    #[test]
    fn debug_output_contains_dismiss_label() {
        let dialog = make_static(true);
        let output = format!("{dialog:?}");

        assert!(output.contains("dismiss_label"));
        assert!(output.contains("confirm-dialog-dismiss"));
    }

    #[test]
    fn widget_trait_object_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<FormState>> = Box::new(make_static(true));

        assert_eq!(widget.accessibility_role(), AccessibilityRole::Dialog);
        assert!(!widget.interaction_state().disabled);
    }

    // --- (i) children（Issue #739） ---

    fn expand_content(_state: &FormState) -> Vec<Node<FormState>> {
        fn on_click(state: &mut FormState) {
            state.dismissed = true;
        }
        vec![Node::Button {
            label: "OK".to_string(),
            on_click,
            id: WidgetId::from_raw(99),
        }]
    }

    #[test]
    fn children_is_empty_when_closed() {
        let dialog = make_dynamic(false).with_content(expand_content);
        let state = FormState {
            dialog_open: false,
            dismissed: false,
        };

        assert!(dialog.children(&state).is_empty());
    }

    #[test]
    fn children_returns_content_when_open() {
        let dialog = make_dynamic(true).with_content(expand_content);
        let state = FormState {
            dialog_open: true,
            dismissed: false,
        };

        assert_eq!(dialog.children(&state).len(), 1);
    }

    #[test]
    fn children_reflects_state_without_sync_from_state() {
        // `sync_from_state` を呼ばない共有参照走査（`render_headless` 相当）でも
        // `open_source` から直接開閉を判定できることを確認する（モジュール doc 参照）。
        let dialog = make_dynamic(false).with_content(expand_content);
        let state = FormState {
            dialog_open: true,
            dismissed: false,
        };

        assert_eq!(dialog.children(&state).len(), 1);
    }

    #[test]
    fn children_is_empty_when_disabled_but_content_nodes_still_returns_content() {
        // PR #792 codex-review 指摘 A: disabled 中は children を空にして
        // resolve_node_at 経由の入力解決から子を除外する。表示用の content_nodes は
        // disabled を見ないため、egui-proxy 側で見た目だけ維持できる。
        let mut dialog = make_dynamic(true).with_content(expand_content);
        let state = FormState {
            dialog_open: true,
            dismissed: false,
        };
        dialog.set_disabled(true);

        assert!(dialog.children(&state).is_empty());
        assert_eq!(dialog.content_nodes(&state).len(), 1);
    }

    #[test]
    fn state_store_dispatch_closes_dialog_via_legacy_button_bridge() {
        // 宣言層はレガシー `.button(label, handler)` で受け、`StateStore::dispatch`
        // を経由して状態へ反映後、`sync_from_state` で新値が還流することを確認する
        // （モジュール doc の設計判断の E2E 検証、`CheckboxWidget` の同種テストと同型）。
        use crate::state::StateStore;
        use crate::ui::{Screen, WidgetId};

        fn close_dialog(state: &mut FormState) {
            state.dialog_open = false;
            state.dismissed = true;
        }

        let screen: Screen<FormState> = Screen::new("form").button(
            WidgetId::from_raw(1),
            "confirm-dialog-dismiss",
            close_dialog,
        );
        let mut store = StateStore::new(FormState {
            dialog_open: true,
            dismissed: false,
        });
        let mut dialog = make_dynamic(true);

        let event = dialog
            .handle_input(DialogInput::Dismiss)
            .expect("開状態の Dismiss は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert!(!store.state().dialog_open);
        assert!(store.state().dismissed);

        dialog.sync_from_state(store.state());
        assert!(!dialog.is_open());
        assert_eq!(dialog.focused_index(), None);
    }
}
