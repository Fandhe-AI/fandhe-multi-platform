//! Windows 向けカスタム URL scheme 受信（Issue #1216、親 #1116）。`driver::platform_url_scheme` の Windows 分岐と
//! `fandhe-gallery` の `main.rs`（起動時の [`win::startup`]）から使われる。
//! macOS の `apple_url_scheme.rs` と同じく、検査・保持は共通側の
//! `crate::contract::UrlInbox` に任せ、本モジュールは「Windows 固有の届き方」だけを持つ。
//!
//! # 届き方
//!
//! Windows には Info.plist に当たる宣言がないため、次の 3 つを自前で用意する。
//!
//! 1. **登録**（[`win::register`] / [`win::unregister`]）: `HKCU\Software\Classes\fandhe` に
//!    既定値 `URL:fandhe Protocol`・値 `URL Protocol`（空文字）・`shell\open\command` の既定値
//!    `"<exe>" "%1"` を書く。起動のたびに黙って書かず、型サンプル画面のボタン操作で登録・解除する。
//!    HKCU のみのため管理者権限は要らない
//! 2. **受信**: URL は起動引数で届く。[`parse_url_args`] が「ちょうど 1 引数・有効な Unicode」だけを
//!    検査候補にし、中身（scheme・長さ・制御文字）は `UrlInbox::receive_bytes` に任せる。Windows の引数解析は
//!    URL 中の `"` や引用符直前の `\` を消費して値を変えるため、引数解析前の生コマンドライン
//!    （`GetCommandLineW`）が [`launch_unaltered`] を満たさない起動は拒否として数える
//!    （`fandhe://a"b` が `fandhe://ab` として通るのを防ぐ）
//! 3. **起動済みの場合の転送**（[`win::startup`]）: 名前付き mutex で既存インスタンスを見つけ、
//!    named pipe で URL を送って新プロセスを終了する。pipe は先頭 1 バイトの種別（`U`=URL、
//!    `F`=ファイル。Issue #420 で file-association が相乗り。`FileRoute` の口で登録され、本 crate は file-association へ依存しない）
//!    で多重化する。
//!    サーバーは受理なら 1、拒否なら 0 を 1 バイトで返す。転送に失敗したら終了コード 1（[`exit_code`]）で、成功として扱わない
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順・不正 URL の非表示・コールドスタート分の保持・転送失敗を成功にしないこと・
//! pipe の先取り防止（`FILE_FLAG_FIRST_PIPE_INSTANCE`）・他セッションの pipe への送信拒否・リモート接続拒否。
//! 守らない: 同一ユーザー・同一セッションの別プロセスによる pipe の占有や妨害（同じ信頼境界）、
//! セッション間の転送、他アプリが同じ scheme を登録した場合の上書き。
//!
//! URL は秘密値を含みうるためログ・エラー文言へ出さない。`unsafe` は [`win`] に閉じ、各呼び出しに
//! `// SAFETY:` を付ける。純関数は全 OS でコンパイルしホストの `cargo test` で検査する。

use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;

use crate::contract::{InboxSnapshot, MAX_URL_BYTES, UrlInbox, UrlScheme};
use crate::error::UrlSchemeError;

/// 受信した URL の受け口（プロセス全体で 1 つ）。UI ができる前のコールドスタート分もここに溜まる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static INBOX: UrlInbox = UrlInbox::new();

/// 単一インスタンス判定の名前付き mutex（`Local\` はセッション単位）。
pub const MUTEX_NAME: &str = "Local\\fandhe-egui-proxy.url-scheme";
/// 登録先（`HKCU` 直下）。
pub const REG_KEY: &str = "Software\\Classes\\fandhe";
/// 転送の期限（ミリ秒）。サーバー起動直後・接続の合間に当たる場合の再試行を含む。
pub const FORWARD_DEADLINE_MS: u64 = 3000;

/// 転送の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forward {
    /// 既存インスタンスが受理した。
    Accepted,
    /// 既存インスタンスが検査で弾いた。
    Rejected,
    /// 届けられなかった（成功として扱わない）。
    Failed,
}

