//! iOS 向け音声デバイス一覧実装（Issue #1331）。`ios/fandhe_ios_audio_devices.m`
//! （`AVAudioSession` の読み取り。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/audio_devices.rs` の iOS 経路から呼ばれ、契約は `fandhe_device::audio_devices`。
//!
//! 写像（[`from_raw`]）はホストでもテストできるようモジュールは `cfg(any(ios, test))`。
//!
//! # 再描画
//!
//! 同期読み取りで OS コールバックを持たないため `crate::ios::request_repaint` は呼ばない。結果は
//! `CompletionSender::send` で渡り、その中の `ctx.request_repaint()` が次フレームを起こす
//! （device-info の iOS 実装と同じ形）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 0 件と取得不可の区別。`AVAudioSession` は共有で書き込まない（mic-record が所有）。
//!   playback 系 category では `availableInputs` が空になるため、入力は `OnlyWhileMicActive` にして
//!   「マイクが無い」と偽らない。0 件は正常な空として `Listed` にし、
//!   nil（-1）・範囲外だけを `Unavailable` にする（例: 録音中は category が Record で出力が 0 件）。ID・種別は取らない。失敗を成功値へ畳まない
//! - 守らない: 出力は現在の経路で全候補ではない。読み取り後の経路変化（再取得まで）。複数プロセス。
//!   同名デバイスの区別
//!
//! # 既知の天井
//!
//! - 権限は不要（usage description・privacy manifest は増やさない）。実機確認は #1329

use std::ffi::CStr;
#[cfg(target_os = "ios")]
use std::sync::Arc;

use fandhe_device::audio_devices::{listed, AudioDeviceList, AudioDirection, MAX_DEVICES};
#[cfg(target_os = "ios")]
use fandhe_device::audio_devices::{require_listed, AudioDevices};

const NAME_BYTES: usize = 512;

/// `.m` の `FandheIosAudioDevicesRaw` と同じレイアウト。
#[repr(C)]
struct Raw {
    category: i32,
    input_count: i32,
    output_count: i32,
    inputs: [[u8; NAME_BYTES]; MAX_DEVICES],
    outputs: [[u8; NAME_BYTES]; MAX_DEVICES],
}

/// 入力の一覧が意味を持つ category か（1 = Record、2 = PlayAndRecord、3 = MultiRoute）。
/// 未知の値は入力不可に倒す。
fn category_allows_input(code: i32) -> bool {
    (1..=3).contains(&code)
}

/// 1 方向を写す。件数が 0..=64 の外（nil の -1・超過）は `Unavailable`。0 件は正常な空（`Listed(vec![])`）。名前は NUL 終端と
/// UTF-8 を検証し、満たさなければ `None`（件数には数える）。
fn direction(count: i32, names: &[[u8; NAME_BYTES]; MAX_DEVICES]) -> AudioDirection {
    let Some(n) = usize::try_from(count)
        .ok()
        .filter(|&n| (0..=MAX_DEVICES).contains(&n))
    else {
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
        inputs: if category_allows_input(raw.category) {
            direction(raw.input_count, &raw.inputs)
        } else {
            AudioDirection::OnlyWhileMicActive
        },
        outputs: direction(raw.output_count, &raw.outputs),
    }
}

#[cfg(target_os = "ios")]
mod ffi {
    use super::Raw;

    // SAFETY: `ios/fandhe_ios_audio_devices.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_audio_devices_list(out: *mut Raw) -> i32;
    }

    /// 0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn list(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_ios_audio_devices_list(out) }
    }
}

#[cfg(target_os = "ios")]
struct IosAudioDevices;

#[cfg(target_os = "ios")]
impl AudioDevices for IosAudioDevices {
    fn list(&self) -> Result<AudioDeviceList, fandhe_device::DeviceError> {
        // 約 64KB。UI スレッドのスタックで足りる。
        let mut raw = Raw {
            category: 0,
            input_count: -1,
            output_count: -1,
            inputs: [[0; NAME_BYTES]; MAX_DEVICES],
            outputs: [[0; NAME_BYTES]; MAX_DEVICES],
        };
        if ffi::list(&mut raw) != 0 {
            return Err(fandhe_device::DeviceError::Internal("audio_devices list"));
        }
        require_listed(from_raw(&raw))
    }
}

/// iOS の `AudioDevices` 実装を返す（`device/audio_devices.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn AudioDevices> {
    Arc::new(IosAudioDevices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::audio_devices::{require_listed, AudioDevice};

    fn blank() -> Box<Raw> {
        Box::new(Raw {
            category: 2,
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
    fn non_input_category_is_only_while_mic_active() {
        for cat in [0, 99] {
            let mut r = blank();
            r.category = cat;
            r.input_count = 1;
            put(&mut r.inputs[0], b"Mic");
            assert_eq!(from_raw(&r).inputs, AudioDirection::OnlyWhileMicActive);
        }
    }

    #[test]
    fn input_categories_list_names() {
        for cat in 1..=3 {
            let mut r = blank();
            r.category = cat;
            r.input_count = 1;
            put(&mut r.inputs[0], "内蔵マイク".as_bytes());
            r.output_count = 1;
            put(&mut r.outputs[0], b"Speaker");
            let l = from_raw(&r);
            assert_eq!(names(&l.inputs), [Some("内蔵マイク")]);
            assert_eq!(names(&l.outputs), [Some("Speaker")]);
        }
    }

    #[test]
    fn nil_or_over_count_is_unavailable() {
        for c in [-1, 65] {
            let mut r = blank();
            r.input_count = c;
            r.output_count = c;
            let l = from_raw(&r);
            assert_eq!(l.inputs, AudioDirection::Unavailable);
            assert_eq!(l.outputs, AudioDirection::Unavailable);
        }
    }

    #[test]
    fn zero_count_is_listed_empty() {
        let mut r = blank();
        r.input_count = 1;
        put(&mut r.inputs[0], b"Mic");
        let l = from_raw(&r);
        assert_eq!(l.outputs, AudioDirection::Listed(vec![]));
        assert!(require_listed(from_raw(&blank())).is_ok());
    }

    #[test]
    fn bad_names_become_none_but_are_counted() {
        let mut r = blank();
        r.output_count = 2;
        r.outputs[0] = [b'a'; NAME_BYTES];
        put(&mut r.outputs[1], &[0xff, 0xfe]);
        assert_eq!(names(&from_raw(&r).outputs), [None, None]);
    }

    #[test]
    fn control_chars_are_removed() {
        let mut r = blank();
        r.input_count = 1;
        put(&mut r.inputs[0], b"Mic\x01\n 2");
        assert_eq!(names(&from_raw(&r).inputs), [Some("Mic 2")]);
    }

    #[test]
    fn all_unknown_is_not_a_success() {
        let mut r = blank();
        r.category = 0;
        r.output_count = -1;
        assert!(require_listed(from_raw(&r)).is_err());
    }
}
