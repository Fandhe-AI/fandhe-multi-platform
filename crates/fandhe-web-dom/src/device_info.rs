//! 型サンプル「端末・OS 情報」の Web 経路（Issue #1307、親 #1131）。
//!
//! `fandhe_core::screens::device_info_show`（端末依存ゼロ）を
//! [`super::mount::mount_with_effect`] の effect として観測し、ブラウザから読んだ値を
//! `finish_read` へ書き戻す。`fandhe-device` の `DeviceInfo` トレイトは経由しない
//! （`fandhe-device/src/device_info.rs` の module doc どおり、Web は本クレートの担当）。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。
//!
//! # 読み取り方
//!
//! - OS 名: `navigator.userAgentData.platform`。無ければ `navigator.userAgent` から
//!   固定の定数名（macOS / iOS 等）だけを判定する。UA の部分文字列は画面へ出さない
//! - OS の版・機種: `userAgentData.getHighEntropyValues(["platformVersion","model"])`。
//!   加工しない。Windows の `platformVersion` は 13 以上が Windows 11 を表す符号で、
//!   Windows 自体の版番号とは意味が違う。非対応ブラウザ（Safari・Firefox）は不明
//! - メモリ: `navigator.deviceMemory`（GiB）。ブラウザが 2 のべき乗へ丸め上限も付けるため、
//!   実際の搭載量より小さく出ることがある（コア画面に「約」を足せないので、ここと対応表に書く）
//! - 空き容量: 常に不明。`StorageManager.estimate()` はオリジンへの割当の見積りで、
//!   端末の空き容量ではないため呼ばない（嘘の値を出さない）
//!
//! 取れない項目は項目ごとに `None`（不明）。OS 名も含め 5 項目すべて `None` のときだけ
//! 失敗にする（`require_any` と違い OS 名も読み取った値なので判定に含める）。
//! 例外の `message` は画面にも `console` にも出さない（security.md A09）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の取得、不明と 0 と失敗の区別、空き容量を偽らないこと、
//! 失敗を成功値へ畳まないこと、個体識別子を取らないこと。
//! 守らない: OS 間・ブラウザ間の粒度の一致、`deviceMemory` の丸めと上限、UA の偽装
//! （iPadOS の Safari がデスクトップ版 UA で macOS と出る等）、複数タブ間の一致、
//! `getHighEntropyValues` が解決しない場合の期限。

use fandhe_core::screens::device_info_show::{DeviceInfoReading, ReadFailure};
use fandhe_device::device_info::clean_text;

/// 全項目が取れなかったときの固定文言（egui-proxy 側と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "端末情報の読み取り";

/// `userAgentData.platform` を OS 名へ。空と `"Unknown"` は不明。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn os_name_from_platform(raw: &str) -> Option<String> {
    clean_text(raw).filter(|s| s != "Unknown")
}

/// UA 文字列から固定の OS 名だけを判定する。iPhone / iPad / Android は
/// `like Mac OS X` / `Linux` を含むため先に見る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn os_name_from_ua(ua: &str) -> Option<&'static str> {
    const TABLE: [(&str, &str); 8] = [
        ("iPhone", "iOS"),
        ("iPad", "iPadOS"),
        ("Android", "Android"),
        ("CrOS", "ChromeOS"),
        ("Windows", "Windows"),
        ("Macintosh", "macOS"),
        ("Mac OS X", "macOS"),
        ("Linux", "Linux"),
    ];
    TABLE.iter().find(|(k, _)| ua.contains(k)).map(|(_, v)| *v)
}

/// `deviceMemory`（GiB）をバイトへ。有限で正の値だけ。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn memory_bytes_from_gib(gib: Option<f64>) -> Option<u64> {
    gib.filter(|g| g.is_finite() && *g > 0.0)
        .map(|g| (g * (1u64 << 30) as f64).round() as u64)
}

