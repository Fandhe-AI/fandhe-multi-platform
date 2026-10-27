//! Windows 向け「無線の状態と切り替え」実装（Issue #973、親 #189）。
//! `device/radio_control.rs` の dispatch（Windows 経路）からワーカースレッドで呼ばれ、
//! WinRT `Windows.Devices.Radios.Radio` で Wi-Fi / Bluetooth を読み書きする。契約の SSOT は
//! `fandhe_device::radio_control`（「OS ごとの写像」節）。
//!
//! # 流れ
//!
//! - `read`: `GetRadiosAsync().get()` の一覧を Kind で絞り、先頭の 1 つの `State()` を返す。
//!   一覧に無ければ `NotPresent`（失敗ではない）
//! - `begin_access`: `RequestAccessAsync` は UI スレッドから呼ぶ必要があるため、ドライバ（UI スレッド）が
//!   `set` の前に呼ぶ。`.get()` では待たず `SetCompleted` で結果をチャネルへ流し、ワーカーが受け取る
//! - `set`: `GetRadiosAsync().get()` → `SetStateAsync(..).get()`（許可要求は済んでいる前提）。
//!   `Allowed` は `Accepted`（受付であり、最終状態は非同期に決まる。読み直しはドライバ側）
//!
//! `unsafe` と `RoInitialize` は使わない。HRESULT と `Radio.Name`（アダプター名）はログにも画面にも
//! `Debug` にも出さず、契約の固定識別子だけを返す（security.md A09）。購読・ハンドラは登録しない。
//!
//! # 守らない範囲
//!
//! - 同じ Kind の無線が複数あるときは先頭だけを扱う
//! - `Allowed` 後の状態変化（ハードウェアスイッチ・ポリシー・他アプリ）と、同意 UI の待ち時間
//!   （`.get()` に期限は設けない）
//!
//! # 既知の天井（実機未確認、#965）
//!
//! - パッケージ無しの素の exe で `RequestAccessAsync` / `SetStateAsync` が `Allowed` を返すか
//! - 拒否時の HRESULT の値
//! - ローカルの型検査は `x86_64-pc-windows-gnu` のみ（MSVC は CI）

#[cfg(any(target_os = "windows", test))]
use fandhe_device::radio_control::{
    RadioKind, RadioState, SwitchOutcome, DENIED_BY_SYSTEM, FAILED, FEATURE,
};
#[cfg(any(target_os = "windows", test))]
use fandhe_device::DeviceError;

/// `E_ACCESSDENIED`。
#[cfg(any(target_os = "windows", test))]
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
/// `REGDB_E_CLASSNOTREG`（この環境に無線 API が無い）。
#[cfg(any(target_os = "windows", test))]
const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154_u32 as i32;

/// `RadioKind`（WinRT 列挙）の生の値。
#[cfg(any(target_os = "windows", test))]
fn kind_raw(kind: RadioKind) -> i32 {
    match kind {
        RadioKind::Wifi => 1,
        RadioKind::Bluetooth => 3,
    }
}

/// 先頭の無線の `RadioState`（生の値）を契約の状態へ写す。`None` は一覧に無い = `NotPresent`。
/// 0（Unknown）と列挙外は `Unknown` のまま返し、`Off` には畳まない。
#[cfg(any(target_os = "windows", test))]
fn state_from_raw(found: Option<i32>) -> RadioState {
    match found {
        None => RadioState::NotPresent,
        Some(1) => RadioState::On,
        Some(2) => RadioState::Off,
        Some(3) => RadioState::Disabled,
        Some(_) => RadioState::Unknown,
    }
}

/// `RadioAccessStatus`（生の値）を結果へ写す。`Allowed` 以外と列挙外は成功にしない。
#[cfg(any(target_os = "windows", test))]
fn access_result(raw: i32) -> Result<SwitchOutcome, DeviceError> {
    match raw {
        1 => Ok(SwitchOutcome::Accepted),
        2 => Err(DeviceError::PermissionDenied(FEATURE)),
        0 | 3 => Err(DeviceError::Internal(DENIED_BY_SYSTEM)),
        _ => Err(DeviceError::Internal(FAILED)),
    }
}

