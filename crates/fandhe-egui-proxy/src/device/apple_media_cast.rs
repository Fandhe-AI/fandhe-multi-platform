//! macOS 向け外部機器への再生実装（Issue #386。iOS は親 #99 の b-ios が cfg を広げて共用する）。
//! `device/media_cast.rs::platform_cast` の macOS 経路から呼ばれる。
//! `apple/fandhe_apple_media_cast.m`（`AVRoutePickerView` + `AVAudioPlayer` + Core Audio の読み取り、
//! `build.rs` の macOS 分岐がコンパイル）を C ABI で呼ぶ薄いラッパーで、世代の採番だけを Rust 側で持つ。
//! 戻りコードを契約上のエラーへ写す純関数は [`result_from_code`]、状態コードは [`playback_from_code`] /
//! [`route_from_code`]。`unsafe` は extern 呼び出しに閉じ、出力先の名前はログに出さず
//! `fandhe_device::device_info::clean_text` を通す。
//! 呼び出しはすべて egui の update（main thread）から来る前提で、`.m` は main thread 以外を拒否する。
//!
//! 出力先のボタンは OS のもの（[`PickerKind::Native`]）が窓の右下に重なるため、`show_picker` は
//! `Unsupported` を返す（ドライバは失敗にせず、画面が案内を出す）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use fandhe_device::device_info::clean_text;
use fandhe_device::media_cast::{
    CastFailure, CastPlayback, CastSnapshot, MediaCast, PickerKind, RouteState,
};
use fandhe_device::DeviceError;

/// 名前を受ける固定長バッファの大きさ（NUL を含む。`clean_text` が 128 文字へ切る前の上限）。
const NAME_BYTES: usize = 512;

/// 現在の OS 向け実装を返す。
pub(crate) fn implementation() -> Arc<dyn MediaCast> {
    Arc::new(AppleMediaCast)
}

struct AppleMediaCast;

/// `prepare` ごとに進める世代（0 は「資源なし」を表すため 1 から）。
static GENERATION: AtomicU64 = AtomicU64::new(0);

// SAFETY: `apple/fandhe_apple_media_cast.m` が定義する C ABI（`build.rs` がコンパイルし
// AVKit・AVFoundation・CoreAudio をリンク）。契約は `.m` 冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_cast_prepare(generation: u64, wav: *const u8, len: usize) -> i32;
    fn fandhe_apple_cast_play(generation: u64) -> i32;
    fn fandhe_apple_cast_stop(generation: u64) -> i32;
    fn fandhe_apple_cast_snapshot(
        generation: *mut u64,
        playback: *mut i32,
        route: *mut i32,
        name: *mut u8,
        name_cap: usize,
    );
    fn fandhe_apple_cast_release();
}

/// `.m` の戻りコードを契約へ写す純関数。OS の生値は見ず固定の種別へ畳む。
fn result_from_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported("media_cast")),
        2 => Err(DeviceError::InvalidInput("media_cast wav")),
        6 => Err(DeviceError::InvalidInput("media_cast generation")),
        _ => Err(DeviceError::Internal("media_cast")),
    }
}

/// `.m` の playback コードを契約の状態へ写す純関数。未知のコードは成功にせず `Internal` 失敗。
fn playback_from_code(code: i32) -> CastPlayback {
    match code {
        0 => CastPlayback::Idle,
        1 => CastPlayback::Ready,
        2 => CastPlayback::Playing,
        3 => CastPlayback::Stopped,
        _ => CastPlayback::Failed(CastFailure::Internal),
    }
}

/// `.m` の route コードを経路へ写す純関数。名前は `Connected` のときだけ使う。
/// 未知のコードは `NotReported`（接続中と言わない）。
fn route_from_code(code: i32, name: &[u8]) -> RouteState {
    match code {
        0 => RouteState::Local,
        1 => RouteState::Choosing,
        2 => RouteState::Connected {
            name: name_from_buffer(name),
        },
        3 => RouteState::Disconnected,
        _ => RouteState::NotReported,
    }
}

/// NUL 終端の UTF-8 バッファから表示用の名前を取る。不正な UTF-8 は捨てて `None`。
fn name_from_buffer(buf: &[u8]) -> Option<String> {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end]).ok().and_then(clean_text)
}

