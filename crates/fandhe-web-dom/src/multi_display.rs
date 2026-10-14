//! 型サンプル「複数ディスプレイ」の Web 経路（Issue #1344、親 #1134）。
//!
//! `fandhe_core::screens::multi_display_show`（端末依存ゼロ）を
//! [`super::mount::mount_with_effect`] の effect として観測し、Window Management API
//! （`window.getScreenDetails()`）の画面一覧を `finish_request` / `apply_read` へ書き戻す。
//! `fandhe-device` の `MultiDisplay` トレイトは経由しない（`fandhe-device/src/multi_display.rs`
//! の module doc どおり、Web は本クレートの担当）。契約の `validate` と `READ_INTERVAL` は共有する。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。
//!
//! # 読み取り方
//!
//! - 許可はボタン押下の時だけ求める。`getScreenDetails()` は transient activation を要するため、
//!   effect の中で**同期に**呼ぶ（`spawn_local` の中では呼ばない）。web-sys の `ScreenDetails` は
//!   unstable feature なので使わず、`Reflect` で読む
//! - 解決後は `READ_INTERVAL` ごとに `details.screens` を読み直す（自己再予約の `setTimeout`。
//!   `screenschange` は購読しない。契約どおりの pull 型）。前回と同じ結果なら反映しない
//! - 各画面から `width` / `height`（CSS px）・`devicePixelRatio`・`isPrimary`・`label` だけを読む。
//!   位置（`left` / `top`）や個体識別子は読まない。値は境界で型を検証し、想定外の型は 0 / false /
//!   `None` へ倒す（security.md A03）
//! - サイズは CSS px × `devicePixelRatio` の約。倍率が不明なら 0 になり、`validate` が一覧全体を
//!   失敗にする（CSS px を物理ピクセルと偽らない）
//! - 解決後に `navigator.permissions.query({name:"window-management"})` を試し、得られた
//!   `PermissionStatus` を毎 tick 見る。`granted` 以外なら一覧を出さず許可なしにする。
//!   query が使えない環境では取り消しを検知できない（守らない範囲）
//!
//! # 嘘の成功を避ける写像
//!
//! reject の `NotAllowedError` は許可なし（失敗とは別扱い。空の一覧にしない）、`SecurityError`
//! （permissions policy 等）は非対応、それ以外は固定文言の失敗。tick の失敗も毎回 view を置き換え、
//! 前回の一覧を残さない。
//!
//! # 停止条件
//!
//! マウント破棄（`crate::mount::is_live`）と、新しい要求への世代交代では次回を予約しない。
//! await の再開後は世代と `is_live` を再確認し、古い完了は捨てる。タイマーは `clearTimeout` して
//! から closure を drop する（`forget` しない）。`setTimeout` の予約失敗は `Failed` を出して止める。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の値を出さないこと、許可なしを空の一覧にしないこと、取り消し後に一覧を出し続けない
//! こと（`PermissionStatus` が得られる場合）、古い世代の完了を捨てること、1 マウント内でタイマーと
//! closure を漏らさないこと、個体識別子を取らないこと。
//! 守らない: Chromium 以外（非対応表示）、サイズの丸め（約）、反映遅れ（最大 1 秒＋背景タブの
//! 間引き）、許可ダイアログの応答待ちの期限、`permissions.query` が無い環境での取り消し検知、
//! 複数タブ・複数マウント、ミラーリングの扱い、画面の位置・並び、画面ごとの別描画。
//!
//! 例外の詳細（`message`）は画面にも `console` にも出さない（security.md A09）。

use fandhe_core::screens::multi_display_show::{ReadFailure, ScreenRow};
use fandhe_device::multi_display::{validate, ScreenInfo};
use fandhe_device::DeviceError;

/// 読み取りに失敗したときの固定文言（egui-proxy と同じ。例外の `message` は出さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "画面一覧の取得";

/// CSS px × `devicePixelRatio` を物理ピクセル（約）へ。非有限・0 以下・`u32` 超過・倍率不明は 0。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn px(css: Option<f64>, dpr: Option<f64>) -> u32 {
    let v = (css.unwrap_or(0.0) * dpr.unwrap_or(0.0)).round();
    if v.is_finite() && v > 0.0 && v <= f64::from(u32::MAX) {
        v as u32
    } else {
        0
    }
}

