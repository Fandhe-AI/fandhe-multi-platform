//! Android 向け外部ストレージの着脱の骨格（Issue #858、親 #175）。`UnsupportedRemovableStorage` を返す
//! スタブで、実体は (b) Issue #859 が埋める。`device/removable_storage.rs::platform_removable_storage` の
//! android 経路から呼ばれる。
//!
//! # #859 への約束
//!
//! - 触ってよいのは本ファイル・Java ヘルパー・JNI・`MainActivity` の最小追記だけ。dispatch、`device.rs` の
//!   `pub mod` 列、共通コア画面、`registry.rs` は触らない
//! - 登録表（`super::removable_storage::{register, unregister, fire}`）は id で使い、生ポインタを渡さない
//! - API 30 未満は `support()` の events をすべて false にし、`start` は `Err(Unsupported)` を返す
//!   （事前判定はこの宣言だけ。画面は「着脱の通知: この OS 版では取れない」と出して一覧は読む）
//! - `onDestroy` でコールバックを解除する。停止を「失敗」として送らない
//! - 写像は `fandhe_device::removable_storage` の `state_from_android` / `event_from_android_state` /
//!   `kind_from_android` を使う。UUID・`getDirectory` は読まない
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる

use std::sync::Arc;

use fandhe_device::removable_storage::{RemovableStorage, UnsupportedRemovableStorage};

/// Android の `RemovableStorage` 実装を返す（#859 までは非対応を返す）。
pub(crate) fn implementation() -> Arc<dyn RemovableStorage> {
    Arc::new(UnsupportedRemovableStorage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_unsupported() {
        let imp = implementation();
        assert!(matches!(
            imp.start(Arc::new(|_| {})),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
        assert!(matches!(
            imp.read_volumes(),
            Err(DeviceError::Unsupported("removable_storage"))
        ));
    }
}
