//! macOS 向け画面スリープ抑止実装（Issue #1278）。IOKit の電源アサーション
//! （`macos/fandhe_macos_screen_wake_lock.m`、`build.rs` が macOS ターゲットの feature `device`
//! 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数で取る / 外す。`unsafe` は
//! この呼び出しに閉じ `// SAFETY:` で契約を明記する（`macos_background_task.rs` と同型、
//! coding-rust.md の FFI 例外規定準拠）。`device/screen_wake_lock.rs` の macOS 経路から使われる。
//!
//! # 状態
//!
//! アサーション ID を `Mutex<Option<u32>>` で持つ。`Some` が「効いている」。判定と更新は同じ
//! ロックの内側で行い（`device-bridge-contracts.md`「2. 並行性・世代」）、取得済みでの再取得は
//! FFI を呼ばない（アサーションを漏らさない、同「9.」）。取得 FFI が失敗したら `None` のまま、
//! 解放 FFI が失敗したら `Some` を残して `Err(Internal)`（効いているものを隠さない）。
//!
//! # アプリ終了時
//!
//! プロセスが終わると OS がアサーションを解放するため、`device::shutdown` への追加は不要
//! （`.m` ファイル冒頭コメントと対応）。

use std::sync::{Arc, Mutex};

use fandhe_device::screen_wake_lock::ScreenWakeLock;
use fandhe_device::DeviceError;

// `macos/fandhe_macos_screen_wake_lock.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_macos_screen_wake_lock_acquire(out_id: *mut u32) -> i32;
    fn fandhe_macos_screen_wake_lock_release(id: u32) -> i32;
}

/// `.m` の戻りコード（0 = 成功、それ以外 = 失敗）を契約のエラー種別へ写す。
fn decode(code: i32) -> Result<(), DeviceError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("screen-wake-lock"))
    }
}

/// アサーション ID の保持と冪等な取得 / 解除。FFI は引数で差し込めるようにし、ホストの
/// テストで二重取得・解除失敗を検証できる。
#[derive(Default)]
struct Holder {
    id: Mutex<Option<u32>>,
}

impl Holder {
    fn set(
        &self,
        keep: bool,
        acquire: impl FnOnce() -> Result<u32, DeviceError>,
        release: impl FnOnce(u32) -> Result<(), DeviceError>,
    ) -> Result<(), DeviceError> {
        let mut id = self.id.lock().unwrap_or_else(|p| p.into_inner());
        match (keep, *id) {
            (true, None) => {
                *id = Some(acquire()?);
                Ok(())
            }
            (false, Some(held)) => {
                release(held)?;
                *id = None;
                Ok(())
            }
            // 取得済みでの再取得・未取得での解除は何もしない（冪等）。
            _ => Ok(()),
        }
    }

    fn is_held(&self) -> bool {
        self.id.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// macOS の `ScreenWakeLock` 実装（IOKit の電源アサーション）。
#[cfg(target_os = "macos")]
struct MacScreenWakeLock(Holder);

#[cfg(target_os = "macos")]
impl ScreenWakeLock for MacScreenWakeLock {
    #[allow(unsafe_code)]
    fn set_keep_awake(&self, keep: bool) -> Result<(), DeviceError> {
        self.0.set(
            keep,
            || {
                let mut id = 0u32;
                // SAFETY: `id` はこの呼び出しの間だけ有効な書き込み先で、`.m` は NULL でない
                // ポインタにだけ書く。他の引数は無く、`.m` は Cocoa オブジェクトを作らない。
                let code = unsafe { fandhe_macos_screen_wake_lock_acquire(&mut id) };
                decode(code).map(|()| id)
            },
            |id| {
                // SAFETY: 引数はプリミティブのみでポインタを経由しない。`id` は acquire が返した
                // ID で、解放に成功するまで `Holder` が保持している（二重解放しない）。
                let code = unsafe { fandhe_macos_screen_wake_lock_release(id) };
                decode(code)
            },
        )
    }

    fn is_held(&self) -> bool {
        self.0.is_held()
    }
}

/// プロセスで 1 個の macOS 実装を返す。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn ScreenWakeLock> {
    Arc::new(MacScreenWakeLock(Holder::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn internal() -> DeviceError {
        DeviceError::Internal("screen-wake-lock")
    }

    #[test]
    fn decode_maps_codes() {
        assert!(decode(0).is_ok());
        assert!(matches!(decode(1), Err(DeviceError::Internal(_))));
        assert!(matches!(decode(2), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn double_acquire_calls_ffi_once() {
        let holder = Holder::default();
        let calls = Cell::new(0);
        for _ in 0..2 {
            holder
                .set(
                    true,
                    || {
                        calls.set(calls.get() + 1);
                        Ok(7)
                    },
                    |_| Ok(()),
                )
                .unwrap();
        }
        assert_eq!(calls.get(), 1);
        assert!(holder.is_held());
    }

    #[test]
    fn acquire_failure_stays_released() {
        let holder = Holder::default();
        assert!(holder.set(true, || Err(internal()), |_| Ok(())).is_err());
        assert!(!holder.is_held());
    }

    #[test]
    fn release_failure_stays_held_and_success_clears() {
        let holder = Holder::default();
        holder.set(true, || Ok(7), |_| Ok(())).unwrap();
        assert!(holder.set(false, || Ok(8), |_| Err(internal())).is_err());
        assert!(holder.is_held());
        holder
            .set(
                false,
                || Ok(8),
                |id| if id == 7 { Ok(()) } else { Err(internal()) },
            )
            .unwrap();
        assert!(!holder.is_held());
    }

    #[test]
    fn release_without_hold_is_noop() {
        let holder = Holder::default();
        holder.set(false, || Ok(1), |_| Err(internal())).unwrap();
        assert!(!holder.is_held());
    }
}
