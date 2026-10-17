//! iOS 向け端末内 LLM 実装のスタブ（Issue #345 が置く。中身は b-ios が置き換える）。
//! `device/on_device_llm.rs` の iOS 経路から呼ばれる。b-ios は `implementation()` の中身だけを
//! `AppleOnDeviceLlm` へ差し替え、dispatch・画面・文言は触らない。それまでは `Unsupported` を返し、
//! ダミーの成功へは倒さない。契約の SSOT は `fandhe_device::on_device_llm`。

use std::sync::Arc;

use fandhe_device::on_device_llm::{OnDeviceLlm, UnsupportedOnDeviceLlm};

/// iOS の `OnDeviceLlm` 実装を返す（現状は `Unsupported` 固定）。
pub(crate) fn implementation() -> Arc<dyn OnDeviceLlm> {
    Arc::new(UnsupportedOnDeviceLlm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::on_device_llm::RESPOND_DEADLINE;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported_not_success() {
        assert!(matches!(
            implementation().respond("x", RESPOND_DEADLINE),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
