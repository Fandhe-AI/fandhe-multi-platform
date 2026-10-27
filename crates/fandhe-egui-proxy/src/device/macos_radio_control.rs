//! macOS 向け「無線の状態と切り替え」実装（Issue #972、親 #189）。
//! `macos/fandhe_macos_radio_control.m`（CoreWLAN の `CWInterface.powerOn` / `setPower:error:` と
//! IOBluetooth の `IOBluetoothHostController.powerState`。`build.rs` が macOS 分岐で feature `device`
//! 有効時にコンパイルする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`device/radio_control.rs` の macOS 経路からワーカースレッドで呼ばれ、
//! 契約は `fandhe_device::radio_control`。
//!
//! Bluetooth の切り替えに公開 API はない（`IOBluetoothPreferenceSetControllerPowerState` は非公開のため
//! 使わない）。`set(Bluetooth, _)` は FFI を呼ばず `Unsupported` を返す。
//!
//! 戻りコードの写像 [`decode_read`] / [`decode_set`] は純関数で、ホストの `cargo test` で検査する。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 未知の戻りコードを成功・`Off` に畳まない、`NSError` をログに出さない、main thread で
//!   `setPower` を呼ばない
//! - 守らない: OS 呼び出しが戻らない場合、管理者パスワードのダイアログの待ち時間、要求後の状態の変化
//!
//! # 未確認事項（Mac 目視 #965 で確認）
//!
//! - `powerState` の読み取りに Bluetooth の TCC が掛かるか
//! - 管理者パスワードを取り消したときに返る `NSError`（識別できれば三値に分ける。いまは失敗）
//! - `powerOn` は失敗時にも NO を返すため、Off と区別できない（既知の天井）
//!
//! 実物の CoreWLAN / IOBluetooth はホストのテストから呼ばない（無線の状態を変えてしまうため）。

#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use fandhe_device::radio_control::{RadioControl, RadioKind};
use fandhe_device::radio_control::{RadioState, SwitchOutcome, FAILED, FEATURE};
use fandhe_device::DeviceError;

/// 状態読み取りの戻りコードを契約の結果へ写す。未知の値は `Internal`（`Off` へ畳まない）。
fn decode_read(rc: i32) -> Result<RadioState, DeviceError> {
    match rc {
        0 => Ok(RadioState::On),
        1 => Ok(RadioState::Off),
        2 => Ok(RadioState::NotPresent),
        3 => Ok(RadioState::Unknown),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

/// Wi-Fi 切り替えの戻りコードを契約の結果へ写す。未知の値は `Internal`（成功へ畳まない）。
fn decode_set(rc: i32) -> Result<SwitchOutcome, DeviceError> {
    match rc {
        0 => Ok(SwitchOutcome::Applied),
        1 => Err(DeviceError::Unsupported(FEATURE)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_radio_control.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_radio_read(kind: i32) -> i32;
        fn fandhe_macos_radio_wifi_set(on: i32) -> i32;
    }

    /// 状態を読んで戻りコードを返す。`kind` は 0 = Wi-Fi / 1 = Bluetooth。
    #[allow(unsafe_code)]
    pub(super) fn read(kind: i32) -> i32 {
        // SAFETY: 値渡しの整数だけを渡し、ポインタを渡さない。`.m` は何も保持しない。
        unsafe { fandhe_macos_radio_read(kind) }
    }

    /// Wi-Fi の電源を切り替えて戻りコードを返す。
    #[allow(unsafe_code)]
    pub(super) fn wifi_set(on: bool) -> i32 {
        // SAFETY: 値渡しの整数だけを渡し、ポインタを渡さない。`.m` は何も保持しない。
        unsafe { fandhe_macos_radio_wifi_set(i32::from(on)) }
    }
}

#[cfg(target_os = "macos")]
struct MacRadioControl;

#[cfg(target_os = "macos")]
impl RadioControl for MacRadioControl {
    fn read(&self, kind: RadioKind) -> Result<RadioState, DeviceError> {
        let kind = match kind {
            RadioKind::Wifi => 0,
            RadioKind::Bluetooth => 1,
        };
        decode_read(ffi::read(kind))
    }

    fn set(&self, kind: RadioKind, on: bool) -> Result<SwitchOutcome, DeviceError> {
        match kind {
            RadioKind::Wifi => decode_set(ffi::wifi_set(on)),
            // 公開 API がないため OS を呼ばない。
            RadioKind::Bluetooth => Err(DeviceError::Unsupported(FEATURE)),
        }
    }
}

/// macOS の `RadioControl` 実装を返す（`device/radio_control.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn RadioControl> {
    Arc::new(MacRadioControl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_codes_map_and_unknown_is_internal() {
        assert!(matches!(decode_read(0), Ok(RadioState::On)));
        assert!(matches!(decode_read(1), Ok(RadioState::Off)));
        assert!(matches!(decode_read(2), Ok(RadioState::NotPresent)));
        assert!(matches!(decode_read(3), Ok(RadioState::Unknown)));
        for rc in [-1, 4, 5, 6, 99, i32::MIN] {
            assert!(matches!(
                decode_read(rc),
                Err(DeviceError::Internal(FAILED))
            ));
        }
    }

    #[test]
    fn set_codes_map_and_unknown_is_internal() {
        assert!(matches!(decode_set(0), Ok(SwitchOutcome::Applied)));
        assert!(matches!(decode_set(1), Err(DeviceError::Unsupported(_))));
        // 5 は管理者パスワードの取り消しを含む失敗。
        for rc in [-1, 2, 4, 5, 6, 99, i32::MIN] {
            assert!(matches!(decode_set(rc), Err(DeviceError::Internal(FAILED))));
        }
    }
}
