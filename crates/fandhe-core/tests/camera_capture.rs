//! `crate::screens::camera_capture`（Issue #749）の結合テスト。`render_headless` の
//! 行内容・`click_button`（動的生成子内のボタン解決）を通して、
//! 「撮影 → 撮影中 → 成功/失敗 → 撮り直す」の各経路が `current_screen(&state)` の
//! 実際の描画・イベント配線で成立することを検証する（`tests/confirm_dialog.rs` と同型）。

use fandhe_core::screens::camera_capture::{
    CameraCaptureState, CaptureFailure, CapturePhase, THUMB_ROWS, Thumbnail, cancel_capture,
    current_screen, finish_capture, mark_capturing, start_capture, status_text,
    thumbnail_from_rgba,
};
use fandhe_core::{click_button, render_headless};

/// 16x8 の 1px 幅チェッカー（列ごとに白黒交互）RGBA を作る。`thumbnail_from_rgba` が
/// 幅を超えない場合は各出力列が対応する 1 列をそのまま反映することを確認するための
/// フィクスチャ。
fn checkerboard_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _y in 0..height {
        for x in 0..width {
            let v = if x % 2 == 0 { 255 } else { 0 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    rgba
}

fn sample_thumbnail() -> Thumbnail {
    thumbnail_from_rgba(16, 8, &checkerboard_rgba(16, 8)).expect("有効な RGBA")
}

#[test]
fn thumbnail_from_rgba_rejects_length_mismatch_and_zero_dimension() {
    assert!(thumbnail_from_rgba(4, 4, &[0; 10]).is_none());
    assert!(thumbnail_from_rgba(0, 4, &[]).is_none());
    assert!(thumbnail_from_rgba(4, 0, &[]).is_none());
}

#[test]
fn thumbnail_from_rgba_alternates_on_checkerboard() {
    let thumb = thumbnail_from_rgba(16, 8, &checkerboard_rgba(16, 8)).expect("有効な RGBA");
    assert_eq!(thumb.rows.len(), 8usize.min(THUMB_ROWS));
    let first_row: Vec<char> = thumb.rows[0].chars().collect();
    assert_eq!(first_row[0], '█');
    assert_eq!(first_row[1], '░');
    assert_eq!(first_row[2], '█');
}

#[test]
fn thumbnail_from_rgba_caps_rows_at_thumb_rows() {
    // 縦幅が THUMB_ROWS を超える画像は行数が THUMB_ROWS に収まる（状態肥大防止）。
    let rgba = vec![0u8; 32 * 40 * 4];
    let thumb = thumbnail_from_rgba(32, 40, &rgba).expect("有効な RGBA");
    assert_eq!(thumb.rows.len(), THUMB_ROWS);
}

#[test]
fn initial_render_shows_idle_text_and_capture_button_only() {
    let state = CameraCaptureState::default();
    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# カメラ撮影");
    assert!(lines.contains(&"「撮影」を押すと 1 枚撮影します".to_string()));
    assert!(lines.contains(&"(撮影)".to_string()));
    assert!(!lines.contains(&"(撮り直す)".to_string()));
}

#[test]
fn click_capture_transitions_to_requested_and_hides_buttons() {
    let mut state = CameraCaptureState::default();
    let screen = current_screen(&state);
    assert!(click_button(&screen, "撮影", &mut state));
    assert_eq!(state.phase, CapturePhase::Requested);

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(lines.contains(&"撮影中…".to_string()));
    assert!(!lines.contains(&"(撮影)".to_string()));
    assert!(!lines.contains(&"(撮り直す)".to_string()));

    // Requested 中の再クリックは no-op（screen にボタン自体が無いため click_button も false）。
    assert!(!click_button(&screen, "撮影", &mut state));
    assert_eq!(state.phase, CapturePhase::Requested);
}

#[test]
fn mark_capturing_only_transitions_from_requested() {
    let mut state = CameraCaptureState::default();
    mark_capturing(&mut state);
    assert_eq!(state.phase, CapturePhase::Idle, "Idle からは遷移しない");

    let mut state = CameraCaptureState {
        phase: CapturePhase::Requested,
    };
    mark_capturing(&mut state);
    assert_eq!(state.phase, CapturePhase::Capturing);
}

#[test]
fn finish_capture_ignored_outside_capturing() {
    let mut state = CameraCaptureState::default();
    finish_capture(&mut state, Err(CaptureFailure::Unsupported));
    assert_eq!(
        state.phase,
        CapturePhase::Idle,
        "Capturing 以外での完了は無視する（古い完了の防御）"
    );
}

/// PR #791 レビュー指摘対応: ユーザーによるキャンセルは `Failed` にせず `Idle` へ戻す
/// （`UIImagePickerController` のキャンセルは正常操作であり失敗表示すべきではない）。
#[test]
fn cancel_capture_returns_to_idle_from_capturing() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    cancel_capture(&mut state);
    assert_eq!(state.phase, CapturePhase::Idle);
}

#[test]
fn cancel_capture_ignored_outside_capturing() {
    let mut state = CameraCaptureState::default();
    cancel_capture(&mut state);
    assert_eq!(
        state.phase,
        CapturePhase::Idle,
        "Idle 以外の想定外遷移が起きていないことの確認（Capturing 以外では無視する）"
    );
}

#[test]
fn finish_capture_success_shows_thumbnail_and_retake_button() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    finish_capture(&mut state, Ok(sample_thumbnail()));
    assert!(matches!(state.phase, CapturePhase::Captured(_)));

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("撮影結果（16×8、サムネイルはモザイク表示）"))
    );
    assert!(lines.iter().any(|l| l.contains('█') || l.contains('░')));
    assert!(lines.contains(&"(撮り直す)".to_string()));
    assert!(!lines.contains(&"(撮影)".to_string()));
}

#[test]
fn finish_capture_permission_denied_shows_message_and_retake_button() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    finish_capture(&mut state, Err(CaptureFailure::PermissionDenied));
    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(
        lines.contains(
            &"カメラの権限が拒否されています。端末の設定から許可してください".to_string()
        )
    );
    assert!(lines.contains(&"(撮り直す)".to_string()));
}

#[test]
fn finish_capture_unsupported_and_other_show_expected_messages() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    finish_capture(&mut state, Err(CaptureFailure::Unsupported));
    assert_eq!(status_text(&state), "この環境ではカメラを利用できません");

    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    finish_capture(
        &mut state,
        Err(CaptureFailure::Other("画像データが不正です".to_string())),
    );
    assert_eq!(
        status_text(&state),
        "撮影に失敗しました（画像データが不正です）"
    );
}

#[test]
fn retake_clears_thumbnail_and_returns_to_requested() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    finish_capture(&mut state, Ok(sample_thumbnail()));

    let screen = current_screen(&state);
    assert!(click_button(&screen, "撮り直す", &mut state));
    assert_eq!(state.phase, CapturePhase::Requested);

    let screen = current_screen(&state);
    let lines = render_headless(&screen, &state);
    assert!(!lines.iter().any(|l| l.contains('█') || l.contains('░')));
}

#[test]
fn start_capture_is_noop_while_requested_or_capturing() {
    let mut state = CameraCaptureState {
        phase: CapturePhase::Requested,
    };
    start_capture(&mut state);
    assert_eq!(state.phase, CapturePhase::Requested);

    let mut state = CameraCaptureState {
        phase: CapturePhase::Capturing,
    };
    start_capture(&mut state);
    assert_eq!(state.phase, CapturePhase::Capturing);
}
