//! 型サンプル「NFC タグ読み取り」の Web 経路（Issue #1046、親 #157、共通部分は #1044）。
//!
//! `fandhe_core::screens::nfc_read`（端末依存ゼロ）の要求を [`super::mount::mount_with_effect`] の
//! effect として観測し、Web NFC（`NDEFReader.scan()`）で最初にかざした 1 枚の NDEF を読んで
//! `finish_read` へ書き戻す。呼び出し元は `demo.rs` の `mount_nfc_read_screen` と、
//! マウント破棄時の `mount::sweep_mounts`（[`release_mount`]）。
//!
//! # ブロッキング trait を通らない理由
//!
//! `fandhe_device::nfc::NfcReader` は `Send + Sync` のブロッキング API で、`JsValue` を持てない。
//! Web は effect で非同期に処理し、契約（写像表・定数・`decode_text`・表示型）だけを
//! `fandhe_device::nfc` と共有する。写像表の Web 列は同モジュールの doc が SSOT。
//!
//! # 期限・ライフサイクル（契約表 1・2・3・9）
//!
//! - `scan()` は click から続く同期チェーンの中で呼ぶ（transient activation と権限プロンプトのため、
//!   呼ぶ前に `await` を挟まない）。リスナーと期限タイマーは `scan()` より先に登録する
//!   （`scan()` の直後に届くイベントを落とさない）。
//! - 期限の起点は呼び出しの開始で、`performance.now()`（単調時計）で測る。期限後に届いた読み取りは
//!   成功にしない。
//! - 確定は 1 回だけ。確定の前にタイマー・リスナーを外し `abort()` してから結果を画面へ届けるので、
//!   確定後に届いた `reading` や `AbortError` は捨てられる。実行中の Closure は次のマイクロタスクで drop する。
//! - ページ非表示（`visibilitychange`）と `pagehide` は `Interrupted`（仕様上、非表示の間は NFC が止まるため
//!   期限まで待たない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る範囲は 1 画面内での要求と完了の順序、嘘の成功の禁止、リスナーの解除。守らない範囲は複数タブ、
//! 権限プロンプト中に非表示になった場合の扱い（ブラウザに従う）、2 枚同時にかざした場合にどれが読まれるか
//! （`nfc.json` の `note` と同じ）。サイズ上限は Web がメッセージの生バイトを持たないため、
//! ペイロード長の合計で近似する（コピーの前に `byteLength` だけで判定する）。
//!
//! # 秘密値（契約表 8）
//!
//! タグの中身・URL・UID（`serialNumber`）・record の `id`・例外の `message` は読まない・出さない。
//! テキストは trim せず、URL は開かない。

use fandhe_core::screens::nfc_read::{CoreFailure, ShownRecord, ShownTag};
use fandhe_device::nfc::{
    decode_text, NdefRecordView, NfcReadFailure, NfcReadOutcome, MAX_NDEF_BYTES, READ_DEADLINE,
};

/// Web NFC の `NDEFRecord` から必要な値だけを写したもの。`id` と UID は読まない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WebRecord {
    pub(crate) record_type: String,
    pub(crate) media_type: Option<String>,
    pub(crate) encoding: Option<String>,
    pub(crate) lang: Option<String>,
    pub(crate) data: Vec<u8>,
}

/// ペイロード長の合計が上限を超えるか。中身をコピーする前に `byteLength` だけで判定する。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn exceeds_limit(data_lens: &[usize]) -> bool {
    data_lens
        .iter()
        .try_fold(0usize, |a, &n| a.checked_add(n))
        .is_none_or(|sum| sum > MAX_NDEF_BYTES)
}

fn utf8(b: &[u8]) -> Result<String, NfcReadFailure> {
    std::str::from_utf8(b)
        .map(str::to_string)
        .map_err(|_| NfcReadFailure::Unreadable)
}

/// `text` レコードを `decode_text`（厳格な復号）に通せるペイロードへ組み直す。
fn text_view(r: &WebRecord) -> Result<NdefRecordView, NfcReadFailure> {
    let lang = r.lang.as_deref().unwrap_or("").as_bytes();
    if lang.len() > 0x3F {
        return Err(NfcReadFailure::Unreadable);
    }
    let mut status = lang.len() as u8;
    let mut body = r.data.clone();
    match r.encoding.as_deref().unwrap_or("utf-8") {
        "utf-8" | "" => {}
        // BOM 無しの BE は `decode_text` の既定どおり。
        "utf-16be" | "utf-16" => status |= 0x80,
        "utf-16le" => {
            status |= 0x80;
            if !body.starts_with(&[0xFF, 0xFE]) {
                body.splice(0..0, [0xFF, 0xFE]);
            }
        }
        _ => return Err(NfcReadFailure::Unreadable),
    }
    let mut payload = vec![status];
    payload.extend_from_slice(lang);
    payload.extend_from_slice(&body);
    decode_text(&payload)
}

