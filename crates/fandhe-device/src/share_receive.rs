//! 共有の受け取り（Phase 11 の拡充ループ、親 #122、Issue #606）。
//!
//! 扱う範囲は「他アプリから共有されたテキスト 1 件を OS から push で受け、検査して最新 1 件を
//! 保持し、画面が読めるようにする」ところまで。画像・ファイル・複数件・件名は扱わない
//! （対応表 `docs/device/capabilities/share-receive.json` 参照）。呼び出し元は
//! `fandhe-egui-proxy` の `device/share_receive.rs`（画面向け effect ドライバ）と各 OS 実装
//! （macOS は `device/macos_share_receive.rs`）。`RequestSlot` 型（要求 1 件に結果 1 件）ではなく、
//! OS が任意のタイミングで書き込む継続配信型として扱う（`url_scheme` と同型）。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は 3 種**: 受理（[`ShareSnapshot::last`] が更新される）、拒否（[`ShareSnapshot::rejected`]
//!   と [`ShareSnapshot::seq`] だけが増え、`last` は変わらない）、未受信（`last == None`。通常の
//!   起動であり失敗ではない）。非対応 OS は `Err(Unsupported)`
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 受信は OS からの
//!   push で、利用者のキャンセル操作も権限ダイアログも無い。検査で弾いたものは失敗ではなく
//!   件数として報告し、弾いた中身は保持も表示もしない
//! - **弾く条件**: 空・[`MAX_TEXT_BYTES`] 超過（途中で切らず拒否）・`\t` `\n` `\r` 以外の制御文字
//!   （NUL を含む）・不正な UTF-8・文字列が取れない。前後の空白は削らない（入力どおりに保持）
//! - **保持は最新 1 件**: 後着を最新とする。際限なく溜めない
//! - **コールドスタート**: 受け口はプロセス全体の static に置き、UI の有無に関係なく書き込む。
//!   画面は構築時と以後のポーリングでスナップショットを読む
//! - **非対応 OS は `Err(Unsupported)`**: 空のスナップショットを成功として返さない
//!   （[`UnsupportedShareReceive`]。同 6 嘘の成功）
//! - **本文をログ・エラー文に出さない・リンクとして開かない**: 受け取った文字列は表示するだけ
//!
//! # 各 OS の届き方
//!
//! - macOS（Issue #606）: Info.plist の `NSServices` によるサービスメニューと
//!   `NSApp.servicesProvider`。`.app` 化して LaunchServices に登録した場合だけ届く
//! - Android（#607）: `ACTION_SEND` + `EXTRA_TEXT`
//! - Web（#608）: manifest の `share_target`（GET）。trait を通らず [`validate_text`] /
//!   [`ShareInbox`] を直接使う
//! - iOS: 保留（App Group が要る）。Windows: 対象外
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順、不正な入力を表示しない、判定と更新を同じ lock の内側で行うこと、
//! 嘘の成功の禁止。
//! 守らない: ポーリング間隔内に連続して届いた場合に最後の 1 件だけ表示されること（受信回数は
//! [`ShareSnapshot::seq`] 上で失われない）、複数プロセス間、OS 側の登録反映の遅れ、
//! 表示の一時的な遅れ。

use std::sync::{Mutex, MutexGuard};

use crate::DeviceError;

/// 受け付けるテキストの最大バイト数。超過は切り詰めずに拒否する。
// ponytail: 共有されるメモ・引用の実用上限の目安。要件が出たら上げる。
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// 受信したテキストを検査する。違反はすべて `InvalidInput("share_receive")`（本文を含めない）。
///
/// 前後の空白は削らない。`\t` `\n` `\r` 以外の制御文字（C0・DEL・C1、NUL を含む）は拒否する。
pub fn validate_text(raw: &str) -> Result<(), DeviceError> {
    let invalid = || DeviceError::InvalidInput("share_receive");
    if raw.is_empty() || raw.len() > MAX_TEXT_BYTES {
        return Err(invalid());
    }
    if raw
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(invalid());
    }
    Ok(())
}

/// 受理したテキストと受信時刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedText {
    /// 検査を通ったテキスト（入力どおり、trim しない）。
    pub text: String,
    /// 受信時刻（UNIX エポックからのミリ秒）。時刻は呼び出し側が渡す
    /// （wasm32 でも本クレートがコンパイルされるため `SystemTime::now()` を呼ばない）。
    pub received_at_unix_ms: u64,
}

/// [`ShareInbox`] の整合した読み出し結果（lock 1 回分）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareSnapshot {
    /// 最後に受理したテキスト。未受信なら `None`。
    pub last: Option<SharedText>,
    /// 検査で弾いた件数。
    pub rejected: u64,
    /// 受信（受理 + 拒否）の通算回数。画面側が変化を検出するための世代番号。
    pub seq: u64,
}

