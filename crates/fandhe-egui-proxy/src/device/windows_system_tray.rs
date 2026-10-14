//! Windows 向けシステムトレイ実装（Issue #1039）。`device/system_tray.rs::
//! platform_tray` の Windows 分岐から呼ばれ、`Shell_NotifyIconW`（通知領域
//! アイコン）+ 不可視のトップレベルウィンドウ + `TrackPopupMenu`（右クリック
//! メニュー）で実装する。`windows_share.rs`/`windows_biometric.rs` と同じく
//! `unsafe` は Win32 呼び出しに限り `// SAFETY:` を各呼び出し直前に付与する
//! （`lib.rs` の Windows `deny(unsafe_code)` 緩和・coding-rust.md の FFI
//! 例外規定準拠）。
//!
//! # 専用スレッド + メッセージループを使う理由
//!
//! 通知領域アイコンのコールバック（`WM_APP + 1`）はアイコンを登録した
//! スレッドのメッセージキューへ届く。本実装は UI スレッド（eframe の
//! メッセージループ）とは別にアイコン専用のスレッドを起こし、そこでウィンドウ・
//! メッセージループを完結させる（`fandhe-egui-proxy` 自体のメインウィンドウの
//! `WndProc` へ割り込まない設計、変更範囲を最小化するため）。
//!
//! # 不可視ウィンドウに `HWND_MESSAGE`（message-only window）を使わない理由
//!
//! message-only ウィンドウは `SetForegroundWindow` が効かず、`TrackPopupMenu` の
//! メニューが外側クリックで閉じない既知の問題がある。そのため `WS_EX_TOOLWINDOW`
//! （タスクバー・Alt+Tab に出さない）だけを付けた通常の（ただし `WS_VISIBLE` を
//! 付けない）トップレベルウィンドウを使う（実装計画 §3.5 参照）。
//!
//! # 待ち時間切れを持たない理由（オーナー判断 10/2、Issue #1039 是正）
//!
//! `hide` の後始末依頼は `SendMessageTimeoutW`（期限付き）ではなく
//! `SendMessageW`（期限無し、同期）で送る。`SendMessageW` は呼び出し先の
//! `WndProc`（アイコン専用スレッド上）が戻り値を返すまで呼び出し元をブロック
//! するため、`NIM_DELETE` の成否をその戻り値として直接受け取れる。これにより
//! 「依頼は届いたが結果が未確定」という中間状態（旧実装の `Slot::PendingHide`・
//! `CLEANED_GENERATION`・`DELETE_OUTCOME`・`wait_for_cleanup` のポーリング）が
//! そもそも発生しない。
//!
//! 無期限待ちでもアイコン専用スレッドが固まらないのは次の理由による。
//! - アイコン専用スレッドはこのモジュールの `WndProc`/メッセージループ専用で、
//!   他の重い処理を行わない。`WM_TRAY_HIDE` の処理（`NIM_DELETE` + 条件付き
//!   `DestroyWindow`）は短時間で完了する
//! - `TrackPopupMenu` のモーダルループ中でも、Windows はそのウィンドウの
//!   スレッド宛てに送られた `SendMessage`（sent message、`PostMessage` とは
//!   異なる）をモーダルループ内部でディスパッチする（送信元がブロックした
//!   まま相手のモーダルループが進まなくなるデッドロックを OS 側が避ける
//!   ための挙動）。そのため `hide` がメニュー表示中に呼ばれても `WM_TRAY_HIDE`
//!   は処理される
//! `show` の起動完了待ち（`ready_rx.recv()`）も同じ理由で期限無しにする。
//! ウィンドウ作成・`Shell_NotifyIconW(NIM_ADD)` はいずれも専用スレッド内の
//! ローカルな Win32 呼び出しで、外部からの応答待ちが無いため、失敗か成功かが
//! 速やかに確定し `ready_tx` へ届く。
//!
//! `show`/`hide` は `fandhe-egui-proxy::device::system_tray::drive` が生成する
//! 作業スレッドから呼ばれる契約（P1 是正、`device/system_tray.rs` モジュール doc
//! 「別スレッドへ逃がす理由」参照）。呼び出しスレッド自体は毎回異なり得るが、
//! `drive` 側の二重 dispatch 防止（`mark_showing`/`mark_hiding` が完了を待たず
//! 次の要求を塞ぐ）により同時に実行される `show`/`hide` は常に 1 件のみのため、
//! `CURRENT` の判定〜更新の間に別の `show`/`hide` が割り込むことは無い。
//!
//! # 選択の受け渡し
//!
//! アイコン専用スレッドの `WndProc` から [`fandhe_device::system_tray::
//! SelectionQueue::push`] へ直接積む（`apple_system_tray.rs` の
//! `fandhe_rust_system_tray_selected` と同じ橋渡し方式だが、FFI を介さず
//! 同一プロセス内の Rust 関数呼び出しで完結する）。

