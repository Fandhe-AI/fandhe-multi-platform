//! Windows 向け生体認証実装（Issue #907）。`device/biometric_auth.rs` の Windows 経路
//! から呼ばれ、WinRT `UserConsentVerifier`（Windows Hello）を
//! `IUserConsentVerifierInterop`（`Win32_System_WinRT`）経由でウィンドウ束縛して呼ぶ
//! （`windows_share.rs` と同じ「子プロセス方式は自分のウィンドウを持てず使えないため、
//! FFI・`unsafe` を持てる本クレート側から直接 `windows` クレートを叩く」判断、
//! `lib.rs` の Windows `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠）。
//!
//! `unsafe` は [`present`] 内の `RequestVerificationForWindowAsync` 呼び出し 1 箇所に
//! 閉じる（`// SAFETY:` を呼び出し直前に付与）。
//!
//! 認証理由・`HRESULT`・`UserConsentVerificationResult` の生値をログへ出力しない
//! （security.md A09）。`DeviceError::Internal` は固定ラベルのみを持つ
//! （`windows_share.rs` と同じ方針）。
//!
//! # 完了の受け取り方（`IAsyncOperation`、コールバック型）
//!
//! `RequestVerificationForWindowAsync` は `IAsyncOperation<UserConsentVerificationResult>`
//! を返す非同期 API で、Windows Hello の認証ダイアログは別プロセスの UI のため
//! `.get()` で同期待ちせず `SetCompleted` にハンドラを登録する
//! （UI スレッドをブロックしない）。完了はスレッドプール側スレッドで届くため、結果は
//! `biometric_auth::bridge::deliver`（`Mutex` 保護）へ置くだけにし、画面への反映は
//! `biometric_auth::drive_callback` の毎フレームポーリングに任せる
//! （`device/share.rs` の `.m` 側コールバックと同じ構造）。
//!
//! `IAsyncOperation` は完了ハンドラが呼ばれる前にドロップされないよう、
//! スレッドローカルの [`HELD`] へ保持する（次回 [`present`] で前回分を置き換える）。
//!
//! `CheckAvailabilityAsync` は別途呼ばない（実装計画の設計判断 3 参照）:
//! `UserConsentVerificationResult` 自体が `DeviceNotPresent`/`NotConfiguredForUser`/
//! `DisabledByPolicy`/`DeviceBusy` を返すため可否判定はそこで吸収できる。
//!
//! `Verified`（`outcome_from_verification` の code 0）は生体一致に限らず PIN 等の
//! Windows Hello 端末所有者認証でも返る既知の制約（`biometric_auth::outcome_from_verification`
//! の doc 参照）。

use std::cell::RefCell;

use windows::core::HSTRING;
use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
// `windows` 0.61.3 は `IAsyncOperation`/`AsyncOperationCompletedHandler`/`AsyncStatus` を
// `windows::Foundation` へ再エクスポートしていない（`windows-future` 側にのみ存在）ため、
// ルート Cargo.toml の直接依存から取る（同モジュールのコメント参照）。
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus, IAsyncOperation};

use fandhe_device::DeviceError;

use super::biometric_auth::{bridge, outcome_from_verification};

thread_local! {
    /// 直近の `present` が発行した `IAsyncOperation`（完了ハンドラが呼ばれる前に
    /// ドロップされないよう保持する。次回 `present` で置き換わる、
    /// `windows_share.rs::HELD` と同じスレッドローカル方針: `present` は
    /// `biometric_auth::drive_callback` から egui の update ループ（UI/メインスレッド）
    /// 上でのみ呼ばれる契約のため `Mutex` は不要）。
    static HELD: RefCell<Option<IAsyncOperation<UserConsentVerificationResult>>> =
        const { RefCell::new(None) };
}

/// `device/biometric_auth.rs` の Windows 経路から呼ばれる。`request_id` は
/// `biometric_auth::bridge::begin` が払い出したプロセス内一意な ID
/// （`bridge::deliver` の照合キー）。同期的に失敗した場合は `Err` を返し、
/// 呼び出し元が `bridge::deliver` する（完了ハンドラは二度と呼ばれないため）。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, reason: &str) -> Result<(), DeviceError> {
    let hwnd = super::windows_share::window().ok_or(DeviceError::Internal("biometric window"))?;
    let interop: IUserConsentVerifierInterop =
        windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|_| DeviceError::Internal("biometric factory"))?;

    // SAFETY: `hwnd` は `windows_share::window()`（`remember_window` が
    // `eframe::CreationContext` から得た HWND、`ProxyApp` の生存中はウィンドウも
    // 生存する）を経由した有効なハンドルで、`interop` は上で取得した有効な COM
    // ポインタ（`windows_share::present` と同じ安全性根拠）。
    let op: IAsyncOperation<UserConsentVerificationResult> =
        unsafe { interop.RequestVerificationForWindowAsync(hwnd, &HSTRING::from(reason)) }
            .map_err(|_| DeviceError::Internal("biometric present"))?;

    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op
                .GetResults()
                .map_err(|_| DeviceError::Internal("biometric outcome"))
                .and_then(|r: UserConsentVerificationResult| outcome_from_verification(r.0)),
            // キャンセル・エラー・未知のステータスはすべて失敗側へ倒す
            // （偽の成功を作らない、モジュール doc 参照）。
            _ => Err(DeviceError::Internal("biometric outcome")),
        };
        bridge::deliver(request_id, result);
        Ok(())
    }))
    .map_err(|_| DeviceError::Internal("biometric completed"))?;

    HELD.with_borrow_mut(|held| *held = Some(op));
    Ok(())
}