/// WinRT 呼び出し失敗の HRESULT を契約のエラーへ写す（全呼び出し箇所でここを通す）。
#[cfg(any(target_os = "windows", test))]
fn error_from_hresult(code: i32) -> DeviceError {
    match code {
        E_ACCESSDENIED => DeviceError::PermissionDenied(FEATURE),
        REGDB_E_CLASSNOTREG => DeviceError::Unsupported(FEATURE),
        _ => DeviceError::Internal(FAILED),
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::cell::RefCell;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;

    use windows::Devices::Radios as win;
    use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};

    use super::{access_result, error_from_hresult, kind_raw, state_from_raw};
    use fandhe_device::radio_control::{RadioControl, RadioKind, RadioState, SwitchOutcome};
    use fandhe_device::DeviceError;

    struct WindowsRadioControl;

    thread_local! {
        /// 完了ハンドラが呼ばれる前に落とさないよう保持する（UI スレッド専用、次回で置き換わる）。
        static HELD: RefCell<Option<IAsyncOperation<win::RadioAccessStatus>>> =
            const { RefCell::new(None) };
    }

    /// 無線のアクセス許可要求を開始する。UI スレッドから呼ぶこと（`RequestAccessAsync` の要件）。
    /// 待たずに返り、結果（`Allowed` のみ `Ok`）は戻り値のチャネルへ 1 回だけ流れる。
    pub(crate) fn begin_access() -> Receiver<Result<(), DeviceError>> {
        let (tx, rx) = channel();
        let started = win::Radio::RequestAccessAsync()
            .map_err(err)
            .and_then(|op| {
                let tx = tx.clone();
                op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
                    let result = match (op.ok(), status) {
                        (Ok(op), AsyncStatus::Completed) => op
                            .GetResults()
                            .map_err(err)
                            .and_then(|s: win::RadioAccessStatus| access_result(s.0))
                            .map(|_| ()),
                        _ => Err(DeviceError::Internal(fandhe_device::radio_control::FAILED)),
                    };
                    let _ = tx.send(result);
                    Ok(())
                }))
                .map_err(err)?;
                HELD.with_borrow_mut(|h| *h = Some(op));
                Ok(())
            });
        if let Err(e) = started {
            let _ = tx.send(Err(e));
        }
        rx
    }

    pub(crate) fn implementation() -> Arc<dyn RadioControl> {
        Arc::new(WindowsRadioControl)
    }

    fn err(e: windows::core::Error) -> DeviceError {
        error_from_hresult(e.code().0)
    }

    /// Kind が一致する先頭の無線。`Kind()` の失敗は飛ばさずエラーにする（`NotPresent` への化け防止）。
    fn find(kind: RadioKind) -> Result<Option<win::Radio>, DeviceError> {
        let list = win::Radio::GetRadiosAsync()
            .map_err(err)?
            .get()
            .map_err(err)?;
        for r in list {
            if r.Kind().map_err(err)?.0 == kind_raw(kind) {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    impl RadioControl for WindowsRadioControl {
        fn read(&self, kind: RadioKind) -> Result<RadioState, DeviceError> {
            match find(kind)? {
                None => Ok(state_from_raw(None)),
                Some(r) => Ok(state_from_raw(Some(r.State().map_err(err)?.0))),
            }
        }

        fn set(&self, kind: RadioKind, on: bool) -> Result<SwitchOutcome, DeviceError> {
            let Some(r) = find(kind)? else {
                return Err(DeviceError::Unsupported(
                    fandhe_device::radio_control::FEATURE,
                ));
            };
            let target = if on {
                win::RadioState::On
            } else {
                win::RadioState::Off
            };
            let status = r.SetStateAsync(target).map_err(err)?.get().map_err(err)?;
            access_result(status.0)
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::{begin_access, implementation};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_mapping_never_folds_to_off() {
        assert_eq!(state_from_raw(None), RadioState::NotPresent);
        assert_eq!(state_from_raw(Some(1)), RadioState::On);
        assert_eq!(state_from_raw(Some(2)), RadioState::Off);
        assert_eq!(state_from_raw(Some(3)), RadioState::Disabled);
        assert_eq!(state_from_raw(Some(0)), RadioState::Unknown);
        assert_eq!(state_from_raw(Some(99)), RadioState::Unknown);
        for raw in [0, 1, 3, 99, -1] {
            assert_ne!(state_from_raw(Some(raw)), RadioState::Off);
        }
    }

    #[test]
    fn only_allowed_is_accepted() {
        assert!(matches!(access_result(1), Ok(SwitchOutcome::Accepted)));
        assert!(matches!(
            access_result(2),
            Err(DeviceError::PermissionDenied(FEATURE))
        ));
        assert!(matches!(
            access_result(0),
            Err(DeviceError::Internal(DENIED_BY_SYSTEM))
        ));
        assert!(matches!(
            access_result(3),
            Err(DeviceError::Internal(DENIED_BY_SYSTEM))
        ));
        assert!(matches!(
            access_result(99),
            Err(DeviceError::Internal(FAILED))
        ));
    }

    #[test]
    fn hresult_mapping() {
        assert!(matches!(
            error_from_hresult(E_ACCESSDENIED),
            DeviceError::PermissionDenied(FEATURE)
        ));
        assert!(matches!(
            error_from_hresult(REGDB_E_CLASSNOTREG),
            DeviceError::Unsupported(FEATURE)
        ));
        assert!(matches!(
            error_from_hresult(-1),
            DeviceError::Internal(FAILED)
        ));
        assert!(matches!(
            error_from_hresult(0),
            DeviceError::Internal(FAILED)
        ));
    }

    #[test]
    fn kind_values() {
        assert_eq!(kind_raw(RadioKind::Wifi), 1);
        assert_eq!(kind_raw(RadioKind::Bluetooth), 3);
    }
}
