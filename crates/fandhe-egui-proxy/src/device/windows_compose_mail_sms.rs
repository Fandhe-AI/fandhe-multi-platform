//! Windows 向けメール・SMS 作成画面の実装（Issue #1384・#31）。`ShellExecuteW` で `mailto:` /
//! `sms:` URL を開き、既定のハンドラの作成画面を出す。`device/compose_mail_sms.rs` の Windows 経路から呼ばれ、
//! 契約は `fandhe_device::compose_mail_sms`。渡すのは `ComposeRequest::to_url` が組み立てた
//! `mailto:` / `sms:` URL の 2 種だけで、scheme は固定。任意の scheme・パス・引数を渡す経路は作らない。
//! 添付付きの要求は URL で渡せないため、Simple MAPI（`MAPISendMailW`）を作業スレッドで呼ぶ
//! （[`begin_attachment`]、Issue #1043・親 #156）。`compose` 自体は添付付きを `to_url` の
//! `Unsupported` で拒否したままにし、嘘の成功を作らない。
//! Windows 本体は `sms:` のハンドラを同梱しない。登録したアプリ（スマートフォン連携など）があれば
//! 開き、無ければ `SE_ERR_NOASSOC` → `NoHandler`。WinRT の `ChatMessageManager` は制限付き
//! capability とパッケージ ID が要るため採らない（#31）。
//!
//! 戻り値の写像は純関数 [`outcome_from_shell`] に置き、ホストでもテストできるよう
//! モジュールは `cfg(any(windows, test))`。`unsafe` は `ShellExecuteW` と、MAPI の `ffi` に閉じる。
//!
//! # 添付付き（Simple MAPI、Issue #1043）
//!
//! - `mapi32.dll` は `LoadLibraryExW(LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress` で解決する
//!   （DLL planting 対策、import table に載せない）。解決できなければ `Internal(MAPI)`
//! - フラグは `MAPI_DIALOG | MAPI_LOGON_UI`。作成画面は modal で利用者が閉じるまで戻らないため、
//!   専用スレッドで呼び、結果は `CompletionSender` 経由で届ける（UI スレッドを止めない）
//! - `SUCCESS_SUCCESS` は `HandedOff`、`MAPI_USER_ABORT` は `Cancelled`、それ以外は `Internal(MAPI)`
//!   （新しい Outlook の `MAPI_E_FAILURE` 等。`NoHandler` に読み替えない）
//! - 添付は関数が返る前にメッセージへコピーされる（公式）ので、戻った直後に一時ファイルを消す。
//!   結果を届ける前に在席フラグの解除と一時ファイルの破棄を済ませる
//! - 前の作成画面が開いている間の添付要求は `Internal(BUSY)`（プロセス内で 1 件）
//! - CC / BCC は `ulRecipClass` で渡す。宛先は `lpszName` = アドレス、`lpszAddress` = `SMTP:` + アドレス
//! - 作業スレッドは STA の COM を初期化して呼ぶ（要否が公式に無いため安全側。実機で確かめる）
//!
//! 既知の天井（ponytail）: `ulUIParam` は 0（UI スレッドの HWND を別スレッドの modal の親にすると
//! 入力が結合してデッドロックしうる）。作成画面はアプリの窓に従属しない。MIME type は MAPI に渡さず
//! （`lpFileType` は null）、受け手は拡張子で判断する。必要になれば `MapiFileTagExt` を足す。
//! ビルド確認はローカルでは gnu ターゲットまで（MSVC は CI）。
//!
//! # 既知の天井
//!
//! - 戻り値 `> 32` は「OS が受理した」だけで、メールアプリが開いたかは不明（`HandedOff`）
//! - `mailto:` の関連付けが無いと、Windows 10 以降は「アプリを選ぶ」を出して `> 32` を返すことが
//!   ある。`NoHandler` にならない場合があり、実機で確かめる（#1383）
//! - `sms:` も同じく、関連付けが無くても「アプリを選ぶ」等で `> 32` が返り `NoHandler` にならない
//!   ことがある。宛先が空の `sms:?body=` をハンドラが受けるかも実機で確かめる（#34）
//!   （ponytail: 事前に `AssocQueryStringW` で調べれば `NoHandler` を確実にできる。必要になったら足す）
//! - URL が 2083 文字を超える場合の挙動は OS 依存で、失敗は `Internal` に倒れる

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Instant;

use fandhe_device::compose_mail_sms::{ComposeOutcome, ComposeRequest, BUSY, MAPI};
use fandhe_device::DeviceError;

