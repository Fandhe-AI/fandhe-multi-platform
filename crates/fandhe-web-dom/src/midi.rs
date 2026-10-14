//! 型サンプル「MIDI 入力」の Web 経路（親 #1137、Issue #1401）。
//!
//! `fandhe_core::screens::midi_show`（端末依存ゼロ）の画面を [`super::mount::mount_with_effect`] の
//! effect として駆動する。`take_command` で要求を観測し、Web MIDI API（`navigator.requestMIDIAccess`）
//! の接続中の入力へ `onmidimessage` を付けて、届いたバイト列を `fandhe_device::midi::MidiParser` で
//! ノートオン / オフにし、`apply_update` で書き戻す。`fandhe-device` の `MidiInput` trait は通らない。
//! `fandhe-egui-proxy` の wasm canvas 経路は対象外（従来どおり `Unsupported`）。
//!
//! # Reflect を使う理由
//!
//! `screen_wake_lock` / `file_watch` と同方針で `js_sys::Reflect` 経由で呼ぶ（web-sys の feature を
//! 足さない。ヘッドレス検証で `navigator.requestMIDIAccess` を偽物に差し替えても型変換で落ちない）。
//!
//! # 結果の種類
//!
//! - 拒否（`NotAllowedError` / `SecurityError`）= `PermissionDenied`、非対応（secure context 外・
//!   `requestMIDIAccess` 無し・`NotSupportedError`）= `Unsupported`、それ以外 = 固定文言の失敗
//! - 入力 0 台は成功（「接続中の MIDI 入力がありません」）。`open()` が全部失敗しても `Started(0, N)` で、
//!   0 台の成功とは別の表示になる
//! - 利用者の停止・`pagehide` は失敗ではなく `Idle`
//!
//! # 期限
//!
//! `requestMIDIAccess` は中断できず許可ダイアログを含むため `START_DEADLINE` は張らない
//! （camera-capture の Web と同じ）。代わりに demo は「停止」を常に出し、`Starting` 中でも取り消せる。
//! 遅れて resolve した結果は世代照合で捨て、何も登録しない。
//!
//! # 保留の上限
//!
//! 開始確定（`Started`）前に届いたノートは Session の `held` に最大 `MAX_SHOWN_EVENTS` 件保留する。
//! 超過分（古い方）は捨てて `dropped`（取りこぼし累計）へ加算する。開始確定後は保留せず直接届け、
//! 「消去」では保留分を捨てて `dropped` を `DroppedBase` で画面へ返す。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の順序と世代照合、失敗・非対応・拒否・`open()` 失敗を成功へ畳まないこと、
//! 停止・破棄・`pagehide`・開始失敗でハンドラ・ポート・リスナーを解放すること、停止後の通知を捨てること。
//! 保留したノートは `Started` 後に順序どおり反映する。「消去」では保留分も捨てる。
//! 守らない: 許可ダイアログ放置時の期限（停止で取り消せる）、Safari（非対応表示）、複数タブ・複数マウント
//! 間の一致、`open()` 確定直前の数 ms に届くノート、背景タブでの受信継続、抜き差し確定との時間差による
//! 件数表示の一時的な食い違い、`mount` が `Screen<S>` を作り直さないための開始ボタン固定
//! （demo に停止・消去を常に足して回避）。
//!
//! SysEx の許可は求めない（`sysex: false`）。演奏内容・ポート名・`id`・例外の `message` は画面にも
//! console にも出さない（security.md A09）。
//!
//! # 既知の天井
//!
//! ponytail: 1 件ごとに全再描画する。バーストで重ければ `requestAnimationFrame` でまとめる。

use fandhe_core::screens::midi_show::{MidiFailure, MidiNoteRow};
use fandhe_device::midi::{MidiParser, NoteEvent};

/// 開始失敗の固定文言（`fandhe-egui-proxy` のドライバと同じ）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const START_FAILED: &str = "MIDI の開始";

/// 入力ポート 1 つの `open()` の状況。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenState {
    Pending,
    Opened,
    Failed,
}

