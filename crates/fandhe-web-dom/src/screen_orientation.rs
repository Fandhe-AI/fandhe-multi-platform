//! 型サンプル「画面の向き」の Web 経路（親 #128、Issue #658、対応表
//! `docs/device/capabilities/screen-orientation.json`）。
//!
//! `fandhe_core::screens::screen_orientation_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。`take_request` で要求を観測し、
//! Screen Orientation API（`screen.orientation`）と Fullscreen API へ渡して `finish` /
//! `apply_read` で結果を返す。`fandhe-egui-proxy` の driver（非 wasm32 専用）は通らず、
//! 本モジュールが同じ契約（`fandhe_device::screen_orientation` の doc）を Web 向けに満たす。
//! 整数コードの検証は `fandhe_device::screen_orientation::parse_reading` を共有する。
//!
//! # 方式
//!
//! - 取得は `screen.orientation.type` / `angle`。更新は push 型で、`screen.orientation` の
//!   `change`、document の `fullscreenchange`（Esc 解除もここに届く）、window の `pageshow`
//!   （bfcache 復帰）で読み直す。pull のポーリングは使わない
//! - 固定は利用者のクリックから effect まで同期に連なる中で `documentElement.requestFullscreen()`
//!   を呼び（`spawn_local` の後だと transient activation を失う）、全画面に入ってから
//!   `screen.orientation.lock("portrait" | "landscape")` を呼ぶ。W3C の pre-lock condition で
//!   `lock()` は全画面の文書でしか効かない
//! - Web には現在の lock を読む API が無い。`lock()` が resolve したときだけ document 単位の記録
//!   （`LOCK`）を立て、読み戻しの `lock` は「記録がある、かつ全画面である」ときだけ `Some`
//!   にする。全画面を抜けるとブラウザが固定を解くため、古い記録を返し続けない
//! - 成功は読み戻し（実際の向きと記録の lock）で確定する。期限は `CONFIRM_TIMEOUT`、起点は
//!   呼び出し開始で、単調時計（`performance.now()`）で測る。期限後に届いた結果は成功にしない
//! - 失敗・期限切れ・破棄の後は `lock()` の希望を残さない。直前が未固定なら `unlock()` とこの画面が
//!   入れた全画面の解除、直前が固定済みなら直前の向きへ `lock()` し直す
//! - 固定済みから別の向きへの固定が失敗したときの復元 `lock()` は期限付きで待ち、確定してから
//!   結果を届ける（その間は処理中のまま）。通知経路でも期限を確認し、遅れて成立した全画面は抜ける
//! - 全画面を抜ける `exitFullscreen()` は完了まで処理中を保つ（終了中の全画面へ次の固定が相乗りしない）。
//!   期限後に成立した復元 `lock()` も最後まで監視し、その時点の記録へ揃える
//! - 解除は `unlock()` を同期に呼び、記録を消し、この画面が入れた全画面だけを抜ける
//!   （他者が入れた全画面は抜けない）。全画面を抜けられなくても lock は解けているので成功とする
//! - `lock()` の `NotSupportedError`（Safari・デスクトップ Chrome）は固定だけ `Unsupported`
//!   （取得は動く）。それ以外の拒否は固定文言の `Failed`。例外の `message` は転送しない
//!   （security.md A09）
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（世代照合）、固定できていない状態を固定中と見せないこと
//! （全画面でない間は lock を `None` とする）、失敗・期限切れ・破棄の後に `lock()` の希望を残さない
//! こと、この画面が入れた全画面だけを抜けること、タイマーとリスナーを漏らさないこと。
//! 守らない: 複数タブ・複数マウント間の表示の食い違い、他のスクリプトや利用者が入れた全画面・固定の
//! 検知（lock を読む API が無い）、解除時に全画面を抜けられなかった場合の全画面の残り、
//! デスクトップ Chrome で固定を押すと一瞬全画面になること、iframe の権限ポリシーによる
//! `Unsupported`、ページ終了時の後片付け（ブラウザが行う）、Safari・Firefox の差
//! （Firefox は 144+）。いずれも表示が一時的に食い違うだけで、データ消失・資源漏れ・嘘の成功には
//! 至らない。

use std::time::Duration;

use fandhe_core::screens::screen_orientation_show::{Orientation, OrientationFailure};