use super::compose_attachment_file::Store;

/// `SE_ERR_NOASSOC`（関連付けが無い）。
pub(crate) const SE_ERR_NOASSOC: isize = 31;

/// `ShellExecuteW` の戻り値 → 結果の写像（SSOT）。公式の表のとおり `> 32` が成功、
/// 31 が関連付け無し、それ以外（0〜32 の他のエラー値）は失敗。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_shell(code: isize) -> Result<ComposeOutcome, DeviceError> {
    match code {
        c if c > 32 => Ok(ComposeOutcome::HandedOff),
        SE_ERR_NOASSOC => Ok(ComposeOutcome::NoHandler),
        _ => Err(DeviceError::Internal("compose present")),
    }
}

/// `MAPISendMailW` の戻り値 → 結果の写像（SSOT）。0 が成功、1（`MAPI_USER_ABORT`）が利用者の取り消し、
/// それ以外（新しい Outlook の `MAPI_E_FAILURE` を含む）はすべて失敗。成功へ畳まない。
pub(crate) fn outcome_from_mapi(code: u32) -> Result<ComposeOutcome, DeviceError> {
    match code {
        0 => Ok(ComposeOutcome::HandedOff),
        1 => Ok(ComposeOutcome::Cancelled),
        _ => Err(DeviceError::Internal(MAPI)),
    }
}

/// `MapiRecipDescW.ulRecipClass`（`MAPI_TO` / `MAPI_CC` / `MAPI_BCC`）。
const CLASS_TO: u32 = 1;
const CLASS_CC: u32 = 2;
const CLASS_BCC: u32 = 3;

/// `MAPISendMailW` へ渡す文字列の所有者。全バッファは UTF-16 の NUL 終端で、呼び出し中だけ使う。
/// `ffi` が構造体を組む（`unsafe` を持ち込まないため、組み立て前の純データ）。
pub(crate) struct MapiMail {
    /// (class, name, address)
    pub(crate) recips: Vec<(u32, Vec<u16>, Vec<u16>)>,
    pub(crate) subject: Vec<u16>,
    pub(crate) body: Vec<u16>,
    pub(crate) path: Vec<u16>,
    pub(crate) file_name: Vec<u16>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_path(p: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn wide_path(p: &std::path::Path) -> Vec<u16> {
    wide(&p.to_string_lossy())
}

/// 検査済みの要求と一時ファイルのパスから MAPI へ渡す値を作る。件名・本文は trim しない。
/// 本文の改行は `\r\n` に正規化する（`to_url` と同じ）。添付が無ければ `None`。
pub(crate) fn mapi_mail(req: &ComposeRequest, path: &std::path::Path) -> Option<MapiMail> {
    let a = req.attachment()?;
    let recip = |class, addr: &String| (class, wide(addr), wide(&format!("SMTP:{addr}")));
    let recips = req
        .to()
        .iter()
        .map(|x| recip(CLASS_TO, x))
        .chain(req.cc().iter().map(|x| recip(CLASS_CC, x)))
        .chain(req.bcc().iter().map(|x| recip(CLASS_BCC, x)))
        .collect();
    Some(MapiMail {
        recips,
        subject: wide(req.subject()),
        body: wide(&req.body().replace("\r\n", "\n").replace('\n', "\r\n")),
        path: wide_path(path),
        file_name: wide(a.file_name()),
    })
}

fn lock(m: &Mutex<Store>) -> std::sync::MutexGuard<'_, Store> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 一時ファイルを用意 → MAPI 呼び出し → 破棄 → 結果の写像。ロックは modal の間握らない。
/// 破棄に失敗しても結果はそのまま返す（失敗は `Store` の `stale` に残り、次の要求の前に消し直す）。
pub(crate) fn run_attachment(
    req: &ComposeRequest,
    store: &Mutex<Store>,
    now: Instant,
    send: impl FnOnce(&mut MapiMail) -> u32,
) -> Result<ComposeOutcome, DeviceError> {
    let a = req
        .attachment()
        .ok_or(DeviceError::Unsupported("compose attachment"))?;
    let file = lock(store).prepare(a, now)?;
    let Some(mut mail) = mapi_mail(req, file.path()) else {
        lock(store).discard(file);
        return Err(DeviceError::Unsupported("compose attachment"));
    };
    let code = send(&mut mail);
    lock(store).discard(file);
    outcome_from_mapi(code)
}

/// 在席フラグ。`Drop` で解除する（panic・スレッド起動失敗でも残さない）。
struct BusyGuard(&'static AtomicBool);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 添付付きの作成画面を作業スレッドで開く。`deliver` は結果が決まった後に作業スレッドから 1 回だけ呼ぶ
/// （呼ぶ前に在席フラグの解除と一時ファイルの破棄を済ませる）。`Err` はスレッドを起動する前の拒否で、
/// `deliver` は呼ばれない。前の画面が開いている間は `Internal(BUSY)`。
pub(crate) fn begin_attachment_with(
    req: ComposeRequest,
    deliver: impl FnOnce(Result<ComposeOutcome, DeviceError>) + Send + 'static,
    busy: &'static AtomicBool,
    store: &'static Mutex<Store>,
    send: impl FnOnce(&mut MapiMail) -> u32 + Send + 'static,
) -> Result<JoinHandle<()>, DeviceError> {
    if req.attachment().is_none() {
        return Err(DeviceError::Unsupported("compose attachment"));
    }
    // 判定と更新を 1 操作で行う（契約表 2）。
    if busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(DeviceError::Internal(BUSY));
    }
    let guard = BusyGuard(busy);
    // スレッドを起動できなければ closure ごと guard が落ちて解除される。
    std::thread::Builder::new()
        .name("fandhe-compose-mapi".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_attachment(&req, store, Instant::now(), send)
            }))
            .unwrap_or(Err(DeviceError::Internal("compose present")));
            drop(guard);
            deliver(result);
        })
        .map_err(|_| DeviceError::Internal("compose present"))
}

