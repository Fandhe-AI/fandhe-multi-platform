//! スクロールビューウィジェット `ScrollView<S>`（TASK-8.3-S40 / REQ-8 / REQ-9、
//! Issue #135）。
//!
//! 親 issue #95（TASK-8.3「ウィジェットセット新規開発」、60 sub-issue）の 40/60。
//! スクロールビューの 3 工程（S40: API 設計・基本描画 → S41: 状態・イベント処理（#136）→
//! S42: テスト・カタログ登録（#137））のうち第 1 工程であり、`Widget<S>` トレイト
//! （TASK-8.3-S2 / #97）と、先行ウィジェット（[`super::label::Label`] S10 /
//! [`super::slider::Slider`] S28 / [`super::image::Image`] S34 等）が確立したパターンを
//! 踏襲し、`Widget<S>` を実装する具体型 [`ScrollView`] を新設して基本描画
//! （sync_from_state / measure / paint / hit_test / accessibility_role）まで実装する。
//!
//! 本ウィジェットセット初のコンテナ型ウィジェットであり、単一の子コンテンツ
//! （`Box<dyn Widget<S>>`）をスクロール可能なビューポート内に配置する。複数子の合成
//! （スタック・グリッド）は後続タスク（TASK-8.3-S43 系列）のスコープとし、
//! 単一子に限定する。
//!
//! 既存 [`super::scroll`]（TASK-5.5-S5 / #88、リストスクロール幾何ヘルパー）とは別物
//! （ファイル名を `scroll_view` とし名前衝突を避ける）。オフセットのクランプは
//! [`super::scroll::clamp_scroll_offset`] を再利用し、丸めロジックを二重管理しない
//! （[`ScrollView::effective_offset`] 参照）。
//!
//! - オフセット・寸法の非有限値（NaN/inf）・負値は構築時・使用時に安全側（`0.0`）へ
//!   丸め、ゼロ除算・inf 伝播・panic を排除する（OWASP A04 相当、`.claude/rules/security.md`）。
//! - 子計測の緩和制約には `f32::MAX` ではなく有限定数 [`MAX_CONTENT_EXTENT`] を使う。
//!   `f32::MAX` を子側の乗算にそのまま渡すとオーバーフローで `inf` が伝播しうるため、
//!   安全側に小さい（それでも十分大きい）有限値を採用する。
//! - `PaintCommand` は拡張しない（`Text(String)` / `Rect` の 2 種のまま、3 バックエンドの
//!   プレースホルダ契約に波及するため安全側に倒す判断は先行ウィジェットと同じ）。
//!   描画列は `[Rect（ビューポート枠）] + content.paint(..) + （オーバーフローする軸ごとに）
//!   Rect（スクロールバーサム）` とする。自前の `Text` は一切出力しない（外部由来文字列を
//!   ヘッドレス出力へ流さない、OWASP A03 対策）。
//! - 描画クリッピング（ビューポート外へはみ出す子コンテンツの視覚的な切り取り）は
//!   `PaintCommand` に座標・クリップ矩形が表現できないため扱わない
//!   （スコープ外 (c) 参照）。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: `render_headless`
//! （`fandhe-core`）・`convert_node`（`fandhe-web-dom`）・`to_snapshot_node`
//! （`fandhe-swiftui-bridge`）は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
//! 要求する [`Widget::sync_from_state`] を呼ぶ経路が存在しない。この契約を
//! 変更しない。検証は `Widget` メソッドの直接呼び出しで行う。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! (a) ホイール・ドラッグ・キーボードによるスクロール入力とオフセット更新の実配線
//! （S41 / #136）、
//! (b) `WidgetCatalog` 登録・テスト網羅拡充（S42 / #137）、
//! (c) 描画クリッピング（`PaintCommand` 拡張は 3 バックエンド契約に波及するため
//! 安全側に見送り）、
//! (d) 慣性スクロール・スクロールバーの掴みドラッグ・複数子コンテンツ
//! （スタックレイアウトとの合成で扱う）、
//! (e) reconcile パス未配線の既知契約（既存契約を変更しない）。

