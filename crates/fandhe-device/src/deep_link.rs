//! ディープリンク（Universal Links / App Links）の受信（Phase 11 の拡充ループ、親 #153、
//! Issue #1278）。
//!
//! 扱う範囲は「検証済みドメインの https URL を検査し、最新 1 件を保持して画面が読める
//! ようにする」ところまで。パース・ルーティングはしない（対応表
//! `docs/device/capabilities/deep-link.json` のトップレベル `note` 参照）。受け口・最新 1 件の
//! 保持は [`crate::url_scheme::UrlInbox`] を再利用し、検査だけを
//! 「scheme が https で、ホストが許可ホストと一致する」に替える。
//! 呼び出し元は `fandhe-egui-proxy` の `device/deep_link.rs`（画面向け effect ドライバ）と
//! 各 OS 実装（Android は `device/android_deep_link.rs`）。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **値は URL 文字列のみ**: 受け取った URL は信頼しない。App Links の検証を経ない
//!   明示的な Intent でも任意の https URL が届きうるため、[`validate_url`] を通ったものだけを
//!   出す
//! - **弾く条件**: 空・[`MAX_URL_BYTES`] 超過・制御文字を含む・scheme が https でない・
//!   authority が許可ホストと完全一致しない（userinfo・ポート・パーセントエンコード・
//!   末尾ドット・接頭辞一致の別ホストを含む）・不正な UTF-8・文字列が取れない。
//!   弾いたものは中身を保持も表示もせず、件数だけ数える。失敗（`Err`）とは区別する
//! - **許可ホスト**: 出どころは呼び出し側（OS 実装）が決める。本リポジトリに実ドメインを書かない。
//!   [`validate_host`] に通らない許可ホストでは全件を拒否する（開いたまま通さない）
//! - **保持は最新 1 件**。**コールドスタート**: 受け口は static に置き、UI の前に届いた URL も
//!   落とさない
//! - **結果は 3 種**: 受信した URL・拒否した件数・非対応。非対応 OS は `Err(Unsupported)` を
//!   返し、空のスナップショットを成功として返さない（[`UnsupportedDeepLink`]）
//! - **成功・失敗・キャンセルの区別（契約表 10）**: 利用者のキャンセルや失敗の概念は無い。
//!   リンクの検証の成否は OS が決め、アプリへ届かなかったリンクはアプリから観測できないので、
//!   数えも推測もしない
//! - **ログに URL を出さない**: トークン等の秘密値が入りうる
//!
//! # 各 OS の届き方
//!
//! - Android: `getIntent()` / `onNewIntent` の `ACTION_VIEW` で https のもの。カスタム scheme
//!   （url-scheme）との振り分けは Issue #1279 の範囲
//! - iOS・macOS: 保留中。届き方は対応表の api 欄を参照
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順、不正 URL を表示しない、コールドスタート分を落とさない、
//! 判定と更新を同じ lock の内側で行うこと。
//! 守らない: ポーリング間隔内に連続して届いた場合に最後の 1 件だけ表示されること
//! （`seq` は失われない）、複数プロセス間、表示の一時的な遅れ、OS 側のリンク検証状態の表示。

use crate::DeviceError;
pub use crate::url_scheme::MAX_URL_BYTES;
use crate::url_scheme::{InboxSnapshot, UrlInbox};

const LABEL: &str = "deep_link";

/// 許可ホストの形式を検査する。空・ASCII 英小文字/数字/`-`/`.` 以外・先頭末尾の `.` は不正。
pub fn validate_host(host: &str) -> Result<(), DeviceError> {
    let ok = !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
    if ok {
        Ok(())
    } else {
        Err(DeviceError::InvalidInput(LABEL))
    }
}