/// `requestMIDIAccess` の例外の `name` → 失敗種別。`message` は使わない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: &str) -> MidiFailure {
    match name {
        "NotAllowedError" | "SecurityError" => MidiFailure::PermissionDenied,
        "NotSupportedError" => MidiFailure::Unsupported,
        _ => MidiFailure::Failed(START_FAILED.to_string()),
    }
}

/// 既登録 id の入力を再登録（open 再試行）してよいか。`Failed` だけが対象で、確定待ち・成功済みは二重に付けない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn should_reattach(existing: Option<OpenState>) -> bool {
    matches!(existing, None | Some(OpenState::Failed))
}

/// 世代が古くなった `open()` の完了後、そのポートを `close()` すべきか。
/// JS の `MIDIInput` は同一インスタンスが再利用されるため、停止 → 再開始で新世代が同じポートを
/// 登録済みなら閉じない（閉じると新世代が無音のまま「受信中」になる）。誰も使っていない開いたポートだけ閉じる。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn should_close_abandoned(opened: bool, owned_by_live_session: bool) -> bool {
    opened && !owned_by_live_session
}

/// `(接続中か, open の状況)` の列から `(接続できた数, 接続できなかった数)`。未接続と確定待ちは数えない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn source_counts(ports: &[(bool, OpenState)]) -> (usize, usize) {
    let n = |s| ports.iter().filter(|(c, o)| *c && *o == s).count();
    (n(OpenState::Opened), n(OpenState::Failed))
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_row(e: &NoteEvent) -> MidiNoteRow {
    MidiNoteRow {
        on: e.on,
        channel: e.channel,
        note: e.note,
        velocity: e.velocity,
    }
}

/// 1 メッセージ分のバイト列から、完成したノートの行（古い順）を返す。CC・SysEx などは出ない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn rows_from_bytes(parser: &mut MidiParser, bytes: &[u8]) -> Vec<MidiNoteRow> {
    bytes
        .iter()
        .filter_map(|b| parser.feed(*b))
        .map(|e| to_row(&e))
        .collect()
}

#[cfg(target_arch = "wasm32")]
pub use web::midi_show_effect;