/// 読んだレコード列を成功値へ写す。未知の型は `Unreadable`（fail-closed）、全件 `empty` なら `EmptyTag`。
/// `Other` の表示は Android の `parse_ndef` に揃える。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_records(
    records: Vec<WebRecord>,
) -> Result<NfcReadOutcome, NfcReadFailure> {
    let mut views = Vec::new();
    for r in &records {
        let len = r.data.len();
        let t = r.record_type.as_str();
        views.push(match t {
            "empty" => continue,
            "text" => text_view(r)?,
            "url" | "absolute-url" => NdefRecordView::Uri(utf8(&r.data)?),
            "mime" => NdefRecordView::Mime {
                type_name: r.media_type.clone().ok_or(NfcReadFailure::Unreadable)?,
                len,
            },
            "smart-poster" => NdefRecordView::Other {
                type_name: "Sp".into(),
                len,
            },
            "unknown" => NdefRecordView::Other {
                type_name: String::new(),
                len,
            },
            _ if t.starts_with(':') => NdefRecordView::Other {
                type_name: t[1..].to_string(),
                len,
            },
            _ if t.contains(':') => NdefRecordView::External {
                type_name: t.to_string(),
                len,
            },
            _ => return Err(NfcReadFailure::Unreadable),
        });
    }
    Ok(NfcReadOutcome::from_records(views))
}

/// 例外の `name` → 失敗。`message` は読まない。どの名前も成功にならない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> NfcReadFailure {
    match name {
        Some("NotAllowedError") => NfcReadFailure::PermissionDenied,
        Some("NotSupportedError") => NfcReadFailure::Unsupported,
        Some("AbortError") => NfcReadFailure::Interrupted,
        _ => NfcReadFailure::Failed,
    }
}

/// 開始から期限以上経ったか（`performance.now()` の単調時計、ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_late(started_ms: f64, now_ms: f64) -> bool {
    now_ms - started_ms >= READ_DEADLINE.as_secs_f64() * 1000.0
}

