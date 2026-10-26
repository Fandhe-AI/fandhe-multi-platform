//! Phase 11 の拡充ループ（親 #181、Issue #903）: アプリ・端末の識別子（iOS の IDFV、Android の
//! `ANDROID_ID`）を 1 回読み、伏せて表示するための契約。`fandhe-egui-proxy` の
//! `device/device_identifier.rs` が「識別子を読む」押下時に [`DeviceIdentifier::read`] を 1 回だけ呼び、
//! OS 実装（iOS・Android: #904）が本トレイトを実装する。macOS・Windows・Linux・Web は
//! 対応表で `no` のため [`UnsupportedDeviceIdentifier`] 固定。
//!
//! # 範囲
//!
//! OS の値を 1 回読み、伏せて表示するだけ。保存・送信・比較・再設定・広告 ID・IMEI・シリアル・MAC は範囲外。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - [`IdentifierRead::Identifier`]: OS の値
//! - [`IdentifierRead::NotYetAvailable`]: iOS の `identifierForVendor` が nil（ロック直後など）。失敗にしない
//! - `Err(DeviceError::Internal(..))`: Android の null・空・例外、JNI 失敗、iOS の空文字
//! - `Err(DeviceError::Unsupported("device_identifier"))`: 非対応 OS と実装前のスタブ
//!
//! キャンセルと権限ダイアログの経路は無い。
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で、押下ごとに 1 回だけ呼ぶ。iOS の `UIDevice` は UIKit なので main thread で読み、
//! `@autoreleasepool` で囲む。main 以外では `dispatch_sync` せず失敗を返す。Android は同期 JNI で 1 回だけ読む。
//! OS を正にする: 事前判定・自動再試行・自前 UUID の生成と保存・前回値との比較をしない。
//!
//! # 値の扱い
//!
//! ログ・画面全体・記録に値を出さない。[`DeviceId`] は全体を取り出す API を持たず、画面へ渡す時点で
//! 末尾 4 文字と文字数に落とす。trim・形式検査はしない（契約表 8）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: OS の値をそのまま 1 回読むこと、nil・null・空・失敗を成功に畳まないこと、値を残さないこと、
//! 世代照合で古い結果を捨てること、権限を足さないこと。
//! 守らない: 値がいつ変わるか、Android 7 でのアプリ間共有、iOS の nil がいつ解けるか、
//! 読んだ後の変化の反映、OS 呼び出しが戻らない場合の期限、複数プロセス・複数ユーザー間の一致。
//!
//! 並行性: UI スレッドの effect 1 本だけで、ワーカー・コールバック・購読を持たない。保留中の要求を
//! 持たないため、Activity 再生成で破棄するものも無い。
//!
//! # 後続 OS 実装の要点（#904）
//!
//! 権限・usage description・`<queries>`・privacy manifest は足さない。`ANDROID_ID` は minSdk 24 の全域で使える。

use crate::DeviceError;

/// 読み取った識別子。全体を取り出す経路を持たない（漏れる経路を作らない）。
pub struct DeviceId(String);

impl DeviceId {
    /// 空文字だけを拒む。trim・大文字化・形式検査はしない。
    pub fn new(value: String) -> Result<Self, DeviceError> {
        if value.is_empty() {
            return Err(DeviceError::Internal("device_identifier empty"));
        }
        Ok(Self(value))
    }

    /// 文字数。
    pub fn char_count(&self) -> usize {
        self.0.chars().count()
    }

    /// 末尾 4 文字。4 文字以下だと全体が見えてしまうため空文字を返す。
    pub fn masked_tail(&self) -> String {
        let n = self.char_count();
        if n <= 4 {
            return String::new();
        }
        self.0.chars().skip(n - 4).collect()
    }
}

impl std::fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceId(<{} chars>)", self.char_count())
    }
}

/// 読み取りの成功結果。
#[derive(Debug)]
pub enum IdentifierRead {
    /// OS の値。
    Identifier(DeviceId),
    /// 今は取れない（iOS の nil）。失敗ではない。
    NotYetAvailable,
}

/// 識別子の読み取り（契約はモジュール doc）。
pub trait DeviceIdentifier: Send + Sync {
    /// OS の値を 1 回読む。
    fn read(&self) -> Result<IdentifierRead, DeviceError>;
}

/// 非対応 OS・実装前のスタブ。常に `Unsupported`。
pub struct UnsupportedDeviceIdentifier;

impl DeviceIdentifier for UnsupportedDeviceIdentifier {
    fn read(&self) -> Result<IdentifierRead, DeviceError> {
        Err(DeviceError::Unsupported("device_identifier"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "00000000-0000-0000-0000-00000000ABCD";

    #[test]
    fn empty_is_internal() {
        assert!(matches!(
            DeviceId::new(String::new()),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn debug_shows_only_char_count() {
        let s = format!("{:?}", DeviceId::new(FAKE.into()).unwrap());
        assert!(!s.contains("ABCD") && !s.contains("0000-"), "{s}");
        assert!(s.contains("36"));
    }

    #[test]
    fn masked_tail_is_last_four() {
        assert_eq!(DeviceId::new(FAKE.into()).unwrap().masked_tail(), "ABCD");
    }

    #[test]
    fn short_value_hides_all() {
        assert_eq!(DeviceId::new("abcd".into()).unwrap().masked_tail(), "");
    }

    #[test]
    fn no_trim() {
        assert_eq!(DeviceId::new("  x ".into()).unwrap().char_count(), 4);
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedDeviceIdentifier.read(),
            Err(DeviceError::Unsupported("device_identifier"))
        ));
    }
}
