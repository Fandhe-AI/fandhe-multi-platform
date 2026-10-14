//! Phase 11 の拡充ループ（親 #911）型サンプル「文字サイズ追従」（Issue #912）。
//! `FANDHE_PROXY_APP=text_scale_preview` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod text_scale_preview;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。OS 値の取得・実描画への倍率反映は
//! 本モジュール外（`fandhe-egui-proxy` の effect ドライバ、
//! `device/text_scale.rs`）が [`OsScale::Unread`] を観測し [`finish_read`] で
//! 結果を注入する（`http_fetch` の Requested/Idle 型状態機械と同型）。
//!
//! 見出し・本文・注釈のサイズ差を持つ描画プリミティブは DSL に無く、親 #911 で
//! 「1 段目は不要」と判断済みのため、`Screen::new` のタイトル（proxy 側で
//! `ui.heading` として描かれる）と本文 2 種（通常文 + 接頭辞つき短文）で
//! 見出し/本文/注釈の 3 段を表す（ponytail: 専用ウィジェットが要る場合は
//! `Screen` にサイズ指定つきテキストノードを追加する設計に上げる）。
//!
//! # 「使えない」と「失敗」を混同しない理由
//!
//! `biometric_check`/`share_text` と同じ再発防止方針。OS 値が読めない
//! （`fandhe_device::text_scale::TextScale::scale_factor` が `Unsupported`）ことと、
//! 何らかの内部エラーで読み取りに失敗したことは別の事象であり、[`OsScale::Unavailable`]
//! と [`OsScale::Failed`] の別バリアントで表現する（`1.0` を「読めた」と偽らない、
//! `fandhe_device::text_scale` モジュール doc の契約に対応）。

use crate::{Screen, WidgetId};

/// 倍率スライダーの `WidgetId`。
const SLIDER_ID: WidgetId = WidgetId::from_raw(1);

/// 倍率の下限（`fandhe_device::text_scale::SCALE_MIN` と同値。`fandhe-core` は
/// `fandhe-device` に依存できないため複製する。同値であることはユニットテスト
/// （`fandhe-egui-proxy` 側の `scale_range_constants_match_device_crate`）で固定する）。
pub const SCALE_MIN: f32 = 0.5;
/// 倍率の上限（`fandhe_device::text_scale::SCALE_MAX` と同値）。
pub const SCALE_MAX: f32 = 3.0;
/// OS 既定（追従設定なし）を表す倍率。
pub const SCALE_DEFAULT: f32 = 1.0;

/// 内部エラー時の案内の接頭辞（`biometric_check::FAILED_PREFIX` と同型）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// OS の文字サイズ設定の読み取り状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OsScale {
    /// 未操作・まだ読み取っていない（effect ドライバが [`finish_read`] するまでの初期状態）。
    #[default]
    Unread,
    /// OS 値を読めた（丸め済みの倍率）。
    Read(f32),
    /// この環境では OS の文字サイズ設定を読めない（`Unsupported`。失敗ではない）。
    Unavailable,
    /// 内部エラーで読み取りに失敗した（`Unsupported` 以外の `DeviceError`）。
    Failed(String),
}

/// [`finish_read`] へ渡す結果（`fandhe-egui-proxy` 側が `DeviceError` から写像する。
/// `fandhe-core` は `fandhe-device` に依存しないため、両者の型を橋渡しするのは
/// 呼び出し元の責務、`device.rs::to_note_storage_error` と同じ判断）。
#[derive(Debug, Clone, PartialEq)]
pub enum OsScaleError {
    /// OS の文字サイズ設定を読めない（`DeviceError::Unsupported`）。
    Unavailable,
    /// その他の内部エラー（`DeviceError` の `Display` 文言のみを保持する。
    /// 内部構造を画面へ漏らさない、security.md A09）。
    Failed(String),
}

