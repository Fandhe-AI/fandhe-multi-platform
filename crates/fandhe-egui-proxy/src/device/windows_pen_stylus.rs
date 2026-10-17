//! Windows 向けペン・筆圧入力実装のスタブ（Issue #355 (a) で骨格のみ置く。後続の子 issue が埋める）。
//! `device/pen_stylus.rs` の dispatch から呼ばれ、実装が入るまでは `UnsupportedPenStylus` を返して
//! 非対応を明示する（ダミーの成功実装へは倒さない）。契約は `fandhe_device::pen_stylus`。

use std::sync::Arc;

use fandhe_device::pen_stylus::{PenStylus, UnsupportedPenStylus};

/// この OS の実装（現状はスタブ）。
pub(crate) fn implementation() -> Arc<dyn PenStylus> {
    Arc::new(UnsupportedPenStylus)
}
