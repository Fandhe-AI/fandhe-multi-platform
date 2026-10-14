//! Phase 11 の拡充ループ（親 #1131、Issue #1304）: 端末・OS 情報（OS 名と版・機種・メモリ量・
//! 空き容量）を 1 回読む契約。`fandhe-egui-proxy` の `device/device_info.rs` が「端末情報を読む」
//! ボタン押下時に [`DeviceInfo::read`] を 1 回だけ呼び、各 OS 実装（macOS: `NSProcessInfo` +
//! `sysctl` + `NSURL` のボリューム容量、Windows: `RtlGetVersion` + レジストリ + `GlobalMemoryStatusEx`
//! + `GetDiskFreeSpaceExW`、iOS: #1305、Android: #1306）が本トレイトを実装する。Web は
//! `fandhe-web-dom` 側（#1307）で `navigator` 系 API を使い、本トレイトは経由しない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`DeviceInfoSnapshot`]。項目ごとの「取れない」は `None`。空き容量 0 バイトは正当な値
//!   （`Some(0)`）で `None` と区別する
//! - 非対応: `Err(DeviceError::Unsupported("device_info"))`
//! - 失敗: `Err(DeviceError::Internal("device_info read"))`（OS 呼び出しの失敗、または全項目が
//!   取れなかった場合。[`require_any`]）
//!
//! キャンセル・権限ダイアログの経路は無い（権限不要の同期読み取りのみ）。
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で、ボタン押下ごとに 1 回だけ呼ばれる。定期 pull はしない。
//!
//! # 取らないもの
//!
//! 個体識別子（シリアル・`hw.uuid`・`MachineGuid`・UDID・広告 ID）は読まない・保持しない・
//! ログに出さない（プライバシー）。機種は型番識別子（`MacBookPro18,3` 等）のことがある。
//!
//! # 空き容量
//!
//! OS の見積り値で、厳密な値ではない。Web の `StorageManager.estimate()` はオリジンへの割当の
//! 見積りで端末の空き容量ではないため、Web は空き容量を `None`（不明）とする。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さない（不明・0 バイト・失敗の区別）、1 プロセス内の取得、失敗を成功値へ
//! 畳まない、個体識別子を取らない。
//! 守らない: OS 間で表記や粒度が揃うこと、空き容量の厳密さ、読み取り後の変化の反映（再読込まで）、
//! BIOS が出す仮の文字列（`To Be Filled By O.E.M.` 等）の判定、OS 呼び出しが戻らない場合の期限。
//!
//! 並行性: 読み取りは UI スレッドの effect 1 本だけで、ワーカー・コールバック・購読を持たない。
//! 集約すべき並行経路は無い。

use crate::DeviceError;

/// 文字列項目の長さ上限（文字数）。
const MAX_TEXT_CHARS: usize = 128;

/// 端末・OS 情報。`None` は「取れなかった」。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfoSnapshot {
    /// OS 名（各 OS 実装の定数）。
    pub os_name: Option<String>,
    /// OS の版。
    pub os_version: Option<String>,
    /// 機種名（型番識別子のことがある）。
    pub model: Option<String>,
    /// 物理メモリ量（バイト）。
    pub memory_bytes: Option<u64>,
    /// 空き容量（バイト、OS の見積り）。0 は有効値。
    pub free_storage_bytes: Option<u64>,
}

/// 端末・OS 情報を 1 回読む。
pub trait DeviceInfo: Send + Sync {
    /// 契約はモジュール doc 参照。
    fn read(&self) -> Result<DeviceInfoSnapshot, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedDeviceInfo;

impl DeviceInfo for UnsupportedDeviceInfo {
    fn read(&self) -> Result<DeviceInfoSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("device_info"))
    }
}

/// OS が返した文字列を表示可能にする。制御文字を除き、前後の空白を除き、128 文字で切り、
/// 空なら `None`。全 OS 実装が共用する。
pub fn clean_text(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    let s: String = s.trim().chars().take(MAX_TEXT_CHARS).collect();
    let s = s.trim_end();
    (!s.is_empty()).then(|| s.to_string())
}

/// 版・機種・メモリ・空き容量がすべて `None` なら `Internal`（嘘の成功の禁止）。`os_name` は
/// 実装の定数なので判定に含めない。
pub fn require_any(s: DeviceInfoSnapshot) -> Result<DeviceInfoSnapshot, DeviceError> {
    if s.os_version.is_none()
        && s.model.is_none()
        && s.memory_bytes.is_none()
        && s.free_storage_bytes.is_none()
    {
        return Err(DeviceError::Internal("device_info read"));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedDeviceInfo.read(),
            Err(DeviceError::Unsupported("device_info"))
        ));
    }

    #[test]
    fn clean_text_strips_control_and_blank() {
        assert_eq!(clean_text("  Mac\u{0}\n14 "), Some("Mac14".into()));
        assert_eq!(clean_text(""), None);
        assert_eq!(clean_text(" \t\n"), None);
        assert_eq!(clean_text("端末 ✓"), Some("端末 ✓".into()));
    }

    #[test]
    fn clean_text_caps_length() {
        let long = "あ".repeat(500);
        assert_eq!(clean_text(&long).unwrap().chars().count(), 128);
    }

    #[test]
    fn require_any_rejects_all_none_only() {
        let named = DeviceInfoSnapshot {
            os_name: Some("X".into()),
            ..Default::default()
        };
        assert!(matches!(
            require_any(named.clone()),
            Err(DeviceError::Internal("device_info read"))
        ));
        let zero = DeviceInfoSnapshot {
            free_storage_bytes: Some(0),
            ..named
        };
        assert!(require_any(zero).is_ok());
    }
}
