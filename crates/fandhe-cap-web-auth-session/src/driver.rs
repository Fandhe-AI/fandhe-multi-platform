//! Phase 11（親 #113）型サンプル「OAuth 用の Web 認証セッション」（Issue #530）の effect ドライバ。
//! `fandhe_core::screens::web_auth_session_demo`（端末依存ゼロ）の要求を観測し、デモ認可サーバー
//! （`crate::contract::DemoAuthServer`）を起こして OS の認証 UI を開き、返った
//! コールバック URL を検査して `CompletionSender` 経由で画面へ返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `crate::contract` のモジュール doc を参照。
//!
//! # スレッド
//!
//! OS の認証 UI は非同期に完了を通知する。結果は pull 型 [`bridge`]（`RequestSlot` の facade）へ
//! 置き、毎フレームの effect が拾う。OS からの唯一の受け口は [`bridge::complete_from_os`]。
//! 期限・取り消し・終了はすべて「進行中の 1 件」（[`bridge`] の `ACTIVE`）の判定と更新を同じロックの
//! 内側で行い、ロックを外してから OS のセッションを閉じ、サーバーを止める。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_web_auth_session`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_web_auth_session`（#530）、Windows: `windows_web_auth_session`（#531）、
//! iOS: `ios_web_auth_session`（#532）、Android: `android_web_auth_session`（#534））。
//! それ以外（Linux 等）は `UnsupportedWebAuthSession` 固定で、ダミーの成功実装へは倒さない。

use std::sync::Arc;

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use crate::contract::UnsupportedWebAuthSession;
use crate::contract::{
    AuthRequest, CallbackKind, SESSION_TIMEOUT, WebAuthFailure, WebAuthOutcome, WebAuthSession,
};
use fandhe_core::screens::web_auth_session_demo::{
    CoreFailure, CoreOutcome, Phase, ReturnedKind, WebAuthSessionDemoState, finish, mark_running,
    take_cancel, take_request,
};

use fandhe_runtime::CompletionSender;
use fandhe_runtime::request_slot::{SlotOwner, poll_and_finish_owned};

/// 現在のプラットフォームに応じた `WebAuthSession` 実装を返す（プロセスで 1 個）。
pub fn platform_web_auth_session() -> Arc<dyn WebAuthSession> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn WebAuthSession>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                crate::macos::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                crate::ios::implementation()
            }
            #[cfg(target_os = "android")]
            {
                crate::android::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                crate::windows::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "windows"
            )))]
            {
                Arc::new(UnsupportedWebAuthSession)
            }
        })
        .clone()
}

fn to_failure(f: WebAuthFailure) -> CoreFailure {
    match f {
        WebAuthFailure::Unsupported => CoreFailure::Unsupported,
        WebAuthFailure::NoBrowser => CoreFailure::NoBrowser,
        WebAuthFailure::InvalidCallback => CoreFailure::InvalidCallback,
        WebAuthFailure::Internal => CoreFailure::Internal,
    }
}

