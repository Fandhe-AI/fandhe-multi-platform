//! 型サンプル「画面のスポイト」の Web 経路（Issue #763、親 #148）。
//!
//! `fandhe_core::screens::eye_dropper_pick`（端末依存ゼロ）を [`super::mount::mount_with_effect`] の
//! effect として観測し、`new EyeDropper().open({ signal })` の結果を `finish_pick` へ書き戻す。
//! 呼び出し元は `demo.rs` の `mount_eye_dropper_screen`。契約（結果の 4 値・`sRGBHex` の検査・
//! 例外名の写像）は `fandhe_device::eye_dropper` が SSOT で、macOS 実装（egui-proxy）と共用する。
//! `fandhe-device` の `EyeDropper` トレイトは経由しない（Web は本クレートの担当）。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。
//!
//! # ブラウザ側の制約
//!
//! - secure context 限定。`open()` はユーザー操作（transient activation）の中で呼ぶ必要があるため、
//!   effect の同期区間で呼び、`open()` の前に await を挟まない
//! - デスクトップの Chrome / Edge 95+ のみ。コンストラクタが無ければ `Unsupported`
//!   （対応の有無を前もって問い合わせず、呼んだ結果で判定する）
//!
//! # 結果の写像
//!
//! - resolve: `sRGBHex` を `color_from_srgb_hex` で検査（trim しない）。不正は `Failed`
//! - reject: `AbortError`（Esc・signal による中断）だけが `Cancelled`、他は `Failed`。
//!   例外の `name` だけを使い、`message` は読まない
//! - 期限は持たない。サンプラーを閉じるのは利用者の Esc と `AbortSignal` だけ
//!
//! # ライフサイクル
//!
//! 要求ごとに `AbortController` を作る。マウント破棄（`release_mount`）と `pagehide`
//! （bfcache 退避を含む）で `abort()` し、`AbortError` 経由で通常どおり確定させる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 取り消し・非対応・不正値・失敗を成功に見せないこと、世代照合、発送記録と
//! `AbortController` の解放、拾った色と例外の `message` をログ・永続化に出さないこと。
//! 守らない: Chromium 以外（非対応表示）、ブラウザ・OS ごとのサンプリング位置や色管理の差、
//! 複数タブ・複数マウントの同時操作。

use fandhe_core::screens::eye_dropper_pick::EyeDropperOutcome;
use fandhe_device::eye_dropper::{color_from_srgb_hex, error_from_dom_exception_name, PickedColor};
use fandhe_device::DeviceError;

/// `Result<PickedColor, DeviceError>` を画面用の結果へ畳む。egui-proxy の同名処理と同じ写像
/// （web-dom は egui-proxy に依存できず、core は device を参照できないため重複を許容する）。
/// 権限拒否が来ても成功にせず `Failed` へ畳む。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_result(result: Result<PickedColor, DeviceError>) -> EyeDropperOutcome {
    match result {
        Ok(c) => EyeDropperOutcome::Picked(c.hex()),
        Err(DeviceError::Cancelled(_)) => EyeDropperOutcome::Cancelled,
        Err(DeviceError::Unsupported(_)) => EyeDropperOutcome::Unsupported,
        Err(e) => EyeDropperOutcome::Failed(e.to_string()),
    }
}

/// resolve 値の `sRGBHex` から結果を作る。取れない・形式不正は `Failed`（嘘の成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_resolved(hex: Option<&str>) -> EyeDropperOutcome {
    let result = match hex {
        Some(h) => color_from_srgb_hex(h),
        None => Err(DeviceError::Internal("eye_dropper hex")),
    };
    outcome_from_result(result)
}

