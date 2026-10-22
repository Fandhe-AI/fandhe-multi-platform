//! 型サンプル「端末内 LLM」の Web 経路（親 #93、Issue #347）。
//!
//! `fandhe_core::screens::on_device_llm`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`take_request` で要求を観測し、Prompt API（`LanguageModel`）へ固定プロンプトを
//! 1 回渡して応答の全文を画面へ返す。`OnDeviceLlm` trait（`Send + Sync`）は `JsValue` を持てないため
//! 通らず、契約（`fandhe_device::on_device_llm` の写像表）だけを共有する。`LanguageModel` は web-sys に
//! 型が無いので `js_sys::Reflect` で呼ぶ。`fandhe-egui-proxy` の wasm canvas 経路は対象外（`Unsupported`）。
//!
//! # 結果の種類
//!
//! - 成功: 空でない応答の全文
//! - 失敗: 非対応（secure context でない・`LanguageModel` が無い・`unavailable`）・モデル未取得
//!   （`downloadable` / `downloading`。`create()` は呼ばずダウンロードも始めない）・許可なし
//!   （`NotAllowedError`）・期限切れ・空の応答・その他
//! - 中断: `pagehide`。失敗とは別で、`pageshow` で戻っても自動では再開しない
//!
//! # 期限
//!
//! 起点は effect での要求観測時の `performance.now()`（単調時計）。`RESPOND_DEADLINE` の
//! `setTimeout` で `abort()` して `Timeout` を確定する。期限の後に届いた成功も `Timeout` にする。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（世代照合）、失敗・期限切れ・非対応を成功へ畳まないこと、
//! セッション（`destroy()`）・タイマー・リスナーの解放。
//! 守らない: ブラウザごとのモデル差、応答の非決定性、複数タブ・複数マウント間の一致、
//! egui-proxy の wasm canvas 経路。プロンプト・応答・例外の `message` は画面にも console にも出さない
//! （security.md A09）。

use fandhe_core::screens::on_device_llm::LlmFailure;
use fandhe_device::on_device_llm::{check_response, EMPTY};
use fandhe_device::DeviceError;

/// 失敗の固定文言（`fandhe-egui-proxy` の `to_failure` と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED: &str = "応答を得られませんでした";

/// `LanguageModel.availability()` の結果を判定する。`available` 以外は `create()` を呼ばせない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn availability_outcome(s: Option<&str>) -> Result<(), LlmFailure> {
    match s {
        Some("available") => Ok(()),
        Some("downloadable" | "downloading") => Err(LlmFailure::ModelNotReady),
        Some("unavailable") => Err(LlmFailure::Unsupported),
        _ => Err(LlmFailure::Failed(FAILED.to_string())),
    }
}

/// 例外の `name` → 失敗。内部の文言は返さない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> LlmFailure {
    match name {
        Some("NotAllowedError") => LlmFailure::PermissionDenied,
        _ => LlmFailure::Failed(FAILED.to_string()),
    }
}

/// `prompt()` の戻り値を判定する。文字列でなければ失敗、空は `Empty`。trim はしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn response_outcome(v: Option<String>) -> Result<String, LlmFailure> {
    match v.map(check_response) {
        Some(Ok(t)) => Ok(t),
        Some(Err(DeviceError::Internal(EMPTY))) => Err(LlmFailure::Empty),
        _ => Err(LlmFailure::Failed(FAILED.to_string())),
    }
}

/// 開始から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    now_ms - started_ms >= fandhe_device::on_device_llm::RESPOND_DEADLINE.as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
pub use web::on_device_llm_effect;

