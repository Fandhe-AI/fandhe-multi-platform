//! iOS 向け UWB 測距実装（Issue #654、親 #129）。`ios/fandhe_ios_uwb_ranging.m`
//! （NearbyInteraction の `NISession` + MultipeerConnectivity による token 交換。`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 4 関数で呼ぶ。`device/uwb_ranging.rs` の
//! iOS 経路が [`implementation`] を呼ぶ。dispatch・共通コア画面・driver・trait 契約
//! （`fandhe_device::uwb_ranging`）は #653 の最終形のまま変えない。
//!
//! `.m` は OS の生値と状態コードを返すだけで、契約値への写像は [`capabilities_result`] /
//! [`start_result`] / [`read_result`] が行う（ホストでテストできるよう `cfg(any(ios, test))`）。
//! 距離の範囲検証と期限（単調時計）は driver が持ち、ここでは丸めも時間測定もしない。
//!
//! # 流れ
//!
//! - `capabilities`: UWB 非搭載（シミュレータ含む）は `distance: No` を `Ok` で返す（画面は開始ボタンを出さない）
//! - `start`: 同じ端末では `NotCapable`。受理後の探索・交換・測距は `.m` の serial queue で進む
//! - `read`: 許可ダイアログ中は `AwaitingPermission`（期限に数えない）。拒否は `PermissionDenied`、
//!   非搭載は `NotCapable`、それ以外の失敗は `Failed`。相手の終了と背景遷移の中断は終端として
//!   `stop` まで同じ値を返す
//! - 値・失敗の確定直後に `crate::ios::request_repaint` で次フレームを起こす（契約表 4）
//! - 相手の token の長さの上限は `fandhe_device::uwb_ranging::is_valid_token_len` が SSOT で、
//!   `.m` は `fandhe_rust_uwb_ranging_token_len_ok` 経由で検査してから解凍する
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非搭載・拒否・未知のコードを成功値へ畳まない、旧世代の通知を返さない、`seq` を巻き戻さない、
//!   停止・中断・失敗の全経路で資源を解放する
//! - 守らない: 相手の認証、3 台以上での選び方、精度と更新間隔、背景継続、複数プロセス
//!
//! # 既知の天井
//!
//! - シミュレータは UWB 非搭載で `distance: No`
//! - `timeout` 理由の再 run、許可拒否時の挙動、窓の長さは実機未確認（#648）

use fandhe_device::uwb_ranging::{RangeSample, Support, UwbCapabilities, UwbFailure, UwbPoll};

/// `.m` の能力値（0 No / 1 Yes / 他 Unknown）を契約へ写す。Yes へ畳まない。
fn support_from_raw(raw: i32) -> Support {
    match raw {
        0 => Support::No,
        1 => Support::Yes,
        _ => Support::Unknown,
    }
}

/// `.m` の capabilities の戻り値を契約へ写す（0 成功 / 他 失敗）。
fn capabilities_result(
    code: i32,
    distance: i32,
    direction: i32,
) -> Result<UwbCapabilities, UwbFailure> {
    if code != 0 {
        return Err(UwbFailure::Failed);
    }
    Ok(UwbCapabilities {
        distance: support_from_raw(distance),
        direction: support_from_raw(direction),
    })
}

/// `.m` の start の戻り値を契約へ写す（0 受理 / 1 非搭載 / 他 失敗）。
fn start_result(code: i32) -> Result<(), UwbFailure> {
    match code {
        0 => Ok(()),
        1 => Err(UwbFailure::NotCapable),
        _ => Err(UwbFailure::Failed),
    }
}

