//! Android 向け MDM 管理アプリ設定のスタブ（Issue #944）。b-android（#946）が実体で置き換えるまで
//! `Unsupported` を返す（ダミーの成功へ倒さない）。`device/managed_app_config.rs` の Android 経路から呼ばれる。
//!
//! # b-android への申し送り
//!
//! - `ManagedAppConfigHelper.java`（`RestrictionsManager.getApplicationRestrictions()`）、
//!   `res/xml/app_restrictions.xml`、manifest の `meta-data` を足す
//! - JNI 署名は `()Ljava/lang/String;`。制限が無いと `null` が返る（`NoConfig`）。返す形は
//!   `fandhe_device::managed_app_config::parse_wire` のワイヤ形式
//! - ディスク I/O を伴うため main thread で呼ばない（ドライバはワーカースレッドで呼ぶ）

use std::sync::Arc;

use fandhe_device::managed_app_config::{ManagedAppConfig, UnsupportedManagedAppConfig};

/// Android の `ManagedAppConfig` 実装（#946 まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(UnsupportedManagedAppConfig)
}
