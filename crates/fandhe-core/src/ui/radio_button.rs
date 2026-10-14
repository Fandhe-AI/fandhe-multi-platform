//! ラジオボタンウィジェット `RadioButton<S>`（TASK-8.3-S22 / REQ-8、Issue #117）。
//!
//! 本モジュールは共通トレイト [`super::widget::Widget`] を実装する具体ウィジェット型
//! `RadioButton<S>` を提供する。
//!
//! **チェックボックス（bool 単体トグル）との違い**: ラジオボタンはグループ内排他選択が
//! 本質である。専用の `RadioGroup` コンテナ型は導入せず、「同一グループの全
//! `RadioButton<S>` が同じ状態フィールド（同じ `get` / `set`）を共有する」ことで排他性を
//! 創発させる最小設計とする。各インスタンスは自身の選択肢値 `value: usize` を持ち、
//! `get: ReadFn<S, usize>` が返す「グループの現在選択値」と一致するかどうかで
//! 選択状態（`cached_selected`）を判定する。
//!
//! **将来拡張候補（out-of-scope-tracking.md 対象、PR 報告でユーザーへ報告）**:
//! 選択肢値の汎用型化（`usize` → `V: PartialEq`）、専用 `RadioGroup` コンテナの
//! 追加。いずれもスコープ外事項。
//!
//! **`set` の実配線・イベント処理（Issue #118）**は本型を直接拡張するのではなく、
//! 兄弟モジュールの [`super::RadioGroupWidget`] が担う別型として実現されている
//! （`RadioButton<S>` 自体の `set` は契約明示のみで完結し、グループ全体の入力処理は
//! `RadioGroupWidget` 側の責務）。`WidgetCatalog` 登録・テスト網羅拡充（S24、#119）は
//! [`super::catalog`] への `radio_button` エントリ登録（[`super::catalog::WidgetCatalog::with_builtins`]）
//! と、本モジュール・`tests/radio_button_widget.rs` のユニット/結合テストとして完了済み。
//! カタログのプレビューは未配線契約（[`super::widget::Widget::sync_from_state`] が
//! 共有参照走査へ未接続）のまま安全側（タイトル行のみ出力）で動作する仕様を維持している
//! （全ウィジェット共通の横断課題であり本モジュール単独では解消しない）。

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, Widget, WidgetId,
};
use crate::state::{ReadFn, WidgetValue, WriteFn};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数（既存ウィジェット
/// と同じ判断）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// 選択円（プレースホルダ矩形で代替）の一辺サイズ（論理ピクセル相当）。
const CIRCLE_SIZE: f32 = 16.0;

/// 選択円とラベルの間隔（論理ピクセル相当）。
const CIRCLE_LABEL_GAP: f32 = 4.0;

/// 選択時に描画する選択マーク文字（●）。
const RADIO_MARK: &str = "\u{25CF}";

/// グループ内排他選択の 1 選択肢を表すラジオボタン（TASK-8.3-S22、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`TextInput<S>` / `Label<S>` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。
pub struct RadioButton<S> {
    id: WidgetId,
    label: String,
    /// このラジオボタンが表す選択肢値。グループ内で一意にする責務は呼び出し側にある。
    value: usize,
    /// アプリ状態 `S` から「グループの現在選択値」を読み取る関数ポインタ
    /// （REQ-13、環境キャプチャ不可）。同一グループの全 `RadioButton` が同じ `get` を
    /// 共有することで排他選択を表現する。
    get: ReadFn<S, usize>,
    /// 選択時に自身の `value` を状態へ書き込む関数ポインタ。実際のイベント配線は
    /// S23（#118）のスコープであり、契約（シグネチャ）のみを確定させる。
    set: WriteFn<S, usize>,
    /// [`Widget::sync_from_state`] が更新する選択状態キャッシュ（`is_selected()` が返す値）。
    cached_selected: bool,
    /// hover/pressed/disabled/focus。実際の入力配線は S23（#118）のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for RadioButton<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadioButton")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("value", &self.value)
            .field("cached_selected", &self.cached_selected)
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for RadioButton<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            value: self.value,
            get: self.get,
            set: self.set,
            cached_selected: self.cached_selected,
            interaction: self.interaction,
        }
    }
}

