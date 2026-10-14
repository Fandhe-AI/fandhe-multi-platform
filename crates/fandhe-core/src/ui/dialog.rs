//! ダイアログ・モーダルウィジェット `Dialog<S>`（TASK-8.3-S46 / REQ-8、Issue #141）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の分解
//! sub-issue（46/60）。ダイアログ・モーダルの API 設計・基本描画を担い、
//! `Widget<S>` トレイト（TASK-8.3-S2 / #97）と、先行ウィジェットが確立した
//! 以下の 2 パターンを組み合わせて [`Dialog`] を新設する。
//!
//! - 制御型の状態バインディング（[`super::checkbox::Checkbox`] S19 / [`super::slider::Slider`]
//!   S28）: 開閉状態の SSOT はアプリ状態 `S` に置き、[`crate::state::ReadFn`] 経由の
//!   [`Widget::sync_from_state`] のみが表示キャッシュ `cached_open` を更新する。
//! - コンテナ型の単一子コンテンツ保持（[`super::scroll_view::ScrollView`] S40）:
//!   任意の `content: Option<Box<dyn Widget<S>>>` を保持し、`sync_from_state` は
//!   子へ委譲するのみで `S` へ直接触れない。子計測の緩和制約が必要な場合は
//!   `ScrollView` と同じ根拠の有限定数 [`MAX_CONTENT_EXTENT`] を使う
//!   （`f32::MAX` を子側の乗算にそのまま渡すとオーバーフローで `inf` が伝播しうるため）。
//!
//! - `cached_open == false` のとき、閉じたダイアログは「存在しないもの」として扱う:
//!   `measure` はゼロ寸法、`paint` は空 `Vec`、`hit_test` は常に `false` を返す。
//! - `cached_open == true` のとき、`measure` が返す確定寸法（[`Widget::paint`] /
//!   [`Widget::hit_test`] へレイアウトエンジンから渡される `size` の根拠）は
//!   modal か否かで異なる（`paint` が modal 時のみ scrim の `Rect` を追加で
//!   描画する事実と整合させる。旧実装は modal でもパネルサイズのみを返し、
//!   `hit_test` の「modal 時は `size` 全域」という契約と不整合を起こして
//!   いた。PR #386 Bugbot 指摘・#141 で修正）:
//!   - **modal**: scrim がビューポート全域を覆う契約のため、`measure` も
//!     `available` を正規化した値をそのまま返す（[`Self::panel_size`] は
//!     パネル矩形のみに使い、scrim 側には使わない）。
//!   - **非 modal**: パネル固定寸法プレースホルダ（[`PANEL_WIDTH`]）とタイトル・
//!     本文の行数からの近似高さ（[`APPROX_LINE_HEIGHT`]、`Slider` / `Checkbox`
//!     の先行定数値と揃える）を `available` へクランプした [`Self::panel_size`]
//!     を返す。
//!   - `paint`: `[（modal 時のみ）Rect（スクリム）, Rect（パネル）, Text(title), Text(body)]`
//!     に続けて `content.paint(..)` を連結する。`PaintCommand` は拡張しない
//!     （`Text(String)` / `Rect` の 2 種のまま、3 バックエンドのプレースホルダ契約に
//!     波及するため安全側に倒す判断は `ScrollView` / `Slider` と同じ）。子コンテンツの
//!     寸法は modal 時の `size`（ビューポート全域）ではなく常に [`Self::panel_size`]
//!     でクランプする（パネル内に収める意図のため、scrim 分の拡大を子へ波及させない）。
//!   - `hit_test`: modal 時は `size` 全域（スクリムを含む＝背景クリック捕捉の S47 への
//!     布石。`measure` が返す確定寸法と一致する）、非 modal 時は [`Self::panel_size`]
//!     の矩形のみを対象とする。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) 開閉イベント配線（背景クリック・Esc・閉じるボタンによる `WriteFn<S, bool>`
//! 書き込み）・フォーカストラップ（S47 / #142）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S48 / #143）、
//! (c) 描画クリッピング・z-order（`PaintCommand` に座標・レイヤが表現できないため。
//! `ScrollView` S40 と同じ判断で拡張を見送り）、
//! (d) `AlertDialog` ロール分岐・アニメーション（フェード等）、
//! (e) reconcile パス未配線の既知契約（`widget.rs` モジュール doc の契約を変更しない）。

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};
use crate::state::ReadFn;