/// 転送結果から新プロセスの終了コードへ写す（受理 0・拒否 2・失敗 1）。
pub fn exit_code(f: Forward) -> i32 {
    match f {
        Forward::Accepted => 0,
        Forward::Rejected => 2,
        Forward::Failed => 1,
    }
}

/// セッションごとの pipe 名。
pub fn pipe_name(session_id: u32) -> String {
    format!("\\\\.\\pipe\\fandhe-egui-proxy.url-scheme.{session_id}")
}

/// 起動引数を解釈する。`None` は URL 起動ではない（通常起動・`--fandhe-background-task` 等）、
/// `Some(None)` は URL 起動だが不正（複数引数・非 Unicode。拒否として数える）、
/// `Some(Some(s))` は検査候補。scheme 等の中身の検査は `UrlInbox` に任せる。
pub fn parse_url_args(args: &[OsString]) -> Option<Option<String>> {
    let is_url = args.iter().any(|a| {
        a.to_string_lossy()
            .get(..7)
            .is_some_and(|h| h.eq_ignore_ascii_case("fandhe:"))
    });
    if !is_url {
        return None;
    }
    match args {
        [one] => Some(one.to_str().map(str::to_owned)),
        _ => Some(None),
    }
}

/// 生コマンドライン `raw` が「`<exe> "<arg>"`」そのままで、引数解析が `arg` を変形していないか。
/// `"` や引用符直前の `\` を含む URL は解析で値が変わり、再構成した `"<arg>"` と一致しなくなる。
pub fn launch_unaltered(raw: &str, arg: &str) -> bool {
    launch_unaltered_after(raw, "", arg)
}

/// [`launch_unaltered`] の、引数の前に目印 `flag`（例: `--fandhe-open-file`）が付く版。
/// 期待形は「`<exe> <flag> "<arg>"`」（`flag` が空なら「`<exe> "<arg>"`」）。
pub fn launch_unaltered_after(raw: &str, flag: &str, arg: &str) -> bool {
    let raw = raw.trim_start();
    let tail = match raw.strip_prefix('"') {
        Some(r) => r.split_once('"').map(|(_, t)| t),
        None => raw.split_once(char::is_whitespace).map(|(_, t)| t),
    };
    let want = if flag.is_empty() {
        format!("\"{arg}\"")
    } else {
        format!("{flag} \"{arg}\"")
    };
    tail.is_some_and(|t| t.trim() == want)
}

/// pipe の種別バイト: URL。
pub const KIND_URL: u8 = b'U';
/// pipe の種別バイト: ファイル（file-association が [`register_file_route`] で登録した経路）。
pub const KIND_FILE: u8 = b'F';

/// ファイル起動の経路。file-association（Windows）が [`register_file_route`] で 1 つだけ登録する。
///
/// 単一インスタンスの調停（mutex・pipe・生コマンドライン照合）は URL 起動と共通で本モジュールが持ち、
/// ファイル固有の解釈（目印の引数・検査・受け口）だけを file-association から受け取る。
/// 登録が無ければファイル起動は扱わない（URL 起動だけ。選ばなかった機能の経路を持たない）。
pub struct FileRoute {
    /// ファイル起動を示す目印の引数（例: `--fandhe-open-file`）。生コマンドラインの照合に使う。
    pub open_flag: &'static str,
    /// 起動引数を解釈する（`None` はファイル起動ではない、`Some(None)` は不正、`Some(Some(p))` は検査候補）。
    pub parse_args: fn(&[OsString]) -> Option<Option<String>>,
    /// 自プロセスの受け口へ入れる。受理なら `true`、拒否として数えたなら `false`。
    pub receive: fn(Option<&str>) -> bool,
    /// pipe で受け付ける本文（パス）の最大バイト数。
    pub max_body: usize,
}

static FILE_ROUTE: OnceLock<FileRoute> = OnceLock::new();

