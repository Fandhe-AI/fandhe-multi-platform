//! macOS 向けログイン時の自動起動の実装（Issue #696）。`macos/fandhe_macos_launch_at_login.m`
//! （`SMAppService.mainApp`、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルし ServiceManagement をリンクする）を `unsafe extern "C"` で呼ぶ。`unsafe` は
//! `mod ffi` に閉じ `// SAFETY:` で契約を明記する。`device/launch_at_login.rs` の macOS 経路から
//! 呼ばれ、契約は `fandhe_device::launch_at_login`。
//!
//! 結果コード・状態の生値の写像（[`status_from_raw`]・[`result_from_code`]）はホストでも
//! テストできるようモジュールは `cfg(any(macos, test))`。未知の値は成功を作らない。
//!
//! # 既知の天井
//!
//! - ad-hoc 署名の .app で登録できるか、`requiresApproval` の出方は未確認（Mac 目視は #694）
//! - OS 設定で変えた状態は「状態を読み直す」まで画面に出ない（守らない範囲）

#[cfg(any(target_os = "macos", test))]
use fandhe_device::launch_at_login::LoginItemStatus;
#[cfg(any(target_os = "macos", test))]
use fandhe_device::DeviceError;

/// `SMAppServiceStatus` の生値 → 状態（SSOT）。`notFound`（3）と未知の値は `Unknown`。
#[cfg(any(target_os = "macos", test))]
pub(crate) fn status_from_raw(raw: i32) -> LoginItemStatus {
    match raw {
        0 => LoginItemStatus::NotRegistered,
        1 => LoginItemStatus::Enabled,
        2 => LoginItemStatus::RequiresApproval,
        _ => LoginItemStatus::Unknown,
    }
}

/// `.m` の戻りコード → 結果の写像（SSOT）。未知のコードは失敗に倒し成功を作らない。
#[cfg(any(target_os = "macos", test))]
pub(crate) fn result_from_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 | 2 => Err(DeviceError::Unsupported("launch_at_login")),
        3 => Err(DeviceError::PermissionDenied("launch_at_login")),
        4 => Err(DeviceError::Internal("launch_at_login signature")),
        _ => Err(DeviceError::Internal("launch_at_login")),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_launch_at_login.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_launch_at_login_status(out_status: *mut i32) -> i32;
        fn fandhe_macos_launch_at_login_enable(out_status: *mut i32) -> i32;
        fn fandhe_macos_launch_at_login_disable(out_status: *mut i32) -> i32;
        fn fandhe_macos_launch_at_login_open_settings() -> i32;
    }

    /// `(戻りコード, status 生値)`。生値はコード 0 のときだけ意味を持つ。
    fn call(f: unsafe extern "C" fn(*mut i32) -> i32) -> (i32, i32) {
        let mut raw: i32 = -1;
        // SAFETY: `raw` はこの関数のローカル `i32` への有効な書き込み可能ポインタで、呼び出しの
        // 間だけ生きる。`.m` は書き込むだけでポインタを保持しない。
        #[allow(unsafe_code)]
        let code = unsafe { f(&mut raw) };
        (code, raw)
    }

    pub(super) fn status() -> (i32, i32) {
        call(fandhe_macos_launch_at_login_status)
    }
    pub(super) fn enable() -> (i32, i32) {
        call(fandhe_macos_launch_at_login_enable)
    }
    pub(super) fn disable() -> (i32, i32) {
        call(fandhe_macos_launch_at_login_disable)
    }
    #[allow(unsafe_code)]
    pub(super) fn open_settings() -> i32 {
        // SAFETY: 引数なしの C 関数で、ポインタを受け取らない。
        unsafe { fandhe_macos_launch_at_login_open_settings() }
    }
}

#[cfg(target_os = "macos")]
struct MacLaunchAtLogin;

#[cfg(target_os = "macos")]
fn read((code, raw): (i32, i32)) -> Result<LoginItemStatus, DeviceError> {
    result_from_code(code).map(|()| status_from_raw(raw))
}

#[cfg(target_os = "macos")]
impl fandhe_device::launch_at_login::LaunchAtLogin for MacLaunchAtLogin {
    fn status(&self) -> Result<LoginItemStatus, DeviceError> {
        read(ffi::status())
    }
    fn enable(&self) -> Result<LoginItemStatus, DeviceError> {
        read(ffi::enable())
    }
    fn disable(&self) -> Result<LoginItemStatus, DeviceError> {
        read(ffi::disable())
    }
    fn open_settings(&self) -> Result<(), DeviceError> {
        result_from_code(ffi::open_settings())
    }
}

/// macOS の `LaunchAtLogin` 実装を返す（`device/launch_at_login.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::launch_at_login::LaunchAtLogin> {
    Box::new(MacLaunchAtLogin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_status_table() {
        assert_eq!(status_from_raw(0), LoginItemStatus::NotRegistered);
        assert_eq!(status_from_raw(1), LoginItemStatus::Enabled);
        assert_eq!(status_from_raw(2), LoginItemStatus::RequiresApproval);
        for raw in [3, -1, 99, i32::MAX] {
            assert_eq!(status_from_raw(raw), LoginItemStatus::Unknown);
        }
    }

    #[test]
    fn code_table() {
        assert!(result_from_code(0).is_ok());
        for code in [1, 2] {
            assert!(matches!(
                result_from_code(code),
                Err(DeviceError::Unsupported("launch_at_login"))
            ));
        }
        assert!(matches!(
            result_from_code(3),
            Err(DeviceError::PermissionDenied("launch_at_login"))
        ));
        assert!(matches!(
            result_from_code(4),
            Err(DeviceError::Internal("launch_at_login signature"))
        ));
        for code in [5, -1, 99, i32::MAX] {
            assert!(matches!(
                result_from_code(code),
                Err(DeviceError::Internal("launch_at_login"))
            ));
        }
    }
}
