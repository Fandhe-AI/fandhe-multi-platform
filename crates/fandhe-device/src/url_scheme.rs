//! カスタム URL scheme の受信（Phase 11 の拡充ループ、親 #1116、Issue #1215）。
//!
//! 扱う範囲は「OS から push で届いた URL 文字列を検査し、最新 1 件を保持して画面が
//! 読めるようにする」ところまで。パース・ルーティングはしない（対応表
//! `docs/device/capabilities/url-scheme.json` のトップレベル `note` 参照）。
//! 呼び出し元は `fandhe-egui-proxy` の `device/url_scheme.rs`（画面向け effect ドライバ）
//! と各 OS 実装（macOS は `device/apple_url_scheme.rs`）。`RequestSlot` 型（要求 1 件に
//! 結果 1 件）ではなく、OS が任意のタイミングで書き込む継続配信型として扱う。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **値は URL 文字列のみ**: 受け取った URL は信頼しない（他アプリも同じ scheme を宣言
//!   できる）。[`validate_url`] を通ったものだけを [`UrlInbox::snapshot`] に出す
//! - **弾く条件**: 空・[`MAX_URL_BYTES`] 超過・制御文字を含む・scheme 不一致
//!   （[`SCHEME`] / Web は [`WEB_SCHEME`]）・コロン以降が空・不正な UTF-8・文字列が取れない。
//!   弾いたものは中身を保持も表示もせず、件数（[`InboxSnapshot::rejected`]）だけ数える。
//!   失敗（`Err`）とは区別する
//! - **保持は最新 1 件**: 後着を最新とする。際限なく溜めない
//! - **コールドスタート**: 受け口はプロセス全体の static に置き、UI の有無に関係なく
//!   書き込む。画面は構築時と以後のポーリングでスナップショットを読むので、UI ができる前に
//!   届いた URL も取りこぼさない
//! - **非対応 OS は `Err(Unsupported)`**: 空のスナップショットを成功として返さない
//!   （[`UnsupportedUrlScheme`]。`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **ログに URL を出さない**: トークン等の秘密値が入りうる（同「8. 秘密値」）
//!
//! # 各 OS の届き方（同じ契約で扱う）
//!
//! - macOS（Issue #1215）: Apple Event `kAEGetURL`。イベントループ開始前にハンドラを登録する。
//!   winit 0.30 は application delegate を登録しないが、Apple Event ハンドラ方式は
//!   delegate に触れないため将来 delegate を入れても衝突しない
//! - iOS: シーン無しの delegate の `application:openURL:options:` と、起動通知の `launchOptions`
//! - Android: `getIntent()` と `onNewIntent`（再配送）
//! - Windows: 起動引数。起動済みなら既存インスタンスへ転送してから [`UrlInbox::receive_bytes`]
//! - Web: `web+fandhe` + クエリ引数。[`WEB_SCHEME`] で検査する
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順、不正 URL を表示しない、コールドスタート分を落とさない、
//! 判定と更新を同じ lock の内側で行うこと。
//! 守らない: ポーリング間隔内に連続して届いた場合に最後の 1 件だけ表示されること
//! （受信回数は [`InboxSnapshot::seq`] 上で失われない）、複数プロセス間（Windows の別プロセス
//! 起動は転送側の責務）、表示の一時的な遅れ。

use std::sync::{Mutex, MutexGuard};

use crate::DeviceError;

/// ネイティブ（macOS / iOS / Android / Windows）で受け付ける scheme。
pub const SCHEME: &str = "fandhe";
/// Web で受け付ける scheme（`registerProtocolHandler` は `web+` 接頭辞が必須）。
pub const WEB_SCHEME: &str = "web+fandhe";
/// 受け付ける URL の最大バイト数。
// ponytail: 実用 URL 長の目安。長い URL が要件になったら上げる。
pub const MAX_URL_BYTES: usize = 2048;

/// 受信した URL を検査する。違反はすべて `InvalidInput("url_scheme")`。
///
/// scheme は RFC 3986 §3.1 に従い大文字小文字を区別しない。`fandhex:` が通らないよう
/// コロン込みで比較する。
pub fn validate_url(raw: &str, scheme: &str) -> Result<(), DeviceError> {
    let invalid = || DeviceError::InvalidInput("url_scheme");
    if raw.is_empty() || raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    let prefix_len = scheme.len() + 1;
    let head = raw.as_bytes().get(..prefix_len).ok_or_else(invalid)?;
    let matches =
        head[..scheme.len()].eq_ignore_ascii_case(scheme.as_bytes()) && head[scheme.len()] == b':';
    if !matches || raw.len() == prefix_len {
        return Err(invalid());
    }
    Ok(())
}

/// 受理した URL と受信時刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedUrl {
    /// 検査を通った URL 文字列。
    pub url: String,
    /// 受信時刻（UNIX エポックからのミリ秒）。時刻は呼び出し側が渡す
    /// （wasm32 でも本クレートがコンパイルされるため `SystemTime::now()` を呼ばない）。
    pub received_at_unix_ms: u64,
}

