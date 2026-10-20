//! Phase 11 の拡充ループ型サンプル「近くの端末との直接通信」（親 #110、Issue #511）。
//! 「交換（30 秒）」ボタン → 同じアプリを開いた近くの端末と約 30 秒の窓で互いを見つけ、固定文字列を
//! 1 回ずつ送り合い、相手の表示名と受け取った文字列を出す 1 画面。`FANDHE_PROXY_APP=peer_connect` で
//! 選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod peer_connect;` と対）。3 台以上・ファイル転送は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。実際の交換は本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/local_peer.rs`）が [`take_request`] で要求を
//! 観測し、[`finish_exchange`] で結果を注入する。状態・定数・文言は全 OS 分をここで確定し、
//! 後続の OS 実装は画面に触らない。
//!
//! # 相手なし・許可なし・非対応を混同しない理由
//!
//! 窓の間に相手が見つからないのは正常な結果で、許可なし・Wi-Fi オフ・非対応を「相手なし」と見せると
//! 利用者は原因を取り違える（`device-bridge-contracts.md` 6 節）。相手なしは `status`（通常色）、
//! 失敗は `error`（`TextRole::Error`）へ別々に書き、失敗時は相手の行を空にする。利用者が取り消す
//! 操作は無いため `Cancelled` は持たない。
//!
//! ```text
//! Idle --「交換」--> phase=Requested, generation += 1
//!   --effect が take_request で観測 → mark_exchanging--> phase=Exchanging
//!   --finish_exchange（世代が一致する完了のみ反映）--> phase=Idle + 結果表示
//! ```

use crate::{Screen, WidgetId};

/// 「交換」ボタンの `WidgetId`。
const EXCHANGE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 交換開始ボタンのラベル。
pub const EXCHANGE_BUTTON_LABEL: &str = "交換（30 秒）";
/// 交換中のボタンのラベル（押しても `start_exchange` が no-op の無効表示）。
pub const EXCHANGING_BUTTON_LABEL: &str = "交換中…";
/// 交換中の案内（自分の表示名を後ろに付ける）。
pub const EXCHANGING_STATUS_PREFIX: &str = "交換中…（約 30 秒）自分の名前: ";
/// 相手が見つからなかったときの案内（成功。失敗ではない）。
pub const EMPTY_STATUS: &str =
    "30 秒以内に相手が見つかりませんでした。近くの端末で同じ画面を開いてください";
/// 許可がないときの失敗文言（ローカルネットワークと付近のデバイスの両方）。
pub const PERMISSION_DENIED_ERROR: &str =
    "ローカルネットワーク（付近のデバイス）へのアクセスが許可されていません";
/// Wi-Fi がオフなど、今は使えないときの失敗文言。
pub const DISABLED_ERROR: &str = "Wi-Fi がオフか、今は使えません";
/// 非対応のときの失敗文言。
pub const UNSUPPORTED_ERROR: &str = "この端末では近くの端末との通信を使えません";
/// その他の失敗の接頭辞（`{接頭辞}{理由}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 画面の注記（交換の範囲）。
pub const NOTE: &str = "同じ OS の端末 2 台で、固定の文字列を 1 回ずつ送り合います（3 台以上・ファイル転送は扱いません）";

/// 交換操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが交換中（[`mark_exchanging`] 済み）。
    Exchanging,
}

/// 交換結果（`fandhe_device::local_peer::PeerExchange` に対応する画面用の型。
/// `fandhe-core` は `fandhe-device` に依存しないため、橋渡しは proxy 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    /// 相手の表示名。
    pub peer_name: String,
    /// 受け取った文字列。
    pub received: String,
}

/// 交換失敗の種別（画面表示用。契約の `DeviceError` を写した形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerError {
    /// 許可なし。
    PermissionDenied,
    /// Wi-Fi オフなど今は使えない。
    Disabled,
    /// 非対応。
    Unsupported,
    /// その他の失敗（理由文字列）。
    Other(String),
}