/// マウント破棄時にハンドラと発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::midi_show::{
        apply_update, mark_command_taken, stop, take_command, MidiCommand, MidiFailure,
        MidiNoteRow, MidiPhase, MidiShowState, MidiUpdate, MAX_SHOWN_EVENTS,
    };
    use fandhe_device::midi::MidiParser;
    use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::{spawn_local, JsFuture};

    use super::{
        failure_from_error_name, rows_from_bytes, should_close_abandoned, should_reattach,
        source_counts, OpenState,
    };
    use crate::mount::{is_live, Applier};

    type State = MidiShowState;
    type Cb = Closure<dyn FnMut(JsValue)>;

    /// 入力ポート 1 つ。`id` は内部の重複判定にだけ使い、表示もログ出力もしない。
    struct Port {
        id: String,
        input: JsValue,
        _on_message: Cb,
        open: OpenState,
    }

    /// マウントごとの受信 1 件。`teardown` で解除してから drop する。
    struct Session {
        generation: u64,
        access: JsValue,
        _on_state: Cb,
        on_hide: Cb,
        ports: Vec<Port>,
        /// 開始確定（`Started`）前か。真の間に届いたノートは `held` へ溜める。
        starting: bool,
        /// 開始確定前に届いたノート（古い順）。`Started` 後に順序どおり反映する。
        held: Vec<MidiNoteRow>,
        /// 保留上限を超えて捨てた累計件数（`Notes` の取りこぼし累計。接続ごとに 0 から数える）。
        dropped: u64,
    }

    thread_local! {
        /// 発送済みの世代（同じ要求を二重に始めない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        /// effect が最後に見た状態（世代, 開始中か受信中か）。await 再開後の照合に使う。
        static CURRENT: RefCell<HashMap<u64, (u64, bool)>> = RefCell::new(HashMap::new());
        /// effect が最後に見た `clear_epoch`。変化で保留ノートを捨てる。
        static EPOCH: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
        static SESSIONS: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
        /// 許可待ち（`requestMIDIAccess` 未確定）の pagehide リスナー（世代, リスナー）。
        /// 停止・差し替え・破棄で Promise の確定を待たずに解除する（契約表 3・9）。
        static PENDING_HIDE: RefCell<HashMap<u64, (u64, Cb)>> = RefCell::new(HashMap::new());
    }

    fn get(o: &JsValue, key: &str) -> JsValue {
        Reflect::get(o, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
    }

    fn set(o: &JsValue, key: &str, v: &JsValue) {
        let _ = Reflect::set(o, &JsValue::from_str(key), v);
    }

    fn call(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let f: Function = get(obj, method).dyn_into()?;
        let a = Array::new();
        for v in args {
            a.push(v);
        }
        f.apply(obj, &a)
    }

    async fn call_await(obj: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
        let p: Promise = call(obj, method, args)?.dyn_into()?;
        JsFuture::from(p).await
    }

    /// 要求がまだ現在（停止・差し替えされていない開始中 / 受信中）か。
    fn current(mount: u64, generation: u64) -> bool {
        is_live(mount) && CURRENT.with(|c| c.borrow().get(&mount) == Some(&(generation, true)))
    }

    fn still_mine(mount: u64, generation: u64) -> bool {
        is_live(mount)
            && SESSIONS.with(|s| {
                s.borrow()
                    .get(&mount)
                    .is_some_and(|x| x.generation == generation)
            })
    }

    fn remove_hide_listener(on_hide: &Cb) {
        if let Some(w) = web_sys::window() {
            let _ = call(
                &w,
                "removeEventListener",
                &[JsValue::from_str("pagehide"), on_hide.as_ref().clone()],
            );
        }
    }

    /// 許可待ちのリスナーを外して返す。`generation` を指定すると世代が一致するときだけ。
    fn take_pending(mount: u64, generation: Option<u64>) -> Option<Cb> {
        PENDING_HIDE.with(|p| {
            let mut p = p.borrow_mut();
            if generation.is_some_and(|g| p.get(&mount).map(|x| x.0) != Some(g)) {
                return None;
            }
            p.remove(&mount).map(|x| x.1)
        })
    }

    /// 許可待ちのリスナーを解除してから drop する。
    fn drop_pending(mount: u64) {
        if let Some(cb) = take_pending(mount, None) {
            remove_hide_listener(&cb);
        }
    }

    /// 解除を済ませてから Closure を drop する（契約表 9）。`close()` の結果は無視する。
    fn teardown(s: Session) {
        for p in &s.ports {
            set(&p.input, "onmidimessage", &JsValue::NULL);
            let _ = call(&p.input, "close", &[]);
        }
        set(&s.access, "onstatechange", &JsValue::NULL);
        remove_hide_listener(&s.on_hide);
        drop(s);
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に解放する。
    pub(crate) fn release_mount(mount: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount));
        CURRENT.with(|d| d.borrow_mut().remove(&mount));
        EPOCH.with(|d| d.borrow_mut().remove(&mount));
        drop_pending(mount);
        if let Some(s) = SESSIONS.with(|s| s.borrow_mut().remove(&mount)) {
            teardown(s);
        }
    }

    fn counts(mount: u64, generation: u64) -> (usize, usize) {
        SESSIONS.with(|s| {
            let s = s.borrow();
            let Some(x) = s.get(&mount).filter(|x| x.generation == generation) else {
                return (0, 0);
            };
            let ports: Vec<_> = x
                .ports
                .iter()
                .map(|p| {
                    (
                        get(&p.input, "state").as_string().as_deref() == Some("connected"),
                        p.open,
                    )
                })
                .collect();
            source_counts(&ports)
        })
    }

    fn inputs_of(access: &JsValue) -> Vec<JsValue> {
        let Ok(it) = call(&get(access, "inputs"), "values", &[]) else {
            return Vec::new();
        };
        Array::from(&it).iter().collect()
    }

    /// `access` の接続中の入力で、まだ登録していないものを付けて開く。確定したら件数を送る。
    async fn sync_inputs(applier: Applier<State>, mount: u64, generation: u64, notify: bool) {
        let access = SESSIONS.with(|s| {
            s.borrow()
                .get(&mount)
                .filter(|x| x.generation == generation)
                .map(|x| x.access.clone())
        });
        let Some(access) = access else { return };
        for input in inputs_of(&access) {
            if get(&input, "state").as_string().as_deref() == Some("connected") {
                attach(&applier, mount, generation, input).await;
            }
        }
        if notify && still_mine(mount, generation) {
            let (n, bad) = counts(mount, generation);
            applier.apply(move |s| apply_update(s, generation, MidiUpdate::Sources(n, bad)));
        }
    }

    /// 入力 1 つにハンドラを付けてから `open()` する（登録から確定までの通知を落とさない。契約表 1）。
    async fn attach(applier: &Applier<State>, mount: u64, generation: u64, input: JsValue) {
        let id = get(&input, "id").as_string().unwrap_or_default();
        let registered = SESSIONS.with(|s| {
            let mut s = s.borrow_mut();
            let Some(x) = s.get_mut(&mount).filter(|x| x.generation == generation) else {
                return false;
            };
            let existing = x.ports.iter().find(|p| p.id == id).map(|p| p.open);
            if !should_reattach(existing) {
                return false;
            }
            // open() に失敗した同 id の入力が復帰した。古い登録を外して付け直す。
            x.ports.retain(|p| p.id != id);
            let mut parser = MidiParser::default();
            let a = applier.clone();
            let cb: Cb = Closure::new(move |ev: JsValue| {
                let bytes = Uint8Array::new(&get(&ev, "data")).to_vec();
                let rows = rows_from_bytes(&mut parser, &bytes);
                if rows.is_empty() {
                    return;
                }
                // 開始確定前は画面が `Starting` でノートを捨てるため、確定後に反映する（契約表 1・2）。
                // 戻り値は届ける場合の取りこぼし累計。保留した場合は None。
                let live = SESSIONS.with(|s| {
                    let mut s = s.borrow_mut();
                    match s.get_mut(&mount).filter(|x| x.generation == generation) {
                        Some(x) if x.starting => {
                            x.held.extend(rows.iter().cloned());
                            // 上限を超えた古い分は一覧から外し、取りこぼしとして数える。
                            let over = x.held.len().saturating_sub(MAX_SHOWN_EVENTS);
                            x.held.drain(..over);
                            x.dropped += over as u64;
                            None
                        }
                        Some(x) => Some(x.dropped),
                        None => Some(0),
                    }
                });
                let Some(dropped) = live else {
                    return;
                };
                let a = a.clone();
                // 同期 apply は再描画 → 照合 → この Closure の drop に至りうるため逃がす。
                spawn_local(async move {
                    a.apply(move |s| apply_update(s, generation, MidiUpdate::Notes(rows, dropped)));
                });
            });
            set(&input, "onmidimessage", cb.as_ref());
            x.ports.push(Port {
                id: id.clone(),
                input: input.clone(),
                _on_message: cb,
                open: OpenState::Pending,
            });
            true
        });
        if !registered {
            return;
        }
        let ok = call_await(&input, "open", &[]).await.is_ok();
        if !still_mine(mount, generation) {
            // 待っている間に停止された。開いてしまったポートを残さない（契約表 3）。
            // ただし新世代が同じ MIDIInput を登録済みなら閉じない（契約表 2。所有判定は同一ブロック内）。
            let owned = SESSIONS.with(|s| {
                s.borrow()
                    .values()
                    .any(|x| x.ports.iter().any(|p| Object::is(&p.input, &input)))
            });
            if should_close_abandoned(ok, owned) {
                let _ = call(&input, "close", &[]);
            }
            return;
        }
        if !ok {
            set(&input, "onmidimessage", &JsValue::NULL);
        }
        SESSIONS.with(|s| {
            if let Some(p) = s
                .borrow_mut()
                .get_mut(&mount)
                .and_then(|x| x.ports.iter_mut().find(|p| p.id == id))
            {
                p.open = if ok {
                    OpenState::Opened
                } else {
                    OpenState::Failed
                };
            }
        });
    }

    fn fail(applier: &Applier<State>, generation: u64, f: MidiFailure) {
        applier.apply(move |s| apply_update(s, generation, MidiUpdate::Failed(f)));
    }

    async fn start_session(applier: Applier<State>, mount: u64, generation: u64) {
        let Some(window) = web_sys::window() else {
            return fail(&applier, generation, MidiFailure::Unsupported);
        };
        // 発送から実行までに停止・差し替えされていたら何も登録しない。
        if !current(mount, generation) {
            return;
        }
        // 許可待ちの間に pagehide が来ても世代を無効化できるよう、要求の発送前に登録する（契約表 1・2）。
        let a = applier.clone();
        let on_hide: Cb = Closure::new(move |_| {
            // 利用者の停止と同じ扱い。bfcache から戻ったら押し直す（契約表 1）。
            let a = a.clone();
            spawn_local(async move { a.apply(stop) });
        });
        let _ = call(
            &window,
            "addEventListener",
            &[JsValue::from_str("pagehide"), on_hide.as_ref().clone()],
        );
        // 登録表に置き、停止・再マウント・破棄で Promise を待たず解除できるようにする（契約表 9）。
        let prev = PENDING_HIDE.with(|p| p.borrow_mut().insert(mount, (generation, on_hide)));
        if let Some((_, prev)) = prev {
            remove_hide_listener(&prev);
        }
        let opts = Object::new();
        set(&opts, "sysex", &JsValue::FALSE);
        let access =
            match call_await(&window.navigator(), "requestMIDIAccess", &[opts.into()]).await {
                Ok(a) => a,
                Err(e) => {
                    if let Some(cb) = take_pending(mount, Some(generation)) {
                        remove_hide_listener(&cb);
                    }
                    let name = get(&e, "name").as_string().unwrap_or_default();
                    return fail(&applier, generation, failure_from_error_name(&name));
                }
            };
        // 許可ダイアログ中に停止・差し替え・破棄されていたら（リスナーは解除済み）何も登録しない（契約表 2）。
        let Some(on_hide) = take_pending(mount, Some(generation)) else {
            return;
        };
        if !current(mount, generation) {
            remove_hide_listener(&on_hide);
            return;
        }
        let a = applier.clone();
        let on_state: Cb = Closure::new(move |_| {
            let a = a.clone();
            spawn_local(sync_inputs(a, mount, generation, true));
        });
        set(&access, "onstatechange", on_state.as_ref());
        let old = SESSIONS.with(|s| {
            s.borrow_mut().insert(
                mount,
                Session {
                    generation,
                    access,
                    _on_state: on_state,
                    on_hide,
                    ports: Vec::new(),
                    starting: true,
                    held: Vec::new(),
                    dropped: 0,
                },
            )
        });
        if let Some(old) = old {
            teardown(old);
        }
        sync_inputs(applier.clone(), mount, generation, false).await;
        if !still_mine(mount, generation) {
            return;
        }
        let (n, bad) = counts(mount, generation);
        let (held, dropped) = SESSIONS.with(|s| {
            s.borrow_mut()
                .get_mut(&mount)
                .map(|x| {
                    x.starting = false;
                    (std::mem::take(&mut x.held), x.dropped)
                })
                .unwrap_or_default()
        });
        applier.apply(move |s| {
            apply_update(s, generation, MidiUpdate::Started(n, bad));
            if !held.is_empty() {
                apply_update(s, generation, MidiUpdate::Notes(held, dropped));
            }
        });
    }

    /// 機能検出。secure context と `requestMIDIAccess` が揃うときだけ真。ブラウザ名では分岐しない。
    fn supported() -> bool {
        let Some(w) = web_sys::window() else {
            return false;
        };
        get(&w, "isSecureContext").as_bool() == Some(true)
            && get(&w.navigator(), "requestMIDIAccess").is_function()
    }

    /// 画面の effect。照合 → 要求の発送の順に、要求を 1 回ずつ処理する。
    pub fn midi_show_effect(state: &State, applier: &Applier<State>) {
        let mount = applier.mount_id();
        let active = matches!(
            state.phase,
            MidiPhase::Starting | MidiPhase::Listening { .. }
        );
        CURRENT.with(|c| c.borrow_mut().insert(mount, (state.generation, active)));

        // 0. 「消去」。開始確定前の保留ノートも捨て、取りこぼし累計を基準として画面へ返す。
        let epoch_changed = EPOCH
            .with(|e| e.borrow_mut().insert(mount, state.clear_epoch))
            .is_some_and(|p| p != state.clear_epoch);
        if epoch_changed {
            let raw = SESSIONS.with(|s| {
                s.borrow_mut()
                    .get_mut(&mount)
                    .filter(|x| x.generation == state.generation)
                    .map(|x| {
                        x.held.clear();
                        x.dropped
                    })
            });
            if let Some(raw) = raw {
                let generation = state.generation;
                applier.apply(move |s| apply_update(s, generation, MidiUpdate::DroppedBase(raw)));
            }
        }

        // 1. 照合。停止・失敗・古い世代を 1 か所で吸収する。
        let stale = SESSIONS.with(|s| {
            let mut s = s.borrow_mut();
            let drop_it = s
                .get(&mount)
                .is_some_and(|x| !active || x.generation != state.generation);
            drop_it.then(|| s.remove(&mount)).flatten()
        });
        if let Some(s) = stale {
            teardown(s);
        }
        let pending_stale = PENDING_HIDE.with(|p| {
            p.borrow()
                .get(&mount)
                .is_some_and(|x| !active || x.0 != state.generation)
        });
        if pending_stale {
            drop_pending(mount);
        }

        // 2. 発送。Stop は照合で片付く。
        let Some((cmd, generation)) = take_command(state) else {
            return;
        };
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount, generation) != Some(generation));
        if !fresh {
            return;
        }
        applier.apply(move |s| mark_command_taken(s, generation));
        if cmd == MidiCommand::Start {
            if supported() {
                spawn_local(start_session(applier.clone(), mount, generation));
            } else {
                // 非対応は同期区間で確定する（effect 実行中の apply は退避される）。
                fail(applier, generation, MidiFailure::Unsupported);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use OpenState::*;

    #[test]
    fn only_failed_or_new_ports_reattach() {
        assert!(should_reattach(None));
        assert!(should_reattach(Some(Failed)));
        assert!(!should_reattach(Some(Pending)));
        assert!(!should_reattach(Some(Opened)));
    }

    #[test]
    fn stale_open_closes_only_unowned_opened_port() {
        assert!(should_close_abandoned(true, false));
        assert!(!should_close_abandoned(true, true));
        assert!(!should_close_abandoned(false, false));
    }

    #[test]
    fn error_name_maps_to_failure() {
        assert_eq!(
            failure_from_error_name("NotAllowedError"),
            MidiFailure::PermissionDenied
        );
        assert_eq!(
            failure_from_error_name("SecurityError"),
            MidiFailure::PermissionDenied
        );
        assert_eq!(
            failure_from_error_name("NotSupportedError"),
            MidiFailure::Unsupported
        );
        assert_eq!(
            failure_from_error_name("InvalidStateError"),
            MidiFailure::Failed("MIDI の開始".to_string())
        );
    }

    #[test]
    fn counts_skip_disconnected_and_pending() {
        assert_eq!(source_counts(&[]), (0, 0));
        assert_eq!(
            source_counts(&[
                (true, Opened),
                (false, Opened),
                (true, Pending),
                (true, Failed)
            ]),
            (1, 1)
        );
        // 全部失敗しても 0 台の成功とは区別できる（失敗数が立つ）。
        assert_eq!(source_counts(&[(true, Failed), (true, Failed)]), (0, 2));
    }

    #[test]
    fn rows_keep_only_notes() {
        let mut p = MidiParser::default();
        let mut rows = Vec::new();
        for msg in [
            &[0x90u8, 60, 100][..],
            &[0x90, 62, 0],         // ベロシティ 0 のオン = オフ
            &[0xB0, 7, 100],        // CC
            &[0xF0, 1, 2, 3, 0xF7], // SysEx
        ] {
            rows.extend(rows_from_bytes(&mut p, msg));
        }
        assert_eq!(rows.len(), 2);
        assert!(rows[0].on && rows[0].note == 60 && rows[0].channel == 1);
        assert!(!rows[1].on && rows[1].note == 62);
    }
}
