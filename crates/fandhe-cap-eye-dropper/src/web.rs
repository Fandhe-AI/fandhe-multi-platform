//! 型サンプル「画面のスポイト」の Web 経路（Issue #763、親 #148。#1359 で `fandhe-web-dom` から機能 crate へ移した）。
//!
//! 画面の状態型には依存しない（`consumer-app-structure.md` §8.1。画面の状態を扱う effect 本体は
//! `driver::eye_dropper_pick_effect` で、本モジュールの [`start_screen`] / [`register_release`] を呼ぶ）。
//! 2 つの入口が同じ `new EyeDropper().open({ signal })` の呼び出し（[`start`]）を使う。
//! - 画面側 [`start_screen`]: マウント（`mount_id`）と世代ごとに要求を 1 件飛ばし、結果をコールバックで返す。
//! - 公開入口 [`begin_public`] / [`abort_public`]: `driver` の `begin_eye_dropper` / `discard_eye_dropper` から呼ばれる。
//!   画面の状態型に依存せず、結果は `driver::bridge::SLOT` へ置く（取り出しは `poll_eye_dropper`）。
//!
//! 画面 effect は slot を「進行中の印」としてだけ使い、結果は slot に置かず `finish_pick` で直接画面へ返す。
//!
//! 契約（結果の 4 値・`sRGBHex` の検査・例外名の写像）は `fandhe_device::eye_dropper` が SSOT で、
//! 写像は `web_result` に置く。macOS 実装と同じ slot・持ち主の印で、画面と公開入口が互いの要求を置き換えない。
//!
//! # ブラウザ側の制約
//!
//! - secure context 限定。`open()` はユーザー操作（transient activation）の中で呼ぶ必要があるため、
//!   同期区間で呼び、`open()` の前に await を挟まない
//! - デスクトップの Chrome / Edge 95+ のみ。コンストラクタが無ければ `Unsupported`
//!   （対応の有無を前もって問い合わせず、呼んだ結果で判定する）
//!
//! # 結果の写像
//!
//! - resolve: `sRGBHex` を `color_from_srgb_hex` で検査（trim しない）。不正は失敗
//! - reject: `AbortError`（Esc・signal による中断）だけが `Cancelled`、他は失敗。
//!   例外の `name` だけを使い、`message` は読まない
//! - 期限は持たない。サンプラーを閉じるのは利用者の Esc と `AbortSignal` だけ
//!
//! # ライフサイクル
//!
//! 要求ごとに `AbortController` を作る。マウント破棄（`release_mount`）・`pagehide`（bfcache 退避を含む）・
//! 公開入口の discard で `abort()` し、`AbortError` 経由で通常どおり確定させる。slot・印は確定（`settle`）まで残し、次の開始を busy で断る。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 取り消し・非対応・不正値・失敗を成功に見せないこと、世代照合、発送記録と
//! `AbortController` の解放、拾った色と例外の `message` をログ・永続化に出さないこと。
//! 守らない: Chromium 以外（非対応表示）、ブラウザ・OS ごとのサンプリング位置や色管理の差、
//! 複数タブの同時操作。

use std::cell::RefCell;
use std::collections::HashMap;

use fandhe_device::eye_dropper::PickedColor;
use fandhe_platform_web::on_mount_release;
use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::prelude::*;
use web_sys::AbortController;

use crate::driver::bridge::{PUBLIC_OWNER, SCREEN_OWNER, SLOT};
use crate::error::EyeDropperError;
use crate::web_result::{error_from_name, result_from_resolved};

/// 画面 effect の飛行中の要求。結果の確定前に必ず解放する。
struct Entry {
    generation: u64,
    /// `SLOT` の要求 id（登録前は `0`）。
    request_id: u64,
    ctrl: Option<AbortController>,
}

type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

thread_local! {
    static PICKS: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    /// 公開入口の飛行中の要求（要求 id と中断用のコントローラ）。
    static PUBLIC: RefCell<Option<(u64, AbortController)>> = const { RefCell::new(None) };
    static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
}

/// マウント破棄時（`fandhe_platform_web::on_mount_release` 経由）に中断し、記録を解放する。
fn release_mount(mount_id: u64) {
    if let Some(e) = PICKS.with(|p| p.borrow_mut().remove(&mount_id)) {
        if let Some(c) = e.ctrl {
            // slot と印は残す。`AbortError` の確定（`settle`）が空けるまで次の開始は busy で断られ、
            // ブラウザの EyeDropper が閉じきる前の `open()`（`InvalidStateError`）を避ける。
            c.abort();
        } else {
            // 中断対象が無い（`open()` 前）。確定は来ないのでここで空ける。
            SLOT.clear(e.request_id);
            SCREEN_OWNER.release(e.request_id);
        }
    }
}

