//! iOS 向け行動認識実装（Issue #614、親 #124）。`ios/fandhe_ios_activity_recognition.m`
//! （CoreMotion `CMMotionActivityManager` の購読、`build.rs` が iOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 3 関数で呼ぶ。`device/activity_recognition.rs` の iOS 経路が
//! [`implementation`] を呼ぶ。dispatch・共通コア画面・ドライバ・trait 契約
//! （`fandhe_device::activity_recognition`）は #613 の最終形のまま変えない。
//!
//! `.m` は OS の生値と状態コードを返すだけで、範囲検査と契約値への写像は [`start_result`] /
//! [`read_result`] / [`kind_from_flags`] が行う（ホストでテストできるよう `cfg(any(ios, test))`）。
//!
//! # 流れ
//!
//! - `start`: 非対応（シミュレータ含む）は `Unsupported`、許可拒否は `PermissionDenied`
//! - `read`: `.m` が `authorizationStatus` を毎回確かめる。拒否コード 2 種
//!   （`CMErrorNotAuthorized` / `CMErrorMotionActivityNotAuthorized`）は `PermissionDenied`、
//!   許可ダイアログの応答待ちは `AwaitingPermission`（期限に数えない）
//! - 分類フラグは `classify_flags` で種別にまとめる。判定なしを静止にしない
//! - 背景遷移・前景復帰の通知で `device/activity_recognition.rs` の `note_backgrounded` /
//!   `note_resumed` を呼ぶ。`.m` は背景で購読を解除し、復帰で張り直す
//! - 値・失敗の確定直後に `crate::ios::request_repaint` で次フレームを起こす（契約表 4）
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非対応・拒否・nil・判定なし・範囲外の確度を成功値へ畳まない、旧購読と背景中の通知を返さない、
//!   `seq` を巻き戻さない
//! - 守らない: OS 間の分類の一致、判定の遅れ、過去の履歴、背景中の受信、複数プロセス
//!
//! # 既知の天井
//!
//! - シミュレータは `isActivityAvailable` が NO で常に `Unsupported`
//! - 許可後 5 秒以内に最初の 1 件が届くかと拒否判定用 query の振る舞いは実機未確認（#603）

use fandhe_device::activity_recognition::{
    classify_flags, ActivityFailure, ActivityKind, ActivityPoll, ActivityReading, Confidence,
};

/// `.m` の start の戻り値を契約へ写す（0 成功 / 1 非対応 / 2 権限拒否 / 他 失敗）。
fn start_result(code: i32) -> Result<(), ActivityFailure> {
    match code {
        0 => Ok(()),
        1 => Err(ActivityFailure::Unsupported),
        2 => Err(ActivityFailure::PermissionDenied),
        _ => Err(ActivityFailure::Failed),
    }
}

/// `CMMotionActivityConfidence` の生値を確度へ写す。範囲外は `None`。
fn confidence_from_raw(raw: i32) -> Option<Confidence> {
    match raw {
        0 => Some(Confidence::Low),
        1 => Some(Confidence::Medium),
        2 => Some(Confidence::High),
        _ => None,
    }
}

/// `.m` のビット（bit0 静止 / bit1 歩行 / bit2 走行 / bit3 乗り物 / bit4 自転車）を種別へまとめる。
/// 未定義ビットは無視し、全部立っていなければ `Unknown`。
fn kind_from_flags(flags: u32) -> ActivityKind {
    classify_flags(
        flags & (1 << 0) != 0,
        flags & (1 << 1) != 0,
        flags & (1 << 2) != 0,
        flags & (1 << 3) != 0,
        flags & (1 << 4) != 0,
    )
}

