//! システムの効果音・ビープ（OS 標準の警告音を 1 回鳴らす）の抽象（Phase 11 の拡充ループ、
//! 親 #183、Issue #939、対応表 `docs/device/capabilities/system-sound.json`）。
//!
//! 扱う範囲は「OS 標準の警告音を 1 回鳴らす」ことだけ。音の種類・音量・長さ・停止・
//! 名前付きサウンド・完了の待機は扱わない。一発呼び出し型（結果を待つ要求を持たない）。
//! 対象は macOS / Windows / Android。iOS と Web は OS 標準の音を鳴らせず対応表で `no`。
//!
//! # 結果の種類
//!
//! - `Ok(BeepOutcome::Requested)`: OS に渡した。聞こえたことの保証ではない
//! - `Ok(BeepOutcome::NotPlayed)`: OS が鳴らさなかったと返した。成功にも失敗の色にもしない
//!   （サウンド設定で音を外した場合と区別できないため。契約表 10）
//! - `Err(Internal)`: OS 呼び出しの失敗（macOS で main スレッド以外から呼んだ場合を含む）
//! - `Err(Unsupported)`: 非対応 OS
//!
//! `PermissionDenied` / `Timeout` / `Cancelled` は返さない（権限は不要、OS 呼び出しは同期で
//! 戻り、取り消す操作もない）。
//!
//! # OS を正にする
//!
//! 結果は OS の戻り値だけで決める。音量・消音・サウンド設定の事前判定はせず、完了通知や
//! タイマーも待たない。鳴らせないとき既定音へ倒れる API（`MessageBeep`、`SND_NODEFAULT`
//! なしの `PlaySound`）は使わない。
//!
//! # スレッド
//!
//! macOS は main スレッドから同期で呼ぶ（`.m` は main 以外なら触らず失敗を返し、
//! `dispatch_sync` は使わない）。Windows はスレッド制約なし。Android は JNI で同期に呼ぶ。
//!
//! # 守る範囲と既知の制約（契約表 11）
//!
//! 経路は画面 → driver の 1 本だけ。守るのは、1 プロセス内の要求と完了の順序（世代の照合）、
//! `NotPlayed` と非対応を成功に畳まないこと、Android の `ToneGenerator` を `onDestroy` で
//! `release()` すること。守らないのは、実際に聞こえたか（消音・音量 0・サウンド設定）、
//! 連打時の聞こえ方（Android は前の音を止めて置き換える、macOS / Windows は OS 任せ）、
//! 音の種類と長さの OS 差、複数プロセス・複数窓、終了時に鳴っている音の後片付け（OS に任せる）。
//!
//! Android の予定: `startTone(TONE_PROP_BEEP)` の `true` は `Requested`、`false` は
//! `NotPlayed`、生成や呼び出しの例外は `Internal`。

use crate::DeviceError;

/// 警告音を OS に要求した結果（モジュール doc「結果の種類」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeepOutcome {
    /// OS に渡した。聞こえたことは保証しない。
    Requested,
    /// OS が鳴らさなかったと返した。成功にも失敗にもしない。
    NotPlayed,
}

/// システムの効果音の契約（モジュール doc）。
pub trait SystemSound: Send + Sync {
    /// OS 標準の警告音を 1 回、OS へ要求する。
    fn beep(&self) -> Result<BeepOutcome, DeviceError>;
}

/// 対象外 OS（iOS・Linux 等）・骨格段階の OS のフォールバック。ダミーの成功へは倒さない。
pub struct UnsupportedSystemSound;

impl SystemSound for UnsupportedSystemSound {
    fn beep(&self) -> Result<BeepOutcome, DeviceError> {
        Err(DeviceError::Unsupported("system_sound"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_not_a_fake_success() {
        assert!(matches!(
            UnsupportedSystemSound.beep(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
