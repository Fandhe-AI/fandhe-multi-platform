//! WinRT 非同期操作の完了をワーカーのキューへ積む部品（契約表 11）。
//!
//! 直列キューのワーカー上で `.get()` により完了を待たず、`SetCompleted` のコールバックから命令をキューへ積む。
//! camera-preview-stream・video-capture など、WinRT の非同期操作を世代照合つきで進める機能が共用する。
//! 命令型 `C` は呼び出し側が決める（この部品は機能の型を知らない）。

use std::sync::mpsc::Sender;

use windows::core::{HRESULT, Interface, RuntimeType};
use windows_future::{
    AsyncActionCompletedHandler, AsyncOperationCompletedHandler, AsyncStatus, IAsyncAction,
    IAsyncInfo, IAsyncOperation,
};

/// `E_FAIL`（完了が成功以外で、OS が理由を返さないときの代用）。
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// `IAsyncOperation` の完了を `wrap` で命令（`C`）にしてキューへ積む。待たない。
pub fn spawn<C: Send + 'static, T: RuntimeType + 'static>(
    op: windows::core::Result<IAsyncOperation<T>>,
    tx: &Sender<C>,
    wrap: impl Fn(windows::core::Result<T>) -> C + Send + 'static,
) -> windows::core::Result<IAsyncInfo> {
    let op = op?;
    let tx = tx.clone();
    op.SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op.GetResults(),
            // 失敗・取り消しは成功にしない（理由が取れなければ E_FAIL）。
            (Ok(op), _) => Err(op
                .GetResults()
                .err()
                .unwrap_or_else(|| HRESULT(E_FAIL).into())),
            _ => Err(HRESULT(E_FAIL).into()),
        };
        // 受信側（ワーカー）が終わっていても無視してよい。
        let _ = tx.send(wrap(result));
        Ok(())
    }))?;
    op.cast()
}

/// `IAsyncAction` 版の [`spawn`]。
pub fn spawn_action<C: Send + 'static>(
    op: windows::core::Result<IAsyncAction>,
    tx: &Sender<C>,
    wrap: impl Fn(windows::core::Result<()>) -> C + Send + 'static,
) -> windows::core::Result<IAsyncInfo> {
    let op = op?;
    let tx = tx.clone();
    op.SetCompleted(&AsyncActionCompletedHandler::new(move |op, status| {
        let result = match (op.ok(), status) {
            (Ok(op), AsyncStatus::Completed) => op.GetResults(),
            (Ok(op), _) => Err(op
                .GetResults()
                .err()
                .unwrap_or_else(|| HRESULT(E_FAIL).into())),
            _ => Err(HRESULT(E_FAIL).into()),
        };
        let _ = tx.send(wrap(result));
        Ok(())
    }))?;
    op.cast()
}
