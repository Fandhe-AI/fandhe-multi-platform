//! 型サンプル「デジタル身分証の提示要求」の Web 経路（Issue #959、親 #186、共通部分は #958）。
//!
//! `fandhe_core::screens::digital_credentials`（端末依存ゼロ）の要求を [`super::mount::mount_with_effect`]
//! の effect として観測し、`navigator.credentials.get({ digital })` で固定の要求（`openid4vp-v1-unsigned`・
//! mDL の `age_over_18`）を 1 件出して、ウォレットが応答したかと種別だけを `finish` へ書き戻す。
//! **本人確認・年齢確認の根拠にしてはならない**（応答の署名・発行者・nonce は検証しない）。
//!
//! # ブロッキング trait を通らない理由
//!
//! `fandhe_device::digital_credentials` に trait は無い。`passkey.rs` と同じく Web は effect で
//! 非同期に処理し、定数と純関数（`request_json`・`outcome_from_dom_error`・`outcome_from_response`）だけを共有する。
//!
//! # ライフサイクル
//!
//! `get` はクリックから続く同期チェーンの中で呼ぶ（呼ぶ前に `await` を挟まない。transient activation のため）。
//! 要求ごとに `AbortController` を持ち、マウント破棄（`release_mount`）と `pagehide`
//! （bfcache 退避を含む）で `abort()` する。アプリ側の期限は置かない（利用者が OS の画面を閉じるまで待つ）。
//! 同じ `(mount_id, generation)` は二度発送しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 契約は `fandhe_device::digital_credentials` の doc を参照。応答の中身（`data`）・例外の `message`・nonce は
//! 画面にも `console` にも storage にも出さない。読むのは例外の `name`、応答の `protocol` と `data` の有無だけ。

use fandhe_core::screens::digital_credentials::CoreOutcome;
use fandhe_device::digital_credentials::CredentialOutcome;
use fandhe_device::DeviceError;

/// 失敗時の固定文言。内部名を含めない（`fandhe-egui-proxy` の文言と揃える）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_TEXT: &str = "身分証の提示要求を完了できませんでした";

