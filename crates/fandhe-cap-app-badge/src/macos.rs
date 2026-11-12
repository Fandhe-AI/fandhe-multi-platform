//! macOS 向けバッジ設定実装（Issue #1360）。`macos/fandhe_macos_app_badge.m`
//! （`NSDockTile.badgeLabel`、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ
//! `// SAFETY:` で契約を明記する。`fandhe-cap-app-badge/src/driver.rs` の macOS 経路から呼ばれ、契約は
//! `fandhe_cap_app_badge`。
//!
//! 結果コードの写像 [`outcome_from_code`] はホストでもテストできるようモジュールは
//! `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 未バンドル実行（`cargo run`）で Dock に出るかは未確認（Mac 目視は #1359）
//! - 表示は確かめられないため成功は `Submitted`（OS へ渡した）に留める

use crate::contract::BadgeOutcome;
use crate::error::AppBadgeError;

/// `.m` の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
/// 0 = OS へ渡した、それ以外（NSApp 無し・main thread 外・未知）は `Internal`。
pub(crate) fn outcome_from_code(code: i32) -> Result<BadgeOutcome, AppBadgeError> {
    match code {
        0 => Ok(BadgeOutcome::Submitted),
        _ => Err(AppBadgeError::Internal),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    // SAFETY: `macos/fandhe_macos_app_badge.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn fandhe_macos_app_badge_set(value: u32) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn set(value: u32) -> i32 {
        // SAFETY: 値渡しの `u32` だけで、`.m` はポインタを受け取らず保持しない。
        unsafe { fandhe_macos_app_badge_set(value) }
    }
}

#[cfg(target_os = "macos")]
struct MacAppBadge;

#[cfg(target_os = "macos")]
impl crate::contract::AppBadge for MacAppBadge {
    fn set(&self, value: crate::contract::BadgeValue) -> Result<BadgeOutcome, AppBadgeError> {
        outcome_from_code(ffi::set(value.get()))
    }
}

/// macOS の `AppBadge` 実装を返す（`fandhe-cap-app-badge/src/driver.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn crate::contract::AppBadge> {
    Box::new(MacAppBadge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_table() {
        assert!(matches!(outcome_from_code(0), Ok(BadgeOutcome::Submitted)));
        for code in [1, 2, 3, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_code(code),
                Err(AppBadgeError::Internal)
            ));
        }
    }
}
