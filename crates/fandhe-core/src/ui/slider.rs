//! スライダーウィジェット `Slider<S>`（TASK-8.3-S28/S29 / REQ-8、Issue #123/#124）。
//!
//! 親 issue #95（TASK-8.3、60 sub-issue）の 28〜29/60。スライダーの 3 工程
//! （S28: API 設計・基本描画 → S29: 状態・イベント処理 → S30: テスト・カタログ登録）の
//! うち第 1・第 2 工程であり、ウィジェット基盤（`Widget<S>` トレイト・`InteractionState`・
//! ヒットテスト・レイアウト・`WidgetHarness`・`WidgetCatalog`）と、同型の先行ウィジェット
//! [`super::toggle_switch::ToggleSwitch`]（S25/S26、Issue #120/#121）・
//! [`super::checkbox::Checkbox`]（S19、Issue #114）・
//! [`super::tab_bar::TabBarWidget`]（座標付き press/release）・
//! [`super::scroll_view_widget::ScrollViewWidget`]（ドラッグ状態機械、S41）が確立した
//! パターンを踏襲し、`Widget<S>` を実装する具体型 [`Slider`] を新設して基本描画
//! （measure / paint / hit_test / accessibility_role、S28）と、ドラッグ・キーボードに
//! よる状態・イベント処理（[`Self::press`] / [`Self::drag`] / [`Self::release`] /
//! [`Self::cancel_press`] / [`Self::apply_input`] / [`Self::handle_input`]、S29）を
//! 実装する。
//!
//! 設計文書 `docs/architecture/widget-architecture.md` §2 は `min`/`max`/`step` を
//! 「enum バリアントに直接持たせない」根拠例として本ウィジェットを挙げており、
//! `Node::Widget(Box<dyn Widget<S>>)` 経由での追加が前提（同文書 §4.1）。
//!
//! - 値の SSOT はアプリ状態 `S`。ウィジェットは [`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] で表示用キャッシュ `cached_value: f32` を更新するのみで、
//!   中間編集状態を持たない（`ToggleSwitch` / `Checkbox` と同じ構造上の理由）。
//! - `set: WriteFn<S, f32>` は [`Self::write_fn`] で取り出せる書き込み経路であり、
//!   本ウィジェットが `&mut S` に直接触れることはない。確定は [`Self::release`] /
//!   [`Self::handle_input`] が返す `Event::ButtonClicked` へブリッジするのみで、
//!   実際の `S` への書き込みは呼び出し側が `write_fn()(state, slider.value())` を
//!   実行する契約を維持する（`ToggleSwitch::apply_input` / `Checkbox::apply_input` と
//!   同じく `Event` enum への専用バリアント追加はスコープ外）。
//! - `min` / `max` / `step` は数値健全性（OWASP A04 相当、`.claude/rules/security.md`）の
//!   ため構築時に正規化する: `min > max` は入れ替え、非有限値（NaN/inf）は既定範囲
//!   （`0.0..=1.0`）へフォールバックする。`step` は `with_step` で任意設定し、
//!   `step <= 0.0` または非有限値は `None`（連続値）扱いへ落とす。`step` のスナップは
//!   [`Self::snap_to_step`]（S29）がドラッグ・キーボード両経路で実際に適用する。
//! - [`super::widget::AccessibilityRole::Slider`] を本ウィジェット専用のロールとして
//!   `widget.rs` へ加法的に追加する（`accesskit::Role::Slider` へ対応予定）。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す — `Checkbox` / `ToggleSwitch` と同じ
//!   判断）。描画列は `[Rect（トラック）, Rect（フィル済みトラック）, Rect（サム）,
//!   Text(値の書式化文字列), Text(label)]` とし、値テキストは内部生成の決め打ち書式
//!   （`format!("{cached_value:.1}/{max:.1}")` 相当）とすることで、ヘッドレス環境でも
//!   値の差分がコマンド列の差分として検証できる構成にする。値テキストは外部入力文字列を
//!   経由せず `f32` から生成するため、インジェクション経路にならない（OWASP A03 対策）。
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
//! (a) `Event::SliderChanged` 専用バリアント新設 + `WidgetId` ベースディスパッチ統合
//! （`checkbox` S20 / `toggle_switch` S26 / `dialog` S47 と同一の既知 scope-out）、
//! (b) `WidgetCatalog` への登録・テスト網羅拡充は TASK-8.3-S30（Issue #125）で完了した、
//! (c) 垂直向きスライダー・目盛り表示・ドラッグアニメーション、
//! (d) reconcile パス未配線の既知契約（上記参照、既存契約を変更しない）。

use crate::state::{Event, ReadFn, WidgetValue, WriteFn};

use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数であり、
/// `measure` の折り返し・複数行はスコープ外（`Checkbox` / `ToggleSwitch` と同じ
/// 既知の制約）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// スライダーのトラック（背景の帯）の幅（論理ピクセル相当）。
const TRACK_WIDTH: f32 = 160.0;

/// スライダーのサム（つまみ）の一辺の長さ（論理ピクセル相当）。
const THUMB_SIZE: f32 = 16.0;

/// トラックとラベルの間隔（論理ピクセル相当）。
const TRACK_LABEL_GAP: f32 = 8.0;

/// `min` / `max` が非有限値（NaN/inf）だった場合にフォールバックする既定の下限値。
///
/// 安全側の判断として、範囲不明時は `[0.0, 1.0]` の単位区間へ倒す
/// （数値健全性、OWASP A04 相当）。
const DEFAULT_MIN: f32 = 0.0;

/// `min` / `max` が非有限値だった場合にフォールバックする既定の上限値。
const DEFAULT_MAX: f32 = 1.0;

/// キーボード操作（[`SliderInput::Increase`] / [`SliderInput::Decrease`]）で `step`
/// 未設定（連続値）のスライダーを動かす際に使う既定刻み幅の比率。
///
/// `span`（`max - min`）に対する割合として定義し、`span * KEYBOARD_STEP_RATIO` を
/// 1 回の入力あたりの増減量とする（矢印キー 1 回で無段階に近い細かさで動かせるようにする
/// ための決め打ち定数）。
const KEYBOARD_STEP_RATIO: f32 = 0.01;

