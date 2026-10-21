//! Phase 11 の拡充ループ（Issue #563、a）: アプリ内レビュー依頼。OS のレビュー依頼 API を
//! 1 回呼ぶ契約。`fandhe-egui-proxy` の `device/in_app_review.rs` が「レビューを依頼する」ボタン
//! 押下時に [`InAppReview::request`] を UI スレッドから同期で呼ぶ。macOS（`SKStoreReviewController
//! requestReview`、10.14 以上・15.0 で非推奨）が実装し、iOS（`requestReviewInScene:`、14 以上・
//! 18.0 で非推奨）は後続（b-ios）。Android（GMS 依存）と Windows（package identity 前提の見込み）は
//! 保留、Web は対象外。
//!
//! # 結果（契約表 10）
//!
//! - `Ok(Requested)`: OS の review API を呼べた（iOS は前面の `UIWindowScene` を渡せたこと）。
//!   **ダイアログが表示されたか、利用者が投稿したかは確かめられない**
//! - `Err(Unsupported("in_app_review"))`: 保留の OS（Android・Windows）、Linux、API が無い OS 版
//! - `Err(Internal("in_app_review request"))`: main thread 外、iOS で前面の scene が取れない等
//! - キャンセルは存在しない（対象 OS のどれも観測できない）。成功・非対応・失敗の 3 種だけ
//! - `PermissionDenied` と `InvalidInput` は使わない。届いた場合も呼び出し側は失敗として扱う
//!
//! OS のエラー文言はログにも画面にも出さない。初期化に失敗した実装は `Unsupported` か
//! `Internal` を返し、ダミーの成功へ倒さない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 押下 1 回につき呼び出し 1 回、嘘の成功の禁止、失敗を握りつぶさない。
//! 守らない: ダイアログの表示有無、年 3 回などの上限による無反応、TestFlight や未バンドル実行での
//! 無反応、利用者が投稿したか。並行経路は無い（UI スレッドで同期に呼ぶ）。
//!
//! Apple の指針では、本番の利用側はボタン操作に直結させず流れの区切りで呼ぶ。型サンプルは確認用
//! なのでボタンで呼ぶ。

use crate::DeviceError;

/// [`InAppReview::request`] の成功の種類。「表示した」を匂わせない名前にしている。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOutcome {
    /// OS へ依頼を渡した。表示・投稿は確かめられない。
    Requested,
}

/// レビュー依頼を 1 回出す。UI スレッドから同期で呼ばれる。契約は本モジュール doc。
pub trait InAppReview: Send + Sync {
    /// 依頼を 1 回出す。
    fn request(&self) -> Result<ReviewOutcome, DeviceError>;
}

/// 非対応環境（保留の OS・Linux 等）用。常に `Unsupported`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedInAppReview;

impl InAppReview for UnsupportedInAppReview {
    fn request(&self) -> Result<ReviewOutcome, DeviceError> {
        Err(DeviceError::Unsupported("in_app_review"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedInAppReview.request(),
            Err(DeviceError::Unsupported("in_app_review"))
        ));
    }
}
