//! テキストフィールドウィジェット `TextInput<S>`（TASK-8.3-S13 / REQ-8、Issue #108）。
//!
//! 本モジュールは共通トレイト [`super::widget::Widget`] を実装する
//! 具体ウィジェット型として `TextInput<S>` を提供する。
//!
//! **既存 `Node::TextField`（TASK-1.1 由来のレガシー enum バリアント）との関係**:
//! 設計文書 `docs/architecture/widget-architecture.md` §4.1 の温存方針に従い、
//! レガシー `Node::TextField` / `Screen::text_field()` は変更しない。
//! `TextInput<S>` は `Node::Widget(Box<dyn Widget<S>>)` 経由でツリーへ組み込む
//! 新規ウィジェットであり、型名の衝突・読み手の混同を避けるため `TextField` という
//! 名前は使わない
//! （アクセシビリティロールは既存の [`super::widget::AccessibilityRole::TextInput`] を
//! そのまま使うため名称も整合する）。
//!
//! **S14/S15 の消化状況**: 入力イベント処理・`InteractionState::apply` の配線は
//! S14（#109、`text_field.rs` の `TextFieldWidget<S>` として新設。`TextInput<S>` 自体との
//! 型統合は別作業のまま、`text_field.rs` モジュール doc §スコープ外参照）で完了した。
//! `WidgetCatalog` への登録・テスト網羅拡充は S15（#110）で完了し、`catalog.rs` の
//! `text_field` エントリはレガシー `Node::TextField` から本型（`TextInput<S>`）へ
//! 差し替え済み（`catalog.rs` の `text_field_preview_screen` doc 参照）。

use super::widget::{
    AccessibilityRole, InteractionState, Measurement, PaintCommand, Widget, WidgetId,
};
use crate::state::{ReadFn, WriteFn};

/// 単一行モノスペース近似に使う 1 文字あたりの幅（論理ピクセル相当）。
///
/// cosmic-text 等の実テキスト計測エンジン導入前のプレースホルダ定数であり、
/// `measure` の折り返し・複数行はスコープ外（S13 の設計判断）。
const APPROX_CHAR_WIDTH: f32 = 8.0;

/// 単一行の高さ近似（論理ピクセル相当）。`APPROX_CHAR_WIDTH` と同じくプレースホルダ。
const APPROX_LINE_HEIGHT: f32 = 20.0;

/// 入力枠の余白（論理ピクセル相当）。`measure` の幅計算に 1 回のみ加算する
/// 合計パディング量であり、左右それぞれの値ではない点に注意。
const INPUT_PADDING: f32 = 8.0;

/// ラベル付きテキスト入力欄（TASK-8.3-S13、`Widget<S>` 実装型）。
///
/// `Debug` / `Clone` は手動実装する（`Screen<S>` / `Node<S>` と同じく、状態型 `S` を
/// 所有していないため `derive` マクロが自動付与する `S: Debug` / `S: Clone` という
/// 不要な境界を避ける）。
///
/// **`Debug` 出力に `cached_value` を含めない**: テキストフィールドは将来パスワード等の
/// 秘密情報を保持しうるため、ログ・エラーメッセージへの値漏えいを型レベルで防ぐ
/// （`.claude/rules/security.md`「ログにシークレットを出力しない」）。マスクとして
/// 文字数のみを出力する。
pub struct TextInput<S> {
    id: WidgetId,
    label: String,
    placeholder: Option<String>,
    /// アプリ状態 `S` から現在値を読み取る関数ポインタ（REQ-13、環境キャプチャ不可）。
    get: ReadFn<S, String>,
    /// ユーザー入力を状態 `S` へ反映する関数ポインタ。実際のイベント配線は S14（#109）の
    /// スコープであり、契約（シグネチャ）のみを確定させる。
    set: WriteFn<S, String>,
    /// [`Widget::sync_from_state`] が更新する表示キャッシュ（`value()` が返す値）。
    cached_value: String,
    /// hover/pressed/disabled/focus。実際の入力配線は S14（#109）のスコープ。
    interaction: InteractionState,
}

impl<S> std::fmt::Debug for TextInput<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextInput")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("placeholder", &self.placeholder)
            // `cached_value` の内容そのものは出力せず、文字数のみをマスクとして出す
            // （秘密情報漏えい防止、security.md）。
            .field("cached_value_len", &self.cached_value.chars().count())
            .field("interaction", &self.interaction)
            .finish()
    }
}

