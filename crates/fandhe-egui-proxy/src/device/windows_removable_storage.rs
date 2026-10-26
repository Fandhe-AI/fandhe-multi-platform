//! Windows 向け外部ストレージの着脱の実装（Issue #858、親 #175）。`device/removable_storage.rs`
//! （effect ドライバ）の windows 経路から UI スレッドで呼ばれ、契約は
//! `fandhe_device::removable_storage`。ホストでは写像（`fandhe_device` の純関数）だけをテストし、
//! Win32 を呼ぶ部分は Windows ビルドだけ。
//!
//! # 方式
//!
//! - 購読ごとに専用スレッド（`fandhe-removable-storage`）を 1 本立てる。スレッドは
//!   `PeekMessageW(PM_NOREMOVE)` でメッセージキューを作り、不可視のトップレベルウィンドウ
//!   （`WS_EX_TOOLWINDOW`・`WS_VISIBLE` なし）を作る。ボリュームの `WM_DEVICECHANGE`
//!   （`DBT_DEVICEARRIVAL` / `DBT_DEVICEREMOVECOMPLETE`）はトップレベルウィンドウへ自動で配られる。
//!   `RegisterDeviceNotification` は `DBT_DEVTYP_VOLUME` では失敗する（公式）ため使わない。
//!   `HWND_MESSAGE` は使わない（ブロードキャストはトップレベルウィンドウ向けで、message-only へ届く保証が無い。
//!   `windows_sleep_lock_events.rs` と同じ判断）
//! - ウィンドウプロシージャは lParam の null とヘッダのサイズを確かめてから `dbch_devicetype` を読み、
//!   `event_from_windows` で出来事に写して、スレッドローカルの `id` で `fire` を呼ぶだけ（すぐ戻る）。
//!   `dbcv_unitmask` は読まない（一覧は OS へ問い直す）
//! - 停止（[`WinGuard`] の drop）は「登録解除 → `PostThreadMessageW` で停止を依頼 → join」の順。
//!   スレッドは `DestroyWindow` してループを抜ける。ローカルの Win32 呼び出しだけなので join に期限は
//!   設けない（`windows_sleep_lock_events.rs` と同じ理由）
//! - `GetMessageW` が -1 を返したら `DestroyWindow` してから `Signal::Failed` を送る（実行中の失敗）
//! - 開始の途中で失敗したら `DestroyWindow` → join → 登録解除の順に片付けて `Err` を返す
//!
//! # OS を正にする範囲
//!
//! マウント（到着）と取り外し（完了）の 2 種を知らせる。取り外しの直前・その他の変化は取らない。
//! 一覧は `GetLogicalDrives` / `GetDriveTypeW` / `GetVolumeInformationW` で毎回 OS から読む。
//! 読み取り中だけ `SetThreadErrorMode(SEM_FAILCRITICALERRORS)` で、空のカードリーダーに対する
//! 「ディスクを挿入してください」のダイアログを抑止し、読み取り後に元へ戻す。
//! ボリュームのシリアル番号・ファイルシステム名・GUID パスは取らない。OS のエラーコードはログにも
//! エラーにも出さない。
//!
//! # 守らない範囲
//!
//! 共通の守備範囲は trait doc を参照。Windows 固有: 同じ挿入で通知が複数回届きうる、
//! `GetDriveTypeW` は USB の HDD を `DRIVE_FIXED` と返す、ネットワークドライブの読み取りは I/O で待つ。
//! 実機での挙動（不可視のトップレベルウィンドウに通知が届くか）は #854 で確認する。

use std::sync::Arc;

use fandhe_device::removable_storage::RemovableStorage;

/// Windows の `RemovableStorage` 実装を返す（`device/removable_storage.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn RemovableStorage> {
    Arc::new(win::WindowsRemovableStorage)
}

mod win {
    use std::cell::Cell;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc::sync_channel;
    use std::thread::JoinHandle;

