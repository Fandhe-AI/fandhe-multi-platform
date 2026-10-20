//! Windows 向けライト（トーチ）実装（Issue #459、親 #106）。`device/flashlight.rs` の dispatch
//! （Windows 経路）から呼ばれ、WinRT `Windows.Devices.Lights.Lamp` で点灯 / 消灯する
//! （契約は `fandhe_device::flashlight`）。最小化での消灯は `device/flashlight.rs` 側が
//! 通常の `set_on(false)` 経路に載せる。
//!
//! # 流れ
//!
//! - `set_on(true)`（ワーカースレッド）: 未保持なら `Lamp::GetDefaultAsync().get()`。null
//!   （`e.code().0 == 0`）は `Unsupported`、他は `Internal`。`SetIsEnabled(true)` に失敗したら
//!   `Close` して保持しない。成功したら `Lamp` を保持する。保持済みなら `SetIsEnabled(true)` だけ
//! - `set_on(false)`: 未保持は `Ok`。`SetIsEnabled(false)` が成功したら `Close` して手放す。
//!   失敗したら保持したまま `Internal`（手放すと点いたまま「消灯」と見せる嘘になり、再試行もできない）
//! - `is_on`（UI スレッド。ブロックしない）: `Unsupported` 確定済みならそれを返す。保持中は
//!   `IsEnabled` の実値（失敗は `Internal`、前回値は返さない）。未保持は `Some(false)`
//!
//! ロックは `.get()` をまたいで持たない（UI スレッドの `is_on` を止めないため）。HRESULT・
//! デバイス id はログにも画面にも出さない（security.md A09）。`unsafe` と `RoInitialize` は
//! 使わない（既存の Windows 実装群と同じ前提）。
//!
//! 一次情報:
//! <https://learn.microsoft.com/en-us/uwp/api/windows.devices.lights.lamp>
//! <https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/camera-independent-flashlight>
//!
//! # 既知の天井
//!
//! - パッケージ無しの素の exe から `GetDefaultAsync` が届くかは実機確認待ち（#456）。届かない
//!   と判明したら対応表の `done` を取り下げる。`.get()` に期限は設けない（haptics と同じ）
//! - 最初の点灯要求までは搭載の有無が分からず「消灯しています」と出る。画面を開いただけで
//!   ライトの利用権を取らないため、事前の probe はしない
//! - `AvailabilityChanged` は購読しない。終了時の後片付けは OS に任せる
//! - ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

#[cfg(any(target_os = "windows", test))]
use fandhe_device::DeviceError;

/// `GetDefaultAsync` の失敗 HRESULT を契約のエラーへ写す。0（S_OK + null）は搭載ライト無し。
/// HRESULT 自体は返さない。
#[cfg(any(target_os = "windows", test))]
fn default_failure(code: i32) -> DeviceError {
    if code == 0 {
        DeviceError::Unsupported("flashlight")
    } else {
        DeviceError::Internal("flashlight")
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use windows::Devices::Lights::Lamp;

    use super::default_failure;
    use fandhe_device::flashlight::Flashlight;
    use fandhe_device::DeviceError;

    /// 点灯中だけ保持する `Lamp`（プロセスで 1 個）と、搭載無し確定の印。
    #[derive(Default)]
    struct WindowsFlashlight {
        lamp: Mutex<Option<Lamp>>,
        unsupported: AtomicBool,
    }

    pub(crate) fn implementation() -> Arc<dyn Flashlight> {
        Arc::new(WindowsFlashlight::default())
    }

    fn lock(m: &Mutex<Option<Lamp>>) -> MutexGuard<'_, Option<Lamp>> {
        m.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn internal() -> DeviceError {
        DeviceError::Internal("flashlight")
    }

    impl Flashlight for WindowsFlashlight {
        fn set_on(&self, on: bool) -> Result<(), DeviceError> {
            if on {
                // 保持済みなら冪等に点けるだけ（ロックは API 呼び出しの間だけ持つ）。
                if let Some(lamp) = lock(&self.lamp).as_ref() {
                    return lamp.SetIsEnabled(true).map_err(|_| internal());
                }
                // `.get()` は同期で待つので、ロックを取らずに呼ぶ。
                let lamp = Lamp::GetDefaultAsync()
                    .and_then(|op| op.get())
                    .map_err(|e| {
                        let err = default_failure(e.code().0);
                        if matches!(err, DeviceError::Unsupported(_)) {
                            self.unsupported.store(true, Ordering::Relaxed);
                        }
                        err
                    })?;
                if lamp.SetIsEnabled(true).is_err() {
                    let _ = lamp.Close();
                    return Err(internal());
                }
                if let Some(old) = lock(&self.lamp).replace(lamp) {
                    let _ = old.Close();
                }
                Ok(())
            } else {
                let mut guard = lock(&self.lamp);
                let Some(lamp) = guard.as_ref() else {
                    return Ok(());
                };
                // 失敗時は保持を続ける（実際には点いたままなので、`is_on` が嘘をつかない）。
                lamp.SetIsEnabled(false).map_err(|_| internal())?;
                if let Some(lamp) = guard.take() {
                    let _ = lamp.Close();
                }
                Ok(())
            }
        }

        fn is_on(&self) -> Result<Option<bool>, DeviceError> {
            if self.unsupported.load(Ordering::Relaxed) {
                return Err(DeviceError::Unsupported("flashlight"));
            }
            match lock(&self.lamp).as_ref() {
                Some(lamp) => lamp.IsEnabled().map(Some).map_err(|_| internal()),
                None => Ok(Some(false)),
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use imp::implementation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_failure_maps_null_to_unsupported_and_others_to_internal() {
        assert!(matches!(default_failure(0), DeviceError::Unsupported(_)));
        for code in [-1, 0x8007_0005_u32 as i32, 0x8000_4005_u32 as i32] {
            assert!(matches!(default_failure(code), DeviceError::Internal(_)));
        }
    }
}
