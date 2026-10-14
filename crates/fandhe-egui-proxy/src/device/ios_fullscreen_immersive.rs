//! iOS 向け全画面表示実装（Issue #1313、親 #1133）。`ios/fandhe_ios_fullscreen_immersive.m`
//! （`build.rs` が iOS 分岐の feature `device` 有効時にコンパイルする）を `unsafe extern "C"`
//! 2 関数で書く / 読む。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/fullscreen_immersive.rs` の iOS 経路が `implementation()` を呼ぶ（dispatch・共通コア
//! 画面は #1312 で確定済みで触らない）。
//!
//! # 方式
//!
//! eframe は winit の `Window` を外へ出さず `WindowExtIOS` が使えないため、winit の
//! `WinitUIViewController` の `prefersStatusBarHidden` / `prefersHomeIndicatorAutoHidden` を
//! `.m` 側で差し替える（前例: `ios/fandhe_ios_text_input.m`）。
//!
//! # 状態
//!
//! Rust 側は状態を持たない。`is_fullscreen` は VC の希望とステータスバーの実状態の読み戻し。
//! 差し込みや VC 取得の失敗は `Internal` / `None`（全画面と見せない）。期限（3 秒）は
//! effect ドライバが持つ。
//!
//! # 再描画（契約表 4）
//!
//! OS コールバックが共有状態へ結果を入れる経路は無く、完了は effect ドライバが毎フレーム同期で
//! 読み戻して判定する（確認待ちは `POLL_INTERVAL` で次フレームを予約）。そのため
//! `crate::ios::request_repaint` は呼ばない（該当なし）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内で表示が実状態に追従、失敗・読めない状態を全画面と見せない。
//! 守らない: 遷移中の一時的な表示ずれ、key window の root が winit の VC でない間（3 を返し反映しない）、横向きでの OS 自動非表示、
//! ホームインジケータの状態（希望するだけで読み戻せず、触ると出る）、終了時の後片付け
//! （OS がプロセスごと片付ける）。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::fullscreen_immersive::Fullscreen;
use fandhe_device::DeviceError;

// `.m` が定義する C ABI。戻りコードの契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_fullscreen_set(on: i32) -> i32;
    fn fandhe_ios_fullscreen_is_on() -> i32;
}

/// `set` の戻りコード（0 = 成功、それ以外 = 失敗）を契約のエラー種別へ写す。
fn decode_set(code: i32) -> Result<(), DeviceError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("fullscreen-immersive"))
    }
}

/// `is_on` の戻りコード。1 = 全画面、0 = 通常、それ以外（main 以外・読めない・未知値）は未取得。
fn decode_is_on(code: i32) -> Option<bool> {
    match code {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// iOS の `Fullscreen` 実装（VC の `prefersStatusBarHidden` 差し込み）。
#[cfg(target_os = "ios")]
struct IosFullscreen;

#[cfg(target_os = "ios")]
impl Fullscreen for IosFullscreen {
    #[allow(unsafe_code)]
    fn request(&self, on: bool) -> Result<(), DeviceError> {
        // SAFETY: 引数は値渡しの i32 のみでポインタを経由しない。`.m` 側で main スレッドを
        // 検査し、main 以外では UIKit に触れず 2 を返す。
        decode_set(unsafe { fandhe_ios_fullscreen_set(i32::from(on)) })
    }

    #[allow(unsafe_code)]
    fn is_fullscreen(&self) -> Option<bool> {
        // SAFETY: 引数なし・戻りは値渡しの i32。main 検査は `.m` 側で行う。
        decode_is_on(unsafe { fandhe_ios_fullscreen_is_on() })
    }
}

/// プロセスで 1 個の iOS 実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn Fullscreen> {
    Arc::new(IosFullscreen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_set_maps_nonzero_to_internal() {
        assert!(decode_set(0).is_ok());
        for code in [2, 3, -1] {
            assert!(matches!(
                decode_set(code),
                Err(DeviceError::Internal("fullscreen-immersive"))
            ));
        }
    }

    #[test]
    fn decode_is_on_never_fakes_unreadable() {
        assert_eq!(decode_is_on(1), Some(true));
        assert_eq!(decode_is_on(0), Some(false));
        for code in [2, 3, 99, -1] {
            assert_eq!(decode_is_on(code), None);
        }
    }
}
