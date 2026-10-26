//! iOS 向けアプリ・端末の識別子実装（Issue #904、親 #181）。`device/device_identifier.rs` の
//! iOS 経路（`platform_device_identifier`）から呼ばれる。`ios/fandhe_ios_device_identifier.m`
//! （`UIDevice.identifierForVendor`、`build.rs` が iOS ターゲットの feature `device` 有効時に
//! コンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。dispatch・共通コア画面・`registry.rs` は
//! 変えない。契約は `fandhe_device::device_identifier`。
//!
//! `.m` の戻り値コードとバッファは [`from_raw`] で契約の値へ写す（ホストでテストできるよう
//! モジュールは `cfg(any(ios, test))`）。
//!
//! - 0: 値。空文字は `DeviceId::new` が拒み `Internal`（trim・形式検査はしない）
//! - 3（nil）: `NotYetAvailable`。失敗にせず、取り直しは利用者の押し直し（自動再試行しない）
//! - それ以外（引数不正・main thread 以外・UTF-8 化失敗・容量不足）: `Internal`
//!
//! # 再描画・ライフサイクル
//!
//! OS コールバックを持たず結果を共有状態へ書く経路が無いため、`crate::ios::request_repaint` は
//! 呼ばない。再描画は `drive` の完了通知（`CompletionSender::send`）が起こす。同期の一発読みで
//! 保留中の要求を持たないので、バックグラウンド遷移などで破棄するものは無い。
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 1 プロセス内での取得、nil・空・失敗の区別、値の非保持・非ログ出力。守らない: 値がいつ
//! 変わるか（同じベンダーのアプリを全て消すと変わる）、nil がいつ解けるか、読み取り後の変化の反映。
//! 実機の目視確認は #889。

#[cfg(target_os = "ios")]
use fandhe_device::device_identifier::DeviceIdentifier;
use fandhe_device::device_identifier::{DeviceId, IdentifierRead};
use fandhe_device::DeviceError;

/// `.m` の戻り値コードとバッファを契約の値へ写す。
fn from_raw(code: i32, buf: &[u8]) -> Result<IdentifierRead, DeviceError> {
    const FAILED: DeviceError = DeviceError::Internal("device_identifier read");
    match code {
        0 => {
            let s = std::ffi::CStr::from_bytes_until_nul(buf)
                .ok()
                .and_then(|c| c.to_str().ok())
                .ok_or(FAILED)?;
            DeviceId::new(s.to_string()).map(IdentifierRead::Identifier)
        }
        3 => Ok(IdentifierRead::NotYetAvailable),
        _ => Err(FAILED),
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `ios/fandhe_ios_device_identifier.m`（`build.rs` が iOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_device_identifier_read(out: *mut u8, cap: usize) -> i32;
    }

    /// 戻り値コードを返す。
    #[allow(unsafe_code)]
    pub(super) fn read(buf: &mut [u8]) -> i32 {
        // SAFETY: `buf` は呼び出しの間有効な排他参照で、`.m` はポインタを保持せず `cap` を超えて書かない。
        unsafe { fandhe_ios_device_identifier_read(buf.as_mut_ptr(), buf.len()) }
    }
}

#[cfg(target_os = "ios")]
struct IosDeviceIdentifier;

#[cfg(target_os = "ios")]
impl DeviceIdentifier for IosDeviceIdentifier {
    fn read(&self) -> Result<IdentifierRead, DeviceError> {
        let mut buf = [0u8; 64];
        let code = ffi::read(&mut buf);
        from_raw(code, &buf)
    }
}

/// iOS の `DeviceIdentifier` 実装を返す（`device/device_identifier.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Box<dyn DeviceIdentifier> {
    Box::new(IosDeviceIdentifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &[u8]) -> [u8; 64] {
        let mut b = [0u8; 64];
        b[..s.len()].copy_from_slice(s);
        b
    }

    #[test]
    fn value_is_masked() {
        let r = from_raw(0, &buf(b"AAAAAAAA-0000-0000-0000-000000001234")).unwrap();
        let IdentifierRead::Identifier(id) = r else {
            panic!("expected Identifier");
        };
        assert_eq!(id.char_count(), 36);
        assert_eq!(id.masked_tail(), "1234");
    }

    #[test]
    fn nil_is_not_yet_available() {
        assert!(matches!(
            from_raw(3, &buf(b"")),
            Ok(IdentifierRead::NotYetAvailable)
        ));
    }

    #[test]
    fn other_codes_are_internal() {
        for c in [1, 2, 4, 99] {
            assert!(matches!(
                from_raw(c, &buf(b"x")),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn empty_and_malformed_are_internal() {
        assert!(from_raw(0, &buf(b"")).is_err());
        assert!(from_raw(0, &[b'a'; 4]).is_err());
        assert!(from_raw(0, &buf(&[0xff, 0xfe])).is_err());
    }
}