/// デバイス側の結果を画面用へ写す。URL は伏せ字だけを渡す（security.md A09）。
fn to_core(r: Result<WebAuthOutcome, WebAuthFailure>) -> Result<CoreOutcome, CoreFailure> {
    match r {
        Ok(WebAuthOutcome::Returned(cb)) => Ok(CoreOutcome::Returned {
            masked_url: cb.masked(),
            kind: match &cb.kind {
                CallbackKind::Code { len } => ReturnedKind::Code { len: *len },
                CallbackKind::Error { error } => ReturnedKind::Denied {
                    error: error.clone(),
                },
            },
        }),
        Ok(WebAuthOutcome::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(WebAuthOutcome::TimedOut) => Ok(CoreOutcome::TimedOut),
        Err(f) => Err(to_failure(f)),
    }
}

/// 進行中の 1 件と OS からの結果の橋渡し（pull 型、`RequestSlot` の facade）。
/// 他機能とは共有しない本機能専用の [`SLOT`](bridge::SLOT)。
pub(crate) mod bridge {
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    use crate::contract::{
        AuthRequest, DemoAuthServer, SESSION_TIMEOUT, WebAuthFailure, WebAuthOutcome,
        WebAuthSession, verify_callback,
    };

    use fandhe_runtime::request_slot::RequestSlot;

    /// OS の生の結果。画面用への写像（`to_core`）は取り出す側が行う
    /// （画面経由でも公開入口 `poll_web_auth` でも同じ slot を使う）。
    pub(crate) type Outcome = Result<WebAuthOutcome, WebAuthFailure>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// OS の認証 UI が返したもの。
    #[allow(dead_code)] // `Cancelled` は macOS / iOS の実装だけが作る。
    pub(crate) enum RawResult {
        /// コールバック URL（未検査）。
        Url(String),
        /// 利用者が UI を閉じた。
        Cancelled,
    }

    pub(crate) struct Active {
        pub(crate) request_id: u64,
        pub(crate) started: Instant,
        pub(crate) req: AuthRequest,
        pub(crate) server: Option<DemoAuthServer>,
    }

    static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

    pub(crate) fn lock() -> MutexGuard<'static, Option<Active>> {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 進行中の要求が `request_id`（`None` なら任意）に一致し、`decide` が結果を決めたら、
    /// 同じロックの内側で結果を確定して `ACTIVE` から外す。ロックを外してから OS のセッションを
    /// 閉じ（`os` が `Some` のとき）、サーバーを止める（期限・取り消しで戻る前に解除まで済ませる）。
    pub(crate) fn conclude_with(
        request_id: Option<u64>,
        decide: impl FnOnce(&Active) -> Option<Result<WebAuthOutcome, WebAuthFailure>>,
        os: Option<&dyn WebAuthSession>,
    ) {
        let taken = {
            let mut g = lock();
            let decided = g
                .as_ref()
                .filter(|a| request_id.is_none_or(|id| a.request_id == id))
                .and_then(decide);
            decided.and_then(|r| {
                let a = g.take()?;
                SLOT.deliver(a.request_id, r);
                Some(a)
            })
        };
        if let Some(mut a) = taken {
            if let Some(os) = os {
                os.cancel(a.request_id);
            }
            // 呼び出しスレッド（UI スレッドを含む）を待ち受けスレッドの終了待ちで止めない。
            if let Some(s) = a.server.as_mut() {
                s.stop_async();
            }
        }
    }

    /// OS 層（macOS / iOS / Android の完了）の受け口。Windows のループバックは
    /// [`complete_loopback`] を使う。現在の要求でない `request_id`・取り消し／期限で確定済みの要求の完了は捨てる。
    // Windows・Linux はループバックの受け口だけを使う。
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "ios", target_os = "android")),
        allow(dead_code)
    )]
    pub(crate) fn complete_from_os(request_id: u64, result: Result<RawResult, WebAuthFailure>) {
        conclude_with(
            Some(request_id),
            |a| {
                Some(if a.started.elapsed() >= SESSION_TIMEOUT {
                    // 期限後に届いた結果は成功として受けない。
                    Ok(WebAuthOutcome::TimedOut)
                } else {
                    match result {
                        Ok(RawResult::Url(u)) => {
                            verify_callback(&u, &a.req).map(WebAuthOutcome::Returned)
                        }
                        Ok(RawResult::Cancelled) => Ok(WebAuthOutcome::Cancelled),
                        Err(f) => Err(f),
                    }
                })
            },
            None,
        );
    }

    /// Windows のループバック（`DemoAuthServer` のスレッド）の受け口。`verify_callback` を通った
    /// URL だけで確定する。通らないもの・現在の要求でないもの・確定済みのものは `false`
    /// （サーバーが 404 を返して待ち続ける）。判定と確定は同じロックの内側で行う。
    pub(crate) fn complete_loopback(request_id: u64, url: String) -> bool {
        let mut accepted = false;
        conclude_with(
            Some(request_id),
            |a| {
                let cb = verify_callback(&url, &a.req).ok()?;
                accepted = true;
                Some(Ok(if a.started.elapsed() >= SESSION_TIMEOUT {
                    WebAuthOutcome::TimedOut
                } else {
                    WebAuthOutcome::Returned(cb)
                }))
            },
            None,
        );
        accepted
    }

    /// 終了時の後片付け。進行中の 1 件があれば取り消し、OS のセッションとサーバーを閉じる。
    pub(crate) fn shutdown(os: &dyn WebAuthSession) {
        conclude_with(None, |_| Some(Ok(WebAuthOutcome::Cancelled)), Some(os));
    }
}

/// 終了時（`fandhe` の入口の終了処理（`src/entry/lifecycle.rs` の `shutdown`））に進行中の認証を閉じる。
pub fn shutdown() {
    if bridge::lock().is_some() {
        bridge::shutdown(&*platform_web_auth_session());
    }
}

/// 要求 1 件ぶんの準備（乱数・PKCE・デモ認可サーバー）。進行中の 1 件として登録する値を返す。
fn setup(imp: &dyn WebAuthSession, request_id: u64) -> Result<bridge::Active, WebAuthFailure> {
    use crate::contract::{
        AuthRequest, CALLBACK_PATH, DemoAuthServer, Pkce, Redirect, RedirectKind, State,
    };
    use fandhe_device::passkey::base64url_encode;
    use fandhe_device::url_scheme::SCHEME;

    let started = std::time::Instant::now();
    let (mut state, mut verifier, mut code) = ([0u8; 16], [0u8; 32], [0u8; 16]);
    imp.fill_random(&mut state)?;
    imp.fill_random(&mut verifier)?;
    imp.fill_random(&mut code)?;
    let state = State::from_bytes(state);
    let pkce = Pkce::from_random(&verifier, |d| imp.sha256(d))?;
    let kind = imp.redirect_kind();
    let custom = format!("{SCHEME}://{CALLBACK_PATH}");
    let tx: Option<crate::contract::LoopbackTx> = (kind == RedirectKind::Loopback)
        .then(|| Box::new(move |u: String| bridge::complete_loopback(request_id, u)) as _);
    let server = DemoAuthServer::start(
        &state.to_hex(),
        &pkce.challenge,
        &base64url_encode(&code),
        (kind == RedirectKind::CustomScheme).then_some(custom.as_str()),
        tx,
    )?;
    let redirect = match kind {
        RedirectKind::CustomScheme => Redirect::CustomScheme {
            scheme: SCHEME.to_string(),
            path: CALLBACK_PATH.to_string(),
        },
        RedirectKind::Loopback => Redirect::Loopback {
            port: server.port(),
        },
    };
    Ok(bridge::Active {
        request_id,
        started,
        req: AuthRequest {
            start_url: server.authorize_url(),
            redirect,
            state,
        },
        server: Some(server),
    })
}

