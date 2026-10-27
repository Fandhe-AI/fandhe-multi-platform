//! OS の「最近使った項目」へのファイル登録と読み戻しの抽象（Phase 11 の拡充ループ、親 #193、
//! Issue #988）。
//!
//! 登録 API は macOS・Windows とも成否を返さない。そのため登録の直後に OS の一覧を読み戻し、
//! 載っていたかで結果を決める（登録 API の無戻り値を成功に畳まない）。OS へ渡せるパスは
//! [`ShellPath`]（`open_in_shell` の検査済み型）だけ。`fandhe-egui-proxy` の effect ドライバが
//! サンプルファイルを用意して検査し、OS 実装（macOS は `NSDocumentController`、Windows は #989）を
//! UI スレッドから同期に呼ぶ。
//!
//! # 結果の種類
//!
//! | 区分 | 返す値 |
//! |---|---|
//! | 読み戻した OS の一覧に、正規化後のパスがある | `Ok(RecentOutcome::Listed)` |
//! | 一覧に無い（設定で無効・反映待ち・件数上限 0 等。成功の色にも失敗の色にもしない） | `Ok(RecentOutcome::NotListed)` |
//! | 読み戻せない・OS の失敗・スレッド違反 | `Err(Internal("recent documents ..."))`（`NotListed` に畳まない） |
//! | 非対応 OS | `Err(Unsupported("recent documents"))` |
//!
//! 比較は両側を正規化してから行う（macOS は `URLByResolvingSymlinksInPath`、Windows は
//! `canonicalize` の `\\?\` を外し大文字小文字を無視して比べる）。
//!
//! # 守る範囲 / 守らない範囲（契約表 10・11）
//!
//! 守る: 1 プロセス内の要求と完了の順序（世代番号）、`NotListed` と失敗を `Listed` と見せない、
//! 登録済みをアプリ内に記録せず毎回 OS の一覧を読むこと、一覧を消去する API を呼ばないこと
//! （Windows の `SHAddToRecentDocs` は `pv` に NULL を渡すと全アプリの履歴を消すため、`pv` は
//! [`ShellPath`] からしか作らない）、COM 等の確保物の解放、パスと一覧の他の項目をログ・エラー文言に
//! 出さないこと。
//!
//! 守らない: 登録後の OS の非同期反映の時間差（一時的に `NotListed` になりうる）、Open Recent・
//! Dock・Jump List の表示そのもの、複数プロセス、利用者が一覧から消した・ピン留めした項目。
//!
//! 範囲外: 任意ファイルの登録、一覧の消去・列挙、Open Recent からの起動の受け口。
//!
//! 期限は設けない。OS の呼び出しは同期で結果を返し、待つ処理を持たないため（契約表 3）。

use crate::DeviceError;
pub use crate::open_in_shell::ShellPath;

/// 読み戻した OS の一覧に対する結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentOutcome {
    /// 一覧にパスがある。
    Listed,
    /// 一覧にパスが無い。失敗ではない。
    NotListed,
}

/// 「最近使った項目」へ登録・確認する OS 機能。
///
/// 呼び出し元は `fandhe-egui-proxy` の effect ドライバ（UI スレッドで同期に呼ぶ）。
/// 引数は [`ShellPath`] 検査済み。
pub trait RecentDocuments: Send + Sync {
    /// 登録して、直後に OS の一覧を読み戻した結果を返す。契約は本モジュールの doc。
    fn register(&self, path: &ShellPath) -> Result<RecentOutcome, DeviceError>;
    /// 登録せず、OS の一覧にパスがあるかだけを読み戻す。
    fn check(&self, path: &ShellPath) -> Result<RecentOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedRecentDocuments;

impl RecentDocuments for UnsupportedRecentDocuments {
    fn register(&self, _path: &ShellPath) -> Result<RecentOutcome, DeviceError> {
        Err(DeviceError::Unsupported("recent documents"))
    }

    fn check(&self, _path: &ShellPath) -> Result<RecentOutcome, DeviceError> {
        Err(DeviceError::Unsupported("recent documents"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let p = ShellPath::new(&std::env::temp_dir()).unwrap();
        assert!(matches!(
            UnsupportedRecentDocuments.register(&p),
            Err(DeviceError::Unsupported("recent documents"))
        ));
        assert!(matches!(
            UnsupportedRecentDocuments.check(&p),
            Err(DeviceError::Unsupported("recent documents"))
        ));
    }
}