/// `DeviceError` を画面用の結果へ写す。`fandhe-egui-proxy/src/device/digital_credentials.rs` の
/// `core_outcome` と同期する（同クレートに依存しないため同じ写像をここに置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_outcome(result: Result<CredentialOutcome, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(CredentialOutcome::Responded) => Ok(CoreOutcome::Responded),
        Ok(CredentialOutcome::NotShared) => Ok(CoreOutcome::NotShared),
        Ok(CredentialOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::digital_credentials::{
        finish, mark_running, take_request, DigitalCredentialsState,
    };
    use fandhe_device::digital_credentials::{
        outcome_from_dom_error, outcome_from_response, request_json, CredentialOutcome, FEATURE,
        NONCE_LEN, PROTOCOL,
    };
    use fandhe_device::DeviceError;
    use js_sys::{Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{AbortController, CredentialRequestOptions};

    use super::to_outcome;
    use crate::mount::Applier;

    /// 発送済みの（世代, 取り消し用コントローラ）。同じ世代を二重に発送しない。
    type Entry = (u64, Option<AbortController>);

    /// `pagehide` 購読クロージャの型。
    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static DISPATCHED: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        /// `pagehide` 購読（初回の要求で 1 度だけ登録し、ページの寿命まで持つ）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> =
            const { RefCell::new(None) };
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に飛行中の要求を取り消し、記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some((_, Some(ctrl))) = DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id)) {
            ctrl.abort();
        }
    }

    /// 全マウントの飛行中の要求を取り消す（`pagehide`）。記録は残す（復帰後の二重発送防止）。
    fn abort_all() {
        let ctrls: Vec<AbortController> = DISPATCHED.with(|d| {
            d.borrow_mut()
                .values_mut()
                .filter_map(|e| e.1.take())
                .collect()
        });
        for c in ctrls {
            c.abort();
        }
    }

    fn register_pagehide_once(w: &web_sys::Window) {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return;
            }
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(|_e: web_sys::Event| abort_all());
            if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
                .is_ok()
            {
                *cell.borrow_mut() = Some(closure);
            }
        });
    }

    fn get(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    /// 例外の `name` だけを結果へ写す（`message` は読まない）。無ければ `Internal` になる。
    fn outcome_of_error(err: &JsValue) -> Result<CredentialOutcome, DeviceError> {
        let name = Reflect::get(err, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        outcome_from_dom_error(&name)
    }

    /// 完了した世代のコントローラを外す（世代が違えば新しい要求のものなので触らない）。
    fn clear(mount_id: u64, generation: u64) {
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                if e.0 == generation {
                    e.1 = None;
                }
            }
        });
    }

    /// 事前判定。使える環境でなければ `Unsupported`、判定自体が例外なら `Internal`。
    fn precheck(window: &web_sys::Window) -> Result<(), DeviceError> {
        let unsupported = DeviceError::Unsupported(FEATURE);
        let nav: JsValue = window.navigator().into();
        let win: JsValue = window.clone().into();
        if !window.is_secure_context() || get(&nav, "credentials").is_none() {
            return Err(unsupported);
        }
        let Some(dc) = get(&win, "DigitalCredential").filter(|v| v.is_function()) else {
            return Err(unsupported);
        };
        let Some(allows) =
            get(&dc, "userAgentAllowsProtocol").and_then(|v| v.dyn_into::<Function>().ok())
        else {
            return Err(unsupported);
        };
        match allows.call1(&dc, &JsValue::from_str(PROTOCOL)) {
            Ok(v) if v.as_bool() == Some(true) => Ok(()),
            Ok(_) => Err(unsupported),
            Err(_) => Err(DeviceError::Internal("digital_credentials failed")),
        }
    }

    /// 解決値から (protocol, data の有無) を取り出す。`DigitalCredential` 以外は (None, false)。
    /// `data` の中身は読まない。
    fn inspect(value: &JsValue) -> (Option<String>, bool) {
        let Some(win) = web_sys::window() else {
            return (None, false);
        };
        let win: JsValue = win.into();
        let is_dc = get(&win, "DigitalCredential")
            .and_then(|dc| get(&dc, "prototype"))
            .and_then(|p| p.dyn_into::<js_sys::Object>().ok())
            .zip(value.dyn_ref::<js_sys::Object>())
            .is_some_and(|(proto, obj)| proto.is_prototype_of(obj));
        if !is_dc {
            return (None, false);
        }
        let protocol = get(value, "protocol").and_then(|v| v.as_string());
        (protocol, get(value, "data").is_some())
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ OS の UI を依頼する。
    pub fn digital_credentials_effect(
        state: &DigitalCredentialsState,
        applier: &Applier<DigitalCredentialsState>,
    ) {
        let Some(view) = take_request(state) else {
            return;
        };
        let generation = view.generation;
        let mount_id = applier.mount_id();
        let fresh = DISPATCHED.with(|d| {
            let mut d = d.borrow_mut();
            if d.get(&mount_id).map(|e| e.0) == Some(generation) {
                return false;
            }
            d.insert(mount_id, (generation, None));
            true
        });
        if !fresh {
            return;
        }
        applier.apply(mark_running);

        let done = move |applier: &Applier<DigitalCredentialsState>,
                         r: Result<CredentialOutcome, DeviceError>| {
            let out = to_outcome(r);
            applier.apply(move |s| finish(s, generation, out));
        };
        let internal = DeviceError::Internal("digital_credentials failed");

        let Some(window) = web_sys::window() else {
            return done(applier, Err(DeviceError::Unsupported(FEATURE)));
        };
        if let Err(e) = precheck(&window) {
            return done(applier, Err(e));
        }

        let mut nonce = [0u8; NONCE_LEN];
        let random_ok = window
            .crypto()
            .ok()
            .is_some_and(|c| c.get_random_values_with_u8_array(&mut nonce).is_ok());
        if !random_ok {
            return done(applier, Err(internal));
        }
        let Ok(parsed) = js_sys::JSON::parse(&request_json(&nonce)) else {
            return done(applier, Err(internal));
        };
        let Ok(ctrl) = AbortController::new() else {
            return done(applier, Err(internal));
        };
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                e.1 = Some(ctrl.clone());
            }
        });
        register_pagehide_once(&window);

        let options = CredentialRequestOptions::new();
        if Reflect::set(&options, &JsValue::from_str("digital"), &parsed).is_err() {
            clear(mount_id, generation);
            return done(applier, Err(internal));
        }
        options.set_signal(&ctrl.signal());

        // transient activation を保つため、ここまでに await を挟まない。
        let promise = match window.navigator().credentials().get_with_options(&options) {
            Ok(p) => p,
            Err(err) => {
                clear(mount_id, generation);
                return done(applier, outcome_of_error(&err));
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = match JsFuture::from(promise).await {
                Ok(v) => {
                    let (protocol, has_data) = inspect(&v);
                    outcome_from_response(protocol.as_deref(), has_data)
                }
                Err(err) => outcome_of_error(&err),
            };
            clear(mount_id, generation);
            done(&applier, result);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::digital_credentials_effect;

/// マウント破棄時に飛行中の要求を取り消す。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::digital_credentials::{
        outcome_from_dom_error, outcome_from_response, FEATURE, PROTOCOL,
    };

    #[test]
    fn maps_every_arm() {
        assert_eq!(
            to_outcome(Ok(CredentialOutcome::Responded)),
            Ok(CoreOutcome::Responded)
        );
        assert_eq!(
            to_outcome(Ok(CredentialOutcome::NotShared)),
            Ok(CoreOutcome::NotShared)
        );
        assert_eq!(
            to_outcome(Ok(CredentialOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported(FEATURE))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("x"))),
            Err(FAILED_TEXT.to_string())
        );
    }

    #[test]
    fn failed_text_hides_internal_names() {
        assert!(!FAILED_TEXT.contains("digital_credentials"));
    }

    #[test]
    fn no_other_dom_error_name_is_a_success() {
        for n in ["SecurityError", "NotSupportedError", "TypeError", ""] {
            assert!(to_outcome(outcome_from_dom_error(n)).is_err(), "{n}");
        }
        assert_eq!(
            to_outcome(outcome_from_dom_error("NotAllowedError")),
            Ok(CoreOutcome::NotShared)
        );
    }

    #[test]
    fn bad_responses_are_failures() {
        assert!(to_outcome(outcome_from_response(None, true)).is_err());
        assert!(to_outcome(outcome_from_response(Some("other"), true)).is_err());
        assert!(to_outcome(outcome_from_response(Some(PROTOCOL), false)).is_err());
        assert_eq!(
            to_outcome(outcome_from_response(Some(PROTOCOL), true)),
            Ok(CoreOutcome::Responded)
        );
    }
}
