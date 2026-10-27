//! macOS / iOS 共用のスキャナー・カメラ機器からの取り込み（Issue #981 macOS、iOS は #982）。
//! `device/scanner_devices.rs` の macOS 経路（iOS は `ios_scanner_devices` 経由）から呼ばれ、
//! `apple/fandhe_apple_scanner_devices.m`（ImageCaptureCore で USB 接続のカメラ機器を探し、最新の
//! 画像 1 枚を一時ディレクトリへ取り込んで ImageIO で長辺 2048 画素以下の JPEG にする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。バイト列は `.m` が戻る直前に呼び出しスレッド自身が
//! コールバックで 1 回だけ渡す。期限切れ・失敗の後に Rust の領域へは書かせない。`unsafe` は
//! この呼び出しとコールバックの 2 箇所に閉じる（coding-rust.md の FFI 例外規定準拠）。
//!
//! 戻りコードの写像の SSOT は [`failure_for`]（純関数。ホストの `cargo test` で全コードを検査する）。
//! 契約の SSOT は `fandhe_device::scanner_devices`: 成功は必ず `ImportedImage::from_os` を通し、
//! `NSError.code` は握りつぶさず（`-1` へ潰さず）契約の失敗種別へ写す。呼び出しはブロックするため、
//! 呼び出し元はワーカーで呼ぶ。`build.rs` のコンパイル条件・`device.rs` の mod cfg と同じ OS 集合に保つ。

use fandhe_device::scanner_devices::ScannerImportFailure;

/// `.m` の戻りコード 9（OS の失敗）のとき、`NSError.code`（ImageCaptureCore の `ICReturnCode` 等）を
/// 契約の失敗種別へ写す。未知のコードと 0（欠けた要素など OS のコードが無い失敗）は `Failed`。
fn failure_for_os_code(os_code: i64) -> ScannerImportFailure {
    match os_code {
        // ICReturnDeviceIsPasscodeLocked / ICReturnDeviceNeedsCredentials
        -9943 | -9953 => ScannerImportFailure::DeviceLocked,
        // ICReturnConnectionNotAuthorizedToOpenDevice / ICReturnPTPNotAuthorizedToSendCommand
        -21343 | -21249 => ScannerImportFailure::PermissionDenied,
        // ICReturnConnectionDriverExited / ClosedSessionSuddenly / EjectedSuddenly
        -21350..=-21348 => ScannerImportFailure::Interrupted,
        // ICReturnCommunicationTimedOut
        -9923 => ScannerImportFailure::TimedOut,
        _ => ScannerImportFailure::Failed,
    }
}