/// [`UrlInbox`] の整合した読み出し結果（lock 1 回分）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InboxSnapshot {
    /// 最後に受理した URL。未受信なら `None`。
    pub last: Option<ReceivedUrl>,
    /// 検査で弾いた件数。
    pub rejected: u64,
    /// 受信（受理 + 拒否）の通算回数。画面側が変化を検出するための世代番号。
    pub seq: u64,
}

/// 受信した URL を保持する共有状態。static に置ける（[`UrlInbox::new`] は `const fn`）。
pub struct UrlInbox(Mutex<InboxSnapshot>);

impl UrlInbox {
    /// 空の受け口を作る。
    pub const fn new() -> Self {
        Self(Mutex::new(InboxSnapshot {
            last: None,
            rejected: 0,
            seq: 0,
        }))
    }

    /// poison していても中身を使う（panic を契約にしない）。
    fn guard(&self) -> MutexGuard<'_, InboxSnapshot> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// OS から届いた生バイトを検査して記録する。`None`（文字列が取れなかった）・不正な
    /// UTF-8・検査違反は拒否として数え、`Err(InvalidInput)` を返す（件数に反映済みのため
    /// 呼び出し側は握って捨ててよい）。検査は lock の外、`seq` と `last` / `rejected` の
    /// 更新は同じ lock の内側で行う。
    pub fn receive_bytes(
        &self,
        bytes: Option<&[u8]>,
        scheme: &str,
        now_ms: u64,
    ) -> Result<(), DeviceError> {
        let checked = bytes
            .ok_or(DeviceError::InvalidInput("url_scheme"))
            .and_then(|b| {
                std::str::from_utf8(b).map_err(|_| DeviceError::InvalidInput("url_scheme"))
            })
            .and_then(|s| validate_url(s, scheme).map(|()| s));
        let mut inner = self.guard();
        inner.seq += 1;
        match checked {
            Ok(url) => {
                inner.last = Some(ReceivedUrl {
                    url: url.to_string(),
                    received_at_unix_ms: now_ms,
                });
                Ok(())
            }
            Err(e) => {
                inner.rejected += 1;
                Err(e)
            }
        }
    }

    /// 現在の状態を lock 1 回で整合した組として返す。
    pub fn snapshot(&self) -> InboxSnapshot {
        self.guard().clone()
    }
}

impl Default for UrlInbox {
    fn default() -> Self {
        Self::new()
    }
}

/// URL scheme 受信の契約（モジュール doc 参照）。
pub trait UrlScheme: Send + Sync {
    /// 受信状態を返す。未対応の OS は `Err(Unsupported)`。
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("url_scheme")` を返す。
pub struct UnsupportedUrlScheme;

impl UrlScheme for UnsupportedUrlScheme {
    fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("url_scheme"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_scheme_case_insensitively() {
        assert!(validate_url("fandhe://open?id=1", SCHEME).is_ok());
        assert!(validate_url("FANDHE://x", SCHEME).is_ok());
        assert!(validate_url("web+fandhe:/x?q=1", WEB_SCHEME).is_ok());
    }

    #[test]
    fn validate_rejects_bad_input() {
        let long = format!("fandhe://{}", "a".repeat(MAX_URL_BYTES));
        for bad in [
            "",
            "fandhe:",
            "https://x",
            "fandhex://x",
            "fandhe//x",
            "fandhe://a\nb",
            "fandhe://a\0b",
            "fandhe://a\u{7f}",
            "fandhe://a\u{85}",
            "fandhe://a\tb",
            "終わり",
            long.as_str(),
        ] {
            assert!(
                matches!(
                    validate_url(bad, SCHEME),
                    Err(DeviceError::InvalidInput("url_scheme"))
                ),
                "{bad:?}"
            );
        }
        assert!(validate_url("fandhe://x", WEB_SCHEME).is_err());
    }

    #[test]
    fn receive_keeps_latest_and_counts_rejects() {
        let inbox = UrlInbox::new();
        assert_eq!(inbox.snapshot(), InboxSnapshot::default());

        inbox
            .receive_bytes(Some(b"fandhe://a"), SCHEME, 10)
            .unwrap();
        inbox
            .receive_bytes(Some(b"fandhe://b"), SCHEME, 20)
            .unwrap();
        let snap = inbox.snapshot();
        assert_eq!(snap.last.as_ref().unwrap().url, "fandhe://b");
        assert_eq!(snap.last.as_ref().unwrap().received_at_unix_ms, 20);
        assert_eq!((snap.rejected, snap.seq), (0, 2));

        // 拒否は last を変えず rejected と seq だけを増やす。
        for bad in [Some(&b"https://x"[..]), Some(&[0xff, 0xfe][..]), None] {
            assert!(inbox.receive_bytes(bad, SCHEME, 30).is_err());
        }
        let snap = inbox.snapshot();
        assert_eq!(snap.last.unwrap().url, "fandhe://b");
        assert_eq!((snap.rejected, snap.seq), (3, 5));
    }

    #[test]
    fn unsupported_never_returns_ok() {
        assert!(matches!(
            UnsupportedUrlScheme.snapshot(),
            Err(DeviceError::Unsupported("url_scheme"))
        ));
    }
}