/// 前の作成画面が開いている間 `true`（プロセス内で 1 件）。
#[cfg(target_os = "windows")]
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 添付付きの要求を本番の static で開く（`device/compose_mail_sms.rs` の `drive` から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn begin_attachment(
    req: ComposeRequest,
    deliver: impl FnOnce(Result<ComposeOutcome, DeviceError>) + Send + 'static,
) -> Result<(), DeviceError> {
    begin_attachment_with(
        req,
        deliver,
        &IN_FLIGHT,
        &super::compose_attachment_file::STORE,
        ffi::send,
    )
    .map(drop)
}

/// `mapi32.dll` の `MAPISendMailW` 呼び出し（`unsafe` はここに閉じる）。
#[cfg(target_os = "windows")]
mod ffi {
    use std::sync::OnceLock;

    use windows::core::{s, w, PWSTR};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    use windows::Win32::System::Mapi::{
        MapiFileDescW, MapiMessageW, MapiRecipDescW, MAPI_DIALOG, MAPI_LOGON_UI,
    };

    use super::MapiMail;

    /// `MAPI_E_FAILURE`（DLL を解決できないときもこれで返す）。
    const MAPI_E_FAILURE: u32 = 2;

    /// MS Learn の `LPMAPISENDMAILW` と同じ宣言。
    type SendMailW = unsafe extern "system" fn(usize, usize, *const MapiMessageW, u32, u32) -> u32;

    /// 解決した関数ポインタ。`FreeLibrary` はしない（`windows_media_playback.rs` と同じ判断）。
    static SEND_MAIL: OnceLock<Option<SendMailW>> = OnceLock::new();