/// 画面（`web_auth_session_demo`）の要求の持ち主。公開入口の要求（[`EXTERNAL_OWNER`]）と 1 つの `SLOT` を分け合い、
/// 互いの進行中の要求を置き換えず、相手の結果を取らない。
static SCREEN_OWNER: SlotOwner = SlotOwner::new();
/// 公開入口（[`begin_web_auth`]）の要求の持ち主。
static EXTERNAL_OWNER: SlotOwner = SlotOwner::new();

/// 画面の要求 1 件を打ち切る（OS のセッションとサーバーを閉じ、結果の置き場を空にする）。
///
/// `imp` が `None` のときは OS のセッションを閉じない（Activity の破棄中に OS 層へ戻らないため。Java が自分で閉じる）。
/// 同じ要求を何度呼んでも 1 度だけ閉じる（`conclude_with` が `ACTIVE` を取り出した側だけが後片付けをする）。
fn abandon_screen_request(imp: Option<&dyn WebAuthSession>, request_id: u64) {
    bridge::conclude_with(
        Some(request_id),
        |_| Some(Ok(WebAuthOutcome::Cancelled)),
        imp,
    );
    let _ = bridge::SLOT.take_ready(request_id);
    SCREEN_OWNER.release(request_id);
}

/// Activity の破棄（機能 crate 自身のライフサイクル、Java の `onHostDestroyed`）で呼ぶ。画面の要求は画面状態ごと失われるので、
/// 表示中の画面に関係なく画面の要求（`SCREEN_OWNER`）だけを回収する（待ち受け・slot・印）。公開入口（`EXTERNAL_OWNER`）の要求は
/// 引き継ぎ型のまま触らない。回収した要求の id を返す（Java が自分の追跡を外す）。`drive` 内の回収と重なっても 1 度だけ閉じる。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn reclaim_screen_request() -> Option<u64> {
    let mine = SCREEN_OWNER.current();
    if mine == 0 {
        return None;
    }
    abandon_screen_request(None, mine);
    Some(mine)
}

/// 毎フレームの処理（テストから任意の実装を差し込めるよう切り出す）。
///
/// 描画層が毎フレーム完了を反映してから effect を呼ぶ順序（`fandhe-runtime` の `Renderer` の doc）を前提にする。
///
/// 画面の状態は Activity の再生成で `default()` へ戻るが、`SLOT`・`ACTIVE` は static のため残る。呼び出し側の掃除
/// （`reset_pending_tracking` 等）には頼らず、新しい要求を受けたとき前の画面状態の要求を打ち切って引き継がない
/// （結果は新しい画面状態へは届けない。公開入口の要求には触れない）。
fn drive(
    state: &WebAuthSessionDemoState,
    sender: &CompletionSender<WebAuthSessionDemoState>,
    imp: &dyn WebAuthSession,
) {
    if take_request(state).is_some() {
        let mine = SCREEN_OWNER.current();
        if mine != 0 {
            // effect は毎フレームの先頭で完了（`mark_running`）を反映した後に呼ばれる（`fandhe-render-egui` の `app.rs`）。
            // よって `Requested` を見た時点で前フレームの発送は反映済みで、自分の持ち主の印が残っていれば
            // 前の画面状態（Activity 再生成で `default()` へ戻る前）の要求。打ち切って発送し直す。
            abandon_screen_request(Some(imp), mine);
        }
        // 公開入口の進行中・未取得の要求があるあいだは始めない（置き換えない）。終わるまで毎フレーム見直す。
        let Some(request_id) = SCREEN_OWNER.begin_excluding(&bridge::SLOT, &[&EXTERNAL_OWNER])
        else {
            sender.request_repaint_after(fandhe_runtime::request_slot::POLL_INTERVAL);
            return;
        };
        let _ = sender.send(Box::new(mark_running));
        match setup(imp, request_id) {
            // 準備に失敗した要求は登録前なので、結果だけを置く（サーバーは `Drop` で止まる）。
            Err(f) => {
                bridge::SLOT.deliver(request_id, Err(f));
            }
            Ok(active) => {
                let req = active.req.clone();
                // OS の完了が即座に届いても照合できるよう、開く前に登録する。
                *bridge::lock() = Some(active);
                if let Err(f) = imp.start(request_id, &req) {
                    bridge::conclude_with(Some(request_id), |_| Some(Err(f)), None);
                }
            }
        }
        return;
    }

    let mine = SCREEN_OWNER.current();
    if state.phase != Phase::Running {
        // 完了の反映は毎フレーム先頭で済んでいるので、`Running` でないのに自分の持ち主の印が残っていれば前の画面状態
        // （Activity 再生成で `Idle` へ戻る前）の要求。OS のセッション・待ち受け・slot・印を回収する
        // （残すと待ち受けが期限後も生き、`begin_web_auth` が断られ続ける）。
        if mine != 0 {
            abandon_screen_request(Some(imp), mine);
        }
        return;
    }
    // 取り消し・期限は画面自身の要求だけに掛ける（公開入口の要求を巻き込まない）。
    if take_cancel(state).is_some() {
        bridge::conclude_with(
            Some(mine),
            |_| Some(Ok(WebAuthOutcome::Cancelled)),
            Some(imp),
        );
    }
    bridge::conclude_with(
        Some(mine),
        |a| {
            (a.started.elapsed() >= crate::contract::SESSION_TIMEOUT)
                .then_some(Ok(WebAuthOutcome::TimedOut))
        },
        Some(imp),
    );
    let generation = state.generation;
    poll_and_finish_owned(
        &bridge::SLOT,
        &SCREEN_OWNER,
        sender,
        |_request_id| {},
        move |s: &mut WebAuthSessionDemoState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != Phase::Idle {
                finish(s, generation, to_core(outcome));
                true
            } else {
                false
            }
        },
        // 自分の要求が失われた（`SLOT` が空・別の要求へ置き換わった）。成功にせず失敗で確定させる。
        move |s: &mut WebAuthSessionDemoState| {
            finish(s, generation, Err(CoreFailure::Internal));
        },
    );
}

