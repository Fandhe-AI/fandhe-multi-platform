//! Phase 11（端末機能の拡充ループ、親 #1059）型サンプル「BLE スキャン」の Web 経路（Issue #1063）。
//!
//! `fandhe_core::screens::ble_scan`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Web Bluetooth の
//! `navigator.bluetooth.requestDevice({ acceptAllDevices: true })` で利用者に 1 台を選ばせ、
//! 1 件の一覧として書き戻す。契約の SSOT は `fandhe_device::bluetooth_le` のモジュール doc
//! （Web は「利用者が 1 台選ぶ形」・RSSI は `None`）。Web は `fandhe-device` の
//! `BluetoothLe` トレイトを通らない（`share.rs` と同じ理由）ため、本モジュールが DOM
//! レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約
//!
//! - **user activation 必須**: `requestDevice()` はボタン `click` の同期チェーン内で呼ぶ
//!   （`share.rs` と同じ構造）。`await` はこの呼び出しより前に挟まない。
//! - **secure context・Chrome 系のみ**: `navigator.bluetooth` / `requestDevice` が無い、または
//!   secure context 外は `Unsupported`。型に現れない JS 側の実態を境界で検証する（security.md A03）。
//! - **web-sys の unstable API を避ける**: `Bluetooth` 系の型は `--cfg=web_sys_unstable_apis`
//!   が要るため、`Reflect` / `Function` だけで呼ぶ（Cargo.toml 変更なし）。
//!
//! # 三値の報告先
//!
//! - 成功: 選ばれた 1 台を `Ok`（id は `BluetoothDevice.id`、RSSI は `None`）。id が取れなければ
//!   偽の id で成功にせず失敗にする。
//! - キャンセル: `NotFoundError`（選択ダイアログを閉じた）は `ScanError::Cancelled`。ただし
//!   `getAvailability()` が `false`（アダプタ無し。Chrome は即 `NotFoundError` を返す）なら
//!   `Unsupported`。
//! - 失敗: それ以外の例外は `Other`（固定文言）。
//!
//! # 守備範囲
//!
//! 守る: 1 タブ内の要求と完了の順序（`finish_scan` の世代照合）・未対応や失敗を成功に見せない・
//! キャンセルを失敗に見せない。守らない: Chrome 系以外・RSSI・複数台選択・選択ダイアログの期限
//! （ページ側から閉じられない。`SCAN_DURATION_MS` は Web に当てはまらず、ボタン文言の
//! 「10 秒」とずれる既知の制約）・Bluetooth オフの区別（Chrome はダイアログ内で ON を促す）・
//! `PermissionDenied` の区別（例外名を確認できていないため `Other`）。
//!
//! 二重実行防止は不要（`share.rs` と同じ判断）: `mark_scanning` 以降は `take_request` が
//! `None` を返し、`Scanning` 中のボタンは no-op。
//!
//! 機器の id・名前・例外の `message` は `console` 出力・送信を行わない（security.md A09）。

use fandhe_core::screens::ble_scan::{BleRow, ScanError};
use fandhe_device::bluetooth_le::{normalize_scan, BleDevice};

/// `SecurityError`（user activation 無し・非 secure context）のときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SECURITY_FAILED: &str =
    "Bluetooth の呼び出しが許可されませんでした（ボタン操作・https が必要です）";
/// 種別を特定できない失敗の汎用案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const REQUEST_FAILED: &str = "Web Bluetooth の呼び出しに失敗しました";

/// 例外の `name` を画面の失敗種別へ写す純関数（native からもテストできるよう gate の外）。
/// `message` は受け取らない（security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn error_from_name(name: Option<&str>) -> ScanError {
    match name {
        Some("NotFoundError") => ScanError::Cancelled,
        Some("SecurityError") => ScanError::Other(SECURITY_FAILED.to_string()),
        _ => ScanError::Other(REQUEST_FAILED.to_string()),
    }
}

