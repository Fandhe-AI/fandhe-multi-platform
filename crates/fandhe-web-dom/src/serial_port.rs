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
//! # 送信前の読み捨て
//!
//! open 後、送信の前に reader を取り、`DRAIN_QUIET_MS` の間何も届かなくなるまで届いたデータを
//! 捨てる（`DRAIN_MAX_MS` を超えて届き続けたら `Timeout`）。静止時に保留中の `read()` は
//! 受信にそのまま引き継ぐ。Web Serial に OS バッファの破棄 API は無いため、静止窓の後に機器が
//! 自発的に送る行は応答と区別できない（守らない範囲）。
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
//! 後片付けを通し、結果は apply しない。cancel・releaseLock・close のいずれかが失敗したら、
//! 往復が成功でも `Internal` の失敗にする（[`merge_cleanup`]）。解放できなかった reader・writer・
//! close はセッションに残し、マウント破棄または同じマウントの次の要求の開始時に 1 回再試行する。
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

/// 往復の結果と後片付けの成否を合わせる。往復が成功でも後片付け（cancel・releaseLock・close）に
/// 失敗していれば `Internal`（固定の失敗種別）にし、成功と見せない。往復が既に失敗していれば
/// その失敗を優先する（後片付けの失敗で原因を隠さない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn merge_cleanup<T>(
    result: Result<T, SerialPortFailure>,
    cleanup_ok: bool,
) -> Result<T, SerialPortFailure> {
    match result {
        Ok(_) if !cleanup_ok => Err(SerialPortFailure::Internal),
        other => other,
    }
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

/// 後片付け（reader の cancel・port の close）を待つ上限（ミリ秒）。保留中の write などで
/// 完了しない場合に、失敗として記録して先へ進むための値。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const CLEANUP_WAIT_MS: u32 = 2000;

/// 送信前の受信バッファ破棄で、データが途切れたとみなす静止時間（ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const DRAIN_QUIET_MS: u32 = 200;

/// 送信前の受信バッファ破棄に使う上限（ミリ秒）。送り続ける機器は応答できないとして `Timeout`。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const DRAIN_MAX_MS: f64 = 1000.0;

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
        merge_cleanup, on_chunk, on_stream_done, open_options, remaining_ms, to_port_failure,
        OptionValue, CLEANUP_WAIT_MS, DRAIN_MAX_MS, DRAIN_QUIET_MS,
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
        port: Option<JsValue>,
        /// `port.close()` が未完了（失敗）。再試行の対象。
        needs_close: bool,
        /// 後片付けが失敗して資源が残っている。再試行の対象。
        leftover: bool,
        /// 後片付けの実行中。同じセッションの後片付けを並行させない。
        cleaning: bool,
        /// 期限・破棄で解決する Promise。保留中の読み書きの完了待ちと競合させる。
        signal: Option<Promise>,
        resolve_signal: Option<Function>,
        /// `signal` が解決する値（読み書きの結果と区別する目印）。
        token: JsValue,
    }

    impl Session {
        fn new() -> Self {
            let slot: Rc<RefCell<Option<Function>>> = Rc::new(RefCell::new(None));
            let slot2 = slot.clone();
            let signal = Promise::new(&mut |resolve, _| {
                *slot2.borrow_mut() = Some(resolve);
            });
            let resolve_signal = slot.borrow_mut().take();
            Session {
                signal: Some(signal),
                resolve_signal,
                token: Object::new().into(),
                ..Default::default()
            }
        }
    }

    /// 保留中の読み書きの待ちを解く合図を出す。
    fn fire_signal(s: &Session) {
        if let Some(f) = &s.resolve_signal {
            let _ = f.call1(&JsValue::UNDEFINED, &s.token);
        }
    }

    enum Wait {
        /// 期限・破棄の合図が先に来た。読み書きの完了は待たない。
        Aborted,
        Failed(JsValue),
    }

    /// 読み書きの Promise を期限・破棄の合図と競合させて待つ。`abort` / `cancel` は
    /// 進行中の sink の完了を待つため、未解決のまま止まる write でも合図で抜けられるようにする。
    async fn wait_io(
        sess: &Rc<RefCell<Session>>,
        pending: Result<JsValue, JsValue>,
    ) -> Result<JsValue, Wait> {
        let v = pending.map_err(Wait::Failed)?;
        let (signal, token) = {
            let s = sess.borrow();
            (s.signal.clone(), s.token.clone())
        };
        let Some(signal) = signal else {
            return JsFuture::from(Promise::resolve(&v))
                .await
                .map_err(Wait::Failed);
        };
        let race = Promise::race(&Array::of2(&Promise::resolve(&v), &signal));
        match JsFuture::from(race).await {
            Ok(r) if Object::is(&r, &token) => Err(Wait::Aborted),
            Ok(r) => Ok(r),
            Err(e) => Err(Wait::Failed(e)),
        }
    }

    /// 後片付けの待ちに上限を付ける。上限内に終わらなければ `Err`（失敗として記録し再試行に回す）。
    async fn bounded(pending: Result<JsValue, JsValue>) -> Result<JsValue, JsValue> {
        let v = pending?;
        let marker = JsValue::from_str("__cleanup_deadline__");
        let m2 = marker.clone();
        let deadline = Promise::new(&mut |resolve, _| {
            let m = m2.clone();
            let cb = Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::UNDEFINED, &m);
            });
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb.unchecked_ref(),
                    CLEANUP_WAIT_MS as i32,
                );
            }
        });
        let race = Promise::race(&Array::of2(&Promise::resolve(&v), &deadline));
        match JsFuture::from(race).await {
            Ok(r) if Object::is(&r, &marker) => Err(JsValue::from_str("cleanup deadline exceeded")),
            other => other,
        }
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
        fire_signal(s);
        if let Some(r) = &s.reader {
            let _ = call(r, "cancel", &[]);
        }
        if let Some(w) = &s.writer {
            let _ = call(w, "abort", &[]);
        }
    }

    /// 後片付けに失敗して残った資源を解放し直す（結果は見ない・apply しない）。
    fn retry_leftover(sess: Rc<RefCell<Session>>) {
        if !sess.borrow().leftover {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            let _ = cleanup(&sess).await;
        });
    }

    /// マウントに残った後片付け失敗のセッションを解放し直す。済むまで待つ。
    /// 成功すれば記録を消して `true`。失敗すれば記録を残し（破棄時にも再試行できる）`false`。
    /// `false` のとき呼び出し側は新しい往復を始めない（未解放の資源を新セッションで上書きしない）。
    async fn settle_leftover(id: u64) -> bool {
        let old = MOUNTS.with(|m| m.borrow().get(&id).and_then(|e| e.session.clone()));
        let Some(old) = old else {
            return true;
        };
        if !old.borrow().leftover {
            return true;
        }
        if cleanup(&old).await {
            MOUNTS.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    if e.session.as_ref().is_some_and(|s| Rc::ptr_eq(s, &old)) {
                        e.session = None;
                    }
                }
            });
            true
        } else {
            false
        }
    }

    /// 一定時間後に `marker` で解決する Promise。
    fn quiet_timer(ms: u32, marker: &JsValue) -> Promise {
        let m2 = marker.clone();
        Promise::new(&mut |resolve, _| {
            let m = m2.clone();
            let cb = Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::UNDEFINED, &m);
            });
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb.unchecked_ref(),
                    ms as i32,
                );
            }
        })
    }

    /// 送信前に、すでに届いている（届き続ける）受信データを読み捨てる。`DRAIN_QUIET_MS` の間
    /// 何も届かなければ静止とみなし、保留中の `read()` を返す（以後の受信に引き継ぐ。cancel しない）。
    /// Web Serial にバッファ破棄 API は無いので、読み捨てが保証できるのは静止窓の手前までで、
    /// 静止窓の後に機器が自発的に送る行は区別できない（守らない範囲）。
    async fn drain_before_send(
        sess: &Rc<RefCell<Session>>,
        reader: &JsValue,
    ) -> Result<JsValue, SerialPortFailure> {
        let began = now();
        let mut pending = call(reader, "read", &[]).map_err(|_| SerialPortFailure::Internal)?;
        loop {
            let (signal, token) = {
                let s = sess.borrow();
                (s.signal.clone(), s.token.clone())
            };
            let marker = JsValue::from_str("__drain_quiet__");
            let race = Array::of3(
                &Promise::resolve(&pending),
                &quiet_timer(DRAIN_QUIET_MS, &marker),
                &signal.map(JsValue::from).unwrap_or(JsValue::NULL),
            );
            match JsFuture::from(Promise::race(&race)).await {
                Ok(r) if Object::is(&r, &marker) => return Ok(pending),
                Ok(r) if Object::is(&r, &token) => return Err(SerialPortFailure::Timeout),
                Ok(r) => {
                    if get(&r, "done").and_then(|d| d.as_bool()) == Some(true) {
                        return Err(if sess.borrow().timed_out {
                            SerialPortFailure::Timeout
                        } else {
                            on_stream_done()
                        });
                    }
                    // 送信前に届いたデータは応答ではない。捨てる。
                    let s = sess.borrow();
                    if s.released || s.timed_out || now() - began >= DRAIN_MAX_MS {
                        return Err(SerialPortFailure::Timeout);
                    }
                    drop(s);
                    pending = call(reader, "read", &[]).map_err(|_| SerialPortFailure::Internal)?;
                }
                Err(e) => return Err(io_failure(sess, &e)),
            }
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
                    // キャンセルの経路でも、残った資源の解放を試みてから結果を出す。
                    // 解放できなくても、要求自体の失敗はそのまま出す（資源は記録に残る）。
                    let _ = settle_leftover(id).await;
                    let f = failure_from_request_error(error_name(&err).as_deref());
                    let pf = to_port_failure(f);
                    applier.apply(move |s| finish_round_trip(s, generation, Err(pf)));
                    return;
                }
            };
            // 前回の後片付け失敗で残った資源を、open の前に解放し終える（open と重ねない）。
            let settled = settle_leftover(id).await;
            // 選択中にマウントが破棄された。ポートは開いていないので捨てる。
            if !is_live(id) {
                return;
            }
            // 旧セッションを解放できないまま新しい往復を始めない。旧セッションは記録に残り、
            // 次の要求・マウント破棄で再試行される。
            if !settled {
                let pf = to_port_failure(SerialPortFailure::Internal);
                applier.apply(move |s| finish_round_trip(s, generation, Err(pf)));
                return;
            }
            let started = now();
            let sess = Rc::new(RefCell::new(Session::new()));
            MOUNTS.with(|m| {
                if let Some(e) = m.borrow_mut().get_mut(&id) {
                    if let Some(old) = e.session.replace(sess.clone()) {
                        retry_leftover(old);
                    }
                }
            });
            arm_timer(&sess, started);

            sess.borrow_mut().port = Some(port.clone());
            let mut opened = false;
            let result = drive(&sess, &port, started, &mut opened).await;
            sess.borrow_mut().needs_close = opened;
            let cleanup_ok = cleanup(&sess).await;

            let released = sess.borrow().released;
            // 資源が残っているセッションはマウントに残し、再試行できるようにする。
            if cleanup_ok {
                MOUNTS.with(|m| {
                    if let Some(e) = m.borrow_mut().get_mut(&id) {
                        if e.session.as_ref().is_some_and(|s| Rc::ptr_eq(s, &sess)) {
                            e.session = None;
                        }
                    }
                });
            } else if released {
                // 破棄済みでマウントから外れている。ここで 1 回だけ再試行する。
                retry_leftover(sess.clone());
            }
            if released {
                return;
            }
            // 後片付けが済んでから結果を出す。後片付けの失敗は成功を失敗に変える。
            let result = merge_cleanup(result, cleanup_ok).map_err(to_port_failure);
            applier.apply(move |s| finish_round_trip(s, generation, result));
        });
    }

    /// 期限タイマーを予約する。発火したら `timed_out` を立てて保留中の読み書きを解く。
    fn arm_timer(sess: &Rc<RefCell<Session>>, started: f64) {
        let Some(ms) = remaining_ms(started, now()) else {
            let mut s = sess.borrow_mut();
            s.timed_out = true;
            fire_signal(&s);
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

        // 送信前に受信データを読み捨てる（起動メッセージなどを応答と取り違えない）。
        let readable = get(port, "readable").ok_or(SerialPortFailure::Internal)?;
        let reader = call(&readable, "getReader", &[]).map_err(|_| SerialPortFailure::Internal)?;
        sess.borrow_mut().reader = Some(reader.clone());
        let mut pending = drain_before_send(sess, &reader).await?;
        if stop(sess) {
            return Err(SerialPortFailure::Timeout);
        }

        // 送信
        let writable = get(port, "writable").ok_or(SerialPortFailure::Internal)?;
        let writer = call(&writable, "getWriter", &[]).map_err(|_| SerialPortFailure::Internal)?;
        sess.borrow_mut().writer = Some(writer.clone());
        let data = Uint8Array::from(SEND_LINE);
        match wait_io(sess, call(&writer, "write", &[data.into()])).await {
            Ok(_) => {}
            Err(Wait::Aborted) => return Err(SerialPortFailure::Timeout),
            Err(Wait::Failed(err)) => return Err(io_failure(sess, &err)),
        }
        let _ = call(&writer, "releaseLock", &[]);
        sess.borrow_mut().writer = None;
        if stop(sess) {
            return Err(SerialPortFailure::Timeout);
        }

        // 受信（静止確認で保留にした read から続ける）
        let mut assembler = LineAssembler::new();
        loop {
            let res = match wait_io(sess, Ok(pending)).await {
                Ok(r) => r,
                Err(Wait::Aborted) => return Err(SerialPortFailure::Timeout),
                Err(Wait::Failed(err)) => return Err(io_failure(sess, &err)),
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
            pending = call(&reader, "read", &[]).map_err(|_| SerialPortFailure::Internal)?;
        }
    }

    /// 全経路の後片付け。タイマー解除 → reader の cancel と releaseLock → writer の
    /// releaseLock → close。各段の失敗を区別して記録し、全て成功したときだけ `true`。
    /// 解放できなかった reader・writer・close はセッションに戻し、再試行できる状態で残す。
    async fn cleanup(sess: &Rc<RefCell<Session>>) -> bool {
        let (timer, reader, writer, port, needs_close) = {
            let mut s = sess.borrow_mut();
            if s.cleaning {
                return false;
            }
            s.cleaning = true;
            (
                s.timer.take(),
                s.reader.take(),
                s.writer.take(),
                s.port.clone(),
                s.needs_close,
            )
        };
        if let Some((handle, cb)) = timer {
            if let Some(w) = web_sys::window() {
                w.clear_timeout_with_handle(handle);
            }
            drop(cb);
        }
        let mut ok = true;
        // writer のロックは待たずに先に手放す（保留中の write の完了に依存しない）。
        if let Some(w) = writer {
            if call(&w, "releaseLock", &[]).is_err() {
                ok = false;
                sess.borrow_mut().writer = Some(w);
            }
        }
        if let Some(r) = reader {
            // cancel の失敗と releaseLock の失敗は別に扱う。releaseLock に失敗した reader は戻す。
            // 待ちには上限を付け、超えたら失敗として記録する。
            if bounded(call(&r, "cancel", &[])).await.is_err() {
                ok = false;
            }
            if call(&r, "releaseLock", &[]).is_err() {
                ok = false;
                sess.borrow_mut().reader = Some(r);
            }
        }
        if needs_close {
            if let Some(port) = port {
                if bounded(call(&port, "close", &[])).await.is_err() {
                    ok = false;
                } else {
                    sess.borrow_mut().needs_close = false;
                }
            }
        }
        {
            let mut s = sess.borrow_mut();
            s.leftover = !ok;
            s.cleaning = false;
        }
        ok
    }

    /// マウント破棄時に進行中の往復を中断する。`mount::sweep_mounts` から呼ばれる。
    /// 資源の解放（close まで）は非同期手順が行い、結果は apply されない。
    pub(crate) fn release_mount(mount_id: u64) {
        let Some(entry) = MOUNTS.with(|m| m.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(sess) = entry.session {
            {
                let mut s = sess.borrow_mut();
                s.released = true;
                abort_io(&s);
            }
            // 後片付けに失敗して資源が残っていれば、ここで解放し直す。
            retry_leftover(sess);
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

    /// 後片付けの待ち上限は 0 より大きく、期限（3 秒）を大きく超えない。
    const _: () = assert!(CLEANUP_WAIT_MS > 0 && CLEANUP_WAIT_MS <= 5000);

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
    fn cleanup_failure_turns_success_into_internal() {
        let ok: Result<Option<String>, _> = Ok(Some("pong".to_string()));
        assert_eq!(merge_cleanup(ok.clone(), true), ok);
        assert_eq!(merge_cleanup(ok, false), Err(SerialPortFailure::Internal));
        // 成功が空行でも同じ。
        assert_eq!(
            merge_cleanup(Ok::<Option<String>, _>(None), false),
            Err(SerialPortFailure::Internal)
        );
    }

    #[test]
    fn cleanup_failure_keeps_original_failure() {
        for f in [
            SerialPortFailure::Timeout,
            SerialPortFailure::Gone,
            SerialPortFailure::LineTooLong,
        ] {
            assert_eq!(merge_cleanup::<()>(Err(f), false), Err(f));
            assert_eq!(merge_cleanup::<()>(Err(f), true), Err(f));
        }
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
