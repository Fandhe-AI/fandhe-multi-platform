//! 型サンプル「システムの色選択ダイアログ」の Web 経路（Issue #943、親 #184）。
//!
//! `fandhe_core::screens::color_picker_pick`（端末依存ゼロ）を [`super::mount::mount_with_effect`] の
//! effect として観測し、`<input type="color">` を開いて `change` の値を `finish_pick` へ書き戻す。
//! 呼び出し元は `demo.rs` の `mount_color_picker_screen`。契約（初期色・`#rrggbb` の検査）は
//! `fandhe_device::color_picker` が SSOT で、macOS / Windows / iOS 実装（egui-proxy）と共用する。
//! `fandhe-device` の `ColorPicker` トレイトは経由しない（Web は本クレートの担当）。
//!
//! # ブラウザ側の制約
//!
//! - 開くのはユーザー操作の中で行う。effect の同期区間で `showPicker()`（無ければ `click()`）を呼び、
//!   その前に await を挟まない
//! - `type = "color"` の読み戻しが `color` でなければ非対応（`Unsupported`）。対応状況を前もって
//!   問い合わせず、ブラウザの結果で判定する
//! - `showPicker()` は描画されていない要素で例外になりうるため、`hidden` / `display:none` を使わず
//!   透明・1px で描画対象のまま置く。`alpha` / `colorspace` は付けない（`value` が `#rrggbb` 以外になりうる）
//!
//! # 結果の写像
//!
//! - `change`: `value` を `color_from_input_value` で検査（trim しない）。不正は `Failed`
//! - 初期色を入れてから購読する（初期設定を「選んだ」と取り違えない）。初期色との比較で推測しない
//! - `showPicker()` の例外は `Failed`。例外の `message` は読まない
//! - `<input type="color">` には取り消しの通知が無い（`cancel` は file 型のみ）ので `Cancelled` は返さない。
//!   利用者は画面の「やめる」（`abandon_pick`）で抜ける
//!
//! # ライフサイクル
//!
//! 要求ごとに `<input>` を新しく作り、`document.body` 直下へ置く。確定・やめた世代の検知・
//! マウント破棄（`release_mount`）・生成後の失敗のいずれでも、リスナーを外して `<input>` を除く。
//! `pagehide`（bfcache 退避）では保留を維持する。結果は `change` が正で、復帰後も「やめる」で抜けられる。
//! 期限・タイマーは持たない。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 非対応・不正値・失敗を成功に見せないこと、世代照合、`<input>` とリスナーの解放、
//! 選んだ色と例外の `message` をログ・永続化に出さないこと。
//! 守らない: 色を変えずに閉じた場合（`change` が来ず `Picking` のまま。「やめる」で抜ける）と
//! 初期色と同じ色を選んだ場合の区別、ブラウザごとの UI・`change` の時機の差、複数タブ・複数マウント。

use fandhe_core::screens::color_picker_pick::ColorPickerOutcome;
use fandhe_device::color_picker::color_from_input_value;
use fandhe_device::eye_dropper::PickedColor;
use fandhe_device::DeviceError;

/// `Result<PickedColor, DeviceError>` を画面用の結果へ畳む。egui-proxy の同名処理と同じ写像
/// （web-dom は egui-proxy に依存できないため重複を許容する）。権限拒否が来ても成功にしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_result(result: Result<PickedColor, DeviceError>) -> ColorPickerOutcome {
    match result {
        Ok(c) => ColorPickerOutcome::Picked(c.rgb()),
        Err(DeviceError::Cancelled(_)) => ColorPickerOutcome::Cancelled,
        Err(DeviceError::Unsupported(_)) => ColorPickerOutcome::Unsupported,
        Err(e) => ColorPickerOutcome::Failed(e.to_string()),
    }
}

