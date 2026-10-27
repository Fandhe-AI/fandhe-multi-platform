//! iOS 向け MDM 管理アプリ設定のスタブ（Issue #944）。b-ios（#945）が実体で置き換えるまで
//! `Unsupported` を返す（ダミーの成功へ倒さない）。`device/managed_app_config.rs` の iOS 経路から呼ばれる。
//!
//! # b-ios への申し送り
//!
//! - `NSUserDefaults` の `com.apple.configuration.managed` を `dictionaryForKey:` で取る。
//!   「無い」と「辞書でない」は同じ nil で返る（前者は `NoConfig`）
//! - キーごとに `objectForKey:` + `isKindOfClass:` で読み分け、型違いは `?`
//! - `.m` は `ios/` に置き、返す形は `fandhe_device::managed_app_config::parse_wire` のワイヤ形式
//! - PrivacyInfo の UserDefaults 理由（`CA92.1`）を確認する

use std::sync::Arc;

use fandhe_device::managed_app_config::{ManagedAppConfig, UnsupportedManagedAppConfig};

/// iOS の `ManagedAppConfig` 実装（#945 まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn ManagedAppConfig> {
    Arc::new(UnsupportedManagedAppConfig)
}
