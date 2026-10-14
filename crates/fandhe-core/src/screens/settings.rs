//! M7（#724）の型サンプル「設定画面」（Issue #727）。通知トグル・文字サイズ調整・
//! テーマ選択・初期値リセットの操作結果を画面下部へ集約表示する 1 画面。
//! `FANDHE_PROXY_APP=settings` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! から呼び出される（`screens/mod.rs` の `pub mod settings;` と対）。
//!
//! `Screen::toggle_switch` / `slider` / `radio_button`（`ui/screen.rs`）が組み立てる
//! `Node::Widget` は egui-proxy（#806）で実描画され、値変更は
//! `Event::WidgetValueChanged` → `ui::set_widget_value_at`（#805）経由で状態へ反映される
//! （Issue #807）。`render_headless` はヘッドレス契約により `Node::Widget` の行を
//! 出力しない（`ui/render.rs` 参照）ため、下部の集約テキスト（`summary`）で
//! ヘッドレス結合テストからも状態を検証できるようにしている。

use crate::ui::{Node, Slider};
use crate::{Screen, WidgetId};

/// 文字サイズの下限（含む）。
pub const FONT_SIZE_MIN: u32 = 12;
/// 文字サイズの上限（含む）。
pub const FONT_SIZE_MAX: u32 = 24;
/// 文字サイズの初期値。
pub const FONT_SIZE_DEFAULT: u32 = 16;

/// テーマ選択の 3 択（ラジオ相当）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// 端末の設定に合わせる（初期値）。
    #[default]
    System,
    /// ライトテーマ固定。
    Light,
    /// ダークテーマ固定。
    Dark,
}

impl Theme {
    /// 表示・集約テキストに使う日本語ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "端末に合わせる",
            Theme::Light => "ライト",
            Theme::Dark => "ダーク",
        }
    }

    /// `.radio_button()` の `value` / `get`/`set` が扱う選択肢インデックス
    /// （`WidgetValue::Index` と揃える、`System=0` / `Light=1` / `Dark=2`）。
    pub fn index(self) -> usize {
        match self {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        }
    }

    /// [`Theme::index`] の逆変換。範囲外（3 択のラジオグループ以外からの
    /// 不正な index）は `None` を返す（panic を API 契約にしない）。
    pub fn from_index(index: usize) -> Option<Theme> {
        match index {
            0 => Some(Theme::System),
            1 => Some(Theme::Light),
            2 => Some(Theme::Dark),
            _ => None,
        }
    }
}

/// 設定画面が保持する状態。
///
/// `font_size` は非公開。`FONT_SIZE_MIN..=FONT_SIZE_MAX` の範囲契約を
/// 型で保証するため、読み取りは [`SettingsState::font_size`]、更新は
/// [`set_font_size`] 経由に限る（codex-review 指摘対応）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsState {
    /// 通知の有効・無効（トグル相当）。
    pub notifications_on: bool,
    /// 文字サイズ（スライダー相当、`FONT_SIZE_MIN..=FONT_SIZE_MAX` に収まる）。
    font_size: u32,
    /// 選択中のテーマ（ラジオ相当）。
    pub theme: Theme,
}

impl SettingsState {
    /// 現在の文字サイズ（常に `FONT_SIZE_MIN..=FONT_SIZE_MAX` の範囲内）。
    pub fn font_size(&self) -> u32 {
        self.font_size
    }
}

impl Default for SettingsState {
    /// 画面レジストリ（`generated_screens!`）は `Default::default()` で初期状態を
    /// 作るため、通知オン・文字サイズ 16・テーマ「端末に合わせる」を返す
    /// （derive では `bool`/`u32` の既定値 `false`/`0` になり要件の初期値と一致しないため
    /// 手動実装、Issue #727 コメント参照）。
    fn default() -> Self {
        SettingsState {
            notifications_on: true,
            font_size: FONT_SIZE_DEFAULT,
            theme: Theme::System,
        }
    }
}

/// 「通知」トグルスイッチの表示値を返す（`Screen::toggle_switch` の getter）。
pub fn get_notifications_on(state: &SettingsState) -> bool {
    state.notifications_on
}

