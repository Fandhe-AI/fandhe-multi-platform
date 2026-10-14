//! iOS / macOS 向け位置情報実装（Issue #889）。`device/location.rs` の Apple cfg 分岐
//! （`any(target_os = "ios", target_os = "macos")`）から呼ばれ、CoreLocation
//! （`CLLocationManager`）実装（`apple/fandhe_apple_location.m`、`build.rs` が
//! iOS・macOS 双方の feature `device` 有効時のみコンパイル・リンクする）を
//! `unsafe extern "C"` 1 関数（[`start`]）で呼ぶ。`unsafe` はこの呼び出し 1 箇所に閉じ
//! `// SAFETY:` で契約を明記する（`ios.rs` の `IosCamera`/`IosMotion` と同型、
//! `lib.rs` の macOS `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の delegate コールバックから [`fandhe_rust_location_fix`] /
//! [`fandhe_rust_location_failure`]（本モジュールが公開する `extern "C"` 関数）が
//! 呼ばれ、`device/location.rs::SHARED` へ push する（push 型、`ios.rs` の
//! `fandhe_rust_tts_outcome` と同じ橋渡し方式）。
//!
//! 座標をログへ出力しない（security.md A09）。

use fandhe_device::location::LocationFailure;

use super::location::SHARED;

// SAFETY: `apple/fandhe_apple_location.m` が定義する C ABI（`build.rs` が iOS・macOS
// 両ターゲットの feature `device` 有効時のみコンパイル・リンクする）。契約は
// `.m` ファイル冒頭コメントおよび下記 `start` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_location_start();
}

/// `.m` 側の `fandhe_apple_location_start` を呼ぶ（`device/location.rs::location_effect`
/// から、初回・再試行（「再試行」ボタン押下）の両方で呼ばれる契約）。冪等: `.m` 側は
/// 初回のみ `CLLocationManager` を生成し、2 回目以降は現在の `authorizationStatus` を
/// 再評価して結果を再 push する（`.m` ファイル冒頭コメント参照）。
#[allow(unsafe_code)]
pub fn start() {
    // SAFETY: `fandhe_apple_location_start` は引数・戻り値を持たず、呼び出しスレッドを
    // 変更しない（メインスレッド以外から呼ばれた場合は `.m` 側が内部で
    // `dispatch_async` へ載せ替える。`.m` ファイル冒頭コメント参照）。副作用は
    // `CLLocationManager` の生成・権限確認・delegate コールバック経由の非同期 push の
    // みで、呼び出し側（本関数）のメモリ安全性には影響しない。
    unsafe { fandhe_apple_location_start() };
}

/// `.m` 側の `code`（1=denied / 2=disabled / 3=failed）を [`LocationFailure`] へ写す
/// 純関数（cfg 無しで全ターゲットからテストできる、`device/location.rs::to_failure_kind`
/// と同じ切り出し方針）。未知のコードは `None` を返し、呼び出し元が無視する
/// （偽の成功・偽の失敗を作らない、実装計画 §8 参照）。
fn failure_from_code(code: i32) -> Option<LocationFailure> {
    match code {
        1 => Some(LocationFailure::PermissionDenied),
        2 => Some(LocationFailure::ServiceDisabled),
        3 => Some(LocationFailure::Failed),
        _ => None,
    }
}

/// `.m` 側の `FandheLocationDelegate`（`didUpdateLocations:`）から呼ばれる `extern "C"`
/// 関数。有効な 1 測位を [`SHARED`] へ push する。`SHARED` が未初期化（`start` が
/// 一度も呼ばれていない）場合は no-op にする（起動前コールバックで panic しない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_location_fix(lat_deg: f64, lon_deg: f64, accuracy_m: f64) {
    if let Some(shared) = SHARED.get() {
        shared.push_fix(fandhe_device::location::Fix {
            latitude_deg: lat_deg,
            longitude_deg: lon_deg,
            accuracy_m,
        });
    }
}

/// `.m` 側の `FandheLocationDelegate`（`didChangeAuthorization:`/`didFailWithError:`）
/// から呼ばれる `extern "C"` 関数。未知のコードは無視し、`SHARED` が未初期化の場合も
/// no-op にする（[`fandhe_rust_location_fix`] と同じ方針）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_location_failure(code: i32) {
    let Some(failure) = failure_from_code(code) else {
        return;
    };
    if let Some(shared) = SHARED.get() {
        shared.push_failure(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_from_code_maps_known_codes() {
        assert_eq!(
            failure_from_code(1),
            Some(LocationFailure::PermissionDenied)
        );
        assert_eq!(failure_from_code(2), Some(LocationFailure::ServiceDisabled));
        assert_eq!(failure_from_code(3), Some(LocationFailure::Failed));
    }

    #[test]
    fn failure_from_code_ignores_unknown_codes() {
        assert_eq!(failure_from_code(0), None);
        assert_eq!(failure_from_code(99), None);
    }

    #[test]
    fn fix_and_failure_are_no_op_before_shared_is_initialized() {
        // `SHARED`（`device/location.rs`）はプロセス内グローバルのため、他テストが
        // 先に `shared_location()` を呼んでいる可能性がある。ここでは「未初期化なら
        // panic しない」という契約のみを検証する（既に初期化済みでも呼び出し自体が
        // panic しないことは変わらない）。
        fandhe_rust_location_fix(1.0, 2.0, 3.0);
        fandhe_rust_location_failure(1);
    }
}
