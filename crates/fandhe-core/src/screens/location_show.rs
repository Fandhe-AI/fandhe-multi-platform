//! Phase 11 の拡充ループ（親 #887）型サンプル「位置情報」（Issue #888）。
//! `FANDHE_PROXY_APP=location_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod location_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の測位は本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/location.rs`）が
//! `fandhe_device::location::SharedLocation` を pull し、[`apply_fix`] /
//! [`apply_failure`] を呼んで結果を注入する。
//!
//! # 4 状態を区別する理由
//!
//! 「取得中」「取得できた」「拒否された」「使えない」を同じ文言・同じ画面レイアウトで
//! 潰さない（`fandhe_device::location` モジュール doc の対応表が示す失敗理由の粒度を
//! 画面側でも保つ）。デモ用の座標注入は作らない
//! （実座標が無いと「取得できた」を作れないが、それは Mac 目視のキャプチャが
//! 「使えない」状態になる理由でもある。`clipboard` の「嘘の成功を作らない」判断と同じ）。

use crate::{Screen, WidgetId};

/// 「取得中」の案内文言。
pub const WAITING_TEXT: &str = "位置情報を取得中…";
/// 「拒否された」の案内文言。
pub const DENIED_TEXT: &str = "位置情報の利用が拒否されました";
/// 「使えない」（プラットフォーム未対応）の案内文言。
pub const UNAVAILABLE_TEXT: &str = "この環境では位置情報を使えません";
/// 「使えない」（位置情報サービス自体が無効）の案内文言。
pub const SERVICE_DISABLED_TEXT: &str = "位置情報サービスがオフです";
/// 「再試行中」の案内文言（タイムアウト・その他の失敗）。
pub const RETRYING_TEXT: &str = "取得できませんでした（再試行中）";

const RETRY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 画面が表示しうる状態。「未取得」と「(0, 0) に取得できた」を型で区別する契約
/// （`fandhe_device::location::Location::last_fix` の doc と対応）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LocationView {
    /// まだ測位できていない。
    Waiting,
    /// 測位できた。
    Fixed { lat: f64, lon: f64, accuracy_m: f64 },
    /// 権限が拒否された。
    Denied,
    /// 位置情報サービス自体が無効（OS 設定）。
    Disabled,
    /// このプラットフォームでは未対応。
    Unavailable,
    /// タイムアウト・その他の失敗（再試行の余地がある）。
    Retryable,
}

impl Default for LocationView {
    fn default() -> Self {
        Self::Waiting
    }
}

/// 位置情報画面の状態。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LocationShowState {
    pub view: LocationView,
    /// 利用者が「再試行」ボタンを押した回数（[`retry`] のみが加算する世代番号）。
    /// effect ドライバ（`fandhe-egui-proxy` の `device/location.rs::location_effect`）が
    /// 前回処理済みの値と比較し、増えていたときだけ測位を再要求する契約。OS 起点の
    /// 回復（例: Android `onProviderEnabled` からの `nativeOnLocationRetrying`）は
    /// [`apply_waiting`] を呼ぶだけで本フィールドを進めないため、`view` の遷移だけを
    /// 見て「再試行」と誤認することがない（codex-review P1 指摘、PR #894）。
    pub retry_seq: u64,
}

/// 測位結果を反映する（effect ドライバが `SharedLocation::last_fix()` の
/// `Ok(Some(fix))` を pull したときに呼ぶ契約）。
pub fn apply_fix(state: &mut LocationShowState, lat: f64, lon: f64, accuracy_m: f64) {
    state.view = LocationView::Fixed {
        lat,
        lon,
        accuracy_m,
    };
}

/// 「未取得」（`Ok(None)`）を反映する。
pub fn apply_waiting(state: &mut LocationShowState) {
    state.view = LocationView::Waiting;
}

/// 失敗の種別を画面表示へ反映する（effect ドライバが `fandhe_device::DeviceError` の
/// バリアントを見て呼び分ける契約。`fandhe-core` は `fandhe-device` に依存しないため、
/// バリアント名の対応は effect ドライバ側の責務）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    PermissionDenied,
    ServiceDisabled,
    Unsupported,
    Retryable,
}

pub fn apply_failure(state: &mut LocationShowState, kind: FailureKind) {
    state.view = match kind {
        FailureKind::PermissionDenied => LocationView::Denied,
        FailureKind::ServiceDisabled => LocationView::Disabled,
        FailureKind::Unsupported => LocationView::Unavailable,
        FailureKind::Retryable => LocationView::Retryable,
    };
}

/// 「再試行」ボタンのハンドラ。表示を「取得中」へ戻し、[`LocationShowState::retry_seq`]
/// を進める。実測位の再開始は、この世代番号の増加を effect ドライバが検知して行う
/// （`retry_seq` の doc 参照。利用者の明示操作のみを識別する契約のため、OS 起点の
/// 回復通知（[`apply_waiting`]）はここを通らず世代番号を進めない）。
pub fn retry(state: &mut LocationShowState) {
    state.view = LocationView::Waiting;
    state.retry_seq = state.retry_seq.wrapping_add(1);
}

