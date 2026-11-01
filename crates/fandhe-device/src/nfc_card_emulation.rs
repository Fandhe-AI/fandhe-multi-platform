//! NFC カードエミュレーション（HCE）の状態（Phase 11 の拡充ループ、親 #151、Issue #1128）。
//!
//! 扱う範囲は「固定の独自 AID への SELECT に固定の応答を返す HCE の状態を読み出す」ところまで。
//! 応答そのものは OS 側のサービス（Android は `HostApduService`）が返し、本 trait は状態の読み出しだけを
//! 持つ。実カードの模擬・決済・応答内容の変更は扱わない。呼び出し元は `fandhe-egui-proxy` の
//! `device/nfc_card_emulation.rs`（画面向け effect ドライバ）と各 OS 実装。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は 3 種**: `Ok(HceStatus)`、`Err(Internal)`（JNI 失敗・Activity の一時的な不在。
//!   `Unsupported` にするとドライバが読み出しをやめ、復帰しても戻らないためこちらにする）、
//!   `Err(Unsupported)`（Android 以外、`FEATURE_NFC_HOST_CARD_EMULATION` が無い、`NfcAdapter` が null）。
//!   初期化できない実装もダミーの成功へ倒さず [`UnsupportedCardEmulation`] へ倒す
//! - **NFC がオフでも失敗にしない**: `nfc_enabled=false` の `Ok` で返す。NFC の有効と既定の振り先は
//!   読むたびに OS（`NfcAdapter.isEnabled`・`CardEmulation.isDefaultServiceForAid`）へ問い、アプリ内に持たない
//! - **応答回数**: `responses` は「OS 側のサービスが応答を返した回数」で、リーダーが受け取ったことは
//!   保証しない。`since_last_ms` は単調時計（Android は `SystemClock.elapsedRealtime`）で測り、
//!   応答 0 回なら `None`（番兵値を使わない）
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 利用者のキャンセル操作は
//!   無い。実行時の権限ダイアログも無い（`NFC` は normal 権限）ので拒否も無い。成功・NFC オフ（正常）・
//!   失敗を混ぜない
//! - **AID の正はこのモジュールの [`AID`] だけ**。Java へは JNI の引数で渡す。`res/xml/apduservice.xml`
//!   への複製は Android 実装（#1129）の `include_str!` テストで固定する
//! - **応答の中身・OS の例外文言をログ・`Debug`・エラー文言に出さない**
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内で読み出しが高々 1 件、古い世代の完了を捨てること、失敗を 0 回や成功として
//! 見せないこと、嘘の成功の禁止。
//! 守らない: 他アプリが同じ AID を登録したときの振り先（`default_for_aid=false` で出す）、画面オフ・
//! ロック中、Secure NFC、リーダーの再試行による重複計数、プロセス終了で回数が 0 に戻ること、
//! ポーリング間隔ぶんの表示の遅れ、iOS（Apple の entitlement 承認待ち）。

use crate::DeviceError;

/// 本アプリの HCE が応答する独自 AID（16 進）。`F0` + ASCII "FANDHE" + `01`（ISO/IEC 7816-4 の
/// 独自範囲、5〜16 バイト）。
pub const AID: &str = "F046414E44484501";

/// HCE の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HceStatus {
    /// NFC が有効か。オフは失敗ではない。
    pub nfc_enabled: bool,
    /// この AID の既定の振り先が本アプリか。
    pub default_for_aid: bool,
    /// OS 側のサービスが応答を返した回数（このプロセス内）。
    pub responses: u64,
    /// 最後の応答からの経過ミリ秒。応答 0 回なら `None`。
    pub since_last_ms: Option<u64>,
}

/// HCE の状態の読み出し。ブロックしうるため UI スレッドではなくワーカーで呼ぶ。
pub trait CardEmulation: Send + Sync {
    /// 現在の状態を読む。
    fn status(&self) -> Result<HceStatus, DeviceError>;
}

/// 非対応 OS 向け。常に `Err(Unsupported)`。
pub struct UnsupportedCardEmulation;

impl CardEmulation for UnsupportedCardEmulation {
    fn status(&self) -> Result<HceStatus, DeviceError> {
        Err(DeviceError::Unsupported("nfc_card_emulation"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aid_is_proprietary_even_hex_within_limit() {
        assert!(AID.starts_with("F0"));
        assert_eq!(AID.len() % 2, 0);
        assert!(
            AID.chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        );
        assert!((5..=16).contains(&(AID.len() / 2)));
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedCardEmulation.status(),
            Err(DeviceError::Unsupported("nfc_card_emulation"))
        ));
    }
}