use super::scroll::clamp_scroll_offset;
use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, VisualState, Widget, WidgetId,
};

/// 子計測を緩和する際の制約上限（論理ピクセル相当のプレースホルダ定数）。
///
/// `f32::MAX` を直接使うと子ウィジェット側の乗算（例: `Image::measure` の
/// アスペクト比計算）でオーバーフローし `inf` が伝播しうるため、実用上十分大きい
/// 有限値（約 100 万論理ピクセル）に留める（OWASP A04 相当の数値健全性対策）。
const MAX_CONTENT_EXTENT: f32 = 1_048_576.0;

/// スクロール可能な軸の指定（本 issue で新設）。既定は [`ScrollAxis::Vertical`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollAxis {
    /// 縦方向のみスクロール可能（既定）。
    #[default]
    Vertical,
    /// 横方向のみスクロール可能。
    Horizontal,
    /// 縦横両方向にスクロール可能。
    Both,
}

/// 非有限値（NaN/inf）または負値を `0.0` へ丸める。
///
/// [`super::scroll`] の `normalize_non_negative_finite` と同じ安全側の丸め方針を
/// 本モジュール内のオフセット正規化にも適用するための小さなヘルパー
/// （0.0 自体は許容するため `> 0.0` ではなく `>= 0.0` で判定する点が
/// `scroll::normalize_non_negative_finite` との違い）。
fn normalize_offset_component(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// スクロールビューウィジェット（TASK-8.3-S40、`Widget<S>` 実装型）。
///
/// 本ウィジェットセット初のコンテナ型であり、単一の子コンテンツ
/// （`Box<dyn Widget<S>>`）をスクロール可能なビューポート内に配置する。
///
/// `Debug` / `Clone` は手動実装する（[`Image`][super::image::Image] 等と同じく、
/// 状態型 `S` を所有していないため `derive` マクロが自動付与する
/// `S: Debug` / `S: Clone` という不要な境界を避ける。`Clone` は
/// [`Widget::clone_box`] 経由で子コンテンツも複製する）。
pub struct ScrollView<S> {
    id: WidgetId,
    content: Box<dyn Widget<S>>,
    axis: ScrollAxis,
    /// 論理スクロールオフセット（`(x, y)`）。構築時は `(0.0, 0.0)`。
    /// 非有限値・負値は [`Self::set_offset`] が構築後も安全側へ丸める
    /// （[`normalize_offset_component`] 参照）。実際の上限クランプ
    /// （コンテンツ高さ・ビューポート実寸に依存）は [`Self::effective_offset`] が行う。
    offset: (f32, f32),
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for ScrollView<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollView")
            .field("id", &self.id)
            .field("content", &self.content)
            .field("axis", &self.axis)
            .field("offset", &self.offset)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for ScrollView<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            content: self.content.clone_box(),
            axis: self.axis,
            offset: self.offset,
            interaction: self.interaction,
        }
    }
}

