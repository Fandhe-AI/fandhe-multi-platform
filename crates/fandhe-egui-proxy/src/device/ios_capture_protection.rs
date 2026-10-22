//! iOS 向け画面キャプチャの検知実装（Issue #583、親 #120）。
//! `device/capture_protection.rs` の iOS 経路が `implementation()` を呼ぶ（dispatch・共通コア画面・
//! 文言は #582 で確定済みで触らない）。
//!
//! iOS に防止の公式 API は無い。録画・ミラーリングは `ios/fandhe_ios_capture_protection.m` が
//! `sceneCaptureState`（iOS 17+）を pull で読み、スクリーンショットは同 `.m` が通知 observer を
//! 初回に 1 回だけ登録して、通知のたびに本モジュールの atomic を加算する。
//!
//! # 契約
//!
//! - `support()` は防止なし・録画検知あり・スクショ検知あり。`set_protected` は on / off とも
//!   `Unsupported`、`is_protected` は `None`（防げないものを「保護中」と見せない）。
//! - 録画状態が読めない（背景・scene なし・main 以外）ときは `None`。`false` で代用しない。
//! - スクショ回数は observer 未登録なら `None`、登録後は単調増加（saturating）。
//!
//! # 再描画（契約表 4）
//!
//! スクショ通知は回数を先に確定してから `crate::ios::request_repaint` を呼ぶ。録画は OS コールバックが
//! 共有状態へ入れる経路が無く、ドライバの `READ_INTERVAL` ごとの pull で追いつく。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 防げない状態を保護中と見せない、読めない値を false にしない、背景中は録画を不明にする、
//! 回数を落とさない、observer を 1 回だけ登録する。守らない: 状態変化から次の pull までの最大 1 秒の遅れ、
//! 複数 scene・外部ディスプレイでの scene ごとの差（foreground の 1 scene のみ見る）、observer 登録前の
//! スクショ、撮影後に届く通知の到着遅延、他プロセスや外部カメラによる撮影、終了時の observer 解除
//! （プロセスと寿命が同じで OS が片付ける）。

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "ios")]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "ios")]
use std::sync::Arc;

use fandhe_device::capture_protection::CaptureSupport;
#[cfg(target_os = "ios")]
use fandhe_device::capture_protection::{CaptureObservation, CaptureProtection};
use fandhe_device::DeviceError;

/// iOS の対応範囲。防止は公式 API が無いため false。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
const IOS_SUPPORT: CaptureSupport = CaptureSupport {
    prevent: false,
    detect_recording: true,
    detect_screenshot: true,
};

/// スクリーンショット通知の累計。`.m` の observer（main キュー）から加算し、`observe` が読む。
static SCREENSHOT_COUNT: AtomicU64 = AtomicU64::new(0);

/// `read_recording` の戻りコードを録画状態へ写す。0 / 1 以外（main 以外・読めない・未知値）は不明。
fn recording_from_code(code: i32) -> Option<bool> {
    match code {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// 未登録なら回数は不明（0 で代用しない）。
fn screenshot_count(registered: bool, count: u64) -> Option<u64> {
    registered.then_some(count)
}

/// 回数を 1 増やす。上限で飽和し、単調増加を保つ。
fn bump(counter: &AtomicU64) {
    let _ = counter.try_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
        Some(v.saturating_add(1))
    });
}

/// 防止は iOS では常に未対応（on / off とも嘘の成功にしない）。
fn set_protected_result() -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("capture-protection"))
}

// `ios/fandhe_ios_capture_protection.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_capture_protection_read_recording() -> i32;
    fn fandhe_ios_capture_protection_start_screenshot() -> i32;
}

#[cfg(target_os = "ios")]
static SCREENSHOT_REGISTERED: AtomicBool = AtomicBool::new(false);

/// iOS の `CaptureProtection` 実装（検知のみ）。
#[cfg(target_os = "ios")]
struct IosCaptureProtection;

#[cfg(target_os = "ios")]
impl CaptureProtection for IosCaptureProtection {
    fn support(&self) -> CaptureSupport {
        IOS_SUPPORT
    }

    fn set_protected(&self, _on: bool) -> Result<(), DeviceError> {
        set_protected_result()
    }

    fn is_protected(&self) -> Option<bool> {
        None
    }

    #[allow(unsafe_code)]
    fn observe(&self) -> CaptureObservation {
        if !SCREENSHOT_REGISTERED.load(Ordering::SeqCst) {
            // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行い、main 以外では
            // 登録せず 2 を返す（次回の observe で再試行する）。
            if unsafe { fandhe_ios_capture_protection_start_screenshot() } == 0 {
                SCREENSHOT_REGISTERED.store(true, Ordering::SeqCst);
            }
        }
        // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行う。
        let code = unsafe { fandhe_ios_capture_protection_read_recording() };
        CaptureObservation {
            recording: recording_from_code(code),
            screenshot_count: screenshot_count(
                SCREENSHOT_REGISTERED.load(Ordering::SeqCst),
                SCREENSHOT_COUNT.load(Ordering::SeqCst),
            ),
        }
    }
}

/// プロセスで 1 個の iOS 実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn CaptureProtection> {
    Arc::new(IosCaptureProtection)
}

/// `.m` の observer（main キュー）がスクショ通知を受けた直後に呼ぶ受け口。回数を先に確定してから
/// 再描画を起こす。引数なし・ポインタなしで panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_capture_protection_screenshot_taken() {
    bump(&SCREENSHOT_COUNT);
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_code_maps_unknown_to_none() {
        assert_eq!(recording_from_code(0), Some(false));
        assert_eq!(recording_from_code(1), Some(true));
        for c in [2, 3, -1, 99] {
            assert_eq!(recording_from_code(c), None);
        }
    }

    #[test]
    fn unregistered_screenshot_count_is_none() {
        assert_eq!(screenshot_count(false, 5), None);
        assert_eq!(screenshot_count(true, 0), Some(0));
    }

    #[test]
    fn bump_is_monotonic_and_saturates() {
        let c = AtomicU64::new(0);
        bump(&c);
        bump(&c);
        assert_eq!(c.load(Ordering::SeqCst), 2);
        let m = AtomicU64::new(u64::MAX);
        bump(&m);
        assert_eq!(m.load(Ordering::SeqCst), u64::MAX);
    }

    #[test]
    fn support_is_detect_only_and_prevent_is_unsupported() {
        // 定数の検証はコンパイル時に行う（clippy::assertions_on_constants 回避）
        const _: () = assert!(!IOS_SUPPORT.prevent);
        const _: () = assert!(IOS_SUPPORT.detect_recording);
        const _: () = assert!(IOS_SUPPORT.detect_screenshot);
        assert!(matches!(
            set_protected_result(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
