//! レイアウト計算のエラー型。
//!
//! `fandhe-core` は `unwrap()`/`expect()` をテスト以外で使わない方針（coding-rust.md）のため、
//! taffy 側のエラー（`taffy::TaffyError`）や本モジュールの入口検証（NaN/負値ガード）を
//! `Result` で呼び出し元へ返すための型。`thiserror` はワークスペース依存にあるが、
//! `fandhe-core` は本 issue 時点でランタイム依存を taffy のみに限定する方針のため、
//! `std::error::Error` を手動実装する。

use core::fmt;

/// `ui::layout` の操作が失敗した際に返すエラー。
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutError {
    /// 存在しない・別ツリーの `LayoutNodeId` を参照した。
    InvalidNode,
    /// 利用可能サイズ（`LayoutConstraints`）に NaN・無限大・負値が渡された。
    InvalidConstraint,
    /// taffy 内部エラーの委譲（`taffy::TaffyError` の `Display` 出力を保持する）。
    Engine(String),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNode => write!(f, "レイアウトツリーに存在しないノードが参照されました"),
            Self::InvalidConstraint => {
                write!(
                    f,
                    "レイアウト制約に不正な値（NaN・無限大・負値）が指定されました"
                )
            }
            Self::Engine(message) => write!(f, "レイアウトエンジンエラー: {message}"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl From<taffy::TaffyError> for LayoutError {
    /// taffy 側のノード不正エラーを `ui::layout` の公開エラー型へ写像する。
    ///
    /// taffy の型を `fandhe-core` の公開 API に露出させない方針（設計文書 §4.3）に従い、
    /// `TaffyError` 自体は呼び出し元へ伝播させず `Display` 文字列のみ保持する。
    fn from(value: taffy::TaffyError) -> Self {
        Self::Engine(value.to_string())
    }
}