impl<S> Clone for TextInput<S> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            placeholder: self.placeholder.clone(),
            get: self.get,
            set: self.set,
            cached_value: self.cached_value.clone(),
            interaction: self.interaction,
        }
    }
}

impl<S> TextInput<S> {
    /// 状態 `S` に束縛したテキスト入力欄を作る（ビルダー形式の起点）。
    ///
    /// `get` / `set` は [`crate::state`] の SSOT 型エイリアス（[`ReadFn`] / [`WriteFn`]）
    /// と代入互換の関数ポインタに限定する（REQ-13、環境キャプチャクロージャを型レベルで
    /// 拒否する）。`set` をコンストラクタに含める理由は、S14（状態・イベント処理）で
    /// ビルダーシグネチャを破壊変更しないためであり、値の書き込み・イベントディスパッチ
    /// 自体は S14 のスコープとする。
    pub fn bound(
        id: WidgetId,
        label: impl Into<String>,
        get: ReadFn<S, String>,
        set: WriteFn<S, String>,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            placeholder: None,
            get,
            set,
            cached_value: String::new(),
            interaction: InteractionState::default(),
        }
    }

    /// プレースホルダ文言を設定する（ビルダー形式）。
    ///
    /// `value()` が空文字列を返す間、[`Widget::paint`] はこの文言を描画する
    /// （§3.3 の設計判断）。
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// `cached_value`（直近の [`Widget::sync_from_state`] が読み取った表示値）を返す。
    pub fn value(&self) -> &str {
        &self.cached_value
    }

    /// 表示文言（ラベル）を返す。
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// プレースホルダ文言を返す（未設定なら `None`）。
    pub fn placeholder(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    /// S14 のイベント配線が使う書き込み経路を返す。
    ///
    /// この関数ポインタを呼び出す経路（ディスパッチ）は
    /// 実装しない契約明示のためのアクセサであり、`dead_code` 警告の回避も兼ねる。
    pub fn write_fn(&self) -> WriteFn<S, String> {
        self.set
    }
}

