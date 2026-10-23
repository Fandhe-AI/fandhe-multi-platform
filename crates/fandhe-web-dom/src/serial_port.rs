//! 型サンプル「シリアルポート」の Web 経路（親 #131、Issue #672）。
//!
//! `fandhe_core::screens::serial_port_show`（端末依存ゼロ、`PortMode::BrowserPicker`）の
//! `RequestKind::WebPick` を [`super::mount::mount_with_effect`] の effect として観測し、
//! Web Serial API（`navigator.serial`）で「利用者がダイアログで 1 つ選ぶ → 固定設定で開く →
//! `fandhe\n` を送る → LF までの 1 行を受ける → 毎回閉じる」を行って画面へ書き戻す。
//! 呼び出し元は `demo.rs` の `mount_serial_port_show_screen`、破棄時の解放は
//! `mount::sweep_mounts` から [`release_mount`]。契約の SSOT は `fandhe_device::serial_port`
//! （定数・[`LineAssembler`]・失敗種別）。Web は `SerialPorts` トレイトと `round_trip` を通らない
//! （ブロッキング契約と合わないため）ので、本モジュールが DOM レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約
//!
//! - **user activation 必須**: `requestPort()` はボタン `click` の同期チェーン内で呼び、
//!   その前に `await` を挟まない。
//! - **secure context・Chromium 系（89 以降）と Firefox 151 以降のみ**: `navigator.serial` が無い、
//!   または secure context 外は `Unsupported`。Permissions Policy の `serial` で禁止されると
//!   `SecurityError`（権限拒否として見せる）。
//! - **web-sys の unstable API を避ける**: `Reflect` / `Function` / `Promise` / `Uint8Array` だけで呼ぶ。
//!
//! # 期限
//!
//! 起点は `requestPort()` の resolve 直後（選択ダイアログの期限は守らない範囲）。
//! 単調時計（`performance.now()`）で [`ROUND_TRIP_TIMEOUT`] を測り、open〜受信の全体にかける。
//! `open()` は中断できないため、期限を過ぎても完了を待ってから判定する。期限後に完成した行は
//! 捨てて `Timeout` にする。タイマー発火時は reader の `cancel` と writer の `abort` で保留中の
//! 読み書きを解く。
//!
//! # 三値
//!
//! - 成功: 1 行（空行を含む）。
//! - キャンセル: `requestPort` の `NotFoundError`（ダイアログを閉じた）だけ。
//! - 失敗: それ以外。0 バイト・ストリーム終端・期限切れを成功に畳まない。
//!
//! # 後片付け
//!
//! 全経路で `clearTimeout` → reader の `cancel` と `releaseLock` → writer の `releaseLock` →
//! `port.close()` を済ませてから結果を apply する。マウント破棄（[`release_mount`]）でも同じ
//! 後片付けを通し、結果は apply しない。
//!
//! # 取らないもの・守る範囲
//!
//! `getInfo()`・`getPorts()`・connect / disconnect イベント・例外の `message`・受信バイトは読まない／
//! `console` へ出さない（security.md A09）。守る: 1 マウント内の要求と完了の順序（世代照合と二重
//! dispatch の禁止）・失敗を成功に見せない・期限後の行を成功にしない・全経路の close。守らない:
//! 選択ダイアログの期限・許可済みポートの列挙・抜き差しイベント・Chromium 系と Firefox 以外・
//! 改行で返さない機器（期限切れ）・行の文字コード・複数タブ間の一致・中断できない `open()` が
//! 期限を大きく超えたときの表示確定の遅れ。
//!
//! # 推測（#667 の実ブラウザ確認で確かめる）
//!
//! 他アプリ使用中の `open()` が `NetworkError` になるか、open 時にブラウザが OS の受信バッファを
//! 捨てるか（Web Serial に破棄 API は無い）、bfcache・離脱時にブラウザがポートを閉じるか。

use fandhe_core::screens::serial_port_show::PortFailure;
use fandhe_device::serial_port::{LineAssembler, SerialPortFailure, BAUD_RATE, ROUND_TRIP_TIMEOUT};

/// 失敗時の固定文言（egui-proxy 側と同じ）。例外の `message` は使わない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FAILED_TEXT: &str = "シリアルポートの操作";

