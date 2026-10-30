//! iOS 向けアラーム実装のスタブ（Issue #1076、親 #1056）。`device/alarm.rs` の iOS 経路から呼ばれる。
//! 契約の SSOT は `fandhe_device::alarm`。実体は #1077 が本ファイルの `implementation()` を差し替える。
//!
//! # #1077 への約束
//!
//! - Rust で触るのは本ファイルだけ。dispatch・fallback 行・`device.rs` の `pub mod` 列・共通コア画面は触らない
//! - 過去時刻の検査には `super::alarm::{now_unix_ms, validate_fire_at}` を使う
//! - 戻りコードの意味は Rust 側の `match` を SSOT にする
//! - iOS 固有: 期限なしで実結果を待つ。読み直しで同 id が無ければ `Internal("alarm add")`。iOS 26 未満は `Unsupported`
//!
//! スタブは `Unsupported` を返し、成功へ倒さない。

use std::sync::Arc;

use fandhe_device::alarm::{Alarm, UnsupportedAlarm};

/// iOS の `Alarm` 実装を返す（#1077 が実体に差し替える）。
pub(crate) fn implementation() -> Arc<dyn Alarm> {
    Arc::new(UnsupportedAlarm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_not_a_fake_success() {
        let a = implementation();
        assert!(matches!(
            a.schedule(u64::MAX),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(a.cancel(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(a.state(), Err(DeviceError::Unsupported(_))));
    }
}