use std::sync::mpsc::sync_channel;
use std::sync::Mutex;
use std::time::Duration;

use fandhe_device::system_tray::{HideOutcome, SelectionQueue, ShowOutcome, SystemTray, TrayMenu};
use fandhe_device::DeviceError;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, EndMenu, GetCursorPos, GetMessageW, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SendMessageW, SetForegroundWindow, TrackPopupMenu,
    TranslateMessage, CW_USEDEFAULT, IDI_APPLICATION, MF_STRING, MSG, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_APP, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
};

/// アイコンのコールバックメッセージ（`uCallbackMessage`）。右クリック/左クリックの
/// 判定は `lParam`（`WM_RBUTTONUP`/`WM_LBUTTONUP`）で行う。
const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
/// `hide()` が専用スレッドへ送る後始末メッセージ（`WndProc` が `NIM_DELETE` +
/// `DestroyWindow` を行う）。`SendMessageW`（同期・期限無し）で送るため、戻り値
/// （1 = 削除成功、0 = 失敗）がそのまま `hide()` の結果になる（モジュール doc
/// 「待ち時間切れを持たない理由」参照）。
const WM_TRAY_HIDE: u32 = WM_APP + 2;
/// [`delete_icon_with_retry`] が `NIM_DELETE` を再試行する回数・間隔。
/// Explorer 再起動直後等の一過性失敗を吸収する。
const DELETE_RETRY_ATTEMPTS: u32 = 3;
const DELETE_RETRY_DELAY: Duration = Duration::from_millis(50);

const WINDOW_CLASS_NAME: PCWSTR = windows::core::w!("FandheSystemTrayWindow");

/// 選択の受け渡し先（`apple_system_tray.rs::QUEUE` と同じ役割）。
static QUEUE: SelectionQueue = SelectionQueue::new();

/// 表示中のアイコン専用スレッドの状態。`hide` が `Slot::Idle` に戻す際に使う。
/// 選択の世代照合（`QUEUE.push`）は専用スレッド自身の `CURRENT_GENERATION`
/// （`thread_local`）で行うため、ここでは `hwnd` のみを持つ。
struct TrayThread {
    hwnd: isize,
}

/// `CURRENT` の状態。「未表示」と「表示中（専用スレッドの hwnd・世代）」の
/// 2 値のみ（待ち時間切れに由来する中間状態は持たない、モジュール doc参照）。
enum Slot {
    Idle,
    Shown(TrayThread),
}

static CURRENT: Mutex<Slot> = Mutex::new(Slot::Idle);

