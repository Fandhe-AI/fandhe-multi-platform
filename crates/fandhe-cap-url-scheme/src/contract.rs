//! カスタム URL scheme の受信（Phase 11 の拡充ループ、親 #1116、Issue #1215）。
//!
//! 扱う範囲は「OS から push で届いた URL 文字列を検査し、最新 1 件を保持して画面が
//! 読めるようにする」ところまで。パース・ルーティングはしない（対応表
//! `docs/device/capabilities/url-scheme.json` のトップレベル `note` 参照）。
//! 呼び出し元は本 crate の `driver`（画面向け effect ドライバ）と各 OS 実装。
//! 検査と受信箱（[`UrlInbox`]・[`validate_url`] など OS に触れない部品）は `fandhe-device::url_scheme`
//! にあり、本 crate が再公開する（deep-link・file-association などが機能 crate 同士の依存なしに同じ検査を使うため）。`RequestSlot` 型（要求 1 件に
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
//!   winit 0.30 は自前の application delegate を登録し、差し替えると panic する。Apple Event
//!   ハンドラ方式は delegate に触れないため、その delegate と衝突しない
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

use crate::error::UrlSchemeError;

pub use fandhe_device::url_scheme::{
    InboxSnapshot, MAX_URL_BYTES, ReceivedUrl, SCHEME, UrlInbox, WEB_SCHEME, validate_url,
};

/// URL scheme 受信の契約（モジュール doc 参照）。
pub trait UrlScheme: Send + Sync {
    /// 受信状態を返す。未対応の OS は `Err(Unsupported)`。
    fn snapshot(&self) -> Result<InboxSnapshot, UrlSchemeError>;
}

/// 非対応 OS 用。常に `Unsupported` を返す。
pub struct UnsupportedUrlScheme;

impl UrlScheme for UnsupportedUrlScheme {
    fn snapshot(&self) -> Result<InboxSnapshot, UrlSchemeError> {
        Err(UrlSchemeError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_returns_ok() {
        assert!(matches!(
            UnsupportedUrlScheme.snapshot(),
            Err(UrlSchemeError::Unsupported)
        ));
    }
}
