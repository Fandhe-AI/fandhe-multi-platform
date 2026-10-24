//! Windows 向けグローバルホットキーの実装（Issue #767、親 #150）。`device/global_hotkey.rs`
//! （effect ドライバ）の windows 経路から UI スレッドで同期に呼ばれ、契約は
//! `fandhe_device::global_hotkey`（`GlobalHotkey` / `OnPress` / 結果の 4 種）。
//! 組み合わせは Ctrl + Shift + Alt + K の固定 1 つ。ホストテストのため test でもコンパイルする
//! （Win32 を呼ぶ部分は Windows ビルドだけ）。
//!
//! # 方式
//!
//! - 登録ごとに専用スレッド（`fandhe-global-hotkey`）を 1 本立てる。同時に存在するのは高々 1 本で、
//!   登録の記録はそのスレッド（`win::Worker`）だけ。別の「登録中」フラグは持たない
//! - スレッドは `PeekMessageW(PM_NOREMOVE)` でメッセージキューを作ってから
//!   `RegisterHotKey(NULL, id, MOD_CONTROL | MOD_SHIFT | MOD_ALT | MOD_NOREPEAT, 'K')` を呼び、
//!   `GetMessageW` のループで `WM_HOTKEY` を受ける。`RegisterHotKey` と `UnregisterHotKey` は
//!   同じスレッドで呼ぶ（`hWnd` が NULL の登録は呼び出しスレッドに結び付くため）
//! - 解除は `PostThreadMessageW` で専用スレッドへ依頼し、スレッドが `UnregisterHotKey` を呼んで
//!   成否を返す。成功したときだけスレッドを終えて join する。失敗なら登録中のまま残す
//! - UI 側の `register` / `unregister` は専用スレッドの返事を**期限なしで同期に待つ**。専用スレッドが
//!   するのはローカルの Win32 呼び出しだけで、すぐ戻るため。待っている間は `WORKER` のロックを握り、
//!   UI スレッドの操作と `shutdown` を直列にする。専用スレッドは `WORKER` を取らないので循環しない
//! - 押下は `OnPress` を clone してロックの外で呼ぶ。`OnPress`（dispatch の `make_on_press`）は
//!   atomic の操作と mpsc の送信・再描画要求だけで、UI スレッドを待たない
//! - 世代の照合は dispatch 側（`make_on_press` の generation）が行う。再登録は OS を呼ばず
//!   受け口（`OnPress`）を差し替えるだけ
//! - `shutdown` は dispatch が `unregister()` を呼ぶ。成功ならスレッドの終了と join まで済む。
//!   失敗して残ったスレッドは、プロセス終了で OS が登録ごと片付ける
//!
//! # 結果の写像
//!
//! 登録の失敗は `register_outcome_from_windows` で写す（1409 `ERROR_HOTKEY_ALREADY_REGISTERED` は
//! `InUse`、それ以外は `Internal("global_hotkey register")`）。HRESULT は [`win32_code_from_hresult`]
//! で Win32 コードへ戻してから渡す。解除失敗・スレッドの異常終了・`PostThreadMessageW` の失敗は
//! `Internal("global_hotkey unregister")`。いずれも成功にも使用中にも畳まない。
//! 押されたキー・OS のエラーコードはログにもエラーにも出さない。
//!
//! # 守らない範囲
//!
//! 共通の守備範囲は trait doc を参照。Windows 固有: Win キーを含む組み合わせと F12 は OS が予約
//! （この組み合わせは該当しない）、他プロセスが同じ組み合わせを登録していれば `InUse`、
//! 専用スレッドの異常終了後は記録を捨てて失敗を返す。
//! 実機での挙動（unpackaged で動くか、1409 が実際に返るか、`MOD_NOREPEAT` が効くか）は #764 で確認する。

use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
use fandhe_device::global_hotkey::UnsupportedGlobalHotkey;
use fandhe_device::global_hotkey::{GlobalHotkey, UnregisterOutcome};
use fandhe_device::DeviceError;

/// アプリ用の範囲（0x0000〜0xBFFF）のホットキー ID。登録は 1 つだけ。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const HOTKEY_ID: i32 = 1;

/// 仮想キーコード `K`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const VK_K_CODE: u32 = 0x4B;

/// `MOD_ALT`（`winuser.h`）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MOD_ALT_BIT: u32 = 0x0001;
/// `MOD_CONTROL`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MOD_CONTROL_BIT: u32 = 0x0002;
/// `MOD_SHIFT`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MOD_SHIFT_BIT: u32 = 0x0004;
/// `MOD_NOREPEAT`（押しっぱなしの連続通知を止める）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MOD_NOREPEAT_BIT: u32 = 0x4000;

/// 登録する修飾キー（Ctrl + Shift + Alt + NOREPEAT）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MODIFIERS: u32 = MOD_CONTROL_BIT | MOD_SHIFT_BIT | MOD_ALT_BIT | MOD_NOREPEAT_BIT;

