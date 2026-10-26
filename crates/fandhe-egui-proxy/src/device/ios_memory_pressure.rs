//! iOS 向けメモリ逼迫の通知の骨格（Issue #911 の (a)、実体は b-ios）。
//! `device/memory_pressure.rs::platform_memory_pressure` の ios 経路から呼ばれ、いまは `Unsupported` を
//! 返すだけのスタブ。ダミーの成功実装へは倒さない。契約は `fandhe_device::memory_pressure`。
//!
//! # b-ios への約束
//!
//! - 触ってよいのはこのファイル、`ios/` の `.m`、`build.rs` の iOS 分岐、対応表の iOS セルだけ。
//!   dispatch・共通コア画面・`fandhe-device` は触らない（`fandhe_rust_memory_pressure_fire` の受け口と
//!   iOS の再描画要求は dispatch 側に置いてある）
//! - メモリ警告（`UIApplication.didReceiveMemoryWarningNotification`）は kind = 1、dispatch source
//!   （`DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`）は kind = 0 と `dispatch_source_get_data` の値を
//!   `fandhe_rust_memory_pressure_fire(id, kind, flags)` へ渡す（符号化は
//!   `fandhe_device::memory_pressure::event_from_apple`）
//! - observer は main キュー指定で受ける。通知の登録トークンは `id` をキーに握り続ける（契約表 1）。
//!   停止で全部外す
//! - `.m` は各関数を `@autoreleasepool` で囲み、`dispatch_sync` をロックの内側で使わない
//! - 今の状態を読む公開 API は無い（`support().current` は false、`read_current` は `Ok(None)`）

use std::sync::Arc;

use fandhe_device::memory_pressure::{MemoryPressure, UnsupportedMemoryPressure};

/// iOS の `MemoryPressure` 実装を返す（実体が入るまでは非対応）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn implementation() -> Arc<dyn MemoryPressure> {
    Arc::new(UnsupportedMemoryPressure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::memory_pressure::EventSupport;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        let imp = implementation();
        assert_eq!(imp.support(), EventSupport::NONE);
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
        assert!(matches!(
            imp.read_current(),
            Err(DeviceError::Unsupported("memory_pressure"))
        ));
    }
}