/// マウント破棄時にセッション・リスナー・タイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::on_device_llm::{
        finish_respond, mark_running, take_request, LlmFailure, OnDeviceLlmState, PROMPT,
    };
    use fandhe_device::on_device_llm::{validate_prompt, RESPOND_DEADLINE};
    use js_sys::{Array, Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{availability_outcome, failure_from_error_name, is_late, response_outcome, FAILED};
    use crate::mount::{is_live, Applier};

    type State = OnDeviceLlmState;

    /// マウントごとの要求 1 件。`teardown` で解除してから drop する。
    struct Session {
        gen: u64,
        /// 結果を確定済みか。以後の通知はすべて捨てる。
        finished: bool,
        timer_id: Option<i32>,
        /// タイマー発火中に drop しないよう、`teardown` まで保持する。
        _timer_cb: Closure<dyn FnMut()>,
        on_pagehide: Closure<dyn FnMut()>,
        controller: web_sys::AbortController,
        /// 作成済みの `LanguageModel` セッション。
        lm: Option<JsValue>,
    }

    thread_local! {
        static SESSIONS: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    fn get(o: &JsValue, k: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(k)).unwrap_or(JsValue::UNDEFINED)
    }

    fn obj(fields: &[(&str, JsValue)]) -> JsValue {
        let o = Object::new();
        for (k, v) in fields {
            let _ = Reflect::set(&o, &JsValue::from_str(k), v);
        }
        o.into()
    }

    /// 例外の `name` だけを読む（`message` は使わない）。
    fn err_name(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    fn call(o: &JsValue, m: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(o, m).dyn_into()?;
        let a = Array::new();
        for v in args {
            a.push(v);
        }
        Reflect::apply(&f, o, &a)
    }

    async fn wait(v: Result<JsValue, JsValue>) -> Result<JsValue, JsValue> {
        JsFuture::from(Promise::resolve(&v?)).await
    }

    /// 生成を打ち切り、セッションを破棄する。例外は握りつぶす（後始末のため）。
    fn release_lm(s: &mut Session) {
        s.controller.abort();
        if let Some(lm) = s.lm.take() {
            let _ = call(&lm, "destroy", &[]);
        }
    }

    /// 解除を済ませてから Closure を drop する（契約表 9）。
    fn teardown(mut s: Session) {
        let w = web_sys::window();
        if let (Some(w), Some(id)) = (&w, s.timer_id.take()) {
            w.clear_timeout_with_handle(id);
        }
        release_lm(&mut s);
        if let Some(w) = w {
            let _ = w.remove_event_listener_with_callback(
                "pagehide",
                s.on_pagehide.as_ref().unchecked_ref(),
            );
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        if let Some(s) = SESSIONS.with(|m| m.borrow_mut().remove(&mount)) {
            teardown(s);
        }
    }

    /// 通知を非同期に書き戻す。
    fn post(applier: &Applier<State>, f: impl FnOnce(&mut State) + 'static) {
        let a = applier.clone();
        spawn_local(async move { a.apply(f) });
    }

    /// マウントが有効で、世代が一致し、未確定のときだけ `f` を実行する（契約表 2）。
    fn with_live<R>(mount: u64, g: u64, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        if !is_live(mount) {
            return None;
        }
        SESSIONS.with(|m| {
            let mut m = m.borrow_mut();
            m.get_mut(&mount)
                .filter(|s| s.gen == g && !s.finished)
                .map(f)
        })
    }

    /// 確定の共通処理。確定済みにし、タイマーを止め、生成を打ち切ってから `after` を書き戻す。
    fn settle(
        applier: &Applier<State>,
        mount: u64,
        g: u64,
        after: impl FnOnce(&mut State) + 'static,
    ) {
        let hit = with_live(mount, g, |s| {
            s.finished = true;
            if let (Some(w), Some(id)) = (web_sys::window(), s.timer_id.take()) {
                w.clear_timeout_with_handle(id);
            }
            release_lm(s);
        });
        if hit.is_some() {
            post(applier, after);
            // 実行中の Closure（タイマー・pagehide）から呼ばれうるため、抜けた後
            // （次のマイクロタスク）にリスナー解除と Closure の破棄を行う（契約表 3・9）。
            spawn_local(async move {
                let done = SESSIONS.with(|m| {
                    let mut m = m.borrow_mut();
                    if m.get(&mount).is_some_and(|s| s.gen == g && s.finished) {
                        m.remove(&mount)
                    } else {
                        None
                    }
                });
                if let Some(s) = done {
                    teardown(s);
                }
            });
        }
    }

    fn conclude(applier: &Applier<State>, mount: u64, g: u64, r: Result<String, LlmFailure>) {
        settle(applier, mount, g, move |s| finish_respond(s, g, r));
    }

    /// 内部の打ち切り理由。`Gone` は要求がもう現在のものでないこと（何も書き戻さない）。
    enum Stop {
        Gone,
        Fail(LlmFailure),
    }

    fn alive(mount: u64, g: u64) -> Result<(), Stop> {
        with_live(mount, g, |_| ()).ok_or(Stop::Gone)
    }

    fn js_fail(e: JsValue) -> Stop {
        Stop::Fail(failure_from_error_name(err_name(&e).as_deref()))
    }

    fn fail(f: LlmFailure) -> Stop {
        Stop::Fail(f)
    }

    /// 可否判定 → 作成 → 1 回の `prompt`。各 `await` の後に現在の要求かを確かめる。
    async fn run(mount: u64, g: u64, signal: JsValue) -> Result<String, Stop> {
        let w = web_sys::window();
        let secure = w
            .as_ref()
            .is_some_and(|w| get(w, "isSecureContext").as_bool() == Some(true));
        let lm_ctor = w
            .as_ref()
            .map_or(JsValue::UNDEFINED, |w| get(w, "LanguageModel"));
        if !secure
            || !(lm_ctor.is_object() || lm_ctor.is_function())
            || !get(&lm_ctor, "availability").is_function()
            || !get(&lm_ctor, "create").is_function()
        {
            return Err(fail(LlmFailure::Unsupported));
        }
        validate_prompt(PROMPT).map_err(|_| fail(LlmFailure::Failed(FAILED.to_string())))?;

        let langs = || {
            let a = Array::new();
            a.push(&obj(&[
                ("type", "text".into()),
                ("languages", Array::of1(&"ja".into()).into()),
            ]));
            JsValue::from(a)
        };
        let opts = obj(&[("expectedInputs", langs()), ("expectedOutputs", langs())]);

        let status = wait(call(&lm_ctor, "availability", std::slice::from_ref(&opts)))
            .await
            .map_err(js_fail)?;
        alive(mount, g)?;
        // `available` 以外では `create()` を呼ばない（ダウンロードを始めない）。
        availability_outcome(status.as_string().as_deref()).map_err(fail)?;

        let _ = Reflect::set(&opts, &"signal".into(), &signal);
        let lm = wait(call(&lm_ctor, "create", &[opts]))
            .await
            .map_err(js_fail)?;
        // 戻った時点で要求が無効なら、作ったセッションをその場で破棄する。
        let kept = with_live(mount, g, |s| s.lm = Some(lm.clone()));
        if kept.is_none() {
            let _ = call(&lm, "destroy", &[]);
            return Err(Stop::Gone);
        }
        let res = wait(call(
            &lm,
            "prompt",
            &[PROMPT.into(), obj(&[("signal", signal)])],
        ))
        .await
        .map_err(js_fail)?;
        alive(mount, g)?;
        response_outcome(res.as_string()).map_err(fail)
    }

    /// 画面の effect。未発送の要求を 1 度だけ発送する（同じ世代を二重に始めない）。
    pub fn on_device_llm_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        let Some(g) = take_request(state) else { return };
        if SESSIONS.with(|m| m.borrow().get(&mount).is_some_and(|s| s.gen == g)) {
            return;
        }
        if let Some(old) = SESSIONS.with(|m| m.borrow_mut().remove(&mount)) {
            teardown(old);
        }
        let Ok(controller) = web_sys::AbortController::new() else {
            // 作れないまま発送すると中断できないため、失敗として確定する（嘘の成功にしない）。
            applier.apply(mark_running);
            post(applier, move |s| {
                finish_respond(s, g, Err(LlmFailure::Failed(FAILED.to_string())))
            });
            return;
        };
        let signal: JsValue = controller.signal().into();
        let started = now();
        let (a_t, a_p) = (applier.clone(), applier.clone());
        let timer_cb: Closure<dyn FnMut()> = Closure::new(move || {
            conclude(&a_t, mount, g, Err(LlmFailure::Timeout));
        });
        // 中断は失敗ではない。復帰しても自動では再開しない。
        let on_pagehide: Closure<dyn FnMut()> = Closure::new(move || {
            conclude(&a_p, mount, g, Err(LlmFailure::Interrupted));
        });
        let mut timer_id = None;
        if let Some(w) = web_sys::window() {
            // リスナーは要求を発送する前に付ける（契約表 1）。
            let _ = w
                .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref());
            timer_id = w
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    timer_cb.as_ref().unchecked_ref(),
                    RESPOND_DEADLINE.as_millis() as i32,
                )
                .ok();
        }
        SESSIONS.with(|m| {
            m.borrow_mut().insert(
                mount,
                Session {
                    gen: g,
                    finished: false,
                    timer_id,
                    _timer_cb: timer_cb,
                    on_pagehide,
                    controller,
                    lm: None,
                },
            );
        });
        applier.apply(mark_running);
        let a = applier.clone();
        spawn_local(async move {
            let r = match run(mount, g, signal).await {
                Err(Stop::Gone) => return,
                Err(Stop::Fail(f)) => Err(f),
                // 期限の後に届いた成功は成功にしない（契約表 3）。
                Ok(_) if is_late(started, now()) => Err(LlmFailure::Timeout),
                Ok(t) => Ok(t),
            };
            conclude(&a, mount, g, r);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> LlmFailure {
        LlmFailure::Failed(FAILED.to_string())
    }

    #[test]
    fn availability_only_available_passes() {
        assert_eq!(availability_outcome(Some("available")), Ok(()));
        for s in ["downloadable", "downloading"] {
            assert_eq!(
                availability_outcome(Some(s)),
                Err(LlmFailure::ModelNotReady)
            );
        }
        assert_eq!(
            availability_outcome(Some("unavailable")),
            Err(LlmFailure::Unsupported)
        );
        assert_eq!(availability_outcome(Some("x")), Err(failed()));
        assert_eq!(availability_outcome(None), Err(failed()));
    }

    #[test]
    fn error_names_hide_messages() {
        assert_eq!(
            failure_from_error_name(Some("NotAllowedError")),
            LlmFailure::PermissionDenied
        );
        assert_eq!(failure_from_error_name(Some("X")), failed());
        assert_eq!(failure_from_error_name(None), failed());
    }

    #[test]
    fn response_outcome_branches() {
        assert_eq!(response_outcome(Some("".into())), Err(LlmFailure::Empty));
        assert_eq!(response_outcome(Some(" \n".into())), Err(LlmFailure::Empty));
        assert_eq!(response_outcome(None), Err(failed()));
        assert_eq!(
            response_outcome(Some(" 春 ".into())),
            Ok(" 春 ".to_string())
        );
    }

    #[test]
    fn late_boundary() {
        assert!(!is_late(0.0, 59_999.0));
        assert!(is_late(0.0, 60_000.0));
        assert!(!is_late(0.0, f64::NAN));
    }
}
