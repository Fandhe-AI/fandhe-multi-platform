//! 型サンプル「パスワード・ワンタイムコードの自動入力」の Web 経路（Issue #587、親 #121、共通部分は #585）。
//!
//! `fandhe_core::screens::otp_input`（端末依存ゼロ）の要求を [`super::mount::mount_with_effect`] の
//! effect として観測し、次の 2 つを行う。
//!
//! 1. ヒント: `fandhe-web-dom` が出す本物の `<input>` に `autocomplete="one-time-code"` と
//!    `inputmode="numeric"` を付ける。
//! 2. SMS 待ち受け: Chromium 系の WebOTP（`navigator.credentials.get({ otp: { transport: ["sms"] } })`）
//!    で受信し、入力欄が空のときだけコードを入れる。
//!
//! # `AutofillOtp` トレイトを通らない理由
//!
//! `fandhe_device::autofill_otp::AutofillOtp` は同期で、wasm のメインスレッドは DOM を直接触る。
//! `passkey.rs` と同じく Web は effect で非同期に処理し、`fandhe_device::autofill_otp` の
//! 純関数（`wait_outcome_from_dom_error`・`should_fill`）と定数だけを共有する。
//!
//! # ヒントの付け方
//!
//! `mount.rs` は描画のたびに DOM を全再構築するため、`<input>` も作り直される。属性は effect が
//! 描画のたびに付け直す。属性の変更は `MutationObserver`（`childList` のみ）に拾われないため、
//! 付け直しが `sweep_mounts` を走らせることはない。`apply` は呼ばないので再描画のループにもならない。
//! 候補が出るかは OS・ブラウザ・SMS の文面しだいで保証しない。
//!
//! # 結果の写像（三値）
//!
//! - コードを受け取った: 空欄のときだけ入れる（`should_fill`）。trim・桁の加工はしない
//! - 取り消し・期限切れ: 中立の案内。期限後に届いた受信は成功にせず `TimedOut` に置き換える
//! - secure context 外・`OTPCredential` 無し・`SecurityError`/`NotSupportedError`: 非対応
//! - それ以外: 固定文言の失敗
//!
//! 例外は `name` だけを見る。`message` とコードの値は画面にも `console` にも `localStorage` にも出さない。
//!
//! # ライフサイクル
//!
//! `get` はクリックからの同期チェーンで呼ぶ。要求ごとに `AbortController` と `setTimeout`
//! （`WEBOTP_TIMEOUT_MS`、起点は呼び出しの開始）を持ち、結果を届ける前に解除する。マウント破棄
//! （`release_mount`）と `pagehide` で `abort()` し、`Waiting` のまま固まらない。同じ
//! `(mount_id, generation)` は二度発送しない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 ページ内の要求と完了の順序（世代照合）、資源の解放、失敗を成功に見せないこと、
//! 入力済みの欄を上書きしないこと。守らない: 候補が出るか、再構築直後の一瞬に属性が無いこと、
//! 複数タブ、サーバー検証。

#[cfg(any(test, target_arch = "wasm32"))]
use fandhe_core::screens::otp_input::{
    CoreWaitFailure, CoreWaitOutcome, HintPhase, CODE_FIELD_LABEL,
};
#[cfg(any(test, target_arch = "wasm32"))]
use fandhe_device::autofill_otp::{should_fill, OtpCode, WaitOutcome, WEBOTP_TIMEOUT_MS};
#[cfg(any(test, target_arch = "wasm32"))]
use fandhe_device::DeviceError;

/// `autocomplete` に付ける値。
#[cfg(any(test, target_arch = "wasm32"))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const AUTOCOMPLETE_VALUE: &str = "one-time-code";
/// `inputmode` に付ける値。
#[cfg(any(test, target_arch = "wasm32"))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const INPUTMODE_VALUE: &str = "numeric";

