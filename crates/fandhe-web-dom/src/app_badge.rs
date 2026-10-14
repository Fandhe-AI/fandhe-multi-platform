//! 型サンプル「バッジの設定」の Web 経路（Issue #1363、親 #1141）。
//!
//! `fandhe_core::screens::app_badge_set`（端末依存ゼロ）を [`super::mount::mount_with_effect`]
//! の effect として観測し、Badging API の `navigator.setAppBadge(n)` を呼んで結果を
//! `finish_set` へ書き戻す。`fandhe-device` の `AppBadge` トレイトは経由しない
//! （`fandhe-device/src/app_badge.rs` の module doc どおり、Web は本クレートの担当）。
//! 値の範囲検証は `BadgeValue::new` に一本化し、数値を JS へ渡す前に行う。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。
//!
//! # 結果の写像
//!
//! - 範囲外（10000 以上）: API を呼ばず失敗
//! - `setAppBadge` が関数でない（非 secure context・非対応ブラウザ）: `Unsupported`
//! - resolve: 0 は `Cleared`、それ以外は `Submitted`（ブラウザへ渡した、まで。PWA 未導入時の
//!   表示有無は推定しない）
//! - reject `NotAllowedError`（通知の許可なし）: `Denied`
//! - reject `SecurityError`（cross-origin iframe）: `Unsupported`
//! - その他の reject・同期例外: 固定文言の失敗。例外の `message` は画面にも `console` にも
//!   出さない（security.md A09）
//!
//! `Cancelled` は iOS 専用のため Web では返さない。消去は `clearAppBadge()` ではなく
//! `setAppBadge(0)`（仕様上同じ）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（発送済み世代の記録と `finish_set` の世代照合）、
//! 失敗を成功へ畳まないこと、マウント破棄時の記録解放（`release_mount`）。
//! 守らない: PWA 未インストール時の無表示、UA による 99+ 等への丸め、複数タブ間の一致、
//! OS や利用者がバッジを消したときの表示値との食い違い、Promise が解決しない場合の期限。

use fandhe_core::screens::app_badge_set::{CoreOutcome, OUT_OF_RANGE_MESSAGE};
use fandhe_device::app_badge::BadgeValue;

/// 想定外の失敗の固定文言（例外の内容は含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const SET_FAILED: &str = "ブラウザのバッジ設定に失敗しました";

/// 範囲検証。`BadgeValue::new` を通し、範囲外は画面へ返す失敗文言にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn check_value(value: u32) -> Result<u32, String> {
    BadgeValue::new(value)
        .map(BadgeValue::get)
        .map_err(|_| OUT_OF_RANGE_MESSAGE.to_string())
}

/// Promise が resolve したときの結果。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn outcome_from_resolve(value: u32) -> Result<CoreOutcome, String> {
    Ok(if value == 0 {
        CoreOutcome::Cleared
    } else {
        CoreOutcome::Submitted
    })
}

/// Promise が reject したときの結果。`name` だけを見る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn outcome_from_reject(name: Option<&str>) -> Result<CoreOutcome, String> {
    match name {
        Some("NotAllowedError") => Ok(CoreOutcome::Denied),
        Some("SecurityError") => Ok(CoreOutcome::Unsupported),
        _ => Err(SET_FAILED.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::app_badge_set::{
        finish_set, mark_setting, take_request, AppBadgeSetState, CoreOutcome,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;

    use super::{check_value, outcome_from_reject, outcome_from_resolve, SET_FAILED};
    use crate::mount::Applier;

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に発送しない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ発送する。
    pub fn app_badge_set_effect(state: &AppBadgeSetState, applier: &Applier<AppBadgeSetState>) {
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
        applier.apply(mark_setting);
        let finish = |applier: &Applier<AppBadgeSetState>, r: Result<CoreOutcome, String>| {
            applier.apply(move |s| finish_set(s, generation, r));
        };
        let value = match check_value(req.value) {
            Ok(v) => v,
            Err(m) => return finish(applier, Err(m)),
        };
        let Some(window) = web_sys::window() else {
            return finish(applier, Ok(CoreOutcome::Unsupported));
        };
        let navigator: JsValue = window.navigator().into();
        let f = Reflect::get(&navigator, &JsValue::from_str("setAppBadge"))
            .unwrap_or(JsValue::UNDEFINED);
        if !f.is_function() {
            return finish(applier, Ok(CoreOutcome::Unsupported));
        }
        let promise = f
            .unchecked_into::<Function>()
            .call1(&navigator, &JsValue::from_f64(f64::from(value)))
            .ok()
            .and_then(|p| p.dyn_into::<Promise>().ok());
        let Some(promise) = promise else {
            return finish(applier, Err(SET_FAILED.to_string()));
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(_) => outcome_from_resolve(value),
                Err(e) => {
                    let name = Reflect::get(&e, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|n| n.as_string());
                    outcome_from_reject(name.as_deref())
                }
            };
            applier.apply(move |s| finish_set(s, generation, result));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::app_badge_set_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_checked_before_api() {
        assert_eq!(check_value(9999), Ok(9999));
        assert_eq!(check_value(10000), Err(OUT_OF_RANGE_MESSAGE.to_string()));
    }

    #[test]
    fn resolve_maps_zero_to_cleared() {
        assert_eq!(outcome_from_resolve(0), Ok(CoreOutcome::Cleared));
        assert_eq!(outcome_from_resolve(5), Ok(CoreOutcome::Submitted));
    }

    #[test]
    fn reject_maps_by_name_and_never_to_success() {
        assert_eq!(
            outcome_from_reject(Some("NotAllowedError")),
            Ok(CoreOutcome::Denied)
        );
        assert_eq!(
            outcome_from_reject(Some("SecurityError")),
            Ok(CoreOutcome::Unsupported)
        );
        for n in [Some("InvalidStateError"), Some("x"), None] {
            assert_eq!(outcome_from_reject(n), Err(SET_FAILED.to_string()));
        }
    }
}
