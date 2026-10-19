//! 型サンプル「システムのアクセント色」の Web 経路（親 #103、Issue #423）。
//!
//! `fandhe_core::screens::system_accent_color_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。値の取得は CSS システムカラー
//! `AccentColor` を非表示の探り用要素の `color` に指定し、`getComputedStyle` で読み戻す。
//! `AccentColorSource` トレイトは経由しない（トレイト doc どおり）。共有するのは
//! `fandhe_device::system_accent_color::READ_INTERVAL` だけ。
//!
//! # 方式
//!
//! `setInterval(READ_INTERVAL)` を 1 マウント 1 本だけ張り、tick ごとに読み直す（`change` 系は
//! 購読しない。bfcache 復帰で古い値が残らない）。値が変わったときだけ apply する。
//! `AccentColor` を解さないブラウザではインライン指定が捨てられ空になる。その場合に継承色
//! （黒など）を読むと嘘の成功になるため、先に非対応として弾く。探り用の要素は読むたびに作り、
//! 読んだ直後に除去する。経過時間は測らない（間隔は `setInterval` 任せ。壁時計は使わない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の色を出さない（非対応は「取得できません」、解析不能は失敗）、破棄時に
//! `clearInterval` と closure・探り用要素を解放する、色を `console` やエラー文言へ出さない。
//! 守らない: 反映の遅れ（最大 `READ_INTERVAL` + 1 フレーム。背景タブではさらに遅れる）、
//! ブラウザが返す固定値（青）を本物と区別しないこと（Safari 16.5+・Firefox 103+ 等）、
//! 複数タブ間の食い違い、`forced-colors` / `color-scheme` による値の違い。

use fandhe_core::screens::system_accent_color_show::{ReadFailure, Rgb};

/// 取得失敗時の固定文言（ブラウザ固有の値を含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_LABEL: &str = "アクセント色の取得";

/// `getComputedStyle` の `color`（`rgb(R, G, B)` / `rgba(R, G, B, A)`）を解析する純関数。
/// 成分は 0..=255 の整数だけ受け、アルファは捨てる。それ以外（`color(srgb ...)` 等）は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn rgb_from_computed(value: &str) -> Option<Rgb> {
    let v = value.trim();
    let (body, has_alpha) = match v.strip_prefix("rgba(") {
        Some(b) => (b, true),
        None => (v.strip_prefix("rgb(")?, false),
    };
    let body = body.strip_suffix(')')?;
    let parts: Vec<&str> = body
        .split([',', ' ', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    // `rgba` は 4 成分、`rgb` は 3 成分（`rgb(r g b / a)` の 4 成分も許す）。
    let ok = if has_alpha {
        parts.len() == 4
    } else {
        parts.len() == 3 || parts.len() == 4
    };
    if !ok {
        return None;
    }
    let ch = |s: &str| s.parse::<u8>().ok();
    Some(Rgb {
        r: ch(parts[0])?,
        g: ch(parts[1])?,
        b: ch(parts[2])?,
    })
}

/// インライン指定の読み戻し（`inline`）と計算済みの色（`computed`）を読み取り結果へ写す。
/// `inline` が空なら `AccentColor` を解さない環境なので、`computed`（継承色）は見ずに非対応。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify(inline: &str, computed: Option<&str>) -> Result<Rgb, ReadFailure> {
    if inline.trim().is_empty() {
        return Err(ReadFailure::Unsupported);
    }
    computed
        .and_then(rgb_from_computed)
        .ok_or_else(|| ReadFailure::Failed(FAILED_LABEL.to_string()))
}

#[cfg(target_arch = "wasm32")]
pub use web::system_accent_color_show_effect;

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::system_accent_color_show::{
        apply_read, ReadFailure, Rgb, SystemAccentColorShowState,
    };
    use fandhe_device::system_accent_color::READ_INTERVAL;
    use wasm_bindgen::prelude::*;

    use super::{classify, FAILED_LABEL};
    use crate::mount::{is_live, Applier};

    struct Entry {
        handle: i32,
        // 破棄まで closure を生存させるために保持する。
        _closure: Closure<dyn FnMut()>,
    }

    thread_local! {
        /// マウントごとのタイマー（1 マウント 1 本）。`None` は登録失敗済みの印。
        static TIMERS: RefCell<HashMap<u64, Option<Entry>>> = RefCell::new(HashMap::new());
    }

    type ReadResult = Result<Rgb, ReadFailure>;

    fn failed() -> ReadFailure {
        ReadFailure::Failed(FAILED_LABEL.to_string())
    }

    /// 探り用の要素を作って読み、結果にかかわらず除去する。
    fn read() -> ReadResult {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        let document = window.document().ok_or(ReadFailure::Unsupported)?;
        let body = document.body().ok_or(ReadFailure::Unsupported)?;
        let probe = document
            .create_element("span")
            .map_err(|_| failed())?
            .dyn_into::<web_sys::HtmlElement>()
            .map_err(|_| failed())?;
        let style = probe.style();
        if style.set_property("display", "none").is_err()
            || style.set_property("color", "AccentColor").is_err()
            || body.append_child(&probe).is_err()
        {
            probe.remove();
            return Err(failed());
        }
        let inline = style.get_property_value("color").unwrap_or_default();
        let computed = window
            .get_computed_style(&probe)
            .ok()
            .flatten()
            .and_then(|s| s.get_property_value("color").ok());
        probe.remove();
        classify(&inline, computed.as_deref())
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけタイマーを張る。
    pub fn system_accent_color_show_effect(
        _state: &SystemAccentColorShowState,
        applier: &Applier<SystemAccentColorShowState>,
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

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    #[test]
    fn parses_rgb_and_rgba() {
        assert_eq!(
            rgb_from_computed("rgb(10, 132, 255)"),
            Some(rgb(10, 132, 255))
        );
        assert_eq!(
            rgb_from_computed("rgba(10, 132, 255, 0.5)"),
            Some(rgb(10, 132, 255))
        );
        assert_eq!(
            rgb_from_computed("rgb(10 132 255)"),
            Some(rgb(10, 132, 255))
        );
        assert_eq!(
            rgb_from_computed("rgb(10 132 255 / 0.5)"),
            Some(rgb(10, 132, 255))
        );
    }

    #[test]
    fn rejects_malformed() {
        for v in [
            "",
            "rgb(256, 0, 0)",
            "rgb(-1, 0, 0)",
            "rgb(1.5, 0, 0)",
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4, 5)",
            "rgba(1, 2, 3)",
            "color(srgb 0.1 0.2 0.3)",
        ] {
            assert_eq!(rgb_from_computed(v), None, "{v}");
        }
    }

    #[test]
    fn classify_three_way() {
        // 非対応: インライン指定が捨てられた環境の継承色は読まない。
        assert_eq!(
            classify("", Some("rgb(0, 0, 0)")),
            Err(ReadFailure::Unsupported)
        );
        assert!(matches!(
            classify("AccentColor", None),
            Err(ReadFailure::Failed(_))
        ));
        assert!(matches!(
            classify("AccentColor", Some("x")),
            Err(ReadFailure::Failed(_))
        ));
        assert_eq!(
            classify("accentcolor", Some("rgb(10, 132, 255)")),
            Ok(rgb(10, 132, 255))
        );
    }
}
