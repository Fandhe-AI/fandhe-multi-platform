//! macOS 向けグローバルホットキーの実装（Issue #766、親 #150）。
//! `macos/fandhe_macos_global_hotkey.m`（Carbon の `RegisterEventHotKey`、`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `extern "C"` 2 関数で呼ぶ。`unsafe` はこの呼び出しに
//! 閉じ `// SAFETY:` で契約を明記する。`device/global_hotkey.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::global_hotkey`。Rust へは生ポインタを渡さず `token`（値）だけを渡す。
//!
//! 押下の受け口は `slot`（`token` と [`OnPress`] の組）。`.m` は押下のたびに main thread で
//! [`fandhe_rust_global_hotkey_fire`] を呼び、`token` が現在の slot と一致したときだけ `OnPress` を
//! ロックの外で呼ぶ（古い世代の通知は捨てる）。slot は配送経路であり「登録中」の判定には使わない
//! （記録は `.m` の `EventHotKeyRef` 1 つだけ）。登録失敗・使用中では slot を元へ戻し、解除は
//! 成功・未登録のときだけ slot を空にする（契約表 9）。
//!
//! 結果コードの写像は `fandhe_device::global_hotkey` の純関数（SSOT）。

#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use fandhe_device::global_hotkey::{
    register_outcome_from_macos, unregister_outcome_from_macos, GlobalHotkey, OnPress,
    RegisterOutcome, UnregisterOutcome,
};
use fandhe_device::DeviceError;

mod ffi {
    // SAFETY: `macos/fandhe_macos_global_hotkey.m`（`build.rs` が macOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_global_hotkey_register(token: u32) -> i32;
        fn fandhe_macos_global_hotkey_unregister() -> i32;
    }

    /// 登録の戻り値（OSStatus または番兵）を返す。
    #[allow(unsafe_code)]
    pub(super) fn register(token: u32) -> i32 {
        // SAFETY: 引数は値渡しの `u32` のみ。ポインタは渡さない。main thread 以外では `.m` が番兵を返す。
        unsafe { fandhe_macos_global_hotkey_register(token) }
    }

    /// 解除の戻り値（OSStatus または番兵）を返す。
    #[allow(unsafe_code)]
    pub(super) fn unregister() -> i32 {
        // SAFETY: 引数なし。main thread 以外では `.m` が番兵を返す。
        unsafe { fandhe_macos_global_hotkey_unregister() }
    }
}

type Slot = Option<(u32, OnPress)>;

/// 押下の受け口。`.m` からの通知（`fandhe_rust_global_hotkey_fire`）が参照する。
static SLOT: Mutex<Slot> = Mutex::new(None);
/// 次に発行する token（0 を飛ばして進める）。
static NEXT_TOKEN: AtomicU32 = AtomicU32::new(1);

fn lock_slot() -> std::sync::MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

fn next_token() -> u32 {
    loop {
        let t = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
        if t != 0 {
            return t;
        }
    }
}

/// `.m` が押下のたびに main thread で呼ぶ。token が現在の slot と一致したときだけ `OnPress` を呼ぶ。
///
/// `#[no_mangle]` はシンボル名を固定するだけで、この関数自体は `unsafe` 操作を含まない。シンボル名
/// `fandhe_rust_global_hotkey_fire` は本クレート内で一意（`.m` の `extern` 宣言と対）。
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn fandhe_rust_global_hotkey_fire(token: u32) {
    let on_press = {
        let slot = lock_slot();
        match slot.as_ref() {
            Some((t, f)) if *t == token => Some(Arc::clone(f)),
            _ => None,
        }
    };
    if let Some(f) = on_press {
        f();
    }
}

struct MacGlobalHotkey;

impl GlobalHotkey for MacGlobalHotkey {
    fn register(&self, on_press: OnPress) -> Result<RegisterOutcome, DeviceError> {
        let token = next_token();
        let previous = lock_slot().replace((token, on_press));
        let result = register_outcome_from_macos(ffi::register(token));
        if !matches!(result, Ok(RegisterOutcome::Registered)) {
            // 失敗・使用中・非 main thread: 受け口を元へ戻す。
            *lock_slot() = previous;
        }
        result
    }

    fn unregister(&self) -> Result<UnregisterOutcome, DeviceError> {
        let result = unregister_outcome_from_macos(ffi::unregister());
        if result.is_ok() {
            *lock_slot() = None;
        }
        result
    }
}

/// macOS の `GlobalHotkey` 実装を返す（`device/global_hotkey.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn GlobalHotkey> {
    Arc::new(MacGlobalHotkey)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test` のスレッドは main thread ではないため、登録は成功を返さず失敗になる。
    /// 実際の登録と押下は #764 の Mac 目視で扱う。
    #[test]
    fn register_off_main_thread_fails_and_leaves_no_slot() {
        let imp = MacGlobalHotkey;
        let r = imp.register(Arc::new(|| {}));
        assert!(matches!(
            r,
            Err(DeviceError::Internal("global_hotkey register"))
        ));
        assert!(lock_slot().is_none());
    }

    #[test]
    fn unregister_off_main_thread_fails() {
        let imp = MacGlobalHotkey;
        assert!(matches!(
            imp.unregister(),
            Err(DeviceError::Internal("global_hotkey unregister"))
        ));
    }

    #[test]
    fn fire_ignores_unknown_token() {
        // 受け口が空・token 不一致では何も呼ばない（パニックしない）。
        fandhe_rust_global_hotkey_fire(u32::MAX);
    }
}
