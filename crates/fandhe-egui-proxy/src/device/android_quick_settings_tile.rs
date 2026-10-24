//! Android 向けクイック設定タイル（`TileService`）の骨格（親 #139、Issue #719）。
//! `device/quick_settings_tile.rs` の Android 経路（dispatch）から呼ばれる。実体は b-android の
//! 子 issue が本ファイルと Java 側の `TileService` に足す。現状は常に `Unsupported` を返し、
//! 成功を装わない。
//!
//! # b-android が埋めるときの約束
//!
//! - dispatch・`device.rs` の列・共通コア画面（`quick_settings_tile_show`）・`registry.rs` は触らない
//!   （本ファイルの `implementation()` の中身だけを差し替える）
//! - `TileService` の Java クラスで `onClick` 時に `synchronized` で読み・+1・`commit()` を行う。
//!   保存に失敗した押下は数えず、static の失敗件数を +1 して `TileSnapshot::failed_presses` に出す
//! - Manifest に `<service>` を置く（exported=true、`android:permission` は
//!   `BIND_QUICK_SETTINGS_TILE`、intent-filter は `QS_TILE`）。`uses-permission` は足さない。
//!   タイル名は `fandhe_core::screens::quick_settings_tile_show::TILE_LABEL` と一致させる
//! - JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//! - 壊れた保存内容は 0 に戻さず `Internal`。回数・OS のエラー文をログに出さない

use std::sync::Arc;

use fandhe_device::quick_settings_tile::{QuickSettingsTile, UnsupportedQuickSettingsTile};

/// Android 用の実装を返す。b-android までは `Unsupported` 固定。
pub(crate) fn implementation() -> Arc<dyn QuickSettingsTile> {
    Arc::new(UnsupportedQuickSettingsTile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_never_succeeds() {
        assert!(matches!(
            implementation().snapshot(),
            Err(DeviceError::Unsupported("quick_settings_tile"))
        ));
    }
}