/// 固定が拒否されたときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const LOCK_REJECTED: &str = "ブラウザが向きの固定を拒否しました";
/// 全画面への切り替えが拒否されたときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FULLSCREEN_REJECTED: &str = "ブラウザが全画面への切り替えを拒否しました";
/// 解除に失敗したときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNLOCK_FAILED: &str = "向きの固定を解除できませんでした";
/// 向きを読み取れなかったときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "画面の向きを読み取れませんでした";

/// `screen.orientation.type` を整数コード（1 = 縦・2 = 横・0 = 不明）へ写す。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn orientation_code(kind: Option<&str>) -> i32 {
    match kind {
        Some(k) if k.starts_with("portrait-") => 1,
        Some(k) if k.starts_with("landscape-") => 2,
        _ => 0,
    }
}

/// 読み戻しの lock コード（0 = なし・1 = 縦・2 = 横）。全画面でない・読めない間は 0 にする
/// （ブラウザは全画面を抜けると固定を解くため、古い記録を返し続けない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn lock_code(recorded: Option<Orientation>, fullscreen: Option<bool>) -> i32 {
    match (recorded, fullscreen) {
        (Some(Orientation::Portrait), Some(true)) => 1,
        (Some(Orientation::Landscape), Some(true)) => 2,
        _ => 0,
    }
}

/// 遅れて成立した全画面を、継続側が解除してよいか。マウントをまたいで後続の全画面要求
/// （`current_epoch` が進んでいる）があれば所有権は移っているので解除しない（契約表 2・9）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn fullscreen_release_allowed(my_epoch: u64, current_epoch: u64) -> bool {
    my_epoch == current_epoch
}

/// 固定できる環境か。`lock` 関数・`fullscreenEnabled`・`requestFullscreen` がすべて要る。
/// 実行時の `NotSupportedError` は core の `downgrade_lock` が拾う。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decide_lock_support(
    has_lock_fn: bool,
    fullscreen_enabled: bool,
    has_request_fullscreen: bool,
) -> bool {
    has_lock_fn && fullscreen_enabled && has_request_fullscreen
}

/// `lock()` の例外名を失敗の種別へ写す。`NotSupportedError` だけが `Unsupported`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: &str) -> OrientationFailure {
    if name == "NotSupportedError" {
        OrientationFailure::Unsupported
    } else {
        OrientationFailure::Failed(LOCK_REJECTED.to_string())
    }
}

/// 復元 `lock()` の reject を「真の失敗」として記録破棄・`unlock()` するか。
/// `AbortError` は後続の `lock()` / `unlock()` に置き換えられた印で、より新しい固定や
/// 有効な記録を壊さないよう後始末しない（契約表 2: 古い世代の完了を捨てる）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn restore_rejection_is_failure(name: &str) -> bool {
    name != "AbortError"
}

/// 期限切れか。非有限値・負の経過は期限切れ扱い（成功にしない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn expired(started_ms: f64, now_ms: f64, timeout: Duration) -> bool {
    let elapsed = now_ms - started_ms;
    !elapsed.is_finite() || elapsed < 0.0 || elapsed >= timeout.as_secs_f64() * 1000.0
}

