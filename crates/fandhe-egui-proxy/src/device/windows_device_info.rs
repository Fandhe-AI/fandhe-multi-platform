//! Windows 向け端末・OS 情報実装（Issue #1304）。`device/device_info.rs` の Windows 経路から呼ばれ、
//! Win32 を UI スレッドから同期で 1 回ずつ呼ぶ（package identity・管理者権限は不要）。契約は
//! `fandhe_device::device_info`。
//!
//! - OS の版: `RtlGetVersion`（`GetVersionEx` は manifest 次第で値をごまかすため使わない）
//! - 機種: レジストリ `HKLM\HARDWARE\DESCRIPTION\System\BIOS` の `SystemManufacturer` /
//!   `SystemProductName`（読み取りのみ）
//! - メモリ: `GlobalMemoryStatusEx` の `ullTotalPhys`
//! - 空き容量: OS のあるドライブ（`GetWindowsDirectoryW` の先頭 `X:\`）の
//!   `GetDiskFreeSpaceExW` の `lpFreeBytesAvailableToCaller`（呼び出し元が使える空き）
//!
//! 個体識別子（`MachineGuid` 等）は読まない。値の写像は純関数に切り出し、ホストでもテストする
//! （Windows でしか走らない写像テストを作らない、#1257 の反省）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-rtlgetversion>
//!
//! # 既知の天井
//!
//! - BIOS が出す仮の文字列（`To Be Filled By O.E.M.` 等）は判定せずそのまま出す
//! - 実機確認は #1301。ローカルに MSVC が無く、型検査は `x86_64-pc-windows-gnu` のみ

use fandhe_device::device_info::clean_text;

/// `RtlGetVersion` の版を `10.0.22631` の形にする。`major == 0` は取れなかったとみなして `None`。
fn version_text(major: u32, minor: u32, build: u32) -> Option<String> {
    (major != 0).then(|| format!("{major}.{minor}.{build}"))
}

/// 製造元と製品名を空白で連結する。片方だけでも、両方空なら `None`。
fn model_text(manufacturer: Option<&str>, product: Option<&str>) -> Option<String> {
    let parts: Vec<String> = [manufacturer, product]
        .into_iter()
        .flatten()
        .filter_map(clean_text)
        .collect();
    clean_text(&parts.join(" "))
}

/// NUL 終端の UTF-16 を文字列にする。NUL が無くても末尾までを採り、不正なサロゲートは `None`。
fn wide_to_string(w: &[u16]) -> Option<String> {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16(&w[..end]).ok()
}

