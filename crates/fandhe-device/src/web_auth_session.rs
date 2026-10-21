//! OAuth 用の Web 認証セッション（Phase 11 の拡充ループ、親 #113、Issue #530）。
//!
//! 扱う範囲は「アプリ内のデモ認可ページ（127.0.0.1）を OS の認証 UI で開き、返ってきた
//! コールバック URL を検査して伏せ字で画面に出す」ところまで。PKCE（S256）の値は作るが、
//! トークン交換はしない（対応表 `docs/device/capabilities/web-auth-session.json`）。
//! 呼び出し元は `fandhe-egui-proxy` の `device/web_auth_session.rs`（effect ドライバ）と
//! 各 OS 実装（macOS は `device/macos_web_auth_session.rs`）。画面は `fandhe-core` の
//! `screens::web_auth_session_demo`（端末依存ゼロ、本クレートを参照しない）。
//!
//! 本クレートは `forbid(unsafe_code)` のため、乱数と SHA-256 は OS 層から [`WebAuthSession`]
//! のメソッドとして受ける。コールバック URL の検査 [`verify_callback`]・PKCE・AS エラーの写像・
//! デモ認可サーバー [`DemoAuthServer`] は OS 非依存でここに置き、ホストの `cargo test` で検証する。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は三値**: [`WebAuthOutcome::Returned`]＝state 一致の URL が返った（認可の成立は
//!   意味しない）、[`WebAuthOutcome::Cancelled`]＝OS の UI を閉じた・画面で取り消した・
//!   Android でコールバックなしに復帰した、[`WebAuthOutcome::TimedOut`]＝期限切れ（中立表示）
//! - **失敗**: state 不一致・パス違い・不正な URL は [`WebAuthFailure::InvalidCallback`]
//!   （成功に畳まない）、ブラウザなしは `NoBrowser`、非対応 OS は `Unsupported`
//! - **期限**: [`SESSION_TIMEOUT`] を呼び出し開始から単調時計（`Instant`）で測る。期限切れで戻る
//!   前に `cancel` とサーバー停止を済ませ、期限後に届いた完了は捨てる
//! - **同時 1 件**: 要求は 1 件ずつ処理する。二重の開始は無視する
//! - **ログに出さない**: URL・state・code・verifier を `Debug` とログに出さない
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内で 1 件ずつ処理すること、嘘の成功を出さないこと、期限・取り消し・終了で
//! サーバーと OS のセッションを残さないこと、秘密値をログと `Debug` に出さないこと。
//! 守らない: Android でブラウザのタブが裏に残ること、戻った後にそのタブで操作した結果（捨てる）、
//! 既定ブラウザの UI の差、複数プロセス、プロセス死後のコールバック、トークン交換。

use std::fmt;

use crate::passkey::base64url_encode;
use crate::url_scheme::MAX_URL_BYTES;

/// Rust 側の唯一の期限（呼び出し開始から）。
pub const SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// カスタム scheme のコールバックのパス部分（`fandhe://web-auth/callback`）。
pub const CALLBACK_PATH: &str = "web-auth/callback";

/// コールバックに載せる CSRF 対策の値（128 bit）。`Debug` は値を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct State([u8; 16]);

impl State {
    /// CSPRNG 由来の 16 バイトから作る。
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// 小文字 16 進 32 文字。
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("State(<redacted>)")
    }
}

/// PKCE（RFC 7636、S256 のみ）。`Debug` は値を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    /// code_verifier（32 バイトの base64url、43 文字）。
    pub verifier: String,
    /// code_challenge（verifier の SHA-256 の base64url）。
    pub challenge: String,
}

impl Pkce {
    /// OS の CSPRNG の 32 バイトと OS の SHA-256 から作る。
    pub fn from_random(
        bytes: &[u8; 32],
        sha256: impl FnOnce(&[u8]) -> Result<[u8; 32], WebAuthFailure>,
    ) -> Result<Self, WebAuthFailure> {
        let verifier = base64url_encode(bytes);
        let challenge = base64url_encode(&sha256(verifier.as_bytes())?);
        Ok(Self {
            verifier,
            challenge,
        })
    }

