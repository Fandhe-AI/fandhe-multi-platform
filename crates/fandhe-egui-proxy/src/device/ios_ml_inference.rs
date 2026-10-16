//! iOS 向け機械学習推論の骨格（Issue #289 で置いたスタブ。実装は b-ios）。
//! `device/ml_inference.rs` の iOS 経路（dispatch は #289 で最終形）から呼ばれる。
//! b-ios が Core ML（`AppleMlInference` を返す形）で `implementation` の中身を置き換える。dispatch・画面・文言は触らない。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use std::sync::Arc;

use fandhe_device::ml_inference::{MlInference, UnsupportedMlInference};

/// iOS の `MlInference` 実装を返す。b-ios まで非対応。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(UnsupportedMlInference)
}
