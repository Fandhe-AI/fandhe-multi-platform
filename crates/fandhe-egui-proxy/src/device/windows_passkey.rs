//! Windows 向けパスキー実装の置き場（Issue #342 はスタブ、#343 が `webauthn.h` で埋める）。
//! `device/passkey.rs` の Windows 経路が UI スレッドで [`implementation`] を呼び、返った [`Passkey`] を
//! ワーカースレッドで走らせる。`windows` crate の FFI は本モジュール内で `cfg(target_os = "windows")` に
//! 閉じる。dispatch・画面・文言は触らない。
//!
//! 現状は常に `Unsupported` を返す。対応表の Windows セルは `possible` のままにし、`impl` に書かない。

use fandhe_device::passkey::{Passkey, UnsupportedPasskey};

/// Windows の [`Passkey`] 実装を返す（UI スレッドから呼ばれる）。
pub(crate) fn implementation() -> Box<dyn Passkey> {
    Box::new(UnsupportedPasskey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::passkey::PasskeyOp;
    use fandhe_device::DeviceError;

    #[test]
    fn stub_is_never_success() {
        assert!(matches!(
            implementation().run(PasskeyOp::Register),
            Err(DeviceError::Unsupported("passkey"))
        ));
    }
}
