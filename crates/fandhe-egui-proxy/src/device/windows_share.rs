//! Windows 向け共有シート実装（Issue #898）。`device/share.rs` の Windows 経路から
//! 呼ばれ、WinRT `DataTransferManager` を `IDataTransferManagerInterop`
//! （`Win32_UI_Shell`）経由でウィンドウ束縛して呼ぶ。`tts`/`file_picker`/`location`
//! の Windows 実装（PowerShell 子プロセス方式）は使わない: 子プロセスは自分の
//! ウィンドウを持てず `ShowShareUIForWindow` に渡す HWND が無いため、FFI・`unsafe` を
//! 持てる本クレート（プラットフォーム最適化レイヤ）側から直接 `windows` クレートを叩く
//! （`lib.rs` の Windows `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠）。
//!
//! `unsafe` は [`present`] 内の `GetForWindow` / `ShowShareUIForWindow` 呼び出し 2 箇所に
//! 閉じる（`// SAFETY:` を各呼び出し直前に付与）。それ以外の WinRT 呼び出し
//! （`DataRequested` / `TargetApplicationChosen` の登録、`DataPackage` への書き込み）は
//! `windows` クレート側で safe fn としてラップ済みのため `unsafe` を要しない。
//!
//! 共有テキスト・URL をログへ出力しない（security.md A09）。HRESULT の失敗も
//! `DeviceError::Internal("share ...")` の固定ラベルへ畳み、内部構造を漏らさない。
//!
//! # ウィンドウの捕捉
//!
//! `remember_window` は `ProxyApp::with_source`（`app.rs`）から構築時に 1 度だけ呼ばれ、
//! `eframe::CreationContext`（`HasWindowHandle` 実装）から得た HWND を static へ控える。
//! effect ドライバ（`device/share.rs`）は `CreationContext` に触れられないため、この
//! static 経由でしか HWND を得られない。
//!
//! # キャンセル検知の天井
//!
//! `DataTransferManager` には「共有 UI を閉じた」イベントが無い（公知の制約）。共有 UI は
//! フォーカスを奪う別ウィンドウのため、`device/share.rs` 側が「フォーカス喪失 → 復帰後も
//! `TargetApplicationChosen` 未着」をキャンセルとみなすヒューリスティックで補う
//! （`share.rs` のポーリング処理を参照。本モジュールは `TargetApplicationChosen` の
//! 到着だけを `bridge::deliver` へ伝える）。

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::OnceLock;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{Ref, HSTRING};
use windows::ApplicationModel::DataTransfer::{
    DataRequestedEventArgs, DataTransferManager, TargetApplicationChosenEventArgs,
};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;

use fandhe_device::share::ShareRequest;
use fandhe_device::DeviceError;

/// [`remember_window`] が控える HWND（プロセス内で 1 ウィンドウのみを前提にする、
/// `ProxyApp` は 1 プロセス 1 ウィンドウの検証用プロキシのため）。
static WINDOW: OnceLock<isize> = OnceLock::new();

/// 直近の `present` が登録したハンドラ一式。次回 `present` 前に解除しないと、
/// `DataTransferManager` は hwnd ごとに同一インスタンスが返るため登録が累積する。
struct Held {
    manager: DataTransferManager,
    data_requested: i64,
    target_chosen: i64,
}

// `DataTransferManager`（COM オブジェクト）は `Send`/`Sync` を実装しないため
// `static Mutex` に置けない。`present` は `device/share.rs` の effect ドライバから
// egui の update ループ（UI/メインスレッド）上でのみ呼ばれる契約（`GetForWindow` /
// `ShowShareUIForWindow` も同じスレッドで呼ぶ必要がある WinRT の STA 前提）のため、
// スレッドローカルで足りる（`Mutex` のような他スレッドとの排他は不要）。
thread_local! {
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
}

/// `ProxyApp::with_source`（`app.rs`）から構築時に 1 度だけ呼ばれる契約。HWND を
/// 取得できない場合（`HandleError` またはウィンドウ以外のハンドル種別）は何もせず、
/// [`present`] 側が「ウィンドウ不在」として `DeviceError::Internal` へ畳む。
pub(crate) fn remember_window(cc: &eframe::CreationContext<'_>) {
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    if let RawWindowHandle::Win32(h) = handle.as_raw() {
        let _ = WINDOW.set(h.hwnd.get());
    }
}

/// [`remember_window`] が控えた HWND を返す（未取得なら `None`）。Issue #907:
/// `device/windows_biometric.rs::present` も同じ HWND を使うため（Windows Hello の
/// 認証ダイアログをメインウィンドウへ束縛する）、`WINDOW` static へのアクセスを
/// ここへ一本化する。
pub(crate) fn window() -> Option<HWND> {
    WINDOW.get().map(|&raw| HWND(raw as *mut c_void))
}