/// 「文字サイズ追従」画面の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct TextScalePreviewState {
    /// 実描画へ適用する倍率（スライダー入力値。OS 値が読めた初回はその値で上書きされる）。
    pub scale: f32,
    os: OsScale,
}

impl Default for TextScalePreviewState {
    fn default() -> Self {
        Self {
            scale: SCALE_DEFAULT,
            os: OsScale::default(),
        }
    }
}

impl TextScalePreviewState {
    /// OS の文字サイズ読み取り状態（effect ドライバが未読み取りかを判定するために使う）。
    pub fn os_scale(&self) -> &OsScale {
        &self.os
    }
}

/// 非有限値（NaN/inf）は無視し、範囲外の値は [`SCALE_MIN`]`..=`[`SCALE_MAX`] へ丸める
/// （`tilt_ball::set_tilt_x` と同じ防御、信頼境界の入力検証）。
fn clamp_scale(value: f32) -> f32 {
    value.clamp(SCALE_MIN, SCALE_MAX)
}

/// `Screen::slider` の getter。
pub fn get_scale(state: &TextScalePreviewState) -> f32 {
    state.scale
}

/// `Screen::slider` の setter。非有限値は現在値を維持する
/// （`tilt_ball::set_tilt_x` と同じ防御）。
pub fn set_scale(state: &mut TextScalePreviewState, value: f32) {
    if value.is_finite() {
        state.scale = clamp_scale(value);
    }
}

/// OS 値の読み取り結果を反映する。effect ドライバが `os == Unread` のときのみ
/// 呼ぶ契約（[`os_scale`](TextScalePreviewState::os_scale) で観測してから呼ぶ、
/// `http_fetch` の Requested/Idle 型状態機械と同じ「観測してから遷移」の分担）。
/// 変更通知を持つプラットフォーム（Web の `ResizeObserver`、`fandhe-web-dom::text_scale`
/// 参照）は、値が変わったときに限り `Read` から再度呼んでよい（`Read → Read` の更新）。
///
/// `Ok(v)` は [`OsScale::Read`] へ遷移し `scale` を `v`（丸め済み想定。
/// `fandhe_device::text_scale::read_scale` が既に丸めるが、直接値が来ても
/// 二重に丸めて安全側に倒す）で上書きする。`Err(Unavailable)` は `scale` を
/// 変えずに [`OsScale::Unavailable`] へ（「使えない」であり `1.0` を代用しない、
/// モジュール doc 参照）。`Err(Failed)` は [`OsScale::Failed`] へ。
pub fn finish_read(state: &mut TextScalePreviewState, result: Result<f32, OsScaleError>) {
    match result {
        Ok(value) => {
            let value = clamp_scale(if value.is_nan() { SCALE_DEFAULT } else { value });
            state.scale = value;
            state.os = OsScale::Read(value);
        }
        Err(OsScaleError::Unavailable) => {
            state.os = OsScale::Unavailable;
        }
        Err(OsScaleError::Failed(message)) => {
            state.os = OsScale::Failed(message);
        }
    }
}

/// OS 読み取り状態の案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &TextScalePreviewState) -> String {
    match &state.os {
        OsScale::Unread => "OS の文字サイズ設定を読み取り中".to_string(),
        OsScale::Read(v) => format!("OS の文字サイズ設定: {v:.2} 倍"),
        OsScale::Unavailable => {
            "この環境では OS の文字サイズ設定を読めません（スライダーで確認できます）".to_string()
        }
        OsScale::Failed(_) => String::new(),
    }
}

/// 内部エラー時の案内文言（`error_text` の `get`。`Failed` のときのみ非空）。
pub fn error_text(state: &TextScalePreviewState) -> String {
    match &state.os {
        OsScale::Failed(message) => format!("{FAILED_PREFIX}{message}"),
        _ => String::new(),
    }
}

/// 適用中の倍率（スライダー値。実描画へ反映される値そのもの）の案内文言。
pub fn applied_text(state: &TextScalePreviewState) -> String {
    format!("適用中の倍率: {:.2} 倍", state.scale)
}