    /// code_challenge_method。常に `S256`（plain は使わない）。
    pub fn method(&self) -> &'static str {
        "S256"
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pkce(<redacted>)")
    }
}

/// リダイレクト方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `fandhe://web-auth/callback`（macOS・iOS・Android）。
    CustomScheme,
    /// `http://127.0.0.1:{port}/callback`（Windows）。
    Loopback,
}

/// コールバック先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    /// カスタム scheme。
    CustomScheme {
        /// scheme（`fandhe`）。
        scheme: String,
        /// `scheme://` の後ろ。
        path: String,
    },
    /// ループバック。
    Loopback {
        /// デモ認可サーバーの待ち受けポート。
        port: u16,
    },
}

impl Redirect {
    /// コールバックのベース URI。
    pub fn redirect_uri(&self) -> String {
        match self {
            Self::CustomScheme { scheme, path } => format!("{scheme}://{path}"),
            Self::Loopback { port } => format!("http://127.0.0.1:{port}/callback"),
        }
    }
}

/// OS へ渡す認可要求。`Debug` は start_url（state を含む）を伏せる。
#[derive(Clone)]
pub struct AuthRequest {
    /// OS の認証 UI で開く URL。
    pub start_url: String,
    /// コールバック先。
    pub redirect: Redirect,
    /// 期待する state。
    pub state: State,
}

impl AuthRequest {
    /// OS が捕まえる scheme。ループバックでは `None`。
    pub fn callback_scheme(&self) -> Option<&str> {
        match &self.redirect {
            Redirect::CustomScheme { scheme, .. } => Some(scheme),
            Redirect::Loopback { .. } => None,
        }
    }
}

impl fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthRequest(<redacted>)")
    }
}

/// 検査を通ったコールバック。code の値は保持しない（トークン交換をしないため）。
#[derive(Clone, PartialEq, Eq)]
pub struct CallbackUrl {
    base: String,
    /// 中身。
    pub kind: CallbackKind,
}

/// コールバックの中身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackKind {
    /// 認可コードが付いていた。
    Code {
        /// code の文字数。
        len: usize,
    },
    /// 認可サーバーがエラーを返した（`access_denied` 等、`[a-z_]` の 1〜64 文字）。
    Error {
        /// エラーコード。
        error: String,
    },
}

impl CallbackUrl {
    /// 画面に出す伏せ字の URL。state・code の値そのものは含まない。
    pub fn masked(&self) -> String {
        match &self.kind {
            CallbackKind::Code { len } => {
                format!("{}?state=（一致）&code=（{len} 文字）", self.base)
            }
            CallbackKind::Error { error } => {
                format!("{}?state=（一致）&error={error}", self.base)
            }
        }
    }
}

impl fmt::Debug for CallbackUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallbackUrl(<redacted>)")
    }
}

/// 認証セッションの結果（三値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebAuthOutcome {
    /// state 一致の URL が返った。
    Returned(CallbackUrl),
    /// 利用者の操作で閉じた。
    Cancelled,
    /// 期限切れ。
    TimedOut,
}

/// 失敗。OS 固有の値は載せない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebAuthFailure {
    /// この環境では使えない。
    Unsupported,
    /// 開けるブラウザがない。
    NoBrowser,
    /// 返ってきた URL が検査に通らない。
    InvalidCallback,
    /// その他（固定文言で表示する）。
    Internal,
}

/// OS の Web 認証セッション。実装は OS 層（`fandhe-egui-proxy`）。
pub trait WebAuthSession: Send + Sync {
    /// この OS のリダイレクト方式。
    fn redirect_kind(&self) -> RedirectKind;
    /// OS の CSPRNG で `buf` を埋める。
    fn fill_random(&self, buf: &mut [u8]) -> Result<(), WebAuthFailure>;
    /// SHA-256。
    fn sha256(&self, data: &[u8]) -> Result<[u8; 32], WebAuthFailure>;
    /// 認証 UI を開く。結果は非同期に届き、OS 層が bridge の受け口へ渡す。
    fn start(&self, request_id: u64, req: &AuthRequest) -> Result<(), WebAuthFailure>;
    /// 開いた UI を閉じる。冪等で、閉じられない OS では何もしない。
    fn cancel(&self, request_id: u64);
}