    #[allow(unsafe_code)]
    fn resolve() -> Option<SendMailW> {
        // SAFETY: 固定の DLL 名を System32 からのみ読み込む（DLL planting 対策）。
        let module =
            unsafe { LoadLibraryExW(w!("mapi32.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()?;
        // SAFETY: `module` は上で得た有効なハンドル。名前は NUL 終端のリテラル。戻りの関数ポインタは
        // MS Learn の `MAPISendMailW` の宣言（`SendMailW` 型、いずれも system ABI）と同じ ABI・引数型へ変換する。
        let p = unsafe { GetProcAddress(module, s!("MAPISendMailW")) }?;
        Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SendMailW>(p) })
    }

    /// 呼び出しスレッドを STA にして、成功したときだけ解除する。
    struct Sta(bool);

    impl Sta {
        #[allow(unsafe_code)]
        fn new() -> Self {
            // SAFETY: 呼び出しスレッドへの初期化要求。失敗してもプロセス状態を壊さない。
            Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
        }
    }

    impl Drop for Sta {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: `new` の `CoInitializeEx` が成功したスレッドで 1 回だけ呼ぶ。
                unsafe { CoUninitialize() };
            }
        }
    }

    /// `MAPI_DIALOG | MAPI_LOGON_UI` で作成画面を開き、利用者が閉じるまで戻らない。生の戻り値を返す。
    #[allow(unsafe_code)]
    pub(super) fn send(mail: &mut MapiMail) -> u32 {
        let Some(f) = *SEND_MAIL.get_or_init(resolve) else {
            return MAPI_E_FAILURE;
        };
        let _sta = Sta::new();
        let mut recips: Vec<MapiRecipDescW> = mail
            .recips
            .iter_mut()
            .map(|(class, name, addr)| MapiRecipDescW {
                ulRecipClass: *class,
                lpszName: PWSTR(name.as_mut_ptr()),
                lpszAddress: PWSTR(addr.as_mut_ptr()),
                ..Default::default()
            })
            .collect();
        let mut file = MapiFileDescW {
            nPosition: u32::MAX, // 本文中の位置を指定しない
            lpszPathName: PWSTR(mail.path.as_mut_ptr()),
            lpszFileName: PWSTR(mail.file_name.as_mut_ptr()),
            ..Default::default()
        };
        let msg = MapiMessageW {
            lpszSubject: PWSTR(mail.subject.as_mut_ptr()),
            lpszNoteText: PWSTR(mail.body.as_mut_ptr()),
            nRecipCount: recips.len() as u32,
            lpRecips: recips.as_mut_ptr(),
            nFileCount: 1,
            lpFiles: &mut file,
            ..Default::default()
        };
        // SAFETY: `msg` が指す全バッファ（`mail` の NUL 終端 UTF-16、`recips`、`file`）はこの呼び出しの間
        // 生きており、MAPI は戻った後に保持しない（添付は戻る前にコピーされる）。`f` は MS Learn の
        // 宣言と一致する型。session / UI 親 / reserved は 0。
        unsafe { f(0, 0, &msg, MAPI_DIALOG | MAPI_LOGON_UI, 0) }
    }
}

#[cfg(target_os = "windows")]
struct WindowsComposeMailSms;

#[cfg(target_os = "windows")]
impl fandhe_device::compose_mail_sms::ComposeMailSms for WindowsComposeMailSms {
    fn compose(
        &self,
        req: &fandhe_device::compose_mail_sms::ComposeRequest,
    ) -> Result<ComposeOutcome, DeviceError> {
        outcome_from_shell(open_url(&req.to_url()?))
    }
}

/// `ShellExecuteW` を呼んで戻り値を返す。UI（STA）スレッドから呼ぶ。
/// `windows_web_auth_session` も既定ブラウザを開くために使う。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(crate) fn open_url(url: &str) -> isize {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let url = HSTRING::from(url);
    // SAFETY: `url` は NUL 終端の UTF-16 で、この呼び出しの間だけ生きる。`w!("open")` は
    // 静的な NUL 終端文字列。`ShellExecuteW` は引数を保持せず、UI スレッドから同期に呼ぶ。
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &url,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    result.0 as isize
}

