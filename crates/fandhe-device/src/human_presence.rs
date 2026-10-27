//! 人の在席・離席の検知（現在の在席を 1 回読む）の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/human-presence.json`、親 #187、Issue #960）。
//!
//! 扱う範囲は「ボタンを押すと既定の在席センサーの現在値を 1 回読む」ことだけ。継続配信
//! （`ReadingChanged`）・距離・注視・覗き見・複数センサー・`HumanPresenceSettings`・
//! `Timestamp` は範囲外で、`IsPresenceSupported` も呼ばない。対象は Windows のみ
//! （他 OS は対応表で `no`）。`fandhe-egui-proxy` の `device/human_presence.rs` が
//! ワーカースレッドで [`HumanPresence::read_presence`] を呼ぶ（ブロッキングの契約）。
//!
//! # 契約
//!
//! - 結果は成功（[`Presence`]）か [`HumanPresenceFailure`]。`DeviceError` に variant を足すと
//!   既存の網羅 match が壊れるため機能固有の enum にする（`pedometer` と同じ判断）
//! - **嘘の成功の禁止**: OS が判定できないと返した [`Presence::Unknown`] は成功値として表示するが、
//!   離席に畳まない。`Unsupported` / `PermissionDenied` / `NoReading` / `Failed` も離席にしない
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 期限は [`READ_TIMEOUT`] が唯一の値。実装は引数で受けた `timeout` 以内に必ず戻り、
//!   戻る前に非同期操作を取り消し、センサーの参照を解放する（購読を残さない。類型 9）
//! - **三値**: 利用者キャンセルの経路は無い。権限拒否は [`HumanPresenceFailure::PermissionDenied`]、
//!   期限切れは [`HumanPresenceFailure::NoReading`]（失敗として表示し、成功にもキャンセルにもしない）。
//!   `Unknown` は成功として表示し、失敗の色にしない
//! - HRESULT・センサー ID・在席の値をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代照合）、`Unknown` と失敗を在席・離席と見せないこと、
//! 非搭載と拒否を成功にしないこと、呼び出しごとにセンサーの参照を解放すること。
//! 守らない: 押してから表示までの間に人が動いたことによる食い違い、OS の検知の精度・距離・遅れ、
//! 既定以外のセンサー、複数プロセス、期限後も OS 側の `GetDefaultAsync` が走り切ること
//! （`Cancel()` は呼ぶが OS が止める保証は無い）。

use std::time::Duration;

/// 在席読み取りの期限（Rust 側の唯一の値）。`GetDefaultAsync` はセンサー サービスを経由するため
/// 即時とは限らず、各 OS 実装は別の期限を持たない。
pub const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// 在席の読み取り結果（いずれも成功値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// OS が人を検知している。
    Present,
    /// OS が人を検知していない。
    NotPresent,
    /// OS が判定できないと返した。離席に畳まない。
    Unknown,
}

/// 在席読み取りの失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanPresenceFailure {
    /// センサーが無い・API が無い・OS 版が足りない。
    Unsupported,
    /// 利用者または OS 設定が在席検知の利用を拒否した。
    PermissionDenied,
    /// 期限内に値が届かない、または OS が値を持っていない。離席にはしない。
    NoReading,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// 在席読み取りの契約（モジュール doc の「契約」節）。
pub trait HumanPresence: Send + Sync {
    /// 現在の在席を 1 回読む。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出しで、
    /// `timeout` 以内に必ず戻る。
    fn read_presence(&self, timeout: Duration) -> Result<Presence, HumanPresenceFailure>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedHumanPresence;

impl HumanPresence for UnsupportedHumanPresence {
    fn read_presence(&self, _timeout: Duration) -> Result<Presence, HumanPresenceFailure> {
        Err(HumanPresenceFailure::Unsupported)
    }
}

/// OS の列挙値（`HumanPresence` enum: 0 Unknown / 1 Present / 2 NotPresent）を写す値域検査。
/// 列挙外（将来 OS が値を足した場合を含む）は `Unknown` にし、在席・離席へ寄せない。
pub fn presence_from_raw(v: i32) -> Presence {
    match v {
        1 => Presence::Present,
        2 => Presence::NotPresent,
        _ => Presence::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedHumanPresence.read_presence(READ_TIMEOUT),
            Err(HumanPresenceFailure::Unsupported)
        );
    }

    #[test]
    fn raw_values_map() {
        assert_eq!(presence_from_raw(0), Presence::Unknown);
        assert_eq!(presence_from_raw(1), Presence::Present);
        assert_eq!(presence_from_raw(2), Presence::NotPresent);
    }

    #[test]
    fn out_of_range_raw_is_unknown() {
        for v in [3, -1, i32::MAX, i32::MIN] {
            assert_eq!(presence_from_raw(v), Presence::Unknown);
        }
    }
}
