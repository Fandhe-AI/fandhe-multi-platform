//! 型サンプル「USB・HID 機器」の Web 経路（親 #112、Issue #508）。
//!
//! `fandhe_core::screens::usb_device_show`（端末依存ゼロ、`ListMode::BrowserPicker`）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、WebUSB の
//! `navigator.usb.requestDevice({ filters: [] })`（1 台）/ WebHID の
//! `navigator.hid.requestDevice({ filters: [] })`（`HIDDevice` の配列）で利用者に機器を選ばせて
//! 一覧として書き戻す。呼び出し元は `demo.rs` の `mount_usb_device_show_screen`。契約の SSOT は
//! `fandhe_device::usb_device`（正規化は `normalize`）。Web は `UsbDevices` トレイトを通らない
//! （`bluetooth_le.rs` と同じ理由）ため、本モジュールが DOM レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約
//!
//! - **user activation 必須**: `requestDevice()` はボタン `click` の同期チェーン内で呼ぶ。
//!   この呼び出しより前に `await` を挟まない。
//! - **secure context・Chromium 系のみ**: `navigator.usb` / `navigator.hid` が無い、または
//!   secure context 外は `Unsupported`。Permissions Policy の `usb` / `hid` で禁止された場合は
//!   `SecurityError`（失敗として見せる）。
//! - **web-sys の unstable API を避ける**: `Reflect` / `Function` / `Promise` だけで呼ぶ。
//!
//! # 三値の報告先
//!
//! - 成功: 選ばれた機器を `Ok`。VID/PID が u16 の整数でなければ偽の ID で成功にせず失敗にする。
//! - キャンセル: WebUSB の `NotFoundError`（ダイアログを閉じた）と、WebHID の空配列
//!   （ダイアログを閉じると空で resolve する）は `Cancelled`。0 件の成功にしない。
//! - 失敗: それ以外の例外は固定文言の `Failed`。
//!
//! # 取らないもの・守る範囲
//!
//! `serialNumber` などの一意な識別子は読まない。`getDevices()` で許可済みの機器を黙って列挙せず、
//! 機器は open しない。守る: 1 タブ内の要求と完了の順序（`finish_read` の世代照合）・非対応や失敗を
//! 成功に見せない・キャンセルを失敗に見せない。守らない: Chromium 系以外・抜き差しへの追従・
//! 選択ダイアログの期限（ページ側から閉じられない）・保護クラスの扱い（Chrome 任せ）。
//!
//! 二重実行防止は不要（`bluetooth_le.rs` と同じ判断）: `mark_reading` 以降は `take_request` が
//! `None` を返し、`Reading` 中のボタンは no-op。機器の ID・名前・例外の `message` は `console` へ
//! 出さない（security.md A09）。

use fandhe_core::screens::usb_device_show::{ReadFailure, UsbDeviceRow};
use fandhe_device::usb_device::{normalize, UsbDeviceEntry};

/// 失敗時の固定文言（egui-proxy 側と同じ）。例外の `message` は使わない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_TEXT: &str = "USB 機器の読み取り";

/// 例外の `name` を画面の失敗種別へ写す純関数（native からもテストできるよう gate の外）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_name(name: Option<&str>) -> ReadFailure {
    match name {
        Some("NotFoundError") => ReadFailure::Cancelled,
        _ => ReadFailure::Failed(FAILED_TEXT.to_string()),
    }
}

/// JS の数値を u16 の ID へ。有限の整数かつ 0..=0xFFFF のときだけ `Some`（変換より前に範囲を検証する）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn id_from_js(v: Option<f64>) -> Option<u16> {
    let v = v?;
    (v.is_finite() && v.fract() == 0.0 && (0.0..=65535.0).contains(&v)).then_some(v as u16)
}