/// スライダーへのキーボード入力を表す列挙（TASK-8.3-S29）。
///
/// 外部由来の生キーコード・文字列を直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`ToggleSwitchInput` /
/// `CheckboxInput` / `ScrollViewInput` と同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderInput {
    /// 値を 1 段階増やす（矢印キー相当）。`step` があれば 1 step、なければ
    /// `span * KEYBOARD_STEP_RATIO` だけ増やす（[`Slider::handle_input`] 参照）。
    Increase,
    /// 値を 1 段階減らす（矢印キー相当）。増減幅は [`SliderInput::Increase`] と対称。
    Decrease,
    /// 値を `min` に設定する（Home キー相当）。
    SetToMin,
    /// 値を `max` に設定する（End キー相当）。
    SetToMax,
}

/// ポインタドラッグ中の基準状態（内部実装詳細、[`ScrollViewWidget`] の `DragState` と
/// 同種の基準点方式）。
///
/// [`Slider::press`] がドラッグ開始時点の値を記録し、[`Slider::cancel_press`] が
/// この値へプレビュー（`cached_value`）を復元できるようにする。
///
/// [`ScrollViewWidget`]: super::scroll_view_widget::ScrollViewWidget
#[derive(Debug, Clone, Copy, PartialEq)]
struct DragState {
    /// [`Slider::press`] 呼び出し時点（ドラッグ開始前）の `cached_value`。
    drag_origin_value: f32,
}

/// スライダーウィジェット（TASK-8.3-S28、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`ToggleSwitch` / `Checkbox` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。スライダー値・ラベルは秘密情報に該当しないため、`Debug` に
/// そのまま含める。
pub struct Slider<S> {
    id: WidgetId,
    label: String,
    min: f32,
    max: f32,
    /// 値のスナップ間隔。`None` は連続値を表す。実際のスナップ適用は S29 のスコープ。
    step: Option<f32>,
    /// アプリ状態 `S` から現在値を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    get: ReadFn<S, f32>,
    /// スライダー操作結果を状態 `S` へ反映する関数ポインタ。実際のイベント配線は
    /// 後続工程（S29）が別途提供する。
    set: WriteFn<S, f32>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`value()` が返す値）。
    /// 常に `[min, max]` へクランプ済みの値を保持する。[`Self::press`] / [`Self::drag`] /
    /// [`Self::handle_input`] もこのキャッシュへプレビュー値を書き込む（`S` へは
    /// 書き込まない、モジュール doc 参照）。
    cached_value: f32,
    /// hover/pressed/disabled/focus。
    interaction: InteractionState,
    /// ポインタドラッグ中の基準状態（非ドラッグ中は `None`、TASK-8.3-S29）。
    drag: Option<DragState>,
}

impl<S> std::fmt::Debug for Slider<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slider")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("min", &self.min)
            .field("max", &self.max)
            .field("step", &self.step)
            .field("cached_value", &self.cached_value)
            .field("interaction", &self.interaction)
            .field("drag", &self.drag)
            .finish()
    }
}

impl<S> Clone for Slider<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            min: self.min,
            max: self.max,
            step: self.step,
            get: self.get,
            set: self.set,
            cached_value: self.cached_value,
            interaction: self.interaction,
            drag: self.drag,
        }
    }
}

/// `min` / `max` を正規化する（`min > max` は入れ替え、非有限値は既定範囲へフォールバック）。
///
/// 数値健全性の確保（OWASP A04 相当）のため、後続工程（S29 のドラッグ座標→値変換）が
/// 異常値を引き継がないよう構築時点で無害化する。
fn normalize_range(min: f32, max: f32) -> (f32, f32) {
    if !min.is_finite() || !max.is_finite() {
        return (DEFAULT_MIN, DEFAULT_MAX);
    }
    if min > max { (max, min) } else { (min, max) }
}

/// `step` を正規化する（`step <= 0.0` または非有限値は `None`＝連続値へ倒す）。
fn normalize_step(step: f32) -> Option<f32> {
    if step.is_finite() && step > 0.0 {
        Some(step)
    } else {
        None
    }
}

