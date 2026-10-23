//! iOS 向け画面向き実装（Issue #656、親 #128）。`ios/fandhe_ios_screen_orientation.m`
//! （`build.rs` が iOS 分岐の feature `device` 有効時にコンパイルする）を `unsafe extern "C"`
//! 3 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/screen_orientation.rs` の iOS 経路が `implementation()` を呼ぶ（dispatch・共通コア
//! 画面・trait は #655 で確定済みで触らない）。
//!
//! # 方式
//!
//! eframe は winit の `Window` を外へ出さず `WindowExtIOS` が使えないため、winit の
//! `WinitUIViewController` の `supportedInterfaceOrientations` を `.m` 側で差し替え、
//! `setNeedsUpdateOfSupportedInterfaceOrientations` と `requestGeometryUpdate` で固定する。
//! 読み取りは `effectiveGeometry.interfaceOrientation`（前例: `ios/fandhe_ios_fullscreen_immersive.m`）。
//! iPad・Mac 上の iOS アプリは `can_lock = false` で、固定は `Unsupported`（取得のみ）。
//!
//! # 状態
//!
//! Rust 側は状態を持たない。成功の確定と期限（3 秒）・失敗時の復元は driver が持つ。
//! 本実装は依頼を同期で受け付け、`read` で実際の向きと OS 側の希望を正直に返すだけ。
//! 失敗（`Err`）のときは OS 側の希望を変えない。
//!
//! # 再描画（契約表 4）
//!
//! `requestGeometryUpdate` の errorHandler が失敗を伝える唯一の OS コールバック。
//! `fandhe_rust_screen_orientation_geometry_failed` が `crate::ios::request_repaint` を呼び、
//! driver に次フレームの読み戻しをさせる。回転自体はリサイズとして egui に届く。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内で読み戻しが実際の向きと希望に追従、固定できていない状態を固定中と
//! 見せない。守らない: 回転アニメーション中の遅れ、背景遷移中・root が winit の VC でない間
//! （`Internal` を返す）、マルチウィンドウでの OS による無視、終了時の後片付け。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::screen_orientation::ScreenOrientation;
use fandhe_device::screen_orientation::{parse_reading, Orientation, OrientationReading};
use fandhe_device::DeviceError;

// `.m` が定義する C ABI。戻りコードの契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_screen_orientation_can_lock() -> i32;
    fn fandhe_ios_screen_orientation_request(lock_code: i32) -> i32;
    fn fandhe_ios_screen_orientation_read(
        orientation: *mut i32,
        degrees: *mut i32,
        lock: *mut i32,
    ) -> i32;
}

/// `read` の戻りコードと out 値を契約の読み戻しへ写す。0 以外は値に関係なく `Internal`。
pub(crate) fn decode_read(
    status: i32,
    orientation: i32,
    degrees: i32,
    lock: i32,
) -> Result<OrientationReading, DeviceError> {
    if status != 0 {
        return Err(DeviceError::Internal("screen-orientation"));
    }
    parse_reading(orientation, degrees, lock)
}

/// `request` の戻りコードを契約のエラー種別へ写す。非対応は 4 のみ、その他の失敗は `Internal`。
pub(crate) fn decode_request(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        4 => Err(DeviceError::Unsupported("screen-orientation")),
        _ => Err(DeviceError::Internal("screen-orientation")),
    }
}

/// 固定の依頼を `.m` の lock コード（0 = 解除、1 = 縦、2 = 横）へ写す。
pub(crate) fn lock_code(target: Option<Orientation>) -> i32 {
    match target {
        None => 0,
        Some(Orientation::Portrait) => 1,
        Some(Orientation::Landscape) => 2,
    }
}

/// iOS の `ScreenOrientation` 実装（VC の `supportedInterfaceOrientations` 差し込み）。
#[cfg(target_os = "ios")]
struct IosScreenOrientation;

#[cfg(target_os = "ios")]
impl ScreenOrientation for IosScreenOrientation {
    #[allow(unsafe_code)]
    fn read(&self) -> Result<OrientationReading, DeviceError> {
        let (mut o, mut d, mut l) = (0_i32, 0_i32, 0_i32);
        // SAFETY: 3 つの引数はこのスタック上のローカル変数への有効な可変ポインタで、呼び出しの間
        // 生きている。`.m` は main スレッド以外では書き込まず 2 を返す。
        let status = unsafe { fandhe_ios_screen_orientation_read(&mut o, &mut d, &mut l) };
        decode_read(status, o, d, l)
    }

    #[allow(unsafe_code)]
    fn request_lock(&self, target: Option<Orientation>) -> Result<(), DeviceError> {
        // SAFETY: 引数は値渡しの i32 のみでポインタを経由しない。main 検査は `.m` 側で行う。
        decode_request(unsafe { fandhe_ios_screen_orientation_request(lock_code(target)) })
    }

    #[allow(unsafe_code)]
    fn can_lock(&self) -> bool {
        // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行う。
        unsafe { fandhe_ios_screen_orientation_can_lock() == 1 }
    }
}

/// `requestGeometryUpdate` の errorHandler（main queue 上）から呼ばれる。状態は変えず、
/// 次フレームを起こして driver に読み戻しをさせる（入力が無いと egui が描画しないため）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_screen_orientation_geometry_failed() {
    crate::ios::request_repaint();
}

/// `device/screen_orientation.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn ScreenOrientation> {
    Arc::new(IosScreenOrientation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::screen_orientation::Rotation;

    #[test]
    fn decode_request_maps_codes() {
        assert!(decode_request(0).is_ok());
        assert!(matches!(
            decode_request(4),
            Err(DeviceError::Unsupported(_))
        ));
        for c in [2, 3, 5, -1, 99] {
            assert!(
                matches!(decode_request(c), Err(DeviceError::Internal(_))),
                "{c}"
            );
        }
    }

    #[test]
    fn decode_read_rejects_nonzero_status() {
        for s in [2, 3, -1] {
            assert!(matches!(
                decode_read(s, 1, 0, 0),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn decode_read_rejects_unknown_values() {
        assert!(matches!(
            decode_read(0, 0, 45, 3),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn decode_read_accepts_valid() {
        let r = decode_read(0, 2, 90, 2).unwrap();
        assert_eq!(r.orientation, Orientation::Landscape);
        assert_eq!(r.rotation, Rotation::Deg90);
        assert_eq!(r.lock, Some(Orientation::Landscape));
    }

    #[test]
    fn lock_code_round_trip() {
        assert_eq!(lock_code(None), 0);
        assert_eq!(lock_code(Some(Orientation::Portrait)), 1);
        assert_eq!(lock_code(Some(Orientation::Landscape)), 2);
    }
}