/// `ScreenDetailed` の生値 1 画面分を契約の `ScreenInfo` へ写す純関数（`validate` は呼び出し側）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn screen_from_raw(
    width: Option<f64>,
    height: Option<f64>,
    dpr: Option<f64>,
    is_primary: Option<bool>,
    label: Option<String>,
) -> ScreenInfo {
    ScreenInfo {
        width_px: px(width, dpr),
        height_px: px(height, dpr),
        scale: dpr.map(|d| d as f32),
        is_primary: is_primary.unwrap_or(false),
        name: label,
    }
}

/// 契約の結果を画面の入力へ写す（egui-proxy と同じ写像）。失敗は固定文言へ畳む。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_result(
    r: Result<Vec<ScreenInfo>, DeviceError>,
) -> Result<Vec<ScreenRow>, ReadFailure> {
    match r {
        Ok(v) => Ok(v
            .into_iter()
            .map(|s| ScreenRow {
                width_px: s.width_px,
                height_px: s.height_px,
                scale: s.scale,
                is_primary: s.is_primary,
                name: s.name,
            })
            .collect()),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(ReadFailure::Failed(READ_FAILED.to_string())),
    }
}

/// 生値の画面一覧を `validate` に通して表示用へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn rows_from_screens(screens: Vec<ScreenInfo>) -> Result<Vec<ScreenRow>, ReadFailure> {
    to_result(validate(Ok(screens)))
}