/// 「通知」トグルスイッチの変更を `state.notifications_on` へ反映する
/// （`Screen::toggle_switch` の setter）。
pub fn set_notifications_on(state: &mut SettingsState, value: bool) {
    state.notifications_on = value;
}

/// 「文字サイズ」スライダーの表示値を返す（`Slider::bound` の getter）。
pub fn get_font_size(state: &SettingsState) -> f32 {
    state.font_size() as f32
}

/// 「文字サイズ」スライダーの変更を `state.font_size` へ反映する
/// （`Slider::bound` の setter）。`apply_value` 側で `NaN` は既に拒否済みだが、
/// 公開関数として直接呼ばれる経路もあるため、`FONT_SIZE_MIN..=FONT_SIZE_MAX` の
/// 範囲契約（[`SettingsState`] doc 参照）をこの関数でも保証する。非有限値
/// （`NaN` / `∞`）は `clamp` を素通りして `as u32` で 0 になり得るため現在値を維持する
/// （PR #810 codex-review P1）。
pub fn set_font_size(state: &mut SettingsState, value: f32) {
    if !value.is_finite() {
        return;
    }
    let rounded = value
        .round()
        .clamp(FONT_SIZE_MIN as f32, FONT_SIZE_MAX as f32);
    state.font_size = rounded as u32;
}

/// テーマのラジオグループの選択中インデックスを返す（`Screen::radio_button` の getter、
/// 3 択で共有する）。
pub fn get_theme_index(state: &SettingsState) -> usize {
    state.theme.index()
}

/// テーマのラジオグループの選択変更を `state.theme` へ反映する
/// （`Screen::radio_button` の setter、3 択で共有する）。範囲外の index は
/// 無視する（[`Theme::from_index`] 参照）。
pub fn set_theme_index(state: &mut SettingsState, index: usize) {
    if let Some(theme) = Theme::from_index(index) {
        state.theme = theme;
    }
}

/// 「初期値に戻す」ボタンのハンドラ。
pub fn reset_to_defaults(state: &mut SettingsState) {
    *state = SettingsState::default();
}

/// 画面下部に表示する集約テキスト（受入 (a) の「通知: オン / 文字サイズ: 16 / テーマ: ライト」形式）。
pub fn summary(state: &SettingsState) -> String {
    format!(
        "通知: {} / 文字サイズ: {} / テーマ: {}",
        if state.notifications_on {
            "オン"
        } else {
            "オフ"
        },
        state.font_size(),
        state.theme.label()
    )
}

/// 設定画面を組み立てる。`crates/fandhe-egui-proxy/src/screens/registry.rs` から
/// `FANDHE_PROXY_APP=settings` で呼び出されるほか、`tests/settings.rs` から
/// ヘッドレス結合テストに利用される公開エントリポイント。
pub fn settings_screen() -> Screen<SettingsState> {
    Screen::new("設定")
        .toggle_switch(
            WidgetId::from_raw(1),
            "通知",
            get_notifications_on,
            set_notifications_on,
        )
        .with_node(Node::Widget(Box::new(
            Slider::bound(
                WidgetId::from_raw(2),
                "文字サイズ",
                FONT_SIZE_MIN as f32,
                FONT_SIZE_MAX as f32,
                get_font_size,
                set_font_size,
            )
            .with_step(1.0),
        )))
        .text("テーマ")
        .radio_button(
            WidgetId::from_raw(3),
            Theme::System.label(),
            Theme::System.index(),
            get_theme_index,
            set_theme_index,
        )
        .radio_button(
            WidgetId::from_raw(4),
            Theme::Light.label(),
            Theme::Light.index(),
            get_theme_index,
            set_theme_index,
        )
        .radio_button(
            WidgetId::from_raw(5),
            Theme::Dark.label(),
            Theme::Dark.index(),
            get_theme_index,
            set_theme_index,
        )
        .button(WidgetId::from_raw(6), "初期値に戻す", reset_to_defaults)
        .text("現在の設定")
        .dynamic_text(summary)
}