/// 受信したテキストを保持する共有状態。static に置ける（[`ShareInbox::new`] は `const fn`）。
pub struct ShareInbox(Mutex<ShareSnapshot>);

impl ShareInbox {
    /// 空の受け口を作る。
    pub const fn new() -> Self {
        Self(Mutex::new(ShareSnapshot {
            last: None,
            rejected: 0,
            seq: 0,
        }))
    }

    /// poison していても中身を使う（panic を契約にしない）。
    fn guard(&self) -> MutexGuard<'_, ShareSnapshot> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// OS から届いた生バイトを検査して記録する。`None`（文字列が取れなかった）・不正な UTF-8・
    /// 検査違反は拒否として数え、`Err(InvalidInput)` を返す（件数に反映済みのため呼び出し側は
    /// 握って捨ててよい）。検査は lock の外、`seq` と `last` / `rejected` の更新は同じ lock の
    /// 内側で行う。
    pub fn receive_bytes(&self, bytes: Option<&[u8]>, now_ms: u64) -> Result<(), DeviceError> {
        let checked = bytes
            .ok_or(DeviceError::InvalidInput("share_receive"))
            .and_then(|b| {
                std::str::from_utf8(b).map_err(|_| DeviceError::InvalidInput("share_receive"))
            })
            .and_then(|s| validate_text(s).map(|()| s));
        let mut inner = self.guard();
        inner.seq += 1;
        match checked {
            Ok(text) => {
                inner.last = Some(SharedText {
                    text: text.to_string(),
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
    pub fn snapshot(&self) -> ShareSnapshot {
        self.guard().clone()
    }
}

impl Default for ShareInbox {
    fn default() -> Self {
        Self::new()
    }
}

/// 共有の受け取りの契約（モジュール doc 参照）。
pub trait ShareReceive: Send + Sync {
    /// 受信状態を返す。未対応の OS は `Err(Unsupported)`。
    fn snapshot(&self) -> Result<ShareSnapshot, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("share_receive")` を返す。
pub struct UnsupportedShareReceive;

impl ShareReceive for UnsupportedShareReceive {
    fn snapshot(&self) -> Result<ShareSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("share_receive"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_boundaries() {
        assert!(validate_text("a").is_ok());
        assert!(validate_text(&"a".repeat(MAX_TEXT_BYTES)).is_ok());
        assert!(validate_text(&"a".repeat(MAX_TEXT_BYTES + 1)).is_err());
        // 多バイト文字で上限をまたぐ場合もバイト数で検査する。
        let multi = "あ".repeat(MAX_TEXT_BYTES / 3 + 1);
        assert!(multi.len() > MAX_TEXT_BYTES);
        assert!(validate_text(&multi).is_err());
    }

    #[test]
    fn validate_rejects_bad_input() {
        for bad in ["", "a\0b", "a\u{7f}", "a\u{85}", "a\u{1b}[0m"] {
            assert!(
                matches!(
                    validate_text(bad),
                    Err(DeviceError::InvalidInput("share_receive"))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn validate_accepts_whitespace_controls_and_keeps_spaces() {
        assert!(validate_text("a\tb\nc\r\n").is_ok());
        let inbox = ShareInbox::new();
        inbox.receive_bytes(Some(b"  hi \n"), 1).unwrap();
        assert_eq!(inbox.snapshot().last.unwrap().text, "  hi \n");
    }

    #[test]
    fn receive_keeps_latest_and_counts_rejects() {
        let inbox = ShareInbox::new();
        assert_eq!(inbox.snapshot(), ShareSnapshot::default());

        inbox.receive_bytes(Some(b"a"), 10).unwrap();
        inbox.receive_bytes(Some(b"b"), 20).unwrap();
        let snap = inbox.snapshot();
        assert_eq!(snap.last.as_ref().unwrap().text, "b");
        assert_eq!(snap.last.as_ref().unwrap().received_at_unix_ms, 20);
        assert_eq!((snap.rejected, snap.seq), (0, 2));

        for bad in [Some(&[0xff, 0xfe][..]), None, Some(&b""[..])] {
            assert!(inbox.receive_bytes(bad, 30).is_err());
        }
        let snap = inbox.snapshot();
        assert_eq!(snap.last.unwrap().text, "b");
        assert_eq!((snap.rejected, snap.seq), (3, 5));
    }

    #[test]
    fn unsupported_never_returns_ok() {
        assert!(matches!(
            UnsupportedShareReceive.snapshot(),
            Err(DeviceError::Unsupported("share_receive"))
        ));
    }
}
