//! NFC カードエミュレーション（HCE、nfc-card-emulation）の機能 crate（REQ-21 / TASK-21.3、Issue #1172）。
//!
//! 契約（[`CardEmulation`]・[`HceStatus`]・[`AID`]・[`CardEmulationError`]・[`UnsupportedCardEmulation`]）、
//! Android 実装（JNI 経由で `NfcCardEmulationHelper`）、画面の状態機械
//! （`fandhe_core::screens::nfc_card_emulation_show`）へ橋渡しする effect ドライバ
//! （`driver`: [`nfc_card_emulation_show_effect`]。画面の状態型を参照してよいのは `driver` だけ）、機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）と
//! Android の Java・res（`android/`）を 1 つに集める。iOS は Apple の entitlement 承認待ちで対象外。
//!
//! 依存方向は本 crate → `fandhe-core` / `fandhe-runtime` / `fandhe-platform-android` の一方向で、
//! egui・`fandhe-device` には依存しない。呼び出し元は画面カタログ
//! （`fandhe-gallery` の `screens/registry.rs` の `effect "nfc_card_emulation_show"` 行）で、
//! `ProxyApp::update` のフレーム末尾から毎フレーム [`nfc_card_emulation_show_effect`] が呼ばれる。
//! 画面の状態機械は `fandhe-core` に残している（置き場の最終判断は #1165 系）。
//!
//! 扱う範囲は「固定の独自 AID への SELECT に固定の応答を返す HCE の状態を読み出す」ところまで。
//! 応答そのものは OS 側のサービス（Android は `HostApduService`）が返し、[`CardEmulation`] は状態の
//! 読み出しだけを持つ。実カードの模擬・決済・応答内容の変更は扱わない。
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
//! - **AID の正はこの crate の [`AID`] だけ**。Java へは JNI の引数で渡す。`android/res/xml/apduservice.xml`
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
//!
//! # pull 型のポーリングと別スレッド読み出し
//!
//! 画面へは effect ドライバが `POLL_INTERVAL` ごとに状態を読んで届ける。読み出しはブロックしうるため
//! ワーカースレッドで行い、`Poller` が高々 1 件に絞る（判定と状態更新は同じロックの内側）。期限は
//! 単調時計（`Instant`）で測る。期限前の分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。完了は先に次回の期限と空きを確定して
//! から画面へ送る。失敗の後も読み出しを続け、復帰したら状態を出す。非対応 OS は結果が変わらない
//! ため 1 回だけ送り、以後は読み出しも再描画の予約もしない。

// unsafe は持たない。JNI の attach と Activity 取得は `fandhe-platform-android::with_activity` に閉じる。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;

use std::sync::{Arc, OnceLock};

/// 画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ。カタログ（`fandhe-gallery`）が使い、
/// 入口 crate `fandhe` からは再公開しない（consumer-app-structure.md §8.1）。
pub mod driver;
// OS 実装（`apple.rs` 等）が crate 直下の名前で使うドライバ内の共有の状態・関数。
#[allow(unused_imports)]
pub(crate) use driver::*;

/// 本アプリの HCE が応答する独自 AID（16 進）。`F0` + ASCII "FANDHE" + `01`（ISO/IEC 7816-4 の
/// 独自範囲、5〜16 バイト）。
pub const AID: &str = "F046414E44484501";

/// 読み出しの失敗。OS の文言は持たない（ログ・`Debug` に出さない契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CardEmulationError {
    /// この OS・端末では HCE を使えない。ドライバは以後読み出さない。
    #[error("nfc_card_emulation is unsupported")]
    Unsupported,
    /// 一時的な失敗（JNI 失敗・Activity の不在・値域外）。次回のポーリングで回復しうる。
    #[error("nfc_card_emulation failed")]
    Internal,
}

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
    fn status(&self) -> Result<HceStatus, CardEmulationError>;
}

/// 非対応 OS 向け。常に `Err(Unsupported)`。
pub struct UnsupportedCardEmulation;

impl CardEmulation for UnsupportedCardEmulation {
    fn status(&self) -> Result<HceStatus, CardEmulationError> {
        Err(CardEmulationError::Unsupported)
    }
}

/// 現在の OS の実装（Android は `android::implementation`、他は `Unsupported` 固定）。
/// 状態型に依存しない入口（consumer-app-structure.md §8.2）。フレームワークの外のアプリも、自分の状態型の effect から使える。
pub fn platform_nfc_card_emulation() -> &'static Arc<dyn CardEmulation> {
    static EMU: OnceLock<Arc<dyn CardEmulation>> = OnceLock::new();
    EMU.get_or_init(|| {
        #[cfg(target_os = "android")]
        {
            android::implementation()
        }
        #[cfg(not(target_os = "android"))]
        {
            Arc::new(UnsupportedCardEmulation)
        }
    })
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
        assert_eq!(
            UnsupportedCardEmulation.status(),
            Err(CardEmulationError::Unsupported)
        );
    }
}
