//! iOS / macOS 向け MIDI 入力実装（Issue #1398 macOS、#1399 iOS）。
//! `device/midi.rs::platform_midi` の Apple 分岐（`any(target_os = "ios", target_os = "macos")`）
//! から呼ばれる。iOS・macOS とも `apple/fandhe_apple_midi.m`（CoreMIDI、`build.rs` の各分岐が
//! コンパイル）を C ABI で呼ぶ。
//!
//! # 再描画（契約表 4）
//!
//! `.m` のコールバックが結果を sink へ渡した直後、iOS では `crate::ios::request_repaint` で
//! 次フレームを起こす（入力が無いと egui が描画せず、一覧が止まって見えるため）。
//!
//! `.m` の受信スレッドが [`fandhe_rust_midi_ump`] / [`fandhe_rust_midi_sources`] を呼び、
//! 保持している [`MidiSink`] へ流す。sink が `false`（受け手なし）を返したら SINK を外すだけで、
//! 受信スレッドから `.m` の stop は呼ばない（受け手が消えるのはアプリ終了時だけ）。
//! 演奏内容はログに出さない。`unsafe` は FFI 呼び出しとポインタ→スライスの 2 種に閉じる。

use std::sync::Arc;

use fandhe_device::midi::{MidiInput, MidiSink};
use fandhe_device::DeviceError;

/// 現在の OS 向け実装を返す。
pub(crate) fn implementation() -> Arc<dyn MidiInput> {
    Arc::new(AppleMidiInput)
}

struct AppleMidiInput;

impl MidiInput for AppleMidiInput {
    fn start(&self, sink: MidiSink) -> Result<(), DeviceError> {
        imp::start(sink)
    }