impl<S> Slider<S> {
    /// 状態 `S` に束縛したスライダーを作る（ビルダー形式の起点）。
    ///
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`] / [`WriteFn`]）と
    /// 代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。`min` > `max` は入れ替えて正規化し、非有限値（NaN/inf）は既定範囲
    /// （`[0.0, 1.0]`）へフォールバックする（安全側、数値健全性）。
    /// `set` をコンストラクタに含める理由は、後続工程でビルダーシグネチャを
    /// 破壊変更しないためであり、値の書き込み・イベントディスパッチは後続工程
    /// （S29）のスコープとする（`ToggleSwitch::bound` / `Checkbox::bound` と同じ判断）。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        min: f32,
        max: f32,
        get: ReadFn<S, f32>,
        set: WriteFn<S, f32>,
    ) -> Self {
        let (min, max) = normalize_range(min, max);
        Self {
            id,
            label: label.into(),
            min,
            max,
            step: None,
            get,
            set,
            cached_value: min,
            interaction: InteractionState::default(),
            drag: None,
        }
    }

    /// スナップ間隔 `step` を設定するビルダーメソッド。
    ///
    /// `step <= 0.0` または非有限値（NaN/inf）は無視して連続値（`None`）のまま扱う
    /// （安全側の判断）。設定した `step` は [`Self::press`] / [`Self::drag`] が呼ぶ
    /// [`Self::point_to_value`] 経由で [`Self::snap_to_step`] のスナップ間隔として使用する。
    pub fn with_step(mut self, step: f32) -> Self {
        self.step = normalize_step(step);
        self
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// 下限値を返す（`bound` 時点で正規化済み）。
    pub fn min(&self) -> f32 {
        self.min
    }

    /// 上限値を返す（`bound` 時点で正規化済み）。
    pub fn max(&self) -> f32 {
        self.max
    }

    /// スナップ間隔を返す（`None` は連続値）。
    pub fn step(&self) -> Option<f32> {
        self.step
    }

    /// `cached_value`（直近の [`Widget::sync_from_state`] が読み取った値、`[min, max]` に
    /// クランプ済み）を返す。
    pub fn value(&self) -> f32 {
        self.cached_value
    }

    /// `[min, max]` 上の正規化位置（`0.0..=1.0`）を返す。
    ///
    /// `min == max`（縮退区間）の場合はゼロ除算を避けるため `0.0` を返す
    /// （安全側の判断）。
    pub fn ratio(&self) -> f32 {
        let span = self.max - self.min;
        if span <= 0.0 {
            0.0
        } else {
            (self.cached_value - self.min) / span
        }
    }

    /// イベント配線が使う書き込み経路を返す。
    ///
    /// この関数ポインタを呼び出す経路（値変更ディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる
    /// （`ToggleSwitch::write_fn` / `Checkbox::write_fn` と同型）。
    pub fn write_fn(&self) -> WriteFn<S, f32> {
        self.set
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    ///
    /// `InteractionState::set_disabled` は hover/press/focus をクリアするが、`drag`
    /// （ウィジェットローカル状態）は関知しないため、ここで明示的に後始末する
    /// （`ScrollViewWidget::set_disabled` と同じ判断）。単に `drag` を `None` にするだけでは
    /// jump-to-click / ドラッグ中のプレビュー値（`cached_value`）が未コミットのまま残り続け、
    /// 無効化後も確定していない値が表示され続けてしまう（Cursor Bugbot 指摘、Issue #124）。
    /// [`Self::cancel_press`] と同じ「ドラッグ開始前の値へ復元し、確定イベントは発火しない」
    /// 挙動へ委譲することで、再度 enable した際に stale なドラッグ基準が残る問題と、
    /// 未コミット値が残留する問題の両方を同時に解消する。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
        if disabled {
            self.cancel_press();
        }
    }

    /// `step` スナップを適用する（内部ヘルパー、TASK-8.3-S29 の核心）。
    ///
    /// `step: Some(s)` のとき `min + round((value - min) / s) * s` を `[min, max]` へ
    /// クランプして返す。`step: None`（連続値）は `[min, max]` へのクランプのみ行う。
    /// 非有限値（NaN/inf）は現在の `cached_value` を維持する安全側の判断とし、
    /// 縮退区間（`min == max`）はゼロ除算を避けるため常に `min` を返す
    /// （数値健全性、OWASP A04 相当）。
    fn snap_to_step(&self, value: f32) -> f32 {
        if !value.is_finite() {
            return self.cached_value;
        }
        let span = self.max - self.min;
        if span <= 0.0 {
            return self.min;
        }
        let clamped = value.clamp(self.min, self.max);
        match self.step {
            Some(step) => {
                (self.min + ((clamped - self.min) / step).round() * step).clamp(self.min, self.max)
            }
            None => clamped,
        }
    }

    /// ポインタ座標をトラック上の値へ変換する（内部ヘルパー、[`Self::press`] /
    /// [`Self::drag`] 共通）。
    ///
    /// `point.0` をトラック幅（`TRACK_WIDTH` と `size.width` の小さい方）に対する比率
    /// `[0.0, 1.0]` へクランプしたうえで `[min, max]` へ写像し、[`Self::snap_to_step`]
    /// を適用する。トラック幅が `0` 以下、または座標・実寸が非有限（NaN/inf）の場合は
    /// `None` を返し、呼び出し側が入力を無視できるようにする
    /// （数値健全性、OWASP A04 相当）。
    fn point_to_value(
        &self,
        point: (f32, f32),
        origin: (f32, f32),
        size: Measurement,
    ) -> Option<f32> {
        if !point.0.is_finite() || !origin.0.is_finite() || !size.width.is_finite() {
            return None;
        }
        let effective_track_width = TRACK_WIDTH.min(size.width);
        if effective_track_width <= 0.0 {
            return None;
        }
        let ratio = ((point.0 - origin.0) / effective_track_width).clamp(0.0, 1.0);
        let span = self.max - self.min;
        Some(self.snap_to_step(self.min + ratio * span))
    }

    /// ポインタ押下を処理し、トラック領域内であればドラッグを開始する（TASK-8.3-S29 の核心）。
    ///
    /// [`Widget::hit_test`] と同じ半開区間（`[origin, origin + size)`）の矩形包含判定を
    /// まずウィジェット全体（トラック・ギャップ・ラベルを含む）に対して行い（`disabled`
    /// 中は開始しない、安全側の判断）、ホバー/フォーカス相当の `InteractionState::apply`
    /// は常に適用する。一方でドラッグ開始・jump-to-click は「トラック本体（`[origin.0,
    /// origin.0 + effective_track_width]`、[`Self::point_to_value`] と同じ実効幅）」に
    /// 押下位置が収まっている場合のみ行う。`point_to_value` はトラック外の X 座標も
    /// `[0.0, 1.0]` へクランプして返すため、ここでトラック内判定を挟まずに呼び出すと、
    /// トラックとラベルの間の隙間やラベル自体をクリックしただけでプレビュー値が `max` へ
    /// 飛んでしまう（Cursor Bugbot 指摘、Issue #124）。右端は閉区間（`<=`）とし、トラック
    /// 右端ちょうど（`ratio == 1.0`、サムの位置）への押下は従来どおりドラッグ開始を許可する。
    ///
    /// ウィジェット全体の x 方向の包含判定も右端を閉区間（`<=`）とする。`size.width <=
    /// TRACK_WIDTH`（ラベル用の余白が無い狭いレイアウト）では実効トラック幅
    /// （[`Self::point_to_value`] 参照）がウィジェット全体の幅と一致し、`hit_test`
    /// と同じ半開区間（`<`）のままだと `point.0 == origin.0 + size.width` ちょうどの
    /// クリック（トラック右端＝`max`）がこのウィジェット全体判定で弾かれ、`within_track`
    /// の閉区間判定に到達する前に `press` が早期リターンしてしまい `max` に到達できない
    /// （Cursor Bugbot 指摘、Issue #124）。`hit_test`（隣接ウィジェットとの境界重複を避ける
    /// 半開区間の契約）はこの局所判定とは独立のため、ここを閉区間にしても `hit_test` の
    /// 契約・既存テストには影響しない。y 方向は `hit_test` と同じ半開区間を維持する。
    pub fn press(&mut self, point: (f32, f32), origin: (f32, f32), size: Measurement) {
        if self.interaction.disabled {
            return;
        }
        let within_bounds = point.0 >= origin.0
            && point.0 <= origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height;
        if !within_bounds {
            return;
        }

        self.interaction.apply(InteractionInput::PointerPressed);

        let effective_track_width = TRACK_WIDTH.min(size.width);
        let within_track = point.0.is_finite()
            && effective_track_width > 0.0
            && point.0 >= origin.0
            && point.0 <= origin.0 + effective_track_width;
        if !within_track {
            return;
        }

        self.drag = Some(DragState {
            drag_origin_value: self.cached_value,
        });
        if let Some(value) = self.point_to_value(point, origin, size) {
            self.cached_value = value;
        }
    }

    /// ドラッグ中の移動を処理し、プレビュー値（`cached_value`）を更新する
    /// （TASK-8.3-S29 の核心）。値が変化した場合のみ `true` を返す。
    ///
    /// ドラッグ未開始（`self.drag` が `None`）での呼び出しは無視して `false` を返す
    /// （`ScrollViewWidget` の「開始通知なしの移動通知を拒否する」判断と同じ）。
    /// `point_to_value`（内部ヘルパー）がトラック外の座標も `[0.0, 1.0]` へクランプするため、
    /// 領域外へ出てもポインタに追従し続ける（ポインタキャプチャの意味論。`Checkbox` の
    /// press-and-drag-away とは異なり、スライダーは press 成立後は領域を離れても
    /// ドラッグを継続する設計とする）。
    pub fn drag(&mut self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if self.drag.is_none() {
            return false;
        }
        let Some(value) = self.point_to_value(point, origin, size) else {
            return false;
        };
        if value == self.cached_value {
            return false;
        }
        self.cached_value = value;
        true
    }

    /// ドラッグを確定する（TASK-8.3-S29 の核心）。
    ///
    /// ドラッグ中でなければ `None` を返す（release 単独では確定しない）。確定値は
    /// [`Self::value`] から取得できる。実際の `S` への書き込みは行わず、呼び出し側が
    /// `write_fn()(state, slider.value())` を実行する既存契約を維持する
    /// （モジュール doc 参照）。
    pub fn release(&mut self) -> Option<Event> {
        self.interaction.apply(InteractionInput::PointerReleased);
        self.drag.take()?;
        Some(Event::ButtonClicked {
            label: self.label.clone(),
        })
    }

    /// ドラッグを中断し、プレビュー値をドラッグ開始前の値（`drag_origin_value`）へ
    /// 復元する。ドラッグ中でなければ何もしない。
    ///
    /// `drag` のクリアと合わせて `InteractionState::apply(PointerReleased)` により
    /// `interaction.pressed` も解除する（[`Self::release`] と同じ「押下状態の終了」を
    /// 表す操作のため）。これを怠ると OS 側のポインタキャプチャキャンセル等で
    /// `cancel_press` のみが呼ばれた経路（[`Self::apply_input`] は `PointerExited` /
    /// `FocusLost` でドラッグを中断しない設計のため、キャンセルは呼び出し側がこの
    /// メソッドを直接呼ぶ契約）で `pressed` が真のまま残り、スライダーが視覚的に
    /// 「押下中」で固定されてしまう（`TabBarWidget::cancel_press` /
    /// `RadioGroupWidget::cancel_press` が `pressed_*` を確実にクリアするのと同じ理由、
    /// Cursor Bugbot 指摘、Issue #124）。ドラッグ未開始（`self.drag` が `None`）での
    /// 呼び出しでも `pressed` 解除は行う（`release` が `drag.take()?` より前に
    /// `PointerReleased` を適用するのと同じく、押下状態の解除は常に安全側で行う判断）。
    pub fn cancel_press(&mut self) {
        self.interaction.apply(InteractionInput::PointerReleased);
        if let Some(drag) = self.drag.take() {
            self.cached_value = drag.drag_origin_value;
        }
    }

    /// ポインタ出入り・フォーカス移動を適用する（`InteractionState::apply` への委譲、
    /// `TabBarWidget::apply_input` と同型、TASK-8.3-S29）。
    ///
    /// `PointerExited` / `FocusLost` ではドラッグを中断しない（[`Self::press`] /
    /// [`Self::drag`] のポインタキャプチャ意味論を優先する）。確定はしない
    /// （[`Self::press`] / [`Self::drag`] / [`Self::release`] が担う）。
    ///
    /// `InteractionState::apply` は `PointerExited` で `hovered` と `pressed` を
    /// 同時に解除する汎用の意味論（`InteractionState::apply` のドキュメント参照）だが、
    /// これはポインタキャプチャを持たない大半のウィジェットを前提にした判断であり、
    /// ドラッグ中（`self.drag.is_some()`）のスライダーには当てはまらない。ドラッグは
    /// ポインタ領域外へ出ても継続する（本メソッドの上記契約）ため、`pressed` だけを
    /// 素通しで解除すると [`Self::visual_state`] が `Idle` に戻り、進行中のジェスチャと
    /// 見た目が乖離する（`pressed` を「ジェスチャ進行中」の指標として扱う呼び出し側も
    /// 同様に食い違う、Cursor Bugbot 指摘、Issue #124）。ドラッグ中の `PointerExited` に
    /// 限り、`hovered` の解除は反映しつつ `pressed` はキャプチャ継続を表す `true` へ
    /// 復元する（`InteractionState` 自体の意味論は変更せず、拡張点はウィジェット側に
    /// 閉じるという `InteractionState::apply` のドキュメントの方針に従う）。
    pub fn apply_input(&mut self, input: InteractionInput) {
        let preserve_pressed_for_capture =
            matches!(input, InteractionInput::PointerExited) && self.drag.is_some();
        self.interaction.apply(input);
        if preserve_pressed_for_capture {
            self.interaction.pressed = true;
        }
    }

    /// キーボード入力を処理する（TASK-8.3-S29 の核心）。
    ///
    /// `disabled` 中・非フォーカス中は全入力を無視する（`ToggleSwitch::handle_input` /
    /// `CheckboxWidget::handle_input` と同じ「フォーカス保持中のみ操作可能」という設計
    /// 判断）。値が実際に変化した場合のみ `cached_value` を更新して
    /// `Event::ButtonClicked` を返す（境界（`min`/`max`）到達後の冗長な
    /// `Increase`/`Decrease` はイベントを発火しない）。
    ///
    /// 値が変化してコミットする場合は進行中の `drag`（[`DragState`]）を破棄する。
    /// キーボード操作は [`Self::press`] を経由しないため `drag` は既に `None` のはずだが、
    /// 万一ポインタドラッグ中に矢印キー等が割り込んだ場合、`drag` を残したままにすると
    /// 後続の [`Self::cancel_press`] や [`Self::set_disabled`] がここでコミット済みの値を
    /// ドラッグ開始前の古い値（`drag_origin_value`）へ巻き戻し、表示と `S` が非同期になる
    /// （[`super::scroll_view_widget::ScrollViewWidget`] がドラッグ以外の値変更時に drag を
    /// クリアするのと同じ判断、Cursor Bugbot 指摘、Issue #124）。
    pub fn handle_input(&mut self, input: SliderInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        let new_value = match input {
            SliderInput::Increase | SliderInput::Decrease => {
                let step = self
                    .step
                    .unwrap_or((self.max - self.min) * KEYBOARD_STEP_RATIO);
                let delta = if matches!(input, SliderInput::Increase) {
                    step
                } else {
                    -step
                };
                self.snap_to_step(self.cached_value + delta)
            }
            SliderInput::SetToMin => self.min,
            SliderInput::SetToMax => self.max,
        };

        if new_value == self.cached_value {
            return None;
        }
        self.cached_value = new_value;
        self.drag = None;
        Some(Event::ButtonClicked {
            label: self.label.clone(),
        })
    }
}

impl<S: 'static> Widget<S> for Slider<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から値を読み取り、`[min, max]` へクランプして `cached_value` を
    /// 更新する（reconcile フェーズ）。NaN は `min` へフォールバックする（安全側）。
    ///
    /// `self.drag.is_some()`（[`Self::press`] / [`Self::drag`] がコミット前のプレビュー値を
    /// 保持中）の間は同期をスキップする。ドラッグ中は `S` がまだドラッグ開始前の値を
    /// 保持しているため、ここで無条件に上書きすると `cached_value` のプレビューが消え、
    /// 後続の [`Self::release`] がドラッグ開始前の古い値を確定させてしまう
    /// （[`super::text_field::TextFieldWidget`] がフォーカス中の編集バッファを外部更新から
    /// 守るのと同じ判断、Cursor Bugbot 指摘、Issue #124）。未配線の契約はモジュール doc を
    /// 参照。
    fn sync_from_state(&mut self, state: &S) {
        if self.drag.is_some() {
            return;
        }
        let raw = (self.get)(state);
        self.cached_value = if raw.is_nan() {
            self.min
        } else {
            raw.clamp(self.min, self.max)
        };
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = トラック幅 + トラック・ラベル間ギャップ + ラベル幅を `available.width` で
    /// クランプする。高さ = サムサイズと行高の大きい方を `available.height` で
    /// クランプする。マルチバイトは `chars().count()` 基準の近似（`Checkbox` /
    /// `ToggleSwitch` と同じ既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (TRACK_WIDTH + TRACK_LABEL_GAP + label_width).min(available.width);
        let height = THUMB_SIZE.max(APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// `[Rect（トラック）, Rect（フィル済みトラック）, Rect（サム）, Text(値の書式化文字列),
    /// Text(label)]` の描画プリミティブ列を返す。
    ///
    /// ヘッドレス環境でも値の変化がコマンド列（`Text` の内容）の差分として検証できる
    /// 構成とし、`PaintCommand` 自体は拡張しない（モジュール doc 参照）。値テキストは
    /// 内部生成の決め打ち書式（`format!("{value:.1}/{max:.1}")`）で、外部入力文字列を
    /// 経由しない（OWASP A03 対策）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let value_text = format!("{:.1}/{:.1}", self.cached_value, self.max);

        vec![
            PaintCommand::Rect,
            PaintCommand::Rect,
            PaintCommand::Rect,
            PaintCommand::Text(value_text),
            PaintCommand::Text(self.label.clone()),
        ]
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたスライダーを入力対象にしない（安全側の判断）。
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
        AccessibilityRole::Slider
    }

    /// スライダーのラベル文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
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
    /// （`style_tokens::input_style_refs`）を用い、`paint()` と同じ構造（トラック・
    /// フィル・サム・テキストの個数/順序）を保ったまま色・タイポグラフィのみを
    /// 解決済み具体値へ差し替える。解決失敗時は `paint()` の無スタイル列へ
    /// フォールバックする。
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

    /// `WidgetValue::Number` を `[min, max]` へクランプしたうえで `write_fn()` へ
    /// 書き込む（Issue #805 / 親 #804）。`NaN` は信頼境界の入力検証として拒否する
    /// （A03 対策）。`Bool` / `Index` 等の型不一致も `false` を返し、状態を変更しない。
    /// range クランプは拒否ではなく飽和（saturate）とする: egui 等の標準部品は
    /// `[min, max]` 内の値しか返さないため、範囲外の入力は乖離した値に対する
    /// 防御的補正として扱い、`NaN` のような不正値と違って発火自体は成立させる。
    fn apply_value(&self, state: &mut S, value: &WidgetValue) -> bool {
        match value {
            WidgetValue::Number(v) if !v.is_nan() => {
                let clamped = v.clamp(self.min(), self.max());
                (self.write_fn())(state, clamped);
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
        volume: f32,
    }

    fn get_volume(state: &FormState) -> f32 {
        state.volume
    }

    fn set_volume(state: &mut FormState, value: f32) {
        state.volume = value;
    }

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_min_max_and_initial_value_at_min() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.label_text(), "音量");
        assert_eq!(slider.min(), 0.0);
        assert_eq!(slider.max(), 100.0);
        assert_eq!(slider.value(), 0.0);
        assert_eq!(slider.step(), None);
        assert_eq!(slider.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let write_fn = slider.write_fn();
        let mut state = FormState { volume: 0.0 };
        write_fn(&mut state, 42.0);
        assert_eq!(state.volume, 42.0);
    }

    // (b) min > max 入れ替え・非有限値のフォールバック・with_step の異常値無視

    #[test]
    fn bound_swaps_min_and_max_when_inverted() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            100.0,
            0.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.min(), 0.0);
        assert_eq!(slider.max(), 100.0);
    }