/// 非対応環境の実装。全メソッドが `Unsupported` を返し、成功へ倒さない。
pub struct UnsupportedWebAuthSession;

impl WebAuthSession for UnsupportedWebAuthSession {
    fn redirect_kind(&self) -> RedirectKind {
        RedirectKind::CustomScheme
    }
    fn fill_random(&self, _buf: &mut [u8]) -> Result<(), WebAuthFailure> {
        Err(WebAuthFailure::Unsupported)
    }
    fn sha256(&self, _data: &[u8]) -> Result<[u8; 32], WebAuthFailure> {
        Err(WebAuthFailure::Unsupported)
    }
    fn start(&self, _request_id: u64, _req: &AuthRequest) -> Result<(), WebAuthFailure> {
        Err(WebAuthFailure::Unsupported)
    }
    fn cancel(&self, _request_id: u64) {}
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn is_code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-')
}

/// 返ってきたコールバック URL を検査する。違反はすべて `InvalidCallback`。
///
/// 前方一致は scheme だけ大文字小文字を区別せず、残りは完全一致。直後は `?` でなければならず、
/// `fandhex:` や `#`・パスの続きは通さない。state は定数時間で比較する。
pub fn verify_callback(raw: &str, req: &AuthRequest) -> Result<CallbackUrl, WebAuthFailure> {
    use WebAuthFailure::InvalidCallback as Bad;
    if raw.is_empty() || raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
        return Err(Bad);
    }
    let base = req.redirect.redirect_uri();
    let (scheme, rest) = base.split_once("://").ok_or(Bad)?;
    let head = raw.get(..scheme.len()).ok_or(Bad)?;
    let tail = raw[scheme.len()..]
        .strip_prefix("://")
        .and_then(|t| t.strip_prefix(rest))
        .and_then(|t| t.strip_prefix('?'))
        .ok_or(Bad)?;
    if !head.eq_ignore_ascii_case(scheme) {
        return Err(Bad);
    }
    let (mut state, mut code, mut error) = (None, None, None);
    let mut seen: Vec<&str> = Vec::new();
    for pair in tail.split('&') {
        let (k, v) = pair.split_once('=').ok_or(Bad)?;
        if seen.contains(&k) {
            return Err(Bad);
        }
        seen.push(k);
        match k {
            "state" => state = Some(v),
            "code" => code = Some(v),
            "error" => error = Some(v),
            _ => {}
        }
    }
    if !ct_eq(state.ok_or(Bad)?.as_bytes(), req.state.to_hex().as_bytes()) {
        return Err(Bad);
    }
    let kind = match (code, error) {
        (Some(c), None) if (1..=512).contains(&c.len()) && c.chars().all(is_code_char) => {
            CallbackKind::Code { len: c.len() }
        }
        (None, Some(e))
            if (1..=64).contains(&e.len())
                && e.chars().all(|c| c == '_' || c.is_ascii_lowercase()) =>
        {
            CallbackKind::Error {
                error: e.to_string(),
            }
        }
        _ => return Err(Bad),
    };
    Ok(CallbackUrl { base, kind })
}

