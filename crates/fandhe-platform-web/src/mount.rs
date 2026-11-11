//! マウントの生存管理と、状態の書き戻しハンドル [`Applier`]（`wasm32` 専用）。
//!
//! DOM レンダラ（`fandhe-web-dom`）が [`register_mount`] でマウントを登録し、`Applier::new` で
//! effect 用ハンドルを作る。機能（端末機能の Web 実装）は [`Applier::apply`] で結果を書き戻し、
//! [`is_live`] / [`root_of`] で破棄済みマウントへの書き込みを避ける。
//! 破棄の検知（同じ root への再マウント・root の DOM 離脱）時は、[`on_mount_release`] で
//! 登録された解放関数を呼ぶ。

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use web_sys::Element;

use crate::release_hooks;

/// 型消去した「再レンダリングを 1 回実行する」関数。イベントハンドラが
/// 状態更新後にこれを呼び出すことで全再構築の再描画をトリガーする。
pub type RenderTrigger = Rc<dyn Fn() -> Result<(), JsValue>>;

/// 再描画の直後に呼ばれる effect。状態を直接は変更できず、結果の書き戻しは
/// [`Applier::apply`] を通す（`fandhe-egui-proxy::app::EffectFn` と同じ形）。
pub type WebEffect<S> = fn(&S, &Applier<S>);

/// effect が状態を更新して再描画を起こすためのハンドル。
///
/// 非同期処理（`navigator.clipboard` の Promise 等）へ move できるよう [`Clone`] を
/// 実装する（`Rc` の複製のみで、状態は共有される）。
pub struct Applier<S> {
    state: Rc<RefCell<S>>,
    render: RenderTrigger,
    mount_id: u64,
}

impl<S> Clone for Applier<S> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            render: self.render.clone(),
            mount_id: self.mount_id,
        }
    }
}

impl<S: 'static> Applier<S> {
    /// レンダラが effect 用のハンドルを作る。`mount_id` は [`register_mount`] の戻り値。
    pub fn new(state: Rc<RefCell<S>>, render: RenderTrigger, mount_id: u64) -> Self {
        Self {
            state,
            render,
            mount_id,
        }
    }

    /// このハンドルが属するマウントの識別子（マウント呼び出しごとに一意）。
    ///
    /// effect は `fn` ポインタで自前の状態を持てないため、「この画面が処理中か」を
    /// effect 側で覚えたい場合はこの ID をキーにする。同じページへ同じ画面を 2 つ
    /// マウントしたときに、一方の飛行中状態がもう一方を止めてしまうのを防ぐ
    /// （codex-review 指摘 P1、PR #860）。
    pub fn mount_id(&self) -> u64 {
        self.mount_id
    }

    /// `update` で状態を書き換えてから再描画する。
    ///
    /// 再描画は `state` を不変借用するため、`update` の可変借用を先に落としてから
    /// 呼ぶ（同一スコープで重ねると `RefCell` が実行時 panic する）。再描画の失敗は
    /// ブラウザコンソールへ出すに留める（panic を API 契約にしない、coding-rust.md）。
    ///
    /// effect は `&S`（借用中の状態）を受け取るため、effect から**同期的に**本メソッドを
    /// 呼ぶと可変借用が取れない。その場合は panic させず、マイクロタスクへ退避して
    /// 借用が落ちた後に適用する（`try_borrow_mut` の失敗分岐）。これにより
    /// 「apply は非同期継続からのみ呼ぶ」という暗黙の契約を呼び出し側に強制せずに済む。
    pub fn apply(&self, update: impl FnOnce(&mut S) + 'static) {
        // 破棄済みマウント（同じ root へ再マウント・root の DOM 離脱）への書き戻しは捨てる。
        // 飛行中の非同期継続が新しい画面を旧画面で上書きしないため（契約表 1・2、PR #1105）。
        if !is_live(self.mount_id) {
            return;
        }
        let Ok(mut state) = self.state.try_borrow_mut() else {
            // effect 実行中（`render_once` が `state` を借用中）からの同期呼び出し。
            // マイクロタスクへ逃がすと借用が落ちた後に実行され、`try_borrow_mut` が通る。
            let deferred = self.clone();
            wasm_bindgen_futures::spawn_local(async move {
                deferred.apply(update);
            });
            return;
        };
        update(&mut state);
        drop(state);
        if let Err(e) = (self.render)() {
            web_sys::console::error_1(&e);
        }
    }

    /// `update` で状態を書き換え、DOM は**作り直さず**に `patch` で既存の要素へ直接反映する。
    ///
    /// 全再構築はボタンを含む要素を毎回作り直すため、mousedown と mouseup の間に挟まると
    /// `click` が発火しない。映像フレームのように高頻度で届く更新は、ボタン等を残したまま
    /// 一部の要素だけを書き換える（Issue #815）。`patch` は root 要素と更新後の状態を受け取り、
    /// 反映できたら `true` を返す。`false`（対象要素が無い等、構造が変わる更新）のときは
    /// [`Applier::apply`] と同じ全再構築へ倒す。effect は呼ばない（要求の観測が要る更新には使わない）。
    pub fn apply_in_place(
        &self,
        update: impl FnOnce(&mut S) + 'static,
        patch: impl FnOnce(&Element, &S) -> bool + 'static,
    ) {
        if !is_live(self.mount_id) {
            return;
        }
        let Ok(mut state) = self.state.try_borrow_mut() else {
            let deferred = self.clone();
            wasm_bindgen_futures::spawn_local(async move {
                deferred.apply_in_place(update, patch);
            });
            return;
        };
        update(&mut state);
        let patched = match root_of(self.mount_id) {
            Some(root) => patch(&root, &state),
            None => false,
        };
        drop(state);
        if patched {
            return;
        }
        if let Err(e) = (self.render)() {
            web_sys::console::error_1(&e);
        }
    }
}