/// 単一行の高さ近似（論理ピクセル相当）。[`super::slider::Slider`] /
/// [`super::checkbox::Checkbox`] の `APPROX_LINE_HEIGHT` と同じ値を採用し、
/// 測定近似の基準を揃える（cosmic-text 等の実テキスト計測エンジン導入前の
/// プレースホルダ定数、折り返し・複数行はスコープ外）。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// パネル（ダイアログ本体）の固定幅プレースホルダ（論理ピクセル相当）。
const PANEL_WIDTH: f32 = 320.0;

/// タイトル・本文の周囲に確保する余白（論理ピクセル相当）。
///
/// TASK-8.4-S15（#171）: 導出元をリテラルからスペーシングトークンの既定スケール
/// （`spacing.md` = 16px）へ差し替えた。`SpacingValue::value()` の `const fn` 化
/// （同 issue）により定数コンテキストから呼び出せる。値そのものは不変
/// （16.0）のため既存の寸法検算・テストへの挙動変化はない。
const PANEL_PADDING: f32 = crate::token::spacing_scale::MD.value();

/// 子計測を緩和する際の制約上限（論理ピクセル相当のプレースホルダ定数）。
///
/// [`super::scroll_view::ScrollView`] の `MAX_CONTENT_EXTENT` と同じ根拠
/// （`f32::MAX` を子側の乗算にそのまま渡すとオーバーフローで `inf` が伝播しうるため、
/// 実用上十分大きい有限値に留める、OWASP A04 相当の数値健全性対策）。
///
/// **既知の制約**: `Dialog` を（`ScrollView` 等の）緩和制約の子として計測すると、
/// modal 時の [`Widget::measure`][super::widget::Widget::measure] は
/// `available`（≈本定数）をそのまま scrim 全域として返す。現実的なビューポート
/// サイズより 1 桁以上大きい値になり得るが、`Dialog` は通常オーバーレイとして
/// トップレベルから計測される想定であり、ネストされた緩和制約下での挙動は
/// スコープ外（プレースホルダレイアウトエンジンの既知の限界）とする。
const MAX_CONTENT_EXTENT: f32 = 1_048_576.0;

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// パネル寸法・`available` の数値健全性確保のため（OWASP A04 相当）、
/// [`super::scroll_view`] の同名ヘルパーと同じ丸め方針を採用する。
fn normalize_non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// ダイアログ・モーダルウィジェット（TASK-8.3-S46、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（[`ScrollView`][super::scroll_view::ScrollView] と
/// 同じく、状態型 `S` を所有していないため `derive` マクロが自動付与する
/// `S: Debug` / `S: Clone` という不要な境界を避ける。`Clone` は
/// `content` を [`Widget::clone_box`] 経由で複製する）。
pub struct Dialog<S> {
    id: WidgetId,
    title: String,
    body: String,
    /// モーダル（背景操作ブロック・スクリム描画）か非モーダルかの区分。既定は `true`
    /// （[`Self::bound`] 参照）。
    modal: bool,
    /// 開閉状態のアプリ状態 `S` からの読み取り経路（REQ-13、環境キャプチャ不可の
    /// 関数ポインタに限定）。開閉状態の SSOT は常に `S` 側に置き、本ウィジェットは
    /// キャッシュのみを保持する（`Checkbox` / `Slider` と同じ制御型の設計判断）。
    open: ReadFn<S, bool>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`is_open()` が返す値）。
    cached_open: bool,
    /// 任意の単一子コンテンツ（[`super::scroll_view::ScrollView`] のコンテナパターンを
    /// 踏襲。ダイアログは子コンテンツを必須としないため `Option` で保持する）。
    content: Option<Box<dyn Widget<S>>>,
    /// hover/pressed/disabled/focus。実際の開閉イベント配線は S47 のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for Dialog<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // title / body は UI 表示文字列でありユーザーの秘密情報に該当しないため、
        // `Slider` の label 等と同様に `Debug` へそのまま含める。
        f.debug_struct("Dialog")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("body", &self.body)
            .field("modal", &self.modal)
            .field("cached_open", &self.cached_open)
            .field("content", &self.content)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for Dialog<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            title: self.title.clone(),
            body: self.body.clone(),
            modal: self.modal,
            open: self.open,
            cached_open: self.cached_open,
            content: self.content.as_ref().map(|c| c.clone_box()),
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> Dialog<S> {
    /// 状態 `S` に束縛したダイアログを作る（ビルダー形式の起点）。
    ///
    /// `open` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`]）と代入互換の
    /// 関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで拒否する）。
    /// 既定はモーダル（[`Self::with_modal`] で変更可能）・子コンテンツなし
    /// （[`Self::with_content`] で設定できる）。
    pub fn bound(
        id: WidgetId,
        title: impl Into<String>,
        body: impl Into<String>,
        open: ReadFn<S, bool>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            body: body.into(),
            modal: true,
            open,
            cached_open: false,
            content: None,
            interaction: InteractionState::default(),
        }
    }