impl<S: 'static> ScrollView<S> {
    /// 単一の子コンテンツを持つスクロールビューを構築する。既定の軸は
    /// [`ScrollAxis::Vertical`]、オフセットは `(0.0, 0.0)`。
    pub fn new(id: WidgetId, content: impl Widget<S> + 'static) -> Self {
        Self {
            id,
            content: Box::new(content),
            axis: ScrollAxis::default(),
            offset: (0.0, 0.0),
            interaction: InteractionState::default(),
        }
    }

    /// スクロール可能な軸を設定するビルダーメソッド。
    pub fn with_axis(mut self, axis: ScrollAxis) -> Self {
        self.axis = axis;
        self
    }

    /// 論理スクロールオフセットを設定する。
    ///
    /// 非有限値（NaN/inf）・負値は `0.0` へ丸めて保持する
    /// （[`normalize_offset_component`] 参照）。コンテンツ・ビューポート実寸に
    /// 応じた上限クランプは描画に使う直前に [`Self::effective_offset`] が行うため、
    /// ここでは非負・有限の保証のみを行う。
    pub fn set_offset(&mut self, x: f32, y: f32) {
        self.offset = (normalize_offset_component(x), normalize_offset_component(y));
    }

    /// 現在保持している生のオフセット（[`Self::set_offset`] で設定した値。
    /// 上限クランプ前）を返す（テスト・観測用）。
    pub fn raw_offset(&self) -> (f32, f32) {
        self.offset
    }

    /// 描画に使う、上限クランプ済みのオフセットを返す。
    ///
    /// `content.measure` の結果（緩和した制約下でのコンテンツ寸法）と `viewport`
    /// から、[`super::scroll::clamp_scroll_offset`] を軸ごとに適用する。
    /// スクロール対象外の軸（`axis` に含まれない方向）は常に `0.0` に固定する
    /// （縦スクロールのみのビューで横方向にコンテンツがずれて見える不整合を防ぐ）。
    pub fn effective_offset(&self, viewport: Measurement) -> (f32, f32) {
        let relaxed = self.relaxed_constraint(viewport);
        let content_size = self.content.measure(relaxed);

        let x = if self.scrolls_horizontal() {
            clamp_scroll_offset(self.offset.0, content_size.width, viewport.width)
        } else {
            0.0
        };
        let y = if self.scrolls_vertical() {
            clamp_scroll_offset(self.offset.1, content_size.height, viewport.height)
        } else {
            0.0
        };
        (x, y)
    }

    /// 軸ごとのオーバーフロー判定（コンテンツ寸法がビューポートを超えるか）。
    /// スクロールバー描画の条件・テスト観測に使う。
    pub fn overflows(&self, viewport: Measurement) -> (bool, bool) {
        let relaxed = self.relaxed_constraint(viewport);
        let content_size = self.content.measure(relaxed);
        let overflows_x = self.scrolls_horizontal() && content_size.width > viewport.width;
        let overflows_y = self.scrolls_vertical() && content_size.height > viewport.height;
        (overflows_x, overflows_y)
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// disabled フラグを切り替える。
    pub fn set_disabled(&mut self, disabled: bool) {
        self.interaction.set_disabled(disabled);
    }

    /// `axis` が横方向のスクロールを許容するか。
    fn scrolls_horizontal(&self) -> bool {
        matches!(self.axis, ScrollAxis::Horizontal | ScrollAxis::Both)
    }

    /// `axis` が縦方向のスクロールを許容するか。
    fn scrolls_vertical(&self) -> bool {
        matches!(self.axis, ScrollAxis::Vertical | ScrollAxis::Both)
    }

    /// 子コンテンツの計測に使う、スクロール軸方向を [`MAX_CONTENT_EXTENT`] へ
    /// 緩和した制約を組み立てる。非スクロール軸は `viewport` の値をそのまま使う
    /// （ビューポート幅に収まるようにする制約）。
    fn relaxed_constraint(&self, viewport: Measurement) -> Measurement {
        Measurement {
            width: if self.scrolls_horizontal() {
                MAX_CONTENT_EXTENT
            } else {
                viewport.width
            },
            height: if self.scrolls_vertical() {
                MAX_CONTENT_EXTENT
            } else {
                viewport.height
            },
        }
    }
}

