//! iOS 向け複数ディスプレイ実装（Issue #27）。`ios/fandhe_ios_multi_display.m`
//! （`UIApplication.connectedScenes` の `UIWindowScene.screen`、`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの
//! 呼び出しに閉じる。`device/multi_display.rs` の iOS 経路から呼ばれ、契約は
//! `fandhe_device::multi_display`。`macos_multi_display.rs` と同型の薄いラッパーで、
//! 生値の写像（`Raw` / `from_raw`）は macOS 版を共用する。
//!
//! 外部ディスプレイは scene 化（`ios/fandhe_ios_scene.m`、Info.plist の
//! `UIApplicationSceneManifest`）で初めて列挙できる。`UIScreen.screens` は iOS 16 で非推奨で、
//! 外部画面を返す保証が無いため使わない。再描画は `CompletionSender` 経由のため
//! `crate::ios::request_repaint` は不要（契約表 4）。
//!
//! # 守る範囲
//!
//! 不正な値・scene 0 件を成功にしない／切断後に古い一覧を返さない（毎回 OS から読む）／
//! 個体識別子を読まない／1 プロセス内の順序（並びは主画面先頭＋（幅, 高さ）昇順で固定）。
//!
//! # 守らない範囲
//!
//! - ミラーリング中の画面は OS が返す scene のとおり（物理数と一致するとは限らない）
//! - Stage Manager で application の scene を外部へ移した場合の主画面判定
//! - 画面名は取れない（`UIScreen` に表示名の API が無く、常に `None`）
//! - 接続・切断の反映は最大 `READ_INTERVAL` 遅れる

use fandhe_device::multi_display::ScreenInfo;

use super::macos_multi_display::{from_raw, Raw, CAP};

// SAFETY: `ios/fandhe_ios_multi_display.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
// コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_ios_multi_display_read(out: *mut Raw, cap: u32, count: *mut u32) -> i32;
}

/// 0 = 成功。成功時は先頭 `count` 件が有効。
#[allow(unsafe_code)]
fn read_raw(out: &mut [Raw], count: &mut u32) -> i32 {
    // SAFETY: `out` は `out.len()` 件分の確保済みで排他的な領域、`cap` はその件数。
    // `count` は有効な書き込み先。`.m` はポインタを保持しない。
    unsafe { fandhe_ios_multi_display_read(out.as_mut_ptr(), out.len() as u32, count) }
}

struct IosMultiDisplay;

impl fandhe_device::multi_display::MultiDisplay for IosMultiDisplay {
    fn read(&self) -> Result<Vec<ScreenInfo>, fandhe_device::DeviceError> {
        let mut buf = [Raw {
            width_px: 0,
            height_px: 0,
            scale: 0.0,
            is_primary: 0,
            name: [0; 128],
        }; CAP];
        let mut count = 0u32;
        if read_raw(&mut buf, &mut count) != 0 || count as usize > CAP {
            return Err(fandhe_device::DeviceError::Internal("multi_display read"));
        }
        Ok(from_raw(&buf[..count as usize]))
    }
}

/// iOS の `MultiDisplay` 実装を返す（`device/multi_display.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Box<dyn fandhe_device::multi_display::MultiDisplay> {
    Box::new(IosMultiDisplay)
}
