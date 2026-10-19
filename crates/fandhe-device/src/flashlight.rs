//! ライト（トーチ）の点灯 / 消灯（Phase 11 の拡充ループ、親 #106、Issue #458、対応表
//! `docs/device/capabilities/flashlight.json`）。
//!
//! 扱う範囲は「ライトを点ける / 消す」と「OS から読み戻した点灯状態」だけ（明るさ調整・点滅は
//! 扱わない）。対象は iOS / Android / Windows / Web（macOS は `n-a`）。`fandhe-egui-proxy` の
//! 橋渡し層が [`Flashlight::set_on`] で要求し、[`Flashlight::is_on`] で読み戻した値を画面
//! （`fandhe_core::screens::flashlight_toggle`）へ返す。Web は `fandhe-web-dom` が直接配線する。
//!
//! # 契約
//!
//! - **「要求した」と「点いている」を分ける**: 画面は [`Flashlight::is_on`] が `Some(true)` の
//!   ときだけ「点灯中」と出す（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **冪等**: 点灯中の再点灯・消灯中の再消灯は `Ok`
//! - **`is_on` の失敗**: 失敗したら前回の値を返し続けず `Err`。`Ok(None)` は「まだ分からない」
//!   （Android で最初の `TorchCallback` が届く前）。ライトが無ければ `Err(Unsupported)`
//! - **エラー種別**: ライトが無い → `Unsupported`、Web の許可拒否 → `PermissionDenied`、
//!   使用中・過熱・例外 → `Internal`。利用者がキャンセルする経路は無いので `Cancelled` は
//!   返さず、返ってきても失敗として扱う。三値のうちキャンセルは該当なしで、成功 / 失敗の二値
//!   （同「10. キャンセル・正常終了・失敗」）
//! - **スレッド**: iOS・Android は UI スレッドから呼ぶ。Windows は同期待ちを含むためワーカーから呼ぶ
//! - **背景遷移・破棄で自分から消灯し、戻っても点け直さない**（iOS は `DidEnterBackground`、
//!   Android は `onStop`、Web は hidden / `pagehide`、Windows は最小化）。親 #106 の判断点 1 (A)
//! - **経路**: 画面 → effect →（Windows はワーカー）の 1 本。画面側の `Applying` / `Confirming`
//!   が直列を保証する（同「11. 並行経路の集約と守備範囲」）。権限ダイアログを待たない一発呼び出しで
//!   読み戻しは pull のため、`RequestSlot` / `RequestTracker` は使わない
//! - **時間**: 期限・間隔は `Instant`（単調時計）で測る
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の順序、嘘の成功の禁止、資源（ライト・Web のトラック）の解放。
//! 守らない（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）: 外部からの点け消しへの追従は
//! 1 秒 + 1 フレーム遅れる、Windows の `AvailabilityChanged` を購読しない、プロセス終了時の
//! 後片付けは OS に任せる、熱による減光、Web の複数タブ。いずれも表示が一時的に食い違うだけで、
//! データ消失・資源漏れ・嘘の成功には至らない。

use std::time::Duration;

use crate::DeviceError;

/// 点灯状態を OS から読み直す間隔。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);
/// 要求後、反映の確認で読み戻す間隔。
pub const CONFIRM_POLL: Duration = Duration::from_millis(100);
/// 要求後、反映を待つ上限。超えたら失敗にする。
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(1);

/// iOS / Android の set 結果コード: 成功。
pub const SET_OK: i32 = 0;
/// set 結果コード: ライト無し。
pub const SET_UNSUPPORTED: i32 = 1;
/// iOS / Android の read 結果コード: 消灯。
pub const READ_OFF: i32 = 0;
/// read 結果コード: 点灯。
pub const READ_ON: i32 = 1;
/// read 結果コード: ライト無し。
pub const READ_UNSUPPORTED: i32 = -1;
/// read 結果コード: まだ分からない（Android の初回コールバック前）。
pub const READ_UNKNOWN: i32 = -2;

/// ライトの契約（モジュール doc の「契約」節）。
pub trait Flashlight: Send + Sync {
    /// `true` で点灯、`false` で消灯。冪等。
    fn set_on(&self, on: bool) -> Result<(), DeviceError>;

    /// OS から読み戻した点灯状態。`Ok(None)` は「まだ分からない」。
    fn is_on(&self) -> Result<Option<bool>, DeviceError>;
}

/// 非対応 OS 向け。どちらも `Unsupported` を返し、ダミーの成功を返さない。
pub struct UnsupportedFlashlight;

impl Flashlight for UnsupportedFlashlight {
    fn set_on(&self, _on: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("flashlight"))
    }

    fn is_on(&self) -> Result<Option<bool>, DeviceError> {
        Err(DeviceError::Unsupported("flashlight"))
    }
}

/// set の結果コード（iOS / Android 共通の SSOT）を契約のエラー種別へ写す。未知の値は `Internal`。
pub fn decode_set(code: i32) -> Result<(), DeviceError> {
    match code {
        SET_OK => Ok(()),
        SET_UNSUPPORTED => Err(DeviceError::Unsupported("flashlight")),
        _ => Err(DeviceError::Internal("flashlight")),
    }
}

/// read の結果コード（iOS / Android 共通の SSOT）を契約の値へ写す。未知の値は `Internal`。
pub fn decode_read(code: i32) -> Result<Option<bool>, DeviceError> {
    match code {
        READ_ON => Ok(Some(true)),
        READ_OFF => Ok(Some(false)),
        READ_UNKNOWN => Ok(None),
        READ_UNSUPPORTED => Err(DeviceError::Unsupported("flashlight")),
        _ => Err(DeviceError::Internal("flashlight")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_set_maps_codes() {
        assert!(decode_set(0).is_ok());
        assert!(matches!(decode_set(1), Err(DeviceError::Unsupported(_))));
        for c in [2, 99, -1] {
            assert!(matches!(decode_set(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn decode_read_maps_codes() {
        assert_eq!(decode_read(1).unwrap(), Some(true));
        assert_eq!(decode_read(0).unwrap(), Some(false));
        assert_eq!(decode_read(-2).unwrap(), None);
        assert!(matches!(decode_read(-1), Err(DeviceError::Unsupported(_))));
        for c in [2, 3, 99] {
            assert!(matches!(decode_read(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn unsupported_is_never_success() {
        let u = UnsupportedFlashlight;
        assert!(matches!(u.set_on(true), Err(DeviceError::Unsupported(_))));
        assert!(matches!(u.set_on(false), Err(DeviceError::Unsupported(_))));
        assert!(matches!(u.is_on(), Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn timing_constants() {
        assert_eq!(READ_INTERVAL, Duration::from_secs(1));
        assert_eq!(CONFIRM_POLL, Duration::from_millis(100));
        assert_eq!(CONFIRM_TIMEOUT, Duration::from_secs(1));
    }
}