/// 受信した URL を検査する。違反はすべて `InvalidInput("deep_link")`。
///
/// authority（`//` の後から最初の `/` `?` `#` の直前まで）を許可ホストと大文字小文字を
/// 区別せず完全一致で比べる。`@`・`:`・`\` 等を含む authority は完全一致しないので弾かれる。
pub fn validate_url(raw: &str, allowed_host: &str) -> Result<(), DeviceError> {
    let invalid = || DeviceError::InvalidInput(LABEL);
    validate_host(allowed_host)?;
    if raw.is_empty() || raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    const PREFIX: &str = "https://";
    let head = raw.as_bytes().get(..PREFIX.len()).ok_or_else(invalid)?;
    if !head.eq_ignore_ascii_case(PREFIX.as_bytes()) {
        return Err(invalid());
    }
    let rest = &raw[PREFIX.len()..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..end].eq_ignore_ascii_case(allowed_host) {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// OS から届いた生バイトを検査して `inbox` へ記録する。拒否は件数に反映済みで
/// `Err(InvalidInput)` を返す（呼び出し側は握って捨ててよい）。
pub fn receive(
    inbox: &UrlInbox,
    bytes: Option<&[u8]>,
    allowed_host: &str,
    now_ms: u64,
) -> Result<(), DeviceError> {
    inbox.receive_with(bytes, LABEL, now_ms, |s| validate_url(s, allowed_host))
}

/// ディープリンク受信の契約（モジュール doc 参照）。
pub trait DeepLink: Send + Sync {
    /// 受信状態を返す。未対応の OS は `Err(Unsupported)`。
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("deep_link")` を返す。
pub struct UnsupportedDeepLink;

impl DeepLink for UnsupportedDeepLink {
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
        Err(DeviceError::Unsupported(LABEL))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "example.com";

    fn is_invalid(r: Result<(), DeviceError>) -> bool {
        matches!(r, Err(DeviceError::InvalidInput("deep_link")))
    }

    #[test]
    fn validate_accepts_https_on_allowed_host() {
        for ok in [
            "https://example.com",
            "https://example.com/",
            "HTTPS://EXAMPLE.COM/a/b?q=1#f",
            "https://example.com?x=1",
            "https://example.com#f",
        ] {
            assert!(validate_url(ok, H).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn validate_rejects_bad_input() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        for bad in [
            "",
            "https://",
            "http://example.com/",
            "intent://example.com/",
            "fandhe://x",
            "https://example.com.evil.test/",
            "https://evil.test/?h=example.com",
            "https://user@example.com/",
            "https://example.com@evil.test/",
            "https://example.com:443/",
            "https://example.com./",
            "https://exa%6Dple.com/",
            "https://example.com\\@evil.test",
            "https://example.com/\nx",
            "https://example.org/",
            long.as_str(),
        ] {
            assert!(is_invalid(validate_url(bad, H)), "{bad:?}");
        }
    }

    #[test]
    fn invalid_allowed_host_rejects_everything() {
        for host in ["", ".", "Example.com", "example.com.", "a b", "ex@mple.com"] {
            assert!(
                is_invalid(validate_url("https://example.com/", host)),
                "{host:?}"
            );
        }
    }

    #[test]
    fn receive_keeps_latest_and_counts_rejects() {
        let inbox = UrlInbox::new();
        receive(&inbox, Some(b"https://example.com/a"), H, 10).unwrap();
        receive(&inbox, Some(b"https://example.com/b"), H, 20).unwrap();
        let snap = inbox.snapshot();
        assert_eq!(snap.last.as_ref().unwrap().url, "https://example.com/b");
        assert_eq!((snap.rejected, snap.seq), (0, 2));

        for bad in [
            Some(&b"https://example.org/"[..]),
            Some(&[0xff, 0xfe][..]),
            None,
        ] {
            assert!(is_invalid(receive(&inbox, bad, H, 30)));
        }
        let snap = inbox.snapshot();
        assert_eq!(snap.last.unwrap().url, "https://example.com/b");
        assert_eq!((snap.rejected, snap.seq), (3, 5));
    }

    #[test]
    fn unsupported_never_returns_ok() {
        assert!(matches!(
            UnsupportedDeepLink.snapshot(),
            Err(DeviceError::Unsupported("deep_link"))
        ));
    }
}
