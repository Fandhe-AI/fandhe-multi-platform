//! macOS 向けメール・SMS 作成画面の実装（Issue #1384）。`macos/fandhe_macos_compose_mail_sms.m`
//! （`NSWorkspace openURL:`、`build.rs` が macOS ターゲットの feature `device` 有効時にコンパイル
//! する）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を
//! 明記する。`device/compose_mail_sms.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_device::compose_mail_sms`。URL は `ComposeRequest::to_url`（scheme は `mailto:` /
//! `sms:` 固定）で組み立て済みのものだけを渡す。
//! 添付付きの要求（Issue #1042）は URL で渡せないため、`NSSharingService`（composeEmail）へ To・件名・
//! 本文・添付だけを渡す（[`compose_attachment`]）。CC / BCC 付きは渡す API が無く、黙って落とさず
//! `Unsupported("compose cc")`。一時ファイルは `compose_attachment_file` が持ち、`HandedOff` では
//! 受け手が後から読むため残して 10 分で回収、それ以外は即消す。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 成功は `HandedOff`（OS が URL を受理した）に留める。メールアプリが開いたか・送ったかは不明
//! - `sms:`（宛先なし含む）を Messages が受けるかは Mac 目視で確認する（#1383）
//! - 添付: `recipients` / `subject` の効き方、Mail 以外が既定のときの `canPerformWithItems:` は
//!   Mac 目視で確認する（#1039）。完了の通知は待たない（取り消しを知れない）

use std::path::Path;
use std::time::Instant;

use fandhe_device::compose_mail_sms::{ComposeAttachment, ComposeOutcome, ComposeRequest};
use fandhe_device::DeviceError;

use super::compose_attachment_file::Store;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = 受理 / 1 = 扱うアプリ無し（`openURL:` が NO）/ 2 = NSURL を作れない・main thread 外。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<ComposeOutcome, DeviceError> {
    match code {
        0 => Ok(ComposeOutcome::HandedOff),
        1 => Ok(ComposeOutcome::NoHandler),
        2 => Err(DeviceError::Internal("compose present")),
        _ => Err(DeviceError::Internal("compose outcome")),
    }
}

/// 添付付きのメールを開く。ファイルを作る前に CC / BCC を拒否し、`open` の結果で一時ファイルを
/// 残す（`HandedOff`）か消す（それ以外）。`open` は (To を `\n` 連結, 件名, 本文, パス) を受けて
/// `.m` の結果コードを返す（本番は `ffi::compose_attachment`、テストは偽物）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn compose_attachment(
    req: &ComposeRequest,
    a: &ComposeAttachment,
    store: &mut Store,
    now: Instant,
    open: impl FnOnce(&str, &str, &str, &Path) -> i32,
) -> Result<ComposeOutcome, DeviceError> {
    // NSSharingService に CC / BCC を渡す API は無い。黙って落とさない。
    if !req.cc().is_empty() || !req.bcc().is_empty() {
        return Err(DeviceError::Unsupported("compose cc"));
    }
    let file = store.prepare(a, now)?;
    let result = outcome_from_code(open(
        &req.to().join("\n"),
        req.subject(),
        req.body(),
        file.path(),
    ));
    if matches!(result, Ok(ComposeOutcome::HandedOff)) {
        store.keep(file, now);
    } else {
        store.discard(file);
    }
    result
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    // SAFETY: `macos/fandhe_macos_compose_mail_sms.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_compose_open_url(url: *const std::ffi::c_char) -> i32;
        fn fandhe_macos_compose_attachment(
            to_nl: *const std::ffi::c_char,
            subject: *const std::ffi::c_char,
            body: *const std::ffi::c_char,
            path: *const std::ffi::c_char,
        ) -> i32;
    }

    /// 結果コードを返す。`CString` を作れなければ 2（NSURL を作れないのと同じ扱い）。
    #[allow(unsafe_code)]
    pub(super) fn open_url(url: &str) -> i32 {
        let Ok(c) = CString::new(url) else {
            return 2;
        };
        // SAFETY: `c` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを呼び出し中に
        // だけ読み、保持しない。
        unsafe { fandhe_macos_compose_open_url(c.as_ptr()) }
    }

    /// 添付付きの作成画面を依頼する。`CString` を作れなければ 2。
    #[allow(unsafe_code)]
    pub(super) fn compose_attachment(to_nl: &str, subject: &str, body: &str, path: &Path) -> i32 {
        let (Ok(t), Ok(s), Ok(b), Ok(p)) = (
            CString::new(to_nl),
            CString::new(subject),
            CString::new(body),
            CString::new(path.as_os_str().as_bytes()),
        ) else {
            return 2;
        };
        // SAFETY: 4 つの `CString` はこの呼び出しの間だけ生きる NUL 終端文字列。`.m` はポインタを
        // 呼び出し中にだけ読み（`NSString` / `NSURL` へコピー）、保持しない。main thread 外なら 2。
        unsafe { fandhe_macos_compose_attachment(t.as_ptr(), s.as_ptr(), b.as_ptr(), p.as_ptr()) }
    }
}

