//! Android 向け機械学習推論の骨格（Issue #289 で置いたスタブ。実装は b-android）。
//! `device/ml_inference.rs` の Android 経路（dispatch は #289 で最終形）から呼ばれる。
//! b-android が NNAPI（`dlopen`）で `implementation` の中身を置き換える。dispatch・画面・文言は触らない。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use std::sync::Arc;

use fandhe_device::ml_inference::{MlInference, UnsupportedMlInference};

/// Android の `MlInference` 実装を返す。b-android まで非対応。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(UnsupportedMlInference)
}