thread_local! {
    // `WndProc` がメニュー項目を組み立てるために参照する、現在のメニュー項目
    // 一覧。アイコン専用スレッドはプロセス中に高々 1 本しか存在しない
    // （`CURRENT` が `Idle` の間だけ次の `show` が新しいスレッドを起こす）ため
    // `thread_local` で足りる（`GWLP_USERDATA` への生ポインタ操作を避ける、
    // 実装計画 §3.5 参照）。
    static THREAD_ITEMS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Windows 向け `SystemTray` 実装。`fandhe-egui-proxy::device::system_tray::drive`
/// が生成する作業スレッドから呼ばれる契約（モジュール doc参照、P1 是正）。
pub(crate) struct WindowsSystemTray;

impl SystemTray for WindowsSystemTray {
    fn show(&self, menu: &TrayMenu) -> Result<ShowOutcome, DeviceError> {
        // `CURRENT` は表示が確定する（`Shown` を書く）まで握り続ける。作業スレッドで
        // 表示中にアプリが終了すると `on_exit` の `hide` が走るが、ロックを放して
        // いると `Idle` を見て `NotShown` を返し、その後に追加されたアイコンが
        // 残る（PR #1041 codex・Bugbot 指摘）。握っていれば `hide` は表示の確定を
        // 待ってから削除する。専用スレッド（`tray_thread_main`/`wnd_proc`）は
        // `CURRENT` を取らないため、ここで待ってもデッドロックしない。
        let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(&*current, Slot::Shown(_)) {
            return Ok(ShowOutcome::AlreadyShown);
        }

        let generation = QUEUE.begin(menu.items().len());
        let title = menu.title().to_string();
        let items = menu.items().to_vec();
        let (ready_tx, ready_rx) = sync_channel::<Option<isize>>(0);

        std::thread::spawn(move || {
            tray_thread_main(generation, title, items, ready_tx);
        });

        // 期限無しで待つ（モジュール doc「待ち時間切れを持たない理由」参照）。
        // 専用スレッドは必ずローカルな Win32 呼び出しの成否を速やかに送る。
        match ready_rx.recv() {
            Ok(Some(hwnd)) => {
                *current = Slot::Shown(TrayThread { hwnd });
                Ok(ShowOutcome::Added)
            }
            Ok(None) | Err(_) => {
                // スレッド側が生成失敗を報告した（または異常終了した）。
                // この経路に到達した専用スレッドは既に自己後始末済みで存在し
                // ないため `Idle` のままでよい。
                QUEUE.end();
                Err(DeviceError::Internal("system_tray show"))
            }
        }
    }

    #[allow(unsafe_code)]
    fn hide(&self) -> Result<HideOutcome, DeviceError> {
        // ロックは Win32 呼び出しが終わるまで握ったままにする。`SendMessageW` は
        // 同期呼び出しのため、ここでロックを握っている間に別の呼び出しが同じ
        // `hwnd` へ送ることは無く、破棄済みハンドルへの送信競合は構造上起きない
        // （`device-bridge-contracts.md`「5. FFI 境界」）。`wnd_proc` 側は
        // `CURRENT` を一切ロックしないため、ここで握ったままでもデッドロックしない。
        let mut guard = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        let hwnd_raw = match &*guard {
            Slot::Shown(thread) => thread.hwnd,
            Slot::Idle => return Ok(HideOutcome::NotShown),
        };

        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        // SAFETY: `hwnd` は `show` が起こした専用スレッドが生存中に作ったウィンドウ
        // ハンドル。`SendMessageW` は同期的にそのスレッドの `WndProc` へ
        // `WM_TRAY_HIDE` を届け、戻り値がそのまま `NIM_DELETE` の成否
        // （1 = 成功、0 = 失敗）になる（モジュール doc 参照）。
        let result = unsafe { SendMessageW(hwnd, WM_TRAY_HIDE, Some(WPARAM(0)), Some(LPARAM(0))) };
        if result.0 != 1 {
            // 削除失敗。`WndProc` はこの場合ウィンドウを破棄せず維持するため、
            // `Shown` のまま残し（`guard` は未更新）、次回 `hide` が同じ `hwnd`
            // へ再試行できるようにする（`device-bridge-contracts.md`
            // 「9. 失敗時のリソース解放・共有状態復元」「6. 嘘の成功」）。
            return Err(DeviceError::Internal("system_tray hide"));
        }
        QUEUE.end();
        *guard = Slot::Idle;
        Ok(HideOutcome::Removed)
    }

    fn take_selection(&self) -> Option<usize> {
        QUEUE.pop()
    }
}

/// `Shell_NotifyIconW(NIM_DELETE)` を一過性失敗に備えて数回再試行する。
/// 成功すれば `true`、[`DELETE_RETRY_ATTEMPTS`] 回失敗したら `false`。
#[allow(unsafe_code)]
fn delete_icon_with_retry(hwnd: HWND) -> bool {
    for attempt in 0..DELETE_RETRY_ATTEMPTS {
        // SAFETY: `hwnd` は呼び出し元が所有する有効なウィンドウハンドル。
        if unsafe { Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd, "")) }.as_bool() {
            return true;
        }
        if attempt + 1 < DELETE_RETRY_ATTEMPTS {
            std::thread::sleep(DELETE_RETRY_DELAY);
        }
    }
    false
}

