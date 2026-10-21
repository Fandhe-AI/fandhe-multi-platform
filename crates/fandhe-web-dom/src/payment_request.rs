//! 型サンプル「物・サービスの決済」の Web 経路（Issue #542、親 #114、共通部分は #541）。
//!
//! `fandhe_core::screens::payment_request`（端末依存ゼロ）の要求を [`super::mount::mount_with_effect`]
//! の effect として観測し、Payment Request API（Google Pay、TEST 環境）で決済シートを出して結果を
//! `finish` へ書き戻す。**実決済はしない**。
//!
//! # `PaymentSheet` トレイトを通らない理由
//!
//! `fandhe_device::payment_request::PaymentSheet` はブロッキングで、wasm のメインスレッドでは呼べない。
//! `passkey.rs` と同じく Web は effect で非同期に処理し、`fandhe_device::payment_request` の
//! const（`METHOD_DATA_JSON`・`DETAILS_JSON`）と `outcome_from_dom_error` だけを共有する。
//!
//! # 課金を起こさない設計
//!
//! - methodData / details は const を `JSON.parse` して渡すだけで、他の経路から組み立てない
//! - `response.details`（トークン）・`toJSON()`・`methodName` は読まない。`complete("success")` だけを呼ぶ。
//!   `complete()` が失敗したら `Failed`（`TestAuthorized` にしない）
//! - 例外は `name` だけを見る。`message`・トークンは画面にも `console` にも storage にもネットワークにも出さない
//!
//! # 結果の写像（三値）
//!
//! - 承認され `complete` も成功: `TestAuthorized`
//! - `AbortError`（利用者の取り消し・`abort()`。区別できない）: `Cancelled`
//! - secure context 外・`PaymentRequest` 無し（Firefox 等）・`NotSupportedError`・`SecurityError`・
//!   `canMakePayment()` が false: `Unsupported`
//! - それ以外の例外・`complete` の失敗: 固定文言の失敗
//!
//! # ライフサイクル
//!
//! `show()` はクリックから続く同期処理の中で呼ぶ（呼ぶ前に `await` を挟まない）。マウント破棄
//! （`release_mount`）と `pagehide`（bfcache 退避を含む）で `abort()` し、`AbortError` → `Cancelled` で
//! `finish` させる。同じ `(mount_id, generation)` は二度発送しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 ページ内の要求と完了の順序（世代照合）、嘘の成功の禁止、課金できるトークンを作らないこと、
//! マウント破棄・ページ離脱での取り消し。守らない: 複数タブ、利用者の取り消しと `abort()` の区別、
//! OS 側の確定との時間差（`fandhe_device::payment_request` の doc と同じ）。

use fandhe_core::screens::payment_request::CoreOutcome;
use fandhe_device::payment_request::PaymentOutcome;
use fandhe_device::DeviceError;

/// 失敗の固定文言（内部名・例外の内容は含めない）。
const FAILED_TEXT: &str = "決済シートを完了できませんでした";

/// `fandhe-egui-proxy/src/device/payment_request.rs` の `check_outcome` と同期する
/// （同クレートに依存しないため同じ写像をここに置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn check_outcome(result: Result<bool, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(true) => Ok(CoreOutcome::Available),
        Ok(false) | Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

/// 同 `pay_outcome` と同期する。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn pay_outcome(result: Result<PaymentOutcome, DeviceError>) -> Result<CoreOutcome, String> {
    match result {
        Ok(PaymentOutcome::TestAuthorized) => Ok(CoreOutcome::TestAuthorized),
        Ok(PaymentOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Ok(PaymentOutcome::Failed) | Err(_) => Err(FAILED_TEXT.to_string()),
    }
}

