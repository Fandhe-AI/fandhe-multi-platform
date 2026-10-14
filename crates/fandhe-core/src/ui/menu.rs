//! メニュー・ドロップダウンウィジェット `Menu<S>`（TASK-8.3-S49 / REQ-8、Issue #144）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の分解
//! sub-issue（49/60）。メニュー・ドロップダウンの API 設計・基本描画を担い、直近の
//! 前例である [`super::dialog::Dialog`]（TASK-8.3-S46 / #141）を主参照として
//! 設計判断を流用する。
//!
//! - 制御型の状態バインディング（[`super::checkbox::Checkbox`] S19 / [`super::slider::Slider`]
//!   S28 / [`super::dialog::Dialog`] S46）: 開閉状態の SSOT はアプリ状態 `S` に置き、
//!   [`crate::state::ReadFn`] 経由の [`Widget::sync_from_state`] のみが表示キャッシュ
//!   `cached_open` を更新する。
//! - 選択肢一覧をグループ単位で保持するモデル（[`super::radio_group::RadioGroupWidget`]
//!   S23）: 選択中項目の SSOT もアプリ状態 `S`（`Option<usize>`）に置き、範囲外
//!   インデックスは表示時に安全側で `None` 相当へフォールバックする（`clamp_index` と
//!   同じ根拠、panic させない）。
//!
//! - `measure` / `paint` / `hit_test` の 3 箇所は、トリガー矩形・ポップアップ矩形の
//!   計算を [`Self::trigger_size`] / [`Self::popup_size`] へ集約して共有する。
//!   **`measure` の返す確定寸法と `hit_test` の対象領域を必ず一致させる**（`Dialog`
//!   が PR #386 Bugbot 指摘で修正された整合性契約と同型。以前の実装が 3 箇所で
//!   別々に矩形を計算していると、どこか 1 箇所だけ変更が漏れて `measure` の返す
//!   寸法と `hit_test` の対象領域が食い違う回帰を招くため、ここへ集約する）。
//! - 閉状態: トリガー矩形のみを計測・描画・ヒットテストの対象とする（オーバーレイの
//!   `Dialog` とは異なり、メニューは閉状態でも「トリガーボタン」が常に存在する UI
//!   のため、`Dialog` の「閉状態はゼロ寸法」という扱いはそのまま踏襲しない）。
//! - 開状態: トリガー + ポップアップ（項目数 × [`APPROX_LINE_HEIGHT`] + パディング）を
//!   計測・描画・ヒットテストの対象とする。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま。3 バックエンドの
//!   プレースホルダ契約への波及を避ける安全側判断、`ScrollView` / `Dialog` と同じ）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! ## 状態・イベント処理（TASK-8.3-S50 / Issue #145）
//!
//! `ToggleSwitch`（S26）/ `ListView`（S17）と同じ判断で自己完結の別型を新設せず、
//! **既存の [`Menu<S>`] へ直接メソッド・型を追加する**。開閉・選択の確定は
//! [`super::dialog_widget::DialogWidget`]（S47）の `hit_region` / `apply_pointer` /
//! `handle_input` パターン（座標解決済み領域を明示引数として受け取る）と、
//! [`super::radio_group::RadioGroupWidget`]（S23）の press/release 同一項目確定・
//! roving focus パターンを組み合わせて実装する。
//!
//! - **座標解決 [`MenuHitRegion`] / [`Self::hit_region`]**: [`Self::trigger_size`] /
//!   [`Self::popup_size`] / [`Self::open_total_size`]（S49 が集約した単一の計算根拠）を
//!   再利用し、`measure` / `hit_test` との整合性契約を維持する。項目行の解決は
//!   `RadioGroupWidget::option_at` と同じ「相対 y ÷ [`APPROX_LINE_HEIGHT`]」の行近似 +
//!   [`clamp_selected`] による範囲外 `None`（＝ `Popup` 扱い）。非有限座標・disabled 中・
//!   閉状態でのポップアップ領域は安全側で `Outside` を返す（`DialogWidget::hit_region` と
//!   同じ判断）。
//! - **ポインタ確定 [`Self::apply_pointer`]**: `press_region` に press 時の領域を記録し、
//!   press 時と release 時の領域が一致した場合のみ確定する（press-and-drag-away
//!   非発火、`DialogWidget` / `RadioGroupWidget` と同じ意味論）。
//!   - `Trigger` + `Trigger` → 開閉トグル（`Event::ButtonClicked { label: self.label }`）。
//!     宣言層はレガシー `.button(label, toggle_handler)` で受け、`StateStore::dispatch` →
//!     `sync_from_state` で開閉が還流する制御型の設計。
//!   - 開状態で `Item(i)` + `Item(i)`（同一 index）→ 選択確定
//!     （`Event::ButtonClicked { label: self.items[i] }`）。`focused_item` も `i` へ
//!     同期する（`RadioGroupWidget::release` の「Pointer select skips focus update」
//!     対応と同じ理由）。
//!   - 開状態で `Outside` + `Outside` → 外側クリック閉じ（`Event::ButtonClicked { label:
//!     self.label }`）。
//!   - `PointerExited` は `press_region` をクリアする。disabled 中・閉状態は全入力を
//!     無視し `press_region` もクリアする（stale な状態を残さない、`DialogWidget` と
//!     同じ判断）。
//! - **キーボード操作 [`MenuInput`] / [`Self::handle_input`]**: 生キーコード・文字列を
//!   一切扱わない意味づけ済み列挙のみで構成する（`RadioGroupInput` / `DialogInput` と
//!   同じ OWASP A03 対策、`.claude/rules/security.md` 準拠）。`focused`
//!   （[`InteractionState::focused`]）かつ非 disabled のときのみ受け付ける。
//!   `FocusNext` / `FocusPrev` はラップアラウンドで `focused_item` を移動するのみで
//!   selection-follows-focus に**しない**（メニューは Enter で確定する WAI-ARIA
//!   menu/listbox の慣例に従う。selection-follows-focus とする `RadioGroupWidget` とは
//!   異なる設計判断）。`Select` は `focused_item` の選択を確定する。空 `items` では
//!   常に `None` を返し panic させない。
//! - **`sync_from_state` の開閉遷移リセット**: 開→閉遷移で `focused_item` /
//!   `press_region` をクリアし、閉→開遷移で `focused_item` を
//!   `cached_selected.unwrap_or(0)` へ初期化する（`DialogWidget::sync_from_state` の
//!   遷移リセットと `RadioGroupWidget::apply_input(FocusGained)` の初期位置決定を
//!   組み合わせた設計）。`set_disabled(true)` でも `press_region` / `focused_item` を
//!   クリアする（`RadioGroupWidget::set_disabled` と同じ後始末）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッド・[`Self::apply_pointer`] / [`Self::handle_input`]
//! の直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) ~~開閉・選択イベント配線~~（S50 / #145 で解消。上記「状態・イベント処理」参照）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S51 / #146）、
//! (c) サブメニュー（ネスト）・セパレータ付き項目・disabled 項目、
//! (d) ポップアップの描画クリッピング・z-order（`PaintCommand` 拡張見送りのため。
//! `ScrollView` / `Dialog` と同じ判断）、
//! (e) 状態束縛の動的項目リスト（`ReadIndexedFn` 経由）・長大リストのポップアップ内
//! スクロール、
//! (f) reconcile パス未配線の既知契約（`widget.rs` モジュール doc の契約を変更しない）、
//! (g) `items` が空の場合でもポップアップ用の余白（[`MENU_PADDING`]）を予約し、
//! 開状態で空のポップアップ `Rect` を描画する（プレースホルダ実装の既知の制約。
//! 項目 0 件時の特別扱いは対象外）、
//! (h) `Event` 専用バリアント化（`Event::ButtonClicked` へのブリッジのまま）・
//! `WidgetId` ベースディスパッチ統合、
//! (i) ディスパッチャ層での「ウィジェット外座標のイベント配送」実配線（`hit_test` が
//! `false` を返す外側クリックを widget まで届ける経路は S5 系の汎用ディスパッチ基盤の
//! スコープ）。

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};
use crate::state::{Event, ReadFn};

