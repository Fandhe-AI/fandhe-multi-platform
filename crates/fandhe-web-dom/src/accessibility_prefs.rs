//! 型サンプル「アクセシビリティ設定の追従」の Web 経路（親 #1132、Issue #1311）。
//!
//! `fandhe_core::screens::accessibility_prefs_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。値の取得は
//! `matchMedia('(prefers-reduced-motion: reduce)')` と `matchMedia('(prefers-contrast: more)')`。
//! `AccessibilityPrefs` トレイトは経由しない（トレイト doc どおり）。`fandhe-egui-proxy` の
//! wasm canvas 経路は対象外で、共有するのは `fandhe_device::accessibility_prefs::READ_INTERVAL` だけ。
//!
//! # 方式
//!
//! `setInterval(FRAME_MS)` を 1 マウント 1 本だけ張る。1 回の tick で、前回の読み取りから
//! `READ_INTERVAL` 以上たっていれば `matchMedia` を読み直し（`change` は購読しない。bfcache
//! 復帰で古い値が残らない）、視差軽減が「オン」でないあいだは丸の位相を進める。
//! 未対応判定は、肯定クエリ（`more` / `reduce`）と否定クエリ（`no-preference`）の
//! 両方が `matches == false` のとき（未知の特性は構文が有効でも常に偽）か `media == "not all"` のとき。
//! その項目は `None`（不明）にする。`no-preference` 側が真なら「オフ」。
//! 経過時間は単調時計 `performance.now()` で測る（システム時計の巻き戻しの影響を受けない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 嘘の「オン」「オフ」を出さない（未対応・取得不能は不明で、丸は動き枠は通常）、
//! 「オン」のあいだ丸を止める、タイマー登録失敗は失敗表示、マウント破棄で `clearInterval` と
//! closure を解放する。
//! 守らない: 反映の遅れ（最大 `READ_INTERVAL` + 1 tick。背景タブでは間引きでさらに遅れる）、
//! 複数タブ間の食い違い、`forced-colors` / `prefers-contrast: custom` の扱い、OS 間で設定の
//! 意味が一致すること。
//!
//! メディアクエリは定数のみ。`media()` は `"not all"` と比べるだけで DOM・ログへ出さない
//! （security.md A03/A09）。

/// タイマー登録失敗・クエリ例外時の固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_LABEL: &str = "設定の取得";

/// 肯定クエリ `pos` と否定クエリ `neg`（`no-preference`）の `matchMedia` 結果
/// （`media()`, `matches()`）を設定値へ写す純関数。
/// null・`"not all"`・両方偽（未対応特性は構文が有効でも偽になる）は不明。
/// native からもテストできるよう `wasm32` gate の外に置く。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn pref_from(pos: Option<(&str, bool)>, neg: Option<(&str, bool)>) -> Option<bool> {
    match (pos, neg) {
        (Some((pm, pb)), Some((nm, nb))) if pm != "not all" && nm != "not all" && (pb || nb) => {
            Some(pb)
        }
        _ => None,
    }
}

/// 前回の読み取りから `interval_ms` 以上たったか（初回は常に真）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn read_due(last_read_ms: Option<f64>, now_ms: f64, interval_ms: f64) -> bool {
    last_read_ms.is_none_or(|p| now_ms - p >= interval_ms)
}

