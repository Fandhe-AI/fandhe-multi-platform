//! Windows 向け機械学習推論の骨格（Issue #289 で置いたスタブ。実装は windows の子 issue）。
//! `device/ml_inference.rs` の Windows 経路（dispatch は #289 で最終形）から呼ばれる。
//! windows の子 issue が WinML（`onnx_model` を組んで推論）で `implementation` の中身を置き換える。dispatch・画面・文言は触らない。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use std::sync::Arc;

use fandhe_device::ml_inference::{MlInference, UnsupportedMlInference};

/// Windows の `MlInference` 実装を返す。windows の子 issue まで非対応。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(UnsupportedMlInference)
}