/// 属性を付けるべき段階か。全再構築で `<input>` が作り直されるため、描画のたびにこの判定で付け直す。
#[cfg(any(test, target_arch = "wasm32"))]
fn wants_hint_attrs(phase: HintPhase) -> bool {
    matches!(phase, HintPhase::Applying | HintPhase::Applied)
}

/// `<label>` の文言がコード欄のものか。
#[cfg(any(test, target_arch = "wasm32"))]
fn is_code_label(text: &str) -> bool {
    text == CODE_FIELD_LABEL
}

/// WebOTP を使える環境か（3 条件すべて必要）。
#[cfg(any(test, target_arch = "wasm32"))]
fn sms_wait_supported(
    secure_context: bool,
    has_credentials: bool,
    has_otp_credential: bool,
) -> bool {
    secure_context && has_credentials && has_otp_credential
}

/// 期限切れのフラグが立っていれば、後から届いた結果にかかわらず `TimedOut` にする。
#[cfg(any(test, target_arch = "wasm32"))]
fn settle(
    timed_out: bool,
    raw: Result<WaitOutcome, DeviceError>,
) -> Result<WaitOutcome, DeviceError> {
    if timed_out {
        Ok(WaitOutcome::TimedOut)
    } else {
        raw
    }
}

/// 待ち受け結果を画面用の結果へ写す。`fandhe-egui-proxy/src/device/autofill_otp.rs` と同じ写像。
/// `current_code` は適用時点の入力値で、`should_fill` に渡す。
#[cfg(any(test, target_arch = "wasm32"))]
fn to_core_wait(
    r: Result<WaitOutcome, DeviceError>,
    current_code: &str,
) -> Result<CoreWaitOutcome, CoreWaitFailure> {
    match r {
        Ok(WaitOutcome::Received(c)) => Ok(CoreWaitOutcome::Received {
            code: c.into_string(),
            fill: should_fill(current_code),
        }),
        Ok(WaitOutcome::Cancelled) => Ok(CoreWaitOutcome::Cancelled),
        Ok(WaitOutcome::TimedOut) => Ok(CoreWaitOutcome::TimedOut),
        Err(DeviceError::Unsupported(_)) => Err(CoreWaitFailure::Unsupported),
        Err(_) => Err(CoreWaitFailure::Failed),
    }
}

