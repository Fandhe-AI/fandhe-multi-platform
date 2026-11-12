//! Windows 向け無操作時間実装（Issue #631）。`driver` の Windows 経路から呼ばれ、
//! Win32 `GetLastInputInfo` と `GetTickCount` を UI スレッドから同期で呼ぶ（数 µs。package identity
//! 不要）。`UserIdle` の契約は `crate::contract`。
//!
//! `LASTINPUTINFO.dwTime` は 32 bit の tick なので、`GetTickCount`（32 bit）と比べる
//! （`GetTickCount64` とは比べられない）。先に `GetLastInputInfo` を呼び、その後で
//! `GetTickCount` を呼ぶことで通常は now >= `dwTime` になる。折り返し・未来の tick の扱いは
//! 純関数 `crate::contract::idle_from_ticks` に任せ、ホストでテストする。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getlastinputinfo>
//!
//! # 既知の天井
//!
//! - 呼び出したセッション内の入力だけ（別セッション・RDP は守らない範囲）
//! - 実機確認は #627。ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

#[cfg(target_os = "windows")]
struct WindowsUserIdle;

#[cfg(target_os = "windows")]
impl crate::contract::UserIdle for WindowsUserIdle {
    #[allow(unsafe_code)]
    fn idle_for(&self) -> Result<std::time::Duration, crate::error::UserIdleError> {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` は呼び出しの間有効なスタック上の構造体で、`cbSize` を設定済み。
        // 排他的な書き込み先として渡し、関数は保持しない。
        let ok = unsafe { GetLastInputInfo(&mut info) }.as_bool();
        if !ok {
            return Err(crate::error::UserIdleError::Internal);
        }
        // SAFETY: 引数なしで状態を持たない読み取り。
        let now = unsafe { GetTickCount() };
        crate::contract::idle_from_ticks(now, info.dwTime)
    }
}

/// Windows の `UserIdle` 実装を返す（`driver` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn crate::contract::UserIdle> {
    Box::new(WindowsUserIdle)
}