/// reject の例外 `name` から結果を作る。`AbortError` だけが `Cancelled`。名前が無ければ `Failed`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_name(name: Option<&str>) -> EyeDropperOutcome {
    let err = match name {
        Some(n) => error_from_dom_exception_name(n),
        None => DeviceError::Internal("eye_dropper exception"),
    };
    outcome_from_result(Err(err))
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::eye_dropper_pick::{
        finish_pick, mark_picking, take_request, EyeDropperOutcome, EyeDropperState,
    };
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen::prelude::*;
    use web_sys::AbortController;

    use super::{outcome_from_error_name, outcome_from_resolved};
    use crate::mount::Applier;

    /// 飛行中の要求。結果の確定前に必ず解放する。
    struct Entry {
        generation: u64,
        ctrl: Option<AbortController>,
    }

    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        static PICKS: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に中断し、記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(e) = PICKS.with(|p| p.borrow_mut().remove(&mount_id)) {
            if let Some(c) = e.ctrl {
                c.abort();
            }
        }
    }

    /// 全マウントを中断する（`pagehide`）。記録は残し、`AbortError` による通常の確定に任せる。
    fn abort_all() {
        let ctrls: Vec<AbortController> =
            PICKS.with(|p| p.borrow().values().filter_map(|e| e.ctrl.clone()).collect());
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

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    fn name_of(e: &JsValue) -> Option<String> {
        get(e, "name").as_string()
    }

    /// 世代が一致する記録だけを取り除く（新しい世代の記録を巻き込まない）。
    fn clear_entry(mount_id: u64, generation: u64) {
        PICKS.with(|p| {
            let mut map = p.borrow_mut();
            if map
                .get(&mount_id)
                .is_some_and(|e| e.generation == generation)
            {
                map.remove(&mount_id);
            }
        });
    }

    /// 記録を片付けてから結果を届ける（届けた後で片付ける順序にしない）。
    fn settle(
        applier: &Applier<EyeDropperState>,
        mount_id: u64,
        generation: u64,
        outcome: EyeDropperOutcome,
    ) {
        clear_entry(mount_id, generation);
        applier.apply(move |s| finish_pick(s, generation, outcome));
    }

    /// `new EyeDropper()` と `open({ signal })` を同期で呼んで Promise を得る。
    /// 失敗は確定済みの結果として返す。
    fn start(ctrl: &AbortController) -> Result<js_sys::Promise, EyeDropperOutcome> {
        let window = web_sys::window().ok_or(EyeDropperOutcome::Unsupported)?;
        let ctor = get(&window.into(), "EyeDropper");
        if !ctor.is_function() {
            return Err(EyeDropperOutcome::Unsupported);
        }
        let ctor: Function = ctor.unchecked_into();
        let dropper = Reflect::construct(&ctor, &Array::new())
            .map_err(|e| outcome_from_error_name(name_of(&e).as_deref()))?;
        let opts = Object::new();
        let failed = || outcome_from_error_name(None);
        Reflect::set(&opts, &"signal".into(), &ctrl.signal()).map_err(|_| failed())?;
        let open = get(&dropper, "open");
        if !open.is_function() {
            return Err(EyeDropperOutcome::Unsupported);
        }
        match open.unchecked_into::<Function>().call1(&dropper, &opts) {
            Ok(p) => p.dyn_into::<js_sys::Promise>().map_err(|_| failed()),
            Err(e) => Err(outcome_from_error_name(name_of(&e).as_deref())),
        }
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ `open()` する。
    pub fn eye_dropper_pick_effect(state: &EyeDropperState, applier: &Applier<EyeDropperState>) {
        let Some(generation) = take_request(state) else {
            return;
        };
        let mount_id = applier.mount_id();
        let already = PICKS.with(|p| {
            p.borrow()
                .get(&mount_id)
                .is_some_and(|e| e.generation == generation)
        });
        if already {
            return;
        }
        // 古い世代が残っていれば中断してから差し替える。
        let previous = PICKS.with(|p| {
            p.borrow_mut().insert(
                mount_id,
                Entry {
                    generation,
                    ctrl: None,
                },
            )
        });
        if let Some(c) = previous.and_then(|e| e.ctrl) {
            c.abort();
        }
        applier.apply(mark_picking);

        let Ok(ctrl) = AbortController::new() else {
            return settle(applier, mount_id, generation, outcome_from_error_name(None));
        };
        PICKS.with(|p| {
            if let Some(e) = p.borrow_mut().get_mut(&mount_id) {
                if e.generation == generation {
                    e.ctrl = Some(ctrl.clone());
                }
            }
        });
        if let Some(w) = web_sys::window() {
            register_pagehide_once(&w);
        }
        // クリックからの同期チェーンで呼ぶ（この前に await を挟まない）。
        let promise = match start(&ctrl) {
            Ok(p) => p,
            Err(outcome) => return settle(applier, mount_id, generation, outcome),
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(v) => outcome_from_resolved(get(&v, "sRGBHex").as_string().as_deref()),
                Err(e) => outcome_from_error_name(name_of(&e).as_deref()),
            };
            settle(&applier, mount_id, generation, outcome);
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::eye_dropper_pick_effect;

/// マウント破棄時に中断・解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_hex_is_normalized_and_picked() {
        assert_eq!(
            outcome_from_resolved(Some("#AABBCC")),
            EyeDropperOutcome::Picked("#aabbcc".into())
        );
    }

    #[test]
    fn invalid_hex_never_picked() {
        for h in [
            None,
            Some("#abc"),
            Some(" #aabbcc"),
            Some("aabbcc"),
            Some("#aabbcc "),
        ] {
            assert!(
                matches!(outcome_from_resolved(h), EyeDropperOutcome::Failed(_)),
                "{h:?}"
            );
        }
    }

    #[test]
    fn only_abort_error_is_cancelled() {
        assert_eq!(
            outcome_from_error_name(Some("AbortError")),
            EyeDropperOutcome::Cancelled
        );
        for n in [
            Some("NotAllowedError"),
            Some("InvalidStateError"),
            Some("OperationError"),
            Some("Whatever"),
            None,
        ] {
            assert!(
                matches!(outcome_from_error_name(n), EyeDropperOutcome::Failed(_)),
                "{n:?}"
            );
        }
    }

    #[test]
    fn result_mapping_covers_all_branches() {
        let c = color_from_srgb_hex("#010203").unwrap();
        assert_eq!(
            outcome_from_result(Ok(c)),
            EyeDropperOutcome::Picked("#010203".into())
        );
        assert_eq!(
            outcome_from_result(Err(DeviceError::Cancelled("x"))),
            EyeDropperOutcome::Cancelled
        );
        assert_eq!(
            outcome_from_result(Err(DeviceError::Unsupported("x"))),
            EyeDropperOutcome::Unsupported
        );
        assert!(matches!(
            outcome_from_result(Err(DeviceError::PermissionDenied("x"))),
            EyeDropperOutcome::Failed(_)
        ));
        assert!(matches!(
            outcome_from_result(Err(DeviceError::Internal("x"))),
            EyeDropperOutcome::Failed(_)
        ));
    }
}