    /// モーダル / 非モーダルを設定するビルダーメソッド（既定は `true`）。
    pub fn with_modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// 単一の子コンテンツを設定するビルダーメソッド（[`ScrollView::new`]
    /// [super::scroll_view::ScrollView::new] と同型の単一子コンテナパターン）。
    pub fn with_content(mut self, content: impl Widget<S> + 'static) -> Self {
        self.content = Some(Box::new(content));
        self
    }

    /// 表示文言（タイトル）を返す。
    pub fn title_text(&self) -> &str {
        &self.title
    }

    /// 表示文言（本文）を返す。
    pub fn body_text(&self) -> &str {
        &self.body
    }

    /// モーダルかどうかを返す（`bound` / `with_modal` 時点で確定済み）。
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// `cached_open`（直近の [`Widget::sync_from_state`] が読み取った開閉状態）を返す。
    pub fn is_open(&self) -> bool {
        self.cached_open
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// タイトル・本文の行数から近似したパネル高さ（論理ピクセル相当）。
    ///
    /// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ実装であり、
    /// 折り返し・複数行はスコープ外（`Slider` / `Checkbox` と同じ既知の制約）。
    /// タイトル・本文それぞれ 1 行として近似し、余白（[`PANEL_PADDING`]）を加える。
    fn approx_panel_height(&self) -> f32 {
        APPROX_LINE_HEIGHT * 2.0 + PANEL_PADDING * 2.0
    }

    /// 子コンテンツの計測に使う、[`MAX_CONTENT_EXTENT`] へ緩和した制約を組み立てる
    /// （`ScrollView::relaxed_constraint` と同じ根拠。`f32::MAX` を直接渡さない）。
    fn relaxed_child_constraint() -> Measurement {
        Measurement {
            width: MAX_CONTENT_EXTENT,
            height: MAX_CONTENT_EXTENT,
        }
    }

    /// パネル（ダイアログ本体、scrim を含まない）矩形を `size` へクランプして返す。
    ///
    /// `measure` の非 modal 分岐・`hit_test` の非 modal 分岐・`paint` の子コンテンツ
    /// クランプの 3 箇所が共有する「パネル矩形」の単一の計算根拠（PR #386 Bugbot
    /// 指摘対応。以前はこの 3 箇所が別々に計算しており、modal 時の `measure` が
    /// scrim 全域を返すよう変更した際に子クランプだけ取り残されると、`content`
    /// がパネルを越えて無制限に広がる回帰を招くため、ここへ集約する）。
    fn panel_size(&self, size: Measurement) -> Measurement {
        Measurement {
            width: PANEL_WIDTH.min(size.width),
            height: self.approx_panel_height().min(size.height),
        }
    }
}

impl<S: 'static> Widget<S> for Dialog<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から開閉状態を読み取り `cached_open` を更新し（reconcile
    /// フェーズ）、子コンテンツへ `sync_from_state` を委譲する
    /// （[`ScrollView::sync_from_state`][super::scroll_view::ScrollView::sync_from_state]
    /// と同じコンテナ委譲パターン。ダイアログ自身は `S` へ直接触れない）。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_open = (self.open)(state);
        if let Some(content) = self.content.as_mut() {
            content.sync_from_state(state);
        }
    }

    /// 閉状態はゼロ寸法（存在しないものとして扱う）。開状態は modal か否かで
    /// 異なる（モジュール doc 参照。`hit_test` へ渡される確定寸法の根拠と
    /// 一致させるための設計、PR #386 Bugbot 指摘対応）:
    /// - modal: scrim がビューポート全域を覆う契約のため、正規化した
    ///   `available` をそのまま返す。
    /// - 非 modal: [`Self::panel_size`]（パネル固定寸法プレースホルダを
    ///   `available` へクランプした値）を返す。
    ///
    /// 非有限値・負値は安全側（`0.0`）へ丸める（OWASP A04 相当の数値健全性対策）。
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

    /// 閉状態は空 `Vec`（存在しないものとして扱う）。開状態は
    /// `[（modal 時のみ）Rect（スクリム）, Rect（パネル）, Text(title), Text(body)]` に
    /// `content.paint(..)` を連結した列を返す。`PaintCommand` は拡張しない
    /// （モジュール doc 参照）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
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

        if let Some(content) = self.content.as_ref() {
            let relaxed = Self::relaxed_child_constraint();
            let content_size = content.measure(relaxed);
            // modal 時の `size` は scrim 分（ビューポート全域）に拡大されているため
            // （`measure` 参照）、子コンテンツは常に `panel_size` でクランプし、
            // scrim の拡大を子へ波及させない（PR #386 Bugbot 指摘対応）。
            commands.extend(content.paint(origin, content_size.min(self.panel_size(size))));
        }

        commands
    }

    /// 閉状態は常に `false`（存在しないものとして扱う）。開状態は modal 時
    /// `size` 全域（スクリムを含む＝背景クリック捕捉の S47 への布石）、非 modal 時は
    /// パネル矩形のみを対象とする。`interaction.disabled` の間は常に `false` を返す
    /// （先行ウィジェットと同じ安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する。
    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        if !self.cached_open || self.interaction.disabled {
            return false;
        }

        let target_size = if self.modal {
            size
        } else {
            self.panel_size(size)
        };

        point.0 >= origin.0
            && point.0 < origin.0 + target_size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + target_size.height
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
}

