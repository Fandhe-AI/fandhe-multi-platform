//! macOS・iOS 共用のロック画面・OS のメディア操作実装（Issue #381、親 #98、契約は `fandhe_device::now_playing`）。
//! `device/now_playing.rs::platform_now_playing` の macOS・iOS 経路（`ios_now_playing`）から呼ばれる。
//! `apple/fandhe_apple_now_playing.m`（MediaPlayer の `MPNowPlayingInfoCenter` / `MPRemoteCommandCenter`、
//! `build.rs` の macOS・iOS 分岐がコンパイル）を C ABI で呼ぶ薄いラッパー。曲名は
//! `fandhe_device::now_playing::DEMO_TITLE` を長さ付き UTF-8 の引数で渡し、`.m` が同期でコピーする
//! （SSOT を 1 か所にし、ポインタを保持させない）。
//!
//! OS からの操作は `.m` の handler が [`fandhe_rust_now_playing_command`] を呼び、
//! `device::now_playing::commands()` へ `push` するだけ（画面の状態には触れない、契約表 11）。
//! 受付停止中（`disarm` 済み）は拒否を OS へ返す。
//!
//! 結果は `.m` の戻りコードを [`result_from_code`] で固定の種別へ写す。Apple は表示の確定が返らないので
//! `Ok` は「OS へ渡した」までを意味する。`unsafe` は extern 呼び出しと `#[no_mangle]` の受け口に閉じ、
//! 曲名・位置・OS の生エラーはログに出さない。iOS（#382）は handler が結果を共有状態へ入れた直後に
//! `crate::ios::request_repaint` で次フレームを起こす（入力が無いと egui が止まる。契約表 4）。

#[cfg(any(target_os = "macos", target_os = "ios"))]
use std::sync::Arc;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use fandhe_device::now_playing::NowPlaying;
use fandhe_device::now_playing::OsMediaCommand;
use fandhe_device::DeviceError;

/// 現在の OS 向け実装を返す。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    Arc::new(AppleNowPlaying)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
struct AppleNowPlaying;

/// `.m` の戻りコードを契約へ写す純関数。OS の生値は見ず固定の種別へ畳む。
fn result_from_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        2 => Err(DeviceError::InvalidInput("now_playing")),
        _ => Err(DeviceError::Internal("now_playing")),
    }
}

/// `.m` から届く操作コード（1 = Play、2 = Pause、3 = Toggle）。未知のコードは `None`（成功にしない）。
fn command_from_code(code: i32) -> Option<OsMediaCommand> {
    match code {
        1 => Some(OsMediaCommand::Play),
        2 => Some(OsMediaCommand::Pause),
        3 => Some(OsMediaCommand::Toggle),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use super::*;
    use fandhe_device::now_playing::{NowPlayingInfo, DEMO_TITLE};

    // SAFETY: `apple/fandhe_apple_now_playing.m` が定義する C ABI（`build.rs` がコンパイルし
    // MediaPlayer をリンク）。契約は `.m` 冒頭コメントに集約する。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_apple_now_playing_publish(
            title: *const u8,
            len: usize,
            playing: i32,
            pos_ms: i64,
            dur_ms: i64,
        ) -> i32;
        fn fandhe_apple_now_playing_clear();
        fn fandhe_apple_now_playing_shutdown();
    }

    /// 未取得は負（`.m` が項目を入れない）。
    fn ms_or_unknown(v: Option<u64>) -> i64 {
        v.and_then(|v| i64::try_from(v).ok()).unwrap_or(-1)
    }

    impl NowPlaying for AppleNowPlaying {
        fn publish(&self, info: NowPlayingInfo) -> Result<(), DeviceError> {
            let title = DEMO_TITLE.as_bytes();
            // SAFETY: `title` はこの呼び出しの間だけ有効な長さ付きバッファで、`.m` は同期でコピーし
            // ポインタを保持しない。その他は値渡し。
            #[allow(unsafe_code)]
            let code = unsafe {
                fandhe_apple_now_playing_publish(
                    title.as_ptr(),
                    title.len(),
                    i32::from(info.playing),
                    ms_or_unknown(info.position_ms),
                    ms_or_unknown(info.duration_ms),
                )
            };
            result_from_code(code)
        }

        fn clear(&self) {
            // SAFETY: 引数なしの冪等な後片付け。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_apple_now_playing_clear()
            }
        }

        fn shutdown(&self) {
            // SAFETY: 引数なしの冪等な後片付け。
            #[allow(unsafe_code)]
            unsafe {
                fandhe_apple_now_playing_shutdown()
            }
        }
    }

    /// `.m` の handler から OS のスレッドで呼ばれる受け口。受け付けたら非 0、拒否（未知のコード・
    /// 受付停止中）は 0。panic しない。
    #[allow(unsafe_code)]
    #[no_mangle]
    pub extern "C" fn fandhe_rust_now_playing_command(code: i32) -> i32 {
        let accepted = match command_from_code(code) {
            Some(cmd) => i32::from(super::super::now_playing::commands().push(cmd)),
            None => 0,
        };
        #[cfg(target_os = "ios")]
        crate::ios::request_repaint();
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_codes_map_without_false_success() {
        assert!(result_from_code(0).is_ok());
        assert!(matches!(
            result_from_code(2),
            Err(DeviceError::InvalidInput(_))
        ));
        for code in [1, 3, 4, -1, 99] {
            assert!(matches!(
                result_from_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn command_codes_map_and_unknown_is_rejected() {
        assert_eq!(command_from_code(1), Some(OsMediaCommand::Play));
        assert_eq!(command_from_code(2), Some(OsMediaCommand::Pause));
        assert_eq!(command_from_code(3), Some(OsMediaCommand::Toggle));
        for code in [0, 4, -1, i32::MAX] {
            assert_eq!(command_from_code(code), None);
        }
    }
}