/// 公開入口: 認証 UI を開き、結果を受け取るための要求 id を返す。画面の状態型に依存せず、フレームワークの外のアプリも使える
/// （`docs/architecture/consumer-app-structure.md` §8.2）。
///
/// `req` は呼び出し側が作る（`state`・PKCE・`start_url`。乱数と SHA-256 は [`platform_web_auth_session`] の
/// `fill_random`・`sha256` を使う）。開始後は [`poll_web_auth`] を呼び続けて結果を 1 度だけ受け取る。
/// 1 プロセスに 1 要求で、進行中または結果が未取得の要求があるうち（画面の要求を含む）は `Internal` で断る。
/// OS が UI を開けなかったときは `Err` で、結果は届かない。
/// Windows はループバックの受け口を呼び出し側が持ち、受けた URL を [`complete_web_auth_loopback`] へ渡す。非対応 OS は `Unsupported`。
///
/// 制約（デモ認可ページ向けの実装）:
/// - Android は開く URL を `http://127.0.0.1`、コールバックを `fandhe://web-auth` に固定している（`WebAuthSessionHelper`）。
///   外部アプリの実際の認可サーバーには使えない。`req.redirect` は OS へ渡さない。`start_url` が `http://127.0.0.1`
///   以外なら開けずに `Err(Internal)`、届いた `fandhe://web-auth` のコールバックが `req.redirect` と合わなければ
///   `InvalidCallback`、コールバックなしで戻れば `Cancelled`、ブラウザが戻らないまま期限を過ぎれば `TimedOut`。
/// - Windows は開く URL を `http://127.0.0.1:` か `https://` に限る（コールバックは呼び出し側のループバック受け口）。
/// - 実 OAuth 対応（開く URL・コールバックの設定化）は未実装。
pub fn begin_web_auth(req: AuthRequest) -> Result<u64, WebAuthFailure> {
    begin_external(&*platform_web_auth_session(), req)
}

fn begin_external(imp: &dyn WebAuthSession, req: AuthRequest) -> Result<u64, WebAuthFailure> {
    let request_id = EXTERNAL_OWNER
        .begin_excluding(&bridge::SLOT, &[&SCREEN_OWNER, &EXTERNAL_OWNER])
        .ok_or(WebAuthFailure::Internal)?;
    // OS の完了が即座に届いても照合できるよう、開く前に登録する。
    *bridge::lock() = Some(bridge::Active {
        request_id,
        started: std::time::Instant::now(),
        req: req.clone(),
        server: None,
    });
    if let Err(f) = imp.start(request_id, &req) {
        // 結果は届かないので、登録を外して `Err` で返す。
        bridge::conclude_with(Some(request_id), |_| Some(Err(f)), None);
        let _ = bridge::SLOT.take_ready(request_id);
        EXTERNAL_OWNER.release(request_id);
        return Err(f);
    }
    Ok(request_id)
}

