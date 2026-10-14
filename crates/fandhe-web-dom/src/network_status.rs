//! 型サンプル「ネットワーク接続状態」の Web 経路（親 #1130、Issue #1285）。
//!
//! `fandhe_core::screens::network_status_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。値の取得は
//! `navigator.onLine` と（Chromium の一部のみ）`navigator.connection.type`。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外。共有するのは
//! `fandhe_device::network_status::READ_INTERVAL` だけで、core の `Reading` へ直接写す。
//!
//! # 方式
//!
//! `setInterval(READ_INTERVAL)` で読み直す pull 型。`online` / `offline` は購読しない
//! （bfcache 復帰で取り逃した古い値も次の tick で直る。反映遅れは最大 1 秒、
//! 背景タブではブラウザの間引きでさらに遅れる）。従量制を示す API は無いので常に不明。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: `onLine=false` は接続なし、`type="none"` は矛盾するので接続ありにせず不明、
//! タイマー登録失敗は失敗表示、マウント破棄で `clearInterval` と closure を解放する。
//! 守らない: インターネット到達性、反映遅れ、複数タブ間の食い違い。
//!
//! 取得した文字列は固定の列挙値と照合するだけで DOM・ログへ出さない（security.md A03/A09）。

use fandhe_core::screens::network_status_show::{KindView, Reading};

/// タイマー登録失敗時の固定文言（egui-proxy 側と同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_LABEL: &str = "接続状態の取得";

/// `navigator.onLine` と `connection.type` を画面の [`Reading`] へ写す純関数。
/// native からもテストできるよう `wasm32` gate の外に置く。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from(on_line: bool, conn_type: Option<&str>) -> Reading {
    if !on_line {
        return Reading::Offline;
    }
    let kind = match conn_type {
        Some("none") => return Reading::Unknown,
        Some("wifi") => KindView::Wifi,
        Some("cellular") => KindView::Cellular,
        Some("ethernet") => KindView::Ethernet,
        Some("bluetooth" | "wimax" | "other" | "mixed") => KindView::Other,
        _ => KindView::Unknown,
    };
    Reading::Online {
        kind,
        metered: None,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::network_status_show_effect;

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::network_status_show::{
        apply_read, NetworkStatusShowState, ReadFailure, Reading,
    };
    use fandhe_device::network_status::READ_INTERVAL;
    use wasm_bindgen::prelude::*;

    use super::{reading_from, FAILED_LABEL};
    use crate::mount::{is_live, Applier};

    /// `Entry` が `None` のマウントは登録失敗済み。再描画のたびに effect が走り直しても
    /// 失敗表示を繰り返さないための印。
    struct Entry {
        handle: i32,
        // 破棄まで closure を生存させるために保持する。
        _closure: Closure<dyn FnMut()>,
    }

    thread_local! {
        /// マウントごとのタイマー（1 マウント 1 本）。
        static TIMERS: RefCell<HashMap<u64, Option<Entry>>> = RefCell::new(HashMap::new());
    }

    type ReadResult = Result<Reading, ReadFailure>;

    fn read() -> ReadResult {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        let nav = window.navigator();
        let conn_type = js_sys::Reflect::get(&nav, &JsValue::from_str("connection"))
            .ok()
            .filter(|c| c.is_object())
            .and_then(|c| js_sys::Reflect::get(&c, &JsValue::from_str("type")).ok())
            .and_then(|t| t.as_string());
        Ok(reading_from(nav.on_line(), conn_type.as_deref()))
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけタイマーを張る。
    pub fn network_status_show_effect(
        _state: &NetworkStatusShowState,
        applier: &Applier<NetworkStatusShowState>,
    ) {
        let mount_id = applier.mount_id();
        if TIMERS.with(|t| t.borrow().contains_key(&mount_id)) {
            return;
        }
        let initial = read();
        let last: Rc<RefCell<ReadResult>> = Rc::new(RefCell::new(initial.clone()));

        let tick_applier = applier.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let now = read();
            if *last.borrow() == now {
                return;
            }
            *last.borrow_mut() = now.clone();
            tick_applier.apply(move |s| apply_read(s, now));
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
            // 失敗済みの印を残し、再描画で effect が走り直しても何もしない。
            None => {
                TIMERS.with(|t| t.borrow_mut().insert(mount_id, None));
                applier
                    .apply(|s| apply_read(s, Err(ReadFailure::Failed(FAILED_LABEL.to_string()))));
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

    fn online(kind: KindView) -> Reading {
        Reading::Online {
            kind,
            metered: None,
        }
    }

    #[test]
    fn offline_wins_over_type() {
        assert_eq!(reading_from(false, None), Reading::Offline);
        assert_eq!(reading_from(false, Some("wifi")), Reading::Offline);
    }

    #[test]
    fn none_type_is_never_online() {
        assert_eq!(reading_from(true, Some("none")), Reading::Unknown);
    }

    #[test]
    fn kinds_map() {
        assert_eq!(reading_from(true, Some("wifi")), online(KindView::Wifi));
        assert_eq!(
            reading_from(true, Some("cellular")),
            online(KindView::Cellular)
        );
        assert_eq!(
            reading_from(true, Some("ethernet")),
            online(KindView::Ethernet)
        );
        for t in ["bluetooth", "wimax", "other", "mixed"] {
            assert_eq!(reading_from(true, Some(t)), online(KindView::Other));
        }
        for t in [Some("unknown"), None, Some("<script>")] {
            assert_eq!(reading_from(true, t), online(KindView::Unknown));
        }
    }
}