/// JS の `OTPCredential.code` を受信結果へ。取れない・空は失敗側（成功に畳まない）。加工しない。
#[cfg(any(test, target_arch = "wasm32"))]
fn code_from_js(v: Option<String>) -> Result<WaitOutcome, DeviceError> {
    let s = v.ok_or(DeviceError::Internal("autofill_otp webotp"))?;
    OtpCode::new(s).map(WaitOutcome::Received)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::otp_input::{
        finish_hint, finish_wait, mark_hint_applying, mark_waiting, set_sms_wait_available,
        take_hint_request, take_wait_request, HintOutcome, OtpInputState, HINT_FAILED_STATUS,
    };
    use fandhe_device::autofill_otp::wait_outcome_from_dom_error;
    use js_sys::{Array, Object, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{AbortController, CredentialRequestOptions, Element};

    use super::*;
    use crate::mount::{root_of, Applier};

    /// 飛行中の待ち受け。タイマーと取り消しを結果の確定前に必ず解放する。
    struct Entry {
        generation: u64,
        ctrl: Option<AbortController>,
        timer: Option<(i32, Closure<dyn FnMut()>)>,
    }

    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static WAITS: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        static HINT_DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    fn stop_entry(e: Entry) {
        if let Some((handle, _closure)) = e.timer {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(handle);
            }
        }
        if let Some(c) = e.ctrl {
            c.abort();
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に待ち受けを取り消し、記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        HINT_DISPATCHED.with(|h| h.borrow_mut().remove(&mount_id));
        if let Some(e) = WAITS.with(|w| w.borrow_mut().remove(&mount_id)) {
            stop_entry(e);
        }
    }

    /// 全マウントの待ち受けを取り消す（`pagehide`）。記録は残し、取り消された Promise の
    /// 完了（`AbortError` → 中立）で通常どおり確定させる。
    fn abort_all() {
        let ctrls: Vec<AbortController> = WAITS.with(|w| {
            w.borrow_mut()
                .values_mut()
                .filter_map(|e| e.ctrl.clone())
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

    fn has_property(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|v| !v.is_undefined() && !v.is_null())
            .unwrap_or(false)
    }

    fn supported(window: &web_sys::Window) -> bool {
        let nav: JsValue = window.navigator().into();
        let win: JsValue = window.clone().into();
        sms_wait_supported(
            window.is_secure_context(),
            has_property(&nav, "credentials"),
            Reflect::has(&win, &JsValue::from_str("OTPCredential")).unwrap_or(false),
        )
    }

    /// 例外の `name` だけを結果へ写す（`message` は読まない）。
    fn outcome_of_error(err: &JsValue) -> Result<WaitOutcome, DeviceError> {
        let name = Reflect::get(err, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        wait_outcome_from_dom_error(&name)
    }

    /// コード欄の `<input>`。描画は `<label>ラベル<input></label>` なので、文言で照合する。
    fn find_code_input(root: &Element) -> Option<Element> {
        let labels = root.query_selector_all("label").ok()?;
        for i in 0..labels.length() {
            let Some(label) = labels.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else {
                continue;
            };
            if is_code_label(&label.text_content().unwrap_or_default()) {
                return label.query_selector("input").ok().flatten();
            }
        }
        None
    }

    fn set_hint_attrs(input: &Element) -> bool {
        input
            .set_attribute("autocomplete", AUTOCOMPLETE_VALUE)
            .and_then(|_| input.set_attribute("inputmode", INPUTMODE_VALUE))
            .is_ok()
    }

    /// 結果を届ける前に、同じ世代のタイマーと取り消しを解放する（世代違いは新しい要求のものなので触らない）。
    fn cleanup(mount_id: u64, generation: u64) {
        let taken = WAITS.with(|w| {
            let mut w = w.borrow_mut();
            if w.get(&mount_id).map(|e| e.generation) == Some(generation) {
                w.remove(&mount_id)
            } else {
                None
            }
        });
        if let Some(e) = taken {
            // 完了済みの Promise に対する abort は無害。タイマーだけ確実に止める。
            stop_entry(e);
        }
    }

    fn deliver(
        applier: &Applier<OtpInputState>,
        generation: u64,
        r: Result<WaitOutcome, DeviceError>,
    ) {
        applier.apply(move |s| {
            let out = to_core_wait(r, &s.code);
            finish_wait(s, generation, out);
        });
    }

    /// `mount_with_effect` へ渡す effect。ヒントの付け直し・対応可否の判定・SMS 待ち受けを行う。
    pub fn otp_input_effect(state: &OtpInputState, applier: &Applier<OtpInputState>) {
        let mount_id = applier.mount_id();
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(root) = root_of(mount_id) else {
            return;
        };

        // 1. ヒント。再構築で消えた属性を付け直す（apply は呼ばない）。
        let input = find_code_input(&root);
        if wants_hint_attrs(state.hint) {
            if let Some(i) = &input {
                set_hint_attrs(i);
            }
        }
        if let Some(req) = take_hint_request(state) {
            let fresh = HINT_DISPATCHED.with(|h| {
                let mut h = h.borrow_mut();
                if h.get(&mount_id) == Some(&req.generation) {
                    return false;
                }
                h.insert(mount_id, req.generation);
                true
            });
            if fresh {
                applier.apply(mark_hint_applying);
                let result = match &input {
                    Some(i) if set_hint_attrs(i) => Ok(HintOutcome::Applied),
                    _ => Err(HINT_FAILED_STATUS.to_string()),
                };
                let generation = req.generation;
                applier.apply(move |s| finish_hint(s, generation, result));
            }
        }

        // 2. SMS 待ち受けを使えるか（変わったときだけ反映してループを避ける）。
        let can_wait = supported(&window);
        if state.sms_wait_available != can_wait {
            applier.apply(move |s| set_sms_wait_available(s, can_wait));
        }

        // 3. WebOTP。
        let Some(req) = take_wait_request(state) else {
            return;
        };
        let generation = req.generation;
        let fresh = WAITS.with(|w| {
            let mut w = w.borrow_mut();
            if w.get(&mount_id).map(|e| e.generation) == Some(generation) {
                return false;
            }
            if let Some(old) = w.insert(
                mount_id,
                Entry {
                    generation,
                    ctrl: None,
                    timer: None,
                },
            ) {
                stop_entry(old);
            }
            true
        });
        if !fresh {
            return;
        }
        applier.apply(mark_waiting);

        if !can_wait {
            cleanup(mount_id, generation);
            applier.apply(move |s| {
                finish_wait(s, generation, Err(CoreWaitFailure::Unsupported));
            });
            return;
        }
        let fail = |applier: &Applier<OtpInputState>| {
            cleanup(mount_id, generation);
            applier.apply(move |s| finish_wait(s, generation, Err(CoreWaitFailure::Failed)));
        };
        let Ok(ctrl) = AbortController::new() else {
            return fail(applier);
        };

        // options = { otp: { transport: ["sms"] }, signal }
        let transport = Array::new();
        transport.push(&JsValue::from_str("sms"));
        let otp = Object::new();
        let opts = Object::new();
        let ok = Reflect::set(&otp, &"transport".into(), &transport).is_ok()
            && Reflect::set(&opts, &"otp".into(), &otp).is_ok()
            && Reflect::set(&opts, &"signal".into(), &ctrl.signal()).is_ok();
        if !ok {
            return fail(applier);
        }
        register_pagehide_once(&window);
        WAITS.with(|w| {
            if let Some(e) = w.borrow_mut().get_mut(&mount_id) {
                e.ctrl = Some(ctrl.clone());
            }
        });

        // クリックからの同期チェーンで呼ぶ（await を挟まない）。
        let promise = window
            .navigator()
            .credentials()
            .get_with_options(opts.unchecked_ref::<CredentialRequestOptions>());
        let promise = match promise {
            Ok(p) => p,
            Err(err) => {
                cleanup(mount_id, generation);
                return deliver(applier, generation, outcome_of_error(&err));
            }
        };

        // 期限は呼び出し開始が起点。単調時計側の setTimeout に任せ、発火したら取り消す。
        let timed_out = Rc::new(Cell::new(false));
        let timer_closure = {
            let timed_out = timed_out.clone();
            let ctrl = ctrl.clone();
            Closure::<dyn FnMut()>::new(move || {
                timed_out.set(true);
                ctrl.abort();
            })
        };
        let handle = i32::try_from(WEBOTP_TIMEOUT_MS).ok().and_then(|ms| {
            window
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    timer_closure.as_ref().unchecked_ref(),
                    ms,
                )
                .ok()
        });
        match handle {
            Some(h) => WAITS.with(|w| {
                if let Some(e) = w.borrow_mut().get_mut(&mount_id) {
                    e.timer = Some((h, timer_closure));
                }
            }),
            None => {
                // 期限を張れないなら待ち続けない。
                ctrl.abort();
            }
        }

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let raw = match JsFuture::from(promise).await {
                Ok(v) => code_from_js(
                    Reflect::get(&v, &JsValue::from_str("code"))
                        .ok()
                        .and_then(|c| c.as_string()),
                ),
                Err(err) => outcome_of_error(&err),
            };
            let result = settle(timed_out.get(), raw);
            cleanup(mount_id, generation);
            deliver(&applier, generation, result);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::otp_input_effect;

/// マウント破棄時に待ち受けを取り消す。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::otp_input::WAITING_STATUS;
    use fandhe_device::autofill_otp::wait_outcome_from_dom_error;

    fn received(s: &str) -> WaitOutcome {
        WaitOutcome::Received(OtpCode::new(s.to_string()).expect("非空"))
    }

    #[test]
    fn waiting_status_matches_timeout() {
        assert!(WAITING_STATUS.contains(&format!("最大 {} 秒", WEBOTP_TIMEOUT_MS / 1000)));
    }

    #[test]
    fn timed_out_overrides_late_success() {
        assert_eq!(
            settle(true, Ok(received("123456"))).expect("ok"),
            WaitOutcome::TimedOut
        );
        assert_eq!(
            settle(true, Ok(WaitOutcome::Cancelled)).expect("ok"),
            WaitOutcome::TimedOut
        );
        assert_eq!(
            settle(false, Ok(WaitOutcome::Cancelled)).expect("ok"),
            WaitOutcome::Cancelled
        );
    }

    #[test]
    fn to_core_wait_maps_every_arm() {
        assert_eq!(
            to_core_wait(Ok(received("123456")), ""),
            Ok(CoreWaitOutcome::Received {
                code: "123456".into(),
                fill: true
            })
        );
        assert_eq!(
            to_core_wait(Ok(received("123456")), "12"),
            Ok(CoreWaitOutcome::Received {
                code: "123456".into(),
                fill: false
            })
        );
        assert_eq!(
            to_core_wait(Ok(WaitOutcome::Cancelled), ""),
            Ok(CoreWaitOutcome::Cancelled)
        );
        assert_eq!(
            to_core_wait(Ok(WaitOutcome::TimedOut), ""),
            Ok(CoreWaitOutcome::TimedOut)
        );
        assert_eq!(
            to_core_wait(Err(DeviceError::Unsupported("autofill_otp")), ""),
            Err(CoreWaitFailure::Unsupported)
        );
        assert_eq!(
            to_core_wait(Err(DeviceError::Internal("x")), ""),
            Err(CoreWaitFailure::Failed)
        );
        assert_eq!(
            to_core_wait(Err(DeviceError::InvalidInput("x")), ""),
            Err(CoreWaitFailure::Failed)
        );
    }

    #[test]
    fn code_from_js_rejects_missing_and_empty_keeps_raw() {
        assert!(code_from_js(None).is_err());
        assert!(code_from_js(Some(String::new())).is_err());
        assert_eq!(
            code_from_js(Some(" 123456 ".into())).expect("ok"),
            received(" 123456 ")
        );
    }

    #[test]
    fn dom_error_names_map_through() {
        let map = |n: &str| to_core_wait(wait_outcome_from_dom_error(n), "");
        assert_eq!(map("AbortError"), Ok(CoreWaitOutcome::Cancelled));
        assert_eq!(map("NotAllowedError"), Ok(CoreWaitOutcome::Cancelled));
        assert_eq!(map("SecurityError"), Err(CoreWaitFailure::Unsupported));
        assert_eq!(map("UnknownError"), Err(CoreWaitFailure::Failed));
    }

    #[test]
    fn hint_attrs_only_while_applying_or_applied() {
        assert!(wants_hint_attrs(HintPhase::Applying));
        assert!(wants_hint_attrs(HintPhase::Applied));
        for p in [
            HintPhase::Requested,
            HintPhase::Unsupported,
            HintPhase::Failed,
        ] {
            assert!(!wants_hint_attrs(p));
        }
    }

    #[test]
    fn is_code_label_matches_core_label() {
        assert!(is_code_label(CODE_FIELD_LABEL));
        assert!(!is_code_label("別の欄"));
    }

    #[test]
    fn sms_wait_supported_requires_all() {
        assert!(sms_wait_supported(true, true, true));
        assert!(!sms_wait_supported(false, true, true));
        assert!(!sms_wait_supported(true, false, true));
        assert!(!sms_wait_supported(true, true, false));
    }

    #[test]
    fn debug_never_contains_code() {
        let s = format!("{:?}", received("987654"));
        assert!(!s.contains("987654"));
    }
}
