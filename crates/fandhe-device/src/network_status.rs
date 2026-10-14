//! ネットワーク接続状態の契約（Issue #1282、親 #1130）。接続の有無・種別・従量制を
//! 画面に出すための橋渡しで、`fandhe-egui-proxy` の `device/network_status.rs` が
//! 間隔つきで [`NetworkStatus::read`] を呼ぶ（pull 型。OS の変化通知は購読しない）。
//!
//! # 契約
//!
//! - 結果は [`NetworkSnapshot`]（接続あり / 接続なし / 不明）か `DeviceError`
//!   （`Unsupported` / `Internal` のみ。新しい失敗 enum は作らない）
//! - 取得できないとき「接続あり」と偽らない。最初の値が届く前・OS が判定不能は
//!   [`NetworkSnapshot::Unknown`]（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 「接続あり」は OS が通信経路を持つという意味で、インターネット到達性は保証しない
//!   （captive portal・VPN。Web の `navigator.onLine` も同じ）
//! - 種別・従量制が取れない環境は [`ConnectionKind::Unknown`] / `metered: None`。
//!   接続なし・不明のとき種別と従量制は持たない（型で不正な組合せを作れない）
//! - **三値**: 本機能には利用者操作のキャンセル・権限ダイアログ・入力が無い。結果は
//!   「読めた（Online / Offline / Unknown）」「非対応」「失敗」の 3 種で、失敗は
//!   `DeviceError::Internal`、非対応は `DeviceError::Unsupported`
//! - 値（インターフェース情報・HRESULT 等）をログへ出さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の「接続あり」を出さないこと、1 プロセス内での取得、資源（macOS の
//! `NWPathMonitor`）をプロセス寿命で 1 つに限ること（OS がプロセス終了で片付ける）。
//! 守らない: インターネット到達性、変化からの反映遅れ（最大 [`READ_INTERVAL`] 程度）、
//! 複数プロセス間の表示の食い違い。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 接続種別。判定できない環境は [`ConnectionKind::Unknown`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    /// Wi-Fi。
    Wifi,
    /// 携帯回線。
    Cellular,
    /// 有線。
    Ethernet,
    /// 上記以外（VPN・ループバック等）。
    Other,
    /// 判定できない。
    Unknown,
}

/// ある時点の接続状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkSnapshot {
    /// 通信経路がある。`metered` は従量制接続か（`None` は不明）。
    Online {
        /// 接続種別。
        kind: ConnectionKind,
        /// 従量制接続か。
        metered: Option<bool>,
    },
    /// 通信経路が無い。
    Offline,
    /// 判定不能・最初の値が届く前。
    Unknown,
}

/// 接続状態を 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait NetworkStatus: Send + Sync {
    /// 現在の接続状態を返す。最後に届いた値を、切断後も返し続けてはならない。
    fn read(&self) -> Result<NetworkSnapshot, DeviceError>;
}

/// 常に非対応を返す実装（対応 OS 以外・未実装 OS の dispatch 先）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedNetworkStatus;

impl NetworkStatus for UnsupportedNetworkStatus {
    fn read(&self) -> Result<NetworkSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("network_status"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_reports_online() {
        let r = UnsupportedNetworkStatus.read();
        assert!(matches!(r, Err(DeviceError::Unsupported("network_status"))));
    }
}