/// [`begin_web_auth`] の結果を取り出す。確定していれば 1 度だけ `Some`（`take_ready` で原子的に取る）、未着・旧 id・取り出し済みは `None`。
///
/// 同じ `request_id` を複数スレッドから並行して呼んでも、結果（失われた場合の `Err` を含む）は 1 つの呼び出しにだけ返り、他は `None`。
///
/// 期限（[`SESSION_TIMEOUT`]、開始から単調時計）の判定はここで行う。超えていれば OS のセッションを閉じ、
/// `Ok(TimedOut)` を返す。呼び出さない間は期限切れが確定しない。要求が失われた（Activity の終了後に結果が返らず
/// 別の呼び出しで置き換わった等）ときは `Err(Internal)` を 1 度だけ返す。
pub fn poll_web_auth(request_id: u64) -> Option<Result<WebAuthOutcome, WebAuthFailure>> {
    poll_external(&*platform_web_auth_session(), request_id)
}

/// [`poll_external`] 全体を直列にする。所有者の確認・消失の判定・結果の取り出し・所有者の解放を 1 つの区間にして、
/// 同じ id を並行して呼んでも結果は 1 つの呼び出しにだけ返る（もう一方は所有者が外れているので `None`）。
/// 他のロックを持ったままは取らない（最外）。
static POLL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn poll_external(
    imp: &dyn WebAuthSession,
    request_id: u64,
) -> Option<Result<WebAuthOutcome, WebAuthFailure>> {
    let _serial = POLL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if request_id == 0 || EXTERNAL_OWNER.current() != request_id {
        return None;
    }
    if !matches!(bridge::SLOT.snapshot(), Some((id, _)) if id == request_id) {
        EXTERNAL_OWNER.release(request_id);
        return Some(Err(WebAuthFailure::Internal));
    }
    bridge::conclude_with(
        Some(request_id),
        |a| (a.started.elapsed() >= SESSION_TIMEOUT).then_some(Ok(WebAuthOutcome::TimedOut)),
        Some(imp),
    );
    let r = bridge::SLOT.take_ready(request_id)?;
    EXTERNAL_OWNER.release(request_id);
    Some(r)
}

/// [`begin_web_auth`] の要求を取り消す。OS のセッションを閉じ、結果は [`poll_web_auth`] が `Ok(Cancelled)` で返す。
/// 現在の要求でない id・確定済みの要求には何もしない。
pub fn cancel_web_auth(request_id: u64) {
    if request_id != 0 && EXTERNAL_OWNER.current() == request_id {
        bridge::conclude_with(
            Some(request_id),
            |_| Some(Ok(WebAuthOutcome::Cancelled)),
            Some(&*platform_web_auth_session()),
        );
    }
}

/// Windows のループバック（呼び出し側が持つ `127.0.0.1` の受け口）が受けたコールバック URL を渡す。
/// `verify_callback` を通った URL だけで確定して `true`。通らない・現在の要求でない・確定済みなら `false`
/// （受け口は 404 を返して待ち続ける）。他の OS は OS 層が受けるため使わない。
pub fn complete_web_auth_loopback(request_id: u64, url: String) -> bool {
    bridge::complete_loopback(request_id, url)
}