/// `lock()` へ渡す文字列（固定値のみ。外部入力は混ぜない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn lock_type_str(o: Orientation) -> &'static str {
    match o {
        Orientation::Portrait => "portrait",
        Orientation::Landscape => "landscape",
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::screen_orientation_show_effect;

/// マウント破棄時にリスナー・タイマー・固定を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use fandhe_core::screens::screen_orientation_show::{
        apply_read, finish, lock_satisfied, mark_applying, take_request, LockRequest, Orientation,
        OrientationFailure, ReadState, Reading, ScreenOrientationShowState, NOT_LOCKED_REASON,
    };
    use fandhe_device::screen_orientation::{
        parse_reading, Orientation as DeviceOrientation, CONFIRM_TIMEOUT,
    };
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    use super::{
        decide_lock_support, expired, failure_from_error_name, fullscreen_release_allowed,
        lock_code, lock_type_str, orientation_code, restore_rejection_is_failure,
        FULLSCREEN_REJECTED, READ_FAILED, UNLOCK_FAILED,
    };
    use crate::mount::{is_live, Applier};

    type State = ScreenOrientationShowState;

    /// 固定の確認待ち 1 件。タイマーは期限の予約で、結果を確定するときに止める。
    struct Pending {
        generation: u64,
        target: Orientation,
        /// 依頼前の固定。失敗時に戻す先。
        before: Option<Orientation>,
        started_ms: f64,
        timer: (i32, Closure<dyn FnMut()>),
    }

    #[derive(Default)]
    struct Entry {
        /// 発送済みの世代（同じ世代を二度渡さない）。
        started: Option<u64>,
        pending: Option<Pending>,
        /// この画面が入れた全画面か（解除・失敗時にこれだけを抜ける）。
        own_fullscreen: bool,
        /// 失敗後の復元 `lock()` を待っている世代。解除・破棄で `None` にして無効化する。
        restoring: Option<u64>,
        /// `change`・`fullscreenchange`・`pageshow` で共有する読み直し。
        on_change: Option<Closure<dyn FnMut()>>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
        /// `lock()` が resolve した向きと、記録したマウント。読み戻しは全画面のときだけ使う。
        static LOCK: Cell<Option<(u64, Orientation)>> = const { Cell::new(None) };
        /// マウントをまたぐ全画面要求の世代。`requestFullscreen()` を呼ぶたびに進め、
        /// 遅延した完了が後続の要求の全画面を解除しないよう照合する。
        static FS_EPOCH: Cell<u64> = const { Cell::new(0) };
    }

    /// 既存の Entry にだけ作用する。破棄済みマウントでは `None`（再生成して漏らさない）。
    fn with<R>(id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        ENTRIES.with(|e| e.borrow_mut().get_mut(&id).map(f))
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn now_ms() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(f64::NAN, |p| p.now())
    }

    fn document() -> Option<web_sys::Document> {
        web_sys::window()?.document()
    }

    /// `screen.orientation`。無ければ `None`。
    fn orientation_obj() -> Option<JsValue> {
        let win = web_sys::window()?;
        let screen = get(&win, "screen");
        let o = get(&screen, "orientation");
        o.is_object().then_some(o)
    }

    /// `null` = 通常、object = 全画面、それ以外 = 読めない。
    fn fullscreen() -> Option<bool> {
        let doc = document()?;
        let v = get(&doc, "fullscreenElement");
        if v.is_null() {
            Some(false)
        } else if v.is_object() {
            Some(true)
        } else {
            None
        }
    }

    fn error_name(e: &JsValue) -> String {
        get(e, "name").as_string().unwrap_or_default()
    }

    fn recorded() -> Option<Orientation> {
        LOCK.with(|l| l.get().map(|(_, o)| o))
    }

    fn can_lock_now() -> bool {
        let has_lock = orientation_obj().is_some_and(|o| get(&o, "lock").is_function());
        let doc = document();
        let enabled = doc
            .as_ref()
            .is_some_and(|d| get(d, "fullscreenEnabled").as_bool() == Some(true));
        let has_req = doc
            .and_then(|d| d.document_element())
            .is_some_and(|r| get(&r, "requestFullscreen").is_function());
        decide_lock_support(has_lock, enabled, has_req)
    }

    /// 現在の読み戻し。`screen.orientation` が無ければ `Unsupported`。
    fn read_now() -> Result<Reading, OrientationFailure> {
        let Some(o) = orientation_obj() else {
            return Err(OrientationFailure::Unsupported);
        };
        let kind = get(&o, "type").as_string();
        let angle = get(&o, "angle")
            .as_f64()
            .filter(|a| a.is_finite() && a.fract() == 0.0 && a.abs() < 1.0e6)
            .map_or(-1, |a| a as i32);
        let lock = lock_code(recorded(), fullscreen());
        let r = parse_reading(orientation_code(kind.as_deref()), angle, lock)
            .map_err(|_| OrientationFailure::Failed(READ_FAILED.to_string()))?;
        let to_core = |o: DeviceOrientation| match o {
            DeviceOrientation::Portrait => Orientation::Portrait,
            DeviceOrientation::Landscape => Orientation::Landscape,
        };
        Ok(Reading {
            orientation: to_core(r.orientation),
            angle: r.rotation.degrees(),
            lock: r.lock.map(to_core),
            can_lock: can_lock_now(),
        })
    }

    fn call0(obj: &JsValue, method: &str) -> Result<JsValue, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        f.call0(obj)
    }

    /// `screen.orientation.lock(type)` を呼ぶ。同期の例外は `Err`。
    fn lock_js(t: Orientation) -> Result<Promise, JsValue> {
        let o = orientation_obj().ok_or(JsValue::UNDEFINED)?;
        let f: Function = get(&o, "lock").dyn_into()?;
        f.call1(&o, &JsValue::from_str(lock_type_str(t)))?
            .dyn_into::<Promise>()
    }

    /// `screen.orientation.unlock()`（同期、戻り値なし）。
    fn unlock_js() -> Result<(), JsValue> {
        let o = orientation_obj().ok_or(JsValue::UNDEFINED)?;
        call0(&o, "unlock").map(|_| ())
    }

    /// 戻り値の Promise を待って捨てる（未処理の reject を残さない）。
    fn swallow(ret: JsValue) {
        if let Ok(p) = ret.dyn_into::<Promise>() {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = JsFuture::from(p).await;
            });
        }
    }

    /// `exitFullscreen()` を呼び、完了を待てる Promise を返す。全画面でなければ `None`。
    fn exit_fullscreen_promise() -> Option<Promise> {
        if fullscreen() != Some(true) {
            return None;
        }
        let d = document()?;
        call0(&d, "exitFullscreen").ok()?.dyn_into::<Promise>().ok()
    }

    /// 完了を待たない全画面解除（破棄・遅れて成立した全画面の始末）。
    fn exit_fullscreen_now() {
        if let Some(p) = exit_fullscreen_promise() {
            swallow(p.into());
        }
    }

    /// この画面が入れた全画面だけを抜ける。完了待ち用の Promise を返す
    /// （終了中の全画面へ次の固定が相乗りしないよう、呼び出し側は完了まで処理中を保つ）。
    fn exit_own_fullscreen(id: u64) -> Option<Promise> {
        if with(id, |e| std::mem::take(&mut e.own_fullscreen)).unwrap_or(false) {
            exit_fullscreen_promise()
        } else {
            None
        }
    }

    /// 全画面終了（あれば）の完了を待ってから結果を届ける。待つ間も core は処理中のまま。
    fn deliver(
        applier: &Applier<State>,
        id: u64,
        gen: u64,
        res: Result<(), OrientationFailure>,
        exit: Option<Promise>,
    ) {
        let Some(p) = exit else {
            applier.apply(move |s| finish(s, gen, res, read_now()));
            return;
        };
        let a = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = JsFuture::from(p).await;
            if is_live(id) {
                a.apply(move |s| finish(s, gen, res, read_now()));
            }
        });
    }

    /// 現在の記録どおりに OS の希望を揃える（期限後・破棄後に遅れて resolve した `lock()` の始末）。
    /// 復元 `lock()` が保留・失敗しても記録と実際の固定がずれないよう、完了後に記録を照合し直す
    /// （保留中に解除・再固定が走っていれば、その時点の記録へ揃え直す。契約表 2）。
    fn restore_to_record() {
        match LOCK.with(|l| l.get()) {
            None => {
                let _ = unlock_js();
            }
            Some((_, p)) => match lock_js(p) {
                Ok(pr) => {
                    wasm_bindgen_futures::spawn_local(async move {
                        let res = JsFuture::from(pr).await;
                        let ok = res.is_ok();
                        let now = LOCK.with(|l| l.get()).map(|(_, o)| o);
                        if let Err(e) = &res {
                            // 後続の lock/unlock に中断されただけなら、新しい固定と記録を守る。
                            if !restore_rejection_is_failure(&error_name(e)) {
                                return;
                            }
                        }
                        if ok {
                            // 待つ間に記録が変わっていたら、今の記録へ揃え直す。
                            if now != Some(p) {
                                restore_to_record();
                            }
                        } else if now == Some(p) {
                            // 復元に失敗した。記録が固定を主張したままにしない。
                            LOCK.with(|l| l.set(None));
                            let _ = unlock_js();
                        }
                    });
                }
                Err(_) => {
                    if recorded() == Some(p) {
                        LOCK.with(|l| l.set(None));
                        let _ = unlock_js();
                    }
                }
            },
        }
    }

    /// 復元 `lock()` を待つ期限付き Promise。期限側は文字列 "timeout" で resolve する。
    fn race_with_timeout(p: Promise) -> Promise {
        let timeout = Promise::new(&mut |resolve, _| {
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_1(
                    &resolve,
                    CONFIRM_TIMEOUT.as_millis() as i32,
                    &JsValue::from_str("timeout"),
                );
            }
        });
        Promise::race(&js_sys::Array::of2(&p, &timeout))
    }

    /// 直前が未固定だった失敗の巻き戻し（同期）。
    fn rollback_unlocked(id: u64) -> Option<Promise> {
        let _ = unlock_js();
        LOCK.with(|l| l.set(None));
        exit_own_fullscreen(id)
    }

    /// 直前が固定済みだった失敗の巻き戻し。復元 `lock()` を期限付きで待ち、確定してから
    /// 結果を届ける（復元が保留のまま次の要求を受け付けない。契約表 2・3・9）。
    /// 待つ間に解除・破棄で世代（`restoring`）が変わったら、遅れて resolve した復元は
    /// 現在の記録へ揃え直す。
    fn rollback_locked(
        applier: &Applier<State>,
        id: u64,
        gen: u64,
        b: Orientation,
        res: Result<(), OrientationFailure>,
    ) {
        LOCK.with(|l| l.set(None));
        with(id, |e| e.restoring = Some(gen));
        let a = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let ok = match lock_js(b) {
                Ok(pr) => match JsFuture::from(race_with_timeout(pr.clone())).await {
                    Ok(v) => {
                        let timed_out = v.as_string().as_deref() == Some("timeout");
                        if timed_out {
                            // 期限後も元の復元 Promise を最後まで監視し、成立したら
                            // その時点の記録へ揃える（解除・再固定を取りこぼさない）。
                            wasm_bindgen_futures::spawn_local(async move {
                                if JsFuture::from(pr).await.is_ok() {
                                    restore_to_record();
                                }
                            });
                        }
                        !timed_out
                    }
                    Err(_) => false,
                },
                Err(_) => false,
            };
            let current = is_live(id) && with(id, |e| e.restoring == Some(gen)).unwrap_or(false);
            if !current {
                // 解除・破棄が先に走った。遅れて効いた復元を現在の記録へ揃える。
                if ok {
                    restore_to_record();
                }
                return;
            }
            with(id, |e| e.restoring = None);
            if ok {
                LOCK.with(|l| {
                    if l.get().is_none() {
                        l.set(Some((id, b)));
                    }
                });
                a.apply(move |s| finish(s, gen, res, read_now()));
            } else {
                let exit = rollback_unlocked(id);
                deliver(&a, id, gen, res, exit);
            }
        });
    }

    /// 保留中の要求を確定する。タイマーを止め、失敗なら希望を戻してから結果を届ける
    /// （状態確定 → 通知の順、契約表 2・9）。
    fn complete(applier: &Applier<State>, id: u64, result: Result<(), OrientationFailure>) {
        let Some(Some(p)) = with(id, |e| e.pending.take()) else {
            return;
        };
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(p.timer.0);
        }
        let gen = p.generation;
        if result.is_err() {
            match p.before {
                None => {
                    let exit = rollback_unlocked(id);
                    return deliver(applier, id, gen, result, exit);
                }
                Some(b) => return rollback_locked(applier, id, gen, b, result),
            }
        }
        applier.apply(move |s| finish(s, gen, result, read_now()));
    }

    /// 要求が現在のもので、期限前か（`spawn_local` 再開後の再確認、契約表 2・3）。
    fn current(id: u64, gen: u64) -> bool {
        is_live(id)
            && with(id, |e| {
                e.pending.as_ref().is_some_and(|p| {
                    p.generation == gen && !expired(p.started_ms, now_ms(), CONFIRM_TIMEOUT)
                })
            })
            .unwrap_or(false)
    }

    /// 通知（`change` など）で読み直す。全画面を抜けたと分かったら記録を消し、保留中の固定が
    /// 満たされていれば確定する。
    fn on_notified(applier: &Applier<State>, id: u64) {
        // 通知経路でも期限を確認する（タイマーより先に届いた期限後の change を成功にしない）。
        let late = with(id, |e| {
            e.pending
                .as_ref()
                .is_some_and(|p| expired(p.started_ms, now_ms(), CONFIRM_TIMEOUT))
        })
        .unwrap_or(false);
        if late {
            complete(
                applier,
                id,
                Err(OrientationFailure::Failed(NOT_LOCKED_REASON.to_string())),
            );
            return;
        }
        if fullscreen() == Some(false) {
            LOCK.with(|l| l.set(None));
            with(id, |e| {
                if e.pending.is_none() {
                    e.own_fullscreen = false;
                }
            });
        }
        let reading = read_now();
        let target = with(id, |e| e.pending.as_ref().map(|p| p.target)).flatten();
        let done = match (&reading, target) {
            (Ok(r), Some(t)) => lock_satisfied(Some(t), r),
            _ => false,
        };
        if done {
            complete(applier, id, Ok(()));
        } else {
            applier.apply(move |s| apply_read(s, reading));
        }
    }

    fn orientation_target() -> Option<web_sys::EventTarget> {
        orientation_obj()?.dyn_into::<web_sys::EventTarget>().ok()
    }

    /// マウントごとに 1 度だけ購読を登録し、登録直後に 1 回読む（登録と保存の間の通知を落とさない）。
    fn install(applier: &Applier<State>, id: u64) {
        let fresh = ENTRIES.with(|e| match e.borrow_mut().entry(id) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Entry::default());
                true
            }
            _ => false,
        });
        if !fresh {
            return;
        }
        let a = applier.clone();
        let on_change = Closure::<dyn FnMut()>::new(move || on_notified(&a, id));
        let f: &Function = on_change.as_ref().unchecked_ref();
        if let Some(w) = web_sys::window() {
            if let Some(d) = w.document() {
                let _ = d.add_event_listener_with_callback("fullscreenchange", f);
            }
            let _ = w.add_event_listener_with_callback("pageshow", f);
        }
        if let Some(t) = orientation_target() {
            let _ = t.add_event_listener_with_callback("change", f);
        }
        with(id, |e| e.on_change = Some(on_change));
        applier.apply(|s| apply_read(s, read_now()));
    }

    /// 解除。`unlock()` を同期に呼び、記録を消し、この画面が入れた全画面だけを抜ける。
    fn start_unlock(applier: &Applier<State>, id: u64, gen: u64) {
        let ok = unlock_js().is_ok();
        LOCK.with(|l| l.set(None));
        with(id, |e| e.restoring = None);
        let exit = exit_own_fullscreen(id);
        let res = if ok {
            Ok(())
        } else {
            Err(OrientationFailure::Failed(UNLOCK_FAILED.to_string()))
        };
        // 全画面終了の完了まで処理中を保ち、次の固定が終了中の全画面に相乗りしないようにする。
        applier.apply(move |s| mark_applying(s, gen));
        deliver(applier, id, gen, res, exit);
    }

    fn finish_now(applier: &Applier<State>, gen: u64, res: Result<(), OrientationFailure>) {
        applier.apply(move |s| {
            mark_applying(s, gen);
            finish(s, gen, res, read_now());
        });
    }

    fn start_lock(applier: &Applier<State>, id: u64, gen: u64, target: Orientation) {
        // 依頼前に読む。読めなければ API を呼ばない。
        let before = match read_now() {
            Ok(r) => r,
            Err(e) => return finish_now(applier, gen, Err(e)),
        };
        if lock_satisfied(Some(target), &before) {
            return finish_now(applier, gen, Ok(()));
        }
        if !before.can_lock {
            return finish_now(applier, gen, Err(OrientationFailure::Unsupported));
        }
        let not_locked = || OrientationFailure::Failed(NOT_LOCKED_REASON.to_string());
        // 期限は呼び出し開始が起点。先に予約し、以降の全経路を complete で一度だけ確定する。
        let a = applier.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            complete(
                &a,
                id,
                Err(OrientationFailure::Failed(NOT_LOCKED_REASON.to_string())),
            )
        });
        let started_ms = now_ms();
        let handle = web_sys::window().and_then(|w| {
            w.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                CONFIRM_TIMEOUT.as_millis() as i32,
            )
            .ok()
        });
        let Some(handle) = handle else {
            return finish_now(applier, gen, Err(not_locked()));
        };
        with(id, |e| {
            e.pending = Some(Pending {
                generation: gen,
                target,
                before: before.lock,
                started_ms,
                timer: (handle, cb),
            })
        });
        applier.apply(move |s| mark_applying(s, gen));
        // transient activation を保つため、全画面への切り替えはここ（同期）で呼ぶ。
        let mut fs_promise = None;
        let mut fs_epoch = 0;
        if fullscreen() != Some(true) {
            let root = document().and_then(|d| d.document_element());
            let ret = match root {
                Some(r) => call0(&r, "requestFullscreen"),
                None => Err(JsValue::UNDEFINED),
            };
            match ret {
                Ok(r) => {
                    with(id, |e| e.own_fullscreen = true);
                    fs_epoch = FS_EPOCH.with(|c| {
                        c.set(c.get() + 1);
                        c.get()
                    });
                    fs_promise = r.dyn_into::<Promise>().ok();
                }
                Err(_) => {
                    complete(
                        applier,
                        id,
                        Err(OrientationFailure::Failed(FULLSCREEN_REJECTED.to_string())),
                    );
                    return;
                }
            }
        }
        let a = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(p) = fs_promise {
                let ok = JsFuture::from(p).await.is_ok();
                if !current(id, gen) {
                    if ok {
                        // 遅れて成立した全画面は、この要求が入れたもの。後続の要求が無ければ抜ける。
                        let live = is_live(id);
                        let late_same = live
                            && with(id, |e| {
                                e.pending.as_ref().is_some_and(|p| p.generation == gen)
                            })
                            .unwrap_or(false);
                        if late_same {
                            complete(
                                &a,
                                id,
                                Err(OrientationFailure::Failed(NOT_LOCKED_REASON.to_string())),
                            );
                        } else {
                            let owned =
                                fullscreen_release_allowed(fs_epoch, FS_EPOCH.with(Cell::get));
                            let free = owned
                                && (!live
                                    || (recorded().is_none()
                                        && with(id, |e| {
                                            e.pending.is_none() && e.restoring.is_none()
                                        })
                                        .unwrap_or(true)));
                            if free {
                                exit_fullscreen_now();
                            } else {
                                // 後続の固定がこの全画面に依存している。落とさず、以後の解除で抜ける。
                                with(id, |e| e.own_fullscreen = true);
                            }
                        }
                    }
                    return;
                }
                if !ok {
                    complete(
                        &a,
                        id,
                        Err(OrientationFailure::Failed(FULLSCREEN_REJECTED.to_string())),
                    );
                    return;
                }
            }
            // 既に全画面の経路でも、破棄・世代交代・期限後に lock() を呼ばない（契約表 2・3）。
            if !current(id, gen) {
                return;
            }
            let pr = match lock_js(target) {
                Ok(pr) => pr,
                Err(e) => {
                    complete(&a, id, Err(failure_from_error_name(&error_name(&e))));
                    return;
                }
            };
            let res = JsFuture::from(pr).await;
            if !current(id, gen) {
                // 期限後・破棄後に resolve した lock を放置せず、現在の記録へ揃える。
                if res.is_ok() {
                    restore_to_record();
                }
                return;
            }
            match res {
                Ok(_) => {
                    LOCK.with(|l| l.set(Some((id, target))));
                    // 未到達なら change か期限を待つ。
                    on_notified(&a, id);
                }
                Err(e) => complete(&a, id, Err(failure_from_error_name(&error_name(&e)))),
            }
        });
    }

    fn start(applier: &Applier<State>, id: u64, req: LockRequest) {
        let gen = req.generation;
        let fresh = with(id, |e| {
            let fresh = e.started != Some(gen);
            e.started = Some(gen);
            fresh
        })
        .unwrap_or(false);
        if !fresh {
            return;
        }
        match req.target {
            Some(t) => start_lock(applier, id, gen, t),
            None => start_unlock(applier, id, gen),
        }
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ読み取り前のときだけ 1 回読む
    /// （失敗・非対応では再読込しない、契約表 4）。
    pub fn screen_orientation_show_effect(state: &State, applier: &Applier<State>) {
        let id = applier.mount_id();
        install(applier, id);
        if let Some(req) = take_request(state) {
            start(applier, id, req);
            return;
        }
        if matches!(state.read_state(), ReadState::Unread) && !state.busy() {
            applier.apply(|s| apply_read(s, read_now()));
        }
    }

    pub(crate) fn release_mount(mount_id: u64) {
        let Some(mut entry) = ENTRIES.with(|e| e.borrow_mut().remove(&mount_id)) else {
            return;
        };
        let win = web_sys::window();
        let had_pending = entry.pending.is_some() || entry.restoring.is_some();
        if let (Some(p), Some(w)) = (entry.pending.take(), &win) {
            w.clear_timeout_with_handle(p.timer.0);
        }
        if let Some(cb) = entry.on_change.take() {
            let f: &Function = cb.as_ref().unchecked_ref();
            if let Some(w) = &win {
                if let Some(d) = w.document() {
                    let _ = d.remove_event_listener_with_callback("fullscreenchange", f);
                }
                let _ = w.remove_event_listener_with_callback("pageshow", f);
            }
            if let Some(t) = orientation_target() {
                let _ = t.remove_event_listener_with_callback("change", f);
            }
        }
        // このマウントの固定（進行中を含む）と、この画面が入れた全画面だけを解く。ベストエフォート。
        let owns = LOCK.with(|l| l.get().is_some_and(|(o, _)| o == mount_id));
        if owns || had_pending {
            let _ = unlock_js();
            if owns {
                LOCK.with(|l| l.set(None));
            }
        }
        if entry.own_fullscreen {
            exit_fullscreen_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_codes() {
        assert_eq!(orientation_code(Some("portrait-primary")), 1);
        assert_eq!(orientation_code(Some("portrait-secondary")), 1);
        assert_eq!(orientation_code(Some("landscape-primary")), 2);
        assert_eq!(orientation_code(Some("landscape-secondary")), 2);
        assert_eq!(orientation_code(Some("diagonal")), 0);
        assert_eq!(orientation_code(Some("portrait")), 0);
        assert_eq!(orientation_code(None), 0);
    }

    #[test]
    fn lock_is_none_outside_fullscreen() {
        let p = Some(Orientation::Portrait);
        assert_eq!(lock_code(p, Some(true)), 1);
        assert_eq!(lock_code(Some(Orientation::Landscape), Some(true)), 2);
        assert_eq!(lock_code(p, Some(false)), 0);
        assert_eq!(lock_code(p, None), 0);
        assert_eq!(lock_code(None, Some(true)), 0);
    }

    #[test]
    fn lock_support_needs_all_three() {
        assert!(decide_lock_support(true, true, true));
        assert!(!decide_lock_support(false, true, true));
        assert!(!decide_lock_support(true, false, true));
        assert!(!decide_lock_support(true, true, false));
    }

    #[test]
    fn only_not_supported_is_unsupported() {
        assert_eq!(
            failure_from_error_name("NotSupportedError"),
            OrientationFailure::Unsupported
        );
        for n in ["SecurityError", "AbortError", "TypeError", ""] {
            assert_eq!(
                failure_from_error_name(n),
                OrientationFailure::Failed(LOCK_REJECTED.to_string())
            );
        }
    }

    #[test]
    fn restore_abort_is_not_failure() {
        assert!(!restore_rejection_is_failure("AbortError"));
        for n in ["NotSupportedError", "SecurityError", "TypeError", ""] {
            assert!(restore_rejection_is_failure(n));
        }
    }

    #[test]
    fn expiry_is_conservative() {
        let t = Duration::from_secs(3);
        assert!(!expired(100.0, 2_999.0 + 100.0, t));
        assert!(expired(100.0, 3_100.0, t));
        assert!(expired(100.0, 50.0, t));
        assert!(expired(f64::NAN, 10.0, t));
        assert!(expired(0.0, f64::INFINITY, t));
    }

    #[test]
    fn fullscreen_release_follows_epoch() {
        assert!(fullscreen_release_allowed(3, 3));
        assert!(!fullscreen_release_allowed(3, 4));
    }

    #[test]
    fn lock_type_strings() {
        assert_eq!(lock_type_str(Orientation::Portrait), "portrait");
        assert_eq!(lock_type_str(Orientation::Landscape), "landscape");
    }
}