/// ブラウザから読んだ生値を表示用の読み取り結果へ写す純関数。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_raw(
    platform: Option<&str>,
    ua: Option<&str>,
    platform_version: Option<&str>,
    model: Option<&str>,
    device_memory: Option<f64>,
) -> Result<DeviceInfoReading, ReadFailure> {
    let reading = DeviceInfoReading {
        os_name: platform
            .and_then(os_name_from_platform)
            .or_else(|| ua.and_then(os_name_from_ua).map(str::to_string)),
        os_version: platform_version.and_then(clean_text),
        model: model.and_then(clean_text),
        memory_bytes: memory_bytes_from_gib(device_memory),
        free_storage_bytes: None,
    };
    if reading == DeviceInfoReading::default() {
        return Err(ReadFailure::Failed(READ_FAILED.to_string()));
    }
    Ok(reading)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::device_info_show::{
        finish_read, mark_reading, take_request, DeviceInfoShowState, ReadFailure,
    };
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;

    use super::reading_from_raw;
    use crate::mount::Applier;

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に読まない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ読む。
    pub fn device_info_show_effect(
        state: &DeviceInfoShowState,
        applier: &Applier<DeviceInfoShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        let generation = req.generation;
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        applier.apply(mark_reading);
        let Some(window) = web_sys::window() else {
            applier.apply(move |s| finish_read(s, generation, Err(ReadFailure::Unsupported)));
            return;
        };
        let navigator: JsValue = window.navigator().into();
        let ua = get(&navigator, "userAgent").as_string();
        let device_memory = get(&navigator, "deviceMemory").as_f64();
        let uad = get(&navigator, "userAgentData");
        let uad = if uad.is_object() {
            uad
        } else {
            JsValue::UNDEFINED
        };
        let platform = get(&uad, "platform").as_string();
        let high = get(&uad, "getHighEntropyValues");
        let promise = if high.is_function() {
            let hints = Array::of2(&"platformVersion".into(), &"model".into());
            high.unchecked_into::<Function>()
                .call1(&uad, &hints)
                .ok()
                .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
        } else {
            None
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // reject・同期例外・Promise でない戻り値は版と機種を不明にするだけ（全体は失敗にしない）。
            let values = match promise {
                Some(p) => wasm_bindgen_futures::JsFuture::from(p).await.ok(),
                None => None,
            };
            let (version, model) = match &values {
                Some(v) if v.is_object() => (
                    get(v, "platformVersion").as_string(),
                    get(v, "model").as_string(),
                ),
                _ => (None, None),
            };
            let result = reading_from_raw(
                platform.as_deref(),
                ua.as_deref(),
                version.as_deref(),
                model.as_deref(),
                device_memory,
            );
            applier.apply(move |s| finish_read(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::device_info_show_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)";
    const ANDROID: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8)";

    #[test]
    fn platform_wins_over_ua_and_unknown_falls_back() {
        let r = reading_from_raw(Some("macOS"), Some(IPHONE), None, None, None).unwrap();
        assert_eq!(r.os_name.as_deref(), Some("macOS"));
        for p in [Some("Unknown"), Some(""), Some("  "), None] {
            let r = reading_from_raw(p, Some(IPHONE), None, None, None).unwrap();
            assert_eq!(r.os_name.as_deref(), Some("iOS"));
        }
    }

    #[test]
    fn ua_order_and_unknown_ua() {
        assert_eq!(os_name_from_ua(IPHONE), Some("iOS"));
        assert_eq!(os_name_from_ua(ANDROID), Some("Android"));
        assert_eq!(os_name_from_ua("X11; Linux x86_64"), Some("Linux"));
        assert_eq!(os_name_from_ua("Macintosh; Intel Mac OS X"), Some("macOS"));
        assert_eq!(os_name_from_ua("curl/8"), None);
    }

    #[test]
    fn version_and_model_blank_is_none_and_version_unmodified() {
        let r = reading_from_raw(Some("Windows"), None, Some("13.0.0"), Some(" "), None).unwrap();
        assert_eq!(r.os_version.as_deref(), Some("13.0.0"));
        assert_eq!(r.model, None);
    }

    #[test]
    fn memory_conversion() {
        assert_eq!(memory_bytes_from_gib(Some(8.0)), Some(8 << 30));
        assert_eq!(memory_bytes_from_gib(Some(0.25)), Some(1 << 28));
        for bad in [
            Some(0.0),
            Some(-1.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
            None,
        ] {
            assert_eq!(memory_bytes_from_gib(bad), None);
        }
    }

    #[test]
    fn free_storage_is_always_unknown() {
        let r = reading_from_raw(Some("macOS"), None, None, None, Some(8.0)).unwrap();
        assert_eq!(r.free_storage_bytes, None);
    }

    #[test]
    fn all_none_is_failure_but_os_only_is_success() {
        let e = reading_from_raw(None, Some("curl/8"), None, Some(""), Some(0.0));
        assert_eq!(e, Err(ReadFailure::Failed(READ_FAILED.to_string())));
        let r = reading_from_raw(Some("Linux"), None, None, None, None).unwrap();
        assert_eq!(r.os_name.as_deref(), Some("Linux"));
        assert_eq!(r.os_version, None);
    }
}