/// 選ばれた機器（vendorId・productId・productName）を画面の行へ写す。
/// 空は `Cancelled`（WebHID の閉じる操作）、ID が 1 件でも不正なら `Failed`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn rows_from_picked(
    raw: Vec<(Option<f64>, Option<f64>, Option<String>)>,
) -> Result<Vec<UsbDeviceRow>, ReadFailure> {
    if raw.is_empty() {
        return Err(ReadFailure::Cancelled);
    }
    let mut entries = Vec::with_capacity(raw.len());
    for (v, p, name) in raw {
        let (Some(vendor_id), Some(product_id)) = (id_from_js(v), id_from_js(p)) else {
            return Err(ReadFailure::Failed(FAILED_TEXT.to_string()));
        };
        entries.push(UsbDeviceEntry {
            vendor_id,
            product_id,
            name,
        });
    }
    Ok(normalize(entries)
        .into_iter()
        .map(|e| UsbDeviceRow {
            vendor_id: e.vendor_id,
            product_id: e.product_id,
            name: e.name,
        })
        .collect())
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::usb_device_show::{
        finish_read, mark_reading, take_request, ReadFailure, ReadSource, UsbDeviceShowState,
    };
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{failure_from_name, rows_from_picked, FAILED_TEXT};
    use crate::mount::Applier;

    fn get(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    fn error_name(err: &JsValue) -> Option<String> {
        get(err, "name").and_then(|v| v.as_string())
    }

    /// `vendorId` / `productId` / `productName` だけを読む（`serialNumber` などは読まない）。
    fn read_device(d: &JsValue) -> (Option<f64>, Option<f64>, Option<String>) {
        (
            get(d, "vendorId").and_then(|v| v.as_f64()),
            get(d, "productId").and_then(|v| v.as_f64()),
            get(d, "productName").and_then(|v| v.as_string()),
        )
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない。
    pub fn usb_device_show_effect(
        state: &UsbDeviceShowState,
        applier: &Applier<UsbDeviceShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        applier.apply(mark_reading);
        let generation = req.generation;
        let finish = |applier: &Applier<UsbDeviceShowState>, result| {
            applier.apply(move |s| finish_read(s, generation, result));
        };
        let failed = || ReadFailure::Failed(FAILED_TEXT.to_string());

        let key = match req.source {
            ReadSource::WebUsb => "usb",
            ReadSource::WebHid => "hid",
            // Web は browser_picker でしかマウントしないので防御の分岐。
            ReadSource::System => {
                finish(applier, Err(ReadFailure::Unsupported));
                return;
            }
        };
        let Some(window) = web_sys::window() else {
            finish(applier, Err(ReadFailure::Unsupported));
            return;
        };
        let navigator: JsValue = window.navigator().into();
        let target = get(&navigator, key);
        let request_device = target
            .as_ref()
            .and_then(|t| get(t, "requestDevice"))
            .and_then(|f| f.dyn_into::<Function>().ok());
        let (Some(target), Some(request_device), true) =
            (target, request_device, window.is_secure_context())
        else {
            finish(applier, Err(ReadFailure::Unsupported));
            return;
        };

        let options = Object::new();
        if Reflect::set(&options, &JsValue::from_str("filters"), &Array::new()).is_err() {
            finish(applier, Err(failed()));
            return;
        }

        // click の同期チェーン内での呼び出し（user activation 要件）。ここより前に await しない。
        let promise = match request_device.call1(&target, &options) {
            Ok(p) => match p.dyn_into::<Promise>() {
                Ok(p) => p,
                Err(_) => {
                    finish(applier, Err(failed()));
                    return;
                }
            },
            Err(err) => {
                finish(applier, Err(failure_from_name(error_name(&err).as_deref())));
                return;
            }
        };

        let hid = req.source == ReadSource::WebHid;
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(v) => {
                    // WebHID は配列、WebUSB は単一の USBDevice。
                    let raw = if hid {
                        Array::from(&v).iter().map(|d| read_device(&d)).collect()
                    } else {
                        vec![read_device(&v)]
                    };
                    rows_from_picked(raw)
                }
                Err(err) => Err(failure_from_name(error_name(&err).as_deref())),
            };
            applier.apply(move |s| finish_read(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::usb_device_show_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_mapping() {
        assert_eq!(
            failure_from_name(Some("NotFoundError")),
            ReadFailure::Cancelled
        );
        for n in [Some("SecurityError"), Some("X"), None] {
            assert!(matches!(failure_from_name(n), ReadFailure::Failed(_)));
        }
    }

    #[test]
    fn id_range_checked() {
        assert_eq!(id_from_js(Some(0.0)), Some(0));
        assert_eq!(id_from_js(Some(65535.0)), Some(65535));
        for v in [Some(-1.0), Some(65536.0), Some(1.5), Some(f64::NAN), None] {
            assert_eq!(id_from_js(v), None);
        }
    }

    #[test]
    fn picked_rows() {
        let ok = |n: &str| (Some(1.0), Some(2.0), Some(n.to_string()));
        assert_eq!(rows_from_picked(vec![]), Err(ReadFailure::Cancelled));
        assert!(matches!(
            rows_from_picked(vec![(Some(70000.0), Some(1.0), None)]),
            Err(ReadFailure::Failed(_))
        ));
        let rows = rows_from_picked(vec![
            ok("a\u{0}b "),
            ok("a\u{0}b "),
            (Some(1.0), Some(2.0), Some(String::new())),
        ])
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].name, None);
        assert!(!rows[0].name.as_deref().unwrap().contains('\u{0}'));
        assert_eq!(rows_from_picked(vec![ok("x"); 65]).unwrap().len(), 64);
    }
}