impl MediaCast for AppleMediaCast {
    #[allow(unsafe_code)]
    fn prepare(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError> {
        if wav.is_empty() {
            return Err(DeviceError::InvalidInput("media_cast wav"));
        }
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        // SAFETY: `wav` はこの呼び出しの間生きており、`.m` は同期でコピーしてポインタを保持しない。
        // 長さは `wav.len()` と一致する。
        let code = unsafe { fandhe_apple_cast_prepare(generation, wav.as_ptr(), wav.len()) };
        result_from_code(code).map(|()| generation)
    }

    #[allow(unsafe_code)]
    fn play(&self, generation: u64) -> Result<(), DeviceError> {
        // SAFETY: 値渡しの引数のみ。`.m` は 1 本のミューテックスの内側で処理する。
        result_from_code(unsafe { fandhe_apple_cast_play(generation) })
    }

    #[allow(unsafe_code)]
    fn stop(&self, generation: u64) -> Result<(), DeviceError> {
        // SAFETY: 同上。
        result_from_code(unsafe { fandhe_apple_cast_stop(generation) })
    }

    fn show_picker(&self, _generation: u64) -> Result<(), DeviceError> {
        // OS のボタンが窓の右下に重なる。アプリからは開かない。
        Err(DeviceError::Unsupported("media_cast picker"))
    }

    #[allow(unsafe_code)]
    fn snapshot(&self) -> CastSnapshot {
        let (mut generation, mut playback, mut route) = (0u64, 0i32, 0i32);
        let mut name = [0u8; NAME_BYTES];
        // SAFETY: 出力先はこの呼び出し中のみ有効なスタック上の値と固定長バッファで、`.m` は必ず 1 回ずつ書き、
        // バッファへは `NAME_BYTES - 1` バイトまでと NUL 終端だけを書く。
        unsafe {
            fandhe_apple_cast_snapshot(
                &mut generation,
                &mut playback,
                &mut route,
                name.as_mut_ptr(),
                NAME_BYTES,
            )
        };
        CastSnapshot {
            generation,
            playback: playback_from_code(playback),
            route: route_from_code(route, &name),
            picker: PickerKind::Native,
        }
    }

    #[allow(unsafe_code)]
    fn release(&self) {
        // SAFETY: 引数なし。`.m` 側は冪等で、player・picker・delegate を解放する。
        unsafe { fandhe_apple_cast_release() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_contract_errors() {
        assert!(result_from_code(0).is_ok());
        assert!(matches!(
            result_from_code(1),
            Err(DeviceError::Unsupported("media_cast"))
        ));
        assert!(matches!(
            result_from_code(2),
            Err(DeviceError::InvalidInput("media_cast wav"))
        ));
        assert!(matches!(
            result_from_code(6),
            Err(DeviceError::InvalidInput("media_cast generation"))
        ));
        for c in [3, 4, 99] {
            assert!(matches!(result_from_code(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn playback_unknown_code_fails() {
        assert_eq!(playback_from_code(2), CastPlayback::Playing);
        assert_eq!(playback_from_code(3), CastPlayback::Stopped);
        assert_eq!(
            playback_from_code(4),
            CastPlayback::Failed(CastFailure::Internal)
        );
        assert_eq!(
            playback_from_code(42),
            CastPlayback::Failed(CastFailure::Internal)
        );
    }

    #[test]
    fn route_unknown_is_not_reported_and_name_is_cleaned() {
        assert_eq!(route_from_code(0, b""), RouteState::Local);
        assert_eq!(route_from_code(4, b"x"), RouteState::NotReported);
        assert_eq!(route_from_code(99, b"x"), RouteState::NotReported);
        assert_eq!(
            route_from_code(2, b" Room\x07\n\0junk"),
            RouteState::Connected {
                name: Some("Room".into())
            }
        );
        // 名前が空・不正な UTF-8 のときは名前なし（接続は OS が通知した事実なので残す）
        assert_eq!(
            route_from_code(2, b"\0"),
            RouteState::Connected { name: None }
        );
        assert_eq!(
            route_from_code(2, &[0xff, 0xfe, 0]),
            RouteState::Connected { name: None }
        );
    }

    #[test]
    fn show_picker_is_unsupported_and_empty_wav_is_invalid() {
        let c = AppleMediaCast;
        assert!(matches!(c.show_picker(1), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            c.prepare(Arc::from(Vec::<u8>::new())),
            Err(DeviceError::InvalidInput("media_cast wav"))
        ));
    }
}
