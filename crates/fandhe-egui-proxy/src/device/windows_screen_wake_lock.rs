//! Windows 向け画面スリープ抑止実装（Issue #1278）。Win32 の `SetThreadExecutionState` を
//! 呼ぶ（package identity 不要）。`device/screen_wake_lock.rs` の Windows 経路から使われる。
//!
//! # スレッド
//!
//! `SetThreadExecutionState(ES_CONTINUOUS | ...)` の効果は**呼んだスレッドに紐づき**、そのスレッドの
//! 終了で消える。そのため effect（winit の UI スレッド）からだけ呼び、ワーカースレッドでは
//! 取得・解除しない。プロセス（スレッド）終了時は OS が片付けるため後片付けは持たない。
//!
//! # 状態
//!
//! 読み戻し API が無いので `Mutex<bool>` で自前に記録する。判定と更新は同じロックの内側で行い、
//! 呼び出しが失敗（戻り値 0）したら記録を変えない（嘘の成功の禁止）。
//!
//! # フラグ
//!
//! 目的は画面の維持なので `ES_CONTINUOUS | ES_DISPLAY_REQUIRED`（表示のアイドルタイマーを
//! 止める）のみ。解除は `ES_CONTINUOUS` 単独。実機での効き方は #1272 で確認する。
//! `unsafe` は `SetThreadExecutionState` の 1 呼び出しに閉じる（coding-rust.md の FFI 例外規定）。

use std::sync::{Arc, Mutex};

use fandhe_device::screen_wake_lock::ScreenWakeLock;
use fandhe_device::DeviceError;
use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED};

use super::screen_wake_lock::exec_state_result;

/// Windows の `ScreenWakeLock` 実装。
struct WindowsScreenWakeLock {
    held: Mutex<bool>,
}

impl ScreenWakeLock for WindowsScreenWakeLock {
    fn set_keep_awake(&self, keep: bool) -> Result<(), DeviceError> {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        if *held == keep {
            return Ok(());
        }
        let flags = if keep {
            ES_CONTINUOUS | ES_DISPLAY_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // SAFETY: 引数は値渡しのフラグのみでポインタを経由しない。呼んだスレッドの
        // 実行状態だけを変え、呼び出し元は UI スレッド固定（モジュール doc）。
        #[allow(unsafe_code)]
        let prev = unsafe { SetThreadExecutionState(flags) };
        exec_state_result(prev.0)?;
        *held = keep;
        Ok(())
    }

    fn is_held(&self) -> bool {
        *self.held.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// プロセスで 1 個の Windows 実装を返す。
pub(crate) fn implementation() -> Arc<dyn ScreenWakeLock> {
    Arc::new(WindowsScreenWakeLock {
        held: Mutex::new(false),
    })
}
