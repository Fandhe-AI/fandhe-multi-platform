//! 共有シートの抽象（Phase 11、Issue #897）。
//!
//! `Clipboard`/`FilePicker` と同じ request/response の一発呼び出し型（1 回の
//! `share()` = OS の共有 UI を 1 回出す）。固定タイムアウトを掛けない。継続 push の
//! 共有状態は持たない（`Motion` とは異なる）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 結果は 4 値: 共有した `Ok(Shared)` / キャンセル `Ok(Cancelled)`（失敗ではない）/
//!   共有先が無い `Ok(NoTarget)`（同じく失敗ではない）/ 結果不明 `Ok(Unknown)`
//!   （共有 UI は閉じたが共有したかどうかを判定できない。失敗ではない。Android の
//!   `Intent.createChooser` 経由（Issue #899）でのみ発生し得る: chooser が閉じたことは
//!   分かるが、選択完了の broadcast（`IntentSender`）が届かない場合に「共有した」とも
//!   「キャンセルした」とも断定できない。`Shared`/`Cancelled` へ畳んで偽の結果を作らない
//!   ための第 4 の腕）/ 失敗 `Err`
//! - 共有テキスト・URL はログへ出さない（security.md A09）
//! - macOS/iOS/Android/Web は OS がコールバック型（共有 UI 提示 → 結果を非同期通知）
//!   のため本トレイトを **通らない**。`fandhe_core::screens::share_text`（端末依存
//!   ゼロの状態機械）へ直接書き戻す（`fandhe_device::file_picker` モジュール doc の
//!   「モバイルは trait を通らない」と同じ判断。iOS/Android/Web は後続 Issue で
//!   同型の橋渡しを足す）
//! - 対応不可プラットフォーム（Linux・本 Issue 時点の Windows）は
//!   `Err(DeviceError::Unsupported("share"))`

use crate::DeviceError;

/// 共有シートの結果。キャンセル・共有先なしは失敗ではない（モジュール doc の契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareOutcome {
    /// 利用者が共有先を選び、OS 側の共有 UI が受理した。
    Shared,
    /// 利用者が共有 UI を閉じるだけで何も選ばなかった。
    Cancelled,
    /// 共有できる先が端末に 1 つも無い。
    NoTarget,
    /// 共有 UI は閉じたが、共有したかどうかを判定できない（Android のみ発生し得る。
    /// モジュール doc の契約節参照）。
    Unknown,
}

/// 検証済みの共有内容。[`ShareRequest::new`] 以外で作れない（検証の SSOT、
/// `fandhe_device::file_picker::read_picked` と同じ「読み取り・検証を 1 箇所に
/// 一本化する」方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareRequest {
    text: String,
    url: Option<String>,
}

impl ShareRequest {
    /// `text`・`url`（任意）を検証して構築する。
    ///
    /// - `text`: trim 後に空なら `InvalidInput("share text")`。内部に NUL を含む場合も
    ///   `InvalidInput("share text")`（`CString` 化する呼び出し元での失敗を事前に防ぐ）
    /// - `url`: `None`・空文字列（trim 後空を含む）は「URL 無し」として `None` 扱い。
    ///   それ以外は `http://`/`https://` で始まる場合のみ許可し（スキームの大小文字は
    ///   区別しない。RFC 3986 でスキームは大小文字を区別しないため）、それ以外
    ///   （`file://` 等）・NUL 混入は `InvalidInput("share url")` にする。`file://` を
    ///   通すと `NSSharingService` がローカルファイルの内容を共有し得るため
    ///   （security.md A03、実装計画の設計判断 3 参照）
    pub fn new(text: &str, url: Option<&str>) -> Result<Self, DeviceError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(DeviceError::InvalidInput("share text"));
        }
        if text.contains('\0') {
            return Err(DeviceError::InvalidInput("share text"));
        }

        let url = match url.map(str::trim) {
            None | Some("") => None,
            Some(u) => {
                let scheme_ok = u.split_once("://").is_some_and(|(scheme, _)| {
                    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
                });
                if u.contains('\0') || !scheme_ok {
                    return Err(DeviceError::InvalidInput("share url"));
                }
                Some(u.to_string())
            }
        };

        Ok(Self {
            text: text.to_string(),
            url,
        })
    }

    /// 共有するテキスト（trim 済み・空でない・NUL 無しを保証）。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 共有する URL（`http(s)://` のみ、無ければ `None`）。
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }
}