    fn stop(&self) {
        imp::stop();
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use fandhe_device::midi::{notes_from_ump_words, MidiEvent, MidiSink};
    use fandhe_device::DeviceError;

    /// 1 回のコールバックで受け付ける最大ワード数（異常値で巨大スライスを作らない）。
    const MAX_WORDS: u32 = 1 << 16;

    /// 現在の購読の (世代, sink)。世代は `start` ごとに採番し `.m` へ渡す。`.m` は受信・通知の
    /// たびにその時点の世代を返すので、停止→再開をまたいだ古いコールバックは照合で捨てる。
    static SINK: Mutex<Option<Sub>> = Mutex::new(None);

    /// 購読 1 件の状態。`starting` の間は `Started` を先に届けるため、届いた接続数を `pending`
    /// に保持し、`start` が `Started` を送った後に最新値を反映する（Started と Sources の順序保証）。
    struct Sub {
        epoch: u64,
        sink: MidiSink,
        starting: bool,
        pending: Option<(usize, usize)>,
    }
    static EPOCH: AtomicU64 = AtomicU64::new(0);

    // SAFETY: `apple/fandhe_apple_midi.m` が定義する C ABI（`build.rs` の iOS / macOS 分岐が
    // コンパイル・CoreMIDI をリンクする）。契約は `.m` ファイル冒頭コメントに集約する。
    #[allow(unsafe_code)]
    unsafe extern "C" {
        /// 戻り値: 0 = 成功（`out_sources` に接続数、`out_failed` に接続失敗数）、1 = client / port 作成失敗、
        /// 2 = ソースがあるのに全接続失敗。
        fn fandhe_apple_midi_start(epoch: u64, out_sources: *mut u32, out_failed: *mut u32) -> i32;
        fn fandhe_apple_midi_stop();
    }

    /// `.m` の戻りコードを契約へ写す純関数。0 以外は OS の生値を見ずに `Internal` へ畳む。
    pub(super) fn result_from_code(
        code: i32,
        sources: u32,
        failed: u32,
    ) -> Result<(usize, usize), DeviceError> {
        match code {
            0 => Ok((sources as usize, failed as usize)),
            _ => Err(DeviceError::Internal("midi start")),
        }
    }

    fn lock_sink() -> std::sync::MutexGuard<'static, Option<Sub>> {
        SINK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 世代が現在の購読と一致するときだけ sink を返す（古い世代のコールバックは捨てる）。
    fn current_sink(epoch: u64) -> Option<MidiSink> {
        lock_sink()
            .as_ref()
            .filter(|s| s.epoch == epoch)
            .map(|s| s.sink.clone())
    }

    #[allow(unsafe_code)]
    pub(super) fn start(sink: MidiSink) -> Result<(), DeviceError> {
        let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
        *lock_sink() = Some(Sub {
            epoch,
            sink: sink.clone(),
            starting: true,
            pending: None,
        });
        let mut sources: u32 = 0;
        let mut failed: u32 = 0;
        // SAFETY: `out_sources` / `out_failed` はこの呼び出し中のみ有効なスタック上の `u32` で、
        // `.m` 側は戻り値 0 のときだけ 1 回ずつ書く。所有権移転・解放は無い。
        let code = unsafe { fandhe_apple_midi_start(epoch, &mut sources, &mut failed) };
        match result_from_code(code, sources, failed) {
            Ok(n) => {
                sink(MidiEvent::Started {
                    sources: n.0,
                    failed: n.1,
                });
                // Started より前に届いた接続数変更を、送信後に最新値で反映する。
                let mut last = n;
                loop {
                    let next = {
                        let mut g = lock_sink();
                        match g.as_mut().filter(|s| s.epoch == epoch) {
                            Some(s) => match s.pending.take() {
                                Some(p) => p,
                                None => {
                                    s.starting = false;
                                    break;
                                }
                            },
                            None => break,
                        }
                    };
                    if next != last {
                        sink(MidiEvent::Sources {
                            sources: next.0,
                            failed: next.1,
                        });
                        last = next;
                    }
                }
                Ok(())
            }
            Err(e) => {
                *lock_sink() = None;
                stop();
                Err(e)
            }
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn stop() {
        // SAFETY: 引数なし。`.m` 側は 1 本のミューテックスの内側で全ソースを切断する冪等な関数。
        unsafe { fandhe_apple_midi_stop() };
        *lock_sink() = None;
    }

    /// `.m` の受信ブロックから、コールバック 1 回につき 1 回呼ばれる。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_midi_ump(words: *const u32, count: u32, epoch: u64) {
        if words.is_null() || count == 0 || count > MAX_WORDS {
            return;
        }
        // SAFETY: `.m` は `count` 個の `u32` を持つ有効なバッファを渡し、この呼び出しの間だけ
        // 保持する（呼び出し後に解放）。null と長さは上で検証済み。ここでコピーせず読むだけ。
        let slice = unsafe { std::slice::from_raw_parts(words, count as usize) };
        let notes = notes_from_ump_words(slice);
        if notes.is_empty() {
            return;
        }
        let Some(sink) = current_sink(epoch) else {
            return;
        };
        // ロックの外で呼ぶ。受け手なしなら（差し替わっていない限り）SINK を外すだけ。
        let alive = sink(MidiEvent::Notes(notes));
        #[cfg(target_os = "ios")]
        crate::ios::request_repaint();
        if !alive {
            let mut g = lock_sink();
            if g.as_ref().is_some_and(|cur| Arc::ptr_eq(&cur.sink, &sink)) {
                *g = None;
            }
        }
    }

    /// `.m` の SetupChanged 通知から呼ばれる。開始中は `start` が `Started` の後に反映する。
    #[allow(unsafe_code)]
    #[no_mangle]
    extern "C" fn fandhe_rust_midi_sources(count: u32, failed: u32, epoch: u64) {
        let sink = {
            let mut g = lock_sink();
            let Some(s) = g.as_mut().filter(|s| s.epoch == epoch) else {
                return;
            };
            if s.starting {
                s.pending = Some((count as usize, failed as usize));
                return;
            }
            s.sink.clone()
        };
        sink(MidiEvent::Sources {
            sources: count as usize,
            failed: failed as usize,
        });
        #[cfg(target_os = "ios")]
        crate::ios::request_repaint();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// SINK はプロセス共有のため、触るテストを直列化する。
        static TEST_LOCK: Mutex<()> = Mutex::new(());

        #[test]
        fn ump_with_null_zero_or_huge_length_does_not_panic() {
            let _t = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            fandhe_rust_midi_ump(std::ptr::null(), 4, 1);
            let w = [0x2090_3C64u32];
            fandhe_rust_midi_ump(w.as_ptr(), 0, 1);
            fandhe_rust_midi_ump(w.as_ptr(), u32::MAX, 1);
            // SINK 未設定でも valid な入力で panic しない
            fandhe_rust_midi_ump(w.as_ptr(), 1, 1);
            fandhe_rust_midi_sources(3, 0, 1);
        }

        #[test]
        fn stale_epoch_callbacks_are_dropped() {
            let _t = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let seen = Arc::new(AtomicU64::new(0));
            let s2 = seen.clone();
            let sink: MidiSink = Arc::new(move |_| {
                s2.fetch_add(1, Ordering::SeqCst);
                true
            });
            *lock_sink() = Some(Sub {
                epoch: 7,
                sink,
                starting: false,
                pending: None,
            });
            fandhe_rust_midi_sources(1, 0, 6);
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            fandhe_rust_midi_sources(1, 0, 7);
            assert_eq!(seen.load(Ordering::SeqCst), 1);
            *lock_sink() = None;
        }

        #[test]
        fn sources_during_start_are_held_until_started() {
            let _t = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let seen = Arc::new(AtomicU64::new(0));
            let s2 = seen.clone();
            let sink: MidiSink = Arc::new(move |_| {
                s2.fetch_add(1, Ordering::SeqCst);
                true
            });
            *lock_sink() = Some(Sub {
                epoch: 9,
                sink,
                starting: true,
                pending: None,
            });
            fandhe_rust_midi_sources(2, 1, 9);
            fandhe_rust_midi_sources(0, 1, 9);
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            assert_eq!(lock_sink().as_ref().unwrap().pending, Some((0, 1)));
            *lock_sink() = None;
        }

        #[test]
        fn nonzero_codes_are_internal_errors() {
            assert_eq!(result_from_code(0, 3, 1).unwrap(), (3, 1));
            for c in [1, 2, -1, 99] {
                assert!(matches!(
                    result_from_code(c, 5, 0),
                    Err(DeviceError::Internal("midi start"))
                ));
            }
        }
    }
}
