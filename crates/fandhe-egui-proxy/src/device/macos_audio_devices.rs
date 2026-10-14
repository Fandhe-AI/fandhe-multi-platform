//! macOS 向け音声デバイス一覧実装（Issue #1330）。`macos/fandhe_macos_audio_devices.m`
//! （Core Audio のデバイス列挙、`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルし CoreAudio をリンクする）を `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` は
//! この呼び出しに閉じ `// SAFETY:` で契約を明記する。`device/audio_devices.rs` の macOS 経路から
//! 呼ばれ、契約は `fandhe_device::audio_devices`。
//!
//! `.m` が返す固定長バッファは [`from_raw`] で契約の値へ写す。写像はホストでもテストできるよう
//! モジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井
//!
//! - 名前は `kAudioObjectPropertyName`。種別・既定・使用中は持たず、デバイス ID・UID は読まない
//! - I/O を開かないため TCC のマイク確認は掛からない見込み（実機確認は #1329）
//! - 入力と出力の両方を持つデバイスは両方に出る

use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use fandhe_device::audio_devices::require_listed;
use fandhe_device::audio_devices::{listed, AudioDeviceList, AudioDirection, MAX_DEVICES};

const NAME_BYTES: usize = 512;

/// `.m` の `FandheAudioDevicesRaw` と同じレイアウト。
#[repr(C)]
struct Raw {
    input_count: i32,
    output_count: i32,
    inputs: [[u8; NAME_BYTES]; MAX_DEVICES],
    outputs: [[u8; NAME_BYTES]; MAX_DEVICES],
}

/// 1 方向を写す。件数が 0..=64 の外なら信頼できないので `Unavailable`。名前は NUL 終端と UTF-8 を
/// 検証し、満たさなければ `None`（件数には数える）。
fn direction(count: i32, names: &[[u8; NAME_BYTES]; MAX_DEVICES]) -> AudioDirection {
    let Some(n) = usize::try_from(count).ok().filter(|&n| n <= MAX_DEVICES) else {
        return AudioDirection::Unavailable;
    };
    listed(names[..n].iter().map(|buf| {
        CStr::from_bytes_until_nul(buf)
            .ok()
            .and_then(|c| c.to_str().ok())
            .map(str::to_owned)
    }))
}

/// `.m` の生値を契約の値へ写す。
fn from_raw(raw: &Raw) -> AudioDeviceList {
    AudioDeviceList {
        inputs: direction(raw.input_count, &raw.inputs),
        outputs: direction(raw.output_count, &raw.outputs),
    }
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_audio_devices.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_audio_devices_list(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn list(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_audio_devices_list(out) }
    }
}

#[cfg(target_os = "macos")]
struct MacAudioDevices;

#[cfg(target_os = "macos")]
impl fandhe_device::audio_devices::AudioDevices for MacAudioDevices {
    fn list(&self) -> Result<AudioDeviceList, fandhe_device::DeviceError> {
        // 約 64KB。UI スレッドのスタックで足りる。
        let mut raw = Raw {
            input_count: 0,
            output_count: 0,
            inputs: [[0; NAME_BYTES]; MAX_DEVICES],
            outputs: [[0; NAME_BYTES]; MAX_DEVICES],
        };
        if ffi::list(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal("audio_devices list"));
        }
        require_listed(from_raw(&raw))
    }
}

/// macOS の `AudioDevices` 実装を返す（`device/audio_devices.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Arc<dyn fandhe_device::audio_devices::AudioDevices> {
    Arc::new(MacAudioDevices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::audio_devices::AudioDevice;

    fn blank() -> Box<Raw> {
        Box::new(Raw {
            input_count: 0,
            output_count: 0,
            inputs: [[0; NAME_BYTES]; MAX_DEVICES],
            outputs: [[0; NAME_BYTES]; MAX_DEVICES],
        })
    }

    fn put(slot: &mut [u8; NAME_BYTES], s: &[u8]) {
        slot[..s.len()].copy_from_slice(s);
    }

    fn names(d: &AudioDirection) -> Vec<Option<&str>> {
        match d {
            AudioDirection::Listed(v) => {
                v.iter().map(|d: &AudioDevice| d.name.as_deref()).collect()
            }
            other => panic!("Listed ではない: {other:?}"),
        }
    }

    #[test]
    fn maps_normal_values() {
        let mut r = blank();
        r.input_count = 1;
        put(&mut r.inputs[0], "内蔵マイク".as_bytes());
        r.output_count = 0;
        let l = from_raw(&r);
        assert_eq!(names(&l.inputs), [Some("内蔵マイク")]);
        assert_eq!(l.outputs, AudioDirection::Listed(vec![]));
    }

    #[test]
    fn out_of_range_count_is_unavailable() {
        let mut r = blank();
        r.input_count = -1;
        r.output_count = 65;
        let l = from_raw(&r);
        assert_eq!(l.inputs, AudioDirection::Unavailable);
        assert_eq!(l.outputs, AudioDirection::Unavailable);
    }

    #[test]
    fn bad_names_become_none_but_are_counted() {
        let mut r = blank();
        r.output_count = 3;
        r.outputs[0] = [b'a'; NAME_BYTES];
        put(&mut r.outputs[1], &[0xff, 0xfe]);
        let l = from_raw(&r);
        assert_eq!(names(&l.outputs), [None, None, None]);
    }

    #[test]
    fn control_chars_are_removed() {
        let mut r = blank();
        r.input_count = 1;
        put(&mut r.inputs[0], b"Mic\x01\n 2");
        assert_eq!(names(&from_raw(&r).inputs), [Some("Mic 2")]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_os_call_succeeds() {
        // CI の Mac にデバイスが無くても `Listed(空)` で成功する。
        assert!(implementation().list().is_ok());
    }
}
