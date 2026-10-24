//! アプリ内ブラウザの抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/in-app-browser.json`、
//! 親 #135、Issue #701）。
//!
//! 扱う範囲は「固定の公開 URL を OS のアプリ内ブラウザ（iOS は `SFSafariViewController`、
//! Android は Custom Tabs）で 1 件開き、利用者が閉じて戻ったことを知る」こと。閲覧内容・遷移先・
//! 読み込みの成否は扱わない。対象は iOS・Android のみで、macOS・Windows は `Unsupported`、
//! Web は対象外（`n-a`）。`fandhe-egui-proxy` の effect ドライバ（`device/in_app_browser.rs`）が
//! 本モジュールの trait を呼び、OS 層（#702 iOS・#703 Android）が実装する。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「閉じた」（[`InAppBrowserOutcome::Closed`]。利用者が閉じて戻った。
//!   読み込みの成功は意味しない）/ 開けなかった（[`InAppBrowserFailure`] の理由別）/ 非対応
//!   （[`InAppBrowserFailure::Unsupported`]）。利用者が閉じたことは失敗にしない
//! - **開けなかった理由**（写像表）:
//!
//! | 状況 | iOS | Android | 返す値 |
//! | --- | --- | --- | --- |
//! | OS が非対応 | — | — | `Unsupported` |
//! | URL が検査に通らない | — | — | `InvalidUrl`（[`validate_url`]） |
//! | 表示先がない | 最前面の View Controller が取れない | 前面の Activity がない | `NoPresenter` |
//! | ブラウザがない | — | `ActivityNotFoundException` | `NoBrowser` |
//! | 開くまでの期限切れ | Rust の期限 | Rust の期限 | `TimedOut` |
//! | 表示前の中断 | 背景遷移 | Activity の再生成・終了 | `Interrupted` |
//! | それ以外・未知のコード | 他のエラー | 他の例外 | `Internal` |
//!
//! - **URL 検査**（SSOT は [`validate_url`]。OS 層でも scheme を再確認する）: scheme が `http` /
//!   `https`（大文字小文字を区別しない）、直後に `://`、authority が空でない、userinfo（`@`）なし、
//!   ホストは ASCII の英数字・`-`・`.` または `[...]` の IPv6 リテラル、ポートは任意で 1〜65535、
//!   全体が ASCII の印字可能文字（空白・制御文字・`\` なし）、長さは
//!   [`crate::url_scheme::MAX_URL_BYTES`] 以下
//! - **期限**（契約表 3）: 開くまでの [`OPEN_DEADLINE`]。起点は要求を観測した時の `Instant`
//!   （単調時計）。閉じるまでの時間には期限を持たない。期限切れと表示開始は同じロックで排他にし、
//!   期限切れを確定した後は OS に表示を始めさせない
//! - **同時 1 件**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と画面の世代の
//!   両方で捨てる
//! - **「閉じた」の条件**（契約表 6）: 表示開始を記録した要求にだけ認める。表示前に届いた「閉じた」は
//!   `Internal` にする
//! - **秘密値**（契約表 8）: URL と OS のエラー文をログ・画面・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、期限切れ・中断後に表示を
//! 始めないこと、URL と OS のエラー文を出さないこと。
//! 守らない範囲: 開いた後に Rust から閉じること（取り消しは置かない）、閲覧内容・遷移先・読み込み
//! 失敗の検知、Custom Tabs 非対応ブラウザでの表示の差、複数プロセス、プロセス死後の結果。

use std::fmt;
use std::time::Duration;

use crate::url_scheme::MAX_URL_BYTES;

/// 要求を観測してからアプリ内ブラウザの表示を始めるまでの期限（Rust のドライバだけが持つ）。
/// 仮の値で、実機確認で見直す。
pub const OPEN_DEADLINE: Duration = Duration::from_secs(10);

/// 検査済みの URL。[`validate_url`] を通ったものだけが作れる。`Debug` は中身を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct BrowserUrl(String);

impl BrowserUrl {
    /// OS へ渡す URL 文字列。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BrowserUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrowserUrl(<redacted>)")
    }
}

/// 成功側の結果。「閉じた」だけで、読み込みの成否は含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InAppBrowserOutcome {
    /// 利用者がアプリ内ブラウザを閉じてアプリへ戻った。
    Closed,
}

/// 開けなかった理由（OS のエラー文・URL は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InAppBrowserFailure {
    /// 非対応 OS。
    Unsupported,
    /// URL が検査に通らない。
    InvalidUrl,
    /// 表示先（View Controller・Activity）がない。
    NoPresenter,
    /// 開けるブラウザがない。
    NoBrowser,
    /// 開くまでの期限切れ。
    TimedOut,
    /// 表示前の中断（背景遷移・Activity 再生成・終了）。
    Interrupted,
    /// それ以外・未知のコード。
    Internal,
}

/// ASCII 印字可能文字か（空白・制御文字・`\` を除く）。
fn is_plain_ascii(c: char) -> bool {
    c.is_ascii_graphic() && c != '\\'
}

