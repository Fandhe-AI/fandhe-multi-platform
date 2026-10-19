//! 型サンプル「セーフエリア」の Web 経路（親 #105、Issue #447）。
//!
//! `fandhe_core::screens::safe_area_insets_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。値の取得は CSS `env()` の
//! `safe-area-inset-*` を非表示の探り用要素の `padding` に指定し、`getComputedStyle` で読み戻す。
//! `SafeAreaSource` トレイトは経由しない。共有するのは `fandhe_device::safe_area_insets` の
//! `READ_INTERVAL` と値域検査 `insets_from_raw`（SSOT）だけ。
//!
//! # 方式
//!
//! `setInterval(READ_INTERVAL)` を 1 マウント 1 本だけ張り、tick ごとに読み直す（購読はしない）。
//! 値が変わったときだけ apply する。`env()` を解さないブラウザではインライン指定が捨てられ空に
//! なる。その場合に計算値（0px）を読むと嘘の成功になるため、先に非対応として弾く。
//! 探り用の要素は読むたびに作り、読んだ直後に除去する。経過時間は測らない（壁時計は使わない）。
//! `viewport-fit=cover`（`web/index.html`）が無いと iPhone Safari でも 0 になる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 非対応・解析不能を 0 に倒さない、値域外はクランプせず失敗、破棄時に `clearInterval` と
//! closure・探り用要素を解放する、値を `console` やエラー文言へ出さない。
//! 守らない範囲: 反映の遅れ（最大 `READ_INTERVAL` + 1 フレーム。背景タブではさらに遅れる）、
//! viewport の `env()` を見ること、zoom が 1 でないときのずれ、複数タブ間の食い違い。

use fandhe_core::screens::safe_area_insets_show::{Insets, ReadFailure};
use fandhe_device::safe_area_insets::insets_from_raw;
use fandhe_device::DeviceError;

/// 取得失敗時の固定文言（ブラウザ固有の値を含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_LABEL: &str = "セーフエリアの取得";

/// `getComputedStyle` の長さ（`"12.5px"`）を数値にする純関数。単位なし・別単位・空は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn px_from_computed(value: &str) -> Option<f64> {
    value.trim().strip_suffix("px")?.trim().parse::<f64>().ok()
}

