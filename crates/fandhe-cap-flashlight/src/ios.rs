//! iOS 向けライト（トーチ）実装（Issue #460、親 #106）。`ios/fandhe_ios_flashlight.m` を呼ぶ
//! C ABI ラッパーで、`driver` の dispatch（iOS 経路）から呼ばれる。契約は
//! `crate::contract`。
//!
//! 状態は Rust 側に持たず、点灯の有無は `.m` が `AVCaptureDevice.isTorchActive` で読み戻す。
//! 結果コードの写像は `crate::contract::decode_*`（SSOT）に任せる。`.m` の失敗コード
//! （set = 2、read = -3）は未知値として `Internal` になる。ライト非搭載（シミュレータ・一部の
//! iPad）は `Unsupported`。背景遷移では `.m` の observer が自分から消灯し、前景に戻っても
//! 点け直さない。
//!
//! 守る範囲: 1 プロセス内の main 直列の順序、背景中の点灯拒否、嘘の成功の禁止、`lock` した
//! 後の確実な `unlock`。守らない範囲: 外部からの点け消しへの追従は 1 秒 + 1 フレーム遅れる、
//! 熱による減光、プロセス終了時の後片付け（OS に任せる）。

use std::sync::Arc;

use crate::contract::Flashlight;
#[cfg(target_os = "ios")]
use crate::contract::{decode_read, decode_set};
use crate::error::FlashlightError;

#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_flashlight_set(on: i32) -> i32;
    fn fandhe_ios_flashlight_read() -> i32;
}

struct IosFlashlight;

impl Flashlight for IosFlashlight {
    fn set_on(&self, on: bool) -> Result<(), FlashlightError> {
        #[cfg(target_os = "ios")]
        {
            // SAFETY: 引数・戻り値は値渡しの i32 だけ。main スレッドの検査は `.m` 側で行う。
            #[allow(unsafe_code)]
            let code = unsafe { fandhe_ios_flashlight_set(i32::from(on)) };
            decode_set(code)
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = on;
            Err(FlashlightError::Unsupported)
        }
    }

    fn is_on(&self) -> Result<Option<bool>, FlashlightError> {
        #[cfg(target_os = "ios")]
        {
            // SAFETY: 引数なし・戻り値は値渡しの i32 だけ。main スレッドの検査は `.m` 側で行う。
            #[allow(unsafe_code)]
            let code = unsafe { fandhe_ios_flashlight_read() };
            decode_read(code)
        }
        #[cfg(not(target_os = "ios"))]
        {
            Err(FlashlightError::Unsupported)
        }
    }
}

/// iOS の実装を返す（`driver` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn Flashlight> {
    Arc::new(IosFlashlight)
}

/// `.m` の背景遷移 observer が消灯した直後に呼ばれ、次フレームを起こす（契約表 4）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn fandhe_rust_flashlight_changed() {
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use crate::contract::{decode_read, decode_set};
    use crate::error::FlashlightError;

    /// `.m` が返す失敗コード（set = 2、read = -3）と非搭載コードが契約どおりに写ること。
    #[test]
    fn m_codes_map_to_contract() {
        assert!(matches!(decode_set(2), Err(FlashlightError::Internal(_))));
        assert!(matches!(decode_set(1), Err(FlashlightError::Unsupported)));
        assert!(matches!(decode_read(-3), Err(FlashlightError::Internal(_))));
        assert!(matches!(decode_read(-1), Err(FlashlightError::Unsupported)));
    }
}