/// 開く URL を検査する（契約は本モジュール doc）。通ったものだけが [`BrowserUrl`] になる。
pub fn validate_url(raw: &str) -> Result<BrowserUrl, InAppBrowserFailure> {
    let bad = InAppBrowserFailure::InvalidUrl;
    if raw.is_empty() || raw.len() > MAX_URL_BYTES || !raw.chars().all(is_plain_ascii) {
        return Err(bad);
    }
    let (scheme, rest) = raw.split_once("://").ok_or(bad)?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return Err(bad);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains('@') {
        return Err(bad);
    }
    let port = if let Some(v6) = authority.strip_prefix('[') {
        let (inner, after) = v6.split_once(']').ok_or(bad)?;
        // 構文まで検証する（`[:]` や `[1:2:3]` のような不正リテラルを弾く）。
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(bad);
        }
        match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or(bad)?),
        }
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        let host_ok = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        if !host_ok {
            return Err(bad);
        }
        port
    };
    if let Some(p) = port {
        let valid = !p.is_empty()
            && p.len() <= 5
            && p.chars().all(|c| c.is_ascii_digit())
            && matches!(p.parse::<u32>(), Ok(1..=65535));
        if !valid {
            return Err(bad);
        }
    }
    Ok(BrowserUrl(raw.to_string()))
}

/// アプリ内ブラウザの OS 実装。`open` は「受理」だけを同期で返し、閉じた結果などは OS 層が
/// `fandhe-egui-proxy` の bridge（`complete_from_os`）へ非同期に届ける。
/// 呼び出し側（effect ドライバ）は [`validate_url`] を通した URL だけを渡す。
pub trait InAppBrowser: Send + Sync {
    /// `request_id` の要求として `url` の表示を OS へ依頼する。
    /// 表示の直前に bridge の `try_begin_present` を呼び、`false` なら表示しない。
    fn open(&self, request_id: u64, url: &BrowserUrl) -> Result<(), InAppBrowserFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedInAppBrowser;

impl InAppBrowser for UnsupportedInAppBrowser {
    fn open(&self, _request_id: u64, _url: &BrowserUrl) -> Result<(), InAppBrowserFailure> {
        Err(InAppBrowserFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) {
        assert!(validate_url(s).is_ok(), "通るはず: {s}");
    }
    fn ng(s: &str) {
        assert_eq!(
            validate_url(s).err(),
            Some(InAppBrowserFailure::InvalidUrl),
            "弾くはず: {s}"
        );
    }

    #[test]
    fn accepts_plain_http_and_https() {
        ok("https://example.com/");
        ok("HTTP://Example.com");
        ok("https://example.com:8443/a?b=c#d");
        ok("https://[::1]:443/x");
        ok("https://[2001:db8::1]/");
        ok("https://192.168.0.1:65535");
    }

    #[test]
    fn rejects_other_schemes() {
        for s in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "intent://x#Intent;end",
            "ftp://example.com",
            "mailto:a@b.c",
            "example.com",
            "//example.com",
        ] {
            ng(s);
        }
    }

    #[test]
    fn rejects_userinfo_and_empty_authority() {
        ng("https://user@example.com/");
        ng("https://user:pw@example.com/");
        ng("https://example.com@evil.test/");
        ng("https:///path");
        ng("https://");
        ng("https://:443/");
        ng("https://?q=1");
    }

    #[test]
    fn rejects_bad_host_port_chars() {
        ng("https://exa mple.com/");
        ng("https://example.com/a b");
        ng("https://example.com\\@evil.test/");
        ng("https://example.com/\n");
        ng("https://exa_mple.com/");
        ng("https://例え.jp/");
        ng("https://example.com:/");
        ng("https://example.com:0/");
        ng("https://example.com:65536/");
        ng("https://example.com:80a/");
        ng("https://example.com:123456/");
        ng("https://[::1/");
        ng("https://[]/");
        ng("https://[zz]/");
        ng("https://[::1]x/");
        ng("https://[abcd]/");
        ng("https://[:]/");
        ng("https://[1:2:3]/");
        ng("https://[1:2:3:4:5:6:7:8:9]/");
        ng("");
    }

    #[test]
    fn rejects_overlong_url() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        ng(&long);
        let exact = format!(
            "https://example.com/{}",
            "a".repeat(MAX_URL_BYTES - "https://example.com/".len())
        );
        ok(&exact);
    }

    #[test]
    fn debug_hides_url() {
        let u = validate_url("https://secret.example/token").unwrap();
        let d = format!("{u:?}");
        assert!(!d.contains("secret") && !d.contains("token"), "{d}");
        assert_eq!(u.as_str(), "https://secret.example/token");
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = validate_url("https://example.com/").unwrap();
        assert_eq!(
            UnsupportedInAppBrowser.open(1, &u),
            Err(InAppBrowserFailure::Unsupported)
        );
    }
}
