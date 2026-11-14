//! ローカル通知の抽象（Phase 11、親 #1003、Issue #1004）。
//!
//! `Share`/`Tts` と同じ request/response の一発呼び出し型（1 回の `notify()` = OS へ
//! 通知の表示を 1 回依頼する）。継続 push の共有状態は持たない。プッシュ通知
//! （APNs/FCM）は扱わない（対応表 `docs/device/capabilities/notification.json` の
//! トップレベル `note` 参照。本 Issue はローカル通知のみ）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 結果は 3 値: OS から表示されたことを確かめられた `Ok(Shown)`（本 Issue の
//!   macOS/Windows 実装では返さない。iOS の `willPresent`・Web の `onshow` 等、
//!   確認手段を持つ実装が後続 Issue で使う）/ OS へ渡したが表示されたかは確かめ
//!   られない `Ok(Submitted)`（macOS `osascript`・Windows Toast はこちら）/
//!   権限ダイアログを閉じただけで未決定の `Ok(Cancelled)`（失敗ではない。
//!   `device-bridge-contracts.md` の「10. キャンセル・正常終了・失敗の三値」参照。
//!   本 Issue の macOS/Windows 実装では発生しない）
//! - 失敗は `Err(NotificationError)`。写像は次のとおり
//!   - タイトル・本文の検証違反 → `InvalidInput`（[`NotificationRequest::new`]）
//!   - 利用者が通知を拒否 → `PermissionDenied("notification")`
//!   - OS 全体・グループポリシーで通知が無効 → `Disabled("notification")`
//!   - API・コマンドが無い（Linux、`osascript` 不在等） → `Unsupported("notification")`
//!   - 子プロセス異常終了・期限超過・OS 呼び出し失敗 → `Internal("notification ...")`
//! - タイトル・本文はログにも、エラーの文言にも出さない（security.md A09）
//! - 初期化に失敗した実装は `Unsupported` を返し、ダミーの成功実装へは倒さない
//!   （`device-bridge-contracts.md` の「6. 嘘の成功」参照。[`UnsupportedNotifier`] を使う）

use crate::error::NotificationError;

/// タイトルの最大長（バイト）。macOS 実装は `osascript` へ標準入力経由で
/// AppleScript ソースを渡す（`macos.rs` の `osascript_script` 参照。argv には出さない、
/// codex 指摘 P1、PR #1008）が、通知の実用上十分な長さとして OS 標準的な
/// 通知のタイトル欄の目安を採用し、際限なく長い入力は事前に弾く。
const TITLE_MAX_LEN: usize = 256;

/// 本文の最大長（バイト）。[`TITLE_MAX_LEN`] と同じ理由。通知本文は複数行を
/// 想定するため、タイトルより広く確保する。
const BODY_MAX_LEN: usize = 4096;

/// 通知の結果。[`Notifier::notify`] の契約（モジュール doc）を参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyOutcome {
    /// OS から実際に表示されたことを確かめられた（本 Issue の実装では返さない）。
    Shown,
    /// OS へ渡したが、表示されたかどうかは確かめられない。
    Submitted,
    /// 利用者が権限ダイアログを閉じるだけで未決定のまま終わった（失敗ではない）。
    Cancelled,
}

/// 検証済みの通知内容。[`NotificationRequest::new`] 以外で作れない（検証の SSOT、
/// `fandhe_device::share::ShareRequest::new` と同じ方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRequest {
    title: String,
    body: String,
}

