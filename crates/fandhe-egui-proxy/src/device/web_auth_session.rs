//! Phase 11（親 #113）型サンプル「OAuth 用の Web 認証セッション」（Issue #530）の effect ドライバ。
//! `fandhe_core::screens::web_auth_session_demo`（端末依存ゼロ）の要求を観測し、デモ認可サーバー
//! （`fandhe_device::web_auth_session::DemoAuthServer`）を起こして OS の認証 UI を開き、返った
//! コールバック URL を検査して `CompletionSender` 経由で画面へ返す
//! （`screens/registry.rs` の `effect` 区画から呼ばれる契約）。契約と守る範囲は
//! `fandhe_device::web_auth_session` のモジュール doc を参照。
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
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（macOS: `macos_web_auth_session`（#530）、Windows: `windows_web_auth_session`（#531）、
//! iOS: `ios_web_auth_session`（#532）、Android: `android_web_auth_session`（#534））。
//! それ以外（Linux 等）は `UnsupportedWebAuthSession` 固定で、ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::web_auth_session_demo::{
    finish, mark_running, take_cancel, take_request, CoreFailure, CoreOutcome, Phase, ReturnedKind,
    WebAuthSessionDemoState,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::web_auth_session::UnsupportedWebAuthSession;
use fandhe_device::web_auth_session::{
    CallbackKind, WebAuthFailure, WebAuthOutcome, WebAuthSession,
};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `WebAuthSession` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn WebAuthSession> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn WebAuthSession>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                super::macos_web_auth_session::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_web_auth_session::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_web_auth_session::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_web_auth_session::implementation()
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

    use fandhe_core::screens::web_auth_session_demo::{CoreFailure, CoreOutcome};
    use fandhe_device::web_auth_session::{
        verify_callback, AuthRequest, DemoAuthServer, WebAuthFailure, WebAuthOutcome,
        WebAuthSession, SESSION_TIMEOUT,
    };

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<CoreOutcome, CoreFailure>;

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
                SLOT.deliver(a.request_id, super::to_core(r));
                Some(a)
            })
        };
        if let Some(mut a) = taken {
            if let Some(os) = os {
                os.cancel(a.request_id);
            }
            if let Some(s) = a.server.as_mut() {
                s.stop();
            }
        }
    }

    /// OS 層（macOS / iOS / Android の完了、Windows のループバック）の唯一の受け口。
    /// 現在の要求でない `request_id`・取り消し／期限で確定済みの要求の完了は捨てる。
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

    /// 終了時の後片付け。進行中の 1 件があれば取り消し、OS のセッションとサーバーを閉じる。
    pub(crate) fn shutdown(os: &dyn WebAuthSession) {
        conclude_with(None, |_| Some(Ok(WebAuthOutcome::Cancelled)), Some(os));
    }
}

/// 終了時（`device::shutdown`）に進行中の認証を閉じる。
pub(crate) fn shutdown() {
    if bridge::lock().is_some() {
        bridge::shutdown(&*implementation());
    }
}

/// 要求 1 件ぶんの準備（乱数・PKCE・デモ認可サーバー）。進行中の 1 件として登録する値を返す。
fn setup(imp: &dyn WebAuthSession, request_id: u64) -> Result<bridge::Active, WebAuthFailure> {
    use fandhe_device::passkey::base64url_encode;
    use fandhe_device::url_scheme::SCHEME;
    use fandhe_device::web_auth_session::{
        AuthRequest, DemoAuthServer, Pkce, Redirect, RedirectKind, State, CALLBACK_PATH,
    };

    let started = std::time::Instant::now();
    let (mut state, mut verifier, mut code) = ([0u8; 16], [0u8; 32], [0u8; 16]);
    imp.fill_random(&mut state)?;
    imp.fill_random(&mut verifier)?;
    imp.fill_random(&mut code)?;
    let state = State::from_bytes(state);
    let pkce = Pkce::from_random(&verifier, |d| imp.sha256(d))?;
    let kind = imp.redirect_kind();
    let custom = format!("{SCHEME}://{CALLBACK_PATH}");
    let tx: Option<fandhe_device::web_auth_session::LoopbackTx> = (kind == RedirectKind::Loopback)
        .then(|| {
            Box::new(move |u: String| {
                bridge::complete_from_os(request_id, Ok(bridge::RawResult::Url(u)))
            }) as _
        });
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

/// 毎フレームの処理（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &WebAuthSessionDemoState,
    sender: &CompletionSender<WebAuthSessionDemoState>,
    imp: &dyn WebAuthSession,
) {
    if take_request(state).is_some() {
        // `mark_running` の反映前に次フレームが同じ要求を観測しても、二重に開かない。
        if bridge::lock().is_some() || bridge::SLOT.snapshot().is_some() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        let request_id = bridge::SLOT.begin();
        match setup(imp, request_id) {
            // 準備に失敗した要求は登録前なので、結果だけを置く（サーバーは `Drop` で止まる）。
            Err(f) => {
                bridge::SLOT.deliver(request_id, Err(to_failure(f)));
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

    if state.phase != Phase::Running {
        return;
    }
    if take_cancel(state).is_some() {
        bridge::conclude_with(None, |_| Some(Ok(WebAuthOutcome::Cancelled)), Some(imp));
    }
    bridge::conclude_with(
        None,
        |a| {
            (a.started.elapsed() >= fandhe_device::web_auth_session::SESSION_TIMEOUT)
                .then_some(Ok(WebAuthOutcome::TimedOut))
        },
        Some(imp),
    );
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut WebAuthSessionDemoState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != Phase::Idle {
                finish(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から配線される effect ドライバ。
pub fn web_auth_session_demo_effect(
    state: &WebAuthSessionDemoState,
    sender: &CompletionSender<WebAuthSessionDemoState>,
) {
    drive(state, sender, &*implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::web_auth_session_demo as screen;
    use fandhe_device::web_auth_session::{
        AuthRequest, RedirectKind, UnsupportedWebAuthSession, SESSION_TIMEOUT,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard};

    /// static を共有するので、テストを直列にする。
    static SERIAL: Mutex<()> = Mutex::new(());

    struct Fake {
        started: AtomicBool,
        cancelled: AtomicBool,
        fail_start: AtomicBool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                started: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                fail_start: AtomicBool::new(false),
            }
        }
    }

    impl WebAuthSession for Fake {
        fn redirect_kind(&self) -> RedirectKind {
            RedirectKind::CustomScheme
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
    }

    struct Harness {
        state: WebAuthSessionDemoState,
        sender: CompletionSender<WebAuthSessionDemoState>,
        rx: std::sync::mpsc::Receiver<crate::app::Completion<WebAuthSessionDemoState>>,
    }

    impl Harness {
        fn new() -> Self {
            let (tx, rx) = std::sync::mpsc::channel();
            Self {
                state: WebAuthSessionDemoState::default(),
                sender: CompletionSender::new(tx, egui::Context::default()),
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
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
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

    #[test]
    fn no_request_never_calls_os() {
        let _g = serial();
        let mut h = Harness::new();
        let fake = Fake::new();
        h.frame(&fake);
        assert!(!fake.started.load(Ordering::SeqCst));
    }
}