/// `device/share.rs` の Windows 経路から呼ばれる。`request_id` は
/// `device/share::bridge::begin` が払い出したプロセス内一意な ID（`bridge::deliver` の
/// 照合キー）。同期的に失敗した場合は `Err` を返し、呼び出し元が `bridge::deliver` する
/// （`TargetApplicationChosen` は二度と来ないため）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, request: &ShareRequest) -> Result<(), DeviceError> {
    if !matches!(DataTransferManager::IsSupported(), Ok(true)) {
        return Err(DeviceError::Unsupported("share"));
    }
    let Some(hwnd) = window() else {
        return Err(DeviceError::Internal("share window"));
    };

    let interop: IDataTransferManagerInterop =
        windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()
            .map_err(|_| DeviceError::Internal("share factory"))?;

    // 前回の登録を解除する（累積防止。失敗しても提示自体は続行する: 前回のマネージャは
    // 今回取得するものと同一インスタンスのはずだが、万一取得できていなくても
    // 新規登録自体は成立するため致命的ではない）。
    if let Some(held) = HELD.with_borrow_mut(Option::take) {
        let _ = held.manager.RemoveDataRequested(held.data_requested);
        let _ = held
            .manager
            .RemoveTargetApplicationChosen(held.target_chosen);
    }

    // SAFETY: `hwnd` は `remember_window` が `eframe::CreationContext`（`ProxyApp`
    // 構築時、winit が生成したウィンドウ）から得た HWND で、`ProxyApp` の生存中は
    // ウィンドウも生存する（`update` から呼ばれる effect の実行中に閉じられない）。
    // `interop` は上で取得した有効な COM ポインタ。
    let manager: DataTransferManager = unsafe { interop.GetForWindow(hwnd) }
        .map_err(|_| DeviceError::Internal("share get manager"))?;

    let text = request.text().to_string();
    let url = request.url().map(str::to_string);
    let data_requested = manager
        .DataRequested(&TypedEventHandler::new(
            move |_: Ref<'_, DataTransferManager>, args: Ref<'_, DataRequestedEventArgs>| {
                let request = args.ok()?.Request()?;
                // データ詰め込みの失敗（例: 不正な URL の `Uri::CreateUri`）はイベント
                // 呼び出し元にしか伝わらず `bridge::deliver` に届かないため、ここで
                // 捕捉して DataRequest 側（FailWithDisplayText）と画面側（bridge::deliver）
                // の双方へ明示的に通知する（P1 修正）。
                let filled: windows::core::Result<()> = (|| {
                    // URL の妥当性は `package.Data()` を呼ぶ前に検証する。WinRT は
                    // `Data()` でパッケージへ書き込んだ後の `FailWithDisplayText` を
                    // 無視するため（システム UI に共有可能なパッケージが残ったまま
                    // になる）、`Uri::CreateUri` の失敗をここで先に弾いてから
                    // パッケージへ触れる（Bugbot 指摘対応）。
                    let weblink = url
                        .as_ref()
                        .map(|url| Uri::CreateUri(&HSTRING::from(url)))
                        .transpose()?;
                    let package = request.Data()?;
                    // Title は必須（未設定だと共有 UI がエラーになる）。共有テキストを
                    // Title に流用しない（固定文字列、Windows のみのアプリ表示名）。
                    package.Properties()?.SetTitle(&HSTRING::from("fandhe"))?;
                    package.SetText(&HSTRING::from(&text))?;
                    if let Some(weblink) = weblink {
                        package.SetWebLink(&weblink)?;
                    }
                    Ok(())
                })();
                if filled.is_err() {
                    let _ = request
                        .FailWithDisplayText(&HSTRING::from("共有データの準備に失敗しました"));
                    super::share::bridge::deliver(
                        request_id,
                        Err(DeviceError::Internal("share data requested")),
                    );
                }
                Ok(())
            },
        ))
        .map_err(|_| DeviceError::Internal("share data requested"))?;

    let target_chosen = match manager.TargetApplicationChosen(&TypedEventHandler::new(
        move |_: Ref<'_, DataTransferManager>, _: Ref<'_, TargetApplicationChosenEventArgs>| {
            super::share::bridge::deliver(
                request_id,
                Ok(fandhe_device::share::ShareOutcome::Shared),
            );
            Ok(())
        },
    )) {
        Ok(token) => token,
        Err(_) => {
            // 登録の部分失敗をロールバックする: 先に成功した DataRequested を解除
            // しないと、次回 present で HELD に何も残らないまま古いハンドラが
            // DataTransferManager に残存し、次回の共有で二重発火する（P2 / Bugbot 指摘）。
            let _ = manager.RemoveDataRequested(data_requested);
            return Err(DeviceError::Internal("share target chosen"));
        }
    };

    HELD.with_borrow_mut(|held| {
        *held = Some(Held {
            manager,
            data_requested,
            target_chosen,
        });
    });

    // SAFETY: `hwnd` の生存根拠は上と同じ。`interop` も同一の有効な COM ポインタ。
    unsafe { interop.ShowShareUIForWindow(hwnd) }
        .map_err(|_| DeviceError::Internal("share present"))
}
