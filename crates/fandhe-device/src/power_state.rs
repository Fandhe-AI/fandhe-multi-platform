//! Phase 11 の拡充ループ（親 #1128、Issue #1274）: 電源状態（電池残量・充電状態・省電力モード・
//! 熱状態）を 1 回読む契約。`fandhe-egui-proxy` の `device/power_state.rs` が 1 秒間隔の pull で
//! [`PowerState::read`] を呼び、各 OS 実装（macOS: IOKit + `NSProcessInfo`、Windows:
//! `GetSystemPowerStatus`、iOS: #1275、Android: #1276）が本トレイトを実装する。Web は
//! `fandhe-web-dom` 側（#1277）で Battery Status API を使い、本トレイトは経由しない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`PowerSnapshot`]。項目ごとの「取れない」は `Ok` の中の `Unknown` / `None` で表す。
//!   電池が無い機種は [`Battery::Absent`] で表し、0% や充電中で代用しない
//! - 非対応: `Err(DeviceError::Unsupported("power_state"))`（API 自体が無い OS）
//! - 失敗: `Err(DeviceError::Internal("power_state read"))`（OS 呼び出しが失敗した）
//!
//! キャンセル・権限ダイアログの経路は無い（権限不要の同期 pull のみ）ため、三値のうち
//! キャンセルは存在しない。
//!
//! # 呼び出し契約
//!
//! UI スレッドから同期で呼ばれる。ブロッキング API は使わず、1 回数 ms 以内で戻ること。
//!
//! # 熱状態の写像
//!
//! - Apple `NSProcessInfoThermalState`: Nominal→通常、Fair→やや高い、Serious→高い、Critical→危険
//! - Android `THERMAL_STATUS_*`: NONE→通常、LIGHT / MODERATE→やや高い、SEVERE→高い、
//!   CRITICAL / EMERGENCY / SHUTDOWN→危険。API 28 以下は [`Thermal::Unknown`]
//! - Windows・Web: [`Thermal::Unknown`]。範囲外の値も [`Thermal::Unknown`]
//!
//! # 「満充電」の条件
//!
//! OS が満充電を明示したときだけ [`Charging::Full`]（macOS `kIOPSIsChargedKey`）。Windows は
//! 「充電中ビットかつ 100%」のときだけ。AC 接続で充電ビットが無い場合などは [`Charging::Unknown`]
//! にし、放電中とは偽らない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さない（電池なし・不明・0% の区別）、1 プロセス内の取得、失敗を成功値へ畳まない。
//! 守らない: OS 間の値の一致、変化の反映遅れ（読み取り間隔 1 秒ぶん）、複数プロセス、
//! スリープ復帰直後の古い値（OS 側の更新待ち）。
//!
//! 並行性: 読み取りは UI スレッドの effect 1 本だけで、ワーカー・コールバック・変化通知の
//! 購読を持たない。集約すべき並行経路は無い。

use crate::DeviceError;

/// 充電状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charging {
    /// 充電中。
    Charging,
    /// 放電中（電池で駆動）。
    Discharging,
    /// 満充電（OS が明示したときのみ）。
    Full,
    /// 取得できない。
    Unknown,
}

/// 電池の有無と状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    /// 電池を搭載していない（デスクトップ機など）。0% ではない。
    Absent,
    /// 電池あり。`level` は 0..=100 の百分率で、取れなければ `None`。
    Present {
        /// 残量（%）。
        level: Option<u8>,
        /// 充電状態。
        charging: Charging,
    },
    /// 電池の有無自体が取得できない。
    Unknown,
}

/// 熱状態（写像は module doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thermal {
    /// 通常。
    Nominal,
    /// やや高い。
    Fair,
    /// 高い。
    Serious,
    /// 危険。
    Critical,
    /// 取得できない。
    Unknown,
}

/// ある時点の電源状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSnapshot {
    /// 電池。
    pub battery: Battery,
    /// 省電力モード。取得できなければ `None`。
    pub low_power_mode: Option<bool>,
    /// 熱状態。
    pub thermal: Thermal,
}

/// 電源状態の読み取り契約（module doc 参照）。
pub trait PowerState: Send + Sync {
    /// 現在の電源状態を同期で読む。
    fn read(&self) -> Result<PowerSnapshot, DeviceError>;
}

/// 常に非対応を返す実装。API が無い OS が成功値（ダミー）へ倒れないための既定。
pub struct UnsupportedPowerState;

impl PowerState for UnsupportedPowerState {
    fn read(&self) -> Result<PowerSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("power_state"))
    }
}

/// OS から来た残量の生値を百分率へ。0..=100 だけが値で、255・負値などの番兵や範囲外は `None`
/// （「不明」を 0% へ畳まない、契約表 8）。
pub fn percent_from_raw(raw: i64) -> Option<u8> {
    u8::try_from(raw).ok().filter(|v| *v <= 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedPowerState.read(),
            Err(DeviceError::Unsupported("power_state"))
        ));
    }

    #[test]
    fn percent_from_raw_boundaries() {
        assert_eq!(percent_from_raw(-1), None);
        assert_eq!(percent_from_raw(0), Some(0));
        assert_eq!(percent_from_raw(100), Some(100));
        assert_eq!(percent_from_raw(101), None);
        assert_eq!(percent_from_raw(255), None);
    }
}