/// 単一行の高さ近似（論理ピクセル相当）。[`super::dialog::Dialog`] /
/// [`super::slider::Slider`] / [`super::checkbox::Checkbox`] の `APPROX_LINE_HEIGHT` と
/// 同じ値を採用し、測定近似の基準を揃える（cosmic-text 等の実テキスト計測エンジン
/// 導入前のプレースホルダ定数、折り返し・複数行はスコープ外）。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// トリガー（ドロップダウンボタン）の固定幅プレースホルダ（論理ピクセル相当）。
/// [`super::dialog::Dialog`] の `PANEL_WIDTH`（= 320.0）に準じた値を採用する。
const TRIGGER_WIDTH: f32 = 320.0;

/// トリガー・ポップアップの周囲に確保する余白（論理ピクセル相当）。
const MENU_PADDING: f32 = 16.0;

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// `available` の数値健全性確保のため（OWASP A04 相当）、[`super::dialog`] /
/// [`super::scroll_view`] の同名ヘルパーと同じ丸め方針を採用する。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// 範囲外インデックスを安全側で `None` にクランプする。
///
/// `items.len()` 以上のインデックスは無効値として扱い、panic させない
/// （[`super::radio_group::clamp_index`] と同じ根拠、`.claude/rules/coding-rust.md`
/// のエラーハンドリング方針）。
fn clamp_selected(selected: Option<usize>, len: usize) -> Option<usize> {
    selected.filter(|&i| i < len)
}

/// ポインタ座標がメニューのどの領域上にあるかを表す列挙（[`Menu::hit_region`] が
/// 算出する）。
///
/// [`Menu::apply_pointer`] が開閉トグル・項目選択・外側クリック閉じを判定するための
/// 座標解決結果であり、生の `(f32, f32)` 座標そのものをイベント配線へ持ち込まない
/// （モジュール doc「状態・イベント処理」参照、`DialogHitRegion` と同型の設計）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuHitRegion {
    /// トリガー（ドロップダウンボタン）矩形上。
    Trigger,
    /// 開状態のポップアップ内の項目行（[`clamp_selected`] で範囲内に収まる index）。
    Item(usize),
    /// ポップアップ内だが項目行に該当しない余白（パディング等）。
    Popup,
    /// 上記いずれでもない（外側クリック閉じの判定対象）。
    Outside,
}

/// [`Menu<S>`] へのキーボード入力を表す列挙。
///
/// 外部由来の生キーコード・文字列を一切扱わず、意味づけ済みの列挙のみで構成する
/// ことで OWASP A03（インジェクション）対策を型レベルで担保する（`RadioGroupInput` /
/// `DialogInput` と同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuInput {
    /// Space/Enter 相当。開閉トグルイベントを発火する。
    ActivateTrigger,
    /// Esc 相当。開状態のみ close イベントを発火する。
    Dismiss,
    /// 下矢印相当。開状態のみ roving focus をラップアラウンドで次項目へ移動する
    /// （selection-follows-focus に**しない**、モジュール doc 参照）。
    FocusNext,
    /// 上矢印相当。開状態のみ roving focus をラップアラウンドで前項目へ移動する。
    FocusPrev,
    /// Enter/Space 相当。`focused_item` の選択イベントを発火する。
    Select,
}

