//! Phase 11（端末機能の拡充ループ、親 #1101）型サンプル「写真選択」（Issue #1103）。
//! 「写真を選ぶ」ボタン → OS の画像選択 → 選んだ画像を実画素で表示する 1 画面。
//! `FANDHE_PROXY_APP=photo_pick` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画（デスクトップ）と `fandhe-web-dom` の `demo.rs`（Web）から呼ばれる
//! （`screens/mod.rs` の `pub mod photo_pick;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。画素は持たず、画面状態に持つのは
//! 不透明なソース識別子（[`ShownPhoto::source`]）だけ。描画バックエンドが識別子から画素を引く
//! （デスクトップは `photo:{世代}`、Web は `blob:` URL）。識別子は [`ImageWidget`] の
//! `Bound` ソースとして常にノードを置き、中身だけを切り替える（Web の `mount` は
//! 状態が変わるたびに画面を作り直すため、状態でノードの有無を変えない）。
//!
//! # 成功・キャンセル・失敗を混同しない
//!
//! [`finish_pick`] の `Ok(Some)` は、橋渡し層が 2 段目（実際の復号確認、
//! `fandhe_device::photo_picker` の契約）を通した結果に限る。[`PhotoPickState::shown`] が
//! 成功の唯一の根拠で、キャンセル・失敗では必ず `None` に戻す（失敗表示の横に前回の画像を
//! 残さない）。新しい要求を出した時点でも表示中の画像を消す。キャンセルは `status`
//! （通常色）、失敗は `error`（`TextRole::Error`）へ書き分ける。
//!
//! 進行状態・世代照合は `file_pick` と同じ（[`PickPhase`] を再利用する）。

use crate::{ImageWidget, Node, Screen, WidgetId};

pub use super::file_pick::PickPhase;

/// 「写真を選ぶ」ボタンの `WidgetId`。
const PICK_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 画像ウィジェットの `WidgetId`。
const IMAGE_ID: WidgetId = WidgetId::from_raw(2);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "「写真を選ぶ」で画像を 1 枚選びます";
/// キャンセルされたときの案内（失敗ではない）。
pub const CANCELLED_STATUS: &str = "選択しませんでした";
/// 失敗時の案内の接頭辞（`{接頭辞}{エラー文字列}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// 表示に成功した写真の要約（橋渡し層が復号を確かめた後にだけ作られる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownPhoto {
    /// 描画バックエンドが画素を引くための不透明な識別子。
    pub source: String,
    /// 形式の表示名（`"JPEG"` 等。コアは `fandhe-device` を import しないため文字列）。
    pub format: String,
    /// 元ファイルのバイト数。
    pub size: usize,
    /// 元画像の幅（画素）。
    pub width: u32,
    /// 元画像の高さ（画素）。
    pub height: u32,
}

/// 写真選択画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhotoPickState {
    /// 選択操作の進行状態。
    pub phase: PickPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 表示中の写真。成功の唯一の根拠（モジュール doc 参照）。
    pub shown: Option<ShownPhoto>,
    /// 成功・キャンセルの案内文言（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。
    pub error: String,
}