/// `.m` の read の戻り値を契約へ写す。距離は生のまま渡し、検証は driver が行う。
fn read_result(code: i32, distance_m: f32, seq: u64) -> Result<UwbPoll, UwbFailure> {
    match code {
        0 => Ok(UwbPoll::Searching),
        1 => Ok(UwbPoll::Exchanging),
        2 => Ok(UwbPoll::AwaitingPermission),
        3 => Ok(UwbPoll::Measured(RangeSample { distance_m, seq })),
        4 => Ok(UwbPoll::NoMeasurement),
        5 => Ok(UwbPoll::PeerEnded),
        6 => Ok(UwbPoll::Interrupted),
        10 => Err(UwbFailure::PermissionDenied),
        11 => Err(UwbFailure::NotCapable),
        _ => Err(UwbFailure::Failed),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::uwb_ranging::{UwbCapabilities, UwbFailure, UwbPoll, UwbRanging};

    use super::{capabilities_result, read_result, start_result};

    // SAFETY: `ios/fandhe_ios_uwb_ranging.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_uwb_ranging_capabilities(distance: *mut i32, direction: *mut i32) -> i32;
        fn fandhe_ios_uwb_ranging_start() -> i32;
        fn fandhe_ios_uwb_ranging_read(distance_m: *mut f32, seq: *mut u64) -> i32;
        fn fandhe_ios_uwb_ranging_stop();
    }

    struct IosUwbRanging;

    impl UwbRanging for IosUwbRanging {
        #[allow(unsafe_code)]
        fn capabilities(&self) -> Result<UwbCapabilities, UwbFailure> {
            let (mut d, mut dir) = (0_i32, 0_i32);
            // SAFETY: 2 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
            let code = unsafe { fandhe_ios_uwb_ranging_capabilities(&mut d, &mut dir) };
            capabilities_result(code, d, dir)
        }

        #[allow(unsafe_code)]
        fn start(&self) -> Result<(), UwbFailure> {
            // SAFETY: 引数なし。`.m` は main 以外なら何もせず失敗コードを返す。
            start_result(unsafe { fandhe_ios_uwb_ranging_start() })
        }

        #[allow(unsafe_code)]
        fn read(&self) -> Result<UwbPoll, UwbFailure> {
            let (mut distance, mut seq) = (0_f32, 0_u64);
            // SAFETY: 2 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
            let code = unsafe { fandhe_ios_uwb_ranging_read(&mut distance, &mut seq) };
            read_result(code, distance, seq)
        }

        #[allow(unsafe_code)]
        fn stop(&self) {
            // SAFETY: 引数なし。`.m` は状態を捨てて解放を serial queue へ非同期に送る。
            unsafe { fandhe_ios_uwb_ranging_stop() }
        }
    }

    /// iOS の `UwbRanging` 実装を返す（`device/uwb_ranging.rs` の iOS 経路から呼ばれ、1 個に保たれる）。
    pub(crate) fn implementation() -> Arc<dyn UwbRanging> {
        Arc::new(IosUwbRanging)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` が状態を確定した直後に呼ぶ受け口。引数なし・ポインタなしで、`dispatch_sync` はしない
/// （契約表 11）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_uwb_ranging_changed() {
    crate::ios::request_repaint();
}

/// `.m` が相手の token を解凍する前に長さを検査する受け口（上限の SSOT は Rust 側）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_uwb_ranging_token_len_ok(len: usize) -> bool {
    fandhe_device::uwb_ranging::is_valid_token_len(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_never_folds_to_yes() {
        assert_eq!(support_from_raw(0), Support::No);
        assert_eq!(support_from_raw(1), Support::Yes);
        assert_eq!(support_from_raw(2), Support::Unknown);
        assert_eq!(support_from_raw(-1), Support::Unknown);
        assert_eq!(support_from_raw(99), Support::Unknown);
    }

    #[test]
    fn capabilities_codes() {
        assert_eq!(
            capabilities_result(0, 0, 0),
            Ok(UwbCapabilities {
                distance: Support::No,
                direction: Support::No
            })
        );
        assert_eq!(
            capabilities_result(0, 1, 2),
            Ok(UwbCapabilities {
                distance: Support::Yes,
                direction: Support::Unknown
            })
        );
        assert_eq!(capabilities_result(3, 1, 1), Err(UwbFailure::Failed));
    }

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(UwbFailure::NotCapable));
        assert_eq!(start_result(3), Err(UwbFailure::Failed));
        assert_eq!(start_result(-1), Err(UwbFailure::Failed));
    }

    #[test]
    fn read_codes() {
        assert_eq!(read_result(0, 0.0, 0), Ok(UwbPoll::Searching));
        assert_eq!(read_result(1, 0.0, 0), Ok(UwbPoll::Exchanging));
        assert_eq!(read_result(2, 0.0, 0), Ok(UwbPoll::AwaitingPermission));
        assert_eq!(read_result(4, 0.0, 0), Ok(UwbPoll::NoMeasurement));
        assert_eq!(read_result(5, 0.0, 0), Ok(UwbPoll::PeerEnded));
        assert_eq!(read_result(6, 0.0, 0), Ok(UwbPoll::Interrupted));
        assert_eq!(read_result(10, 0.0, 0), Err(UwbFailure::PermissionDenied));
        assert_eq!(read_result(11, 0.0, 0), Err(UwbFailure::NotCapable));
        assert_eq!(read_result(12, 0.0, 0), Err(UwbFailure::Failed));
        assert_eq!(read_result(99, 0.0, 0), Err(UwbFailure::Failed));
        assert_eq!(read_result(-1, 0.0, 0), Err(UwbFailure::Failed));
    }

    #[test]
    fn measured_passes_raw_value_through() {
        // 範囲外の値も丸めず渡す（検証は driver の `validate_distance_m` の 1 か所）。
        assert_eq!(
            read_result(3, 123.0, 7),
            Ok(UwbPoll::Measured(RangeSample {
                distance_m: 123.0,
                seq: 7
            }))
        );
    }
}