/// `canMakePayment()` 側の例外を `outcome_from_dom_error` の結果から bool 系へ寄せる。
/// 結果が返る（取り消し・失敗）ものは調べ損ねなので `Internal`（失敗文言）にし、成功へ畳まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn check_from_error(result: Result<PaymentOutcome, DeviceError>) -> Result<bool, DeviceError> {
    match result {
        Ok(_) => Err(DeviceError::Internal("payment_request failed")),
        Err(e) => Err(e),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::payment_request::{
        finish, mark_running, take_request, CoreOutcome, PaymentAction, PaymentRequestState,
    };
    use fandhe_device::payment_request::{
        outcome_from_dom_error, PaymentOutcome, DETAILS_JSON, METHOD_DATA_JSON,
    };
    use fandhe_device::DeviceError;
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    use super::{check_from_error, check_outcome, pay_outcome};
    use crate::mount::Applier;

    /// 発送済みの（世代, 取り消し用 `PaymentRequest`）。同じ世代を二重に発送しない。
    type Entry = (u64, Option<JsValue>);

    /// `pagehide` 購読クロージャの型。
    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static DISPATCHED: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        /// `pagehide` 購読（初回の要求で 1 度だけ登録し、ページの寿命まで持つ）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> =
            const { RefCell::new(None) };
    }

    fn key(name: &str) -> JsValue {
        JsValue::from_str(name)
    }

    /// `target[name]()` を引数なしで呼ぶ（web-sys に `PaymentRequest` 型が無いため Reflect で呼ぶ）。
    fn call0(target: &JsValue, name: &str) -> Result<JsValue, JsValue> {
        let f: Function = Reflect::get(target, &key(name))?.dyn_into()?;
        f.call0(target)
    }

    /// 飛行中の要求を `abort()` する。拒否（`InvalidStateError` 等）は黙って捨てる（ログを残さない）。
    fn abort_req(req: &JsValue) {
        if let Ok(p) = call0(req, "abort") {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = JsFuture::from(js_sys::Promise::resolve(&p)).await;
            });
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に飛行中の要求を取り消し、記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some((_, Some(req))) = DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id)) {
            abort_req(&req);
        }
    }

    /// 全マウントの飛行中の要求を取り消す（`pagehide`）。記録は残す（復帰後の二重発送防止）。
    fn abort_all() {
        let reqs: Vec<JsValue> = DISPATCHED.with(|d| {
            d.borrow_mut()
                .values_mut()
                .filter_map(|e| e.1.take())
                .collect()
        });
        for r in reqs {
            abort_req(&r);
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

    /// 完了した世代の要求を外す（世代が違えば新しい要求のものなので触らない）。
    fn clear(mount_id: u64, generation: u64) {
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                if e.0 == generation {
                    e.1 = None;
                }
            }
        });
    }

    /// 例外の `name` だけを結果へ写す（`message` は読まない）。無ければ `Failed` になる。
    fn outcome_of_error(err: &JsValue) -> Result<PaymentOutcome, DeviceError> {
        let name = Reflect::get(err, &key("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        outcome_from_dom_error(&name)
    }

    /// 承認後の応答を閉じる。`details` には触れず `complete("success")` だけを呼ぶ。
    /// どこで失敗しても `Failed`（成功にしない）。
    async fn complete_response(response: JsValue) -> Result<PaymentOutcome, DeviceError> {
        let called = Reflect::get(&response, &key("complete"))
            .and_then(|f| f.dyn_into::<Function>())
            .and_then(|f| f.call1(&response, &key("success")));
        let Ok(p) = called else {
            return Ok(PaymentOutcome::Failed);
        };
        match JsFuture::from(js_sys::Promise::resolve(&p)).await {
            Ok(_) => Ok(PaymentOutcome::TestAuthorized),
            Err(_) => Ok(PaymentOutcome::Failed),
        }
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ決済シートを依頼する。
    pub fn payment_request_effect(
        state: &PaymentRequestState,
        applier: &Applier<PaymentRequestState>,
    ) {
        let Some(view) = take_request(state) else {
            return;
        };
        let generation = view.generation;
        let action = view.action;
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

        // Check / Pay で結果の写像だけが違う。`checked` は canMakePayment() が返した bool。
        let done = move |applier: &Applier<PaymentRequestState>,
                         r: Result<PaymentOutcome, DeviceError>,
                         checked: Option<bool>| {
            let out: Result<CoreOutcome, String> = match action {
                PaymentAction::Check => check_outcome(match checked {
                    Some(b) => Ok(b),
                    None => check_from_error(r),
                }),
                PaymentAction::Pay => pay_outcome(r),
            };
            applier.apply(move |s| finish(s, generation, out));
        };
        let unsupported = || Err(DeviceError::Unsupported("payment_request"));
        let internal = || Err(DeviceError::Internal("payment_request failed"));

        let Some(window) = web_sys::window() else {
            return done(applier, unsupported(), None);
        };
        let win_value: JsValue = window.clone().into();
        let ctor: Option<Function> = Reflect::get(&win_value, &key("PaymentRequest"))
            .ok()
            .and_then(|v| v.dyn_into().ok());
        let Some(ctor) = ctor.filter(|_| window.is_secure_context()) else {
            return done(applier, unsupported(), None);
        };

        // 支払い方法と金額は const の parse 結果だけを渡す。
        let (Ok(method), Ok(details)) = (
            js_sys::JSON::parse(METHOD_DATA_JSON),
            js_sys::JSON::parse(DETAILS_JSON),
        ) else {
            return done(applier, internal(), None);
        };
        let req = match Reflect::construct(&ctor, &Array::of2(&method, &details)) {
            Ok(r) => r,
            Err(err) => return done(applier, outcome_of_error(&err), None),
        };
        DISPATCHED.with(|d| {
            if let Some(e) = d.borrow_mut().get_mut(&mount_id) {
                e.1 = Some(req.clone());
            }
        });
        register_pagehide_once(&window);

        // show() / canMakePayment() はここ（クリックから続く同期処理）で呼ぶ。await は後。
        let method_name = match action {
            PaymentAction::Pay => "show",
            PaymentAction::Check => "canMakePayment",
        };
        let promise = match call0(&req, method_name) {
            Ok(p) => p,
            Err(err) => {
                clear(mount_id, generation);
                return done(applier, outcome_of_error(&err), None);
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let settled = JsFuture::from(js_sys::Promise::resolve(&promise)).await;
            let (result, checked) = match (action, settled) {
                (PaymentAction::Pay, Ok(response)) => (complete_response(response).await, None),
                (PaymentAction::Check, Ok(v)) => match v.as_bool() {
                    Some(b) => (Ok(PaymentOutcome::Failed), Some(b)),
                    None => (internal(), None),
                },
                (_, Err(err)) => (outcome_of_error(&err), None),
            };
            clear(mount_id, generation);
            done(&applier, result, checked);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::payment_request_effect;

/// マウント破棄時に飛行中の要求を取り消す。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::payment_request::outcome_from_dom_error;

    #[test]
    fn maps_every_arm() {
        assert_eq!(check_outcome(Ok(true)), Ok(CoreOutcome::Available));
        assert_eq!(check_outcome(Ok(false)), Ok(CoreOutcome::Unsupported));
        assert_eq!(
            check_outcome(Err(DeviceError::Unsupported("payment_request"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            check_outcome(Err(DeviceError::Internal("payment_request failed"))),
            Err(FAILED_TEXT.to_string())
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::TestAuthorized)),
            Ok(CoreOutcome::TestAuthorized)
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::Cancelled)),
            Ok(CoreOutcome::Cancelled)
        );
        assert_eq!(
            pay_outcome(Ok(PaymentOutcome::Failed)),
            Err(FAILED_TEXT.to_string())
        );
        assert_eq!(
            pay_outcome(Err(DeviceError::Unsupported("payment_request"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert!(!FAILED_TEXT.contains("payment"));
    }

    #[test]
    fn check_from_error_never_succeeds() {
        assert!(matches!(
            check_from_error(Ok(PaymentOutcome::Cancelled)),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            check_from_error(Err(DeviceError::Unsupported("payment_request"))),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn dom_errors_never_succeed() {
        for name in [
            "AbortError",
            "NotSupportedError",
            "SecurityError",
            "NotAllowedError",
            "InvalidStateError",
            "TypeError",
            "",
            "Whatever",
        ] {
            assert_ne!(
                pay_outcome(outcome_from_dom_error(name)),
                Ok(CoreOutcome::TestAuthorized)
            );
            assert_ne!(
                check_outcome(check_from_error(outcome_from_dom_error(name))),
                Ok(CoreOutcome::Available)
            );
        }
    }
}