impl<S> RadioButton<S> {
    /// 状態 `S` に束縛したラジオボタンを作る（ビルダー形式の起点）。
    ///
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`] / [`WriteFn`]）
    /// と代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。`set` をコンストラクタに含める理由は、S23（状態・イベント処理）で
    /// ビルダーシグネチャを破壊変更しないためであり、値の書き込み・イベントディスパッチ
    /// 自体は S23 のスコープとする。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        value: usize,
        get: ReadFn<S, usize>,
        set: WriteFn<S, usize>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            value,
            get,
            set,
            cached_selected: false,
            interaction: InteractionState::default(),
        }
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// このラジオボタンが表す選択肢値を返す。
    pub fn value(&self) -> usize {
        self.value
    }

    /// 直近の [`Widget::sync_from_state`] が判定した選択状態を返す。
    pub fn is_selected(&self) -> bool {
        self.cached_selected
    }

    /// S23 のイベント配線が使う書き込み経路を返す。
    ///
    /// この関数ポインタを呼び出す経路（ディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる。
    pub fn write_fn(&self) -> WriteFn<S, usize> {
        self.set
    }

    /// グループの現在選択値を読み取る関数ポインタを返す（`sync_from_state` と同じ経路）。
    ///
    /// 同一グループの全 `RadioButton<S>`（モジュール doc 参照）はこの `get` を共有するため、
    /// 関数ポインタのアドレス自体がグループ識別子として使える。`fandhe-web-dom` が
    /// `<input type="radio">` の `name` 属性を同一グループ間で揃えるために参照する
    /// （Issue #999 レビュー指摘: name 未設定だと矢印キー選択・Tab 巡回が壊れる）。
    pub fn read_fn(&self) -> ReadFn<S, usize> {
        self.get
    }
}

impl<S: 'static> Widget<S> for RadioButton<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` からグループの現在選択値を読み取り、自身の `value` と一致するかで
    /// `cached_selected` を更新する（reconcile フェーズ）。
    ///
    /// **未配線の契約（widget.rs 参照）**: `render_headless` / `convert_node` /
    /// `to_snapshot_node` は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
    /// 要求する本メソッドを呼ぶ経路が現時点で存在しない。`WidgetHarness` 等の
    /// 直接駆動経由でのみ呼ばれる想定である（既存ウィジェットと同じ判断）。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_selected = (self.get)(state) == self.value;
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = 選択円サイズ + 円とラベルの間隔 + ラベル幅、を `available.width` でクランプ。
    /// 高さ = 選択円サイズと行高の大きい方を `available.height` でクランプ。
    /// マルチバイト文字は `chars().count()` を文字数とする近似のため、実際の描画幅とは
    /// 一致しない場合がある（既存ウィジェットと同じ既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (CIRCLE_SIZE + CIRCLE_LABEL_GAP + label_width).min(available.width);
        let height = CIRCLE_SIZE.max(APPROX_LINE_HEIGHT).min(available.height);