/// `ASWebAuthenticationSessionErrorDomain` のコードを結果へ写す（iOS・macOS 共有）。
///
/// 1（CanceledLogin）だけがキャンセル。2・3・未知の値は `Internal`（コードを潰さず種別で分ける。
/// 別ドメインのエラーは OS 層が `Internal` にする）。
pub fn outcome_from_as_error(code: i64) -> Result<WebAuthOutcome, WebAuthFailure> {
    match code {
        1 => Ok(WebAuthOutcome::Cancelled),
        _ => Err(WebAuthFailure::Internal),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use server::{DemoAuthServer, LoopbackTx};

#[cfg(not(target_arch = "wasm32"))]
mod server {
    use super::WebAuthFailure;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const MAX_REQUEST: usize = 8 * 1024;
    const READ_DEADLINE: Duration = Duration::from_secs(2);

    /// ループバックの完了通知。生の URL を受ける。
    pub type LoopbackTx = Box<dyn Fn(String) + Send + 'static>;

    /// アプリ内のデモ認可ページ（127.0.0.1 のみ待ち受け）。
    ///
    /// リダイレクト先は構築時に固定し、クエリの `redirect_uri` は受けない（オープンリダイレクタに
    /// しない）。HTML に埋める値は構築時に文字集合を検査する。`stop` / `Drop` で listener を閉じる。
    pub struct DemoAuthServer {
        port: u16,
        authorize_query: String,
        redirect_uri: String,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    struct Ctx {
        port: u16,
        authorize_query: String,
        html: String,
        loopback_tx: Option<LoopbackTx>,
    }

    fn all(s: &str, f: impl Fn(char) -> bool) -> bool {
        !s.is_empty() && s.chars().all(f)
    }

    impl DemoAuthServer {
        /// 待ち受けを始める。`custom_redirect` が `Some` ならカスタム scheme（`/callback` は 404）、
        /// `None` ならループバック（`/callback` の生 URL を `loopback_tx` へ渡す）。
        pub fn start(
            state_hex: &str,
            challenge: &str,
            code: &str,
            custom_redirect: Option<&str>,
            loopback_tx: Option<LoopbackTx>,
        ) -> Result<Self, WebAuthFailure> {
            let b64 = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
            if !all(state_hex, |c| c.is_ascii_hexdigit())
                || !all(challenge, b64)
                || !all(code, b64)
                || custom_redirect
                    .is_some_and(|r| !all(r, |c| c.is_ascii_alphanumeric() || "-./:".contains(c)))
            {
                return Err(WebAuthFailure::Internal);
            }
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .map_err(|_| WebAuthFailure::Internal)?;
            listener
                .set_nonblocking(true)
                .map_err(|_| WebAuthFailure::Internal)?;
            let port = listener
                .local_addr()
                .map_err(|_| WebAuthFailure::Internal)?
                .port();
            let redirect_uri = custom_redirect
                .map(str::to_owned)
                .unwrap_or_else(|| format!("http://127.0.0.1:{port}/callback"));
            let html = format!(
                "<!doctype html><html lang=\"ja\"><head><meta charset=\"utf-8\"><title>デモの認可画面</title></head>\
<body><h1>デモの認可画面</h1><p>アプリ内のデモです。実在のサービスではありません。</p>\
<p><a href=\"{redirect_uri}?state={state_hex}&amp;code={code}\">許可する</a></p>\
<p><a href=\"{redirect_uri}?state={state_hex}&amp;error=access_denied\">拒否する</a></p></body></html>",
            );
            let ctx = Ctx {
                port,
                authorize_query: format!(
                    "state={state_hex}&code_challenge={challenge}&code_challenge_method=S256"
                ),
                html,
                loopback_tx: if custom_redirect.is_some() {
                    None
                } else {
                    loopback_tx
                },
            };
            let authorize_query = ctx.authorize_query.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::Builder::new()
                .name("web-auth-demo".into())
                .spawn(move || {
                    // ponytail: 50ms 間隔のポーリング。停止が最大 50ms 遅れる。問題になれば自分へ接続して起こす。
                    while !flag.load(Ordering::SeqCst) {
                        match listener.accept() {
                            Ok((s, _)) => handle(s, &ctx),
                            Err(_) => std::thread::sleep(Duration::from_millis(50)),
                        }
                    }
                })
                .map_err(|_| WebAuthFailure::Internal)?;
            Ok(Self {
                port,
                authorize_query,
                redirect_uri,
                stop,
                thread: Some(thread),
            })
        }

        /// 待ち受けポート。
        pub fn port(&self) -> u16 {
            self.port
        }

        /// コールバック先のベース URI。
        pub fn redirect_uri(&self) -> &str {
            &self.redirect_uri
        }

        /// OS の認証 UI で開く URL。
        pub fn authorize_url(&self) -> String {
            format!(
                "http://127.0.0.1:{}/authorize?{}",
                self.port, self.authorize_query
            )
        }

        /// 停止して listener を閉じる。自スレッド（ループバックの完了通知経由）からは join しない。
        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.thread.take() {
                if h.thread().id() != std::thread::current().id() {
                    let _ = h.join();
                }
            }
        }
    }

    impl Drop for DemoAuthServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn respond(s: &mut TcpStream, status: &str, body: &str) {
        let r = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\n\
X-Frame-Options: DENY\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = s.write_all(r.as_bytes());
    }

    /// 要求ヘッダを 8 KiB・2 秒までで読む。超過・不正は `Err(ステータス)`。
    fn read_head(s: &mut TcpStream) -> Result<String, &'static str> {
        let deadline = Instant::now() + READ_DEADLINE;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || s.set_read_timeout(Some(left)).is_err() {
                return Err("408 Request Timeout");
            }
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => return Err("400 Bad Request"),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            if buf.len() > MAX_REQUEST {
                return Err("431 Request Header Fields Too Large");
            }
        }
        String::from_utf8(buf).map_err(|_| "400 Bad Request")
    }

    fn handle(mut s: TcpStream, ctx: &Ctx) {
        // 受け付けたソケットが listener の nonblocking を引き継ぐ OS があるため明示する。
        let _ = s.set_nonblocking(false);
        let head = match read_head(&mut s) {
            Ok(h) => h,
            Err(st) => return respond(&mut s, st, ""),
        };
        let mut lines = head.split("\r\n");
        let mut req = lines.next().unwrap_or("").split(' ');
        let (method, target) = (req.next().unwrap_or(""), req.next().unwrap_or(""));
        let host_ok = lines.filter_map(|l| l.split_once(':')).any(|(k, v)| {
            k.eq_ignore_ascii_case("host")
                && v.trim() == SocketAddr::from((Ipv4Addr::LOCALHOST, ctx.port)).to_string()
        });
        if !host_ok {
            return respond(&mut s, "421 Misdirected Request", "");
        }
        if method != "GET" {
            return respond(&mut s, "405 Method Not Allowed", "");
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/authorize" => {
                let want: Vec<&str> = ctx.authorize_query.split('&').collect();
                let got: Vec<&str> = query.split('&').collect();
                if got.len() == want.len() && want.iter().all(|w| got.contains(w)) {
                    respond(&mut s, "200 OK", &ctx.html);
                } else {
                    respond(&mut s, "400 Bad Request", "");
                }
            }
            "/callback" => match &ctx.loopback_tx {
                Some(tx) => {
                    tx(format!("http://127.0.0.1:{}{target}", ctx.port));
                    respond(
                        &mut s,
                        "200 OK",
                        "<!doctype html><meta charset=\"utf-8\"><title>完了</title><p>アプリに戻ってください。</p>",
                    );
                }
                None => respond(&mut s, "404 Not Found", ""),
            },
            _ => respond(&mut s, "404 Not Found", ""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::url_scheme::SCHEME;

    fn req_with(redirect: Redirect) -> AuthRequest {
        AuthRequest {
            start_url: "http://127.0.0.1:1/authorize".into(),
            redirect,
            state: State::from_bytes([0xab; 16]),
        }
    }

    fn custom() -> AuthRequest {
        req_with(Redirect::CustomScheme {
            scheme: SCHEME.into(),
            path: CALLBACK_PATH.into(),
        })
    }

    const OK_STATE: &str = "abababababababababababababababab";

    #[test]
    fn pkce_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let bytes: [u8; 32] = crate::passkey::base64url_decode(verifier)
            .unwrap()
            .try_into()
            .unwrap();
        let digest: [u8; 32] =
            crate::passkey::base64url_decode("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
                .unwrap()
                .try_into()
                .unwrap();
        let p = Pkce::from_random(&bytes, |d| {
            assert_eq!(d, verifier.as_bytes());
            Ok(digest)
        })
        .unwrap();
        assert_eq!(p.verifier, verifier);
        assert_eq!(p.challenge, base64url_encode(&digest));
        assert_eq!(p.method(), "S256");
        assert!(Pkce::from_random(&bytes, |_| Err(WebAuthFailure::Unsupported)).is_err());
    }

    #[test]
    fn verify_accepts_code_and_error_and_masks() {
        let r = custom();
        let ok = verify_callback(
            &format!("fandhe://web-auth/callback?state={OK_STATE}&code=abc.DEF_1~-"),
            &r,
        )
        .unwrap();
        assert_eq!(ok.kind, CallbackKind::Code { len: 11 });
        assert!(ok.masked().contains("（11 文字）") && !ok.masked().contains("abc"));
        assert!(!format!("{ok:?}").contains("abc"));
        let denied = verify_callback(
            &format!("FANDHE://web-auth/callback?state={OK_STATE}&error=access_denied"),
            &r,
        )
        .unwrap();
        assert_eq!(
            denied.kind,
            CallbackKind::Error {
                error: "access_denied".into()
            }
        );
    }

    #[test]
    fn verify_rejects_bad_inputs() {
        let r = custom();
        let b = "fandhe://web-auth/callback";
        let bad = [
            String::new(),
            format!("{b}?state={}&code=x", "0".repeat(32)),
            format!("{b}?code=x"),
            format!("fandhe://web-auth/other?state={OK_STATE}&code=x"),
            format!("fandhex://web-auth/callback?state={OK_STATE}&code=x"),
            format!("{b}#x?state={OK_STATE}&code=x"),
            format!("{b}/x?state={OK_STATE}&code=x"),
            format!("{b}?state={OK_STATE}&code=x&code=y"),
            format!("{b}?state={OK_STATE}&code=x&error=e"),
            format!("{b}?state={OK_STATE}"),
            format!("{b}?state={OK_STATE}&code=a%20b"),
            format!("{b}?state={OK_STATE}&code="),
            format!("{b}?state={OK_STATE}&error=Access"),
            format!("{b}?state={OK_STATE}&code=x\n"),
            format!("{b}?state={OK_STATE}&code={}", "a".repeat(2048)),
            format!("{b}?state={OK_STATE}&code={}", "a".repeat(513)),
            "日本語".into(),
        ];
        for u in bad {
            assert_eq!(
                verify_callback(&u, &r).unwrap_err(),
                WebAuthFailure::InvalidCallback,
                "{u:?}"
            );
        }
    }

    #[test]
    fn verify_loopback_requires_exact_port() {
        let r = req_with(Redirect::Loopback { port: 4000 });
        assert!(
            verify_callback(
                &format!("http://127.0.0.1:4000/callback?state={OK_STATE}&code=x"),
                &r
            )
            .is_ok()
        );
        assert!(
            verify_callback(
                &format!("http://127.0.0.1:4001/callback?state={OK_STATE}&code=x"),
                &r
            )
            .is_err()
        );
        assert!(
            verify_callback(
                &format!("http://127.0.0.1:40000/callback?state={OK_STATE}&code=x"),
                &r
            )
            .is_err()
        );
    }

    #[test]
    fn as_error_mapping() {
        assert_eq!(outcome_from_as_error(1), Ok(WebAuthOutcome::Cancelled));
        for c in [2, 3, 0, -1, 99] {
            assert_eq!(outcome_from_as_error(c), Err(WebAuthFailure::Internal));
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedWebAuthSession;
        assert_eq!(u.fill_random(&mut [0; 4]), Err(WebAuthFailure::Unsupported));
        assert_eq!(u.sha256(b"x"), Err(WebAuthFailure::Unsupported));
        assert_eq!(u.start(1, &custom()), Err(WebAuthFailure::Unsupported));
    }

    #[test]
    fn debug_hides_secrets() {
        let r = custom();
        assert!(!format!("{r:?}{:?}", r.state).contains("abab"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod server {
        use super::*;
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::mpsc;

        fn http(port: u16, raw: &str) -> String {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        }

        fn get(port: u16, target: &str, host: &str) -> String {
            http(
                port,
                &format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
            )
        }

        fn custom_server() -> DemoAuthServer {
            DemoAuthServer::start(
                OK_STATE,
                "chal-_1",
                "code-_1",
                Some("fandhe://web-auth/callback"),
                None,
            )
            .unwrap()
        }

        #[test]
        fn authorize_ok_has_only_fixed_redirects() {
            let mut s = custom_server();
            let url = s.authorize_url();
            let target = url
                .strip_prefix(&format!("http://127.0.0.1:{}", s.port()))
                .unwrap();
            let r = get(s.port(), target, &format!("127.0.0.1:{}", s.port()));
            assert!(r.starts_with("HTTP/1.1 200"), "{r}");
            assert!(r.contains("Cache-Control: no-store") && r.contains("default-src 'none'"));
            assert_eq!(r.matches("href=\"").count(), 2);
            assert_eq!(
                r.matches("href=\"fandhe://web-auth/callback?state=")
                    .count(),
                2
            );
            assert!(r.contains(&format!("state={OK_STATE}&amp;code=code-_1")));
            assert!(r.contains("error=access_denied"));
            s.stop();
        }

        #[test]
        fn rejects_bad_requests() {
            let mut s = custom_server();
            let (p, host) = (s.port(), format!("127.0.0.1:{}", s.port()));
            let q = s.authorize_url().split_once('?').unwrap().1.to_string();
            assert!(get(p, &format!("/authorize?{q}"), "evil.example").starts_with("HTTP/1.1 421"));
            assert!(get(p, "/authorize?state=zz", &host).starts_with("HTTP/1.1 400"));
            assert!(get(p, &format!("/authorize?{q}&x=1"), &host).starts_with("HTTP/1.1 400"));
            assert!(get(p, "/nope", &host).starts_with("HTTP/1.1 404"));
            assert!(get(p, "/callback?state=x", &host).starts_with("HTTP/1.1 404"));
            assert!(
                http(
                    p,
                    &format!("POST /authorize HTTP/1.1\r\nHost: {host}\r\n\r\n")
                )
                .starts_with("HTTP/1.1 405")
            );
            let big = format!(
                "GET / HTTP/1.1\r\nHost: {host}\r\nX: {}\r\n\r\n",
                "a".repeat(9000)
            );
            assert!(http(p, &big).starts_with("HTTP/1.1 431"));
            s.stop();
        }

        #[test]
        fn loopback_callback_reaches_tx_and_stop_closes() {
            let (tx, rx) = mpsc::channel();
            let mut s = DemoAuthServer::start(
                OK_STATE,
                "chal",
                "code",
                None,
                Some(Box::new(move |u| {
                    let _ = tx.send(u);
                })),
            )
            .unwrap();
            let p = s.port();
            assert_eq!(s.redirect_uri(), format!("http://127.0.0.1:{p}/callback"));
            let r = get(
                p,
                &format!("/callback?state={OK_STATE}&code=code"),
                &format!("127.0.0.1:{p}"),
            );
            assert!(r.starts_with("HTTP/1.1 200"));
            assert_eq!(
                rx.recv().unwrap(),
                format!("http://127.0.0.1:{p}/callback?state={OK_STATE}&code=code")
            );
            s.stop();
            assert!(TcpStream::connect(("127.0.0.1", p)).is_err());
        }

        #[test]
        fn rejects_unsafe_embedded_values() {
            for (st, ch, co) in [
                ("zz", "c", "c"),
                (OK_STATE, "a\"b", "c"),
                (OK_STATE, "c", "<x>"),
            ] {
                assert!(DemoAuthServer::start(st, ch, co, None, None).is_err());
            }
        }
    }
}
