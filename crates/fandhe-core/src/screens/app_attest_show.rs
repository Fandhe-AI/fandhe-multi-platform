//! Phase 11（端末機能の拡充ループ、親 #132）型サンプル「端末・アプリの真正性証明」（Issue #677）。
//! 端末の鍵の証明データ（Android の Key Attestation）を 1 回取得し、件数と合計サイズだけを表示する
//! 1 画面。`FANDHE_PROXY_APP=app_attest_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod app_attest_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/app_attest.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish_attest`] で状態を進める。全対象 OS の分の状態・文言は
//! ここで定義するため、OS 別の子 issue はこのファイルを触らない。iOS 再開時の
//! `serverUnavailable` は対応表で deferred のため、今は [`AttestFailure::Failed`] に寄せる。
//!
//! 画面に出すのは件数とバイト数だけ。challenge・証明データ・鍵名は表示しない。端末上では
//! 検証しないため、成功の文言で「正規」「改ざんなし」とは言わない（判断はサーバーの責務）。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_attest（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const ATTEST_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 未操作時の案内。
pub const IDLE_HINT: &str = "この端末の鍵の証明データを 1 回取得し、件数とサイズだけを表示します";
/// 取得を待っている間の案内。
pub const RUNNING_STATUS: &str = "証明データを取得しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 成功時の文言の接頭辞（サーバーで検証するまで正しさは未確定であることを必ず添える）。
pub const DONE_PREFIX: &str = "証明データを取得しました（サーバーで検証するまで未確定）: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 入力が不正だったときの案内。
pub const INVALID_INPUT_MESSAGE: &str = "証明の要求が不正でした";
/// 鍵ストアの失敗のときの案内。
pub const KEYSTORE_MESSAGE: &str = "端末の鍵ストアで鍵を扱えませんでした";
/// 空の証明データが返ったときの案内。
pub const EMPTY_MESSAGE: &str = "空の証明データが返りました";
/// 証明データが上限を超えたときの案内。
pub const TOO_LARGE_MESSAGE: &str = "証明データが大きすぎます";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に取得が終わりませんでした";

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 要求（challenge）が不正。
    InvalidInput,
    /// 鍵ストアの失敗。
    KeystoreFailed,
    /// 空のチェーン。
    Empty,
    /// 上限超過。
    TooLarge,
    /// 期限切れ。
    Timeout,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AttestPhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 取得待ち。
    Running,
    /// 成功（証明書の件数と合計バイト数。中身は保持しない）。
    Done {
        /// 証明書の件数。
        count: usize,
        /// 合計バイト数。
        total_bytes: usize,
    },
    /// 失敗。
    Failed(AttestFailure),
}

/// 真正性証明画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppAttestState {
    /// 操作の進行状態。
    pub phase: AttestPhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

fn is_busy(state: &AppAttestState) -> bool {
    matches!(state.phase, AttestPhase::Requested | AttestPhase::Running)
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_attest(state: &mut AppAttestState) {
    if is_busy(state) {
        return;
    }
    state.phase = AttestPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_attest`] へ
/// そのまま渡す。
pub fn take_request(state: &AppAttestState) -> Option<u64> {
    matches!(state.phase, AttestPhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut AppAttestState) {
    if state.phase == AttestPhase::Requested {
        state.phase = AttestPhase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_attest(
    state: &mut AppAttestState,
    generation: u64,
    result: Result<(usize, usize), AttestFailure>,
) {
    if state.phase != AttestPhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok((count, total_bytes)) => AttestPhase::Done { count, total_bytes },
        Err(f) => AttestPhase::Failed(f),
    };
}

/// 処理中の案内文言。
pub fn status_text(state: &AppAttestState) -> String {
    if is_busy(state) {
        RUNNING_STATUS.to_string()
    } else {
        String::new()
    }
}

/// 成功時の文言（件数とバイト数だけ）。
pub fn result_text(state: &AppAttestState) -> String {
    match &state.phase {
        AttestPhase::Done { count, total_bytes } => {
            format!("{DONE_PREFIX}{count} 件・{total_bytes} バイト")
        }
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &AppAttestState) -> String {
    let AttestPhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        AttestFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        AttestFailure::InvalidInput => INVALID_INPUT_MESSAGE,
        AttestFailure::KeystoreFailed => KEYSTORE_MESSAGE,
        AttestFailure::Empty => EMPTY_MESSAGE,
        AttestFailure::TooLarge => TOO_LARGE_MESSAGE,
        AttestFailure::Timeout => TIMEOUT_MESSAGE,
        AttestFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn result_nodes(state: &AppAttestState) -> Vec<Node<AppAttestState>> {
    let t = result_text(state);
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Node::Text(t)]
    }
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &AppAttestState) -> Screen<AppAttestState> {
    let screen = Screen::new("端末・アプリの真正性証明")
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if is_busy(state) {
        return screen;
    }
    screen.button(ATTEST_BUTTON_ID, "証明データを取得する", request_attest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> AppAttestState {
        let mut s = AppAttestState::default();
        request_attest(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = AppAttestState::default();
        request_attest(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_attest(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        assert_eq!(take_request(&s), None);
        request_attest(&mut s);
        assert_eq!(s.generation, 1);
        finish_attest(&mut s, 1, Ok((3, 2048)));
        assert_eq!(
            s.phase,
            AttestPhase::Done {
                count: 3,
                total_bytes: 2048
            }
        );
        request_attest(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(result_text(&s), "");
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish_attest(&mut s, 0, Ok((1, 1)));
        assert_eq!(s.phase, AttestPhase::Running);
        finish_attest(&mut s, 1, Err(AttestFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_attest(&mut s, 1, Ok((1, 1)));
        assert_eq!(s.phase, AttestPhase::Failed(AttestFailure::Timeout));
        let mut idle = AppAttestState::default();
        finish_attest(&mut idle, 0, Ok((1, 1)));
        assert_eq!(idle.phase, AttestPhase::Idle);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = AppAttestState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, AttestPhase::Idle);
    }

    #[test]
    fn done_text_shows_counts_and_claims_no_trust() {
        let mut s = running();
        finish_attest(&mut s, 1, Ok((4, 5120)));
        let t = result_text(&s);
        assert!(t.contains("4 件") && t.contains("5120 バイト"), "{t}");
        for word in ["正規", "改ざん", "安全", "信頼できる"] {
            assert!(!t.contains(word), "{word}");
        }
        assert!(t.contains("未確定"));
    }

    #[test]
    fn every_failure_has_text_without_internal_ids() {
        let all = [
            AttestFailure::Unsupported,
            AttestFailure::InvalidInput,
            AttestFailure::KeystoreFailed,
            AttestFailure::Empty,
            AttestFailure::TooLarge,
            AttestFailure::Timeout,
            AttestFailure::Failed("x".into()),
        ];
        for f in all {
            let mut s = running();
            finish_attest(&mut s, 1, Err(f.clone()));
            let t = error_text(&s);
            assert!(!t.is_empty() && !t.contains("app_attest"), "{f:?}: {t}");
            assert_eq!(result_text(&s), "");
        }
        let mut s = running();
        finish_attest(&mut s, 1, Err(AttestFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 4 ノード（案内・状態・失敗・結果）+ ボタン 1 つ
        let count = |s: &AppAttestState| current_screen(s).len() - 4;
        let mut s = AppAttestState::default();
        assert_eq!(count(&s), 1);
        request_attest(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_attest(&mut s, 1, Err(AttestFailure::Timeout));
        assert_eq!(count(&s), 1);
    }
}