/// effect ドライバへ渡す要求（[`take_request`] の戻り値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeRequest {
    /// [`finish_exchange`] へそのまま渡す世代番号。
    pub generation: u64,
}

/// 近くの端末との直接通信画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerConnectState {
    /// 交換操作の進行状態。
    pub phase: PeerPhase,
    /// 直近に立てた要求の世代番号（完了通知が「今の要求」かの照合に使う）。
    pub generation: u64,
    /// 交換できた相手。相手なし・失敗は `None`。
    pub row: Option<PeerRow>,
    /// 相手なし・交換中の案内（通常色）。
    pub status: String,
    /// 失敗理由（`TextRole::Error`）。正常系は空文字列。
    pub error: String,
}

/// 「交換」ボタンのハンドラ。`Idle` 以外は二重要求を防ぐため no-op。
pub fn start_exchange(state: &mut PeerConnectState) {
    if state.phase != PeerPhase::Idle {
        return;
    }
    state.phase = PeerPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    // 前回の結果を残さない。
    state.row = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する getter（状態は変えない）。
pub fn take_request(state: &PeerConnectState) -> Option<ExchangeRequest> {
    (state.phase == PeerPhase::Requested).then_some(ExchangeRequest {
        generation: state.generation,
    })
}

/// 要求を交換中へ進める。`Requested` 以外では何もしない。同じ Mac で 2 プロセスを並べたとき
/// 見分けられるよう、自分の表示名を案内に出す。
pub fn mark_exchanging(state: &mut PeerConnectState, own_name: &str) {
    if state.phase == PeerPhase::Requested {
        state.phase = PeerPhase::Exchanging;
        state.status = format!("{EXCHANGING_STATUS_PREFIX}{own_name}");
    }
}

/// 交換結果を反映する。`Idle` のとき、または世代が現在と一致しないとき
/// （新しい要求で既に上書きされた完了）は無視する。
pub fn finish_exchange(
    state: &mut PeerConnectState,
    generation: u64,
    result: Result<Option<PeerRow>, PeerError>,
) {
    if state.phase == PeerPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = PeerPhase::Idle;
    state.row = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(None) => state.status = EMPTY_STATUS.to_string(),
        Ok(Some(row)) => state.row = Some(row),
        Err(PeerError::PermissionDenied) => state.error = PERMISSION_DENIED_ERROR.to_string(),
        Err(PeerError::Disabled) => state.error = DISABLED_ERROR.to_string(),
        Err(PeerError::Unsupported) => state.error = UNSUPPORTED_ERROR.to_string(),
        Err(PeerError::Other(message)) => state.error = format!("{FAILED_PREFIX}{message}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &PeerConnectState) -> String {
    state.status.clone()
}

/// 失敗文言（`error_text` の `get`）。
pub fn error_text(state: &PeerConnectState) -> String {
    state.error.clone()
}

/// 相手の表示名の行（相手なしは空）。
pub fn peer_text(state: &PeerConnectState) -> String {
    state
        .row
        .as_ref()
        .map(|r| format!("相手: {}", r.peer_name))
        .unwrap_or_default()
}

/// 受信文字列の行（相手なしは空）。
pub fn received_text(state: &PeerConnectState) -> String {
    state
        .row
        .as_ref()
        .map(|r| format!("受信: {}", r.received))
        .unwrap_or_default()
}

/// 画面を組み立てる。ボタンは常に出し、処理中はラベルだけ [`EXCHANGING_BUTTON_LABEL`] に変える
/// （押しても `start_exchange` が no-op。要素の path を安定させる `lan_discover` と同じ判断）。
pub fn current_screen(state: &PeerConnectState) -> Screen<PeerConnectState> {
    let label = if state.phase == PeerPhase::Idle {
        EXCHANGE_BUTTON_LABEL
    } else {
        EXCHANGING_BUTTON_LABEL
    };
    Screen::new("近くの端末との直接通信")
        .text(NOTE)
        .button(EXCHANGE_BUTTON_ID, label, start_exchange)
        .dynamic_text(status_text)
        .dynamic_text(peer_text)
        .dynamic_text(received_text)
        .error_text(error_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> PeerRow {
        PeerRow {
            peer_name: "fandhe-00000001".into(),
            received: "hello".into(),
        }
    }

    fn exchanging() -> PeerConnectState {
        let mut s = PeerConnectState::default();
        start_exchange(&mut s);
        mark_exchanging(&mut s, "fandhe-0000000a");
        s
    }

    #[test]
    fn start_advances_generation_and_clears_previous_result() {
        let mut s = PeerConnectState {
            row: Some(row()),
            status: "x".into(),
            error: "y".into(),
            ..Default::default()
        };
        start_exchange(&mut s);
        assert_eq!((s.phase, s.generation), (PeerPhase::Requested, 1));
        assert!(s.row.is_none() && s.status.is_empty() && s.error.is_empty());
    }

    #[test]
    fn exchanging_press_is_noop_and_status_shows_own_name() {
        let mut s = exchanging();
        assert!(s.status.ends_with("fandhe-0000000a"));
        assert_eq!(take_request(&s), None);
        start_exchange(&mut s);
        assert_eq!((s.phase, s.generation), (PeerPhase::Exchanging, 1));
    }

    #[test]
    fn empty_result_is_success_not_error() {
        let mut s = exchanging();
        finish_exchange(&mut s, 1, Ok(None));
        assert_eq!(s.phase, PeerPhase::Idle);
        assert_eq!(s.status, EMPTY_STATUS);
        assert_eq!(s.error, "");
        assert!(s.row.is_none());
    }

    #[test]
    fn failures_are_distinct_errors_and_drop_row() {
        let cases = [
            (
                PeerError::PermissionDenied,
                PERMISSION_DENIED_ERROR.to_string(),
            ),
            (PeerError::Disabled, DISABLED_ERROR.to_string()),
            (PeerError::Unsupported, UNSUPPORTED_ERROR.to_string()),
            (PeerError::Other("x".into()), format!("{FAILED_PREFIX}x")),
        ];
        for (err, expected) in cases {
            let mut s = exchanging();
            s.row = Some(row());
            finish_exchange(&mut s, 1, Err(err));
            assert_eq!(s.error, expected);
            assert!(s.row.is_none());
            assert_eq!(s.status, "");
        }
    }

    #[test]
    fn stale_generation_and_idle_completions_are_ignored() {
        let mut s = exchanging();
        finish_exchange(&mut s, 99, Ok(None));
        assert_eq!(s.phase, PeerPhase::Exchanging);
        let mut idle = PeerConnectState {
            status: "変わらない".into(),
            ..Default::default()
        };
        finish_exchange(&mut idle, 0, Ok(None));
        assert_eq!(idle.status, "変わらない");
    }

    #[test]
    fn found_peer_is_shown() {
        let mut s = exchanging();
        finish_exchange(&mut s, 1, Ok(Some(row())));
        assert_eq!(peer_text(&s), "相手: fandhe-00000001");
        assert_eq!(received_text(&s), "受信: hello");
        assert_eq!((s.status.as_str(), s.error.as_str()), ("", ""));
    }

    #[test]
    fn button_is_inert_while_exchanging_and_clickable_when_idle() {
        use crate::click_button;
        let mut s = exchanging();
        assert!(click_button(
            &current_screen(&s),
            EXCHANGING_BUTTON_LABEL,
            &mut s
        ));
        assert_eq!((s.phase, s.generation), (PeerPhase::Exchanging, 1));
        let mut idle = PeerConnectState::default();
        assert!(click_button(
            &current_screen(&idle),
            EXCHANGE_BUTTON_LABEL,
            &mut idle
        ));
        assert_eq!(idle.phase, PeerPhase::Requested);
    }
}