        Measurement { width, height }
    }

    /// 選択時は `[Rect（選択円のプレースホルダ）, Text(選択マーク), Text(label)]`、
    /// 非選択時は `[Rect, Text(label)]` を返す。
    ///
    /// `PaintCommand` は拡張せず既存の Rect / Text の 2 種のみを使う（3 バックエンドの
    /// プレースホルダ契約への波及を避ける安全側の判断、既存ウィジェットと同一方針）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect];

        if self.cached_selected {
            commands.push(PaintCommand::Text(RADIO_MARK.to_string()));
        }

        commands.push(PaintCommand::Text(self.label.clone()));
        commands
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたラジオボタンを選択操作の対象にしない（安全側の判断）。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する
    /// （既存ウィジェットと同一の契約）。
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
        AccessibilityRole::RadioButton
    }

    /// 選択肢ラベルをアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
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

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// `WidgetValue::Index` が自身の `value()` と一致するときのみ `write_fn()` へ
    /// 書き込む（Issue #805 / 親 #804）。`WidgetId` の一致は呼び出し元
    /// `ui::event::set_widget_value_at` が本メソッド呼び出し前に確認済みのため、
    /// ここでは行わない（id ガードは一元化する設計、レビュー指摘対応）。不一致は
    /// `ButtonClickedAt` の label ガードと同じ整合性検証（描画時と dispatch 時の状態
    /// 乖離による誤発火防止、A08 対策。id ガードに加えたグループ内選択肢の二段目）
    /// として `false` を返す。`Bool` / `Number` 等の型不一致も `false` を返す。
    fn apply_value(&self, state: &mut S, value: &WidgetValue) -> bool {
        match value {
            WidgetValue::Index(i) if *i == self.value() => {
                (self.write_fn())(state, *i);
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
        selected: usize,
    }

    fn get_selected(state: &FormState) -> usize {
        state.selected
    }

    fn set_selected(state: &mut FormState, value: usize) {
        state.selected = value;
    }

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_value_and_unselected() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        assert_eq!(radio.label_text(), "選択肢A");
        assert_eq!(radio.value(), 0);
        assert!(!radio.is_selected());
        assert_eq!(radio.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let write_fn = radio.write_fn();
        let mut state = FormState { selected: 1 };
        write_fn(&mut state, 0);
        assert_eq!(state.selected, 0);
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_selected_when_get_matches_value() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let state = FormState { selected: 0 };

        radio.sync_from_state(&state);

        assert!(radio.is_selected());
    }

    #[test]
    fn sync_from_state_unselected_when_get_does_not_match_value() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let state = FormState { selected: 1 };

        radio.sync_from_state(&state);

        assert!(!radio.is_selected());
    }

    #[test]
    fn sync_from_state_follows_state_changes_across_calls() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let mut state = FormState { selected: 0 };

        radio.sync_from_state(&state);
        assert!(radio.is_selected());

        state.selected = 1;
        radio.sync_from_state(&state);
        assert!(!radio.is_selected());
    }

    // (c) 排他性: 同一状態を共有する複数の RadioButton で、選択値の切替により
    // 一方のみが選択状態になること

    #[test]
    fn exclusive_selection_across_group_members() {
        let mut option_a: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let mut option_b: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(2),
            "選択肢B",
            1,
            get_selected,
            set_selected,
        );
        let mut state = FormState { selected: 0 };

        option_a.sync_from_state(&state);
        option_b.sync_from_state(&state);
        assert!(option_a.is_selected());
        assert!(!option_b.is_selected());

        state.selected = 1;
        option_a.sync_from_state(&state);
        option_b.sync_from_state(&state);
        assert!(!option_a.is_selected());
        assert!(option_b.is_selected());
    }

    // (d) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let radio: RadioButton<FormState> =
            RadioButton::bound(WidgetId::from_raw(1), "AB", 0, get_selected, set_selected);

        let measurement = radio.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let expected_width = CIRCLE_SIZE + CIRCLE_LABEL_GAP + (2.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, CIRCLE_SIZE.max(APPROX_LINE_HEIGHT));
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです",
            0,
            get_selected,
            set_selected,
        );

        let measurement = radio.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let radio: RadioButton<FormState> =
            RadioButton::bound(WidgetId::from_raw(1), "あ", 0, get_selected, set_selected);

        let measurement = radio.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // "あ" は 1 文字（3 バイト）。バイト数基準なら幅がずれてしまう回帰の防止。
        let expected_width = CIRCLE_SIZE + CIRCLE_LABEL_GAP + (1.0 * APPROX_CHAR_WIDTH);
        assert_eq!(measurement.width, expected_width);
    }

    // (e) paint

    #[test]
    fn paint_returns_rect_mark_and_label_when_selected() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let state = FormState { selected: 0 };
        radio.sync_from_state(&state);

        let commands = radio.paint(
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
                PaintCommand::Text(RADIO_MARK.to_string()),
                PaintCommand::Text("選択肢A".to_string()),
            ]
        );
    }

    #[test]
    fn paint_returns_rect_and_label_when_unselected() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let state = FormState { selected: 1 };
        radio.sync_from_state(&state);

        let commands = radio.paint(
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
                PaintCommand::Text("選択肢A".to_string()),
            ]
        );
    }

    // (f) hit_test

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let hit = radio.hit_test(
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
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let hit = radio.hit_test(
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
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let hit = radio.hit_test(
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
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let hit = radio.hit_test(
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
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        radio.interaction.set_disabled(true);

        let hit = radio.hit_test(
            (5.0, 5.0),
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert!(!hit);
    }

    // (g) accessibility_role / clone_box / Debug

    #[test]
    fn accessibility_role_is_radio_button() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        assert_eq!(radio.accessibility_role(), AccessibilityRole::RadioButton);
    }

    #[test]
    fn accessibility_label_is_option_label() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        assert_eq!(radio.accessibility_label(), Some("選択肢A".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );
        let state = FormState { selected: 0 };
        radio.sync_from_state(&state);

        let mut cloned = radio.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState { selected: 1 };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        let radio_commands = radio.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            radio_commands.contains(&PaintCommand::Text(RADIO_MARK.to_string())),
            "clone 元は影響を受けず選択状態を維持するべき"
        );

        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            !cloned_commands.contains(&PaintCommand::Text(RADIO_MARK.to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn debug_smoke_test() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        let output = format!("{radio:?}");
        assert!(output.contains("RadioButton"));
    }

    // (h) interaction_state（TASK-8.3-S24 / Issue #119 でのテスト拡充）

    #[test]
    fn interaction_state_defaults_to_not_disabled() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        assert!(!radio.interaction_state().disabled);
    }

    #[test]
    fn interaction_state_reflects_disabled_flag() {
        let mut radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(1),
            "選択肢A",
            0,
            get_selected,
            set_selected,
        );

        radio.interaction.set_disabled(true);

        assert!(radio.interaction_state().disabled);
    }

    // (i) Screen::radio_button ビルダーの結線確認は screen.rs 側のテストで行う。

    // (j) TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let radio: RadioButton<FormState> = RadioButton::bound(
            WidgetId::from_raw(200),
            "選択肢A",
            0,
            get_selected,
            set_selected,
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
            radio.paint_styled((0.0, 0.0), size, &empty_resolver),
            radio.paint((0.0, 0.0), size)
        );
    }
}