/// [`Measurement`] 同士の成分ごとの最小値を返す小さなヘルパー
/// （`content.paint` へ渡す寸法をダイアログ自身の確定寸法でクランプするために使う）。
trait MeasurementMin {
    fn min(self, other: Measurement) -> Measurement;
}

impl MeasurementMin for Measurement {
    fn min(self, other: Measurement) -> Measurement {
        Measurement {
            width: self.width.min(other.width),
            height: self.height.min(other.height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::label::Label;

    struct FormState {
        dialog_open: bool,
    }

    fn get_dialog_open(state: &FormState) -> bool {
        state.dialog_open
    }

    fn make_dialog() -> Dialog<FormState> {
        Dialog::bound(
            WidgetId::from_raw(1),
            "確認",
            "この操作を実行しますか？",
            get_dialog_open,
        )
    }

    fn sized(width: f32, height: f32) -> Measurement {
        Measurement { width, height }
    }

    // (a) 構築直後の既定値

    #[test]
    fn bound_constructs_with_title_body_and_closed_default() {
        let dialog = make_dialog();

        assert_eq!(dialog.title_text(), "確認");
        assert_eq!(dialog.body_text(), "この操作を実行しますか？");
        assert!(dialog.is_modal());
        assert!(!dialog.is_open());
        assert_eq!(dialog.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_interaction_state_is_all_false() {
        let dialog = make_dialog();
        let interaction = dialog.interaction_state();

        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    #[test]
    fn with_modal_overrides_default() {
        let dialog = make_dialog().with_modal(false);
        assert!(!dialog.is_modal());
    }

    // (b) 閉状態: ゼロ寸法・空 paint・hit_test false

    #[test]
    fn closed_dialog_measures_to_zero() {
        let dialog = make_dialog();
        let measurement = dialog.measure(sized(800.0, 600.0));
        assert_eq!(measurement.width, 0.0);
        assert_eq!(measurement.height, 0.0);
    }

    #[test]
    fn closed_dialog_paints_nothing() {
        let dialog = make_dialog();
        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));
        assert!(commands.is_empty());
    }

    #[test]
    fn closed_dialog_hit_test_is_always_false() {
        let dialog = make_dialog();
        assert!(!dialog.hit_test((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)));
    }

    // (c) 開状態（modal）: paint 列・hit_test 全域

    #[test]
    fn open_modal_dialog_paints_scrim_panel_title_and_body() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });

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
    fn open_modal_dialog_hit_test_covers_full_size() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });

        let size = sized(800.0, 600.0);
        assert!(dialog.hit_test((0.0, 0.0), (0.0, 0.0), size));
        assert!(dialog.hit_test((799.0, 599.0), (0.0, 0.0), size));
        assert!(!dialog.hit_test((800.0, 600.0), (0.0, 0.0), size));
    }

    // (d) 開状態（非 modal）: スクリムなし・hit_test はパネル矩形のみ

    #[test]
    fn open_non_modal_dialog_paints_without_scrim() {
        let mut dialog = make_dialog().with_modal(false);
        dialog.sync_from_state(&FormState { dialog_open: true });

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

    #[test]
    fn open_non_modal_dialog_hit_test_is_limited_to_panel_rect() {
        let mut dialog = make_dialog().with_modal(false);
        dialog.sync_from_state(&FormState { dialog_open: true });

        let size = sized(800.0, 600.0);
        // パネル矩形内（左上近傍）はヒットする。
        assert!(dialog.hit_test((10.0, 10.0), (0.0, 0.0), size));
        // パネル矩形の外側（右下遠方）はヒットしない。
        assert!(!dialog.hit_test((700.0, 500.0), (0.0, 0.0), size));
    }

    // (e) sync_from_state による cached_open の遷移・content への委譲

    #[test]
    fn sync_from_state_transitions_open_closed_open() {
        let mut dialog = make_dialog();
        let mut state = FormState { dialog_open: true };

        dialog.sync_from_state(&state);
        assert!(dialog.is_open());

        state.dialog_open = false;
        dialog.sync_from_state(&state);
        assert!(!dialog.is_open());

        state.dialog_open = true;
        dialog.sync_from_state(&state);
        assert!(dialog.is_open());
    }

    #[test]
    fn with_content_appends_child_paint_when_open() {
        let mut dialog =
            make_dialog().with_content(Label::bound(WidgetId::from_raw(2), |state: &FormState| {
                if state.dialog_open {
                    "子".to_string()
                } else {
                    String::new()
                }
            }));
        dialog.sync_from_state(&FormState { dialog_open: true });

        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));

        assert_eq!(commands.len(), 5);
        assert_eq!(commands[4], PaintCommand::Text("子".to_string()));
    }

    #[test]
    fn sync_from_state_delegates_to_content() {
        let mut dialog =
            make_dialog().with_content(Label::bound(WidgetId::from_raw(3), |state: &FormState| {
                if state.dialog_open {
                    "開".to_string()
                } else {
                    "閉".to_string()
                }
            }));

        dialog.sync_from_state(&FormState { dialog_open: false });
        let commands = {
            // 閉状態のダイアログ自体は空 paint を返すため、子の同期結果は
            // 開状態にしてから観測する。
            let mut opened = dialog.clone();
            opened.sync_from_state(&FormState { dialog_open: true });
            opened.paint((0.0, 0.0), sized(800.0, 600.0))
        };
        assert_eq!(commands[4], PaintCommand::Text("開".to_string()));
    }

    // (f) 数値健全性: 非有限・負の available

    #[test]
    fn measure_handles_non_finite_and_negative_available() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });

        let non_finite = dialog.measure(sized(f32::NAN, f32::INFINITY));
        assert!(non_finite.width.is_finite());
        assert!(non_finite.height.is_finite());
        assert_eq!(non_finite.width, 0.0);

        let negative = dialog.measure(sized(-10.0, -10.0));
        assert_eq!(negative.width, 0.0);
        assert_eq!(negative.height, 0.0);
    }

    #[test]
    fn measure_clamps_to_small_available_space() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });

        let measurement = dialog.measure(sized(10.0, 5.0));
        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    // (f') measure と hit_test の確定寸法の整合性
    // （PR #386 Bugbot 指摘対応: measure が返す確定寸法をそのまま hit_test へ
    // 渡すレイアウトエンジンの利用パターンを再現し、modal 時の scrim が
    // ビューポート全域をカバーできることをリテラル値ではなくパイプで検証する）。

    #[test]
    fn modal_measure_result_covers_full_viewport_in_hit_test() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });

        let available = sized(800.0, 600.0);
        let measured = dialog.measure(available);

        // scrim 全域（available 全体）をカバーするため、対角の隅までヒットする。
        assert!(dialog.hit_test((0.0, 0.0), (0.0, 0.0), measured));
        assert!(dialog.hit_test((799.0, 599.0), (0.0, 0.0), measured));
        assert!(!dialog.hit_test((800.0, 600.0), (0.0, 0.0), measured));
    }

    #[test]
    fn non_modal_measure_result_limits_hit_test_to_panel_rect() {
        let mut dialog = make_dialog().with_modal(false);
        dialog.sync_from_state(&FormState { dialog_open: true });

        let available = sized(800.0, 600.0);
        let measured = dialog.measure(available);

        // パネル矩形内はヒットし、パネルを越えた領域（measured の外側）はヒットしない。
        assert!(dialog.hit_test((10.0, 10.0), (0.0, 0.0), measured));
        assert!(!dialog.hit_test((700.0, 500.0), (0.0, 0.0), measured));
        assert!(measured.width < available.width);
        assert!(measured.height < available.height);
    }

    // (g) InteractionState（disabled 時の hit_test 無効化）

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_open_and_inside() {
        let mut dialog = make_dialog();
        dialog.sync_from_state(&FormState { dialog_open: true });
        dialog.set_disabled(true);

        assert!(!dialog.hit_test((10.0, 10.0), (0.0, 0.0), sized(800.0, 600.0)));
    }

    #[test]
    fn set_disabled_transitions_visual_state() {
        let mut dialog = make_dialog();
        assert_eq!(dialog.visual_state(), VisualState::Idle);

        dialog.set_disabled(true);
        assert_eq!(dialog.visual_state(), VisualState::Disabled);
    }

    // (h) accessibility_role / clone_box / Debug / Send+Sync

    #[test]
    fn accessibility_role_is_dialog() {
        let dialog = make_dialog();
        assert_eq!(dialog.accessibility_role(), AccessibilityRole::Dialog);
    }

    #[test]
    fn clone_box_preserves_state_and_content_independently() {
        let mut dialog =
            make_dialog().with_content(Label::bound(WidgetId::from_raw(4), |state: &FormState| {
                if state.dialog_open {
                    "A".to_string()
                } else {
                    "B".to_string()
                }
            }));
        dialog.sync_from_state(&FormState { dialog_open: true });

        let mut cloned = dialog.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        cloned.sync_from_state(&FormState { dialog_open: false });

        // clone 元は影響を受けない（独立性の検証）。
        assert!(dialog.is_open());
        let commands = dialog.paint((0.0, 0.0), sized(800.0, 600.0));
        assert_eq!(commands[4], PaintCommand::Text("A".to_string()));
    }

    #[test]
    fn debug_output_does_not_panic() {
        let dialog = make_dialog();
        let output = format!("{dialog:?}");
        assert!(output.contains("Dialog"));
    }

    #[test]
    fn dialog_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Dialog<FormState>>();
    }

    // (i) Box<dyn Widget<S>> への格納（object safety）

    #[test]
    fn dialog_can_be_boxed_as_dyn_widget() {
        let dialog: Box<dyn Widget<FormState>> = Box::new(make_dialog());
        assert_eq!(dialog.accessibility_role(), AccessibilityRole::Dialog);
    }
}