/// 全要求を中断する（`pagehide`）。記録は残し、`AbortError` による通常の確定に任せる。
fn abort_all() {
    let mut ctrls: Vec<AbortController> =
        PICKS.with(|p| p.borrow().values().filter_map(|e| e.ctrl.clone()).collect());
    ctrls.extend(PUBLIC.with(|p| p.borrow().as_ref().map(|(_, c)| c.clone())));
    for c in ctrls {
        c.abort();
    }
}

fn register_pagehide_once(w: &web_sys::Window) {
    PAGEHIDE.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(|_e: web_sys::Event| abort_all());
        if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
            .is_ok()
        {
            *cell.borrow_mut() = Some(closure);
        }
    });
}

fn get(target: &JsValue, name: &str) -> JsValue {
    Reflect::get(target, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn name_of(e: &JsValue) -> Option<String> {
    get(e, "name").as_string()
}

/// `new EyeDropper()` と `open({ signal })` を同期で呼んで Promise を得る。
/// 失敗は確定済みの失敗として返す。
fn start(ctrl: &AbortController) -> Result<js_sys::Promise, EyeDropperError> {
    let window = web_sys::window().ok_or(EyeDropperError::Unsupported)?;
    let ctor = get(&window.into(), "EyeDropper");
    if !ctor.is_function() {
        return Err(EyeDropperError::Unsupported);
    }
    let ctor: Function = ctor.unchecked_into();
    let dropper = Reflect::construct(&ctor, &Array::new())
        .map_err(|e| error_from_name(name_of(&e).as_deref()))?;
    let opts = Object::new();
    let failed = || error_from_name(None);
    Reflect::set(&opts, &"signal".into(), &ctrl.signal()).map_err(|_| failed())?;
    let open = get(&dropper, "open");
    if !open.is_function() {
        return Err(EyeDropperError::Unsupported);
    }
    match open.unchecked_into::<Function>().call1(&dropper, &opts) {
        Ok(p) => p.dyn_into::<js_sys::Promise>().map_err(|_| failed()),
        Err(e) => Err(error_from_name(name_of(&e).as_deref())),
    }
}

/// Promise の完了を結果へ写す。
async fn await_result(promise: js_sys::Promise) -> Result<PickedColor, EyeDropperError> {
    match wasm_bindgen_futures::JsFuture::from(promise).await {
        Ok(v) => result_from_resolved(get(&v, "sRGBHex").as_string().as_deref()),
        Err(e) => Err(error_from_name(name_of(&e).as_deref())),
    }
}

/// 世代が一致する記録だけを取り除く（新しい世代の記録を巻き込まない）。
fn clear_entry(mount_id: u64, generation: u64) {
    PICKS.with(|p| {
        let mut map = p.borrow_mut();
        if map
            .get(&mount_id)
            .is_some_and(|e| e.generation == generation)
        {
            map.remove(&mount_id);
        }
    });
}

/// 記録・slot・印を片付けてから結果を届ける（届けた後で片付ける順序にしない）。
fn settle(
    mount_id: u64,
    generation: u64,
    request_id: u64,
    result: Result<PickedColor, EyeDropperError>,
    on_done: impl FnOnce(Result<PickedColor, EyeDropperError>),
) {
    clear_entry(mount_id, generation);
    SLOT.clear(request_id);
    SCREEN_OWNER.release(request_id);
    on_done(result);
}

/// マウント破棄時の解放を登録する（同じ key の再登録は無視される）。`driver` の effect が毎回呼ぶ。
pub(crate) fn register_release() {
    on_mount_release("eye_dropper", release_mount);
}

/// 画面側の要求を 1 件飛ばす。`mount_id` と `generation` ごとに 1 回だけ `open()` する（同じ世代の再呼び出しは何もしない）。
///
/// 古い世代が残っていれば中断して差し替える（`open()` 開始済みなら slot と印は確定まで残す）。
/// `on_picking` は要求を受け付けた直後（結果の確定より前）に 1 度呼ぶ。`on_done` は確定のとき 1 度だけ呼ぶ。
/// どちらも画面の状態型を知らない呼び出し側（`driver`）が、状態への反映に使う。
pub(crate) fn start_screen(
    mount_id: u64,
    generation: u64,
    on_picking: impl FnOnce(),
    on_done: impl FnOnce(Result<PickedColor, EyeDropperError>) + 'static,
) {
    let already = PICKS.with(|p| {
        p.borrow()
            .get(&mount_id)
            .is_some_and(|e| e.generation == generation)
    });
    if already {
        return;
    }
    let previous = PICKS.with(|p| {
        p.borrow_mut().insert(
            mount_id,
            Entry {
                generation,
                request_id: 0,
                ctrl: None,
            },
        )
    });
    if let Some(prev) = previous {
        if let Some(c) = prev.ctrl {
            // slot と印は残す（`release_mount` と同じ）。`AbortError` の確定（`settle`）まで、
            // 新しい要求は下の `begin_excluding` が busy で断る。
            c.abort();
        } else {
            SLOT.clear(prev.request_id);
            SCREEN_OWNER.release(prev.request_id);
        }
    }
    on_picking();

    // 公開入口の進行中・未取得の要求は置き換えず、画面側を処理中の失敗で終える。
    let Some(request_id) = SCREEN_OWNER.begin_excluding(&SLOT, &[&PUBLIC_OWNER]) else {
        return settle(
            mount_id,
            generation,
            0,
            Err(EyeDropperError::Internal("eye_dropper busy")),
            on_done,
        );
    };
    let Ok(ctrl) = AbortController::new() else {
        return settle(
            mount_id,
            generation,
            request_id,
            Err(error_from_name(None)),
            on_done,
        );
    };
    PICKS.with(|p| {
        if let Some(e) = p.borrow_mut().get_mut(&mount_id)
            && e.generation == generation
        {
            e.request_id = request_id;
            e.ctrl = Some(ctrl.clone());
        }
    });
    if let Some(w) = web_sys::window() {
        register_pagehide_once(&w);
    }
    // クリックからの同期チェーンで呼ぶ（この前に await を挟まない）。
    let promise = match start(&ctrl) {
        Ok(p) => p,
        Err(e) => return settle(mount_id, generation, request_id, Err(e), on_done),
    };
    wasm_bindgen_futures::spawn_local(async move {
        let result = await_result(promise).await;
        settle(mount_id, generation, request_id, result, on_done);
    });
}

/// 公開入口の開始（`driver::begin_eye_dropper` の Web 実装）。画面 effect と同じく、利用者の操作の中から同期で呼ぶ。
/// 失敗は slot・印を空けて `Err` で返す（結果は届かない）。
pub(crate) fn begin_public() -> Result<u64, EyeDropperError> {
    let request_id = PUBLIC_OWNER
        .begin_excluding(&SLOT, &[&SCREEN_OWNER, &PUBLIC_OWNER])
        .ok_or(EyeDropperError::Internal("eye_dropper busy"))?;
    let abandon = |e: EyeDropperError| {
        SLOT.clear(request_id);
        PUBLIC_OWNER.release(request_id);
        e
    };
    let ctrl = AbortController::new().map_err(|_| abandon(error_from_name(None)))?;
    if let Some(w) = web_sys::window() {
        register_pagehide_once(&w);
    }
    let promise = start(&ctrl).map_err(abandon)?;
    PUBLIC.with(|p| *p.borrow_mut() = Some((request_id, ctrl)));
    wasm_bindgen_futures::spawn_local(async move {
        let result = await_result(promise).await;
        // 記録（コントローラ）を片付けてから結果を届ける。
        PUBLIC.with(|p| {
            let mut cell = p.borrow_mut();
            if cell.as_ref().is_some_and(|(id, _)| *id == request_id) {
                *cell = None;
            }
        });
        // 持ち主が外れていても（discard 済み）slot は届いた結果で確定し、次の開始が置き換える。
        SLOT.deliver(request_id, result);
    });
    Ok(request_id)
}

/// 公開入口の進行中の要求を中断する（`driver::discard_eye_dropper` から）。`AbortError` が `Cancelled` として確定する。
pub(crate) fn abort_public(request_id: u64) {
    let ctrl = PUBLIC.with(|p| {
        p.borrow()
            .as_ref()
            .filter(|(id, _)| *id == request_id)
            .map(|(_, c)| c.clone())
    });
    if let Some(c) = ctrl {
        c.abort();
    }
}