/// `.m` の read の戻り値を契約へ写す（0 値なし / 1 値あり / 2 権限拒否 / 4 許可待ち / 他 失敗）。
/// 範囲外の確度はサンプルごと捨てて `Waiting`（変換前に検査する）。
fn read_result(
    code: i32,
    flags: u32,
    confidence: i32,
    seq: u64,
) -> Result<ActivityPoll, ActivityFailure> {
    match code {
        0 => Ok(ActivityPoll::Waiting),
        1 => Ok(match confidence_from_raw(confidence) {
            Some(confidence) => ActivityPoll::Reading(ActivityReading {
                kind: kind_from_flags(flags),
                confidence,
                seq,
            }),
            None => ActivityPoll::Waiting,
        }),
        2 => Err(ActivityFailure::PermissionDenied),
        4 => Ok(ActivityPoll::AwaitingPermission),
        _ => Err(ActivityFailure::Failed),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::activity_recognition::{ActivityFailure, ActivityPoll, ActivityRecognizer};

    use super::{read_result, start_result};

    // SAFETY: `ios/fandhe_ios_activity_recognition.m`（`build.rs` が iOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_activity_recognition_start() -> i32;
        fn fandhe_ios_activity_recognition_read(
            flags: *mut u32,
            confidence: *mut i32,
            seq: *mut u64,
        ) -> i32;
        fn fandhe_ios_activity_recognition_stop();
    }

    struct IosActivityRecognizer;

    impl ActivityRecognizer for IosActivityRecognizer {
        #[allow(unsafe_code)]
        fn start(&self) -> Result<(), ActivityFailure> {
            // SAFETY: 引数なし。`.m` は main 以外なら何もせず失敗コードを返す。
            start_result(unsafe { fandhe_ios_activity_recognition_start() })
        }

        #[allow(unsafe_code)]
        fn read(&self) -> Result<ActivityPoll, ActivityFailure> {
            let (mut flags, mut confidence, mut seq) = (0_u32, 0_i32, 0_u64);
            // SAFETY: 3 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
            let code = unsafe {
                fandhe_ios_activity_recognition_read(&mut flags, &mut confidence, &mut seq)
            };
            read_result(code, flags, confidence, seq)
        }

        #[allow(unsafe_code)]
        fn stop(&self) {
            // SAFETY: 引数なし。`.m` は main 以外なら main queue へ非同期に送る。
            unsafe { fandhe_ios_activity_recognition_stop() }
        }
    }

    /// iOS の `ActivityRecognizer` 実装を返す（`device/activity_recognition.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn ActivityRecognizer> {
        Arc::new(IosActivityRecognizer)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` が値・失敗を確定した直後に呼ぶ受け口。引数なし・ポインタなしで、`dispatch_sync` は
/// しない（契約表 11）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_activity_recognition_changed() {
    crate::ios::request_repaint();
}

/// 背景遷移の通知（`.m` の `DidEnterBackground` から）。背景中は `NoReading` の待機を止める。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_activity_recognition_backgrounded() {
    super::activity_recognition::note_backgrounded();
}

/// 前景復帰の通知（`.m` の `WillEnterForeground` から）。ドライバの待機起点を引き直させる。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_activity_recognition_resumed() {
    super::activity_recognition::note_resumed();
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(ActivityFailure::Unsupported));
        assert_eq!(start_result(2), Err(ActivityFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(ActivityFailure::Failed));
        assert_eq!(start_result(-1), Err(ActivityFailure::Failed));
    }

    #[test]
    fn read_codes() {
        assert_eq!(read_result(0, 0, 0, 0), Ok(ActivityPoll::Waiting));
        assert_eq!(
            read_result(4, 0, 0, 0),
            Ok(ActivityPoll::AwaitingPermission)
        );
        assert_eq!(
            read_result(2, 0, 0, 0),
            Err(ActivityFailure::PermissionDenied)
        );
        assert_eq!(read_result(3, 0, 0, 0), Err(ActivityFailure::Failed));
        assert_eq!(read_result(99, 0, 0, 0), Err(ActivityFailure::Failed));
        assert_eq!(
            read_result(1, 1 << 1, 2, 7),
            Ok(ActivityPoll::Reading(ActivityReading {
                kind: ActivityKind::Walking,
                confidence: Confidence::High,
                seq: 7,
            }))
        );
    }

    #[test]
    fn out_of_range_confidence_is_waiting() {
        assert_eq!(read_result(1, 1, -1, 1), Ok(ActivityPoll::Waiting));
        assert_eq!(read_result(1, 1, 3, 1), Ok(ActivityPoll::Waiting));
    }

    #[test]
    fn flags_are_classified() {
        assert_eq!(kind_from_flags(0), ActivityKind::Unknown);
        assert_eq!(
            kind_from_flags((1 << 0) | (1 << 3)),
            ActivityKind::Automotive
        );
        assert_eq!(kind_from_flags((1 << 1) | (1 << 2)), ActivityKind::Running);
        assert_eq!(kind_from_flags(1 << 4), ActivityKind::Cycling);
        // 未定義ビットは無視する。
        assert_eq!(kind_from_flags(1 << 9), ActivityKind::Unknown);
    }
}