/// 期限（ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const TIMEOUT_MS: f64 = ROUND_TRIP_TIMEOUT.as_millis() as f64;

/// `port.open()` に渡す固定設定（キー, 値）。数値は `Num`、文字列は `Str`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OptionValue {
    Num(f64),
    Str(&'static str),
}

/// 9600 bps・8N1・フロー制御なし。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn open_options() -> [(&'static str, OptionValue); 5] {
    [
        ("baudRate", OptionValue::Num(f64::from(BAUD_RATE))),
        ("dataBits", OptionValue::Num(8.0)),
        ("stopBits", OptionValue::Num(1.0)),
        ("parity", OptionValue::Str("none")),
        ("flowControl", OptionValue::Str("none")),
    ]
}

/// `requestPort()` の例外名を失敗種別へ。`NotFoundError` だけがキャンセル。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_request_error(name: Option<&str>) -> SerialPortFailure {
    match name {
        Some("NotFoundError") => SerialPortFailure::Cancelled,
        Some("SecurityError") => SerialPortFailure::PermissionDenied,
        _ => SerialPortFailure::Internal,
    }
}

/// `open()` の例外名を失敗種別へ。
///
/// `NetworkError`（開けない）を `Busy` に寄せるのは文書からの推測で、他アプリ使用中の実挙動は
/// #667 で確かめる。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_open_error(name: Option<&str>) -> SerialPortFailure {
    match name {
        Some("InvalidStateError") | Some("NetworkError") => SerialPortFailure::Busy,
        Some("SecurityError") => SerialPortFailure::PermissionDenied,
        Some("NotFoundError") => SerialPortFailure::Gone,
        _ => SerialPortFailure::Internal,
    }
}

/// 読み書き途中の例外名を失敗種別へ。`NetworkError` は抜去。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_io_error(name: Option<&str>) -> SerialPortFailure {
    match name {
        Some("NetworkError") => SerialPortFailure::Gone,
        _ => SerialPortFailure::Internal,
    }
}

/// 契約の失敗種別を画面の失敗種別へ（egui-proxy の `to_failure` と同じ写像）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_port_failure(f: SerialPortFailure) -> PortFailure {
    match f {
        SerialPortFailure::Timeout => PortFailure::Timeout,
        SerialPortFailure::Busy => PortFailure::Busy,
        SerialPortFailure::Gone => PortFailure::Gone,
        SerialPortFailure::PermissionDenied => PortFailure::PermissionDenied,
        SerialPortFailure::Unsupported => PortFailure::Unsupported,
        SerialPortFailure::LineTooLong => PortFailure::LineTooLong,
        SerialPortFailure::Cancelled => PortFailure::Cancelled,
        SerialPortFailure::Internal => PortFailure::Failed(FAILED_TEXT.to_string()),
    }
}

/// 期限までの残り（ミリ秒）。尽きていれば `None`。時計が逆行したら 0 経過として扱う。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn remaining_ms(started_ms: f64, now_ms: f64) -> Option<f64> {
    let elapsed = (now_ms - started_ms).max(0.0);
    let left = TIMEOUT_MS - elapsed;
    (left > 0.0).then_some(left)
}

/// 期限を過ぎているか（ちょうど期限は過ぎた扱い）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    remaining_ms(started_ms, now_ms).is_none()
}

/// 受信チャンクを組み立て器へ渡す。行が完成したら `Some`（期限後なら `Timeout`）、未完成は `None`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn on_chunk(
    assembler: &mut LineAssembler,
    bytes: &[u8],
    started_ms: f64,
    now_ms: f64,
) -> Option<Result<Option<String>, SerialPortFailure>> {
    match assembler.push(bytes) {
        Err(f) => Some(Err(f)),
        Ok(None) => None,
        Ok(Some(line)) => Some(if is_late(started_ms, now_ms) {
            Err(SerialPortFailure::Timeout)
        } else {
            Ok(line.text)
        }),
    }
}