/// `C:\Windows` のような文字列から `C:\` を取り出す。ドライブ文字で始まらなければ `None`。
fn drive_root(windows_dir: &str) -> Option<String> {
    let mut chars = windows_dir.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':')).then(|| format!("{letter}:\\"))
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win {
    use super::{drive_root, model_text, version_text, wide_to_string};
    use fandhe_device::device_info::{require_any, DeviceInfo, DeviceInfoSnapshot};
    use fandhe_device::DeviceError;
    use windows::core::{w, PCWSTR};
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    use windows::Win32::System::SystemInformation::{
        GetWindowsDirectoryW, GlobalMemoryStatusEx, MEMORYSTATUSEX, OSVERSIONINFOW,
    };

    /// レジストリ文字列の読み取り上限（バイト）。超える値は不明として扱う。
    const MAX_REG_BYTES: u32 = 1024;

    pub(super) struct WindowsDeviceInfo;

    fn os_version() -> Option<String> {
        let mut v = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // SAFETY: `v` は呼び出しの間有効なスタック上の構造体で、サイズフィールドを設定済み。
        // 関数は保持しない。
        let status = unsafe { RtlGetVersion(&mut v) };
        status
            .is_ok()
            .then(|| version_text(v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber))
            .flatten()
    }

    /// BIOS キー配下の文字列値を読む。サイズ問い合わせ → 確保 → 読み取りの 2 段。
    fn bios_value(name: PCWSTR) -> Option<String> {
        let key = w!("HARDWARE\\DESCRIPTION\\System\\BIOS");
        let mut bytes: u32 = 0;
        // SAFETY: `bytes` は有効な書き込み先。データ先は NULL でサイズだけを問い合わせる。
        let r = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut bytes),
            )
        };
        if r.0 != 0 || bytes == 0 || bytes > MAX_REG_BYTES {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
        let mut cap = (buf.len() * 2) as u32;
        // SAFETY: `buf` は `cap` バイト分の確保済み領域で、`cap` はその実サイズ。
        let r = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut cap),
            )
        };
        if r.0 != 0 {
            return None;
        }
        wide_to_string(&buf)
    }

    fn memory_bytes() -> Option<u64> {
        let mut m = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `m` は有効なスタック上の構造体で、`dwLength` を設定済み。関数は保持しない。
        unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
        Some(m.ullTotalPhys)
    }

    fn free_storage_bytes() -> Option<u64> {
        let mut dir = [0u16; 260];
        // SAFETY: `dir` は有効な可変スライスで、関数は長さを超えて書かない。
        let n = unsafe { GetWindowsDirectoryW(Some(&mut dir)) } as usize;
        if n == 0 || n > dir.len() {
            return None;
        }
        let root = drive_root(&wide_to_string(&dir[..n])?)?;
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        // SAFETY: `wide` は NUL 終端で呼び出しの間生存し、`free` は有効な書き込み先。
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
        Some(free)
    }

    impl DeviceInfo for WindowsDeviceInfo {
        fn read(&self) -> Result<DeviceInfoSnapshot, DeviceError> {
            let manufacturer = bios_value(w!("SystemManufacturer"));
            let product = bios_value(w!("SystemProductName"));
            require_any(DeviceInfoSnapshot {
                os_name: Some("Windows".to_string()),
                os_version: os_version(),
                model: model_text(manufacturer.as_deref(), product.as_deref()),
                memory_bytes: memory_bytes(),
                free_storage_bytes: free_storage_bytes(),
            })
        }
    }
}

/// Windows の `DeviceInfo` 実装を返す（`device/device_info.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::device_info::DeviceInfo> {
    Box::new(win::WindowsDeviceInfo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_zero_major_is_none() {
        assert_eq!(version_text(0, 0, 0), None);
        assert_eq!(version_text(10, 0, 22631).as_deref(), Some("10.0.22631"));
    }

    #[test]
    fn model_joins_present_parts() {
        assert_eq!(
            model_text(Some("Dell Inc."), Some("XPS 13")).as_deref(),
            Some("Dell Inc. XPS 13")
        );
        assert_eq!(model_text(None, Some("XPS")).as_deref(), Some("XPS"));
        assert_eq!(model_text(Some(" "), Some("")), None);
        assert_eq!(model_text(None, None), None);
        assert_eq!(model_text(Some("A\u{7}"), None).as_deref(), Some("A"));
    }

    #[test]
    fn wide_stops_at_nul_and_rejects_bad_surrogates() {
        let w: Vec<u16> = "ab\0cd".encode_utf16().collect();
        assert_eq!(wide_to_string(&w).as_deref(), Some("ab"));
        let no_nul: Vec<u16> = "ab".encode_utf16().collect();
        assert_eq!(wide_to_string(&no_nul).as_deref(), Some("ab"));
        assert_eq!(wide_to_string(&[0xD800, 0x41]), None);
    }

    #[test]
    fn drive_root_needs_drive_letter() {
        assert_eq!(drive_root("C:\\Windows").as_deref(), Some("C:\\"));
        assert_eq!(drive_root("\\\\server\\share"), None);
        assert_eq!(drive_root("1:\\x"), None);
        assert_eq!(drive_root(""), None);
    }
}
