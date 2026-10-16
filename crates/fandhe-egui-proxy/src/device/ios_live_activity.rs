//! iOS（ActivityKit）向けライブアクティビティ実装（Issue #295、親 #88 / #294 の b-ios）。
//! `device/live_activity.rs::platform_live_activity` の ios 経路から呼ばれ、
//! `ios/fandhe_ios_live_activity.swift`（`build.rs` が iOS の feature `device` 有効時に swiftc で静的
//! ライブラリ化し ActivityKit をリンクする）を C ABI で呼ぶ。dispatch・共通コア画面・trait 契約
//! （`fandhe_device::live_activity`）は変えない。要求の照合はドライバ側の `RequestSlot` が持つ。
//!
//! # ABI
//!
//! - Rust → Swift: `fandhe_ios_live_activity_submit(request_id, op, value, done) -> i32`（0 = 受理）と
//!   `fandhe_ios_live_activity_dismiss_all()`。どのスレッドからでも呼べ、Swift 側は main actor の
//!   直列チェーンへ積んで即 return する。
//! - Swift → Rust: 関数ポインタ [`on_done`]（`(request_id, op, code)`）。main actor から要求ごとに
//!   ちょうど 1 回呼ばれる。ポインタではなく整数と `'static` の関数ポインタだけを渡す。
//!
//! # 結果コード（写像の SSOT は [`outcome_from_code`]）
//!
//! 0 開始 / 1 更新 / 2 終了 / 3 既に閉じられていた（`Dismissed`）/ 10 許可なし / 11 非対応
//! （拡張の未同梱を含む）。それ以外（20 以上・未知）と、op に合わない成功コードは `Internal`。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 失敗・未知のコードを成功へ倒さない、`ActivityAuthorizationError` を `-1` へ潰さない、
//!   初回生成時に前回プロセスの残りを閉じる、完了直後に再描画を起こす
//! - 守らない: プロセス死後の表示の残り（次回起動で片付く）、利用者が閉じてから次の操作までの
//!   表示の食い違い、OS が描画へ反映するまでの時間差、iOS の 8 時間上限
//!
//! # 既知の天井
//!
//! 実機ビルドは拡張を同梱しない（署名手順がアプリ 1 つ分しか無い）ため `Unsupported` を返す。
//! 追跡は #293。

use std::sync::Arc;

use fandhe_device::live_activity::{LiveActivity, LiveOutcome};
use fandhe_device::DeviceError;

const OP_START: i32 = 0;
const OP_UPDATE: i32 = 1;
const OP_END: i32 = 2;

/// 受理コード。開始 0・更新 1・終了 2 は op と同じ値で、3 は「既に閉じられていた」。
const CODE_DISMISSED: i32 = 3;
const CODE_PERMISSION_DENIED: i32 = 10;
const CODE_UNSUPPORTED: i32 = 11;

