//! Windows 共通層（TASK-21.4 / Issue #1184）。機能をまたいで使う Windows 固有の部品だけを置く。
//!
//! - メインウィンドウの HWND の保持: 起動側（`fandhe-gallery`）が [`remember_window`] で 1 度控え、
//!   各機能の Windows 実装が [`window`] で読む。1 プロセス 1 ウィンドウが前提
//! - COM 初期化（MTA）の RAII ガード [`ComInit`]: 機能ごとの作業スレッドで使う
//!
//! egui・eframe・winit・`fandhe-device` には依存しない（機能 crate の依存グラフに入れないため）。
//! Windows 以外ではクレート全体が空。`unsafe` は [`ComInit`] の COM 呼び出し 2 か所だけで、
//! クレートルートは `deny(unsafe_code)`。

#![cfg(target_os = "windows")]
#![deny(unsafe_code)]

use std::marker::PhantomData;
use std::sync::OnceLock;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// [`remember_window`] が控える HWND（`isize` で持つのは `HWND` が `Sync` でないため）。
static WINDOW: OnceLock<isize> = OnceLock::new();

/// 起動クロージャから構築時に 1 度だけ呼ぶ契約。HWND を取得できない場合
/// （`HandleError` またはウィンドウ以外のハンドル種別）は何もせず、[`window`] が `None` を返す。
/// effect ドライバは起動コンテキストに触れられないため、この static 経由でしか HWND を得られない。
pub fn remember_window(window: &impl HasWindowHandle) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let _ = WINDOW.set(h.hwnd.get());
    }
}

/// [`remember_window`] が控えたメインウィンドウの HWND。未登録なら `None`。
pub fn window() -> Option<HWND> {
    WINDOW.get().map(|&h| HWND(h as *mut std::ffi::c_void))
}

/// [`ComInit::new`] の失敗（`CoInitializeEx` が失敗）。呼び出し元が自分の契約上のエラーへ写像する
/// （共通層は `fandhe-device` に依存できないため、エラー型を持ち込まない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComInitFailed;

/// COM 初期化（MTA）の RAII ガード。呼び出しのたびに初期化し、作ったスレッドで drop して
/// `CoUninitialize` と対にする（`!Send` で型として守る）。`CoInitializeEx` が失敗したら `Err` を返し、
/// 後続の COM 操作を未初期化のまま続けさせない。
///
/// MTA 専用。winit が STA で初期化済みの UI スレッドでは `RPC_E_CHANGED_MODE` で失敗するため、
/// 専用の作業スレッドで使う。
#[must_use]
pub struct ComInit {
    _not_send: PhantomData<*const ()>,
}

impl ComInit {
    /// 呼び出しスレッドを MTA で COM 初期化する。
    #[allow(unsafe_code)]
    pub fn new() -> Result<Self, ComInitFailed> {
        // SAFETY: `CoInitializeEx` は呼び出しスレッドに対する初期化要求であり、
        // 失敗時（`RPC_E_CHANGED_MODE` 等）もプロセス状態を破壊しない。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            // 成功時（`S_OK`/`S_FALSE`）のみガードを作る。失敗時は作らないので
            // `CoUninitialize` も呼ばれない（二重 uninit を避ける）。
            Ok(Self {
                _not_send: PhantomData,
            })
        } else {
            Err(ComInitFailed)
        }
    }
}

impl Drop for ComInit {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: ガードは `new` 内の `CoInitializeEx` が成功したときだけ作られ、
        // `!Send` のため作ったスレッドで 1 度だけ drop される。
        unsafe { CoUninitialize() };
    }
}