/// 前回の tick からの経過秒を `[0, max]` に収める。初回は `default`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn frame_dt(prev_ms: Option<f64>, now_ms: f64, default: f32, max: f32) -> f32 {
    match prev_ms {
        Some(p) => (((now_ms - p) / 1000.0) as f32).clamp(0.0, max),
        None => default,
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::accessibility_prefs_show_effect;

/// マウント破棄時にタイマーを解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::accessibility_prefs_show::{
        advance_phase, animates, apply_read, AccessibilityPrefsShowState, Prefs, ReadFailure,
    };
    use fandhe_device::accessibility_prefs::READ_INTERVAL;
    use wasm_bindgen::prelude::*;

    use super::{frame_dt, pref_from, read_due, FAILED_LABEL};
    use crate::mount::{is_live, Applier};

    /// tick 間隔（約 60fps。止まっているあいだも空振りの tick は走る）。
    // ponytail: 停止中も 16ms ごとに空振り。電池が問題なら rAF へ切り替えるか停止中は間隔を延ばす。
    const FRAME_MS: i32 = 16;
    /// 1 tick で進める時間の上限（背景タブで間引かれても位相が飛ばない）。
    const MAX_DT: f32 = 0.1;
    /// 初回 tick の既定 dt（60fps 相当）。
    const DEFAULT_DT: f32 = 1.0 / 60.0;

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

    type ReadResult = Result<Prefs, ReadFailure>;

    /// tick が持ち越す状態。
    struct Tick {
        last: ReadResult,
        last_read_ms: Option<f64>,
        last_tick_ms: Option<f64>,
    }

    fn mq(window: &web_sys::Window, q: &str) -> Result<Option<(String, bool)>, ReadFailure> {
        let mql = window
            .match_media(q)
            .map_err(|_| ReadFailure::Failed(FAILED_LABEL.to_string()))?;
        Ok(mql.map(|m| (m.media(), m.matches())))
    }

    fn query(window: &web_sys::Window, pos: &str, neg: &str) -> Result<Option<bool>, ReadFailure> {
        let p = mq(window, pos)?;
        let n = mq(window, neg)?;
        Ok(pref_from(
            p.as_ref().map(|(s, b)| (s.as_str(), *b)),
            n.as_ref().map(|(s, b)| (s.as_str(), *b)),
        ))
    }

    /// 単調時計（ミリ秒）。取得不能なら `None`（呼び出し側は毎 tick 読み取り、dt は既定値にする）。
    fn now_ms() -> Option<f64> {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
    }

    fn read() -> ReadResult {
        let window = web_sys::window().ok_or(ReadFailure::Unsupported)?;
        Ok(Prefs {
            reduce_motion: query(
                &window,
                "(prefers-reduced-motion: reduce)",
                "(prefers-reduced-motion: no-preference)",
            )?,
            increase_contrast: query(
                &window,
                "(prefers-contrast: more)",
                "(prefers-contrast: no-preference)",
            )?,
        })
    }

    /// core の `animates` に判定を任せる（ロジックを複製しない）。
    fn moving(r: &ReadResult) -> bool {
        let mut s = AccessibilityPrefsShowState::default();
        apply_read(&mut s, r.clone());
        animates(&s)
    }

    /// `mount_with_effect` へ渡す effect。マウントごとに 1 度だけタイマーを張る。
    pub fn accessibility_prefs_show_effect(
        _state: &AccessibilityPrefsShowState,
        applier: &Applier<AccessibilityPrefsShowState>,
    ) {
        let mount_id = applier.mount_id();
        if TIMERS.with(|t| t.borrow().contains_key(&mount_id)) {
            return;
        }
        let initial = read();
        let tick = Rc::new(RefCell::new(Tick {
            last: initial.clone(),
            last_read_ms: now_ms(),
            last_tick_ms: None,
        }));

        let tick_applier = applier.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            if !is_live(mount_id) {
                return;
            }
            let now = now_ms();
            let mut t = tick.borrow_mut();
            // 時計が無いときは 0 に固定せず、毎 tick 読み取り・dt は既定値で動かす。
            let dt = now.map_or(DEFAULT_DT, |n| {
                frame_dt(t.last_tick_ms, n, DEFAULT_DT, MAX_DT)
            });
            t.last_tick_ms = now;
            let mut fresh = None;
            let due =
                now.is_none_or(|n| read_due(t.last_read_ms, n, READ_INTERVAL.as_millis() as f64));
            if due {
                t.last_read_ms = now;
                let r = read();
                if r != t.last {
                    t.last = r.clone();
                    fresh = Some(r);
                }
            }
            let moves = moving(&t.last);
            drop(t);
            // 新しい値が無く止まっているときだけ apply を省く。次の読み取りは同じタイマーが担う。
            if fresh.is_none() && !moves {
                return;
            }
            tick_applier.apply(move |s| {
                if let Some(r) = fresh {
                    apply_read(s, r);
                }
                if animates(s) {
                    advance_phase(s, dt);
                }
            });
        });

        let handle = web_sys::window().and_then(|w| {
            w.set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                FRAME_MS,
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

    #[test]
    fn unknown_media_is_none() {
        let m = Some(("(x)", false));
        assert_eq!(pref_from(None, m), None);
        assert_eq!(pref_from(m, None), None);
        assert_eq!(pref_from(Some(("not all", false)), m), None);
        // 未対応特性: 構文は有効で media は保たれるが、肯定・否定とも偽。
        assert_eq!(pref_from(m, m), None);
    }

    #[test]
    fn known_media_passes_matches() {
        let t = Some(("(x)", true));
        let f = Some(("(x)", false));
        assert_eq!(pref_from(t, f), Some(true));
        assert_eq!(pref_from(f, t), Some(false));
    }

    #[test]
    fn read_due_boundary() {
        assert!(read_due(None, 0.0, 1000.0));
        assert!(!read_due(Some(0.0), 999.0, 1000.0));
        assert!(read_due(Some(0.0), 1000.0, 1000.0));
    }

    #[test]
    fn dt_clamped() {
        assert_eq!(frame_dt(None, 5.0, 0.5, 0.1), 0.5);
        assert_eq!(frame_dt(Some(0.0), 50.0, 0.5, 0.1), 0.05);
        assert_eq!(frame_dt(Some(0.0), 5000.0, 0.5, 0.1), 0.1);
        assert_eq!(frame_dt(Some(100.0), 0.0, 0.5, 0.1), 0.0);
    }
}