impl<S: 'static> Widget<S> for TextInput<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    /// アプリ状態 `S` から現在値を読み取り `cached_value` を更新する（reconcile フェーズ）。
    ///
    /// **未配線の契約（widget.rs 参照）**: `render_headless` / `convert_node` /
    /// `to_snapshot_node` は `&Node<S>` の共有参照のみで走査するため、`&mut self` を
    /// 要求する本メソッドを呼ぶ経路が現時点で存在しない。S3（#98）確立済みの reconcile/sync
    /// パスが `Node::Widget` へ実際に `&mut` を届けるまでは、本メソッドは
    /// `WidgetHarness` 等の直接駆動経由でのみ呼ばれる想定である。
    fn sync_from_state(&mut self, state: &S) {
        self.cached_value = (self.get)(state);
    }

    /// 単一行モノスペース近似で必要寸法を返す（cosmic-text 導入前のプレースホルダ実装、
    /// 折り返し・複数行はスコープ外）。
    ///
    /// 幅 = ラベル幅 + 表示値（空なら placeholder）の幅 + 入力枠パディング。
    /// マルチバイト文字は `chars().count()` を文字数とする近似のため、実際の描画幅とは
    /// 一致しない場合がある（cosmic-text 導入までの既知の制約）。
    fn measure(&self, available: Measurement) -> Measurement {
        let display = if self.cached_value.is_empty() {
            self.placeholder.as_deref().unwrap_or("")
        } else {
            &self.cached_value
        };

        let label_width = self.label.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let display_width = display.chars().count() as f32 * APPROX_CHAR_WIDTH;
        let width = (label_width + display_width + INPUT_PADDING).min(available.width);
        let height = APPROX_LINE_HEIGHT.min(available.height);

        Measurement { width, height }
    }

    /// `[Rect, Text(label), Text(値 or placeholder)]` の描画プリミティブ列を返す。
    ///
    /// 値が空で placeholder が設定されている場合のみ placeholder を描画する
    /// （値が空かつ placeholder 未設定なら値側の `Text` は描かない）。
    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        let mut commands = vec![PaintCommand::Rect, PaintCommand::Text(self.label.clone())];

        if !self.cached_value.is_empty() {
            commands.push(PaintCommand::Text(self.cached_value.clone()));
        } else if let Some(placeholder) = &self.placeholder {
            commands.push(PaintCommand::Text(placeholder.clone()));
        }

        commands
    }

    /// 矩形包含判定（AABB）。`interaction.disabled` の間は `false` を返し、
    /// 無効化されたテキストフィールドを入力対象にしない（安全側の判断）。
    ///
    /// 表示専用の `Label`（常に `false`）と異なり、テキストフィールドは S14 で
    /// フォーカス・入力イベントの対象になるため包含判定を実際に返す。
    ///
    /// 共有ジオメトリ契約である [`crate::ui::Rect::contains`] に合わせ、
    /// 左上を含み右下を含まない半開区間（`[origin, origin + size)`）で判定する
    /// （右端・下端を含む閉区間にすると隣接ウィジェットとの境界上の点が重複してヒットしうる。
    /// 半開区間なら 0 サイズの矩形は原点を含め常に非ヒットになり、閉区間との不整合も解消する）。
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
        AccessibilityRole::TextInput
    }

    /// 入力欄のラベル文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
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
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FormState {
        note: String,
    }

    fn get_note(state: &FormState) -> String {
        state.note.clone()
    }

    fn set_note(state: &mut FormState, value: String) {
        state.note = value;
    }

    // (a) 構築と各アクセサ

    #[test]
    fn bound_constructs_with_label_and_no_placeholder() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        assert_eq!(input.label_text(), "メモ");
        assert_eq!(input.placeholder(), None);
        assert_eq!(input.value(), "");
        assert_eq!(input.id(), WidgetId::from_raw(1));
    }

    #[test]
    fn with_placeholder_sets_placeholder() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note)
                .with_placeholder("未入力");

        assert_eq!(input.placeholder(), Some("未入力"));
    }

    #[test]
    fn write_fn_returns_configured_set_function_pointer() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let write_fn = input.write_fn();
        let mut state = FormState {
            note: String::new(),
        };
        write_fn(&mut state, "更新".to_string());
        assert_eq!(state.note, "更新");
    }

    // (b) sync_from_state

    #[test]
    fn sync_from_state_updates_cached_value_from_get() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "こんにちは".to_string(),
        };

        input.sync_from_state(&state);

        assert_eq!(input.value(), "こんにちは");
    }

    #[test]
    fn sync_from_state_follows_state_changes_across_calls() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let mut state = FormState {
            note: "最初".to_string(),
        };

        input.sync_from_state(&state);
        assert_eq!(input.value(), "最初");

        state.note = "更新後".to_string();
        input.sync_from_state(&state);
        assert_eq!(input.value(), "更新後");
    }

    // (c) measure

    #[test]
    fn measure_approximates_width_from_char_count() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "abc".to_string(),
        };
        input.sync_from_state(&state);

        let measurement = input.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // label(2 文字) + value(3 文字) 分の近似幅 + パディング。
        let expected_width = (2.0 * APPROX_CHAR_WIDTH) + (3.0 * APPROX_CHAR_WIDTH) + INPUT_PADDING;
        assert_eq!(measurement.width, expected_width);
        assert_eq!(measurement.height, APPROX_LINE_HEIGHT);
    }

    #[test]
    fn measure_uses_placeholder_width_when_value_empty() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note)
                .with_placeholder("入力してください");

        let measurement = input.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        let placeholder_chars = "入力してください".chars().count() as f32;
        let label_chars = "メモ".chars().count() as f32;
        let expected_width = (label_chars * APPROX_CHAR_WIDTH)
            + (placeholder_chars * APPROX_CHAR_WIDTH)
            + INPUT_PADDING;
        assert_eq!(measurement.width, expected_width);
    }

    #[test]
    fn measure_clamps_to_available_space() {
        let mut input: TextInput<FormState> = TextInput::bound(
            WidgetId::from_raw(1),
            "非常に長いラベルです",
            get_note,
            set_note,
        );
        let state = FormState {
            note: "非常に長い値がここに入ります".to_string(),
        };
        input.sync_from_state(&state);

        let measurement = input.measure(Measurement {
            width: 10.0,
            height: 5.0,
        });

        assert_eq!(measurement.width, 10.0);
        assert_eq!(measurement.height, 5.0);
    }

    #[test]
    fn measure_counts_multibyte_chars_not_bytes() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "", get_note, set_note);
        let state = FormState {
            note: "あ".to_string(),
        };
        input.sync_from_state(&state);

        let measurement = input.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });

        // "あ" は 1 文字（3 バイト）。バイト数基準なら幅がずれてしまう回帰の防止。
        let expected_width = (1.0 * APPROX_CHAR_WIDTH) + INPUT_PADDING;
        assert_eq!(measurement.width, expected_width);
    }

    // (d) paint

    #[test]
    fn paint_returns_rect_label_and_value_when_value_present() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "値".to_string(),
        };
        input.sync_from_state(&state);

        let commands = input.paint(
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
                PaintCommand::Text("メモ".to_string()),
                PaintCommand::Text("値".to_string()),
            ]
        );
    }

    #[test]
    fn paint_draws_placeholder_when_value_empty() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note)
                .with_placeholder("未入力");

        let commands = input.paint(
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
                PaintCommand::Text("メモ".to_string()),
                PaintCommand::Text("未入力".to_string()),
            ]
        );
    }

    #[test]
    fn paint_omits_value_text_when_empty_and_no_placeholder() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let commands = input.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("メモ".to_string())]
        );
    }

    // (e) hit_test / accessibility_role / clone_box / Debug

    #[test]
    fn hit_test_returns_true_when_point_inside_rect() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let hit = input.hit_test(
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
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let hit = input.hit_test(
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
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let hit = input.hit_test(
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
        // 0 サイズの矩形は半開区間では常に非ヒットになる（閉区間だった旧実装では
        // 原点がヒットしてしまう不整合があった）。
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        let hit = input.hit_test(
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
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        input.interaction.set_disabled(true);

        let hit = input.hit_test(
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
    fn accessibility_role_is_text_input() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        assert_eq!(input.accessibility_role(), AccessibilityRole::TextInput);
    }

    #[test]
    fn accessibility_label_is_input_label() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);

        assert_eq!(input.accessibility_label(), Some("メモ".to_string()));
    }

    #[test]
    fn clone_box_produces_independent_copy() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "元の値".to_string(),
        };
        input.sync_from_state(&state);

        let mut cloned = input.clone_box();
        assert_eq!(cloned.id(), WidgetId::from_raw(1));

        let other_state = FormState {
            note: "別の値".to_string(),
        };
        cloned.sync_from_state(&other_state);

        // clone 元は影響を受けない（独立性の検証）。
        assert_eq!(input.value(), "元の値");
        let cloned_commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 100.0,
                height: 20.0,
            },
        );
        assert!(
            cloned_commands.contains(&PaintCommand::Text("別の値".to_string())),
            "clone 側は独立して sync_from_state を反映するべき"
        );
    }

    #[test]
    fn debug_output_does_not_contain_cached_value_content() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "秘密の値123".to_string(),
        };
        input.sync_from_state(&state);

        let output = format!("{input:?}");

        assert!(
            !output.contains("秘密の値123"),
            "Debug 出力に cached_value の内容が含まれてはならない（値マスク契約）"
        );
        assert!(output.contains("cached_value_len"));
    }

    // (h) sync -> measure -> paint のライフサイクル順の直接駆動テスト

    #[test]
    fn lifecycle_sync_then_measure_then_paint_reflects_state() {
        let mut input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(1), "メモ", get_note, set_note);
        let state = FormState {
            note: "順序確認".to_string(),
        };

        input.sync_from_state(&state);
        let measurement = input.measure(Measurement {
            width: 1000.0,
            height: 1000.0,
        });
        let commands = input.paint((0.0, 0.0), measurement);

        assert!(measurement.width > 0.0);
        assert_eq!(
            commands,
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("メモ".to_string()),
                PaintCommand::Text("順序確認".to_string()),
            ]
        );
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let input: TextInput<FormState> =
            TextInput::bound(WidgetId::from_raw(200), "メモ", get_note, set_note);
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert_eq!(
            input.paint_styled((0.0, 0.0), size, &empty_resolver),
            input.paint((0.0, 0.0), size)
        );
    }
}
