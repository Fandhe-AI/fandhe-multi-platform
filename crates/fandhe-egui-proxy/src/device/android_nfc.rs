//! Android 向け NFC タグ読み取りの骨格（Issue #1044、親 #157）。`device/nfc.rs` の Android 経路
//! （`platform`）から呼ばれる。契約は `fandhe_device::nfc`。
//!
//! 本ファイルは常に `Unsupported` を返す薄いスタブ（嘘の成功を返さない）。(b-android) が
//! このファイルと Java・Manifest だけを埋め、dispatch（`device/nfc.rs`）・共通コア画面は触らない。
//! 実装時は `NdefMessage.toByteArray()` を `fandhe_device::nfc::parse_ndef` に渡し、
//! reader mode の解除を終えてから結果を返すこと。

use std::sync::Arc;

use fandhe_device::nfc::{NfcReader, UnsupportedNfcReader};

/// Android 実装を返す（現状はスタブ）。
pub(crate) fn implementation() -> Arc<dyn NfcReader> {
    Arc::new(UnsupportedNfcReader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::nfc::NfcReadFailure;
    use std::time::Duration;

    #[test]
    fn stub_never_succeeds() {
        assert_eq!(
            implementation().read_one(Duration::from_secs(1)),
            Err(NfcReadFailure::Unsupported)
        );
    }
}
