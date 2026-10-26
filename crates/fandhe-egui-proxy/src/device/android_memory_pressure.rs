//! Android 向けメモリ逼迫の通知の骨格（Issue #911 の (a)、実体は b-android）。
//! `device/memory_pressure.rs::platform_memory_pressure` の android 経路から呼ばれ、いまは `Unsupported` を
//! 返すだけのスタブ。ダミーの成功実装へは倒さない。契約は `fandhe_device::memory_pressure`。
//!
//! # b-android への約束
//!
//! - 触ってよいのはこのファイル、Java（補助クラス）、対応表の Android セルだけ。
//!   dispatch・共通コア画面・`fandhe-device` は触らない
//! - Application Context へ `ComponentCallbacks2` を登録し、`onTrimMemory(level)` を `id` 付きで流す。
//!   値は `PressureEvent::Trim(level)` にそのまま持ち、段階は今後増えうるため特定の値に依存せず
//!   大小で扱う（未知値も捨てない）
//! - 今の状態は `ActivityManager.MemoryInfo.lowMemory` を呼ぶたびに読み、`MemoryLevel::Low` /
//!   `MemoryLevel::NotLow` で返す。読めなければ `Err`（`NotLow` へ畳まない）
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - 停止は登録解除を伴い、失敗にしない。解除後の古い `id` の通知は登録表が捨てる

use std::sync::Arc;

use fandhe_device::memory_pressure::{MemoryPressure, UnsupportedMemoryPressure};

/// Android の `MemoryPressure` 実装を返す（実体が入るまでは非対応）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
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