/// Windows の `ComposeMailSms` 実装を返す（`device/compose_mail_sms.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::compose_mail_sms::ComposeMailSms> {
    Box::new(WindowsComposeMailSms)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;

    use super::super::compose_attachment_file::{fail_remove, test_store};

    fn att_req(to: &[&str], cc: &[&str], bcc: &[&str], body: &str) -> ComposeRequest {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        ComposeRequest::mail(&s(to), &s(cc), &s(bcc), " 件名 ", body)
            .unwrap()
            .with_attachment(
                fandhe_device::compose_mail_sms::ComposeAttachment::new(
                    "a.txt",
                    "text/plain",
                    b"x",
                )
                .unwrap(),
            )
            .unwrap()
    }

    fn leaked_store() -> &'static Mutex<Store> {
        Box::leak(Box::new(Mutex::new(test_store().0)))
    }

    fn leaked_flag() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    #[test]
    fn mapi_code_table() {
        assert_eq!(outcome_from_mapi(0).unwrap(), ComposeOutcome::HandedOff);
        assert_eq!(outcome_from_mapi(1).unwrap(), ComposeOutcome::Cancelled);
        for c in (2..=27).chain([u32::MAX]) {
            assert!(
                matches!(outcome_from_mapi(c), Err(DeviceError::Internal(l)) if l == MAPI),
                "{c}"
            );
        }
    }

    #[test]
    fn mapi_mail_keeps_classes_order_and_nul_terminates() {
        let r = att_req(&["a@b.c"], &["c@d.e"], &["f@g.h"], "x\ny\r\nz");
        let m = mapi_mail(&r, std::path::Path::new("/t/a.txt")).unwrap();
        let classes: Vec<u32> = m.recips.iter().map(|r| r.0).collect();
        assert_eq!(classes, [1, 2, 3]);
        assert_eq!(String::from_utf16_lossy(&m.recips[1].2), "SMTP:c@d.e\0");
        for v in m.recips.iter().flat_map(|r| [&r.1, &r.2]).chain([
            &m.subject,
            &m.body,
            &m.path,
            &m.file_name,
        ]) {
            assert_eq!(v.last(), Some(&0));
        }
        assert_eq!(String::from_utf16_lossy(&m.body), "x\r\ny\r\nz\0");
        assert_eq!(String::from_utf16_lossy(&m.subject), " 件名 \0");
        assert_eq!(String::from_utf16_lossy(&m.file_name), "a.txt\0");
        let plain = ComposeRequest::mail(&["a@b.c".to_string()], &[], &[], "", "").unwrap();
        assert!(mapi_mail(&plain, std::path::Path::new("x")).is_none());
    }

    #[test]
    fn run_attachment_deletes_file_for_every_result() {
        for code in [0, 1, 2] {
            let store = Mutex::new(test_store().0);
            let r = att_req(&["a@b.c"], &[], &[], "b");
            let mut seen = None;
            let out = run_attachment(&r, &store, Instant::now(), |m| {
                let p =
                    std::path::PathBuf::from(String::from_utf16_lossy(&m.path[..m.path.len() - 1]));
                assert!(p.is_file());
                seen = p.parent().map(|d| d.to_path_buf());
                code
            });
            assert_eq!(out.ok(), outcome_from_mapi(code).ok());
            assert!(!seen.unwrap().exists(), "{code}");
        }
    }

    #[test]
    fn run_attachment_stops_with_cleanup_when_leftover_cannot_be_removed() {
        let mut s = test_store().0;
        s.remove = fail_remove;
        let store = Mutex::new(s);
        let r = att_req(&["a@b.c"], &[], &[], "b");
        // 1 件目: 破棄に失敗しても結果は返り、stale に積まれる。
        assert_eq!(
            run_attachment(&r, &store, Instant::now(), |_| 1).ok(),
            Some(ComposeOutcome::Cancelled)
        );
        assert_eq!(lock(&store).stale.len(), 1);
        // 2 件目: 消せないので OS を呼ばない。
        let mut called = false;
        let out = run_attachment(&r, &store, Instant::now(), |_| {
            called = true;
            0
        });
        assert!(matches!(
            out,
            Err(DeviceError::Internal(l)) if l == fandhe_device::compose_mail_sms::CLEANUP
        ));
        assert!(!called);
    }

    #[test]
    fn second_request_while_open_is_busy_then_released_before_deliver() {
        let (busy, store) = (leaked_flag(), leaked_store());
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let (res_tx, res_rx) = mpsc::channel();
        let r = att_req(&["a@b.c"], &[], &[], "b");
        let h = begin_attachment_with(
            r.clone(),
            move |res| {
                res_tx.send((res, busy.load(Ordering::SeqCst))).unwrap();
            },
            busy,
            store,
            move |_| {
                hold_rx.recv().unwrap();
                1
            },
        )
        .unwrap();
        let second = begin_attachment_with(
            r.clone(),
            |_| panic!("not delivered"),
            busy,
            store,
            |_| panic!("send not called"),
        );
        assert!(matches!(second, Err(DeviceError::Internal(BUSY))));
        hold_tx.send(()).unwrap();
        let (res, busy_in_deliver) = res_rx.recv().unwrap();
        h.join().unwrap();
        assert_eq!(res.ok(), Some(ComposeOutcome::Cancelled));
        assert!(!busy_in_deliver);
        let third = begin_attachment_with(r, |_| {}, busy, store, |_| 0).unwrap();
        third.join().unwrap();
    }

    #[test]
    fn request_without_attachment_is_unsupported_and_leaves_flag_clear() {
        let busy = leaked_flag();
        let plain = ComposeRequest::mail(&["a@b.c".to_string()], &[], &[], "", "").unwrap();
        let r = begin_attachment_with(plain, |_| {}, busy, leaked_store(), |_| 0);
        assert!(matches!(r, Err(DeviceError::Unsupported(_))));
        assert!(!busy.load(Ordering::SeqCst));
    }

    #[test]
    fn shell_code_table() {
        assert_eq!(outcome_from_shell(33).unwrap(), ComposeOutcome::HandedOff);
        assert_eq!(outcome_from_shell(31).unwrap(), ComposeOutcome::NoHandler);
        for c in [0, 2, 5, 32, -1, isize::MIN] {
            assert!(outcome_from_shell(c).is_err(), "{c}");
        }
    }
}