/// `WM_APP + 1`。UI 側から専用スレッドへ解除を依頼するメッセージ（wParam・lParam は 0）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WM_APP_UNREGISTER: u32 = 0x8000 + 1;

/// HRESULT から Win32 エラーコードを取り出す。`0x8007xxxx`（FACILITY_WIN32）なら下位 16 bit、
/// それ以外は値をそのまま返す（`register_outcome_from_windows` へ渡すため）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn win32_code_from_hresult(hresult: i32) -> u32 {
    let v = hresult as u32;
    if v & 0xFFFF_0000 == 0x8007_0000 {
        v & 0xFFFF
    } else {
        v
    }
}

/// `UnregisterHotKey` の成否 → 解除の結果。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn unregister_outcome(succeeded: bool) -> Result<UnregisterOutcome, DeviceError> {
    if succeeded {
        Ok(UnregisterOutcome::Unregistered)
    } else {
        Err(DeviceError::Internal("global_hotkey unregister"))
    }
}

/// Windows の `GlobalHotkey` 実装を返す。Windows 以外（ホストテスト）では非対応。
pub(crate) fn implementation() -> Arc<dyn GlobalHotkey> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(win::WindowsGlobalHotkey)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(UnsupportedGlobalHotkey)
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    use fandhe_device::global_hotkey::{
        register_outcome_from_windows, GlobalHotkey, OnPress, RegisterOutcome, UnregisterOutcome,
    };
    use fandhe_device::DeviceError;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        MOD_SHIFT, VK_K,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY,
    };

    use super::{
        unregister_outcome, win32_code_from_hresult, HOTKEY_ID, MODIFIERS, VK_K_CODE,
        WM_APP_UNREGISTER,
    };

    // 自前の定数が windows クレートの値と一致していることをビルド時に確かめる。
    const _: () = assert!(MOD_ALT.0 | MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_NOREPEAT.0 == MODIFIERS);
    const _: () = assert!(VK_K.0 as u32 == VK_K_CODE);
    const _: () = assert!(WM_APP + 1 == WM_APP_UNREGISTER);

    /// 登録中の専用スレッドと、その連絡口。これが登録の記録のすべて。
    struct Worker {
        thread_id: u32,
        join: JoinHandle<()>,
        /// 解除依頼への返事（OS が成功したか）。
        reply_rx: Receiver<bool>,
        /// 押下の通知先。再登録で差し替える。
        on_press: Arc<Mutex<OnPress>>,
    }

    /// UI スレッドと `shutdown` が触る唯一の共有状態。往復の間ずっと握って操作を直列にする。
    static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

    /// Windows の `GlobalHotkey`。状態は `WORKER` に持つためフィールドは無い。
    pub(super) struct WindowsGlobalHotkey;

    impl GlobalHotkey for WindowsGlobalHotkey {
        fn register(&self, on_press: OnPress) -> Result<RegisterOutcome, DeviceError> {
            let mut guard = WORKER.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(worker) = guard.as_ref() {
                if !worker.join.is_finished() {
                    // 登録中の再呼び出し。OS は呼ばず受け口だけ差し替える。
                    *worker.on_press.lock().unwrap_or_else(|e| e.into_inner()) = on_press;
                    return Ok(RegisterOutcome::Registered);
                }
                // スレッドが異常終了している。死んだ登録で成功を返さず、記録を捨てて登録し直す。
                if let Some(dead) = guard.take() {
                    let _ = dead.join.join();
                }
            }

            let on_press = Arc::new(Mutex::new(on_press));
            let (ready_tx, ready_rx) = sync_channel::<Result<u32, u32>>(1);
            let (reply_tx, reply_rx) = channel::<bool>();
            let shared = Arc::clone(&on_press);
            let join = std::thread::Builder::new()
                .name("fandhe-global-hotkey".into())
                .spawn(move || worker_main(ready_tx, reply_tx, shared))
                .map_err(|_| DeviceError::Internal("global_hotkey register"))?;

            // 専用スレッドはローカルの Win32 呼び出しだけで即座に返事をするため期限は設けない。
            match ready_rx.recv() {
                Ok(Ok(thread_id)) => {
                    *guard = Some(Worker {
                        thread_id,
                        join,
                        reply_rx,
                        on_press,
                    });
                    Ok(RegisterOutcome::Registered)
                }
                Ok(Err(code)) => {
                    let _ = join.join();
                    register_outcome_from_windows(false, code)
                }
                Err(_) => {
                    let _ = join.join();
                    Err(DeviceError::Internal("global_hotkey register"))
                }
            }
        }

        #[allow(unsafe_code)]
        fn unregister(&self) -> Result<UnregisterOutcome, DeviceError> {
            let mut guard = WORKER.lock().unwrap_or_else(|e| e.into_inner());
            let Some(worker) = guard.as_ref() else {
                return Ok(UnregisterOutcome::NotRegistered);
            };
            if worker.join.is_finished() {
                // スレッドが消えた以上、OS 側の状態を確かめられない。成功にはせず記録を捨てる。
                if let Some(dead) = guard.take() {
                    let _ = dead.join.join();
                }
                return unregister_outcome(false);
            }
            // SAFETY: ポインタは渡さない（wParam・lParam は 0）。宛先は自分が起こして
            // 生存を確認した専用スレッドの id で、不正な id でも失敗を返すだけ。
            let posted = unsafe {
                PostThreadMessageW(worker.thread_id, WM_APP_UNREGISTER, WPARAM(0), LPARAM(0))
            };
            if posted.is_err() {
                return unregister_outcome(false);
            }
            match worker.reply_rx.recv() {
                Ok(true) => {
                    if let Some(done) = guard.take() {
                        let _ = done.join.join();
                    }
                    unregister_outcome(true)
                }
                Ok(false) => unregister_outcome(false),
                Err(_) => {
                    // 返事の前にスレッドが死んだ。
                    if let Some(dead) = guard.take() {
                        let _ = dead.join.join();
                    }
                    unregister_outcome(false)
                }
            }
        }
    }

    /// 専用スレッドの本体。キュー作成 → 登録 → 返事 → メッセージループ → 解除。
    #[allow(unsafe_code)]
    fn worker_main(
        ready_tx: SyncSender<Result<u32, u32>>,
        reply_tx: Sender<bool>,
        on_press: Arc<Mutex<OnPress>>,
    ) {
        let mut msg = MSG::default();
        // SAFETY: `msg` はこのスレッドのスタック上の有効な MSG。メッセージを取り出さずに呼ぶだけで、
        // `PostThreadMessageW` を受けられるキューが作られる。
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: 引数のない取得関数。
        let thread_id = unsafe { GetCurrentThreadId() };
        // SAFETY: hWnd を NULL にして呼び出しスレッドへ結び付ける。ID・修飾キー・仮想キーは定数。
        let registered =
            unsafe { RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(MODIFIERS), VK_K_CODE) };
        match registered {
            Ok(()) => {
                if ready_tx.send(Ok(thread_id)).is_err() {
                    // 呼び出し側が消えた。登録を残さない。
                    unregister_hotkey();
                    return;
                }
            }
            Err(e) => {
                let _ = ready_tx.send(Err(win32_code_from_hresult(e.code().0)));
                return;
            }
        }

        loop {
            // SAFETY: `msg` は有効な MSG。hWnd NULL・範囲 0 でこのスレッドの全メッセージを受ける。
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if ret <= 0 {
                // WM_QUIT（0）かエラー（-1）。登録を残さず抜ける。
                unregister_hotkey();
                return;
            }
            match msg.message {
                WM_HOTKEY if msg.wParam.0 == HOTKEY_ID as usize => {
                    let cb = on_press.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    // 通知先の panic でループを止めない。ロックの外で呼ぶ。
                    let _ = catch_unwind(AssertUnwindSafe(|| cb()));
                }
                WM_APP_UNREGISTER => {
                    let ok = unregister_hotkey();
                    let _ = reply_tx.send(ok);
                    if ok {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// 登録したスレッドから `UnregisterHotKey` を呼ぶ。
    #[allow(unsafe_code)]
    fn unregister_hotkey() -> bool {
        // SAFETY: 登録したのと同じスレッドで、同じ ID を NULL の hWnd に対して解除する。
        unsafe { UnregisterHotKey(None, HOTKEY_ID) }.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::global_hotkey::{register_outcome_from_windows, RegisterOutcome};

    #[test]
    fn hresult_for_hotkey_already_registered_maps_to_in_use() {
        let code = win32_code_from_hresult(0x8007_0581_u32 as i32);
        assert_eq!(code, 1409);
        assert!(matches!(
            register_outcome_from_windows(false, code),
            Ok(RegisterOutcome::InUse)
        ));
    }

    #[test]
    fn other_hresult_is_internal_failure() {
        let code = win32_code_from_hresult(0x8000_4005_u32 as i32);
        assert_eq!(code, 0x8000_4005);
        assert!(matches!(
            register_outcome_from_windows(false, code),
            Err(DeviceError::Internal("global_hotkey register"))
        ));
    }

    #[test]
    fn unregister_outcome_two_values() {
        assert!(matches!(
            unregister_outcome(true),
            Ok(UnregisterOutcome::Unregistered)
        ));
        assert!(matches!(
            unregister_outcome(false),
            Err(DeviceError::Internal("global_hotkey unregister"))
        ));
    }

    #[test]
    fn combination_is_ctrl_shift_alt_k_norepeat() {
        assert_eq!(VK_K_CODE, 0x4B);
        assert_eq!(MODIFIERS & MOD_NOREPEAT_BIT, MOD_NOREPEAT_BIT);
        assert_eq!(
            MODIFIERS,
            MOD_CONTROL_BIT | MOD_SHIFT_BIT | MOD_ALT_BIT | MOD_NOREPEAT_BIT
        );
    }
}