    use fandhe_device::removable_storage::{
        event_from_windows, kind_from_windows_drive_type, state_from_windows_volume_info, volumes,
        EventSupport, OnSignal, RemovableStorage, Signal, SubscriptionGuard, Volume, VolumeKind,
        VolumeState,
    };
    use fandhe_device::DeviceError;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PeekMessageW, PostThreadMessageW, RegisterClassW, TranslateMessage, DEV_BROADCAST_HDR, MSG,
        PM_NOREMOVE, WM_APP, WM_DEVICECHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
    };

    use super::super::removable_storage::{fire, register, unregister};

    /// `WM_APP + 1`。UI 側から専用スレッドへ停止を依頼するメッセージ。
    const WM_APP_STOP: u32 = WM_APP + 1;

    // 契約側の自前定数が windows クレートの値と一致していることをビルド時に確かめる。
    // `DRIVE_*` は `Win32_System_WindowsProgramming`（未導入）にあるため `GetDriveTypeW` の値は自前定数だけを使う。
    const _: () = assert!(WM_DEVICECHANGE == fandhe_device::removable_storage::WM_DEVICECHANGE);
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::DBT_DEVICEARRIVAL as usize
            == fandhe_device::removable_storage::DBT_DEVICEARRIVAL
    );
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::DBT_DEVICEREMOVECOMPLETE as usize
            == fandhe_device::removable_storage::DBT_DEVICEREMOVECOMPLETE
    );
    const _: () = assert!(
        windows::Win32::UI::WindowsAndMessaging::DBT_DEVTYP_VOLUME.0
            == fandhe_device::removable_storage::DBT_DEVTYP_VOLUME
    );
    const _: () = assert!(
        windows::Win32::Foundation::ERROR_NOT_READY.0
            == fandhe_device::removable_storage::ERROR_NOT_READY
    );

    const START_FAILED: DeviceError = DeviceError::Internal("removable_storage start");
    const READ_FAILED: DeviceError = DeviceError::Internal("removable_storage read");

    /// ボリューム名のバッファ長（`MAX_PATH + 1`）。
    const LABEL_CHARS: usize = 261;

    thread_local! {
        /// このスレッドの購読 id（ウィンドウプロシージャが `fire` の宛先に使う）。
        static SUBSCRIPTION_ID: Cell<u64> = const { Cell::new(0) };
    }

    /// Windows の `RemovableStorage`。状態は購読ごとの専用スレッドに持つためフィールドは無い。
    pub(super) struct WindowsRemovableStorage;

    impl RemovableStorage for WindowsRemovableStorage {
        fn support(&self) -> EventSupport {
            // 順序は ALL_EVENTS: マウント・取り外しは取れる、取り外しの直前・その他の変化は取れない。
            EventSupport {
                events: [true, false, true, false],
            }
        }

        fn start(&self, on_signal: OnSignal) -> Result<Box<dyn SubscriptionGuard>, DeviceError> {
            let id = register(on_signal);
            let (ready_tx, ready_rx) = sync_channel::<Result<u32, ()>>(1);
            let join = match std::thread::Builder::new()
                .name("fandhe-removable-storage".into())
                .spawn(move || worker_main(id, ready_tx))
            {
                Ok(j) => j,
                Err(_) => {
                    unregister(id);
                    return Err(START_FAILED);
                }
            };
            // 専用スレッドはローカルの Win32 呼び出しだけで即座に返事をするため期限は設けない。
            match ready_rx.recv() {
                Ok(Ok(thread_id)) => Ok(Box::new(WinGuard {
                    id,
                    thread_id,
                    join: Some(join),
                })),
                _ => {
                    let _ = join.join();
                    unregister(id);
                    Err(START_FAILED)
                }
            }
        }

        #[allow(unsafe_code)]
        fn read_volumes(&self) -> Result<Vec<Volume>, DeviceError> {
            // ダイアログが出うる状態で読まない: 設定に失敗したら読まずに失敗を返す。
            let _mode = ErrorModeGuard::new().ok_or(READ_FAILED)?;
            // SAFETY: 引数のない取得関数。
            let mask = unsafe { GetLogicalDrives() };
            if mask == 0 {
                return Err(READ_FAILED);
            }
            let mut raw: Vec<(Option<String>, VolumeKind, VolumeState)> = Vec::new();
            for bit in 0..26u32 {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                let letter = char::from(b'A' + bit as u8);
                // 自前で組み立てた "X:\" の NUL 終端（OS から受けたパスは使わない）。
                let root: [u16; 4] = [letter as u16, ':' as u16, '\\' as u16, 0];
                // SAFETY: `root` は NUL 終端の有効な UTF-16 で、呼び出しの間生きている。
                let drive_type = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
                let kind = kind_from_windows_drive_type(drive_type);
                let mut label = [0u16; LABEL_CHARS];
                // シリアル番号・ファイルシステム名は `None` で取らない。
                // SAFETY: `root` は上と同じ。`label` は有効な書き込み先で、長さはスライスが伝える。
                let result = unsafe {
                    GetVolumeInformationW(
                        PCWSTR(root.as_ptr()),
                        Some(&mut label[..]),
                        None,
                        None,
                        None,
                        None,
                    )
                };
                let (state, label_text) = match result {
                    Ok(()) => {
                        let end = label.iter().position(|&c| c == 0).unwrap_or(label.len());
                        (
                            state_from_windows_volume_info(true, 0),
                            String::from_utf16_lossy(&label[..end]),
                        )
                    }
                    Err(e) => {
                        // HRESULT_FROM_WIN32 形式（0x8007xxxx）なら下位 16 ビットが Win32 エラー。
                        let h = e.code().0 as u32;
                        let win32 = if (h >> 16) == 0x8007 { h & 0xFFFF } else { h };
                        (state_from_windows_volume_info(false, win32), String::new())
                    }
                };
                // 表示名は「X:」にラベルがあれば続ける。マウントパス・ボリューム GUID は取らない。
                let name = if label_text.trim().is_empty() {
                    format!("{letter}:")
                } else {
                    format!("{letter}: {label_text}")
                };
                raw.push((Some(name), kind, state));
            }
            Ok(volumes(raw))
        }
    }

    /// 一覧の読み取り中だけ `SEM_FAILCRITICALERRORS` を設定し、drop で元へ戻す。
    struct ErrorModeGuard {
        old: THREAD_ERROR_MODE,
    }

    impl ErrorModeGuard {
        #[allow(unsafe_code)]
        fn new() -> Option<Self> {
            let mut old = THREAD_ERROR_MODE(0);
            // SAFETY: `old` は有効なローカル変数への書き込み先。スレッド単位の設定で、drop で戻す。
            unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut old)) }.ok()?;
            Some(Self { old })
        }
    }

    impl Drop for ErrorModeGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: 設定時に OS が返した元の値へ戻す。失敗しても呼び出し側へ影響しない。
            let _ = unsafe { SetThreadErrorMode(self.old, None) };
        }
    }

    /// 実行中の購読。drop で専用スレッドを止める。
    struct WinGuard {
        id: u64,
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    impl SubscriptionGuard for WinGuard {}

    impl Drop for WinGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // 先に登録を外し、以後の遅れた呼び出しを捨てさせてから専用スレッドを止める。
            unregister(self.id);
            // SAFETY: ポインタは渡さない（wParam・lParam は 0）。宛先は自分が起こした専用スレッドの
            // id で、既に終了していれば失敗を返すだけ（その場合も join は戻る）。
            let _ =
                unsafe { PostThreadMessageW(self.thread_id, WM_APP_STOP, WPARAM(0), LPARAM(0)) };
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    /// ウィンドウプロシージャ。出来事に写せるものだけを `fire` し、すぐ戻る。
    #[allow(unsafe_code)]
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_DEVICECHANGE && lparam.0 != 0 {
            // SAFETY: `WM_DEVICECHANGE` で lParam が非 NULL のとき `DEV_BROADCAST_HDR` 以上の構造体を
            // 指す（公式）。アラインメントを仮定せず `read_unaligned` でヘッダだけ読み、サイズを
            // 確かめるまで `dbch_devicetype` を信用しない。読んだ値はコピーでポインタは残らない。
            let hdr = unsafe { std::ptr::read_unaligned(lparam.0 as *const DEV_BROADCAST_HDR) };
            if hdr.dbch_size as usize >= std::mem::size_of::<DEV_BROADCAST_HDR>() {
                if let Some(event) = event_from_windows(wparam.0, hdr.dbch_devicetype.0) {
                    let id = SUBSCRIPTION_ID.with(|c| c.get());
                    // 通知先の panic でメッセージループを止めない。
                    let _ = catch_unwind(AssertUnwindSafe(|| fire(id, Signal::Event(event))));
                }
            }
        }
        // SAFETY: OS から渡された引数をそのまま既定の処理へ渡す。
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// 専用スレッドの本体。キュー作成 → ウィンドウ → 返事 → メッセージループ → 解除。
    #[allow(unsafe_code)]
    fn worker_main(id: u64, ready_tx: std::sync::mpsc::SyncSender<Result<u32, ()>>) {
        SUBSCRIPTION_ID.with(|c| c.set(id));
        let mut msg = MSG::default();
        // SAFETY: `msg` はこのスレッドのスタック上の有効な MSG。メッセージを取り出さずに呼ぶだけで、
        // `PostThreadMessageW` を受けられるキューが作られる。
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: 引数のない取得関数。
        let thread_id = unsafe { GetCurrentThreadId() };

        let Ok(hwnd) = create_window() else {
            let _ = ready_tx.send(Err(()));
            return;
        };
        if ready_tx.send(Ok(thread_id)).is_err() {
            // 呼び出し側が消えた。ウィンドウを残さない。
            destroy(hwnd);
            return;
        }

        let mut failed = false;
        loop {
            // SAFETY: `msg` は有効な MSG。hWnd NULL・範囲 0 でこのスレッドの全メッセージを受ける。
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if ret < 0 {
                failed = true;
                break;
            }
            if ret == 0 || msg.message == WM_APP_STOP {
                break;
            }
            // SAFETY: `GetMessageW` が埋めた有効な MSG。
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        destroy(hwnd);
        if failed {
            fire(id, Signal::Failed);
        }
    }

    /// このスレッドが作ったウィンドウを壊す。
    #[allow(unsafe_code)]
    fn destroy(hwnd: HWND) {
        // SAFETY: このスレッドが作ったウィンドウで、他では保持していない。
        let _ = unsafe { DestroyWindow(hwnd) };
    }

    /// 不可視のトップレベルウィンドウを作る。`WS_VISIBLE` を付けず `HWND_MESSAGE` も使わない。
    #[allow(unsafe_code)]
    fn create_window() -> Result<HWND, ()> {
        let class_name = w!("FandheRemovableStorageWindow");
        // SAFETY: 引数 NULL で現在のモジュールのハンドルを得る。
        let instance = unsafe { GetModuleHandleW(None) }.map_err(|_| ())?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // SAFETY: `class` は有効な WNDCLASSW。クラス名は静的な文字列で、プロシージャは `'static`。
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            // SAFETY: 引数のない取得関数。
            if unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
                return Err(());
            }
        }
        // SAFETY: 全パラメータは固定値かローカル変数。`WS_VISIBLE` を付けないため画面に出ない。
        unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name,
                class_name,
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|_| ())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Arc;
        use windows::Win32::System::Diagnostics::Debug::GetThreadErrorMode;

        #[test]
        fn start_and_drop_returns() {
            let imp = WindowsRemovableStorage;
            let guard = imp.start(Arc::new(|_| {})).expect("開始できる");
            drop(guard);
        }

        #[test]
        #[allow(unsafe_code)]
        fn read_volumes_succeeds_and_restores_error_mode() {
            let imp = WindowsRemovableStorage;
            // SAFETY: 引数のない取得関数。
            let before = unsafe { GetThreadErrorMode() };
            let v = imp.read_volumes().expect("読める");
            assert!(!v.is_empty(), "システムドライブは常にある");
            // SAFETY: 同上。
            let after = unsafe { GetThreadErrorMode() };
            assert_eq!(before, after);
        }
    }
}
