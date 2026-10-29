//! アラーム・正確な時刻の予約の抽象（Phase 11 の拡充ループ、親 #140、Issue #721）。
//!
//! 扱う範囲は「壁時計の絶対時刻を 1 件予約する → アプリが閉じていても OS がその
//! 時刻に知らせる」の 1 本道に限る。実装は Windows（`ScheduledToastNotification`、
//! `scenario="alarm"` のトースト）・iOS（AlarmKit）・Android
//! （`AlarmManager.setAlarmClock`）で、`fandhe-egui-proxy` の `device/alarm.rs` が
//! 呼ぶ。その他の OS は [`UnsupportedAlarm`] 固定（対応表 `docs/device/capabilities/
//! alarm.json`）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - **予約できない場合**: 通知が無効（Windows・Android）なら `PermissionDenied`/`Disabled`、
//!   OS 版が API に届かない（iOS 26 未満）なら `Unsupported`、過去・現在の時刻は `InvalidInput("alarm time")`、非対応は `Unsupported`、
//!   OS の失敗は `Internal` の `Err` を返す。予約の成功を装わない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）。初期化に失敗した実装は
//!   [`UnsupportedAlarm`] へ倒し、ダミーの成功実装は使わない
//! - **許可**: 実行時の許可が要る OS では、[`Alarm::schedule`] が OS の許可 UI を出して
//!   よい。iOS は未決定のときだけ許可ダイアログを出し、応答を待って結果を返す
//!   （拒否なら `PermissionDenied`）。拒否済みは UI を出さず `PermissionDenied`。
//!   Android は許可が無いとき設定の許可画面を開き、**応答を待たずに**
//!   `PermissionDenied` を返す（許可は利用者の再操作で効く）。どちらも許可 UI を
//!   出したことを成功に数えない。[`Alarm::cancel`]・[`Alarm::state`] は許可 UI を出さない
//! - **二重予約は置き換え**: [`Alarm::schedule`] は常に高々 1 件の予約にする。
//!   途中で失敗した場合、古い予約はすでに消えていることがあり、古い予約を戻さずに
//!   `Err` を返す（呼び出し側は「予約なし」と表示されうる）
//! - **取り消しはべき等**: [`Alarm::cancel`] は予約が無くても `Ok(())`。取り消しの
//!   直前に鳴っていた場合は防がない
//! - **[`Alarm::state`] は参考値**: OS の予約一覧を読んだ結果で、時刻を過ぎて一覧から
//!   消えたのか取り消されたのかは区別しない。「鳴った」とは言わない。iOS は鳴っている間
//!   （`alerting`）も一覧に残るので `Scheduled`。Android は OS に一覧が無いため、予約の
//!   有無を OS が保持する一回きりの `PendingIntent` の有無で読み、時刻は予約時に記録した
//!   値を返す（有るのに記録が無ければ `Internal`。`Scheduled` を装わない）
//! - **結果の三値**: 成功は `Ok`、利用者による取り消しは成功として `Ok(())`
//!   （失敗として見せない。契約表「10」）、失敗は `Err`
//! - **時刻の扱い**: 壁時計の絶対時刻（UNIX エポックからのミリ秒）を OS へ渡す。
//!   アプリ側で経過時間は測らない（単調時計の問い〔契約表「3」〕は該当なし）。
//!   予約後にシステム時刻が変わると、鳴る時刻もずれる
//! - **守る範囲**: 1 プロセス内の順序、嘘の成功の禁止、資源の解放。
//!   **守らない範囲**: 鳴ったかどうかの検知、スヌーズ・繰り返し、電源断の間の配信
//!   （Windows は猶予 5 分、Android は電源断で予約が消え再登録しない）、iOS のカウント
//!   ダウン・スヌーズ表示、予約後のシステム時刻変更、複数プロセス、通知・アラーム画面から
//!   アプリを開くこと
//!   （`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）

use crate::DeviceError;

/// 予約状態（[`Alarm::state`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmState {
    /// このアプリの予約が OS の一覧に無い（未予約・取り消し後・時刻経過後）。
    NotScheduled,
    /// このアプリの予約が OS の一覧に残っている。
    Scheduled {
        /// 鳴る時刻（UNIX エポックからのミリ秒）。
        fire_at_unix_ms: u64,
    },
}

/// アラームの契約（モジュール doc の「契約」節参照）。
pub trait Alarm: Send + Sync {
    /// 壁時計の絶対時刻（UNIX ms）で 1 件予約する。既存の予約は置き換える。
    fn schedule(&self, fire_at_unix_ms: u64) -> Result<(), DeviceError>;
    /// 予約を取り消す。予約が無くても `Ok(())`（べき等）。
    fn cancel(&self) -> Result<(), DeviceError>;
    /// OS の予約一覧を読み、このアプリの予約が残っているかを返す。
    fn state(&self) -> Result<AlarmState, DeviceError>;
}

/// 非対応 OS 用（初期化失敗時のフォールバックも含む）。常に
/// `Unsupported("alarm")` を返し、ダミーの成功へは倒さない。
pub struct UnsupportedAlarm;

impl Alarm for UnsupportedAlarm {
    fn schedule(&self, _fire_at_unix_ms: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("alarm"))
    }

    fn cancel(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("alarm"))
    }

    fn state(&self) -> Result<AlarmState, DeviceError> {
        Err(DeviceError::Unsupported("alarm"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_reports_success() {
        let a = UnsupportedAlarm;
        assert!(matches!(
            a.schedule(1),
            Err(DeviceError::Unsupported("alarm"))
        ));
        assert!(matches!(a.cancel(), Err(DeviceError::Unsupported("alarm"))));
        assert!(matches!(a.state(), Err(DeviceError::Unsupported("alarm"))));
    }
}