/// `getScreenDetails()` が reject した例外の `name` を失敗種別へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_name(name: &str) -> ReadFailure {
    match name {
        "NotAllowedError" => ReadFailure::PermissionDenied,
        "SecurityError" => ReadFailure::Unsupported,
        _ => ReadFailure::Failed(READ_FAILED.to_string()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::multi_display_show::{
        apply_read, finish_request, take_request, MultiDisplayShowState, ReadFailure, ScreenRow,
    };
    use fandhe_device::multi_display::READ_INTERVAL;
    use js_sys::{Array, Function, Object, Reflect};
    use wasm_bindgen::prelude::*;

    use super::{failure_from_name, rows_from_screens, screen_from_raw, READ_FAILED};
    use crate::mount::{is_live, Applier};

    type Rows = Result<Vec<ScreenRow>, ReadFailure>;
    /// `setTimeout` のハンドルと、その発火で呼ばれるクロージャ。
    type PendingTimer = (i32, Closure<dyn FnMut()>);

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に発送しない・古い完了の照合）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        /// 予約中タイマー（マウントごとに 1 本）。次回予約・停止時に clear してから drop する。
        static TIMERS: RefCell<HashMap<u64, PendingTimer>> = RefCell::new(HashMap::new());
    }

    fn failed() -> ReadFailure {
        ReadFailure::Failed(READ_FAILED.to_string())
    }

    fn get(target: &JsValue, name: &str) -> JsValue {
        Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
    }

    /// 保持中のタイマーを clearTimeout してから closure を解放する（無ければ何もしない）。
    fn drop_timer(mount_id: u64) {
        let old = TIMERS.with(|t| t.borrow_mut().remove(&mount_id));
        if let Some((handle, closure)) = old {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(handle);
            }
            drop(closure);
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録とタイマーを解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
        drop_timer(mount_id);
    }

    /// このマウントの現在の要求が `generation` か。
    fn is_current(mount_id: u64, generation: u64) -> bool {
        is_live(mount_id)
            && DISPATCHED.with(|d| d.borrow().get(&mount_id).copied()) == Some(generation)
    }

    /// `details.screens` を読んで契約を通す。
    fn read_screens(details: &JsValue) -> Rows {
        let list = get(details, "screens");
        if !Array::is_array(&list) {
            return Err(failed());
        }
        let screens = Array::from(&list)
            .iter()
            .map(|s| {
                screen_from_raw(
                    get(&s, "width").as_f64(),
                    get(&s, "height").as_f64(),
                    get(&s, "devicePixelRatio").as_f64(),
                    get(&s, "isPrimary").as_bool(),
                    get(&s, "label").as_string(),
                )
            })
            .collect();
        rows_from_screens(screens)
    }

    /// 取り消しを追うための `PermissionStatus`。取れない環境では `None`。
    async fn query_permission() -> Option<JsValue> {
        let window = web_sys::window()?;
        let navigator: JsValue = window.navigator().into();
        let permissions = get(&navigator, "permissions");
        let query = get(&permissions, "query");
        if !query.is_function() {
            return None;
        }
        let desc = Object::new();
        Reflect::set(&desc, &"name".into(), &"window-management".into()).ok()?;
        let promise = query
            .unchecked_into::<Function>()
            .call1(&permissions, &desc)
            .ok()?
            .dyn_into::<js_sys::Promise>()
            .ok()?;
        let status = wasm_bindgen_futures::JsFuture::from(promise).await.ok()?;
        status.is_object().then_some(status)
    }

    /// `READ_INTERVAL` ごとの読み取り。自己再予約し、世代交代・破棄で止まる。
    fn tick(
        mount_id: u64,
        generation: u64,
        applier: Applier<MultiDisplayShowState>,
        details: JsValue,
        permission: Option<JsValue>,
        last: Option<Rows>,
    ) {
        if !is_current(mount_id, generation) {
            // 世代交代なら新しい要求が自分のタイマーを持つので触らない。破棄なら release 済み。
            return;
        }
        let revoked = permission
            .as_ref()
            .is_some_and(|p| get(p, "state").as_string().as_deref() != Some("granted"));
        let result = if revoked {
            Err(ReadFailure::PermissionDenied)
        } else {
            read_screens(&details)
        };
        if last.as_ref() != Some(&result) {
            let r = result.clone();
            // 新しい要求の許可待ち（Requesting）には割り込まない。
            applier.apply(move |s| {
                if take_request(s).is_none() {
                    apply_read(s, r)
                }
            });
        }
        let reporter = applier.clone();
        let next = Closure::once(move || {
            tick(
                mount_id,
                generation,
                applier,
                details,
                permission,
                Some(result),
            )
        });
        let handle = web_sys::window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                next.as_ref().unchecked_ref(),
                READ_INTERVAL.as_millis() as i32,
            )
            .ok()
        });
        match handle {
            Some(handle) => {
                // 発火中の旧ハンドルは発火済み。置き換えて drop するだけでよい。
                let old = TIMERS.with(|t| t.borrow_mut().insert(mount_id, (handle, next)));
                drop(old);
            }
            None => {
                drop(next);
                drop_timer(mount_id);
                reporter.apply(|s| {
                    if take_request(s).is_none() {
                        apply_read(s, Err(failed()))
                    }
                });
            }
        }
    }

    /// ボタン押下の同期区間で `getScreenDetails()` を呼ぶ。
    fn call_get_screen_details() -> Result<js_sys::Promise, ReadFailure> {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        let target: JsValue = window.clone().into();
        let f = get(&target, "getScreenDetails");
        if !window.is_secure_context() || !f.is_function() {
            return Err(ReadFailure::Unsupported);
        }
        f.unchecked_into::<Function>()
            .call0(&target)
            .ok()
            .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
            .ok_or_else(failed)
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ発送する。
    pub fn multi_display_show_effect(
        state: &MultiDisplayShowState,
        applier: &Applier<MultiDisplayShowState>,
    ) {
        let Some(generation) = take_request(state) else {
            return;
        };
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        // 旧要求のタイマーを止める（予約済みを残さない）。
        drop_timer(mount_id);
        let promise = match call_get_screen_details() {
            Ok(p) => p,
            Err(f) => {
                applier.apply(move |s| finish_request(s, generation, Err(f)));
                return;
            }
        };
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let settled = wasm_bindgen_futures::JsFuture::from(promise).await;
            if !is_current(mount_id, generation) {
                return;
            }
            let details = match settled {
                Ok(d) if d.is_object() => d,
                other => {
                    let f = match other {
                        Err(e) => {
                            failure_from_name(&get(&e, "name").as_string().unwrap_or_default())
                        }
                        Ok(_) => failed(),
                    };
                    applier.apply(move |s| finish_request(s, generation, Err(f)));
                    return;
                }
            };
            let permission = query_permission().await;
            if !is_current(mount_id, generation) {
                return;
            }
            let first = read_screens(&details);
            let r = first.clone();
            applier.apply(move |s| finish_request(s, generation, r));
            tick(
                mount_id,
                generation,
                applier,
                details,
                permission,
                Some(first),
            );
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::multi_display_show_effect;

/// マウント破棄時に発送記録とタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_rounds_css_times_dpr() {
        assert_eq!(px(Some(1440.0), Some(2.0)), 2880);
        assert_eq!(px(Some(1000.4), Some(1.5)), 1501);
    }

    #[test]
    fn px_bad_inputs_become_zero() {
        for (c, d) in [
            (Some(f64::NAN), Some(2.0)),
            (Some(f64::INFINITY), Some(2.0)),
            (Some(0.0), Some(2.0)),
            (Some(-5.0), Some(2.0)),
            (Some(1e12), Some(2.0)),
            (Some(100.0), None),
            (Some(100.0), Some(0.0)),
            (None, Some(2.0)),
        ] {
            assert_eq!(px(c, d), 0);
        }
    }

    #[test]
    fn unknown_dpr_fails_whole_list() {
        let ok = screen_from_raw(Some(100.0), Some(50.0), Some(2.0), Some(true), None);
        let bad = screen_from_raw(Some(100.0), Some(50.0), None, Some(false), None);
        assert_eq!(
            rows_from_screens(vec![ok, bad]),
            Err(ReadFailure::Failed(READ_FAILED.to_string()))
        );
    }

    #[test]
    fn empty_list_is_failure() {
        assert_eq!(
            rows_from_screens(vec![]),
            Err(ReadFailure::Failed(READ_FAILED.to_string()))
        );
    }

    #[test]
    fn maps_fields_and_keeps_label_untrimmed() {
        let a = screen_from_raw(
            Some(1512.0),
            Some(982.0),
            Some(2.0),
            Some(true),
            Some(" 内蔵 ".into()),
        );
        let rows = rows_from_screens(vec![a]).unwrap();
        assert_eq!(rows[0].width_px, 3024);
        assert_eq!(rows[0].height_px, 1964);
        assert_eq!(rows[0].scale, Some(2.0));
        assert!(rows[0].is_primary);
        assert_eq!(rows[0].name.as_deref(), Some(" 内蔵 "));
    }

    #[test]
    fn empty_label_is_none_and_two_primaries_clear_all() {
        let mk =
            |p, l: &str| screen_from_raw(Some(10.0), Some(10.0), Some(1.0), p, Some(l.to_string()));
        let rows = rows_from_screens(vec![mk(Some(true), ""), mk(Some(true), "b")]).unwrap();
        assert_eq!(rows[0].name, None);
        assert!(rows.iter().all(|r| !r.is_primary));
        // isPrimary が bool でなければ false。
        let rows = rows_from_screens(vec![mk(None, "a")]).unwrap();
        assert!(!rows[0].is_primary);
    }

    #[test]
    fn to_result_maps_errors() {
        assert_eq!(
            to_result(Err(DeviceError::Unsupported("multi_display"))),
            Err(ReadFailure::Unsupported)
        );
        assert_eq!(
            to_result(Err(DeviceError::Internal("x"))),
            Err(ReadFailure::Failed(READ_FAILED.to_string()))
        );
    }

    #[test]
    fn error_names_are_classified() {
        assert_eq!(
            failure_from_name("NotAllowedError"),
            ReadFailure::PermissionDenied
        );
        assert_eq!(failure_from_name("SecurityError"), ReadFailure::Unsupported);
        let failed = ReadFailure::Failed(READ_FAILED.to_string());
        assert_eq!(failure_from_name("AbortError"), failed);
        assert_eq!(failure_from_name(""), failed);
    }
}