/// `.m` の 0 以外の戻りコードを契約の失敗種別へ写す。未知のコードは `Failed`。
fn failure_for(code: i32, os_code: i64) -> ScannerImportFailure {
    match code {
        1 => ScannerImportFailure::Unsupported,
        2 => ScannerImportFailure::NoDevice,
        3 => ScannerImportFailure::NoImage,
        4 => ScannerImportFailure::DeviceLocked,
        5 => ScannerImportFailure::PermissionDenied,
        6 => ScannerImportFailure::TimedOut,
        7 => ScannerImportFailure::Interrupted,
        9 => failure_for_os_code(os_code),
        10 => ScannerImportFailure::TooLarge,
        11 => ScannerImportFailure::Undecodable,
        // 8（main スレッドからの呼び出し）と未知のコード。
        _ => ScannerImportFailure::Failed,
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod os {
    use std::ffi::c_void;
    use std::time::Duration;

    use fandhe_device::scanner_devices::{
        DeviceKind, ImportedImage, ScannerDevices, ScannerImportFailure, DISCOVERY_WINDOW,
        MAX_PAGE_BYTES,
    };

    use super::failure_for;

    // SAFETY: `apple/fandhe_apple_scanner_devices.m` が定義する C ABI（`build.rs` が macOS / iOS
    // ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は `.m` 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_apple_scanner_devices_import(
            deadline_ms: u64,
            discovery_ms: u64,
            max_bytes: u64,
            ctx: *mut c_void,
            on_bytes: extern "C" fn(*mut c_void, *const u8, usize),
            out_kind: *mut i32,
            out_found: *mut u32,
            out_os_code: *mut i64,
        ) -> i32;
    }

    /// `.m` から同期に 1 回だけ呼ばれ、バイト列を `ctx`（`&mut Vec<u8>`）へ複製する。
    /// panic しない処理だけにする（unwind を FFI 越しに伝えない）。
    #[allow(unsafe_code)]
    extern "C" fn push_bytes(ctx: *mut c_void, bytes: *const u8, len: usize) {
        // SAFETY: `ctx` は `import_one` が渡した `&mut Vec<u8>` で、`.m` は呼び出しの間だけ同期に使い
        // 保持しない。`bytes` は `len` バイトの読み取り専用領域で、このコールバックの間だけ有効。
        let (out, src) = unsafe {
            (
                &mut *(ctx as *mut Vec<u8>),
                std::slice::from_raw_parts(bytes, len),
            )
        };
        out.extend_from_slice(src);
    }

    /// ImageCaptureCore（USB 接続のカメラ機器）で取り込む実装。
    pub(crate) struct AppleScannerDevices;

    impl ScannerDevices for AppleScannerDevices {
        #[allow(unsafe_code)]
        fn import_one(&self, remaining: Duration) -> Result<ImportedImage, ScannerImportFailure> {
            // 残りが無いのに OS を起こさない。
            if remaining.is_zero() {
                return Err(ScannerImportFailure::TimedOut);
            }
            let deadline_ms = u64::try_from(remaining.as_millis())
                .unwrap_or(u64::MAX)
                .max(1);
            let discovery_ms = u64::try_from(remaining.min(DISCOVERY_WINDOW).as_millis())
                .unwrap_or(u64::MAX)
                .max(1);
            let mut bytes: Vec<u8> = Vec::new();
            let (mut kind, mut found, mut os_code) = (0i32, 0u32, 0i64);
            // SAFETY: `bytes` への可変参照はこの呼び出しの間だけ `ctx` として渡し、`.m` 側は戻る前に
            // 使い終えて保持しない。出力引数 3 つはこのスタックの有効な書き込み先で、呼び出しの間だけ
            // 使われる。`.m` は期限切れ・失敗の後に `ctx` へ書かない。
            let code = unsafe {
                fandhe_apple_scanner_devices_import(
                    deadline_ms,
                    discovery_ms,
                    MAX_PAGE_BYTES as u64,
                    &mut bytes as *mut Vec<u8> as *mut c_void,
                    push_bytes,
                    &mut kind,
                    &mut found,
                    &mut os_code,
                )
            };
            if code != 0 {
                return Err(failure_for(code, os_code));
            }
            let kind = if kind == 0 {
                DeviceKind::Camera
            } else {
                DeviceKind::Scanner
            };
            ImportedImage::from_os(bytes, kind, found)
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[cfg_attr(target_os = "ios", allow(unused_imports))] // iOS は #982 が使う
pub(crate) use os::AppleScannerDevices;

#[cfg(test)]
mod tests {
    use super::*;
    use ScannerImportFailure as F;

    #[test]
    fn plain_codes_map_one_to_one() {
        let cases = [
            (1, F::Unsupported),
            (2, F::NoDevice),
            (3, F::NoImage),
            (4, F::DeviceLocked),
            (5, F::PermissionDenied),
            (6, F::TimedOut),
            (7, F::Interrupted),
            (10, F::TooLarge),
            (11, F::Undecodable),
        ];
        for (code, expect) in cases {
            assert_eq!(failure_for(code, 0), expect, "code {code}");
        }
    }

    #[test]
    fn misuse_and_unknown_codes_are_failed_not_success() {
        for code in [-1, 8, 12, 99] {
            assert_eq!(failure_for(code, 0), F::Failed, "code {code}");
        }
    }

    #[test]
    fn ns_error_codes_are_mapped_not_flattened() {
        assert_eq!(failure_for(9, -9943), F::DeviceLocked);
        assert_eq!(failure_for(9, -9953), F::DeviceLocked);
        assert_eq!(failure_for(9, -21343), F::PermissionDenied);
        assert_eq!(failure_for(9, -21249), F::PermissionDenied);
        for removed in [-21350, -21349, -21348] {
            assert_eq!(failure_for(9, removed), F::Interrupted);
        }
        assert_eq!(failure_for(9, -9923), F::TimedOut);
    }

    #[test]
    fn unknown_or_missing_os_code_is_failed() {
        assert_eq!(failure_for(9, 0), F::Failed);
        assert_eq!(failure_for(9, -12345), F::Failed);
    }
}
