//! iOS 向け AR 実装のスタブ（Issue #307、親 #90 の (a)）。`device/ar.rs` の ios 経路から呼ばれる。
//! (b-ios) が ARKit の `.m` で埋める。dispatch・共通コア画面・trait 契約（`fandhe_device::ar`）は変えない。
//! それまでは `UnsupportedAr` を返し、ダミーの成功へ倒さない。

use std::sync::Arc;

use fandhe_device::ar::{ArTracking, UnsupportedAr};

/// iOS の AR 実装を返す（(a) では非対応）。
pub(crate) fn implementation() -> Arc<dyn ArTracking> {
    Arc::new(UnsupportedAr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::ar::ArFailure;

    #[test]
    fn stub_is_unsupported() {
        let ar = implementation();
        assert_eq!(ar.start(), Err(ArFailure::Unsupported));
        assert_eq!(ar.read(), Err(ArFailure::Unsupported));
    }
}