/// ファイル起動の経路を登録する。`startup` より前に 1 度だけ呼ぶ。登録済みなら `false`（先着を保つ）。
pub fn register_file_route(route: FileRoute) -> bool {
    FILE_ROUTE.set(route).is_ok()
}

fn file_route() -> Option<&'static FileRoute> {
    FILE_ROUTE.get()
}

/// 種別を除いた本文の上限（URL・登録されたファイル経路のうち大きい方）。
pub fn max_frame_body() -> usize {
    MAX_URL_BYTES.max(file_route().map_or(0, |r| r.max_body))
}

/// pipe へ実際に書くフレーム（先頭に種別、後ろに本文）。`None`・上限超過の本文は不正を示す
/// 1 バイト（0xFF は不正な UTF-8 のためサーバーで拒否として数えられる）に置き換える。長大な本文を
/// 同期 pipe へ全量書くと、受信バッファを超えた分でサーバーの読み取りと相互待ちになり得るため転送しない。
pub fn frame(kind: u8, payload: Option<&str>, max: usize) -> Vec<u8> {
    let mut v = vec![kind];
    match payload {
        Some(s) if s.len() <= max => v.extend_from_slice(s.as_bytes()),
        _ => v.push(0xFF),
    }
    v
}

/// フレームを種別と本文に分ける。空なら `None`（種別なし）。
pub fn split_frame(buf: &[u8]) -> Option<(u8, &[u8])> {
    buf.split_first().map(|(k, rest)| (*k, rest))
}

/// レジストリへ書く `shell\open\command` の値。`"` を含む exe パスは引数注入になりうるため拒否する。
pub fn register_command(exe: &Path) -> Result<String, UrlSchemeError> {
    let exe = exe
        .to_str()
        .filter(|s| !s.contains('"'))
        .ok_or(UrlSchemeError::InvalidInput("url_scheme"))?;
    Ok(format!("\"{exe}\" \"%1\""))
}

/// レジストリ操作が返した Win32 エラーコード。呼び出す機能（url-scheme・file-association）が
/// 自分のエラー型へ写す（種別の写像を 1 か所に集め、機能 crate のエラー型を共有しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryError(pub u32);

impl RegistryError {
    /// `ERROR_ACCESS_DENIED`（5）か。権限拒否として扱う唯一のコード。
    pub fn is_access_denied(self) -> bool {
        self.0 == 5
    }
}

impl From<RegistryError> for UrlSchemeError {
    fn from(e: RegistryError) -> Self {
        if e.is_access_denied() {
            UrlSchemeError::PermissionDenied("url_scheme")
        } else {
            UrlSchemeError::Internal("url_scheme register")
        }
    }
}

/// 登録・解除の差し込み口（effect ドライバのテストで偽物に替える）。
pub struct Registrar {
    /// 現在の exe を登録する。
    pub register: fn() -> Result<(), UrlSchemeError>,
    /// 登録を解除する（もともと未登録でも `Ok`）。
    pub unregister: fn() -> Result<(), UrlSchemeError>,
}

/// 実機のレジストリを操作する [`Registrar`]。
#[cfg(target_os = "windows")]
pub const SYSTEM_REGISTRAR: Registrar = Registrar {
    register: win::register,
    unregister: win::unregister,
};

/// Windows 向け `UrlScheme` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) struct WindowsUrlScheme;

impl UrlScheme for WindowsUrlScheme {
    fn snapshot(&self) -> Result<InboxSnapshot, UrlSchemeError> {
        Ok(INBOX.snapshot())
    }
}