thread_local! {
    /// これまでにマウントした `(mount_id, root)`。
    static MOUNTS: RefCell<Vec<(u64, Element)>> = const { RefCell::new(Vec::new()) };
    /// DOM 離脱の監視（`MutationObserver`）を張ったか。
    static OBSERVING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// マウントごとに一意な ID の採番元。wasm32 はシングルスレッドのため `Cell` で足りる。
    static NEXT_MOUNT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// 破棄された `mount_id` ごとに呼ぶ解放関数を登録する（破棄時の資源解放、契約表 1・9）。
///
/// 同じ `key`（機能の id、`fandhe-cap-` を除いた名前）の再登録は無視するため、機能は effect の
/// 入口で毎回呼んでよい。レンダラのマウントより前に呼ぶこと。呼ばれる順は登録順。
pub fn on_mount_release(key: &'static str, release: fn(u64)) {
    release_hooks::register(key, release);
}

/// レンダラ用。マウントの ID を採番し、同じ `root` への再マウントで破棄される旧マウントを解放し、
/// 新マウントを登録して DOM 離脱の監視を始める。戻り値の ID を [`Applier::new`] に渡す。
pub fn register_mount(root: &Element) -> u64 {
    let mount_id = NEXT_MOUNT_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    });
    release_replaced_mounts(root, mount_id);
    mount_id
}

/// 破棄済みマウント（同じ `root` への再マウント・`root` の DOM 離脱）の資源を解放する。
/// `new` があれば新マウントとして登録する。マウント破棄 API が無いため、再マウント時と、
/// `document` 全体の `MutationObserver` 経由（次のマウントが無くても離脱を検知する）で呼ぶ。
fn sweep_mounts(new: Option<(u64, &Element)>) {
    let gone: Vec<u64> = MOUNTS.with(|m| {
        let mut m = m.borrow_mut();
        let mut gone = Vec::new();
        m.retain(|(id, el)| {
            let dead =
                new.is_some_and(|(_, root)| el.is_same_node(Some(root))) || !el.is_connected();
            if dead {
                gone.push(*id);
            }
            !dead
        });
        if let Some((id, root)) = new {
            m.push((id, root.clone()));
        }
        gone
    });
    for id in gone {
        release_hooks::run(id);
    }
}

/// `mount_id` のマウントがまだ有効か（破棄されていないか）。
///
/// 非同期継続が結果を反映・資源を確保する直前に確かめる。`root` は document に
/// 接続済みであることが前提（未接続の root は DOM 変化のたびに破棄扱いになる）。
pub fn is_live(mount_id: u64) -> bool {
    MOUNTS.with(|m| m.borrow().iter().any(|(id, _)| *id == mount_id))
}

/// マウント先の要素（WebXR の `dom-overlay` の root）。破棄済みなら `None`。
pub fn root_of(mount_id: u64) -> Option<Element> {
    MOUNTS.with(|m| {
        m.borrow()
            .iter()
            .find(|(id, _)| *id == mount_id)
            .map(|(_, el)| el.clone())
    })
}

fn release_replaced_mounts(root: &Element, new_id: u64) {
    sweep_mounts(Some((new_id, root)));
    if OBSERVING.with(|o| o.replace(true)) {
        return;
    }
    let Some(target) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        OBSERVING.with(|o| o.set(false));
        return;
    };
    let callback = Closure::wrap(
        Box::new(move |_: js_sys::Array, _: web_sys::MutationObserver| {
            sweep_mounts(None);
        }) as Box<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>,
    );
    let observer =
        web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()).and_then(|o| {
            let init = web_sys::MutationObserverInit::new();
            init.set_child_list(true);
            init.set_subtree(true);
            o.observe_with_options(&target, &init).map(|()| o)
        });
    if let Ok(observer) = observer {
        // 監視はページ寿命と同じ。observer 自身も手放すと JS 側で回収されて監視が止まる
        // ため、callback と合わせて 1 度だけ意図的にリークする（リークは 1 組で済む）。
        std::mem::forget(observer);
        callback.forget();
    } else {
        OBSERVING.with(|o| o.set(false));
    }
}