/// メニュー・ドロップダウンウィジェット（TASK-8.3-S49、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（[`Dialog`][super::dialog::Dialog] と同じく、
/// 状態型 `S` を所有していないため `derive` マクロが自動付与する `S: Debug` /
/// `S: Clone` という不要な境界を避ける。関数ポインタである `open` / `selected` は
/// `Copy` のため手動実装のコストは小さい）。
pub struct Menu<S> {
    id: WidgetId,
    /// トリガー（ドロップダウンボタン）の表示文言・プレースホルダ。
    label: String,
    /// メニュー項目の表示文言（構築時固定。状態束縛の動的項目は S50 以降の
    /// スコープ外事項、モジュール doc 参照）。
    items: Vec<String>,
    /// 開閉状態のアプリ状態 `S` からの読み取り経路（REQ-13、環境キャプチャ不可の
    /// 関数ポインタに限定）。開閉状態の SSOT は常に `S` 側に置き、本ウィジェットは
    /// キャッシュのみを保持する（`Checkbox` / `Slider` / `Dialog` と同じ制御型の
    /// 設計判断）。
    open: ReadFn<S, bool>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`is_open()` が返す値）。
    cached_open: bool,
    /// 選択中項目 index の読み取り経路（`RadioGroupWidget` S23 と同じ `Option<usize>`
    /// の SSOT パターン）。
    selected: ReadFn<S, Option<usize>>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ。範囲外 index は
    /// [`clamp_selected`] により安全側で `None` へフォールバックする。
    cached_selected: Option<usize>,
    /// ポップアップ内 roving focus の現在位置（`None` は未確立、モジュール doc
    /// 「状態・イベント処理」参照。`InteractionState::focused` とは直交する
    /// グループ固有の一時 UI 状態、`RadioGroupWidget::focused_index` と同型）。
    focused_item: Option<usize>,
    /// 直近の `PointerPressed` が発生した領域（[`Self::apply_pointer`] の
    /// press-and-drag-away 判定に使う、`DialogWidget::press_region` と同型）。
    press_region: Option<MenuHitRegion>,
    /// hover/pressed/disabled/focus。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Menu<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // label / items は UI 表示文字列でありユーザーの秘密情報に該当しないため、
        // `Dialog` の title / body 等と同様に `Debug` へそのまま含める。
        f.debug_struct("Menu")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("items", &self.items)
            .field("cached_open", &self.cached_open)
            .field("cached_selected", &self.cached_selected)
            .field("focused_item", &self.focused_item)
            .field("press_region", &self.press_region)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Menu<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            items: self.items.clone(),
            open: self.open,
            cached_open: self.cached_open,
            selected: self.selected,
            cached_selected: self.cached_selected,
            focused_item: self.focused_item,
            press_region: self.press_region,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Menu<S> {
    /// 状態 `S` に束縛したメニューを作る（ビルダー形式の起点）。
    ///
    /// `open` / `selected` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`]）と
    /// 代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。既定は閉状態・未選択（`sync_from_state` 呼び出し前のプレースホルダ、
    /// [`Self::bound`] を呼んだ直後の初期値）。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        items: Vec<String>,
        open: ReadFn<S, bool>,
        selected: ReadFn<S, Option<usize>>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            items,
            open,
            cached_open: false,
            selected,
            cached_selected: None,
            focused_item: None,
            press_region: None,
            interaction: InteractionState::default(),
        }
    }

    /// トリガーの表示文言（プレースホルダ）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// メニュー項目の表示文言一覧を返す。
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// `cached_open`（直近の [`Widget::sync_from_state`] が読み取った開閉状態）を返す。
    pub fn is_open(&self) -> bool {
        self.cached_open
    }

    /// `cached_selected`（直近の [`Widget::sync_from_state`] が読み取った選択中項目
    /// index。範囲外は [`clamp_selected`] により `None` へフォールバック済み）を返す。
    pub fn selected_index(&self) -> Option<usize> {
        self.cached_selected
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// ポップアップ内 roving focus の現在位置を返す（`None` は閉状態、または
    /// まだ一度も開いていないことを表す）。
    pub fn focused_item(&self) -> Option<usize> {
        self.focused_item
    }

    /// disabled フラグを切り替える。`true` へ遷移する際は `press_region` もクリアする
    /// （[`Self::apply_pointer`] / `PointerExited` と同じ理由。stale な状態を残さない、
    /// `DialogWidget::set_disabled` / `RadioGroupWidget::set_disabled` と同じ判断）。
    pub fn set_disabled(&mut self, disabled: bool) {
        if disabled {
            self.press_region = None;
        }
        self.interaction.set_disabled(disabled);
    }

    /// トリガーの表示テキスト（選択中項目の文言、なければ [`Self::label_text`]）を返す。
    ///
    /// `cached_selected` が範囲外（構築後の `items` 変更等、通常は発生しない想定）の
    /// 場合も安全側で `label` へフォールバックする（panic させない）。
    fn display_text(&self) -> String {
        self.cached_selected
            .and_then(|index| self.items.get(index))
            .cloned()
            .unwrap_or_else(|| self.label.clone())
    }

    /// トリガー矩形の寸法を `available` へクランプして返す
    /// （`measure` / `paint` / `hit_test` の 3 箇所が共有する単一の計算根拠）。
    fn trigger_size(&self, available: Measurement) -> Measurement {
        Measurement {
            width: TRIGGER_WIDTH.min(available.width),
            height: (APPROX_LINE_HEIGHT + MENU_PADDING).min(available.height),
        }
    }

    /// ポップアップ（項目一覧パネル）矩形の寸法を `available` へクランプして返す
    /// （`measure` / `paint` / `hit_test` の 3 箇所が共有する単一の計算根拠）。
    fn popup_size(&self, available: Measurement) -> Measurement {
        let item_count = self.items.len() as f32;
        Measurement {
            width: TRIGGER_WIDTH.min(available.width),
            height: (APPROX_LINE_HEIGHT * item_count + MENU_PADDING).min(available.height),
        }
    }

    /// トリガー + ポップアップを縦積みした開状態の合計寸法を `available` へ
    /// クランプして返す（[`Self::trigger_size`] / [`Self::popup_size`] を合成する、
    /// `measure` / `hit_test` の開状態分岐が共有する）。
    ///
    /// `trigger_size` / `popup_size` はそれぞれ独立に `available` へクランプするため、
    /// 高さの単純合計は `available.height` を超え得る（モジュール doc の「開状態は
    /// … を `available` へクランプする」契約に反する）。合成後の高さも
    /// `available.height` へ再クランプし、幅は既にどちらも `available.width` 以下の
    /// ため `min` は不要（`max` で広い方を採用するのみ）。
    fn open_total_size(&self, available: Measurement) -> Measurement {
        let trigger = self.trigger_size(available);
        let popup = self.popup_size(available);
        Measurement {
            width: trigger.width.max(popup.width),
            height: (trigger.height + popup.height).min(available.height),
        }
    }

    /// ポップアップ内の相対 y 座標から項目行インデックスを解決する
    /// （`RadioGroupWidget::option_at` と同じ「相対 y ÷ [`APPROX_LINE_HEIGHT`]」の
    /// 行近似）。範囲外インデックスは [`clamp_selected`] により安全側で `None` へ
    /// クランプする（panic させない）。
    fn item_at_relative_y(&self, relative_y: f32) -> Option<usize> {
        if relative_y < 0.0 {
            return None;
        }
        let index = (relative_y / APPROX_LINE_HEIGHT) as usize;
        clamp_selected(Some(index), self.items.len())
    }

    /// ポインタ座標がメニューのどの領域上にあるかを解決する（モジュール doc
    /// 「状態・イベント処理」参照）。
    ///
    /// 非有限座標・disabled 中・閉状態は安全側の [`MenuHitRegion::Outside`] を返す
    /// （`DialogWidget::hit_region` と同じ判断）。`size` は [`Self::trigger_size`] /
    /// [`Self::open_total_size`] へ渡し、`measure` が返す確定寸法と一致させる
    /// （`hit_test` と同じ整合性契約）。
    pub fn hit_region(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> MenuHitRegion {
        // disabled 中は常に Outside（`DialogWidget::hit_region` と同じ安全側判断）。
        // 閉状態でも「トリガー矩形上」だけは判定するため（後段の `trigger_contains`
        // 呼び出しへ委譲）、ここでは disabled のみを早期リターンで弾く。
        if self.interaction.disabled {
            return MenuHitRegion::Outside;
        }

        let (px, py) = point;
        if !px.is_finite() || !py.is_finite() {
            return MenuHitRegion::Outside;
        }

        if self.trigger_contains(point, origin, size) {
            return MenuHitRegion::Trigger;
        }

        if !self.cached_open {
            return MenuHitRegion::Outside;
        }

        let trigger = self.trigger_size(size);
        let popup = self.popup_size(size);
        // `open_total_size` は trigger + popup の合計高さを `size.height` へ再クランプ
        // する（`trigger_size` / `popup_size` はそれぞれ独立にクランプするため単純合計
        // が `size.height` を超え得る）。`measure` / `hit_test` が返す確定寸法と
        // 一致させるため、ここでもクランプ後の高さを popup の実効高さとして使う
        // （Bugbot 指摘対応: クランプ前の `popup.height` のままだとオーバーフロー分が
        // `Item` / `Popup` 扱いになり `Outside` 判定と食い違う）。
        let open_total = self.open_total_size(size);
        let effective_popup_height = (open_total.height - trigger.height).max(0.0);
        let popup_origin_y = origin.1 + trigger.height;
        let in_popup = px >= origin.0
            && px < origin.0 + popup.width
            && py >= popup_origin_y
            && py < popup_origin_y + effective_popup_height;

        if !in_popup {
            return MenuHitRegion::Outside;
        }

        match self.item_at_relative_y(py - popup_origin_y) {
            Some(index) => MenuHitRegion::Item(index),
            None => MenuHitRegion::Popup,
        }
    }

    /// トリガー矩形上かどうかを判定する（[`Self::hit_region`] が委譲する小さなヘルパー、
    /// `Self::trigger_size` を単一の計算根拠として再利用する）。
    fn trigger_contains(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        let (px, py) = point;
        if !px.is_finite() || !py.is_finite() {
            return false;
        }
        let trigger = self.trigger_size(size);
        px >= origin.0
            && px < origin.0 + trigger.width
            && py >= origin.1
            && py < origin.1 + trigger.height
    }

    /// ポインタ入力を、[`Self::hit_region`] が解決した領域とともに適用する
    /// （モジュール doc「状態・イベント処理」参照）。
    ///
    /// 確定は次の 3 経路のいずれか（press 時の領域 `press_region` と release 時の
    /// 領域が一致した場合のみ、press-and-drag-away では発火しない）:
    /// (1) トリガー同士 → 開閉トグル（閉状態でも有効。トリガークリックがメニューを
    /// 開く操作そのものであるため、`Menu` は「閉状態でもトリガーが常に存在する UI」
    /// というモジュール doc 冒頭の設計を踏襲する）、(2) 開状態で同一 `Item(i)` 同士 →
    /// 選択確定（`focused_item` も同期）、(3) 開状態で `Outside` 同士 → 外側クリック閉じ。
    /// `PointerExited` は `press_region` をクリアする。disabled 中は全入力を無視し
    /// `press_region` もクリアする（`DialogWidget::apply_pointer` と同じ判断）。
    pub fn apply_pointer(
        &mut self,
        input: InteractionInput,
        region: MenuHitRegion,
    ) -> Option<Event> {
        if self.interaction.disabled {
            self.press_region = None;
            return None;
        }

        let event = match input {
            InteractionInput::PointerPressed => {
                self.press_region = Some(region);
                None
            }
            InteractionInput::PointerReleased => {
                let pressed = self.press_region.take();
                match (pressed, region) {
                    (Some(MenuHitRegion::Trigger), MenuHitRegion::Trigger) => {
                        Some(Event::ButtonClicked {
                            label: self.label.clone(),
                        })
                    }
                    (Some(MenuHitRegion::Item(p)), MenuHitRegion::Item(r))
                        if self.cached_open && p == r =>
                    {
                        self.focused_item = Some(r);
                        self.items.get(r).map(|label| Event::ButtonClicked {
                            label: label.clone(),
                        })
                    }
                    (Some(MenuHitRegion::Outside), MenuHitRegion::Outside) if self.cached_open => {
                        Some(Event::ButtonClicked {
                            label: self.label.clone(),
                        })
                    }
                    _ => None,
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

    /// フォーカスリングを `delta` 項目分ラップアラウンドで進める（負値は逆方向）。
    /// 開状態かつ項目が存在する場合のみ意味を持つ（呼び出し元が事前にガードする）。
    fn advance_focused_item(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as i32;
        let current = self.focused_item.map(|i| i as i32).unwrap_or(-1);
        let next = (current + delta).rem_euclid(len);
        self.focused_item = Some(next as usize);
    }

    /// キーボード入力を処理する。
    ///
    /// `focused`（[`InteractionState::focused`]）かつ非 disabled のときのみ受け付ける
    /// （`RadioGroupWidget::handle_input` と同じ「フォーカス保持中のみ操作可能」の
    /// 設計判断）。`Dismiss` / 確定系の発火時は `press_region` をクリアする（Esc 直後の
    /// release 二重発火防止、`DialogWidget::handle_input` と同じ理由）。
    pub fn handle_input(&mut self, input: MenuInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        match input {
            MenuInput::ActivateTrigger => {
                self.press_region = None;
                Some(Event::ButtonClicked {
                    label: self.label.clone(),
                })
            }
            MenuInput::Dismiss => {
                if !self.cached_open {
                    return None;
                }
                self.press_region = None;
                Some(Event::ButtonClicked {
                    label: self.label.clone(),
                })
            }
            MenuInput::FocusNext => {
                if !self.cached_open {
                    return None;
                }
                self.advance_focused_item(1);
                None
            }
            MenuInput::FocusPrev => {
                if !self.cached_open {
                    return None;
                }
                self.advance_focused_item(-1);
                None
            }
            MenuInput::Select => {
                if !self.cached_open {
                    return None;
                }
                let index = self.focused_item?;
                self.press_region = None;
                self.items.get(index).map(|label| Event::ButtonClicked {
                    label: label.clone(),
                })
            }
        }
    }
}

impl<S: 'static> Widget<S> for Menu<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から開閉状態・選択中項目 index を読み取り `cached_open` /
    /// `cached_selected` を更新する（reconcile フェーズ）。範囲外 index は
    /// [`clamp_selected`] により安全側で `None` へフォールバックする。開閉遷移も
    /// あわせて検知し、開→閉で `focused_item` / `press_region` をクリア、閉→開で
    /// `focused_item` を `cached_selected.unwrap_or(0)` へ初期化する（モジュール doc
    /// 「状態・イベント処理」参照、`DialogWidget::sync_from_state` の遷移リセットと
    /// `RadioGroupWidget::apply_input(FocusGained)` の初期位置決定を組み合わせた設計）。
    fn sync_from_state(&mut self, state: &S) {
        let previous_open = self.cached_open;
        self.cached_open = (self.open)(state);
        self.cached_selected = clamp_selected((self.selected)(state), self.items.len());

        if !previous_open && self.cached_open {
            self.focused_item = if self.items.is_empty() {
                None
            } else {
                Some(self.cached_selected.unwrap_or(0))
            };
        } else if previous_open && !self.cached_open {
            self.focused_item = None;
            self.press_region = None;
            // close 時点で pressed/hovered を残さない（Bugbot 指摘対応、
            // `DialogWidget::sync_from_state` の close 時リセットと同型）。
            // ポップアップ押下中にキーボード操作（Select/Dismiss 相当）で
            // 同期的に閉じると、閉じたトリガーの `hit_test` 対象がトリガー矩形
            // のみへ縮小されるため PointerReleased / PointerExited が届かなく
            // なり、`interaction.pressed` / `hovered` が古い値のまま残り得る。
            self.interaction.pressed = false;
            self.interaction.hovered = false;
        }
    }

    /// 閉状態はトリガー矩形のみ、開状態はトリガー + ポップアップの合計寸法を返す
    /// （[`Self::trigger_size`] / [`Self::open_total_size`] 参照）。`hit_test` へ
    /// 渡される確定寸法の根拠と一致させるための設計（`Dialog` が PR #386 Bugbot
    /// 指摘で修正された整合性契約と同型）。
    ///
    /// 非有限値・負値は安全側（`0.0`）へ丸める（OWASP A04 相当の数値健全性対策）。
    fn measure(&self, available: Measurement) -> Measurement {
        let available = Measurement {
            width: normalize_non_negative_finite(available.width),
            height: normalize_non_negative_finite(available.height),
        };

        if self.cached_open {
            self.open_total_size(available)
        } else {
            self.trigger_size(available)
        }
    }

    /// 閉状態は `[Rect（トリガー）, Text(選択中項目の文言 or label)]`、開状態は
    /// これに続けて `[Rect（ポップアップパネル）, Text(item) × 項目数]` を返す。
    /// `PaintCommand` は拡張しない（モジュール doc 参照）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect, PaintCommand::Text(self.display_text())];

        if self.cached_open {
            commands.push(PaintCommand::Rect);
            commands.extend(self.items.iter().cloned().map(PaintCommand::Text));
        }

        commands
    }

    /// 閉状態はトリガー矩形のみを対象とする。開状態はトリガー + ポップアップ
    /// （縦積みの合計領域）を対象とする（S50 での項目クリック・外側クリック閉じの
    /// 布石）。対象領域は [`Self::trigger_size`] / [`Self::open_total_size`] へ
    /// 集約し、`measure` が返す確定寸法と一致させる（モジュール doc の整合性契約、
    /// `Dialog` が PR #386 Bugbot 指摘で修正した設計と同型。呼び出し側から渡される
    /// `size` は `measure` の戻り値の使い回しを想定しない — レイアウトエンジンが
    /// ビューポート全域の `size` をそのまま渡すケースでも対象領域を正しく限定する）。
    /// `interaction.disabled` の間は常に `false` を返す（`checkbox::Checkbox` /
    /// `slider::Slider` / `toggle_switch::ToggleSwitch` と同じ、無効化された
    /// ウィジェットがポインタ入力に反応しない安全側の判断。`Dialog::hit_test` も
    /// 同じ判断で disabled を先にガードしている）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.interaction.disabled {
            return false;
        }

        let target_size = if self.cached_open {
            self.open_total_size(size)
        } else {
            self.trigger_size(size)
        };

        point.0 >= origin.0
            && point.0 < origin.0 + target_size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + target_size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ComboBox
    }

    /// トリガー文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
    fn accessibility_label(&self) -> Option<String> {
        Some(self.label.clone())
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

    struct MenuState {
        menu_open: bool,
        selected: Option<usize>,
    }

    fn get_menu_open(state: &MenuState) -> bool {
        state.menu_open
    }

    fn get_selected(state: &MenuState) -> Option<usize> {
        state.selected
    }

    fn make_items() -> Vec<String> {
        vec![
            "項目 A".to_string(),
            "項目 B".to_string(),
            "項目 C".to_string(),
        ]
    }

    fn make_menu() -> Menu<MenuState> {
        Menu::bound(
            WidgetId::from_raw(1),
            "選択してください",
            make_items(),
            get_menu_open,
            get_selected,
        )
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    // (a) 構築直後の既定値

    #[test]
    fn bound_constructs_with_label_items_and_closed_unselected_default() {
        let menu = make_menu();

        assert_eq!(menu.label_text(), "選択してください");
        assert_eq!(menu.items(), make_items().as_slice());
        assert!(!menu.is_open());
        assert_eq!(menu.selected_index(), None);
        assert_eq!(menu.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_interaction_state_is_all_false() {
        let menu = make_menu();
        let interaction = menu.interaction_state();

        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    // (b) 閉状態: measure がトリガー寸法のみ / paint が [Rect, Text(label)] /
    // hit_test がトリガー矩形のみ（外側 false）

    #[test]
    fn closed_menu_measures_trigger_only() {
        let menu = make_menu();
        let measurement = menu.measure(sized(800.0, 600.0));

        assert_eq!(measurement.width, TRIGGER_WIDTH);
        assert_eq!(measurement.height, APPROX_LINE_HEIGHT + MENU_PADDING);
    }

    #[test]
    fn closed_menu_paints_trigger_and_label() {
        let menu = make_menu();
        let commands = menu.paint((0.0, 0.0), menu.measure(sized(800.0, 600.0)));

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("選択してください".to_string()),
            ]
        );
    }

    #[test]
    fn closed_menu_hit_test_is_limited_to_trigger_rect() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });

        let measured = menu.measure(sized(800.0, 600.0));
        assert!(menu.hit_test((10.0, 10.0), (0.0, 0.0), measured));
        assert!(!menu.hit_test((800.0, 600.0), (0.0, 0.0), measured));
    }

    /// `size` に `measure()` の戻り値ではなくビューポート全域をそのまま渡した場合の
    /// 判別テスト。トリガー矩形への限定を `size` の値そのものに依存させず
    /// `Self::trigger_size` の再計算で行っていることを確認する（`size` をそのまま
    /// 矩形として使う実装だとこのテストは失敗する）。
    #[test]
    fn closed_menu_hit_test_limits_to_trigger_even_when_size_is_full_viewport() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });

        let viewport = sized(800.0, 600.0);
        // トリガー高さ（APPROX_LINE_HEIGHT + MENU_PADDING = 36.0）より下は
        // トリガー矩形の外側のため、ヒットしない。
        assert!(!menu.hit_test((10.0, 200.0), (0.0, 0.0), viewport));
    }

    // (c) 開状態: paint が [Rect, Text, Rect, Text * items] / hit_test がトリガー +
    // ポップアップ域

    #[test]
    fn open_menu_paints_trigger_and_popup_items() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        let commands = menu.paint((0.0, 0.0), sized(800.0, 600.0));

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("選択してください".to_string()),
                PaintCommand::Rect,
                PaintCommand::Text("項目 A".to_string()),
                PaintCommand::Text("項目 B".to_string()),
                PaintCommand::Text("項目 C".to_string()),
            ]
        );
    }

    #[test]
    fn open_menu_measures_trigger_plus_popup() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        let measurement = menu.measure(sized(800.0, 600.0));
        let expected_height =
            (APPROX_LINE_HEIGHT + MENU_PADDING) + (APPROX_LINE_HEIGHT * 3.0 + MENU_PADDING);

        assert_eq!(measurement.width, TRIGGER_WIDTH);
        assert_eq!(measurement.height, expected_height);
    }

    #[test]
    fn open_menu_hit_test_covers_trigger_and_popup_area() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        let measured = menu.measure(sized(800.0, 600.0));
        assert!(menu.hit_test((0.0, 0.0), (0.0, 0.0), measured));
        assert!(menu.hit_test((10.0, measured.height - 1.0), (0.0, 0.0), measured));
        assert!(!menu.hit_test((0.0, measured.height), (0.0, 0.0), measured));
    }

    // (d) 選択表示: cached_selected が項目文言表示に反映される / 範囲外 index は
    // label へフォールバック

    #[test]
    fn selected_index_reflects_in_displayed_trigger_text() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: Some(1),
        });

        assert_eq!(menu.selected_index(), Some(1));
        let commands = menu.paint((0.0, 0.0), menu.measure(sized(800.0, 600.0)));
        assert_eq!(commands[1], PaintCommand::Text("項目 B".to_string()));
    }

    #[test]
    fn out_of_range_selected_index_falls_back_to_label() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: Some(99),
        });

        assert_eq!(menu.selected_index(), None);
        let commands = menu.paint((0.0, 0.0), menu.measure(sized(800.0, 600.0)));
        assert_eq!(
            commands[1],
            PaintCommand::Text("選択してください".to_string())
        );
    }

    // (e) sync_from_state による open/selected の遷移

    #[test]
    fn sync_from_state_transitions_open_closed_open() {
        let mut menu = make_menu();
        let mut state = MenuState {
            menu_open: true,
            selected: None,
        };

        menu.sync_from_state(&state);
        assert!(menu.is_open());

        state.menu_open = false;
        menu.sync_from_state(&state);
        assert!(!menu.is_open());

        state.menu_open = true;
        menu.sync_from_state(&state);
        assert!(menu.is_open());
    }

    /// 開→閉の遷移で `interaction.pressed` / `hovered` もクリアされることを確認
    /// する（`DialogWidget::sync_from_state` の close 時リセットと同型の回帰
    /// テスト。Bugbot 指摘対応: 修正前は `focused_item` / `press_region` のみ
    /// クリアし、`interaction.pressed` / `hovered` が古い値のまま残っていた）。
    #[test]
    fn sync_from_state_clears_pressed_and_hovered_on_close_transition() {
        let mut menu = make_menu();
        let mut state = MenuState {
            menu_open: true,
            selected: None,
        };
        menu.sync_from_state(&state);

        let size = sized(800.0, 600.0);
        let region = menu.hit_region((10.0, 10.0), (0.0, 0.0), size);
        menu.apply_pointer(InteractionInput::PointerPressed, region);
        assert!(menu.interaction_state().pressed);

        state.menu_open = false;
        menu.sync_from_state(&state);

        let interaction = menu.interaction_state();
        assert!(!interaction.pressed);
        assert!(!interaction.hovered);
    }

    // (f) 数値健全性: 非有限・負の available で 0 寸法 / 小さい available へのクランプ

    #[test]
    fn measure_handles_non_finite_and_negative_available() {
        let menu = make_menu();

        let non_finite = menu.measure(sized(f32::NAN, f32::INFINITY));
        assert!(non_finite.width.is_finite());
        assert!(non_finite.height.is_finite());
        assert_eq!(non_finite.width, 0.0);

        let negative = menu.measure(sized(-10.0, -10.0));
        assert_eq!(negative.width, 0.0);
        assert_eq!(negative.height, 0.0);
    }

    #[test]
    fn measure_clamps_to_small_available_space() {
        let menu = make_menu();

        let measurement = menu.measure(sized(10.0, 5.0));
        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    /// 開状態でも合計寸法（トリガー + ポップアップ）が `available` を超えない
    /// ことを確認する（`trigger_size` / `popup_size` はそれぞれ独立にクランプする
    /// ため、単純合計だけでは `available` を超え得るという回帰を防ぐ）。
    #[test]
    fn measure_clamps_open_total_to_small_available_space() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        let measurement = menu.measure(sized(800.0, 50.0));
        assert_eq!(measurement.height, 50.0);
    }

    // (g) measure 結果をそのまま hit_test へパイプした整合性
    // （`Dialog` の PR #386 Bugbot 対応と同型の回帰テスト）

    #[test]
    fn measure_result_covers_full_range_in_hit_test_when_open() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        let available = sized(800.0, 600.0);
        let measured = menu.measure(available);

        assert!(menu.hit_test((0.0, 0.0), (0.0, 0.0), measured));
        assert!(menu.hit_test(
            (measured.width - 1.0, measured.height - 1.0),
            (0.0, 0.0),
            measured
        ));
        assert!(!menu.hit_test((measured.width, measured.height), (0.0, 0.0), measured));
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_open_and_inside() {
        // `checkbox::Checkbox` / `slider::Slider` / `toggle_switch::ToggleSwitch` /
        // `dialog::Dialog` と同じ、無効化されている間は常に `false` を返す安全側の
        // 判断（実際のイベント抑止配線自体は S50 のスコープ）。
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        menu.set_disabled(true);

        let measured = menu.measure(sized(800.0, 600.0));
        assert!(!menu.hit_test((10.0, 10.0), (0.0, 0.0), measured));
    }

    #[test]
    fn set_disabled_transitions_visual_state() {
        let mut menu = make_menu();
        assert_eq!(menu.visual_state(), VisualState::Idle);

        menu.set_disabled(true);
        assert_eq!(menu.visual_state(), VisualState::Disabled);
    }

    // (h) accessibility_role / clone_box / Debug / Send+Sync

    #[test]
    fn accessibility_role_is_combo_box() {
        let menu = make_menu();
        assert_eq!(menu.accessibility_role(), AccessibilityRole::ComboBox);
    }

    #[test]
    fn accessibility_label_is_trigger_label() {
        let menu = make_menu();
        assert_eq!(
            menu.accessibility_label(),
            Some("選択してください".to_string())
        );
    }

    #[test]
    fn clone_box_preserves_state_independently() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: Some(0),
        });

        let mut cloned = menu.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        cloned.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });

        // clone 元は影響を受けない（独立性の検証）。
        assert!(menu.is_open());
        assert_eq!(menu.selected_index(), Some(0));
    }

    #[test]
    fn debug_output_does_not_panic() {
        let menu = make_menu();
        let output = format!("{menu:?}");
        assert!(output.contains("Menu"));
    }

    #[test]
    fn menu_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Menu<MenuState>>();
    }

    // (i) Box<dyn Widget<S>> への格納（object safety）

    #[test]
    fn menu_can_be_boxed_as_dyn_widget() {
        let menu: Box<dyn Widget<MenuState>> = Box::new(make_menu());
        assert_eq!(menu.accessibility_role(), AccessibilityRole::ComboBox);
    }

    // (j) hit_region（TASK-8.3-S50 / #145）

    #[test]
    fn closed_menu_hit_region_resolves_trigger_or_outside() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });
        let size = sized(800.0, 600.0);

        assert_eq!(
            menu.hit_region((10.0, 10.0), (0.0, 0.0), size),
            MenuHitRegion::Trigger
        );
        assert_eq!(
            menu.hit_region((10.0, 200.0), (0.0, 0.0), size),
            MenuHitRegion::Outside
        );
    }

    #[test]
    fn open_menu_hit_region_resolves_item_rows() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        let size = sized(800.0, 600.0);
        let trigger_height = APPROX_LINE_HEIGHT + MENU_PADDING;

        // トリガー矩形上。
        assert_eq!(
            menu.hit_region((10.0, 10.0), (0.0, 0.0), size),
            MenuHitRegion::Trigger
        );
        // ポップアップ先頭行（項目 A）。
        assert_eq!(
            menu.hit_region((10.0, trigger_height + 1.0), (0.0, 0.0), size),
            MenuHitRegion::Item(0)
        );
        // ポップアップ 2 行目（項目 B）。
        assert_eq!(
            menu.hit_region(
                (10.0, trigger_height + APPROX_LINE_HEIGHT + 1.0),
                (0.0, 0.0),
                size
            ),
            MenuHitRegion::Item(1)
        );
    }

    #[test]
    fn open_menu_hit_region_padding_area_is_popup() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        let size = sized(800.0, 600.0);
        let measured = menu.measure(size);
        // ポップアップ末尾の余白（3 項目分の行を過ぎた位置）はパディング域。
        let padding_y = measured.height - 1.0;

        assert_eq!(
            menu.hit_region((10.0, padding_y), (0.0, 0.0), size),
            MenuHitRegion::Popup
        );
    }

    #[test]
    fn hit_region_is_outside_for_non_finite_point() {
        let menu = make_menu();
        let size = sized(800.0, 600.0);

        assert_eq!(
            menu.hit_region((f32::NAN, 10.0), (0.0, 0.0), size),
            MenuHitRegion::Outside
        );
        assert_eq!(
            menu.hit_region((10.0, f32::INFINITY), (0.0, 0.0), size),
            MenuHitRegion::Outside
        );
    }

    #[test]
    fn hit_region_is_outside_when_disabled() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        menu.set_disabled(true);

        assert_eq!(
            menu.hit_region((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)),
            MenuHitRegion::Outside
        );
    }

    /// `hit_region` が `open_total_size` の高さクランプを反映することを確認する
    /// （`measure_clamps_open_total_to_small_available_space` と同じ極小 available
    /// を与え、クランプ後の高さを超える座標は `Outside` になる回帰テスト。
    /// Bugbot 指摘対応: 修正前は `popup_size` の未クランプ高さをそのまま使い、
    /// クランプ後にはみ出す座標まで `Item` / `Popup` として扱っていた）。
    #[test]
    fn open_menu_hit_region_respects_open_total_clamp() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        let size = sized(800.0, 50.0);
        let measured = menu.measure(size);

        // クランプ後の合計高さ内は境界直前まで Item/Popup 判定になり得る。
        assert_ne!(
            menu.hit_region((10.0, measured.height - 1.0), (0.0, 0.0), size),
            MenuHitRegion::Trigger
        );
        // クランプ後の合計高さちょうど、またはそれ以降は Outside（トリガー矩形の
        // 判定にも popup 判定にも該当しない安全側の結果に収束する）。
        assert_eq!(
            menu.hit_region((10.0, measured.height), (0.0, 0.0), size),
            MenuHitRegion::Outside
        );
        assert_eq!(
            menu.hit_region((10.0, measured.height + 100.0), (0.0, 0.0), size),
            MenuHitRegion::Outside
        );
    }

    // (k) apply_pointer（TASK-8.3-S50 / #145）

    #[test]
    fn trigger_press_and_release_toggles_open() {
        let mut menu = make_menu();

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Trigger);
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Trigger);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "選択してください".to_string(),
            })
        );
    }

    #[test]
    fn same_item_press_and_release_selects_and_syncs_focus() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Item(1));
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Item(1));

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "項目 B".to_string(),
            })
        );
        assert_eq!(menu.focused_item(), Some(1));
    }

    #[test]
    fn press_and_drag_away_does_not_fire() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Item(0));
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Item(1));

        assert_eq!(event, None);
    }

    #[test]
    fn open_outside_press_and_release_closes() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Outside);
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Outside);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "選択してください".to_string(),
            })
        );
    }

    #[test]
    fn closed_outside_press_and_release_does_not_fire() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Outside);
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Outside);

        assert_eq!(event, None);
    }

    #[test]
    fn pointer_exited_clears_press_region() {
        let mut menu = make_menu();

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Trigger);
        menu.apply_pointer(InteractionInput::PointerExited, MenuHitRegion::Outside);
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Trigger);

        assert_eq!(event, None);
    }

    #[test]
    fn disabled_menu_ignores_pointer_input() {
        let mut menu = make_menu();
        menu.set_disabled(true);

        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Trigger);
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Trigger);

        assert_eq!(event, None);
    }

    #[test]
    fn stale_press_region_is_cleared_when_disabled() {
        let mut menu = make_menu();
        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Trigger);

        menu.set_disabled(true);
        menu.set_disabled(false);

        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Trigger);
        assert_eq!(event, None);
    }

    // (l) handle_input（TASK-8.3-S50 / #145）

    fn focused_menu(open: bool) -> Menu<MenuState> {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: open,
            selected: None,
        });
        menu.interaction.apply(InteractionInput::FocusGained);
        menu
    }

    #[test]
    fn activate_trigger_toggles_open() {
        let mut menu = focused_menu(false);

        let event = menu.handle_input(MenuInput::ActivateTrigger);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "選択してください".to_string(),
            })
        );
    }

    #[test]
    fn dismiss_fires_only_when_open() {
        let mut closed = focused_menu(false);
        assert_eq!(closed.handle_input(MenuInput::Dismiss), None);

        let mut open = focused_menu(true);
        let event = open.handle_input(MenuInput::Dismiss);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "選択してください".to_string(),
            })
        );
    }

    #[test]
    fn focus_next_and_prev_wrap_around_without_selection_follows_focus() {
        let mut menu = focused_menu(true);
        assert_eq!(menu.focused_item(), Some(0));

        menu.handle_input(MenuInput::FocusNext);
        assert_eq!(menu.focused_item(), Some(1));
        // FocusNext は selection-follows-focus ではないため確定イベントを返さない。
        assert_eq!(menu.handle_input(MenuInput::FocusNext), None);
        assert_eq!(menu.focused_item(), Some(2));

        menu.handle_input(MenuInput::FocusNext);
        assert_eq!(menu.focused_item(), Some(0));

        menu.handle_input(MenuInput::FocusPrev);
        assert_eq!(menu.focused_item(), Some(2));
    }

    #[test]
    fn focus_next_ignored_when_closed() {
        let mut menu = focused_menu(false);
        menu.handle_input(MenuInput::FocusNext);
        assert_eq!(menu.focused_item(), None);
    }

    #[test]
    fn select_confirms_focused_item() {
        let mut menu = focused_menu(true);
        menu.handle_input(MenuInput::FocusNext);

        let event = menu.handle_input(MenuInput::Select);

        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "項目 B".to_string(),
            })
        );
    }

    #[test]
    fn select_on_empty_items_does_not_panic() {
        let mut menu = Menu::bound(
            WidgetId::from_raw(2),
            "空メニュー",
            Vec::new(),
            get_menu_open,
            get_selected,
        );
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        menu.interaction.apply(InteractionInput::FocusGained);

        assert_eq!(menu.handle_input(MenuInput::Select), None);
        menu.handle_input(MenuInput::FocusNext);
        assert_eq!(menu.focused_item(), None);
    }

    #[test]
    fn handle_input_ignored_when_not_focused_or_disabled() {
        let mut not_focused = make_menu();
        not_focused.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        assert_eq!(not_focused.handle_input(MenuInput::Dismiss), None);

        let mut disabled = focused_menu(true);
        disabled.set_disabled(true);
        assert_eq!(disabled.handle_input(MenuInput::Dismiss), None);
    }

    #[test]
    fn stale_press_region_is_cleared_after_keyboard_dismiss() {
        let mut menu = focused_menu(true);
        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Outside);

        menu.handle_input(MenuInput::Dismiss);

        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Outside);
        assert_eq!(event, None);
    }

    // (m) sync_from_state の開閉遷移リセット（TASK-8.3-S50 / #145）

    #[test]
    fn open_transition_initializes_focused_item_from_selection() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: Some(2),
        });
        assert_eq!(menu.focused_item(), None);

        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: Some(2),
        });
        assert_eq!(menu.focused_item(), Some(2));
    }

    #[test]
    fn open_transition_without_selection_focuses_first_item() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });

        assert_eq!(menu.focused_item(), Some(0));
    }

    #[test]
    fn close_transition_clears_focused_item_and_press_region() {
        let mut menu = make_menu();
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        menu.apply_pointer(InteractionInput::PointerPressed, MenuHitRegion::Item(0));
        assert_eq!(menu.focused_item(), Some(0));

        menu.sync_from_state(&MenuState {
            menu_open: false,
            selected: None,
        });
        assert_eq!(menu.focused_item(), None);

        // press_region も連動してクリアされているため、開き直しても直前の press は
        // 引き継がれない。
        menu.sync_from_state(&MenuState {
            menu_open: true,
            selected: None,
        });
        let event = menu.apply_pointer(InteractionInput::PointerReleased, MenuHitRegion::Item(0));
        assert_eq!(event, None);
    }
}