/// Swift の結果コードを契約の結果へ写す（コード表の SSOT）。op に合わない成功コード・未知のコードは
/// `Internal` にして、成功へ倒さない。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn outcome_from_code(op: i32, code: i32) -> Result<LiveOutcome, DeviceError> {
    let internal = DeviceError::Internal(match op {
        OP_START => "live_activity start",
        OP_UPDATE => "live_activity update",
        OP_END => "live_activity end",
        _ => "live_activity",
    });
    match (op, code) {
        (OP_START, 0) => Ok(LiveOutcome::Started),
        (OP_UPDATE, 1) => Ok(LiveOutcome::Updated),
        (OP_UPDATE, CODE_DISMISSED) => Ok(LiveOutcome::Dismissed),
        (OP_END, 2) => Ok(LiveOutcome::Ended),
        (_, CODE_PERMISSION_DENIED) => Err(DeviceError::PermissionDenied("live_activity")),
        (_, CODE_UNSUPPORTED) => Err(DeviceError::Unsupported("live_activity")),
        _ => Err(internal),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use super::*;
    use crate::device::live_activity::bridge;
    use fandhe_device::live_activity::LiveOperation;

    // `ios/fandhe_ios_live_activity.swift` が定義する C ABI。契約は Swift 側冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_live_activity_submit(
            request_id: u64,
            op: i32,
            value: u32,
            done: extern "C" fn(u64, i32, i32),
        ) -> i32;
        fn fandhe_ios_live_activity_dismiss_all();
    }

    /// Swift の完了受け口。結果を共有状態へ置いた直後に次フレームを起こす（入力がないと egui が
    /// 描画せず、画面が処理中のまま止まる。契約表 4）。
    extern "C" fn on_done(request_id: u64, op: i32, code: i32) {
        bridge::deliver(request_id, outcome_from_code(op, code));
        crate::ios::request_repaint();
    }

    pub(super) struct IosLiveActivity;

    impl LiveActivity for IosLiveActivity {
        fn submit(&self, request_id: u64, op: LiveOperation) -> Result<(), DeviceError> {
            let (code, value, name) = match op {
                LiveOperation::Start(v) => (OP_START, v.get(), "live_activity start"),
                LiveOperation::Update(v) => (OP_UPDATE, v.get(), "live_activity update"),
                LiveOperation::End => (OP_END, 0, "live_activity end"),
            };
            // SAFETY: 渡すのは整数と `'static` の関数ポインタのみ。Swift 側は即 return し、完了は
            // main actor から `on_done` をちょうど 1 回呼ぶ。
            #[allow(unsafe_code)]
            let accepted =
                unsafe { fandhe_ios_live_activity_submit(request_id, code, value, on_done) };
            if accepted == 0 {
                Ok(())
            } else {
                Err(DeviceError::Internal(name))
            }
        }

        fn dismiss_all(&self) {
            // SAFETY: 引数なし。Swift 側が main actor の直列チェーンへ積んで即 return する。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_ios_live_activity_dismiss_all()
            }
        }
    }
}

/// この OS の `LiveActivity` 実装。初回生成時に前回プロセスの残りを閉じる（trait doc の契約）。
pub(crate) fn implementation() -> Arc<dyn LiveActivity> {
    #[cfg(target_os = "ios")]
    {
        let imp = imp::IosLiveActivity;
        imp.dismiss_all();
        Arc::new(imp)
    }
    #[cfg(not(target_os = "ios"))]
    {
        Arc::new(fandhe_device::live_activity::UnsupportedLiveActivity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(op: i32, code: i32) -> Option<LiveOutcome> {
        outcome_from_code(op, code).ok()
    }

    #[test]
    fn success_codes_map_per_op() {
        assert_eq!(ok(OP_START, 0), Some(LiveOutcome::Started));
        assert_eq!(ok(OP_UPDATE, 1), Some(LiveOutcome::Updated));
        assert_eq!(ok(OP_UPDATE, 3), Some(LiveOutcome::Dismissed));
        assert_eq!(ok(OP_END, 2), Some(LiveOutcome::Ended));
    }

    #[test]
    fn permission_and_unsupported() {
        for op in [OP_START, OP_UPDATE, OP_END] {
            assert!(matches!(
                outcome_from_code(op, 10),
                Err(DeviceError::PermissionDenied("live_activity"))
            ));
            assert!(matches!(
                outcome_from_code(op, 11),
                Err(DeviceError::Unsupported("live_activity"))
            ));
        }
    }

    #[test]
    fn mismatched_or_unknown_codes_are_internal() {
        assert!(matches!(
            outcome_from_code(OP_START, 1),
            Err(DeviceError::Internal("live_activity start"))
        ));
        assert!(matches!(
            outcome_from_code(OP_END, 3),
            Err(DeviceError::Internal("live_activity end"))
        ));
        for code in [20, 21, 22, 23, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(OP_UPDATE, code),
                Err(DeviceError::Internal("live_activity update"))
            ));
        }
        assert!(matches!(
            outcome_from_code(99, 0),
            Err(DeviceError::Internal("live_activity"))
        ));
    }
}