/// 「写真を選ぶ」ボタンのハンドラ。`Idle` 以外は no-op。表示中の画像は新しい要求の
/// 時点で消す。
pub fn request_pick(state: &mut PhotoPickState) {
    if state.phase != PickPhase::Idle {
        return;
    }
    state.phase = PickPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.shown = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。戻り値の世代番号は
/// [`finish_pick`] へそのまま渡す契約。
pub fn take_request(state: &PhotoPickState) -> Option<u64> {
    (state.phase == PickPhase::Requested).then_some(state.generation)
}

/// 要求を処理中（`Picking`）へ進める。`Requested` 以外では何もしない。
pub fn mark_picking(state: &mut PhotoPickState) {
    if state.phase == PickPhase::Requested {
        state.phase = PickPhase::Picking;
    }
}

/// 選択結果を反映する。`Ok(Some)` は復号確認済みの写真、`Ok(None)` はキャンセル、
/// `Err` は失敗。`Idle` のとき・世代が一致しないときは無視する。
pub fn finish_pick(
    state: &mut PhotoPickState,
    generation: u64,
    result: Result<Option<ShownPhoto>, String>,
) {
    if state.phase == PickPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PickPhase::Idle;
    match result {
        Ok(Some(photo)) => {
            state.status = format!(
                "{}（{} バイト・{}×{}）を表示しています",
                photo.format, photo.size, photo.width, photo.height
            );
            state.error = String::new();
            state.shown = Some(photo);
        }
        Ok(None) => {
            state.status = CANCELLED_STATUS.to_string();
            state.error = String::new();
            state.shown = None;
        }
        Err(message) => {
            state.status = String::new();
            state.error = format!("{FAILED_PREFIX}{message}");
            state.shown = None;
        }
    }
}

/// 画像ウィジェットが読むソース識別子。未選択なら空文字列。
pub fn image_source(state: &PhotoPickState) -> String {
    state
        .shown
        .as_ref()
        .map(|p| p.source.clone())
        .unwrap_or_default()
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PhotoPickState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`）。
pub fn error_text(state: &PhotoPickState) -> String {
    state.error.clone()
}

/// 写真選択画面を組み立てる。画像ノードは常に置き、ボタンは `Idle` のときだけ出す。
pub fn current_screen(state: &PhotoPickState) -> Screen<PhotoPickState> {
    let screen = Screen::new("写真選択")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .with_node(Node::Widget(Box::new(ImageWidget::bound(
            IMAGE_ID,
            image_source,
            "選んだ画像",
        ))));
    if state.phase != PickPhase::Idle {
        return screen;
    }
    screen.button(PICK_BUTTON_ID, "写真を選ぶ", request_pick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> ShownPhoto {
        ShownPhoto {
            source: "photo:1".into(),
            format: "PNG".into(),
            size: 10,
            width: 4,
            height: 3,
        }
    }

    fn picking() -> PhotoPickState {
        let mut s = PhotoPickState::default();
        request_pick(&mut s);
        mark_picking(&mut s);
        s
    }

    #[test]
    fn request_advances_generation_and_clears_shown() {
        let mut s = PhotoPickState {
            shown: Some(photo()),
            ..Default::default()
        };
        request_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Requested);
        assert_eq!(s.generation, 1);
        assert!(s.shown.is_none());
        assert_eq!(image_source(&s), "");
    }

    #[test]
    fn request_is_noop_while_in_flight() {
        let mut s = picking();
        request_pick(&mut s);
        assert_eq!(s.phase, PickPhase::Picking);
        assert_eq!(s.generation, 1);
    }

    #[test]
    fn success_sets_shown_and_status() {
        let mut s = picking();
        finish_pick(&mut s, 1, Ok(Some(photo())));
        assert_eq!(s.phase, PickPhase::Idle);
        assert_eq!(image_source(&s), "photo:1");
        assert!(s.status.contains("PNG") && s.status.contains("4×3"));
        assert!(s.error.is_empty());
    }

    #[test]
    fn cancel_is_not_success() {
        let mut s = picking();
        finish_pick(&mut s, 1, Ok(None));
        assert_eq!(s.status, CANCELLED_STATUS);
        assert!(s.shown.is_none());
        assert!(s.error.is_empty());
        assert!(!s.status.contains("表示しています"));
    }

    #[test]
    fn failure_sets_error_only() {
        let mut s = picking();
        finish_pick(&mut s, 1, Err("画像を表示できませんでした".into()));
        assert!(s.shown.is_none());
        assert!(s.status.is_empty());
        assert_eq!(s.error, "失敗しました: 画像を表示できませんでした");
    }

    #[test]
    fn stale_generation_and_idle_completion_are_ignored() {
        let mut s = picking();
        finish_pick(&mut s, 99, Ok(Some(photo())));
        assert_eq!(s.phase, PickPhase::Picking);
        assert!(s.shown.is_none());
        let mut idle = PhotoPickState::default();
        finish_pick(&mut idle, 0, Ok(Some(photo())));
        assert!(idle.shown.is_none());
    }

    #[test]
    fn image_node_always_present_button_only_when_idle() {
        let idle = PhotoPickState::default();
        let busy = picking();
        let count = |s: &PhotoPickState| current_screen(s).len();
        assert_eq!(count(&idle), count(&busy) + 1);
    }
}