/// 「文字サイズ追従」画面。見出し（タイトル）・本文・注釈の 3 段 + 現在の OS 読み取り
/// 状態・適用中の倍率を表示し、スライダーで倍率を差し替えられる。
///
/// スライダーをタイトル直下（本文・注釈より前）に置く。本画面は `CentralPanel` 直下に
/// ノードを並べるだけで `ScrollArea` を持たない（codex-review P1 #920 指摘）ため、
/// 倍率 3.0 で本文・注釈が折り返して縦に伸びてもスライダー自身の手前の行数が増えず、
/// 常に到達可能な位置（タイトルの次）に留まる。
pub fn text_scale_preview_screen() -> Screen<TextScalePreviewState> {
    Screen::new("文字サイズ追従")
        .slider(
            SLIDER_ID, "倍率", SCALE_MIN, SCALE_MAX, get_scale, set_scale,
        )
        .text("本文: 端末の文字サイズ設定に応じて、この画面の文字も拡大・縮小されます。")
        .text("注釈: スライダーで倍率を差し替えて確認できます。")
        .dynamic_text(status_text)
        .error_text(error_text)
        .dynamic_text(applied_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口
/// （`tilt_ball::current_screen` と同型）。画面は固定 1 枚のため中身は
/// [`text_scale_preview_screen`] を返すだけでよい。
pub fn current_screen(_state: &TextScalePreviewState) -> Screen<TextScalePreviewState> {
    text_scale_preview_screen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    #[test]
    fn default_state_is_unread_with_default_scale() {
        let state = TextScalePreviewState::default();
        assert_eq!(state.scale, SCALE_DEFAULT);
        assert_eq!(state.os_scale(), &OsScale::Unread);
    }

    #[test]
    fn set_scale_ignores_non_finite_and_clamps_range() {
        let mut state = TextScalePreviewState::default();
        set_scale(&mut state, f32::NAN);
        assert_eq!(state.scale, SCALE_DEFAULT);
        set_scale(&mut state, 10.0);
        assert_eq!(state.scale, SCALE_MAX);
        set_scale(&mut state, -1.0);
        assert_eq!(state.scale, SCALE_MIN);
    }

    #[test]
    fn finish_read_ok_sets_scale_and_read_state() {
        let mut state = TextScalePreviewState::default();
        finish_read(&mut state, Ok(1.6));
        assert_eq!(state.scale, 1.6);
        assert_eq!(state.os_scale(), &OsScale::Read(1.6));
    }

    #[test]
    fn finish_read_unavailable_keeps_scale_but_marks_unavailable() {
        let mut state = TextScalePreviewState::default();
        finish_read(&mut state, Err(OsScaleError::Unavailable));
        assert_eq!(state.scale, SCALE_DEFAULT);
        assert_eq!(state.os_scale(), &OsScale::Unavailable);
        // Read(1.0) と Unavailable は表示で区別できる（status_text が異なる）。
        let mut read_default = TextScalePreviewState::default();
        finish_read(&mut read_default, Ok(1.0));
        assert_ne!(status_text(&state), status_text(&read_default));
    }

    #[test]
    fn finish_read_failed_sets_error_text() {
        let mut state = TextScalePreviewState::default();
        finish_read(
            &mut state,
            Err(OsScaleError::Failed("内部エラー".to_string())),
        );
        assert!(error_text(&state).starts_with(FAILED_PREFIX));
        assert_eq!(status_text(&state), "");
    }

    #[test]
    fn render_headless_includes_body_note_and_status() {
        let state = TextScalePreviewState::default();
        let screen = text_scale_preview_screen();
        let rendered = render_headless(&screen, &state);
        let joined = rendered.join("\n");
        assert!(joined.contains("本文:"));
        assert!(joined.contains("注釈:"));
        assert!(joined.contains("読み取り中"));
    }
}