/// アイコン専用スレッドの本体。ウィンドウクラス登録 → ウィンドウ作成 →
/// `Shell_NotifyIconW(NIM_ADD)` → 成功を `ready_tx` へ通知 → メッセージループ、
/// の順で進む。失敗した各段階で確保済みのリソースを解放してから `ready_tx` へ
/// 失敗（`None`）を送る（`device-bridge-contracts.md`「9. 失敗時のリソース解放」）。
#[allow(unsafe_code)]
fn tray_thread_main(
    generation: u64,
    title: String,
    items: Vec<String>,
    ready_tx: std::sync::mpsc::SyncSender<Option<isize>>,
) {
    THREAD_ITEMS.with(|cell| *cell.borrow_mut() = items);
    THREAD_TITLE.with(|cell| *cell.borrow_mut() = title.clone());
    // Explorer が再起動すると通知領域のアイコンは全て消え、トップレベル
    // ウィンドウへ `TaskbarCreated` が送られる。受けたら登録し直す
    // （Shell_NotifyIcon の定番の作法、PR #1041 codex 指摘）。
    // SAFETY: 定数のワイド文字列を渡すのみ。
    let taskbar_created = unsafe { RegisterWindowMessageW(windows::core::w!("TaskbarCreated")) };
    TASKBAR_CREATED.with(|cell| cell.set(taskbar_created));

    let Some(hwnd) = create_hidden_window() else {
        let _ = ready_tx.send(None);
        return;
    };

    if !add_icon(hwnd, &title) {
        // SAFETY: `hwnd` はこの関数内で直前に作成し、他スレッドへ未公開。
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = ready_tx.send(None);
        return;
    }

    // 成功を通知した直後からメニュー選択が届き得るため、世代は通知より先に
    // 設定する（初期値 0 のまま `QUEUE.push` されて選択が捨てられるのを防ぐ、
    // PR #1041 codex 指摘）。
    CURRENT_GENERATION.with(|cell| cell.set(generation));
    if ready_tx.send(Some(hwnd.0 as isize)).is_err() {
        // `show` 側の `ready_rx.recv()` は期限無しで待つため、通常この経路には
        // 到達しない（呼び出し側がパニック等で既に居なくなった異常系のみ）。
        // それでもアイコン・ウィンドウを残してリークさせない
        // （`device-bridge-contracts.md`「9. 失敗時のリソース解放」）。
        if delete_icon_with_retry(hwnd) {
            // SAFETY: `hwnd` はこのスレッドが所有する有効なウィンドウハンドル。
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        return;
    }

    // メッセージループ。`WM_TRAY_HIDE`（`WndProc`）が `DestroyWindow` を呼ぶと
    // `WM_DESTROY` → `PostQuitMessage` 経由で `GetMessageW` が `0` を返しループを
    // 抜ける（generation は `WndProc` が `QUEUE` の世代照合に使うため
    // クロージャでは渡さず、`WndProc` が free function であることに合わせて
    // `thread_local` 経由で参照する）。
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` はこのスレッドのスタック上のローカル変数。`hwnd` フィルタは
        // 0（全ウィンドウ）ではなく自スレッドが所有する `hwnd` に限定しない
        // （`GetMessageW` の `hwnd` 引数は「このウィンドウ宛てのメッセージのみ」に
        // 絞るフィルタだが、スレッド専用ウィンドウのため絞らなくても安全）。
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        // SAFETY: `msg` は直前に `GetMessageW` が書き込んだ有効なメッセージ。
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

thread_local! {
    /// [`tray_thread_main`] がメッセージループ開始前に設定する現在の世代
    /// （`WndProc` が `QUEUE.push` の照合に使う）。
    static CURRENT_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Explorer 再起動時の再登録（`TaskbarCreated`）に使うタイトル。
    static THREAD_TITLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// `RegisterWindowMessageW("TaskbarCreated")` の戻り値（失敗時 0 ＝ 受けない）。
    static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Explorer 再起動後、`NIM_ADD`（再登録）と確認用の `NIM_MODIFY` が両方失敗し、
    /// 通知領域にアイコンが無いと確認できた状態（`wnd_proc` の `TaskbarCreated`
    /// 処理参照、P1-2 是正）。`NIM_ADD` のみの失敗（`NIM_MODIFY` は成功、アイコン
    /// 実在）では `true` にしない。`true` のときだけ `WM_TRAY_HIDE` は削除する
    /// ものが無いので成功扱いでウィンドウを畳む（`hide` → 再度 `show` で復旧
    /// できるようにする。PR #1041 codex 指摘）。
    static ICON_MISSING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// ウィンドウクラスを（未登録なら）登録し、`WS_EX_TOOLWINDOW` 付きの不可視
/// トップレベルウィンドウを作る。失敗時は `None`。
#[allow(unsafe_code)]
fn create_hidden_window() -> Option<HWND> {
    // SAFETY: 引数無しで現在のモジュールのハンドルを取得するのみ。
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;

    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: WINDOW_CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `wc` はこのスコープ内で生存するローカル変数で、`RegisterClassW` は
    // 呼び出し中のみ参照する。2 回目以降の `show`（`hide` 後の再表示）では同じ
    // クラス名が既に登録済みのため `RegisterClassW` は `0` を返し、直後の
    // `GetLastError` が `ERROR_CLASS_ALREADY_EXISTS` になる（Win32 の実際の
    // 挙動）。この場合は登録済みクラスをそのまま使ってよいため失敗として扱わない。
    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        // SAFETY: 直前の `RegisterClassW` 失敗に対する `GetLastError` 呼び出し。
        let already_exists = unsafe { GetLastError() } == ERROR_CLASS_ALREADY_EXISTS;
        if !already_exists {
            return None;
        }
    }

    // SAFETY: 全パラメータは固定値かローカル変数。`lpparam` は使わないため `None`。
    // `WS_VISIBLE` を付けないため実際には画面に出ない（モジュール doc参照）。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            WINDOW_CLASS_NAME,
            WINDOW_CLASS_NAME,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .ok()?;
    Some(hwnd)
}

/// `NOTIFYICONDATAW` を組み立てる（`hide` 時の `NIM_DELETE` は `szTip` を使わない
/// ため空文字列で足りる）。タイトルは `szTip`（128 `WCHAR`、NUL 終端）へ境界内で
/// コピーする（`device-bridge-contracts.md`「8. 秘密値」の番兵値対策と同種の
/// 境界検証、OWASP A03）。
fn icon_data(hwnd: HWND, tooltip: &str) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        // SAFETY: `None` ではなく実アイコンを安全に取得できなかった場合のみ
        // `HICON::default()`（空ハンドル）になる。`LoadIconW` 自体の呼び出しは
        // この関数の外（[`add_icon`]）で行い、ここでは組み立てのみを担う。
        ..Default::default()
    };
    write_wtext(&mut data.szTip, tooltip);
    data
}

/// 文字列を固定長 `[u16; N]`（NUL 終端）へ境界内でコピーする純関数。収まらない
/// 分は切り詰める（`TrayMenu::new` の文字数検証で実運用上は発生しない防御的
/// 処理）。
fn write_wtext<const N: usize>(dest: &mut [u16; N], text: &str) {
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let copy_len = encoded.len().min(N - 1);
    dest[..copy_len].copy_from_slice(&encoded[..copy_len]);
    dest[copy_len] = 0;
}

/// アイコンを追加・更新する（`message` は `NIM_ADD`/`NIM_MODIFY`）。失敗時は
/// `false`。`NIM_MODIFY` での呼び出しは [`wnd_proc`] の `TaskbarCreated` 処理が
/// 「`NIM_ADD` 失敗時にアイコンが実在するか」を確かめるのに使う（`set_icon` の
/// 呼び出し元参照、P1-2 是正）。
#[allow(unsafe_code)]
fn set_icon(hwnd: HWND, title: &str, message: NOTIFY_ICON_MESSAGE) -> bool {
    // SAFETY: `None` を渡すと既定のアプリケーションアイコンを返す（失敗しうるが
    // `ponytail`: 画像アイコンを要件化したら差し替える、`fandhe_apple_system_tray.m`
    // と同じ「テキストタイトルのみ」の最小実装に寄せる判断）。
    let Ok(icon) = (unsafe { LoadIconW(None, IDI_APPLICATION) }) else {
        return false;
    };
    let mut data = icon_data(hwnd, title);
    data.hIcon = icon;
    // SAFETY: `data` はこの呼び出し中のみ参照されるローカル変数。
    unsafe { Shell_NotifyIconW(message, &data) }.as_bool()
}

/// [`set_icon`] を `NIM_ADD` で呼ぶ薄いラッパー（既存呼び出し元の意図を名前で
/// 残す）。
fn add_icon(hwnd: HWND, title: &str) -> bool {
    set_icon(hwnd, title, NIM_ADD)
}

/// アイコン専用ウィンドウの `WndProc`。クロージャを持てない `extern "system" fn`
/// のため、[`THREAD_ITEMS`]/[`CURRENT_GENERATION`]（いずれも `thread_local`）
/// 経由で状態を参照する（`GWLP_USERDATA` への生ポインタ操作を避ける、
/// 実装計画 §3.5 参照）。`CURRENT`（`hide`/`show` が握るロック）は一切ロックしない
/// （`hide` が `SendMessageW` の間 `CURRENT` を握ったまま同期的に本関数の戻りを
/// 待つため、ここで `CURRENT` を取ろうとするとデッドロックする、モジュール doc
/// 「待ち時間切れを持たない理由」参照）。
#[allow(unsafe_code)]
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED.with(|cell| cell.get());
    if taskbar_created != 0 && msg == taskbar_created {
        let title = THREAD_TITLE.with(|cell| cell.borrow().clone());
        let added = add_icon(hwnd, &title);
        // `NIM_ADD` 失敗だけでは「アイコンが存在しない」と確定できない（既に
        // 同じ uID で存在するための失敗もあり得る）。`NIM_MODIFY`（同じ uID・
        // hwnd）を試し、それが成功すれば実在すると確認できる（`ICON_MISSING =
        // false`）。両方失敗したときのみ不在と確認できたとして `true` にする
        // （P1-2 是正、PR #1041 codex 指摘: 再登録失敗だけでアイコン削除を
        // 成功扱いしない）。
        let missing = !added && !set_icon(hwnd, &title, NIM_MODIFY);
        ICON_MISSING.with(|cell| cell.set(missing));
        return LRESULT(0);
    }
    match msg {
        WM_TRAY_CALLBACK => {
            let event = lparam.0 as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                show_popup_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_TRAY_HIDE => {
            // SAFETY: メニュー表示中に hide が来た場合に備え、先に `EndMenu` で
            // 開いているメニューを閉じてから後始末する（`device-bridge-contracts.md`
            // 「1. ライフサイクル境界」）。
            let _ = unsafe { EndMenu() };
            let data = icon_data(hwnd, "");
            // SAFETY: `hwnd` はこのスレッドが所有する有効なウィンドウ。
            // `NIM_DELETE` の結果は戻り値（`SendMessageW` 経由で `hide()` が
            // そのまま受け取る）に反映する（`device-bridge-contracts.md`
            // 「6. 嘘の成功」）。
            // `NIM_DELETE` は常に試す（`TaskbarCreated` は DPI 変更等でアイコンが
            // 残ったまま届くこともあり、そのとき `NIM_ADD` の失敗はアイコンが既に
            // あるためなので、ここで消さないと残骸が残る。Cursor Bugbot 指摘）。
            // 再登録に失敗してアイコンが本当に無い場合は削除の失敗を成功扱いにする
            // （`ICON_MISSING` doc 参照）。
            let deleted = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) }.as_bool();
            let icon_removed = deleted || ICON_MISSING.with(|cell| cell.get());
            if icon_removed {
                // SAFETY: `hwnd` はこのスレッドが所有する有効なウィンドウ。アイコン
                // 削除が確定した場合のみウィンドウを破棄してスレッドを終了させる。
                // `SendMessageW` は同期呼び出しのため、この `DestroyWindow` が
                // 完了してから `hide()` 側へ戻り値が返る（破棄済み `hwnd` を
                // 後から誰かが使う経路は無い）。
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            // 削除失敗時はウィンドウ・メッセージループを維持する（`hide()` の
            // 再試行先を残す、`device-bridge-contracts.md`「9. 失敗時のリソース
            // 解放」）。次回の `hide` は同じ `hwnd` へ `WM_TRAY_HIDE` を再送できる。
            LRESULT(if icon_removed { 1 } else { 0 })
        }
        WM_DESTROY => {
            // SAFETY: 引数無し、このスレッドのメッセージループを終了させるのみ。
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: 未処理メッセージは既定の処理へ委譲する（Win32 の契約どおり）。
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 右クリック/左クリックで呼ばれるポップアップメニューの組み立て・表示・後始末。
/// `device-bridge-contracts.md`「5. FFI 境界」の「UI を要する呼び出しは main
/// queue で」に相当する契約を、専用スレッド＝このメッセージループのスレッドで
/// 完結させることで満たす（マルチスレッド UI ではなく 1 スレッド 1 ウィンドウ）。
#[allow(unsafe_code)]
fn show_popup_menu(hwnd: HWND) {
    let items = THREAD_ITEMS.with(|cell| cell.borrow().clone());
    // SAFETY: 戻り値の `HMENU` はこの関数内でのみ使い、`DestroyMenu` で解放する。
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    for (index, label) in items.iter().enumerate() {
        let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` はこのループの呼び出し中のみ生存すればよい
        // （`AppendMenuW` は文字列をメニュー内部へコピーする契約）。
        let _ = unsafe { AppendMenuW(menu, MF_STRING, index + 1, PCWSTR(wide.as_ptr())) };
    }

    let mut point = Default::default();
    // SAFETY: `point` はこのスコープのローカル変数。
    let _ = unsafe { GetCursorPos(&mut point) };
    // SAFETY: `hwnd` はこのスレッドが所有するウィンドウ。`TPM_RETURNCMD` により
    // 選択された項目 id（`index + 1`）が戻り値として返る（0 は未選択）。
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let cmd = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        )
    };
    // SAFETY: メニューを閉じるために必要な定型呼び出し（`TrackPopupMenu` の
    // 戻り後、ポップアップメニューを閉じたことを確実にする Microsoft 推奨手順）。
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    // SAFETY: `menu` はこの関数内で生成したハンドルで、以後使わない。
    let _ = unsafe { DestroyMenu(menu) };

    if cmd.0 > 0 {
        let index = (cmd.0 - 1) as i64;
        let generation = CURRENT_GENERATION.with(|cell| cell.get());
        QUEUE.push(generation, index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_wtext_truncates_to_fit_and_nul_terminates() {
        let mut buf = [0u16; 4];
        write_wtext(&mut buf, "abcdef");
        assert_eq!(buf, ['a' as u16, 'b' as u16, 'c' as u16, 0]);
    }

    #[test]
    fn write_wtext_copies_short_text_and_nul_terminates() {
        let mut buf = [0u16; 4];
        write_wtext(&mut buf, "ab");
        assert_eq!(buf, ['a' as u16, 'b' as u16, 0, 0]);
    }
}