/// `change` 時の `input.value` から結果を作る。取れない・形式不正は `Failed`（嘘の成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_value(value: Option<&str>) -> ColorPickerOutcome {
    let result = match value {
        Some(v) => color_from_input_value(v),
        None => Err(DeviceError::Internal("color_picker hex")),
    };
    outcome_from_result(result)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::color_picker_pick::{
        finish_pick, mark_picking, take_request, ColorPickerOutcome, ColorPickerState,
    };
    use fandhe_device::color_picker::INITIAL_RGB;
    use fandhe_device::DeviceError;
    use js_sys::{Function, Reflect};
    use wasm_bindgen::prelude::*;
    use web_sys::{Event, HtmlInputElement};

    use super::{outcome_from_result, outcome_from_value};
    use crate::mount::Applier;

    /// 飛行中の要求。結果の確定前に必ず解放する。
    struct Entry {
        generation: u64,
        input: HtmlInputElement,
        on_change: Closure<dyn FnMut(Event)>,
    }

    thread_local! {
        static ACTIVE: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn teardown(e: Entry) {
        let _ = e
            .input
            .remove_event_listener_with_callback("change", e.on_change.as_ref().unchecked_ref());
        e.input.remove();
        // Closure はここで drop される。確定は spawn_local 経由なので、実行中の closure 自身ではない。
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に `<input>`・リスナーを解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(e) = ACTIVE.with(|a| a.borrow_mut().remove(&mount_id)) {
            teardown(e);
        }
    }

    /// 世代が一致する記録だけを解放する（新しい世代を巻き込まない）。
    fn release_if(mount_id: u64, generation: u64) {
        let e = ACTIVE.with(|a| {
            let mut map = a.borrow_mut();
            if map
                .get(&mount_id)
                .is_some_and(|e| e.generation == generation)
            {
                map.remove(&mount_id)
            } else {
                None
            }
        });
        if let Some(e) = e {
            teardown(e);
        }
    }

    /// 片付けてから結果を届ける（届けた後で片付ける順序にしない）。
    fn settle(
        applier: &Applier<ColorPickerState>,
        mount_id: u64,
        generation: u64,
        outcome: ColorPickerOutcome,
    ) {
        release_if(mount_id, generation);
        applier.apply(move |s| finish_pick(s, generation, outcome));
    }

    fn failed(reason: &'static str) -> ColorPickerOutcome {
        outcome_from_result(Err(DeviceError::Internal(reason)))
    }

    /// 透明・1px で描画対象のまま置く（`showPicker()` は描画されない要素で例外になりうる）。
    fn make_input() -> Result<HtmlInputElement, ColorPickerOutcome> {
        let doc = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| failed("color_picker unavailable"))?;
        let input = doc
            .create_element("input")
            .ok()
            .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
            .ok_or_else(|| failed("color_picker unavailable"))?;
        input.set_type("color");
        if input.type_() != "color" {
            return Err(ColorPickerOutcome::Unsupported);
        }
        // 初期色を入れてから購読する。
        input.set_value(&format!("#{INITIAL_RGB:06x}"));
        let styled = input
            .set_attribute(
                "style",
                "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;border:0;padding:0;pointer-events:none",
            )
            .and_then(|_| input.set_attribute("aria-hidden", "true"))
            .and_then(|_| input.set_attribute("tabindex", "-1"));
        let body = doc
            .body()
            .ok_or_else(|| failed("color_picker unavailable"))?;
        if styled.is_err() || body.append_child(&input).is_err() {
            return Err(failed("color_picker attach"));
        }
        Ok(input)
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ色選択を開く。
    pub fn color_picker_pick_effect(state: &ColorPickerState, applier: &Applier<ColorPickerState>) {
        let mount_id = applier.mount_id();
        // やめて世代が進んだ要求の `<input>` は残さない。
        let stale = ACTIVE.with(|a| {
            a.borrow()
                .get(&mount_id)
                .is_some_and(|e| e.generation != state.generation)
        });
        if stale {
            release_mount(mount_id);
        }
        let Some(generation) = take_request(state) else {
            return;
        };
        if ACTIVE.with(|a| a.borrow().contains_key(&mount_id)) {
            return;
        }
        applier.apply(mark_picking);

        let input = match make_input() {
            Ok(i) => i,
            Err(o) => return settle(applier, mount_id, generation, o),
        };
        let cb_applier = applier.clone();
        let cb_input = input.clone();
        let on_change = Closure::<dyn FnMut(Event)>::new(move |_e: Event| {
            let outcome = outcome_from_value(Some(&cb_input.value()));
            let applier = cb_applier.clone();
            // 実行中の closure を自分の中で drop しないよう、確定は別タスクへ逃がす。
            wasm_bindgen_futures::spawn_local(async move {
                settle(&applier, mount_id, generation, outcome);
            });
        });
        if input
            .add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())
            .is_err()
        {
            input.remove();
            return settle(applier, mount_id, generation, failed("color_picker listen"));
        }
        ACTIVE.with(|a| {
            a.borrow_mut().insert(
                mount_id,
                Entry {
                    generation,
                    input: input.clone(),
                    on_change,
                },
            )
        });

        // クリックからの同期チェーンで開く（この前に await を挟まない）。
        let show = Reflect::get(&input, &"showPicker".into()).unwrap_or(JsValue::UNDEFINED);
        if show.is_function() {
            if show.unchecked_into::<Function>().call0(&input).is_err() {
                settle(applier, mount_id, generation, failed("color_picker show"));
            }
        } else {
            input.click();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::color_picker_pick_effect;

/// マウント破棄時に `<input>`・リスナーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_hex_is_picked() {
        assert_eq!(
            outcome_from_value(Some("#AABBCC")),
            ColorPickerOutcome::Picked(0xaabbcc)
        );
    }

    #[test]
    fn invalid_value_never_picked() {
        for v in [
            None,
            Some(""),
            Some("#abc"),
            Some(" #aabbcc"),
            Some("#aabbcc "),
            Some("aabbcc"),
        ] {
            assert!(
                matches!(outcome_from_value(v), ColorPickerOutcome::Failed(_)),
                "{v:?}"
            );
        }
    }

    #[test]
    fn result_mapping_covers_all_branches() {
        assert_eq!(
            outcome_from_result(Err(DeviceError::Cancelled("x"))),
            ColorPickerOutcome::Cancelled
        );
        assert_eq!(
            outcome_from_result(Err(DeviceError::Unsupported("x"))),
            ColorPickerOutcome::Unsupported
        );
        for e in [
            DeviceError::PermissionDenied("x"),
            DeviceError::Internal("x"),
        ] {
            assert!(matches!(
                outcome_from_result(Err(e)),
                ColorPickerOutcome::Failed(_)
            ));
        }
    }
}