/// 4 辺（top / right / bottom / left）のインライン指定の読み戻しと計算値を読み取り結果へ写す。
/// どれかのインライン指定が空なら `env()` を解さない環境なので、計算値は見ずに非対応。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify(
    inline: [&str; 4],
    computed: Option<[String; 4]>,
) -> Result<Option<Insets>, ReadFailure> {
    if inline.iter().any(|s| s.trim().is_empty()) {
        return Err(ReadFailure::Unsupported);
    }
    let failed = || ReadFailure::Failed(FAILED_LABEL.to_string());
    let computed = computed.ok_or_else(failed)?;
    let mut raw = [0.0; 4];
    for (r, c) in raw.iter_mut().zip(computed.iter()) {
        *r = px_from_computed(c).ok_or_else(failed)?;
    }
    match insets_from_raw(0, raw) {
        Ok(i) => Ok(Some(Insets {
            top: i.top,
            right: i.right,
            bottom: i.bottom,
            left: i.left,
        })),
        Err(DeviceError::Unsupported(_)) => Err(ReadFailure::Unsupported),
        Err(_) => Err(failed()),
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::safe_area_insets_show_effect;

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::safe_area_insets_show::{
        apply_read, Insets, ReadFailure, SafeAreaInsetsShowState,
    };
    use fandhe_device::safe_area_insets::READ_INTERVAL;
    use wasm_bindgen::prelude::*;

    use super::{classify, FAILED_LABEL};
    use crate::mount::{is_live, Applier};

    /// 探り用要素に指定する (padding のプロパティ名, env 変数)。top / right / bottom / left の順。
    const SIDES: [(&str, &str); 4] = [
        ("padding-top", "env(safe-area-inset-top)"),
        ("padding-right", "env(safe-area-inset-right)"),
        ("padding-bottom", "env(safe-area-inset-bottom)"),
        ("padding-left", "env(safe-area-inset-left)"),
    ];

    struct Entry {
        handle: i32,
        // 破棄まで closure を生存させるために保持する。
        _closure: Closure<dyn FnMut()>,
    }

    thread_local! {
        /// マウントごとのタイマー（1 マウント 1 本）。`None` は登録失敗済みの印。
        static TIMERS: RefCell<HashMap<u64, Option<Entry>>> = RefCell::new(HashMap::new());
    }

    type ReadResult = Result<Option<Insets>, ReadFailure>;

    fn failed() -> ReadFailure {
        ReadFailure::Failed(FAILED_LABEL.to_string())
    }

    /// 探り用の要素を作って読み、結果にかかわらず除去する。
    fn read() -> ReadResult {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        let document = window.document().ok_or(ReadFailure::Unsupported)?;
        let body = document.body().ok_or(ReadFailure::Unsupported)?;
        let probe = document
            .create_element("div")
            .map_err(|_| failed())?
            .dyn_into::<web_sys::HtmlElement>()
            .map_err(|_| failed())?;
        let style = probe.style();
        let mut ok = style.set_property("display", "none").is_ok();
        for (prop, value) in SIDES {
            ok = ok && style.set_property(prop, value).is_ok();
        }
        if !ok || body.append_child(&probe).is_err() {
            probe.remove();
            return Err(failed());
        }
        let inline = SIDES.map(|(p, _)| style.get_property_value(p).unwrap_or_default());
        let computed = window
            .get_computed_style(&probe)
            .ok()
            .flatten()
            .and_then(|s| {
                let mut out: [String; 4] = Default::default();
                for (o, (p, _)) in out.iter_mut().zip(SIDES) {
                    *o = s.get_property_value(p).ok()?;
                }
                Some(out)
            });
        probe.remove();
        classify([&inline[0], &inline[1], &inline[2], &inline[3]], computed)
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけタイマーを張る。
    pub fn safe_area_insets_show_effect(
        _state: &SafeAreaInsetsShowState,
        applier: &Applier<SafeAreaInsetsShowState>,
    ) {
        let mount_id = applier.mount_id();
        if TIMERS.with(|t| t.borrow().contains_key(&mount_id)) {
            return;
        }
        let initial = read();
        let last = Rc::new(RefCell::new(initial.clone()));

        let tick_applier = applier.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let r = read();
            if r == *last.borrow() {
                return;
            }
            *last.borrow_mut() = r.clone();
            tick_applier.apply(move |s| apply_read(s, r));
        });

        let handle = web_sys::window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                READ_INTERVAL.as_millis() as i32,
            )
            .ok()
        });
        match handle {
            Some(handle) => {
                TIMERS.with(|t| {
                    t.borrow_mut().insert(
                        mount_id,
                        Some(Entry {
                            handle,
                            _closure: closure,
                        }),
                    );
                });
                applier.apply(move |s| apply_read(s, initial));
            }
            // 最初の値を出し続けず失敗表示にする（嘘の成功を避ける）。
            None => {
                TIMERS.with(|t| t.borrow_mut().insert(mount_id, None));
                applier.apply(|s| apply_read(s, Err(failed())));
            }
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        if let Some(Some(entry)) = TIMERS.with(|t| t.borrow_mut().remove(&mount_id)) {
            if let Some(w) = web_sys::window() {
                w.clear_interval_with_handle(entry.handle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: [&str; 4]) -> Option<[String; 4]> {
        Some(v.map(String::from))
    }

    #[test]
    fn parses_px() {
        assert_eq!(px_from_computed("12.5px"), Some(12.5));
        assert_eq!(px_from_computed(" 0px "), Some(0.0));
        for v in ["", "auto", "1em", "12", "px"] {
            assert_eq!(px_from_computed(v), None, "{v}");
        }
    }

    #[test]
    fn unsupported_when_any_inline_empty() {
        let z = c(["0px"; 4]);
        assert_eq!(
            classify(["env(a)", "env(b)", "", "env(d)"], z),
            Err(ReadFailure::Unsupported)
        );
    }

    #[test]
    fn failed_on_unparsable_or_out_of_range() {
        let inl = ["e"; 4];
        assert!(matches!(classify(inl, None), Err(ReadFailure::Failed(_))));
        for bad in ["auto", "1em", "", "-3px", "NaNpx", "3000px"] {
            assert!(
                matches!(
                    classify(inl, c(["0px", "0px", bad, "0px"])),
                    Err(ReadFailure::Failed(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn reads_in_trbl_order_and_real_zero_is_success() {
        let inl = ["e"; 4];
        assert_eq!(
            classify(inl, c(["47px", "0px", "34px", "1px"])),
            Ok(Some(Insets {
                top: 47.0,
                right: 0.0,
                bottom: 34.0,
                left: 1.0
            }))
        );
        assert_eq!(
            classify(inl, c(["0px"; 4])),
            Ok(Some(Insets {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0
            }))
        );
    }
}