/// 行の完成前にストリームが終わった（`done: true`）ときの失敗。成功にはしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn on_stream_done() -> SerialPortFailure {
    SerialPortFailure::Gone
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};

    use fandhe_core::screens::serial_port_show::{
        finish_list, finish_round_trip, mark_running, take_request, PortFailure, RequestKind,
        SerialPortShowState,
    };
    use fandhe_device::serial_port::{LineAssembler, SerialPortFailure, SEND_LINE};
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{
        failure_from_io_error, failure_from_open_error, failure_from_request_error, is_late,
        on_chunk, on_stream_done, open_options, remaining_ms, to_port_failure, OptionValue,
    };
    use crate::mount::{is_live, Applier};

    /// 進行中の往復が持つ資源。後片付け（`cleanup`）が全て解放する。
    #[derive(Default)]
    struct Session {
        timed_out: bool,
        released: bool,
        reader: Option<JsValue>,
        writer: Option<JsValue>,
        timer: Option<(i32, Closure<dyn FnMut()>)>,
    }

    struct MountEntry {
        /// 処理済みの世代。同じ要求で `requestPort` を 2 回呼ばない（契約表 2）。
        dispatched: u64,
        session: Option<Rc<RefCell<Session>>>,
    }

    thread_local! {
        static MOUNTS: RefCell<HashMap<u64, MountEntry>> = RefCell::new(HashMap::new());
    }

    fn get(target: &JsValue, name: &str) -> Option<JsValue> {
        Reflect::get(target, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    fn error_name(err: &JsValue) -> Option<String> {
        get(err, "name").and_then(|v| v.as_string())
    }

    /// `target.name(...args)` を呼ぶ。関数でなければ名前なしの `Err`。
    fn call(target: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f = get(target, name)
            .and_then(|f| f.dyn_into::<Function>().ok())
            .ok_or_else(|| JsValue::from_str("not a function"))?;
        Reflect::apply(&f, target, &args.iter().collect::<Array>())
    }

    async fn call_await(
        target: &JsValue,
        name: &str,
        args: &[JsValue],
    ) -> Result<JsValue, JsValue> {
        let v = call(target, name, args)?;
        JsFuture::from(Promise::resolve(&v)).await
    }

    /// 単調時計（ミリ秒）。取得できなければ 0（呼び出し側は差分でしか使わない）。
    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0)
    }

    /// 保留中の読み書きを解く。reader は `cancel`（保留中の `read()` は `done: true`）、
    /// writer は `abort`。結果は見ない。ロックの解放は `cleanup` が行う。
    fn abort_io(s: &Session) {
        if let Some(r) = &s.reader {
            let _ = call(r, "cancel", &[]);
        }
        if let Some(w) = &s.writer {
            let _ = call(w, "abort", &[]);
        }
    }

    fn make_options() -> Result<Object, JsValue> {
        let o = Object::new();
        for (k, v) in open_options() {
            let v = match v {
                OptionValue::Num(n) => JsValue::from_f64(n),
                OptionValue::Str(s) => JsValue::from_str(s),
            };
            Reflect::set(&o, &JsValue::from_str(k), &v)?;
        }
        Ok(o)
    }

    /// `mount_with_effect` へ渡す effect。要求が無ければ何もしない。
    pub fn serial_port_show_effect(
        state: &SerialPortShowState,
        applier: &Applier<SerialPortShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        let generation = req.generation;
        let id = applier.mount_id();
        let fail = |applier: &Applier<SerialPortShowState>, kind: &RequestKind, f: PortFailure| {
            let is_list = matches!(kind, RequestKind::List);
            applier.apply(move |s| {
                if is_list {
                    finish_list(s, generation, Err(f));
                } else {
                    finish_round_trip(s, generation, Err(f));
                }
            });
        };

        // 二重 dispatch の禁止。同じ世代は 1 回しか処理しない。
        let already = MOUNTS.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.entry(id).or_insert(MountEntry {
                dispatched: 0,
                session: None,
            });
            if e.dispatched == generation {
                true
            } else {
                e.dispatched = generation;
                false
            }
        });
        if already {
            return;
        }
        applier.apply(mark_running);

        // Web は browser_picker でしかマウントしないので、WebPick 以外は防御の分岐。
        if req.kind != RequestKind::WebPick {
            fail(applier, &req.kind, PortFailure::Unsupported);
            return;
        }
        let Some(window) = web_sys::window() else {
            fail(applier, &req.kind, PortFailure::Unsupported);
            return;
        };
        let navigator: JsValue = window.navigator().into();
        let serial = get(&navigator, "serial");
        let has_request = serial
            .as_ref()
            .and_then(|s| get(s, "requestPort"))
            .is_some_and(|f| f.is_function());
        let (Some(serial), true, true) = (serial, has_request, window.is_secure_context()) else {
            fail(applier, &req.kind, PortFailure::Unsupported);
            return;
        };

        let options = Object::new();
        if Reflect::set(&options, &JsValue::from_str("filters"), &Array::new()).is_err() {
            fail(
                applier,
                &req.kind,
                to_port_failure(SerialPortFailure::Internal),
            );
            return;
        }
        // click の同期チェーン内での呼び出し（user activation 要件）。ここより前に await しない。
        let promise = match call(&serial, "requestPort", &[options.into()]) {
            Ok(p) => match p.dyn_into::<Promise>() {
                Ok(p) => p,
                Err(_) => {
                    fail(
                        applier,
                        &req.kind,
                        to_port_failure(SerialPortFailure::Internal),
                    );
                    return;
                }
            },
            Err(err) => {
                let f = failure_from_request_error(error_name(&err).as_deref());
                fail(applier, &req.kind, to_port_failure(f));
                return;
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let port = match JsFuture::from(promise).await {
                Ok(p) => p,
                Err(err) => {
                    let f = failure_from_request_error(error_name(&err).as_deref());
                    let pf = to_port_failure(f);
                    applier.apply(move |s| finish_round_trip(s, generation, Err(pf)));
                    return;
                }
            };
            // 選択中にマウントが破棄された。ポートは開いていないので捨てる。
            if !is_live(id) {
                return;
            }
            let started = now();
            let sess = Rc::new(RefCell::new(Session::default()));
            MOUNTS.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    e.session = Some(sess.clone());
                }
            });
            arm_timer(&sess, started);

            let mut opened = false;
            let result = drive(&sess, &port, started, &mut opened).await;
            cleanup(&sess, &port, opened).await;

            let released = sess.borrow().released;
            MOUNTS.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    if e.session.as_ref().is_some_and(|s| Rc::ptr_eq(s, &sess)) {
                        e.session = None;
                    }
                }
            });
            if released {
                return;
            }
            // 後片付けが済んでから結果を出す。
            let result = result.map_err(to_port_failure);
            applier.apply(move |s| finish_round_trip(s, generation, result));
        });
    }

    /// 期限タイマーを予約する。発火したら `timed_out` を立てて保留中の読み書きを解く。
    fn arm_timer(sess: &Rc<RefCell<Session>>, started: f64) {
        let Some(ms) = remaining_ms(started, now()) else {
            sess.borrow_mut().timed_out = true;
            return;
        };
        let Some(window) = web_sys::window() else {
            return;
        };
        let weak: Weak<RefCell<Session>> = Rc::downgrade(sess);
        let cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(s) = weak.upgrade() {
                let mut s = s.borrow_mut();
                s.timed_out = true;
                abort_io(&s);
            }
        });
        if let Ok(handle) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            ms as i32,
        ) {
            sess.borrow_mut().timer = Some((handle, cb));
        }
    }

    fn io_failure(sess: &Rc<RefCell<Session>>, err: &JsValue) -> SerialPortFailure {
        if sess.borrow().timed_out {
            SerialPortFailure::Timeout
        } else {
            failure_from_io_error(error_name(err).as_deref())
        }
    }

    /// open → 送信 → 受信。`opened` は後片付けで close するかの印。
    async fn drive(
        sess: &Rc<RefCell<Session>>,
        port: &JsValue,
        started: f64,
        opened: &mut bool,
    ) -> Result<Option<String>, SerialPortFailure> {
        let options = make_options().map_err(|_| SerialPortFailure::Internal)?;
        // open() は中断できない。期限を過ぎても完了を待ってから判定する。
        if let Err(err) = call_await(port, "open", &[options.into()]).await {
            return Err(if sess.borrow().timed_out {
                SerialPortFailure::Timeout
            } else {
                failure_from_open_error(error_name(&err).as_deref())
            });
        }
        *opened = true;
        let stop = |s: &Rc<RefCell<Session>>| {
            let s = s.borrow();
            s.released || s.timed_out
        };
        if stop(sess) {
            return Err(SerialPortFailure::Timeout);
        }

        // 送信
        let writable = get(port, "writable").ok_or(SerialPortFailure::Internal)?;
        let writer = call(&writable, "getWriter", &[]).map_err(|_| SerialPortFailure::Internal)?;
        sess.borrow_mut().writer = Some(writer.clone());
        let data = Uint8Array::from(SEND_LINE);
        if let Err(err) = call_await(&writer, "write", &[data.into()]).await {
            return Err(io_failure(sess, &err));
        }
        let _ = call(&writer, "releaseLock", &[]);
        sess.borrow_mut().writer = None;
        if stop(sess) {
            return Err(SerialPortFailure::Timeout);
        }

        // 受信
        let readable = get(port, "readable").ok_or(SerialPortFailure::Internal)?;
        let reader = call(&readable, "getReader", &[]).map_err(|_| SerialPortFailure::Internal)?;
        sess.borrow_mut().reader = Some(reader.clone());
        let mut assembler = LineAssembler::new();
        loop {
            let res = match call_await(&reader, "read", &[]).await {
                Ok(r) => r,
                Err(err) => return Err(io_failure(sess, &err)),
            };
            if get(&res, "done").and_then(|d| d.as_bool()) == Some(true) {
                return Err(if sess.borrow().timed_out {
                    SerialPortFailure::Timeout
                } else {
                    on_stream_done()
                });
            }
            let Some(bytes) = get(&res, "value").and_then(|v| v.dyn_into::<Uint8Array>().ok())
            else {
                return Err(SerialPortFailure::Internal);
            };
            if let Some(r) = on_chunk(&mut assembler, &bytes.to_vec(), started, now()) {
                return r;
            }
            let (released, timed_out) = {
                let s = sess.borrow();
                (s.released, s.timed_out)
            };
            if released || timed_out || is_late(started, now()) {
                return Err(SerialPortFailure::Timeout);
            }
        }
    }

    /// 全経路の後片付け。タイマー解除 → reader の cancel と releaseLock → writer の
    /// releaseLock → close。失敗しても結果は変えず、何も出力しない。
    async fn cleanup(sess: &Rc<RefCell<Session>>, port: &JsValue, opened: bool) {
        let (timer, reader, writer) = {
            let mut s = sess.borrow_mut();
            (s.timer.take(), s.reader.take(), s.writer.take())
        };
        if let Some((handle, cb)) = timer {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(handle);
            }
            drop(cb);
        }
        if let Some(r) = reader {
            let _ = call_await(&r, "cancel", &[]).await;
            let _ = call(&r, "releaseLock", &[]);
        }
        if let Some(w) = writer {
            let _ = call(&w, "releaseLock", &[]);
        }
        if opened {
            let _ = call_await(port, "close", &[]).await;
        }
    }

    /// マウント破棄時に進行中の往復を中断する。`mount::sweep_mounts` から呼ばれる。
    /// 資源の解放（close まで）は非同期手順が行い、結果は apply されない。
    pub(crate) fn release_mount(mount_id: u64) {
        let Some(entry) = MOUNTS.with(|m| m.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(sess) = entry.session {
            let mut s = sess.borrow_mut();
            s.released = true;
            abort_io(&s);
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::serial_port_show_effect;

/// マウント破棄時に進行中の往復を中断する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_error_mapping() {
        assert_eq!(
            failure_from_request_error(Some("NotFoundError")),
            SerialPortFailure::Cancelled
        );
        assert_eq!(
            failure_from_request_error(Some("SecurityError")),
            SerialPortFailure::PermissionDenied
        );
        for n in [Some("X"), Some("NetworkError"), None] {
            assert_eq!(failure_from_request_error(n), SerialPortFailure::Internal);
        }
    }

    #[test]
    fn open_error_mapping() {
        assert_eq!(
            failure_from_open_error(Some("InvalidStateError")),
            SerialPortFailure::Busy
        );
        assert_eq!(
            failure_from_open_error(Some("NetworkError")),
            SerialPortFailure::Busy
        );
        assert_eq!(
            failure_from_open_error(Some("SecurityError")),
            SerialPortFailure::PermissionDenied
        );
        assert_eq!(
            failure_from_open_error(Some("NotFoundError")),
            SerialPortFailure::Gone
        );
        assert_eq!(failure_from_open_error(None), SerialPortFailure::Internal);
    }

    #[test]
    fn io_error_mapping() {
        assert_eq!(
            failure_from_io_error(Some("NetworkError")),
            SerialPortFailure::Gone
        );
        for n in [
            Some("BufferOverrunError"),
            Some("BreakError"),
            Some("FramingError"),
            Some("ParityError"),
            Some("X"),
            None,
        ] {
            assert_eq!(failure_from_io_error(n), SerialPortFailure::Internal);
        }
    }

    #[test]
    fn port_failure_covers_all_variants() {
        use SerialPortFailure as F;
        assert_eq!(to_port_failure(F::Timeout), PortFailure::Timeout);
        assert_eq!(to_port_failure(F::Busy), PortFailure::Busy);
        assert_eq!(to_port_failure(F::Gone), PortFailure::Gone);
        assert_eq!(
            to_port_failure(F::PermissionDenied),
            PortFailure::PermissionDenied
        );
        assert_eq!(to_port_failure(F::Unsupported), PortFailure::Unsupported);
        assert_eq!(to_port_failure(F::LineTooLong), PortFailure::LineTooLong);
        assert_eq!(to_port_failure(F::Cancelled), PortFailure::Cancelled);
        assert_eq!(
            to_port_failure(F::Internal),
            PortFailure::Failed(FAILED_TEXT.to_string())
        );
    }

    #[test]
    fn deadline_boundaries() {
        assert_eq!(remaining_ms(100.0, 100.0), Some(3000.0));
        assert_eq!(remaining_ms(100.0, 3099.0), Some(1.0));
        assert_eq!(remaining_ms(100.0, 3100.0), None);
        assert!(is_late(100.0, 3100.0));
        assert!(!is_late(100.0, 3099.0));
        // 時計が逆行しても 0 経過として扱う。
        assert_eq!(remaining_ms(100.0, 50.0), Some(3000.0));
    }

    #[test]
    fn chunks_assemble_a_line() {
        let mut a = LineAssembler::new();
        assert!(on_chunk(&mut a, b"po", 0.0, 10.0).is_none());
        assert_eq!(
            on_chunk(&mut a, b"ng\r\nrest", 0.0, 20.0),
            Some(Ok(Some("pong".to_string())))
        );
    }

    #[test]
    fn empty_line_is_success_without_text() {
        let mut a = LineAssembler::new();
        assert_eq!(on_chunk(&mut a, b"\n", 0.0, 1.0), Some(Ok(None)));
    }

    #[test]
    fn late_line_is_timeout() {
        let mut a = LineAssembler::new();
        assert_eq!(
            on_chunk(&mut a, b"x\n", 0.0, 3000.0),
            Some(Err(SerialPortFailure::Timeout))
        );
    }

    #[test]
    fn overlong_line_fails() {
        let mut a = LineAssembler::new();
        assert_eq!(
            on_chunk(&mut a, &[b'x'; 257], 0.0, 1.0),
            Some(Err(SerialPortFailure::LineTooLong))
        );
    }

    #[test]
    fn stream_end_is_never_success() {
        assert_eq!(on_stream_done(), SerialPortFailure::Gone);
    }

    #[test]
    fn open_options_match_contract() {
        let o = open_options();
        assert_eq!(o[0], ("baudRate", OptionValue::Num(f64::from(BAUD_RATE))));
        assert_eq!(o[1], ("dataBits", OptionValue::Num(8.0)));
        assert_eq!(o[2], ("stopBits", OptionValue::Num(1.0)));
        assert_eq!(o[3], ("parity", OptionValue::Str("none")));
        assert_eq!(o[4], ("flowControl", OptionValue::Str("none")));
    }
}