/// 案内文言（`DynamicText` の `get`）。4 状態＋座標の表示を 1 箇所に集約する
/// （表示文言の SSOT、実装計画 §3.5 参照）。
pub fn status_text(state: &LocationShowState) -> String {
    match state.view {
        LocationView::Waiting => WAITING_TEXT.to_string(),
        LocationView::Fixed {
            lat,
            lon,
            accuracy_m,
        } => format!("緯度 {lat:.6} / 経度 {lon:.6} / 精度 ±{accuracy_m:.0} m"),
        LocationView::Denied => DENIED_TEXT.to_string(),
        LocationView::Disabled => SERVICE_DISABLED_TEXT.to_string(),
        LocationView::Unavailable => UNAVAILABLE_TEXT.to_string(),
        LocationView::Retryable => RETRYING_TEXT.to_string(),
    }
}

/// 位置情報画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。
pub fn current_screen(_state: &LocationShowState) -> Screen<LocationShowState> {
    Screen::new("位置情報")
        .dynamic_text(status_text)
        .button(RETRY_BUTTON_ID, "再試行", retry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_button;

    #[test]
    fn default_state_is_waiting_with_no_coordinates() {
        let state = LocationShowState::default();
        assert_eq!(state.view, LocationView::Waiting);
        assert_eq!(status_text(&state), WAITING_TEXT);
        assert!(!status_text(&state).contains("緯度"));
    }

    #[test]
    fn apply_fix_shows_coordinates_and_accuracy() {
        let mut state = LocationShowState::default();
        apply_fix(&mut state, 35.681236, 139.767125, 12.0);
        let text = status_text(&state);
        assert!(text.contains("35.681236"));
        assert!(text.contains("139.767125"));
        assert!(text.contains("±12"));
    }

    #[test]
    fn zero_zero_fix_is_distinguishable_from_waiting() {
        // 受入 2 の画面側担保: (0, 0) は「取得中」と別の文言になる。
        let mut waiting = LocationShowState::default();
        let mut fixed = LocationShowState::default();
        apply_fix(&mut fixed, 0.0, 0.0, 5.0);

        assert_ne!(status_text(&waiting), status_text(&fixed));
        assert_eq!(status_text(&waiting), WAITING_TEXT);
        apply_waiting(&mut waiting);
        assert_eq!(waiting.view, LocationView::Waiting);
    }

    #[test]
    fn apply_failure_distinguishes_all_kinds() {
        let mut denied = LocationShowState::default();
        apply_failure(&mut denied, FailureKind::PermissionDenied);
        assert_eq!(status_text(&denied), DENIED_TEXT);

        let mut disabled = LocationShowState::default();
        apply_failure(&mut disabled, FailureKind::ServiceDisabled);
        assert_eq!(status_text(&disabled), SERVICE_DISABLED_TEXT);

        let mut unavailable = LocationShowState::default();
        apply_failure(&mut unavailable, FailureKind::Unsupported);
        assert_eq!(status_text(&unavailable), UNAVAILABLE_TEXT);

        let mut retryable = LocationShowState::default();
        apply_failure(&mut retryable, FailureKind::Retryable);
        assert_eq!(status_text(&retryable), RETRYING_TEXT);

        // 4 状態すべてが互いに異なる文言を持つ（区別できることの固定）。
        let texts = [
            status_text(&denied),
            status_text(&disabled),
            status_text(&unavailable),
            status_text(&retryable),
        ];
        for (i, a) in texts.iter().enumerate() {
            for (j, b) in texts.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn retry_resets_to_waiting() {
        let mut state = LocationShowState::default();
        apply_failure(&mut state, FailureKind::Retryable);
        assert_eq!(state.view, LocationView::Retryable);

        let screen = current_screen(&state);
        assert!(click_button(&screen, "再試行", &mut state));
        assert_eq!(state.view, LocationView::Waiting);
    }

    #[test]
    fn retry_advances_retry_seq_but_apply_waiting_does_not() {
        // `retry_seq` は「再試行」ボタン経由のみで進む世代番号（codex-review P1
        // 指摘、PR #894）。OS 起点の回復通知に相当する `apply_waiting` は表示のみ
        // 戻し、世代番号を進めない。
        let mut state = LocationShowState::default();
        assert_eq!(state.retry_seq, 0);

        apply_waiting(&mut state);
        assert_eq!(state.retry_seq, 0);

        retry(&mut state);
        assert_eq!(state.retry_seq, 1);

        retry(&mut state);
        assert_eq!(state.retry_seq, 2);
    }

    #[test]
    fn screen_renders_without_panicking_in_every_state() {
        // センサ非搭載・権限なしでも画面が壊れない（tilt_ball と同じ固定 1 画面 +
        // dynamic_text の方針）。
        for view in [
            LocationView::Waiting,
            LocationView::Fixed {
                lat: 1.0,
                lon: 2.0,
                accuracy_m: 3.0,
            },
            LocationView::Denied,
            LocationView::Disabled,
            LocationView::Unavailable,
            LocationView::Retryable,
        ] {
            let state = LocationShowState {
                view,
                ..LocationShowState::default()
            };
            let _screen = current_screen(&state);
            assert!(!status_text(&state).is_empty());
        }
    }
}
