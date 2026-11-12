//! Phase 11 の拡充ループ（親 #87、Issue #276）: 疑似着信を OS の着信 UI に出し、応答・拒否・終了の
//! 結果を画面へ返す契約。`fandhe-egui-proxy` の `device/call_ui.rs`（effect ドライバ）が
//! [`CallUi::report_incoming`] / [`CallUi::end`] を呼び、各 OS 実装（iOS: CallKit = #277、
//! Android: self-managed `ConnectionService` = #278）が本トレイトを実装する。macOS・Windows・Web は
//! 対象外で [`UnsupportedCallUi`] 固定。
//!
//! 範囲は「アプリ内の操作で疑似着信を 1 件出し、結果を返す」ことのみ。固定の発信者名を使う。
//! 音声・VoIP push・発信・履歴・複数通話・電話番号の入力は扱わない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`CallUiEvent::Answered`]。受理の確証（OS が着信を受け付けた）を得たときだけ
//!   [`CallUiEvent::Ringing`] を送る
//! - 正常終了（失敗として見せない）: [`CallUiEvent::Ended`] の [`EndReason`] 4 種
//! - 失敗: [`CallUiFailure`] 4 種
//!   - `Unsupported`: 非対応 OS、Android API 24〜25、Telecom の無い端末
//!   - `PermissionDenied`: 通知の許可拒否・通知無効
//!   - `Disallowed`: OS が着信を拒んだ（iOS のおやすみモード・ブロックリスト、
//!     Android の `isIncomingCallPermitted == false`）
//!   - `Failed`: それ以外（iOS の `providerDidReset` を含む）
//!
//! OS の生コード（`CXErrorCodeIncomingCallError` 等）は上の 4 種へ全コード写像し、`-1` 等へ潰さない。
//!
//! # 世代と期限（契約表 2・3）
//!
//! - `call_id` はドライバが発番する。一致しない通知は OS 側・ドライバ側の両方で捨てる。取り消し後に
//!   OS の受理が遅れて届いたら、OS 側で即座に閉じる
//! - 期限は Rust 側（ドライバ）だけが持つ（[`REPORT_DEADLINE`]・[`RING_TIMEOUT`]）。OS 側に独自タイマー
//!   を持たせない。経過時間が要るなら単調時計を使う
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序、嘘の成功の禁止、通知・`Connection`・`CXCall` の解放。
//! 守らない範囲: 背景での通話の継続、プロセス死、iOS で着信中に背景へ移ったときの扱い（前景復帰時に
//! ドライバが期限を判定する）、他アプリ通話との調停結果の表示の時間差、Bluetooth・車載機器での表示、
//! `PhoneAccount` 登録を残すこと（冪等）。
//!
//! 並行性の集約先は OS ごとに 1 本（iOS は main queue、Android は main looper）。sink は
//! ブロックしない。OS の生コード・call_id・発信者情報はログに出さない（security.md A09）。
//! 本ファイルの定数は仮置き値で、実機の確認（#275）で見直しうる。

use std::sync::Arc;
use std::time::Duration;

/// `report_incoming` から [`CallUiEvent::Ringing`] / [`CallUiEvent::Failed`] までの期限。
pub const REPORT_DEADLINE: Duration = Duration::from_secs(5);
/// [`CallUiEvent::Ringing`] から応答までの期限。切れたら `Ended(Unanswered)`。
pub const RING_TIMEOUT: Duration = Duration::from_secs(30);

/// 通話の終わり方。いずれも失敗ではない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// 利用者が拒否した。
    Declined,
    /// 相手側（OS）が終了した。
    RemoteEnded,
    /// 利用者が画面から取り消した・終了した。
    LocalEnded,
    /// 期限内に応答されなかった。
    Unanswered,
}

/// 失敗の種別（発生条件はモジュール doc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallUiFailure {
    /// 非対応。
    Unsupported,
    /// 通知が許可されていない。
    PermissionDenied,
    /// OS が着信を受け付けなかった。
    Disallowed,
    /// それ以外。
    Failed,
}

/// OS 実装から sink へ流す通知。全 variant が `call_id` を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallUiEvent {
    /// OS が着信を受理し、呼び出し中になった。
    Ringing { call_id: u64 },
    /// 利用者が応答した。
    Answered { call_id: u64 },
    /// 通話が終わった（失敗ではない）。
    Ended { call_id: u64, reason: EndReason },
    /// 非同期に確定した失敗。
    Failed {
        call_id: u64,
        failure: CallUiFailure,
    },
}

/// 通知の届け先。戻り値 `false` は「受け手がいない」。
pub type CallUiSink = Arc<dyn Fn(CallUiEvent) -> bool + Send + Sync>;

/// 着信 UI の契約（プロセスに 1 つ）。
pub trait CallUi: Send + Sync {
    /// 疑似着信を 1 件出す。同期で確定する失敗は戻り値で返し、受理は後から `Ringing` で通知する。
    fn report_incoming(&self, call_id: u64, sink: CallUiSink) -> Result<(), CallUiFailure>;
    /// 着信中なら取り消し、通話中なら切断する。冪等で失敗しない。不一致の id では何もしない。
    fn end(&self, call_id: u64);
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功値へ倒さない。
pub struct UnsupportedCallUi;

impl CallUi for UnsupportedCallUi {
    fn report_incoming(&self, _call_id: u64, _sink: CallUiSink) -> Result<(), CallUiFailure> {
        Err(CallUiFailure::Unsupported)
    }
    fn end(&self, _call_id: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn unsupported_never_succeeds_and_never_notifies() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let sink: CallUiSink = Arc::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert_eq!(
            UnsupportedCallUi.report_incoming(1, sink),
            Err(CallUiFailure::Unsupported)
        );
        UnsupportedCallUi.end(1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
