//! OS の検索への登録の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/spotlight-index.json`、親 #117、Issue #560）。
//!
//! 扱う範囲は「固定のメモ 1 件を OS の検索へ登録すること」と「自アプリのドメインに属する項目の
//! 全削除」だけ。OS の検索結果から開かれたときの受信は `url_scheme` の入口（`fandhe://spotlight/<id>`）
//! を使い、この trait には含めない。対象は macOS / iOS（Core Spotlight）・Android（AppSearch、API 31+）。
//! Windows・Web は `Unsupported` 固定。同期 API で、呼び出し側（`fandhe-egui-proxy` の effect
//! ドライバ）は UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **三値**（契約表 10）: 成功 / 非対応（`Err(Unsupported)`）/ 失敗（その他の `Err`）。
//!   利用者によるキャンセル操作は無いため「該当なし」
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`SPOTLIGHT_DEADLINE`]）。起点は
//!   要求を観測した時の `Instant`（単調時計）。`remaining` は OS 側の完了待ちの上限で、時間切れは
//!   `Internal(`[`TIMEOUT`]`)`。期限後に届いた成功は捨てる
//! - **入力不正**: [`validate_item`] が `InvalidInput` を返す。OS の呼び出しより前に行う
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ適用する
//! - **ライフサイクル**（契約表 1）: Android は Activity 再生成でドライバの記録を捨てる
//! - **ログ**: 識別子・題名・本文・OS のエラー文はログにも画面にも出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、古い完了を捨てること、嘘の成功の禁止、ワーカーの
//! 資源解放。守らない範囲: OS の検索に実際に出るか（OS の設定・OEM・Spotlight の設定）、索引への
//! 反映の時間差、期限切れの後に OS 側で登録・削除が済んでいた場合の表示の食い違い（失敗と表示しても
//! 登録は済んでいることがある。成功を装う向きのずれではない）、他アプリが同じ URL を送った場合との
//! 区別（`url_scheme` と同じ信頼境界）、複数プロセス。

use std::time::Duration;

use crate::DeviceError;

/// 自アプリの項目をまとめる識別子（全削除はこのドメインだけを消す）。
pub const DOMAIN_ID: &str = "com.fandhe.demo.spotlight";
/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const SPOTLIGHT_DEADLINE: Duration = Duration::from_secs(10);
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "spotlight_index timeout";
/// 項目 id の最大バイト数。
pub const MAX_ID_BYTES: usize = 64;
/// 題名の最大バイト数。
pub const MAX_TITLE_BYTES: usize = 256;
/// 本文の最大バイト数。
pub const MAX_TEXT_BYTES: usize = 1024;

/// 登録する項目 1 件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexItem<'a> {
    /// 項目 id（`[A-Za-z0-9._-]` の 1〜64 バイト）。
    pub id: &'a str,
    /// 題名。
    pub title: &'a str,
    /// 本文（説明文）。
    pub text: &'a str,
}

/// OS の検索への登録の契約（モジュール doc の「契約」節）。
pub trait SpotlightIndex: Send + Sync {
    /// `item` を [`DOMAIN_ID`] のドメインで登録する。同じ id は上書きされる。
    fn index(&self, item: &IndexItem<'_>, remaining: Duration) -> Result<(), DeviceError>;
    /// [`DOMAIN_ID`] に属する項目だけを全削除する（他のドメインには触れない）。
    fn remove_all(&self, remaining: Duration) -> Result<(), DeviceError>;
}

/// 非対応 OS 用。どちらも `Unsupported` を返し、成功を装わない。
pub struct UnsupportedSpotlightIndex;

impl SpotlightIndex for UnsupportedSpotlightIndex {
    fn index(&self, _: &IndexItem<'_>, _: Duration) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("spotlight_index"))
    }
    fn remove_all(&self, _: Duration) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("spotlight_index"))
    }
}

/// id の文字集合（`[A-Za-z0-9._-]`、1〜64 バイト）。
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// 項目を検査する。id は [`is_valid_id`]、題名は 1〜256 バイト、本文は 1024 バイト以下で、
/// 題名・本文とも制御文字を含まない。
pub fn validate_item(item: &IndexItem<'_>) -> Result<(), DeviceError> {
    if !is_valid_id(item.id) {
        return Err(DeviceError::InvalidInput("spotlight_index id"));
    }
    if item.title.is_empty()
        || item.title.len() > MAX_TITLE_BYTES
        || item.title.chars().any(char::is_control)
    {
        return Err(DeviceError::InvalidInput("spotlight_index title"));
    }
    if item.text.len() > MAX_TEXT_BYTES || item.text.chars().any(char::is_control) {
        return Err(DeviceError::InvalidInput("spotlight_index text"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item<'a>(id: &'a str, title: &'a str, text: &'a str) -> IndexItem<'a> {
        IndexItem { id, title, text }
    }

    #[test]
    fn valid_item_passes() {
        assert!(validate_item(&item("demo-memo", "題名", "本文")).is_ok());
        assert!(validate_item(&item(&"a".repeat(64), "t", "")).is_ok());
    }

    #[test]
    fn invalid_items_are_rejected() {
        let long_id = "a".repeat(65);
        let long_title = "あ".repeat(86);
        let long_text = "a".repeat(1025);
        let cases = [
            item("", "t", "x"),
            item(&long_id, "t", "x"),
            item("a/b", "t", "x"),
            item("a b", "t", "x"),
            item("id", "", "x"),
            item("id", &long_title, "x"),
            item("id", "t\nx", "x"),
            item("id", "t", "x\0"),
            item("id", "t", &long_text),
        ];
        for c in cases {
            assert!(
                matches!(validate_item(&c), Err(DeviceError::InvalidInput(_))),
                "{c:?}"
            );
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedSpotlightIndex;
        let i = item("id", "t", "x");
        assert!(matches!(
            u.index(&i, Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            u.remove_all(Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
