//! Phase 11 の拡充ループ（親 #112、Issue #505）: 接続中の USB 機器の一覧（ベンダー ID・製品 ID・
//! 製品名）を 1 回読む契約。`fandhe-egui-proxy` の `device/usb_device.rs` が起動時と「再取得」
//! ボタン押下時に [`UsbDevices::list`] を 1 回だけ呼び、各 OS 実装（macOS: IOKit、Windows: SetupAPI（#506）、
//! Android: #507）が本トレイトを実装する。Web は `fandhe-web-dom` 側（#508）で WebUSB / WebHID を
//! 使い、本トレイトは経由しない。
//!
//! 範囲は「一覧を出すだけ」。機器は開かず、読み書き・転送は扱わない。抜き差しには追従しない
//! （1 回読み + 再取得）。対象は USB バスの機器だけで、Bluetooth HID は含めない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: `Vec<UsbDeviceEntry>`（0 件を含む。0 件は正常）
//! - 非対応: `Err(DeviceError::Unsupported("usb_device"))`（iOS、その他の OS、USB ホスト非搭載の
//!   Android 機）
//! - 失敗: `Err(DeviceError::Internal("usb_device list"))`（OS 呼び出しの失敗、橋渡しの値の不正）
//!
//! `PermissionDenied` は返さない。機器を開かないため権限の経路が無い。`Cancelled` は Web の画面側
//! だけで使う（選択ダイアログを閉じた場合）。失敗として見せない。
//!
//! # 取らないもの
//!
//! シリアル番号・インスタンス ID・locationID・`/dev/bus/usb` のパス・Web の `serialNumber` は
//! 読まない・保持しない（フィンガープリント防止）。製品名は画面にだけ出し、ログ出力・送信をしない。
//! 名前の加工は [`crate::device_info::clean_text`]（制御文字除去・128 文字）のみ。
//!
//! # 件数上限
//!
//! [`MAX_DEVICES`] 件。超過分は捨てる。同じ VID/PID は畳まない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 0 件と失敗の区別、失敗を成功値へ畳まないこと、識別子を取らないこと、1 プロセス内の
//! 世代照合。
//! 守らない範囲: 抜き差しの反映（再取得まで）、OS 間の名前の表記と粒度の一致、ハブや内蔵 USB 機器を
//! 含めるか（OS 任せ）、Bluetooth HID、Web は選ばれた 1 台だけ、複数タブ・複数プロセス間の一致、
//! 同期呼び出し（IOKit / SetupAPI）が戻らない場合の期限（既知の天井。期限を持たない）。
//!
//! 並行性: 読み取りは 1 要求 1 回の同期読みで、購読・コールバックを持たない。集約すべき並行経路は無い。

use crate::DeviceError;
use crate::device_info::clean_text;

/// 最大件数。
pub const MAX_DEVICES: usize = 64;

/// USB 機器 1 件。`name` が `None` は名前を取れなかった項目（件数には数える）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDeviceEntry {
    /// ベンダー ID。
    pub vendor_id: u16,
    /// 製品 ID。
    pub product_id: u16,
    /// 製品名（[`clean_text`] 済み）。
    pub name: Option<String>,
}

/// 接続中の USB 機器を 1 回読む。
pub trait UsbDevices: Send + Sync {
    /// 契約はモジュール doc 参照。
    fn list(&self) -> Result<Vec<UsbDeviceEntry>, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedUsbDevices;

impl UsbDevices for UnsupportedUsbDevices {
    fn list(&self) -> Result<Vec<UsbDeviceEntry>, DeviceError> {
        Err(DeviceError::Unsupported("usb_device"))
    }
}

/// OS が返した項目の名前を [`clean_text`] に通し、先頭 [`MAX_DEVICES`] 件だけ残す。同じ VID/PID は
/// 畳まず、名前の無い項目も残す。全 OS 実装が共用する。
pub fn normalize(entries: impl IntoIterator<Item = UsbDeviceEntry>) -> Vec<UsbDeviceEntry> {
    entries
        .into_iter()
        .take(MAX_DEVICES)
        .map(|e| UsbDeviceEntry {
            name: e.name.as_deref().and_then(clean_text),
            ..e
        })
        .collect()
}

/// SetupAPI の hardware ID（`USB\\VID_05AC&PID_024F&REV_0100` の形）から VID/PID を取り出す。
/// 大文字小文字を区別せず、`VID_` / `PID_` の直後 4 桁の 16 進を読む。どちらかが無い・桁不足・
/// 16 進でないときは `None`。Windows 実装が呼ぶ純関数で、OS 非依存のためここに置く（ホストで試験する）。
pub fn parse_hardware_id(id: &str) -> Option<(u16, u16)> {
    fn field(id: &str, key: &str) -> Option<u16> {
        let upper = id.to_ascii_uppercase();
        let rest = &upper[upper.find(key)? + key.len()..];
        let hex = rest.get(..4)?;
        hex.bytes()
            .all(|b| b.is_ascii_hexdigit())
            .then(|| u16::from_str_radix(hex, 16).ok())
            .flatten()
    }
    Some((field(id, "VID_")?, field(id, "PID_")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hardware_id_cases() {
        assert_eq!(
            parse_hardware_id("USB\\VID_05AC&PID_024F&REV_0100"),
            Some((0x05AC, 0x024F))
        );
        assert_eq!(
            parse_hardware_id("usb\\vid_05ac&pid_024f"),
            Some((0x05AC, 0x024F))
        );
        assert_eq!(parse_hardware_id("USB\\VID_05AC&REV_0100"), None);
        assert_eq!(parse_hardware_id("USB\\VID_05A&PID_024F"), None);
        assert_eq!(parse_hardware_id("USB\\VID_05AG&PID_024F"), None);
        assert_eq!(parse_hardware_id("USB\\VID_é"), None);
        assert_eq!(parse_hardware_id(""), None);
    }

    fn e(v: u16, name: Option<&str>) -> UsbDeviceEntry {
        UsbDeviceEntry {
            vendor_id: v,
            product_id: 1,
            name: name.map(Into::into),
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedUsbDevices.list(),
            Err(DeviceError::Unsupported("usb_device"))
        ));
    }

    #[test]
    fn normalize_cleans_names_and_keeps_nameless_and_duplicates() {
        let v = normalize([
            e(1, Some(" K\u{0}\nB ")),
            e(1, Some("")),
            e(1, None),
            e(1, Some("KB")),
        ]);
        let names: Vec<_> = v.iter().map(|d| d.name.as_deref()).collect();
        assert_eq!(names, [Some("KB"), None, None, Some("KB")]);
    }

    #[test]
    fn normalize_caps_at_max() {
        let v = normalize((0..65).map(|i| e(i, None)));
        assert_eq!(v.len(), MAX_DEVICES);
        assert_eq!(v[63].vendor_id, 63);
    }
}
