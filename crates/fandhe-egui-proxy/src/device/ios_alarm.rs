//! iOS 向けアラーム実装（Issue #1077、親 #1056）。`device/alarm.rs` の iOS 経路から呼ばれ、
//! `fandhe_device::alarm::Alarm` を AlarmKit（`ios/fandhe_ios_alarm.swift`。`build.rs` が feature
//! `device` 有効時に swiftc で静的ライブラリ化し弱リンクする）で実装する。`unsafe` は
//! `@_cdecl` の C ABI 3 関数の宣言と呼び出し 3 か所に閉じる（coding-rust.md の FFI 例外規定準拠）。
//!
//! 契約の SSOT は `fandhe_device::alarm`。戻りコード → `DeviceError` の写像の SSOT は本ファイルの
//! `match`（`unit_from_code`・`state_from_code`）で、Swift 冒頭コメントはその写し。
//!
//! # 守る範囲
//!
//! - 呼び出しはブロックする（許可ダイアログの応答待ちを含み、期限は掛けない。中断できない処理は
//!   完了を待って実結果を返す）。呼び出し元の effect ドライバは専用ワーカーで呼ぶ。
//! - schedule・cancel・state は `Mutex` で直列化し、ポーリングと操作が重なっても判定と更新を
//!   1 本の順序にする。
//! - OS コールバックが共有状態へ書く経路は無い。結果は Swift 関数の戻り値として同期に返り、
//!   ドライバが `CompletionSender::send`（内部で `request_repaint` を起こす）で画面へ届けるため、
//!   `crate::ios::request_repaint` をここで重ねて呼ばない。
//! - iOS 26 未満・旧 SDK は `Unsupported`、許可の拒否は `PermissionDenied`。ホスト（`cargo test`）では
//!   `UnsupportedAlarm` を返し、成功へ倒さない。

use std::sync::Arc;

use fandhe_device::alarm::{Alarm, AlarmState};
use fandhe_device::DeviceError;

/// 戻りコード 8（予約中。`state` 専用）。
const CODE_SCHEDULED: i32 = 8;

/// `schedule`・`cancel` の戻りコードを写す。`state` 専用の 8 は成功へ畳まず `Internal` にする。
// ホストのビルドでは iOS 実体（`imp`）が無く、テストからしか呼ばれない。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
fn unit_from_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        c => Err(error_from_code(c)),
    }
}

/// `state` の戻りコードを写す。`out_ms` は 8 のときだけ読む。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
fn state_from_code(code: i32, out_ms: u64) -> Result<AlarmState, DeviceError> {
    match code {
        0 => Ok(AlarmState::NotScheduled),
        CODE_SCHEDULED => Ok(AlarmState::Scheduled {
            fire_at_unix_ms: out_ms,
        }),
        c => Err(error_from_code(c)),
    }
}

/// 成功以外の戻りコードを `DeviceError` へ写す（同じ意味のコードを 1 つに潰さない）。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
fn error_from_code(code: i32) -> DeviceError {
    match code {
        1 => DeviceError::Unsupported("alarm"),
        2 => DeviceError::PermissionDenied("alarm"),
        3 => DeviceError::Internal("alarm limit"),
        4 | 5 => DeviceError::Internal("alarm add"),
        6 => DeviceError::Internal("alarm list"),
        7 => DeviceError::Internal("alarm remove"),
        _ => DeviceError::Internal("alarm"),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use crate::device::alarm::{now_unix_ms, validate_fire_at};
    use std::sync::Mutex;

    // SAFETY: `ios/fandhe_ios_alarm.swift` が `@_cdecl` で定義する C ABI（`build.rs` が iOS ターゲットの
    // feature `device` 有効時のみコンパイル・リンクする）。契約は swift 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_ios_alarm_schedule(fire_at_ms: u64) -> i32;
        fn fandhe_ios_alarm_cancel() -> i32;
        fn fandhe_ios_alarm_state(out_ms: *mut u64) -> i32;
    }

    /// AlarmKit で予約する実装。
    pub(super) struct IosAlarm {
        lock: Mutex<()>,
    }

    impl IosAlarm {
        pub(super) fn new() -> Self {
            Self {
                lock: Mutex::new(()),
            }
        }

        fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
            self.lock.lock().unwrap_or_else(|e| e.into_inner())
        }
    }

    impl Alarm for IosAlarm {
        #[allow(unsafe_code)]
        fn schedule(&self, fire_at_unix_ms: u64) -> Result<(), DeviceError> {
            validate_fire_at(fire_at_unix_ms, now_unix_ms()?)?;
            let _g = self.guard();
            // SAFETY: 引数は値渡しのみ。
            unit_from_code(unsafe { fandhe_ios_alarm_schedule(fire_at_unix_ms) })
        }

        #[allow(unsafe_code)]
        fn cancel(&self) -> Result<(), DeviceError> {
            let _g = self.guard();
            // SAFETY: 引数なし。
            unit_from_code(unsafe { fandhe_ios_alarm_cancel() })
        }

        #[allow(unsafe_code)]
        fn state(&self) -> Result<AlarmState, DeviceError> {
            let _g = self.guard();
            let mut out_ms = 0u64;
            // SAFETY: `out_ms` は呼び出しの間生存する 1 要素の書き込み先で、Swift は呼び出しの間だけ
            // 参照し保持しない。
            let code = unsafe { fandhe_ios_alarm_state(&mut out_ms) };
            state_from_code(code, out_ms)
        }
    }
}

/// iOS の `Alarm` 実装を返す。ホスト（テスト）では `UnsupportedAlarm`。
pub(crate) fn implementation() -> Arc<dyn Alarm> {
    #[cfg(target_os = "ios")]
    {
        Arc::new(imp::IosAlarm::new())
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(fandhe_device::alarm::UnsupportedAlarm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "ios"))]
    fn host_is_not_a_fake_success() {
        let a = implementation();
        assert!(matches!(
            a.schedule(u64::MAX),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(a.cancel(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(a.state(), Err(DeviceError::Unsupported(_))));
    }

    /// `DeviceError` は `PartialEq` を持たないので Debug 表現で比べる。
    fn dbg<T: std::fmt::Debug>(v: T) -> String {
        format!("{v:?}")
    }

    #[test]
    fn unit_codes_map_without_collapsing() {
        assert!(unit_from_code(0).is_ok());
        let cases = [
            (1, "Unsupported(\"alarm\")"),
            (2, "PermissionDenied(\"alarm\")"),
            (3, "Internal(\"alarm limit\")"),
            (4, "Internal(\"alarm add\")"),
            (5, "Internal(\"alarm add\")"),
            (6, "Internal(\"alarm list\")"),
            (7, "Internal(\"alarm remove\")"),
            // state 専用の 8・未知・負値を成功へ畳まない。
            (8, "Internal(\"alarm\")"),
            (9, "Internal(\"alarm\")"),
            (-1, "Internal(\"alarm\")"),
            (i32::MIN, "Internal(\"alarm\")"),
        ];
        for (code, want) in cases {
            assert_eq!(dbg(unit_from_code(code).unwrap_err()), want, "code {code}");
        }
    }

    #[test]
    fn state_codes_map() {
        assert!(matches!(
            state_from_code(0, 123),
            Ok(AlarmState::NotScheduled)
        ));
        assert!(matches!(
            state_from_code(8, 123),
            Ok(AlarmState::Scheduled {
                fire_at_unix_ms: 123
            })
        ));
        assert!(matches!(
            state_from_code(2, 0),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(
            state_from_code(-1, 0),
            Err(DeviceError::Internal("alarm"))
        ));
    }
}