impl<S: 'static> Widget<S> for ScrollView<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// reconcile を子へ伝播する（本ウィジェットセット初のコンテナ実装）。
    /// スクロールビュー自身は `S` へ直接触れず、委譲のみを行う。
    fn sync_from_state(&mut self, state: &S) {
        self.content.sync_from_state(state);
    }

    /// スクロールビュー自身はビューポートとして `available` を占有する
    /// （非有限・負値成分は `0.0` へ丸める、`Image::measure` と同型の安全側判断）。
    ///
    /// ただし `available` の軸成分が [`MAX_CONTENT_EXTENT`] 以上の場合、それは
    /// 祖先の `ScrollView` が [`Self::relaxed_constraint`] 経由で「子コンテンツの
    /// 自然な寸法を知るための確認用の緩和制約」として渡した値である可能性が高い
    /// （祖先から見て本ウィジェットは「入れ子にされたコンテンツ」）。この値を
    /// そのまま自身のビューポート寸法として採用すると、入れ子の `ScrollView` が
    /// 約 100 万論理ピクセルという人為的な寸法を報告し、祖先側の overflow 判定・
    /// オフセットクランプが常に破綻する（Cursor Bugbot 指摘、PR #381）。
    /// 該当軸は自身の [`Self::relaxed_constraint`] で自身の子コンテンツを計測した
    /// 自然な寸法に置き換える（軸ごとに独立判定。多段の入れ子でも各段が同じ判定を
    /// 適用するため再帰的に解決される）。
    fn measure(&self, available: Measurement) -> Measurement {
        let width = if available.width.is_finite() && available.width > 0.0 {
            available.width
        } else {
            0.0
        };
        let height = if available.height.is_finite() && available.height > 0.0 {
            available.height
        } else {
            0.0
        };

        if width >= MAX_CONTENT_EXTENT || height >= MAX_CONTENT_EXTENT {
            let content_size = self
                .content
                .measure(self.relaxed_constraint(Measurement { width, height }));
            let width = if width >= MAX_CONTENT_EXTENT {
                content_size.width
            } else {
                width
            };
            let height = if height >= MAX_CONTENT_EXTENT {
                content_size.height
            } else {
                height
            };
            return Measurement { width, height };
        }

        Measurement { width, height }
    }

    /// `[Rect（ビューポート枠）] + content.paint(..)（クランプ済みオフセットで
    /// 平行移動した origin）+ （オーバーフローする軸ごとに）Rect（スクロールバーサム）`
    /// を返す。`PaintCommand` は拡張せず、自前の `Text` は一切出力しない
    /// （モジュール doc 参照、OWASP A03 対策）。
    fn paint(&self, origin: (f32, f32), size: Measurement) -> Vec<PaintCommand> {
        let (offset_x, offset_y) = self.effective_offset(size);
        let content_origin = (origin.0 - offset_x, origin.1 - offset_y);
        let relaxed = self.relaxed_constraint(size);
        let content_size = self.content.measure(relaxed);

        let mut commands = vec![PaintCommand::Rect];
        commands.extend(self.content.paint(content_origin, content_size));

        let (overflows_x, overflows_y) = self.overflows(size);
        if overflows_x {
            commands.push(PaintCommand::Rect);
        }
        if overflows_y {
            commands.push(PaintCommand::Rect);
        }
        commands
    }

    /// スクロールビュー自身は常に `false` を返す（先行ウィジェットと同じ安全側の
    /// 既定。ホイール・ドラッグの実配線は S41、モジュール doc 参照）。
    fn hit_test(&self, _point: (f32, f32), _origin: (f32, f32), _size: Measurement) -> bool {
        false
    }

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
    use crate::ui::label::Label;

    struct ProfileState {
        name: String,
    }

    fn get_name(state: &ProfileState) -> String {
        state.name.clone()
    }

    fn tall_label() -> Label<ProfileState> {
        Label::bound(WidgetId::from_raw(100), get_name)
    }

    // (a) 構築直後の既定値

    #[test]
    fn new_has_default_axis_and_zero_offset() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(1), tall_label());

        assert_eq!(view.axis, ScrollAxis::Vertical);
        assert_eq!(view.raw_offset(), (0.0, 0.0));
        assert_eq!(view.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn new_interaction_state_is_all_false() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(2), tall_label());
        let interaction = view.interaction_state();

        assert!(!interaction.hovered);
        assert!(!interaction.pressed);
        assert!(!interaction.disabled);
        assert!(!interaction.focused);
    }

    #[test]
    fn with_axis_overrides_default() {
        let view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(3), tall_label()).with_axis(ScrollAxis::Horizontal);

        assert_eq!(view.axis, ScrollAxis::Horizontal);
    }

    // (b) set_offset の無害化（NaN / inf / 負値 → 0.0）

    #[test]
    fn set_offset_accepts_positive_finite_values() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(4), tall_label());
        view.set_offset(10.0, 20.0);
        assert_eq!(view.raw_offset(), (10.0, 20.0));
    }

    #[test]
    fn set_offset_clamps_negative_to_zero() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(5), tall_label());
        view.set_offset(-5.0, -10.0);
        assert_eq!(view.raw_offset(), (0.0, 0.0));
    }

    #[test]
    fn set_offset_clamps_nan_to_zero() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(6), tall_label());
        view.set_offset(f32::NAN, f32::NAN);
        assert_eq!(view.raw_offset(), (0.0, 0.0));
    }

    #[test]
    fn set_offset_clamps_infinite_to_zero() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(7), tall_label());
        view.set_offset(f32::INFINITY, f32::NEG_INFINITY);
        assert_eq!(view.raw_offset(), (0.0, 0.0));
    }

    // (c) effective_offset の上限クランプ・非スクロール軸の固定・Both 軸

    #[test]
    fn effective_offset_is_zero_when_content_fits_viewport() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(8), tall_label());
        view.set_offset(0.0, 999.0);

        let viewport = Measurement {
            width: 100.0,
            height: 100.0,
        };
        // Label の測定は available をそのまま返す実装のため、ビューポートと
        // 同一の緩和後測定になり、クランプで 0.0 に落ちる（overflow なし）。
        let (_, offset_y) = view.effective_offset(viewport);
        assert_eq!(offset_y, 0.0);
    }

    #[test]
    fn effective_offset_fixes_non_scrolling_axis_to_zero() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(9), tall_label()).with_axis(ScrollAxis::Vertical);
        // 縦軸のみ許容。横方向へオフセットを設定しても常に 0.0 に固定される。
        view.set_offset(50.0, 0.0);

        let viewport = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let (offset_x, _) = view.effective_offset(viewport);
        assert_eq!(offset_x, 0.0);
    }

    #[test]
    fn effective_offset_clamps_both_axes_when_axis_is_both() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(10), tall_label()).with_axis(ScrollAxis::Both);
        view.set_offset(-1.0, -1.0);

        let viewport = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let (offset_x, offset_y) = view.effective_offset(viewport);
        assert_eq!(offset_x, 0.0);
        assert_eq!(offset_y, 0.0);
    }

    // (d) measure

    #[test]
    fn measure_returns_available_as_viewport() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(11), tall_label());

        let measurement = view.measure(Measurement {
            width: 300.0,
            height: 400.0,
        });
        assert_eq!(measurement.width, 300.0);
        assert_eq!(measurement.height, 400.0);
    }

    #[test]
    fn measure_falls_back_to_zero_when_available_is_non_positive_or_non_finite() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(12), tall_label());

        let zero_width = view.measure(Measurement {
            width: 0.0,
            height: 100.0,
        });
        assert_eq!(zero_width.width, 0.0);
        assert_eq!(zero_width.height, 100.0);

        // 成分ごとに独立して丸めるため、無効なのは height のみで width は
        // 有効値のまま維持される（本ウィジェットの丸めは軸ごとの独立判定）。
        let negative_height = view.measure(Measurement {
            width: 100.0,
            height: -5.0,
        });
        assert_eq!(negative_height.width, 100.0);
        assert_eq!(negative_height.height, 0.0);

        let non_finite = view.measure(Measurement {
            width: f32::NAN,
            height: f32::INFINITY,
        });
        assert_eq!(non_finite.width, 0.0);
        assert_eq!(non_finite.height, 0.0);
    }

    #[test]
    fn measure_replaces_sentinel_relaxed_extent_with_content_natural_size() {
        // 祖先 ScrollView が relaxed_constraint 経由で渡す MAX_CONTENT_EXTENT
        // 相当の値を模す（本ウィジェットが入れ子コンテンツとして計測される想定）。
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(21), tall_label());

        let measured = view.measure(Measurement {
            width: 100.0,
            height: MAX_CONTENT_EXTENT,
        });

        // 非スクロール軸ではない width はそのまま。height は sentinel を
        // 自身の子コンテンツの自然な寸法（Label の測定結果）へ置き換える
        // ため、MAX_CONTENT_EXTENT より十分小さい値になる（回帰テスト、PR #381）。
        assert_eq!(measured.width, 100.0);
        assert!(measured.height < MAX_CONTENT_EXTENT);
    }

    #[test]
    fn nested_scroll_view_does_not_report_artificial_overflow() {
        // ScrollView を content として持つ ScrollView（入れ子）を構築する。
        let inner: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(22), tall_label());
        let outer: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(23), inner);

        let viewport = Measurement {
            width: 200.0,
            height: 200.0,
        };
        let (_, overflows_y) = outer.overflows(viewport);

        // 修正前は入れ子の ScrollView（inner）の measure が relaxed_constraint 経由の
        // MAX_CONTENT_EXTENT をそのまま採用し、outer から見て常に overflow 扱いに
        // なっていた。Label の自然な高さはビューポート未満のため overflow しない
        // のが正しい（Cursor Bugbot 指摘、PR #381 の回帰テスト）。
        assert!(!overflows_y);
    }

    // (e) paint の描画列

    #[test]
    fn paint_returns_viewport_rect_and_content_commands_when_not_overflowing() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(13), tall_label());

        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let commands = view.paint((0.0, 0.0), size);

        // 先頭はビューポート枠の Rect、続いて Label の paint 結果（Text 1 件）。
        // Label は状態未同期のため空文字列を Text として出す。
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], PaintCommand::Rect);
        assert!(matches!(commands[1], PaintCommand::Text(_)));
    }

    #[test]
    fn paint_never_emits_own_text_command() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(14), tall_label());

        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let commands = view.paint((0.0, 0.0), size);

        // ScrollView 自身が追加する Rect（先頭）以外に、Label が出す Text 以外の
        // 自前 Text が混入していないことを確認する（外部由来文字列の非流出）。
        let rect_count = commands
            .iter()
            .filter(|c| matches!(c, PaintCommand::Rect))
            .count();
        assert_eq!(rect_count, 1);
    }

    // (f) sync_from_state の委譲

    #[test]
    fn sync_from_state_updates_child_cache() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(15), tall_label());
        let state = ProfileState {
            name: "太郎".to_string(),
        };

        view.sync_from_state(&state);

        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let commands = view.paint((0.0, 0.0), size);
        assert_eq!(commands[1], PaintCommand::Text("太郎".to_string()));
    }

    // (g) hit_test / accessibility_role

    #[test]
    fn hit_test_is_always_false() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(16), tall_label());
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        assert!(!view.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_scroll_view() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(17), tall_label());
        assert_eq!(view.accessibility_role(), AccessibilityRole::ScrollView);
    }

    // (h) set_disabled / visual_state / clone_box / Debug

    #[test]
    fn set_disabled_transitions_visual_state() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(18), tall_label());
        assert_eq!(view.visual_state(), VisualState::Idle);

        view.set_disabled(true);
        assert_eq!(view.visual_state(), VisualState::Disabled);
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut view: ScrollView<ProfileState> =
            ScrollView::new(WidgetId::from_raw(19), tall_label());
        let state = ProfileState {
            name: "一郎".to_string(),
        };
        view.sync_from_state(&state);

        let mut cloned = view.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(19));

        let other_state = ProfileState {
            name: "二郎".to_string(),
        };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        let size = Measurement {
            width: 100.0,
            height: 100.0,
        };
        let commands = view.paint((0.0, 0.0), size);
        assert_eq!(commands[1], PaintCommand::Text("一郎".to_string()));
    }

    #[test]
    fn debug_output_does_not_panic() {
        let view: ScrollView<ProfileState> = ScrollView::new(WidgetId::from_raw(20), tall_label());
        let output = format!("{view:?}");
        assert!(output.contains("ScrollView"));
    }

    #[test]
    fn scroll_view_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ScrollView<ProfileState>>();
    }
}