/// 契約の結果を画面の結果へ写す。`fandhe-egui-proxy/src/device/nfc.rs` の `to_core_record` /
/// `to_core_failure` と同期する（同クレートに依存しないため同じ写像をここに置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_core(r: Result<NfcReadOutcome, NfcReadFailure>) -> Result<ShownTag, CoreFailure> {
    match r {
        Ok(NfcReadOutcome::Read(rs)) => Ok(ShownTag::Records(
            rs.into_iter()
                .map(|v| match v {
                    NdefRecordView::Text { lang, text } => ShownRecord::Text { lang, text },
                    NdefRecordView::Uri(u) => ShownRecord::Uri(u),
                    NdefRecordView::Mime { type_name, len } => ShownRecord::Mime { type_name, len },
                    NdefRecordView::External { type_name, len } => {
                        ShownRecord::External { type_name, len }
                    }
                    NdefRecordView::Other { type_name, len } => {
                        ShownRecord::Other { type_name, len }
                    }
                })
                .collect(),
        )),
        Ok(NfcReadOutcome::EmptyTag) => Ok(ShownTag::Empty),
        Err(f) => Err(match f {
            NfcReadFailure::Unreadable => CoreFailure::Unreadable,
            NfcReadFailure::TooLarge => CoreFailure::TooLarge,
            NfcReadFailure::Disabled => CoreFailure::Disabled,
            NfcReadFailure::PermissionDenied => CoreFailure::PermissionDenied,
            NfcReadFailure::TimedOut => CoreFailure::TimedOut,
            NfcReadFailure::Interrupted => CoreFailure::Interrupted,
            NfcReadFailure::Unsupported => CoreFailure::Unsupported,
            NfcReadFailure::Failed => CoreFailure::Failed,
        }),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::nfc_read::{finish_read, mark_running, take_request, NfcReadState};
    use fandhe_device::nfc::{NfcReadFailure, NfcReadOutcome, READ_DEADLINE};
    use js_sys::{Array, DataView, Function, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::{spawn_local, JsFuture};
    use web_sys::{AbortController, EventTarget};

    use super::{
        exceeds_limit, failure_from_error_name, is_late, outcome_from_records, to_core, WebRecord,
    };
    use crate::mount::Applier;

    type Cb = Closure<dyn FnMut(JsValue)>;

    /// 飛行中の読み取り 1 件。`finalize` で解除してから drop する。
    struct Active {
        ctrl: AbortController,
        reader: EventTarget,
        on_reading: Cb,
        on_error: Cb,
        on_vis: Cb,
        on_hide: Cb,
        timer_id: i32,
        _timer_cb: Closure<dyn FnMut()>,
    }

    /// マウントごとの記録。`generation` は発送済みの世代（二重発送の防止）。
    struct Entry {
        generation: u64,
        active: Option<Active>,
    }

    thread_local! {
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    fn now() -> f64 {
        web_sys::window()
            .and_then(|w| w.performance())
            .map_or(0.0, |p| p.now())
    }

    fn get(o: &JsValue, k: &str) -> Option<JsValue> {
        Reflect::get(o, &JsValue::from_str(k))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_null())
    }

    /// 例外の `name` だけを失敗へ写す（`message` は読まない）。
    fn failure_of(err: &JsValue) -> NfcReadFailure {
        failure_from_error_name(get(err, "name").and_then(|v| v.as_string()).as_deref())
    }

    /// 解除を済ませる。実行中の Closure を自分の中で drop しないよう、drop は次のマイクロタスクへ送る。
    fn teardown(a: Active) {
        if let Some(w) = web_sys::window() {
            w.clear_timeout_with_handle(a.timer_id);
            let _ = w.remove_event_listener_with_callback(
                "pagehide",
                a.on_hide.as_ref().unchecked_ref(),
            );
            if let Some(d) = w.document() {
                let _ = d.remove_event_listener_with_callback(
                    "visibilitychange",
                    a.on_vis.as_ref().unchecked_ref(),
                );
            }
        }
        let _ = a
            .reader
            .remove_event_listener_with_callback("reading", a.on_reading.as_ref().unchecked_ref());
        let _ = a.reader.remove_event_listener_with_callback(
            "readingerror",
            a.on_error.as_ref().unchecked_ref(),
        );
        a.ctrl.abort();
        spawn_local(async move { drop(a) });
    }

    /// 現在の世代の読み取りを 1 回だけ確定する。確定できたときだけ true（解除済み）。
    fn finalize(mount: u64, generation: u64) -> bool {
        let taken = ENTRIES.with(|m| {
            m.borrow_mut()
                .get_mut(&mount)
                .filter(|e| e.generation == generation)
                .and_then(|e| e.active.take())
        });
        match taken {
            Some(a) => {
                teardown(a);
                true
            }
            None => false,
        }
    }

    /// 解除してから結果を画面へ届ける。確定済み・古い世代なら何もしない（契約表 2・3）。
    fn settle(
        applier: &Applier<NfcReadState>,
        generation: u64,
        r: Result<NfcReadOutcome, NfcReadFailure>,
    ) {
        if finalize(applier.mount_id(), generation) {
            applier.apply(move |s| {
                finish_read(s, generation, to_core(r));
            });
        }
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解除して記録を消す。画面へは書き戻さない。
    pub(crate) fn release_mount(mount: u64) {
        if let Some(e) = ENTRIES.with(|m| m.borrow_mut().remove(&mount)) {
            if let Some(a) = e.active {
                teardown(a);
            }
        }
    }

    /// `reading` イベントの `message.records` を読む。コピーの前に合計長で上限を判定する。
    fn read_message(ev: &JsValue) -> Result<NfcReadOutcome, NfcReadFailure> {
        let records = get(ev, "message")
            .and_then(|m| get(&m, "records"))
            .and_then(|r| r.dyn_into::<Array>().ok())
            .ok_or(NfcReadFailure::Unreadable)?;
        let views: Vec<(JsValue, Option<DataView>)> = records
            .iter()
            .map(|r| {
                let dv = get(&r, "data").and_then(|d| d.dyn_into::<DataView>().ok());
                (r, dv)
            })
            .collect();
        let lens: Vec<usize> = views
            .iter()
            .map(|(_, dv)| dv.as_ref().map_or(0, |d| d.byte_length()))
            .collect();
        if exceeds_limit(&lens) {
            return Err(NfcReadFailure::TooLarge);
        }
        let mut out = Vec::with_capacity(views.len());
        for (r, dv) in views {
            let s = |k: &str| get(&r, k).and_then(|v| v.as_string());
            let data = dv.map_or_else(Vec::new, |d| {
                Uint8Array::new_with_byte_offset_and_length(
                    &d.buffer(),
                    d.byte_offset() as u32,
                    d.byte_length() as u32,
                )
                .to_vec()
            });
            out.push(WebRecord {
                record_type: s("recordType").ok_or(NfcReadFailure::Unreadable)?,
                media_type: s("mediaType"),
                encoding: s("encoding"),
                lang: s("lang"),
                data,
            });
        }
        outcome_from_records(out)
    }

    /// 事前判定。使える環境でなければ `Unsupported`。要求のたびに問う（機種判定は置かない）。
    fn precheck(window: &web_sys::Window) -> bool {
        let win: JsValue = window.clone().into();
        window.is_secure_context() && get(&win, "NDEFReader").is_some_and(|c| c.is_function())
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ `scan()` を依頼する。
    pub fn nfc_read_effect(state: &NfcReadState, applier: &Applier<NfcReadState>) {
        let Some(generation) = take_request(state) else {
            return;
        };
        let mount = applier.mount_id();
        let fresh = ENTRIES.with(|m| {
            let mut m = m.borrow_mut();
            if m.get(&mount).map(|e| e.generation) == Some(generation) {
                return false;
            }
            // 前の世代が残っていれば解除してから置き換える。
            if let Some(a) = m
                .insert(
                    mount,
                    Entry {
                        generation,
                        active: None,
                    },
                )
                .and_then(|e| e.active)
            {
                teardown(a);
            }
            true
        });
        if !fresh {
            return;
        }
        applier.apply(mark_running);

        let fail = |f: NfcReadFailure| settle_unstarted(applier, generation, f);
        let Some(window) = web_sys::window() else {
            return fail(NfcReadFailure::Unsupported);
        };
        if !precheck(&window) {
            return fail(NfcReadFailure::Unsupported);
        }
        let started = now();
        let win: JsValue = window.clone().into();
        let Some(reader) = get(&win, "NDEFReader")
            .and_then(|c| c.dyn_into::<Function>().ok())
            .and_then(|c| Reflect::construct(&c, &Array::new()).ok())
        else {
            return fail(NfcReadFailure::Failed);
        };
        let Ok(ctrl) = AbortController::new() else {
            return fail(NfcReadFailure::Failed);
        };
        let Ok(target) = reader.clone().dyn_into::<EventTarget>() else {
            return fail(NfcReadFailure::Failed);
        };

        let settle_with = |r: Result<NfcReadOutcome, NfcReadFailure>| {
            let a = applier.clone();
            move || settle(&a, generation, r.clone())
        };
        let on_reading: Cb = {
            let a = applier.clone();
            Closure::new(move |ev: JsValue| {
                let r = if is_late(started, now()) {
                    Err(NfcReadFailure::TimedOut)
                } else {
                    read_message(&ev)
                };
                settle(&a, generation, r);
            })
        };
        let on_error: Cb = {
            let f = settle_with(Err(NfcReadFailure::Unreadable));
            Closure::new(move |_| f())
        };
        let on_vis: Cb = {
            let f = settle_with(Err(NfcReadFailure::Interrupted));
            Closure::new(move |_| {
                if web_sys::window()
                    .and_then(|w| w.document())
                    .is_some_and(|d| d.hidden())
                {
                    f();
                }
            })
        };
        let on_hide: Cb = {
            let f = settle_with(Err(NfcReadFailure::Interrupted));
            Closure::new(move |_| f())
        };
        let timer_cb: Closure<dyn FnMut()> = {
            let f = settle_with(Err(NfcReadFailure::TimedOut));
            Closure::new(f)
        };

        // リスナーと期限タイマーを scan() より先に登録する（契約表 1）。
        let ms = READ_DEADLINE.as_millis() as i32;
        let Ok(timer_id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            timer_cb.as_ref().unchecked_ref(),
            ms,
        ) else {
            return fail(NfcReadFailure::Failed);
        };
        let _ =
            target.add_event_listener_with_callback("reading", on_reading.as_ref().unchecked_ref());
        let _ = target
            .add_event_listener_with_callback("readingerror", on_error.as_ref().unchecked_ref());
        let _ =
            window.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
        if let Some(d) = window.document() {
            let _ = d.add_event_listener_with_callback(
                "visibilitychange",
                on_vis.as_ref().unchecked_ref(),
            );
        }
        let signal = ctrl.signal();
        ENTRIES.with(|m| {
            if let Some(e) = m
                .borrow_mut()
                .get_mut(&mount)
                .filter(|e| e.generation == generation)
            {
                e.active = Some(Active {
                    ctrl,
                    reader: target,
                    on_reading,
                    on_error,
                    on_vis,
                    on_hide,
                    timer_id,
                    _timer_cb: timer_cb,
                });
            }
        });

        // transient activation を保つため、ここまでに await を挟まない。
        let opts = js_sys::Object::new();
        let _ = Reflect::set(&opts, &JsValue::from_str("signal"), &signal);
        let scan = get(&reader, "scan").and_then(|f| f.dyn_into::<Function>().ok());
        let Some(scan) = scan else {
            return settle(applier, generation, Err(NfcReadFailure::Failed));
        };
        match scan.call1(&reader, &opts) {
            // resolve は何もしない（イベントを待つ）。reject は名前で確定する。
            Ok(p) => {
                let a = applier.clone();
                spawn_local(async move {
                    if let Err(e) = JsFuture::from(js_sys::Promise::resolve(&p)).await {
                        settle(&a, generation, Err(failure_of(&e)));
                    }
                });
            }
            Err(e) => settle(applier, generation, Err(failure_of(&e))),
        }
    }

    /// リスナー登録前の失敗。記録だけあり `active` は無いので、そのまま結果を届ける。
    fn settle_unstarted(applier: &Applier<NfcReadState>, generation: u64, f: NfcReadFailure) {
        applier.apply(move |s| {
            finish_read(s, generation, to_core(Err(f)));
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::nfc_read_effect;

/// マウント破棄時に飛行中の読み取りを解除する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use NfcReadFailure as F;

    fn rec(t: &str, data: &[u8]) -> WebRecord {
        WebRecord {
            record_type: t.into(),
            data: data.to_vec(),
            ..Default::default()
        }
    }

    fn text(enc: Option<&str>, lang: Option<&str>, data: &[u8]) -> WebRecord {
        WebRecord {
            record_type: "text".into(),
            encoding: enc.map(Into::into),
            lang: lang.map(Into::into),
            data: data.to_vec(),
            ..Default::default()
        }
    }

    fn one(r: WebRecord) -> Result<NdefRecordView, F> {
        match outcome_from_records(vec![r])? {
            NfcReadOutcome::Read(mut v) if v.len() == 1 => Ok(v.remove(0)),
            o => panic!("1 件のはず: {o:?}"),
        }
    }

    fn t(lang: &str, s: &str) -> NdefRecordView {
        NdefRecordView::Text {
            lang: lang.into(),
            text: s.into(),
        }
    }

    #[test]
    fn text_encodings() {
        assert_eq!(
            one(text(Some("utf-8"), Some("ja"), " あ ".as_bytes())),
            Ok(t("ja", " あ "))
        );
        assert_eq!(one(text(None, None, b"x")), Ok(t("", "x")));
        assert_eq!(
            one(text(Some("utf-16le"), Some("en"), &[b'a', 0, b'b', 0])),
            Ok(t("en", "ab"))
        );
        // BOM が既にあれば二重に足さない。
        assert_eq!(
            one(text(Some("utf-16le"), Some("en"), &[0xFF, 0xFE, b'a', 0])),
            Ok(t("en", "a"))
        );
        assert_eq!(
            one(text(Some("utf-16be"), Some("en"), &[0, b'a'])),
            Ok(t("en", "a"))
        );
    }

    #[test]
    fn text_bad_inputs_are_unreadable() {
        let long = "a".repeat(64);
        for r in [
            text(Some("utf-8"), Some(&long), b"x"),
            text(Some("utf-8"), Some("en"), &[0xFF, 0xFE]),
            text(Some("utf-16le"), Some("en"), b"a"),
            text(Some("utf-16le"), Some("en"), &[0, 0xD8, 0x41, 0]),
            text(Some("latin1"), Some("en"), b"x"),
        ] {
            assert_eq!(one(r), Err(F::Unreadable));
        }
        // lang ちょうど 63 バイトは通る。
        assert!(one(text(None, Some(&"a".repeat(63)), b"x")).is_ok());
    }

    #[test]
    fn record_types() {
        assert_eq!(
            one(rec("url", b"https://a.example/")),
            Ok(NdefRecordView::Uri("https://a.example/".into()))
        );
        assert_eq!(one(rec("absolute-url", &[0xFF])), Err(F::Unreadable));
        let mime = WebRecord {
            media_type: Some("x/y".into()),
            ..rec("mime", b"abc")
        };
        assert_eq!(
            one(mime),
            Ok(NdefRecordView::Mime {
                type_name: "x/y".into(),
                len: 3
            })
        );
        assert_eq!(one(rec("mime", b"abc")), Err(F::Unreadable));
        assert_eq!(
            one(rec("smart-poster", b"ab")),
            Ok(NdefRecordView::Other {
                type_name: "Sp".into(),
                len: 2
            })
        );
        assert_eq!(
            one(rec("unknown", b"a")),
            Ok(NdefRecordView::Other {
                type_name: String::new(),
                len: 1
            })
        );
        assert_eq!(
            one(rec(":xyz", b"a")),
            Ok(NdefRecordView::Other {
                type_name: "xyz".into(),
                len: 1
            })
        );
        assert_eq!(
            one(rec("example.com:t", b"ab")),
            Ok(NdefRecordView::External {
                type_name: "example.com:t".into(),
                len: 2
            })
        );
        assert_eq!(one(rec("bogus", b"")), Err(F::Unreadable));
    }

    #[test]
    fn empty_records() {
        assert_eq!(outcome_from_records(vec![]), Ok(NfcReadOutcome::EmptyTag));
        assert_eq!(
            outcome_from_records(vec![rec("empty", b""), rec("empty", b"")]),
            Ok(NfcReadOutcome::EmptyTag)
        );
        let mixed = outcome_from_records(vec![rec("empty", b""), rec("url", b"u")]).unwrap();
        assert!(matches!(mixed, NfcReadOutcome::Read(v) if v.len() == 1));
    }

    #[test]
    fn size_limit_boundary() {
        assert!(!exceeds_limit(&[]));
        assert!(!exceeds_limit(&[MAX_NDEF_BYTES]));
        assert!(!exceeds_limit(&[MAX_NDEF_BYTES - 1, 1]));
        assert!(exceeds_limit(&[MAX_NDEF_BYTES, 1]));
        assert!(exceeds_limit(&[usize::MAX, 1]));
    }

    #[test]
    fn error_names_never_succeed() {
        assert_eq!(
            failure_from_error_name(Some("NotAllowedError")),
            F::PermissionDenied
        );
        assert_eq!(
            failure_from_error_name(Some("NotSupportedError")),
            F::Unsupported
        );
        assert_eq!(failure_from_error_name(Some("AbortError")), F::Interrupted);
        for n in [
            Some("NotReadableError"),
            Some("InvalidStateError"),
            Some("SecurityError"),
            Some(""),
            None,
        ] {
            assert_eq!(failure_from_error_name(n), F::Failed, "{n:?}");
        }
    }

    #[test]
    fn late_boundary() {
        assert!(!is_late(100.0, 30_099.0));
        assert!(is_late(100.0, 30_100.0));
        assert!(!is_late(100.0, f64::NAN));
    }

    #[test]
    fn to_core_maps_every_arm() {
        assert_eq!(to_core(Ok(NfcReadOutcome::EmptyTag)), Ok(ShownTag::Empty));
        let r = to_core(Ok(NfcReadOutcome::Read(vec![
            t("ja", "x"),
            NdefRecordView::Uri("u".into()),
            NdefRecordView::Mime {
                type_name: "m".into(),
                len: 1,
            },
            NdefRecordView::External {
                type_name: "e".into(),
                len: 2,
            },
            NdefRecordView::Other {
                type_name: "o".into(),
                len: 3,
            },
        ])));
        assert!(matches!(r, Ok(ShownTag::Records(v)) if v.len() == 5));
        for (f, c) in [
            (F::Unreadable, CoreFailure::Unreadable),
            (F::TooLarge, CoreFailure::TooLarge),
            (F::Disabled, CoreFailure::Disabled),
            (F::PermissionDenied, CoreFailure::PermissionDenied),
            (F::TimedOut, CoreFailure::TimedOut),
            (F::Interrupted, CoreFailure::Interrupted),
            (F::Unsupported, CoreFailure::Unsupported),
            (F::Failed, CoreFailure::Failed),
        ] {
            assert_eq!(to_core(Err(f)), Err(c));
        }
    }
}