/// Win32 呼び出しを閉じる（`unsafe` はこのサブモジュールに限る）。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod win {
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    use crate::contract::SCHEME;
    use crate::error::UrlSchemeError;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PIPE_BUSY,
        ERROR_PIPE_CONNECTED, ERROR_SUCCESS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows::Win32::System::Environment::GetCommandLineW;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
        WaitNamedPipeW,
    };
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
    };
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
    use windows::core::PCWSTR;

    use super::{
        FORWARD_DEADLINE_MS, FileRoute, Forward, INBOX, KIND_FILE, KIND_URL, MUTEX_NAME, REG_KEY,
        RegistryError, exit_code, file_route, frame, launch_unaltered, launch_unaltered_after,
        max_frame_body, parse_url_args, pipe_name, register_command, split_frame,
    };

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(crate) fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn session_of(pid: u32) -> Option<u32> {
        let mut session = 0u32;
        // SAFETY: `session` は呼び出し中有効な書き込み先。pid は値渡し。
        unsafe { ProcessIdToSessionId(pid, &mut session) }
            .ok()
            .map(|()| session)
    }

    fn own_session() -> Option<u32> {
        // SAFETY: 引数なしで現在プロセス ID を返すだけ。
        session_of(unsafe { GetCurrentProcessId() })
    }

    /// `payload`（`None` は不正な URL 起動）を自分の受け口へ入れる。
    fn receive_local(payload: Option<&str>) {
        // 戻り値の `Err` は拒否件数に反映済み（URL をログに出さない）。
        let _ = INBOX.receive_bytes(payload.map(str::as_bytes), SCHEME, now_ms());
    }

    /// 引数解析前の生コマンドラインで URL が変形されていないかを確かめる（取得できなければ拒否側）。
    fn raw_unaltered(url: &str) -> bool {
        // SAFETY: 引数なしで、プロセス存続中有効な NUL 終端文字列へのポインタを返す。
        let p = unsafe { GetCommandLineW() };
        if p.is_null() {
            return false;
        }
        // SAFETY: `p` は上記のとおり NUL 終端の有効な UTF-16 列。
        let raw = String::from_utf16_lossy(unsafe { p.as_wide() });
        launch_unaltered(&raw, url)
    }

    /// ファイル起動のパスが生コマンドラインと一致するか（`"` の消費などの変形を拒否。取得不能は拒否）。
    fn raw_unaltered_file(route: &FileRoute, path: &str) -> bool {
        // SAFETY: 引数なしで、プロセス存続中有効な NUL 終端文字列へのポインタを返す。
        let p = unsafe { GetCommandLineW() };
        if p.is_null() {
            return false;
        }
        // SAFETY: `p` は上記のとおり NUL 終端の有効な UTF-16 列。
        let raw = String::from_utf16_lossy(unsafe { p.as_wide() });
        launch_unaltered_after(&raw, route.open_flag, path)
    }

    /// 種別付きの起動（URL かファイル）。`None` 側のペイロードは不正な起動。
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Url,
        File,
    }

    fn receive_kind(kind: Kind, payload: Option<&str>) {
        match kind {
            Kind::Url => receive_local(payload),
            Kind::File => {
                // 拒否は件数に反映済み（パスをログに出さない）。経路が無ければ（file-association を
                // 選んでいないアプリ）ファイル起動は解釈されず、ここへは来ない。
                if let Some(route) = file_route() {
                    let _ = (route.receive)(payload);
                }
            }
        }
    }

    /// 起動直後に呼ぶ。既存インスタンスへ転送して終えるべきなら終了コードを返す
    /// （呼び出し側が `exit` する）。自分が受ける側なら `None`。
    pub fn startup(args: &[OsString]) -> Option<i32> {
        // URL 起動でなければファイル起動かを見る（目印の引数があるときだけ）。
        let parsed = match parse_url_args(args) {
            Some(p) => Some((Kind::Url, p.filter(|u| raw_unaltered(u)))),
            None => file_route().and_then(|route| {
                (route.parse_args)(args)
                    .map(|p| (Kind::File, p.filter(|f| raw_unaltered_file(route, f))))
            }),
        };
        let name = wide(MUTEX_NAME);
        // SAFETY: `name` は NUL 終端で呼び出し中有効。属性は既定（None）。
        let created = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
        // SAFETY: 直前の API 呼び出しのエラー値を読むだけ（間に別の Win32 呼び出しを挟まない）。
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let Ok(mutex) = created else {
            // 調停できない場合は自分が受ける側として続行する（実際に受けているので嘘の成功にならない）。
            if let Some((k, p)) = &parsed {
                receive_kind(*k, p.as_deref());
            }
            return None;
        };
        if already {
            // 受信側にならないため mutex を手放す（保持し続けると、先行インスタンス終了後も
            // 名前が残って後続が受信側になれない）。
            // SAFETY: `mutex` は直前の `CreateMutexW` が返した有効なハンドル。1 度だけ閉じる。
            unsafe {
                let _ = CloseHandle(mutex);
            }
            // 既存インスタンスあり。URL 起動でなければ従来どおり別ウィンドウで続行する。
            return parsed.map(|(k, p)| {
                let byte = if k == Kind::Url { KIND_URL } else { KIND_FILE };
                let result = forward(byte, p.as_deref());
                if result == Forward::Failed {
                    // URL・パスを含めない固定文言のみ。
                    eprintln!("[fandhe-egui-proxy] 起動済みインスタンスへの転送に失敗した。");
                }
                exit_code(result)
            });
        }
        // 最初のインスタンス。mutex ハンドルはプロセス終了（OS が解放）まで保持する。
        // 起動引数の記録は serve の spawn より前に済ませる（後から pipe で届く URL を
        // 古い起動 URL で上書きしないため）。
        if let Some((k, p)) = &parsed {
            receive_kind(*k, p.as_deref());
        }
        let _ = std::thread::Builder::new()
            .name("fandhe-url-scheme-pipe".into())
            .spawn(serve);
        None
    }

    /// 既存インスタンスの pipe へ種別付きで送り、応答を待つ。`payload` が `None` なら不正を示す 1 バイトを送る。
    fn forward(kind: u8, payload: Option<&str>) -> Forward {
        let Some(session) = own_session() else {
            return Forward::Failed;
        };
        let name = pipe_name(session);
        let wname = wide(&name);
        let deadline = Instant::now() + Duration::from_millis(FORWARD_DEADLINE_MS);
        let mut file = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
            {
                Ok(f) => break f,
                Err(e) if Instant::now() < deadline => {
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32) {
                        // SAFETY: `wname` は NUL 終端で呼び出し中有効。結果は再試行で確かめる。
                        let _ = unsafe { WaitNamedPipeW(PCWSTR(wname.as_ptr()), 100) };
                    } else if e.kind() == std::io::ErrorKind::NotFound {
                        std::thread::sleep(Duration::from_millis(100));
                    } else {
                        return Forward::Failed;
                    }
                }
                Err(_) => return Forward::Failed,
            }
        };
        // 他セッションが先に作った pipe へ URL を渡さない。
        let mut server_pid = 0u32;
        // SAFETY: ハンドルは `file` が保持する有効な pipe。`server_pid` は書き込み先として有効。
        let got =
            unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut server_pid) };
        if got.is_err() || session_of(server_pid) != Some(session) {
            return Forward::Failed;
        }
        let bytes = frame(kind, payload, max_frame_body());
        match file.write(&bytes) {
            Ok(n) if n == bytes.len() => {}
            _ => return Forward::Failed,
        }
        // ponytail: 応答待ちに期限を持たない。サーバーが応答せず固まる場合は新プロセスも待つ
        // （嘘の成功にはならない）。必要になったら重複 I/O で期限を付ける。
        let mut reply = [0u8; 1];
        match file.read(&mut reply) {
            Ok(1) if reply[0] == 1 => Forward::Accepted,
            Ok(1) => Forward::Rejected,
            _ => Forward::Failed,
        }
    }

    /// 受信側スレッド。pipe を 1 本作って使い回し（作り直す隙に先取りされない）、接続ごとに処理する。
    fn serve() {
        let Some(session) = own_session() else { return };
        let name = wide(&pipe_name(session));
        // SAFETY: `name` は NUL 終端で呼び出し中有効。属性は既定 DACL（他ユーザーは書き込めない）。
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                512,
                4096,
                0,
                None,
            )
        };
        // 先取りされた等で作れない場合はここで終える。後から来る転送は期限切れで失敗扱いになる。
        if pipe == INVALID_HANDLE_VALUE {
            return;
        }
        loop {
            // SAFETY: `pipe` は上で作った有効なハンドル（このスレッドが所有し閉じない）。
            let connected = unsafe { ConnectNamedPipe(pipe, None) };
            let ok = connected.is_ok()
                // SAFETY: 直前の呼び出しのエラー値を読むだけ。
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if ok {
                handle_connection(pipe);
            }
            // SAFETY: 同上。切断して次の接続を待てる状態に戻す。
            let _ = unsafe { DisconnectNamedPipe(pipe) };
            if !ok {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// 現在のメッセージの残りを読み捨てる（`ERROR_MORE_DATA` の間だけ続ける）。
    // ponytail: I/O 全体の期限は持たない。送信側が止まれば待つ（同一ユーザー・同一セッションの
    // 妨害は守備範囲外）。必要になったら重複 I/O で期限を付ける。
    fn drain_message(pipe: HANDLE) {
        let mut sink = [0u8; 1024];
        let mut read = 0u32;
        loop {
            // SAFETY: `pipe` は有効な接続済みハンドル。`sink` と `read` は呼び出し中有効。
            match unsafe { ReadFile(pipe, Some(&mut sink), Some(&mut read), None) } {
                Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => continue,
                _ => return,
            }
        }
    }

    fn handle_connection(pipe: HANDLE) {
        // 種別 1 バイト + 上限 + 1 バイトまで読む。超過分は検査で弾かれる。
        let mut buf = vec![0u8; 1 + max_frame_body() + 1];
        let mut read = 0u32;
        // SAFETY: `pipe` は有効な接続済みハンドル。`buf` と `read` は呼び出し中有効。
        let res = unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut read), None) };
        let n = match res {
            Ok(()) => read as usize,
            // メッセージが上限を超えた。残りを排出してから（送信側の write と応答待ちを
            // 解いてから）、バッファ全体を渡して長さ超過で拒否させる。
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => {
                drain_message(pipe);
                buf.len()
            }
            Err(_) => return,
        };
        // 種別なし・不明な種別は URL の拒否として数える。
        let accepted = match split_frame(&buf[..n]) {
            // 経路が登録されていなければ拒否（ファイル起動を扱わないアプリへ届いた転送は成功にしない）。
            Some((KIND_FILE, body)) => {
                file_route().is_some_and(|r| (r.receive)(std::str::from_utf8(body).ok()))
            }
            Some((KIND_URL, body)) => INBOX
                .receive_bytes((!body.is_empty()).then_some(body), SCHEME, now_ms())
                .is_ok(),
            _ => INBOX.receive_bytes(None, SCHEME, now_ms()).is_ok(),
        };
        // 同期 I/O（`OVERLAPPED` なし）の `WriteFile` は書き込みバイト数の受け取り先が必須。
        let mut written = 0u32;
        // SAFETY: `pipe` は有効。応答は 1 バイトで、書き込み中有効なスライス。`written` も有効。
        unsafe {
            let _ = WriteFile(pipe, Some(&[u8::from(accepted)]), Some(&mut written), None);
            let _ = FlushFileBuffers(pipe);
        }
    }

    /// 開いたレジストリキーを全経路で閉じる。
    pub struct KeyGuard(HKEY);
    impl Drop for KeyGuard {
        fn drop(&mut self) {
            // SAFETY: `self.0` は `RegCreateKeyExW` が返した有効なキー。1 度だけ閉じる。
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub fn open_key(subkey: &str) -> Result<KeyGuard, RegistryError> {
        let sub = wide(subkey);
        let mut key = HKEY::default();
        // SAFETY: `sub` は NUL 終端で呼び出し中有効。`key` は書き込み先として有効。
        let err = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            return Err(RegistryError(err.0));
        }
        Ok(KeyGuard(key))
    }

    /// UTF-16 + NUL 終端のバイト列（`REG_SZ` の値）。
    pub fn utf16_bytes(value: &str) -> Vec<u8> {
        wide(value).iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    /// 値を書く。`name` が `None` なら既定値。`REG_NONE` は空の `data` を渡す。
    pub fn set_value(
        key: &KeyGuard,
        name: Option<&str>,
        ty: REG_VALUE_TYPE,
        data: &[u8],
    ) -> Result<(), RegistryError> {
        let wname = name.map(wide);
        let pname = wname
            .as_ref()
            .map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr()));
        // SAFETY: `pname` は null（既定値）または NUL 終端で呼び出し中有効。`data` も同様。
        let err = unsafe { RegSetValueExW(key.0, pname, None, ty, Some(data)) };
        if err != ERROR_SUCCESS {
            return Err(RegistryError(err.0));
        }
        Ok(())
    }

    fn set_str(key: &KeyGuard, name: Option<&str>, value: &str) -> Result<(), UrlSchemeError> {
        Ok(set_value(key, name, REG_SZ, &utf16_bytes(value))?)
    }

    /// 現在の exe を `fandhe://` の起動先として登録する。途中で失敗したら `Err`（巻き戻しはしない）。
    // ponytail: 他アプリの `fandhe` 登録を上書きしうる（scheme は本フレームワーク固有として受容）。
    // exe を移動したら再登録が要る。
    pub fn register() -> Result<(), UrlSchemeError> {
        let exe =
            std::env::current_exe().map_err(|_| UrlSchemeError::Internal("url_scheme register"))?;
        let command = register_command(&exe)?;
        let root = open_key(REG_KEY)?;
        set_str(&root, None, "URL:fandhe Protocol")?;
        set_str(&root, Some("URL Protocol"), "")?;
        let cmd = open_key(&format!("{REG_KEY}\\shell\\open\\command"))?;
        set_str(&cmd, None, &command)
    }

    /// 登録を解除する。もともと未登録なら `Ok`。
    pub fn unregister() -> Result<(), UrlSchemeError> {
        let sub = wide(REG_KEY);
        // SAFETY: `sub` は NUL 終端で呼び出し中有効。
        let err = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr())) };
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(RegistryError(err.0).into())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 実 pipe で転送を往復させる（Windows 上でだけ走る）。
        #[test]
        fn forward_round_trip_accept_and_reject() {
            std::thread::spawn(serve);
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(forward(KIND_URL, Some("fandhe://hello")), Forward::Accepted);
            assert_eq!(forward(KIND_URL, Some("https://x")), Forward::Rejected);
            assert_eq!(forward(KIND_URL, None), Forward::Rejected);
            // ファイルの拒否は別の受け口に数えられ、URL 側の件数は変わらない。
            assert_eq!(forward(KIND_FILE, Some("C:\\a.md")), Forward::Rejected);
            assert_eq!(
                forward(KIND_FILE, Some("\\\\server\\s\\a.txt")),
                Forward::Rejected
            );
            let snap = INBOX.snapshot();
            assert_eq!(snap.last.map(|r| r.url).as_deref(), Some("fandhe://hello"));
            assert_eq!(snap.rejected, 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_url_args_classifies_launches() {
        assert_eq!(parse_url_args(&args(&[])), None);
        assert_eq!(parse_url_args(&args(&["--foo"])), None);
        assert_eq!(parse_url_args(&args(&["--fandhe-background-task"])), None);
        assert_eq!(
            parse_url_args(&args(&["fandhe://a"])),
            Some(Some("fandhe://a".into()))
        );
        assert_eq!(
            parse_url_args(&args(&["FANDHE://a"])),
            Some(Some("FANDHE://a".into()))
        );
        // 引数が割れた・他の引数が混ざった場合は拒否側へ倒す。
        assert_eq!(parse_url_args(&args(&["fandhe://a", "b"])), Some(None));
        assert_eq!(parse_url_args(&args(&["x", "fandhe://a"])), Some(None));
    }

    #[cfg(windows)]
    #[test]
    fn parse_url_args_rejects_non_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let mut w: Vec<u16> = "fandhe://".encode_utf16().collect();
        w.push(0xD800);
        assert_eq!(parse_url_args(&[OsString::from_wide(&w)]), Some(None));
    }

    #[test]
    fn launch_unaltered_after_rejects_consumed_quote() {
        let f = "--fandhe-open-file";
        assert!(launch_unaltered_after(
            "\"app.exe\" --fandhe-open-file \"C:/ab.txt\"",
            f,
            "C:/ab.txt"
        ));
        // `"C:/a"b.txt` は解析で `C:/ab.txt` になるが、生コマンドラインとは一致しない。
        assert!(!launch_unaltered_after(
            "\"app.exe\" --fandhe-open-file \"C:/a\"b.txt\"",
            f,
            "C:/ab.txt"
        ));
    }

    #[test]
    fn launch_unaltered_rejects_quote_and_trailing_backslash() {
        let ok = |raw: &str, a: &str| launch_unaltered(raw, a);
        assert!(ok("\"C:\\a b\\app.exe\" \"fandhe://a\"", "fandhe://a"));
        // 引用符入り: 解析後は `fandhe://ab`。
        assert!(!ok("\"app.exe\" \"fandhe://a\"b\"", "fandhe://ab"));
        // 末尾バックスラッシュ: 解析後は `fandhe://a"`。
        assert!(!ok("\"app.exe\" \"fandhe://a\\\"", "fandhe://a\""));
        assert!(!ok("\"app.exe\"", "fandhe://a"));
    }

    #[test]
    fn frame_prefixes_kind_and_replaces_oversize_and_none() {
        assert_eq!(frame(KIND_URL, Some("fandhe://a"), 2048), b"Ufandhe://a");
        assert_eq!(frame(KIND_FILE, Some("C:\\a.txt"), 2048), b"FC:\\a.txt");
        assert_eq!(frame(KIND_URL, Some(&"a".repeat(2049)), 2048), [b'U', 0xFF]);
        assert_eq!(frame(KIND_FILE, None, 2048), [b'F', 0xFF]);
        // ファイル経路が未登録なら URL の上限だけ。
        assert_eq!(max_frame_body(), MAX_URL_BYTES);
    }

    #[test]
    fn split_frame_separates_kind() {
        assert_eq!(split_frame(b""), None);
        assert_eq!(split_frame(b"U"), Some((b'U', &b""[..])));
        assert_eq!(split_frame(b"Fab"), Some((b'F', &b"ab"[..])));
        assert_eq!(split_frame(b"Zab"), Some((b'Z', &b"ab"[..])));
    }

    #[test]
    fn register_command_quotes_and_rejects_quote_in_path() {
        assert_eq!(
            register_command(Path::new("C:\\a b\\app.exe")).unwrap(),
            "\"C:\\a b\\app.exe\" \"%1\""
        );
        assert!(register_command(Path::new("C:\\a\"b.exe")).is_err());
    }

    #[test]
    fn pipe_name_exit_code_and_error_mapping() {
        assert_eq!(pipe_name(3), "\\\\.\\pipe\\fandhe-egui-proxy.url-scheme.3");
        assert_eq!(exit_code(Forward::Accepted), 0);
        assert_eq!(exit_code(Forward::Rejected), 2);
        assert_eq!(exit_code(Forward::Failed), 1);
        assert!(matches!(
            UrlSchemeError::from(RegistryError(5)),
            UrlSchemeError::PermissionDenied("url_scheme")
        ));
        assert!(matches!(
            UrlSchemeError::from(RegistryError(2)),
            UrlSchemeError::Internal("url_scheme register")
        ));
        assert!(RegistryError(5).is_access_denied());
        assert!(!RegistryError(2).is_access_denied());
    }
}