    #[test]
    fn bound_falls_back_to_default_range_when_min_is_nan() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            f32::NAN,
            100.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.min(), DEFAULT_MIN);
        assert_eq!(slider.max(), DEFAULT_MAX);
    }

    #[test]
    fn bound_falls_back_to_default_range_when_max_is_infinite() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            f32::INFINITY,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.min(), DEFAULT_MIN);
        assert_eq!(slider.max(), DEFAULT_MAX);
    }

    #[test]
    fn with_step_sets_positive_finite_step() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        )
        .with_step(5.0);

        assert_eq!(slider.step(), Some(5.0));
    }

    #[test]
    fn with_step_ignores_zero_or_negative_step() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        )
        .with_step(-1.0);

        assert_eq!(slider.step(), None);
    }

    #[test]
    fn with_step_ignores_non_finite_step() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        )
        .with_step(f32::NAN);

        assert_eq!(slider.step(), None);
    }

    // (c) sync_from_state の値追従・範囲クランプ・NaN フォールバック

    #[test]
    fn sync_from_state_updates_cached_value_from_get() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 42.0 };

        slider.sync_from_state(&state);

        assert_eq!(slider.value(), 42.0);
    }

    #[test]
    fn sync_from_state_clamps_value_above_max() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 999.0 };

        slider.sync_from_state(&state);

        assert_eq!(slider.value(), 100.0);
    }

    #[test]
    fn sync_from_state_clamps_value_below_min() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: -50.0 };

        slider.sync_from_state(&state);

        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn sync_from_state_falls_back_to_min_when_value_is_nan() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            10.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: f32::NAN };

        slider.sync_from_state(&state);

        assert_eq!(slider.value(), 10.0);
    }

    #[test]
    fn sync_from_state_follows_state_changes_across_calls() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let mut state = FormState { volume: 30.0 };

        slider.sync_from_state(&state);
        assert_eq!(slider.value(), 30.0);

        state.volume = 70.0;
        slider.sync_from_state(&state);
        assert_eq!(slider.value(), 70.0);
    }

    #[test]
    fn sync_from_state_does_not_clobber_drag_preview() {
        // Cursor Bugbot 指摘（Issue #124, Medium, review 4824653465,
        // BUGBOT_BUG_ID: dfc20ebd-7fba-4788-becb-21a521294289）: ドラッグ中は `S` が
        // まだドラッグ開始前の値を保持しているため、reconcile（sync_from_state）を
        // 無条件で適用するとプレビュー値が消え、後続の release が古い値を確定して
        // しまっていた。ドラッグ中の sync_from_state が no-op であることを検証する。
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        // ドラッグ中に S 側が古い値（20.0）のまま reconcile が走っても、プレビューは
        // 上書きされない。
        slider.sync_from_state(&FormState { volume: 20.0 });
        assert_eq!(slider.value(), 50.0);

        let event = slider.release();
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
        assert_eq!(slider.value(), 50.0);

        // release 後（drag == None）は通常どおり reconcile が反映される。
        slider.sync_from_state(&FormState { volume: 75.0 });
        assert_eq!(slider.value(), 75.0);
    }

    #[test]
    fn ratio_reflects_normalized_position() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            200.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 50.0 };

        slider.sync_from_state(&state);

        assert_eq!(slider.ratio(), 0.25);
    }

    #[test]
    fn ratio_is_zero_for_degenerate_range() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            5.0,
            5.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.ratio(), 0.0);
    }

    // (d) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "メモ",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let measurement = slider.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = TRACK_WIDTH + TRACK_LABEL_GAP + (2.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, THUMB_SIZE.max(APPROX_LINE_HEIGHT));
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです非常に長いラベルです",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let measurement = slider.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "あ",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let measurement = slider.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = TRACK_WIDTH + TRACK_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
    }

    // (e) paint

    #[test]
    fn paint_returns_track_fill_thumb_value_text_and_label() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 42.0 };
        slider.sync_from_state(&state);

        let commands = slider.paint(
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("42.0/100.0".to_string()),
                PaintCommand::Text("音量".to_string()),
            ]
        );
    }

    #[test]
    fn paint_value_text_changes_with_value_diff() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let before = slider.paint(
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        slider.sync_from_state(&FormState { volume: 80.0 });

        let after = slider.paint(
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert_ne!(before, after);
        assert!(after.contains(&PaintCommand::Text("80.0/100.0".to_string())));
    }

    // (f) hit_test の半開区間・disabled 時 false

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let hit = slider.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert!(hit);
    }

    #[test]
    fn hit_test_returns_false_when_point_outside_rect() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let hit = slider.hit_test(
            (500.0, 500.0),
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_on_bottom_right_edge() {
        // `Rect::contains` の半開区間仕様（[origin, origin + size)）に合わせ、
        // 右端・下端ちょうどの点は非ヒットとする（隣接ウィジェットとの境界重複を避ける）。
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let hit = slider.hit_test(
            (200.0, 20.0),
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    #[test]
    fn hit_test_returns_false_for_zero_size_at_origin() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        let hit = slider.hit_test(
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
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        slider.set_disabled(true);

        let hit = slider.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    // (g) accessibility_role / clone_box / write_fn / Debug / visual_state

    #[test]
    fn accessibility_role_is_slider() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.accessibility_role(), AccessibilityRole::Slider);
    }

    #[test]
    fn accessibility_label_is_slider_label() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        assert_eq!(slider.accessibility_label(), Some("音量".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 42.0 };
        slider.sync_from_state(&state);

        let mut cloned = slider.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState { volume: 0.0 };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(slider.value(), 42.0);
        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 200.0,
                height: 20.0,
            },
        );
        assert!(
            cloned_commands.contains(&PaintCommand::Text("0.0/100.0".to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn debug_output_contains_cached_value() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 42.0 };
        slider.sync_from_state(&state);

        let output = format!("{slider:?}");

        assert!(output.contains("cached_value"));
        assert!(output.contains("42"));
    }

    #[test]
    fn visual_state_priority_is_delegated_to_interaction_state() {
        use super::super::widget::InteractionInput;

        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );

        slider.interaction.apply(InteractionInput::PointerEntered);
        assert_eq!(slider.visual_state(), VisualState::Hovered);

        slider.set_disabled(true);
        assert_eq!(slider.visual_state(), VisualState::Disabled);
    }

    // (h) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
        );
        let state = FormState { volume: 65.0 };

        slider.sync_from_state(&state);
        let measurement = slider.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = slider.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Rect,
                PaintCommand::Text("65.0/100.0".to_string()),
                PaintCommand::Text("音量".to_string()),
            ]
        );
    }

    // (i) press → drag → release のライフサイクル・jump-to-click（TASK-8.3-S29）

    const TRACK_SIZE: Measurement = Measurement {
        width: 200.0,
        height: 20.0,
    };

    fn new_slider(min: f32, max: f32) -> Slider<FormState> {
        Slider::bound(
            WidgetId::from_raw(1),
            "音量",
            min,
            max,
            get_volume,
            set_volume,
        )
    }

    #[test]
    fn press_jumps_preview_value_to_click_position() {
        let mut slider = new_slider(0.0, 100.0);

        // トラック幅は TRACK_WIDTH(160.0) と size.width(200.0) の小さい方 = 160.0。
        // x=80.0 はトラック中央（比率 0.5）に相当する。
        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 50.0);
    }

    #[test]
    fn press_then_drag_then_release_follows_pointer_and_fires_event() {
        let mut slider = new_slider(0.0, 100.0);

        slider.press((0.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 0.0);

        let changed = slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert!(changed);
        assert_eq!(slider.value(), 100.0);

        let event = slider.release();
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
    }

    #[test]
    fn drag_without_prior_press_is_ignored() {
        let mut slider = new_slider(0.0, 100.0);

        let changed = slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert!(!changed);
        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn release_without_prior_press_does_not_confirm() {
        let mut slider = new_slider(0.0, 100.0);

        let event = slider.release();

        assert_eq!(event, None);
    }

    #[test]
    fn cancel_press_restores_preview_to_drag_origin_value() {
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        slider.cancel_press();

        assert_eq!(slider.value(), 20.0);
        // ドラッグ状態が消えているため、続く drag は無視される。
        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
    }

    #[test]
    fn cancel_press_clears_pressed_interaction_state() {
        // Cursor Bugbot 指摘（Issue #124, Medium）: cancel_press はプレビュー値・drag は
        // クリアするが interaction.pressed を戻さないため、OS 側のポインタキャプチャ
        // キャンセル等の後もスライダーが視覚的に「押下中」のまま残ってしまっていた。
        let mut slider = new_slider(0.0, 100.0);

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert!(slider.interaction_state().pressed);

        slider.cancel_press();

        assert!(!slider.interaction_state().pressed);
    }

    #[test]
    fn cancel_press_without_prior_press_does_not_panic_and_clears_pressed() {
        // ドラッグ未開始（`drag` が `None`）でも `cancel_press` の呼び出しは安全であり、
        // 押下状態解除は常に行われる（`release` の `PointerReleased` 適用が
        // `drag.take()?` より前に行われるのと同じ判断）。
        let mut slider = new_slider(0.0, 100.0);

        slider.cancel_press();

        assert!(!slider.interaction_state().pressed);
    }

    #[test]
    fn press_reaches_max_when_track_fills_entire_narrow_widget() {
        // Cursor Bugbot 指摘（Issue #124, Medium）: `size.width <= TRACK_WIDTH` の
        // 狭いレイアウトでは実効トラック幅（`TRACK_WIDTH.min(size.width)`）がウィジェット
        // 全体の幅と一致するため、`point_to_value` が `max` に対応させる座標
        // （`origin.0 + size.width` ちょうど）が、`press` のウィジェット全体ヒット判定
        // （半開区間）で弾かれ、クリックで `max` に到達できなかった。
        let narrow = Measurement {
            width: 100.0, // TRACK_WIDTH (160.0) 未満のため実効トラック幅 = size.width
            height: 20.0,
        };
        let mut slider = new_slider(0.0, 100.0);

        slider.press((100.0, 0.0), (0.0, 0.0), narrow);

        assert_eq!(slider.value(), 100.0);
        assert_eq!(
            slider.release(),
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
    }

    #[test]
    fn drag_clamps_to_track_bounds_when_point_outside_track() {
        let mut slider = new_slider(0.0, 100.0);
        slider.press((0.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        let changed = slider.drag((-500.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert!(!changed);
        assert_eq!(slider.value(), 0.0);

        let changed = slider.drag((9999.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert!(changed);
        assert_eq!(slider.value(), 100.0);
    }

    #[test]
    fn drag_ignores_non_finite_point() {
        let mut slider = new_slider(0.0, 100.0);
        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        let changed = slider.drag((f32::NAN, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert!(!changed);
        assert_eq!(slider.value(), 50.0);
    }

    #[test]
    fn press_outside_hit_area_does_not_start_drag() {
        let mut slider = new_slider(0.0, 100.0);

        slider.press((500.0, 500.0), (0.0, 0.0), TRACK_SIZE);

        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn press_on_track_right_edge_still_starts_drag_and_jumps_to_max() {
        // トラック右端ちょうど（実効トラック幅 = min(TRACK_WIDTH, size.width) = 160.0）は
        // サムの位置に相当するため、従来どおりドラッグ開始・jump-to-click を許可する。
        let mut slider = new_slider(0.0, 100.0);

        slider.press((160.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 100.0);
        // ドラッグが実際に開始していること（release で確定イベントが飛ぶ）も確認する。
        assert_eq!(
            slider.release(),
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
    }

    #[test]
    fn press_on_label_area_does_not_start_drag_or_jump_to_max() {
        // Cursor Bugbot 指摘（Issue #124, High）: ラベル領域（トラック外）をクリックすると
        // press はヒットするが、point_to_value がトラック幅基準で範囲外を 1.0 にクランプする
        // ため、値が max へ飛んでしまっていた。トラック外の押下はドラッグを開始しないこと、
        // かつプレビュー値も変化しないことを検証する。TRACK_SIZE.width(200.0) はトラック幅
        // (160.0) より広いため、x=180.0 はラベル・ギャップ領域に相当する。
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((180.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 20.0);
        // ドラッグが開始していないため、続く drag も無視される。
        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
    }

    #[test]
    fn press_on_gap_just_past_track_does_not_start_drag() {
        // トラック実効幅（160.0）のすぐ外（161.0）はトラック・ラベル間の隙間に相当し、
        // ドラッグを開始しない（ウィジェット全体のヒットテストは通るが、トラック内判定で
        // 弾かれることを確認する）。
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((161.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 20.0);
        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
    }

    #[test]
    fn press_on_label_area_still_applies_pointer_pressed_interaction() {
        // トラック外の押下でもウィジェット全体としてはヒットしているため、
        // hover/press 相当の視覚状態は更新されるべき（ドラッグ開始のみを拒否する）。
        let mut slider = new_slider(0.0, 100.0);

        slider.press((180.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.visual_state(), VisualState::Pressed);
    }

    #[test]
    fn press_does_nothing_when_disabled() {
        let mut slider = new_slider(0.0, 100.0);
        slider.set_disabled(true);

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 0.0);
        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
    }

    #[test]
    fn set_disabled_true_clears_active_drag_state() {
        let mut slider = new_slider(0.0, 100.0);
        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        slider.set_disabled(true);

        // stale なドラッグ基準が残らないこと（drag 呼び出しが無視される）。
        assert!(!slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
    }

    #[test]
    fn set_disabled_true_restores_preview_to_drag_origin_value() {
        // Cursor Bugbot 指摘（Issue #124, Medium）: disable が drag をクリアするだけでは
        // jump-to-click / ドラッグ中の未コミットプレビュー値が残留してしまう。
        // cancel_press と同じ「ドラッグ開始前の値へ復元する」挙動を検証する。
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        slider.set_disabled(true);

        assert_eq!(slider.value(), 20.0);
    }

    #[test]
    fn set_disabled_true_without_active_drag_keeps_current_value() {
        // ドラッグ中でない場合は cancel_press 相当の復元処理が何もしないため、
        // 直近の確定値がそのまま維持されることを確認する。
        let mut slider = new_slider(0.0, 100.0);
        slider.sync_from_state(&FormState { volume: 65.0 });

        slider.set_disabled(true);

        assert_eq!(slider.value(), 65.0);
    }

    // (j) step スナップ（ドラッグ・キーボード両経路）

    #[test]
    fn press_snaps_to_nearest_step() {
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);

        // x=85.0 -> ratio ≈ 0.531 -> raw value ≈ 53.1 -> snap to 50.0。
        slider.press((85.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 50.0);
    }

    #[test]
    fn snap_to_step_clamps_within_min_max() {
        let mut slider = new_slider(0.0, 95.0).with_step(10.0);

        slider.press((160.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        assert_eq!(slider.value(), 95.0);
    }

    // (k) SliderInput によるキーボード操作

    #[test]
    fn handle_input_increase_uses_step_when_set() {
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);
        slider.apply_input(InteractionInput::FocusGained);

        let event = slider.handle_input(SliderInput::Increase);

        assert_eq!(slider.value(), 10.0);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
    }

    #[test]
    fn handle_input_increase_uses_keyboard_step_ratio_when_step_unset() {
        let mut slider = new_slider(0.0, 100.0);
        slider.apply_input(InteractionInput::FocusGained);

        slider.handle_input(SliderInput::Increase);

        assert_eq!(slider.value(), 100.0 * KEYBOARD_STEP_RATIO);
    }

    #[test]
    fn handle_input_decrease_moves_value_down() {
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.sync_from_state(&FormState { volume: 50.0 });

        slider.handle_input(SliderInput::Decrease);

        assert_eq!(slider.value(), 40.0);
    }

    #[test]
    fn handle_input_set_to_min_and_set_to_max() {
        let mut slider = new_slider(10.0, 90.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.sync_from_state(&FormState { volume: 50.0 });

        slider.handle_input(SliderInput::SetToMin);
        assert_eq!(slider.value(), 10.0);

        slider.handle_input(SliderInput::SetToMax);
        assert_eq!(slider.value(), 90.0);
    }

    #[test]
    fn handle_input_at_max_boundary_does_not_fire_event() {
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.sync_from_state(&FormState { volume: 100.0 });

        let event = slider.handle_input(SliderInput::Increase);

        assert_eq!(event, None);
        assert_eq!(slider.value(), 100.0);
    }

    #[test]
    fn handle_input_returns_none_when_disabled() {
        let mut slider = new_slider(0.0, 100.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.set_disabled(true);

        let event = slider.handle_input(SliderInput::Increase);

        assert_eq!(event, None);
        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn handle_input_returns_none_when_not_focused() {
        let mut slider = new_slider(0.0, 100.0);

        let event = slider.handle_input(SliderInput::Increase);

        assert_eq!(event, None);
        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn handle_input_commit_clears_stale_drag_so_cancel_press_does_not_revert() {
        // Cursor Bugbot 指摘（Issue #124, Medium, review 4824653465,
        // BUGBOT_BUG_ID: f716f30a-0588-4a48-a5fe-503503fa75a6）: ポインタドラッグ中に
        // キーボード操作（handle_input）が割り込んでコミットしても `drag`
        // （`drag_origin_value`）が残ったままだと、後続の cancel_press がキーボードで
        // 確定済みの値をドラッグ開始前の値へ巻き戻してしまっていた。handle_input が
        // 値を変化させた場合は drag を破棄し、続く cancel_press が no-op になることを
        // 検証する。
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        let event = slider.handle_input(SliderInput::Increase);
        assert_eq!(
            event,
            Some(Event::ButtonClicked {
                label: "音量".to_string()
            })
        );
        assert_eq!(slider.value(), 60.0);

        // drag は既に破棄されているため、cancel_press はコミット済みの値を巻き戻さない。
        slider.cancel_press();
        assert_eq!(slider.value(), 60.0);
    }

    #[test]
    fn handle_input_commit_clears_stale_drag_so_set_disabled_does_not_revert() {
        // 上記と同じ根拠（Issue #124, BUGBOT_BUG_ID: f716f30a-0588-4a48-a5fe-503503fa75a6）を
        // `set_disabled(true)`（内部で `cancel_press` を呼ぶ）経路でも検証する。
        let mut slider = new_slider(0.0, 100.0).with_step(10.0);
        slider.apply_input(InteractionInput::FocusGained);
        slider.sync_from_state(&FormState { volume: 20.0 });

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 50.0);

        slider.handle_input(SliderInput::Increase);
        assert_eq!(slider.value(), 60.0);

        slider.set_disabled(true);
        assert_eq!(slider.value(), 60.0);
    }

    // (l) 縮退区間（min == max）で panic せず安全動作する

    #[test]
    fn degenerate_range_press_and_handle_input_do_not_panic() {
        let mut slider = new_slider(5.0, 5.0);
        slider.apply_input(InteractionInput::FocusGained);

        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.value(), 5.0);

        let event = slider.handle_input(SliderInput::Increase);
        assert_eq!(event, None);
        assert_eq!(slider.value(), 5.0);
    }

    // (m) apply_input はポインタ出入り・フォーカスを反映するのみで確定しない

    #[test]
    fn apply_input_does_not_cancel_drag_on_pointer_exited_or_focus_lost() {
        let mut slider = new_slider(0.0, 100.0);
        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);

        slider.apply_input(InteractionInput::PointerExited);
        slider.apply_input(InteractionInput::FocusLost);

        // ポインタキャプチャの意味論により、ドラッグは中断されず追従を続ける。
        assert!(slider.drag((160.0, 0.0), (0.0, 0.0), TRACK_SIZE));
        assert_eq!(slider.value(), 100.0);
    }

    #[test]
    fn apply_input_keeps_pressed_visual_state_during_capture_on_pointer_exited() {
        let mut slider = new_slider(0.0, 100.0);
        slider.press((80.0, 0.0), (0.0, 0.0), TRACK_SIZE);
        assert_eq!(slider.visual_state(), VisualState::Pressed);

        // ドラッグ中に領域外へ出ても、ポインタキャプチャ継続中は pressed 見た目を維持する
        // （drag は中断されないため。Cursor Bugbot 指摘、Issue #124）。
        slider.apply_input(InteractionInput::PointerExited);
        assert!(slider.interaction_state().pressed);
        assert_eq!(slider.visual_state(), VisualState::Pressed);

        // ドラッグ中でない通常の PointerExited では従来どおり pressed を解除する。
        slider.release();
        slider.apply_input(InteractionInput::PointerPressed);
        slider.apply_input(InteractionInput::PointerExited);
        assert!(!slider.interaction_state().pressed);
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let slider: Slider<FormState> = Slider::bound(
            WidgetId::from_raw(200),
            "音量",
            0.0,
            100.0,
            get_volume,
            set_volume,
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
            slider.paint_styled((0.0, 0.0), size, &empty_resolver),
            slider.paint((0.0, 0.0), size)
        );
    }
}