/// 選ばれた 1 台を画面の行へ写す。契約の SSOT（`normalize_scan`）を通し、RSSI は常に `None`。
/// 名前は入力どおり（trim しない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn rows_from_picked(id: String, name: Option<String>) -> Vec<BleRow> {
    normalize_scan([BleDevice {
        id,
        name,
        rssi_dbm: None,
    }])
    .into_iter()
    .map(|d| BleRow {
        id: d.id,
        name: d.name,
        rssi_dbm: d.rssi_dbm,
    })
    .collect()
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::ble_scan::{
        finish_scan, mark_scanning, take_request, BleScanState, ScanError,
    };
    use js_sys::{Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{error_from_name, rows_from_picked, REQUEST_FAILED};
    use crate::mount::Applier;

    fn get(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    fn error_name(err: &JsValue) -> Option<String> {
        get(err, "name").and_then(|v| v.as_string())
    }

    /// `NotFoundError` 時だけ、アダプタが無い（`getAvailability()` が `false`）かを確かめる。
    /// 確認不能・未実装は利用者キャンセル扱い。選択要求より後に呼ぶので user activation に影響しない。
    async fn adapter_missing(bluetooth: &JsValue) -> bool {
        let Some(f) = get(bluetooth, "getAvailability").and_then(|v| v.dyn_into::<Function>().ok())
        else {
            return false;
        };
        let Ok(promise) = f.call0(bluetooth).and_then(|p| p.dyn_into::<Promise>()) else {
            return false;
        };
        matches!(JsFuture::from(promise).await, Ok(v) if v.as_bool() == Some(false))
    }

    /// `mount_with_effect` へ渡す effect。`ble_scan` 画面のスキャン要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn ble_scan_effect(state: &BleScanState, applier: &Applier<BleScanState>) {
        let Some(request) = take_request(state) else {
            return;
        };
        applier.apply(mark_scanning);
        let generation = request.generation;

        let finish = |applier: &Applier<BleScanState>, result| {
            applier.apply(move |s| finish_scan(s, generation, result));
        };

        let Some(window) = web_sys::window() else {
            finish(applier, Err(ScanError::Unsupported));
            return;
        };
        let navigator: JsValue = window.navigator().into();
        let bluetooth = get(&navigator, "bluetooth");
        let request_device = bluetooth
            .as_ref()
            .and_then(|b| get(b, "requestDevice"))
            .and_then(|f| f.dyn_into::<Function>().ok());
        let (Some(bluetooth), Some(request_device), true) =
            (bluetooth, request_device, window.is_secure_context())
        else {
            finish(applier, Err(ScanError::Unsupported));
            return;
        };

        let options = Object::new();
        if Reflect::set(
            &options,
            &JsValue::from_str("acceptAllDevices"),
            &JsValue::TRUE,
        )
        .is_err()
        {
            finish(applier, Err(ScanError::Other(REQUEST_FAILED.to_string())));
            return;
        }

        // click の同期チェーン内での呼び出し（user activation 要件）。ここより前に await しない。
        let promise = match request_device.call1(&bluetooth, &options) {
            Ok(p) => match p.dyn_into::<Promise>() {
                Ok(p) => p,
                Err(_) => {
                    finish(applier, Err(ScanError::Other(REQUEST_FAILED.to_string())));
                    return;
                }
            },
            Err(err) => {
                finish(applier, Err(error_from_name(error_name(&err).as_deref())));
                return;
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(device) => match get(&device, "id").and_then(|v| v.as_string()) {
                    // id が取れなければ偽の id で成功にしない（契約表 6）。
                    None => Err(ScanError::Other(REQUEST_FAILED.to_string())),
                    Some(id) => {
                        let name = get(&device, "name").and_then(|v| v.as_string());
                        Ok(rows_from_picked(id, name))
                    }
                },
                Err(err) => match error_from_name(error_name(&err).as_deref()) {
                    ScanError::Cancelled if adapter_missing(&bluetooth).await => {
                        Err(ScanError::Unsupported)
                    }
                    e => Err(e),
                },
            };
            applier.apply(move |s| finish_scan(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::ble_scan_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_is_cancelled() {
        assert_eq!(error_from_name(Some("NotFoundError")), ScanError::Cancelled);
    }

    #[test]
    fn security_and_unknown_are_failures_not_cancelled() {
        for name in [Some("SecurityError"), Some("Other"), None] {
            assert!(matches!(error_from_name(name), ScanError::Other(_)));
        }
    }

    #[test]
    fn picked_row_has_no_rssi_and_keeps_name_untrimmed() {
        let rows = rows_from_picked("id1".into(), Some(" dev ".into()));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "id1");
        assert_eq!(rows[0].name.as_deref(), Some(" dev "));
        assert_eq!(rows[0].rssi_dbm, None);
        assert_eq!(rows_from_picked("id2".into(), None)[0].name, None);
    }
}