/// `registry.rs` から配線される effect ドライバ。
pub fn web_auth_session_demo_effect(
    state: &WebAuthSessionDemoState,
    sender: &CompletionSender<WebAuthSessionDemoState>,
) {
    drive(state, sender, &*platform_web_auth_session());
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }
    use crate::contract::{AuthRequest, RedirectKind, SESSION_TIMEOUT, UnsupportedWebAuthSession};
    use fandhe_core::screens::web_auth_session_demo as screen;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard};

    /// static を共有するので、テストを直列にする。
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Fake {
        started: AtomicBool,
        cancelled: AtomicBool,
        fail_start: AtomicBool,
        loopback: AtomicBool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                started: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                fail_start: AtomicBool::new(false),
                loopback: AtomicBool::new(false),
            }
        }
    }

    impl WebAuthSession for Fake {
        fn redirect_kind(&self) -> RedirectKind {
            if self.loopback.load(Ordering::SeqCst) {
                RedirectKind::Loopback
            } else {
                RedirectKind::CustomScheme
            }
        }
        fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure> {
            let n = buf.len() as u8;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(31).wrapping_add(n);
            }
            Ok(())
        }
        fn sha256(&self, _data: &[u8]) -> Result<[u8; 32], WebAuthFailure> {
            Ok([7; 32])
        }
        fn start(&self, _id: u64, _req: &AuthRequest) -> Result<(), WebAuthFailure> {
            self.started.store(true, Ordering::SeqCst);
            if self.fail_start.load(Ordering::SeqCst) {
                return Err(WebAuthFailure::NoBrowser);
            }
            Ok(())
        }
        fn cancel(&self, _id: u64) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn serial() -> MutexGuard<'static, ()> {
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        g
    }

    fn reset() {
        bridge::shutdown(&Fake::new());
        bridge::SLOT.reset();
        SCREEN_OWNER.adopt(0);
        EXTERNAL_OWNER.adopt(0);
    }

    struct Harness {
        state: WebAuthSessionDemoState,
        sender: CompletionSender<WebAuthSessionDemoState>,
        rx: std::sync::mpsc::Receiver<fandhe_runtime::Completion<WebAuthSessionDemoState>>,
    }

    impl Harness {
        fn new() -> Self {
            let (tx, rx) = std::sync::mpsc::channel();
            Self {
                state: WebAuthSessionDemoState::default(),
                sender: fandhe_runtime::CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
                rx,
            }
        }
        fn frame(&mut self, imp: &dyn WebAuthSession) {
            drive(&self.state, &self.sender, imp);
            while let Ok(apply) = self.rx.try_recv() {
                apply(&mut self.state);
            }
        }
    }

    /// 待ち受けの停止は非同期（join は別スレッド）なので、閉じるまで短く待つ。
    fn assert_port_closes(port: u16) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(std::time::Instant::now() < end, "待ち受けが閉じない");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn valid_url() -> String {
        let hex = bridge::lock().as_ref().unwrap().req.state.to_hex();
        format!("fandhe://web-auth/callback?state={hex}&code=abc123")
    }

    fn id() -> u64 {
        bridge::lock().as_ref().unwrap().request_id
    }

    #[test]
    fn unsupported_never_succeeds() {
        let _g = serial();
        let mut h = Harness::new();
        screen::start(&mut h.state);
        h.frame(&UnsupportedWebAuthSession);
        h.frame(&UnsupportedWebAuthSession);
        assert_eq!(h.state.phase, Phase::Idle);
        assert_eq!(h.state.error, screen::UNSUPPORTED_TEXT);
        assert!(h.state.status.is_empty() && h.state.detail.is_empty());
        assert!(bridge::lock().is_none() && bridge::SLOT.snapshot().is_none());
    }

    #[test]
    fn valid_callback_returns_masked_and_releases_server() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        assert!(fake.started.load(Ordering::SeqCst));
        let port = bridge::lock()
            .as_ref()
            .unwrap()
            .server
            .as_ref()
            .unwrap()
            .port();
        bridge::complete_from_os(id(), Ok(bridge::RawResult::Url(valid_url())));
        assert!(bridge::lock().is_none());
        assert_port_closes(port);
        h.frame(&fake);
        assert_eq!(h.state.status, screen::RETURNED_CODE_STATUS);
        assert!(h.state.detail.contains("（6 文字）") && !h.state.detail.contains("abc123"));
        assert!(bridge::SLOT.snapshot().is_none());
    }

    #[test]
    fn state_mismatch_is_invalid_callback() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        let url = format!("fandhe://web-auth/callback?state={}&code=x", "0".repeat(32));
        bridge::complete_from_os(id(), Ok(bridge::RawResult::Url(url)));
        h.frame(&fake);
        assert_eq!(h.state.error, screen::INVALID_CALLBACK_TEXT);
        assert!(h.state.status.is_empty());
    }

    fn http_get(port: u16, target: &str) -> String {
        use std::io::{Read, Write};
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(c, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        out
    }

    #[test]
    fn loopback_rejects_bad_state_and_keeps_waiting() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        fake.loopback.store(true, Ordering::SeqCst);
        screen::start(&mut h.state);
        h.frame(&fake);
        let (port, hex) = {
            let g = bridge::lock();
            let a = g.as_ref().unwrap();
            (a.server.as_ref().unwrap().port(), a.req.state.to_hex())
        };
        let bad = format!("/callback?state={}&code=x", "0".repeat(32));
        assert!(http_get(port, &bad).starts_with("HTTP/1.1 404"));
        assert!(bridge::lock().is_some());
        let good = format!("/callback?state={hex}&code=abc123");
        assert!(http_get(port, &good).starts_with("HTTP/1.1 200"));
        assert!(bridge::lock().is_none());
        h.frame(&fake);
        assert_eq!(h.state.status, screen::RETURNED_CODE_STATUS);
    }

    #[test]
    fn stale_request_id_is_dropped() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        bridge::complete_from_os(id() + 1000, Ok(bridge::RawResult::Url(valid_url())));
        assert!(bridge::lock().is_some());
        assert!(bridge::SLOT.snapshot().is_some_and(|(_, r)| r.is_none()));
    }

    #[test]
    fn late_success_after_deadline_is_timed_out() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        let url = valid_url();
        let past = std::time::Instant::now()
            .checked_sub(SESSION_TIMEOUT)
            .expect("monotonic clock is old enough");
        bridge::lock().as_mut().unwrap().started = past;
        bridge::complete_from_os(id(), Ok(bridge::RawResult::Url(url)));
        h.frame(&fake);
        assert_eq!(h.state.status, screen::TIMED_OUT_STATUS);
        assert!(h.state.error.is_empty());
    }

    #[test]
    fn deadline_in_frame_closes_session_before_returning() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        let url = valid_url();
        bridge::lock().as_mut().unwrap().started = std::time::Instant::now()
            .checked_sub(SESSION_TIMEOUT)
            .expect("monotonic clock is old enough");
        let rid = id();
        h.frame(&fake);
        assert!(fake.cancelled.load(Ordering::SeqCst) && bridge::lock().is_none());
        bridge::complete_from_os(rid, Ok(bridge::RawResult::Url(url)));
        h.frame(&fake);
        assert_eq!(h.state.status, screen::TIMED_OUT_STATUS);
    }

    #[test]
    fn cancel_closes_os_session_and_late_return_is_dropped() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        let (rid, url) = (id(), valid_url());
        screen::cancel(&mut h.state);
        h.frame(&fake);
        assert!(fake.cancelled.load(Ordering::SeqCst) && bridge::lock().is_none());
        bridge::complete_from_os(rid, Ok(bridge::RawResult::Url(url)));
        h.frame(&fake);
        assert_eq!(h.state.status, screen::CANCELLED_STATUS);
        assert!(h.state.error.is_empty() && h.state.detail.is_empty());
    }

    #[test]
    fn os_cancel_is_cancelled_not_error() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        bridge::complete_from_os(id(), Ok(bridge::RawResult::Cancelled));
        h.frame(&fake);
        assert_eq!(h.state.status, screen::CANCELLED_STATUS);
        assert!(h.state.error.is_empty());
    }

    #[test]
    fn os_failure_is_error() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        bridge::complete_from_os(id(), Err(WebAuthFailure::Internal));
        h.frame(&fake);
        assert!(h.state.error.starts_with(screen::FAILED_PREFIX) && h.state.status.is_empty());
    }

    #[test]
    fn start_failure_is_error_and_releases_server() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        fake.fail_start.store(true, Ordering::SeqCst);
        screen::start(&mut h.state);
        h.frame(&fake);
        h.frame(&fake);
        assert_eq!(h.state.error, screen::NO_BROWSER_TEXT);
        assert!(h.state.status.is_empty() && bridge::lock().is_none());
    }

    #[test]
    fn shutdown_releases_everything() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        bridge::shutdown(&fake);
        assert!(fake.cancelled.load(Ordering::SeqCst) && bridge::lock().is_none());
    }

    fn cancel_external(imp: &dyn WebAuthSession, id: u64) {
        bridge::conclude_with(Some(id), |_| Some(Ok(WebAuthOutcome::Cancelled)), Some(imp));
    }

    fn ext_request() -> AuthRequest {
        let state = crate::contract::State::from_bytes([3; 16]);
        AuthRequest {
            start_url: "http://127.0.0.1:1/authorize".into(),
            redirect: crate::contract::Redirect::CustomScheme {
                scheme: "fandhe".into(),
                path: crate::contract::CALLBACK_PATH.into(),
            },
            state,
        }
    }

    fn ext_url(req: &AuthRequest) -> String {
        format!(
            "fandhe://web-auth/callback?state={}&code=abc123",
            req.state.to_hex()
        )
    }

    #[test]
    fn external_result_is_taken_exactly_once() {
        let _g = serial();
        let fake = Fake::new();
        let req = ext_request();
        let id = begin_external(&fake, req.clone()).expect("開始");
        assert!(fake.started.load(Ordering::SeqCst));
        assert!(poll_external(&fake, id).is_none(), "未着は None");
        bridge::complete_from_os(id, Ok(bridge::RawResult::Url(ext_url(&req))));
        assert!(matches!(
            poll_external(&fake, id),
            Some(Ok(WebAuthOutcome::Returned(_)))
        ));
        assert!(poll_external(&fake, id).is_none(), "2 回目は取れない");
        assert!(bridge::SLOT.snapshot().is_none());
    }

    #[test]
    fn external_start_failure_is_err_and_leaves_nothing() {
        let _g = serial();
        let fake = Fake::new();
        fake.fail_start.store(true, Ordering::SeqCst);
        assert_eq!(
            begin_external(&fake, ext_request()).unwrap_err(),
            WebAuthFailure::NoBrowser
        );
        assert!(bridge::lock().is_none() && bridge::SLOT.snapshot().is_none());
        assert!(begin_external(&Fake::new(), ext_request()).is_ok());
    }

    #[test]
    fn external_deadline_is_decided_by_poll_and_closes_os_session() {
        let _g = serial();
        let fake = Fake::new();
        let id = begin_external(&fake, ext_request()).unwrap();
        bridge::lock().as_mut().unwrap().started = std::time::Instant::now()
            .checked_sub(SESSION_TIMEOUT)
            .expect("monotonic clock is old enough");
        assert!(matches!(
            poll_external(&fake, id),
            Some(Ok(WebAuthOutcome::TimedOut))
        ));
        assert!(fake.cancelled.load(Ordering::SeqCst) && bridge::lock().is_none());
    }

    #[test]
    fn external_lost_request_is_reported_once_as_error() {
        let _g = serial();
        let fake = Fake::new();
        let id = begin_external(&fake, ext_request()).unwrap();
        bridge::SLOT.reset();
        assert_eq!(
            poll_external(&fake, id),
            Some(Err(WebAuthFailure::Internal))
        );
        assert!(poll_external(&fake, id).is_none());
    }

    #[test]
    fn external_and_screen_do_not_replace_each_other() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        let id = begin_external(&fake, ext_request()).unwrap();
        // 画面は外の要求の進行中は始めない。外の要求も画面の要求を取り消さない。
        screen::start(&mut h.state);
        h.frame(&fake);
        assert_eq!(h.state.phase, Phase::Requested);
        cancel_external(&fake, id);
        // 外の結果が未取得のあいだも画面は始めない（結果を落とさない）。
        h.frame(&fake);
        assert_eq!(h.state.phase, Phase::Requested);
        assert!(matches!(
            poll_external(&fake, id),
            Some(Ok(WebAuthOutcome::Cancelled))
        ));
        h.frame(&fake);
        assert_eq!(h.state.phase, Phase::Running);
        // 画面の取り消し・期限は外の要求を巻き込まない。
        assert!(begin_external(&fake, ext_request()).is_err());
    }

    #[test]
    fn external_begin_is_refused_while_screen_request_runs() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        assert_eq!(
            begin_external(&fake, ext_request()).unwrap_err(),
            WebAuthFailure::Internal
        );
        assert!(bridge::lock().as_ref().is_some_and(|a| a.server.is_some()));
    }

    #[test]
    fn new_screen_state_abandons_the_previous_states_request_without_a_reset() {
        let _g = serial();
        let fake = Fake::new();
        let mut old = Harness::new();
        screen::start(&mut old.state);
        old.frame(&fake);
        let old_id = id();
        // Activity 再生成: 画面状態は作り直され、static の要求だけが残る（掃除の呼び出しなし）。
        let mut fresh = Harness::new();
        screen::start(&mut fresh.state);
        fresh.frame(&fake);
        assert!(
            fake.cancelled.load(Ordering::SeqCst),
            "前の要求の OS セッションを閉じる"
        );
        assert_ne!(id(), old_id);
        assert_eq!(fresh.state.phase, Phase::Running);
        // 古い要求の遅延コールバックは新しい要求を確定させない。
        bridge::complete_from_os(old_id, Ok(bridge::RawResult::Url(valid_url())));
        assert!(bridge::lock().is_some());
    }

    #[test]
    fn idle_screen_state_reclaims_the_previous_states_request() {
        let _g = serial();
        let (mut old, fake) = (Harness::new(), Fake::new());
        screen::start(&mut old.state);
        old.frame(&fake);
        let port = bridge::lock()
            .as_ref()
            .unwrap()
            .server
            .as_ref()
            .unwrap()
            .port();
        // Activity 再生成: 画面状態が Idle に戻る。次のフレームで旧要求を閉じる（開始操作を待たない）。
        let mut fresh = Harness::new();
        fresh.frame(&fake);
        assert!(fake.cancelled.load(Ordering::SeqCst));
        assert!(bridge::lock().is_none() && bridge::SLOT.snapshot().is_none());
        assert_port_closes(port);
        assert!(begin_external(&fake, ext_request()).is_ok());
    }

    #[test]
    fn concurrent_polls_return_the_result_once() {
        let _g = serial();
        let fake = Fake::new();
        let req = ext_request();
        let id = begin_external(&fake, req.clone()).unwrap();
        bridge::complete_from_os(id, Ok(bridge::RawResult::Url(ext_url(&req))));
        let got = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..8)
                .map(|_| sc.spawn(|| poll_external(&fake, id)))
                .collect();
            hs.into_iter()
                .filter_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0], Ok(WebAuthOutcome::Returned(_))));
    }

    #[test]
    fn lifecycle_reclaim_closes_only_the_screen_request() {
        let _g = serial();
        let (mut h, fake) = (Harness::new(), Fake::new());
        screen::start(&mut h.state);
        h.frame(&fake);
        let port = bridge::lock()
            .as_ref()
            .unwrap()
            .server
            .as_ref()
            .unwrap()
            .port();
        let rid = id();
        // 画面の描画に関係なく、破棄の通知だけで回収できる。
        assert_eq!(reclaim_screen_request(), Some(rid));
        assert!(bridge::lock().is_none() && bridge::SLOT.snapshot().is_none());
        assert_port_closes(port);
        assert_eq!(reclaim_screen_request(), None, "二重には閉じない");
        assert!(begin_external(&fake, ext_request()).is_ok());
        // 公開入口の要求は回収しない。
        assert_eq!(reclaim_screen_request(), None);
        assert!(bridge::lock().is_some());
    }

    #[test]
    fn no_request_never_calls_os() {
        let _g = serial();
        let mut h = Harness::new();
        let fake = Fake::new();
        h.frame(&fake);
        assert!(!fake.started.load(Ordering::SeqCst));
    }
}
