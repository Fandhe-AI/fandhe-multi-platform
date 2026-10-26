//! Android 向け「電話発信・地図など他アプリの起動」の骨格（親 #176、Issue #861）。`device/external_app_launch.rs` の
//! Android 経路（dispatch）から呼ばれる。実体は (b-android) が埋めるまで `Unsupported` を返すスタブで、
//! 成功を装わない。契約の SSOT は `fandhe_device::external_app_launch`。
//!
//! # (b-android) への約束
//!
//! - 触ってよいのは本ファイル・Java ヘルパー・JNI だけ。dispatch・fallback 行・`device.rs`・
//!   共通コア画面・`registry.rs` は触らない
//! - 要求 id の追跡は `RequestTracker` を使う
//! - JNI の署名は 3 か所（Java 側・no-op スタブ・実体）で一致させる
//! - Activity が RESUMED でなければ起動せずコード 2 を返す
//! - 電話は `ACTION_DIAL` + `PHONE_URI`、地図は `ACTION_VIEW` + `GEO_URI`
//!   （`fandhe_device::external_app_launch`）。`ACTION_CALL`・`CALL_PHONE` は使わない
//! - `ActivityNotFoundException` はコード 1 にする
//! - 結果コードは `device/external_app_launch.rs` の `outcome_from_android_code` に従う
//! - `resolveActivity` は呼ばない。`<queries>` と権限は足さない。操作を別の URI で代わりに開かない

use fandhe_device::external_app_launch::LaunchTarget;
use fandhe_device::DeviceError;

/// OS へ起動を依頼する。`Ok` は受理済みで、結果は後から `bridge::deliver` で届く。
/// 実体が入るまでは `Unsupported`。
pub(crate) fn begin_launch(_request_id: u64, _target: LaunchTarget) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("external app launch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_unsupported_for_both_targets() {
        for t in [LaunchTarget::Phone, LaunchTarget::Map] {
            assert!(matches!(
                begin_launch(1, t),
                Err(DeviceError::Unsupported("external app launch"))
            ));
        }
    }
}