impl NotificationRequest {
    /// `title`・`body` を検証して構築する。
    ///
    /// - `title`: trim 後に空なら `InvalidInput("notification title")`。NUL 混入・
    ///   [`TITLE_MAX_LEN`] 超過も同様
    /// - `body`: 空でもよい（trim 後の空判定はしない）。NUL 混入・[`BODY_MAX_LEN`]
    ///   超過は `InvalidInput("notification body")`
    /// - 値は trim せずそのまま保持する（`device-bridge-contracts.md` の「8. 秘密値」の
    ///   「入力どおりに保存する」方針を通知文にも適用する）
    /// - 長さ上限は際限のない入力による子プロセスへの負荷・スクリプト生成コストを
    ///   避けるための事前検証（codex 指摘、PR #1008）
    pub fn new(title: &str, body: &str) -> Result<Self, NotificationError> {
        if title.trim().is_empty() {
            return Err(NotificationError::InvalidInput("notification title"));
        }
        if title.contains('\0') {
            return Err(NotificationError::InvalidInput("notification title"));
        }
        if title.len() > TITLE_MAX_LEN {
            return Err(NotificationError::InvalidInput("notification title"));
        }
        if body.contains('\0') {
            return Err(NotificationError::InvalidInput("notification body"));
        }
        if body.len() > BODY_MAX_LEN {
            return Err(NotificationError::InvalidInput("notification body"));
        }
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    /// 通知のタイトル（trim 前・NUL 無しを保証）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 通知の本文（空文字列を許容、NUL 無しを保証）。
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// ローカル通知の契約（モジュール doc の「契約」節を参照）。
pub trait Notifier: Send + Sync {
    /// OS へ通知の表示を 1 回依頼する。
    fn notify(&self, request: &NotificationRequest) -> Result<NotifyOutcome, NotificationError>;
}

/// 対応不可プラットフォーム用（Linux、初期化失敗時のフォールバック）。常に
/// `Unsupported("notification")` を返す（ダミーの成功実装へは倒さない契約）。
pub struct UnsupportedNotifier;

impl Notifier for UnsupportedNotifier {
    fn notify(&self, _request: &NotificationRequest) -> Result<NotifyOutcome, NotificationError> {
        Err(NotificationError::Unsupported("notification"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_title() {
        let err = NotificationRequest::new("", "body").unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_blank_only_title() {
        let err = NotificationRequest::new("   ", "body").unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_title_with_embedded_nul() {
        let err = NotificationRequest::new("a\0b", "body").unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_body_with_embedded_nul() {
        let err = NotificationRequest::new("title", "a\0b").unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification body")
        ));
    }

    #[test]
    fn new_rejects_title_over_max_len() {
        let title = "a".repeat(TITLE_MAX_LEN + 1);
        let err = NotificationRequest::new(&title, "body").unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_accepts_title_at_max_len() {
        let title = "a".repeat(TITLE_MAX_LEN);
        assert!(NotificationRequest::new(&title, "body").is_ok());
    }

    #[test]
    fn new_rejects_body_over_max_len() {
        let body = "a".repeat(BODY_MAX_LEN + 1);
        let err = NotificationRequest::new("title", &body).unwrap_err();
        assert!(matches!(
            err,
            NotificationError::InvalidInput("notification body")
        ));
    }

    #[test]
    fn new_accepts_body_at_max_len() {
        let body = "a".repeat(BODY_MAX_LEN);
        assert!(NotificationRequest::new("title", &body).is_ok());
    }

    #[test]
    fn new_accepts_empty_body() {
        let request = NotificationRequest::new("title", "").unwrap();
        assert_eq!(request.title(), "title");
        assert_eq!(request.body(), "");
    }

    #[test]
    fn new_does_not_trim_title_or_body() {
        let request = NotificationRequest::new("  title  ", "  body  ").unwrap();
        assert_eq!(request.title(), "  title  ");
        assert_eq!(request.body(), "  body  ");
    }

    #[test]
    fn unsupported_notifier_returns_unsupported_error() {
        let request = NotificationRequest::new("title", "body").unwrap();
        let err = UnsupportedNotifier.notify(&request).unwrap_err();
        assert!(matches!(
            err,
            NotificationError::Unsupported("notification")
        ));
    }

    /// モック `Notifier`（`PermissionDenied` 固定）が成功へ化けないことを固定する
    /// （`share.rs::AlwaysCancelledShare` と同じ「契約側の経路を固定する」目的）。
    struct AlwaysDeniedNotifier;

    impl Notifier for AlwaysDeniedNotifier {
        fn notify(
            &self,
            _request: &NotificationRequest,
        ) -> Result<NotifyOutcome, NotificationError> {
            Err(NotificationError::PermissionDenied("notification"))
        }
    }

    #[test]
    fn mock_permission_denied_does_not_become_success() {
        let request = NotificationRequest::new("title", "body").unwrap();
        let err = AlwaysDeniedNotifier.notify(&request).unwrap_err();
        assert!(matches!(
            err,
            NotificationError::PermissionDenied("notification")
        ));
    }
}