#[cfg(target_os = "macos")]
struct MacComposeMailSms;

#[cfg(target_os = "macos")]
impl fandhe_device::compose_mail_sms::ComposeMailSms for MacComposeMailSms {
    fn compose(
        &self,
        req: &fandhe_device::compose_mail_sms::ComposeRequest,
    ) -> Result<ComposeOutcome, DeviceError> {
        // ponytail: UI スレッドで同期に呼ぶ（`performWithItems:` は作成画面を出すだけで戻る想定）。
        // 入れ子の run loop が見つかったら `dispatch_async(main)` へ移す。
        match req.attachment() {
            None => outcome_from_code(ffi::open_url(&req.to_url()?)),
            Some(a) => compose_attachment(
                req,
                a,
                &mut super::compose_attachment_file::lock(),
                Instant::now(),
                ffi::compose_attachment,
            ),
        }
    }
}

/// macOS の `ComposeMailSms` 実装を返す（`device/compose_mail_sms.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::compose_mail_sms::ComposeMailSms> {
    Box::new(MacComposeMailSms)
}

#[cfg(test)]
mod tests {
    use super::super::compose_attachment_file::{fail_remove, test_store};
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    fn req(cc: bool) -> ComposeRequest {
        let cc: &[&str] = if cc { &["c@d.e"] } else { &[] };
        ComposeRequest::mail(&["a@b.c", "x@y.z"], cc, &[], " 件名 ", " 本文 ")
            .unwrap()
            .with_attachment(att())
            .unwrap()
    }

    fn att() -> ComposeAttachment {
        ComposeAttachment::new("fandhe-sample.txt", "text/plain", b"hi\n").unwrap()
    }

    type Calls = Vec<(String, String, String, bool)>;

    /// `open` が受けた引数と呼び出しを記録して `code` を返す。
    fn run(code: i32, cc: bool) -> (Result<ComposeOutcome, DeviceError>, Calls, Store, PathBuf) {
        let (mut st, root) = test_store();
        let calls = RefCell::new(Vec::new());
        let r = compose_attachment(&req(cc), &att(), &mut st, Instant::now(), |t, s, b, p| {
            calls
                .borrow_mut()
                .push((t.to_string(), s.to_string(), b.to_string(), p.exists()));
            code
        });
        (r, calls.into_inner(), st, root)
    }

    #[test]
    fn cc_attachment_is_unsupported_without_open_or_file() {
        let (r, calls, st, root) = run(0, true);
        assert!(matches!(r, Err(DeviceError::Unsupported("compose cc"))));
        assert!(calls.is_empty() && st.kept.is_empty() && !root.exists());
    }

    #[test]
    fn handed_off_keeps_file_and_passes_values_untrimmed() {
        let (r, calls, st, root) = run(0, false);
        assert!(matches!(r, Ok(ComposeOutcome::HandedOff)));
        assert_eq!(
            calls,
            vec![(
                "a@b.c\nx@y.z".into(),
                " 件名 ".into(),
                " 本文 ".into(),
                true
            )]
        );
        assert_eq!(st.kept.len(), 1);
        assert!(st.kept[0].0.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_handler_and_failure_remove_file() {
        for (code, ok) in [(1, true), (2, false), (9, false)] {
            let (r, calls, st, root) = run(code, false);
            assert_eq!(r.is_ok(), ok);
            assert!(!matches!(r, Ok(ComposeOutcome::HandedOff)));
            assert_eq!(calls.len(), 1);
            assert!(st.kept.is_empty() && st.stale.is_empty());
            // 作った `<pid>-<seq>` ディレクトリは残らない。
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn stale_cleanup_failure_blocks_open() {
        let (mut st, root) = test_store();
        st.stale.push(root.join("left-over"));
        st.remove = fail_remove;
        let n = RefCell::new(0);
        let r = compose_attachment(
            &req(false),
            &att(),
            &mut st,
            Instant::now(),
            |_, _, _, _| {
                *n.borrow_mut() += 1;
                0
            },
        );
        assert!(
            matches!(r, Err(DeviceError::Internal(l)) if l == fandhe_device::compose_mail_sms::CLEANUP)
        );
        assert_eq!(*n.borrow(), 0);
    }

    #[test]
    fn code_table() {
        assert_eq!(outcome_from_code(0).unwrap(), ComposeOutcome::HandedOff);
        assert_eq!(outcome_from_code(1).unwrap(), ComposeOutcome::NoHandler);
        for code in [2, 3, -1, i32::MAX] {
            assert!(outcome_from_code(code).is_err());
        }
    }
}