/// 共有シートの契約（モジュール doc の「契約」節を参照）。
pub trait Share: Send + Sync {
    /// OS の共有 UI を 1 回出す。固定タイムアウトを掛けない。
    fn share(&self, request: &ShareRequest) -> Result<ShareOutcome, DeviceError>;
}

/// 対応不可プラットフォーム用（Linux、本 Issue 時点の Windows）。常に
/// `Unsupported("share")` を返す。
pub struct UnsupportedShare;

impl Share for UnsupportedShare {
    fn share(&self, _request: &ShareRequest) -> Result<ShareOutcome, DeviceError> {
        Err(DeviceError::Unsupported("share"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_text() {
        let err = ShareRequest::new("", None).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share text")));
    }

    #[test]
    fn new_rejects_blank_only_text() {
        let err = ShareRequest::new("   ", None).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share text")));
    }

    #[test]
    fn new_rejects_text_with_embedded_nul() {
        let err = ShareRequest::new("a\0b", None).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share text")));
    }

    #[test]
    fn new_trims_text() {
        let request = ShareRequest::new("  hello  ", None).unwrap();
        assert_eq!(request.text(), "hello");
    }

    #[test]
    fn new_treats_empty_url_as_none() {
        let request = ShareRequest::new("hello", Some("")).unwrap();
        assert_eq!(request.url(), None);
    }

    #[test]
    fn new_treats_blank_only_url_as_none() {
        let request = ShareRequest::new("hello", Some("   ")).unwrap();
        assert_eq!(request.url(), None);
    }

    #[test]
    fn new_accepts_https_url() {
        let request = ShareRequest::new("hello", Some("https://example.com")).unwrap();
        assert_eq!(request.url(), Some("https://example.com"));
    }

    #[test]
    fn new_accepts_http_url() {
        let request = ShareRequest::new("hello", Some("http://example.com")).unwrap();
        assert_eq!(request.url(), Some("http://example.com"));
    }

    #[test]
    fn new_accepts_uppercase_scheme_url() {
        let request = ShareRequest::new("hello", Some("HTTPS://example.com")).unwrap();
        assert_eq!(request.url(), Some("HTTPS://example.com"));
    }

    #[test]
    fn new_rejects_file_scheme_url() {
        let err = ShareRequest::new("hello", Some("file:///etc/passwd")).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share url")));
    }

    #[test]
    fn new_rejects_javascript_scheme_url() {
        let err = ShareRequest::new("hello", Some("javascript:alert(1)")).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share url")));
    }

    #[test]
    fn new_rejects_url_with_embedded_nul() {
        let err = ShareRequest::new("hello", Some("https://example.com/\0")).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("share url")));
    }

    #[test]
    fn unsupported_share_returns_unsupported_error() {
        let request = ShareRequest::new("hello", None).unwrap();
        let err = UnsupportedShare.share(&request).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("share")));
    }

    /// モック `Share`（`Cancelled` 固定）が `Shared` へ化けないことを固定する
    /// （契約の 3 値を取り違えない、`clipboard.rs` の `UnsupportedClipboard` と
    /// 同じ「契約側の経路を固定する」目的のテスト）。
    struct AlwaysCancelledShare;

    impl Share for AlwaysCancelledShare {
        fn share(&self, _request: &ShareRequest) -> Result<ShareOutcome, DeviceError> {
            Ok(ShareOutcome::Cancelled)
        }
    }

    #[test]
    fn mock_cancelled_share_does_not_become_shared() {
        let request = ShareRequest::new("hello", None).unwrap();
        let outcome = AlwaysCancelledShare.share(&request).unwrap();
        assert_eq!(outcome, ShareOutcome::Cancelled);
        assert_ne!(outcome, ShareOutcome::Shared);
    }

    /// モック `Share`（`Unknown` 固定）が `Shared`/`Cancelled` のどちらへも化けない
    /// ことを固定する（Android の chooser 結果不明ケース、§2.1 の設計判断参照）。
    struct AlwaysUnknownShare;

    impl Share for AlwaysUnknownShare {
        fn share(&self, _request: &ShareRequest) -> Result<ShareOutcome, DeviceError> {
            Ok(ShareOutcome::Unknown)
        }
    }

    #[test]
    fn mock_unknown_share_does_not_become_shared_or_cancelled() {
        let request = ShareRequest::new("hello", None).unwrap();
        let outcome = AlwaysUnknownShare.share(&request).unwrap();
        assert_eq!(outcome, ShareOutcome::Unknown);
        assert_ne!(outcome, ShareOutcome::Shared);
        assert_ne!(outcome, ShareOutcome::Cancelled);
    }
}
