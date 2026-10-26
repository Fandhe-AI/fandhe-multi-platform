//! iOS 向け「電話発信・地図など他アプリの起動」の骨格（親 #176、Issue #861）。`device/external_app_launch.rs` の
//! iOS 経路（dispatch）から呼ばれる。実体は (b-ios) が埋めるまで `Unsupported` を返すスタブで、
//! 成功を装わない。契約の SSOT は `fandhe_device::external_app_launch`。
//!
//! # (b-ios) への約束
//!
//! - 触ってよいのは本ファイル・`ios/fandhe_ios_external_app_launch.m`・`build.rs` の iOS 分岐だけ。
//!   dispatch・fallback 行・`device.rs`・共通コア画面・`registry.rs` は触らない
//! - `UIApplication.open(_:options:completionHandler:)` は main queue で呼ぶ
//! - completion は 1 回だけ `bridge::deliver` へ送り、直後に `crate::ios::request_repaint` を呼ぶ
//!   （入力がないと egui が描画せず、画面が処理中のまま止まる）
//! - 結果コードは `device/external_app_launch.rs` の `outcome_from_ios_code` に従う
//! - 渡す値は `fandhe_device::external_app_launch` の `PHONE_URI` / `APPLE_MAPS_URL` だけ
//! - `canOpenURL` と `LSApplicationQueriesSchemes` は使わない。期限・タイマーは設けない

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
