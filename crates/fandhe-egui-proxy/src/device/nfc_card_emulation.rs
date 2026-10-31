//! Phase 11 の拡充ループ（親 #151、Issue #1128）型サンプル「NFC カードエミュレーション」の
//! effect ドライバ。`fandhe_core::screens::nfc_card_emulation_show`（端末依存ゼロ）へ、
//! `fandhe_device::nfc_card_emulation::CardEmulation` が読んだ HCE の状態と AID を注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる（`device.rs` モジュール doc と同じ）。
//!
//! # pull 型のポーリングと別スレッド読み出し
//!
//! 画面へは本ドライバが [`POLL_INTERVAL`] ごとに状態を読んで届ける。読み出しはブロックしうるため
//! ワーカースレッドで行い、[`Poller`] が高々 1 件に絞る（判定と状態更新は同じロックの内側）。期限は
//! 単調時計（`Instant`）で測る。期限前の分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。完了は先に次回の期限と空きを確定して
//! から画面へ送る。失敗の後も読み出しを続け、復帰したら状態を出す。非対応 OS は結果が変わらない
//! ため 1 回だけ送り、以後は読み出しも再描画の予約もしない。
//!
//! # 実装のある OS
//!
//! Android は [`android_nfc_card_emulation`](super::android_nfc_card_emulation)（実体は #1129）。
//! iOS は Apple の entitlement 承認待ちで dispatch に入れない。他 OS は `UnsupportedCardEmulation` 固定。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::nfc_card_emulation_show::{
    apply_unsupported, finish_read, set_aid, NfcCardEmulationShowState, ReadOutcome,
};
#[cfg(not(target_os = "android"))]
use fandhe_device::nfc_card_emulation::UnsupportedCardEmulation;
use fandhe_device::nfc_card_emulation::{CardEmulation, HceStatus, AID};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 状態を読み直す間隔。表示の遅れはこの間隔ぶん許容する（trait doc の守らない範囲）。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 読み出しの二重起動防止・世代・次回期限をまとめて持つ。
pub(super) struct Poller {
    inner: Mutex<PollerInner>,
}

struct PollerInner {
    /// 次に読んでよい時刻。`None` は未開始（すぐ読む）。
    next_due: Option<Instant>,
    /// 発行済みの最新世代。
    generation: u64,
    /// 読み出しスレッドが走っているか。
    in_flight: bool,
}

impl Poller {
    pub(super) const fn new() -> Self {
        Self {
            inner: Mutex::new(PollerInner {
                next_due: None,
                generation: 0,
                in_flight: false,
            }),
        }
    }
}

/// 現在の OS の実装（Android は `android_nfc_card_emulation`、他は `Unsupported` 固定）。
fn platform_emulation() -> &'static Arc<dyn CardEmulation> {
    static EMU: OnceLock<Arc<dyn CardEmulation>> = OnceLock::new();
    EMU.get_or_init(|| {
        #[cfg(target_os = "android")]
        {
            super::android_nfc_card_emulation::implementation()
        }
        #[cfg(not(target_os = "android"))]
        {
            Arc::new(UnsupportedCardEmulation)
        }
    })
}

/// 毎フレーム呼ばれ、期限が来ていて読み出し中でなければ読み出しを 1 件起動する。
fn drive(
    state: &NfcCardEmulationShowState,
    sender: &CompletionSender<NfcCardEmulationShowState>,
    emu: &Arc<dyn CardEmulation>,
    poller: &'static Poller,
    now: Instant,
) {
    // 非対応は確定済み。読み出しも再描画の予約もしない。
    if state.is_unsupported() {
        return;
    }
    // 受信側がドロップ済みの送信失敗は無視してよい（coding-rust.md: panic を API 契約にしない）。
    if state.aid().is_none() {
        let _ = sender.send(Box::new(|s: &mut NfcCardEmulationShowState| {
            set_aid(s, AID);
        }));
    }

    let generation = {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // 読み出し中は何もしない。完了の send が再描画を起こす（二重起動の防止）。
        if inner.in_flight {
            return;
        }
        if let Some(due) = inner.next_due {
            if now < due {
                // 送信を省く分岐でも次回を予約する。
                sender.request_repaint_after(due - now);
                return;
            }
        }
        inner.generation += 1;
        inner.in_flight = true;
        inner.generation
    };

    let emu = Arc::clone(emu);
    let tx = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("fandhe-nfc-card-emulation".into())
        .spawn({
            let tx = tx.clone();
            move || {
                let result = emu.status();
                finish(poller, &tx, generation, result);
            }
        });
    if spawned.is_err() {
        // 起動できなければ失敗として確定する（成功を装わない）。
        finish(
            poller,
            &tx,
            generation,
            Err(DeviceError::Internal("nfc_card_emulation")),
        );
    }
}

/// 読み出しの完了。先に次回の期限と空きを確定してから画面へ送る。
fn finish(
    poller: &Poller,
    sender: &CompletionSender<NfcCardEmulationShowState>,
    generation: u64,
    result: Result<HceStatus, DeviceError>,
) {
    {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_due = Some(Instant::now() + POLL_INTERVAL);
        inner.in_flight = false;
    }
    match result {
        Ok(st) => {
            let outcome = ReadOutcome::Status {
                nfc_enabled: st.nfc_enabled,
                default_for_aid: st.default_for_aid,
                responses: st.responses,
                since_last_ms: st.since_last_ms,
            };
            let _ = sender.send(Box::new(move |s: &mut NfcCardEmulationShowState| {
                finish_read(s, generation, outcome);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
        // 非対応は結果が変わらないため 1 回だけ送る。再描画は予約しない。
        Err(DeviceError::Unsupported(_)) => {
            let _ = sender.send(Box::new(apply_unsupported));
        }
        // 理由（OS の文言）は画面へ渡さない。次回も読み、復帰したら状態へ戻る。
        Err(_) => {
            let _ = sender.send(Box::new(move |s: &mut NfcCardEmulationShowState| {
                finish_read(s, generation, ReadOutcome::Failed);
            }));
            sender.request_repaint_after(POLL_INTERVAL);
        }
    }
}

/// `registry.rs` の `effect` 区画から `nfc_card_emulation_show` 画面用に配線される effect ドライバ。
pub fn nfc_card_emulation_show_effect(
    state: &NfcCardEmulationShowState,
    sender: &CompletionSender<NfcCardEmulationShowState>,
) {
    static POLLER: Poller = Poller::new();
    drive(state, sender, platform_emulation(), &POLLER, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Completion;
    use fandhe_core::screens::nfc_card_emulation_show::{responses_text, status_text};
    use fandhe_device::nfc_card_emulation::UnsupportedCardEmulation as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};

    type St = NfcCardEmulationShowState;

    fn sender_with_channel() -> (CompletionSender<St>, Receiver<Completion<St>>) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn drain_wait(rx: &Receiver<Completion<St>>, state: &mut St, want: usize) -> usize {
        let mut n = 0;
        while n < want {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(c) => {
                    c(state);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        n
    }

    fn drain_now(rx: &Receiver<Completion<St>>, state: &mut St) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    struct Scripted {
        calls: Arc<AtomicUsize>,
        results: Mutex<Vec<Result<HceStatus, DeviceError>>>,
    }
    impl CardEmulation for Scripted {
        fn status(&self) -> Result<HceStatus, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut r = self.results.lock().unwrap();
            if r.len() > 1 {
                r.remove(0)
            } else {
                match &r[0] {
                    Ok(s) => Ok(*s),
                    Err(_) => Err(DeviceError::Internal("nfc_card_emulation")),
                }
            }
        }
    }

    fn scripted(
        results: Vec<Result<HceStatus, DeviceError>>,
    ) -> (Arc<dyn CardEmulation>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Scripted {
                calls: calls.clone(),
                results: Mutex::new(results),
            }),
            calls,
        )
    }

    fn leak_poller() -> &'static Poller {
        Box::leak(Box::new(Poller::new()))
    }

    fn st(n: u64) -> Result<HceStatus, DeviceError> {
        Ok(HceStatus {
            nfc_enabled: true,
            default_for_aid: true,
            responses: n,
            since_last_ms: None,
        })
    }

    fn fail() -> Result<HceStatus, DeviceError> {
        Err(DeviceError::Internal("nfc_card_emulation"))
    }

    #[test]
    fn unsupported_is_sent_once_then_silent() {
        let (tx, rx) = sender_with_channel();
        let emu: Arc<dyn CardEmulation> = Arc::new(Unsupported);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        // AID 注入 + 非対応
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert!(state.is_unsupported());

        drive(
            &state,
            &tx,
            &emu,
            poller,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(drain_now(&rx, &mut state), 0);
    }

    #[test]
    fn reads_status_and_injects_aid() {
        let (tx, rx) = sender_with_channel();
        let (emu, _) = scripted(vec![st(3)]);
        let mut state = St::default();
        drive(&state, &tx, &emu, leak_poller(), Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(responses_text(&state), "応答した回数: 3（このプロセス内）");
        assert_eq!(state.aid(), Some(AID));
    }

    #[test]
    fn failure_is_not_zero_and_recovers() {
        let (tx, rx) = sender_with_channel();
        let (emu, _) = scripted(vec![fail(), st(4)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(status_text(&state), "状態を読み出せません");
        assert_eq!(responses_text(&state), "");

        drive(
            &state,
            &tx,
            &emu,
            poller,
            Instant::now() + POLL_INTERVAL * 2,
        );
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(responses_text(&state), "応答した回数: 4（このプロセス内）");
    }

    #[test]
    fn before_due_does_not_read_or_send() {
        let (tx, rx) = sender_with_channel();
        let (emu, calls) = scripted(vec![st(1)]);
        let poller = leak_poller();
        let mut state = St::default();

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        drive(&state, &tx, &emu, poller, Instant::now());
        assert_eq!(drain_now(&rx, &mut state), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct Blocking {
        calls: Arc<AtomicUsize>,
        gate: Mutex<Receiver<()>>,
    }
    impl CardEmulation for Blocking {
        fn status(&self) -> Result<HceStatus, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self
                .gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            st(9)
        }
    }

    #[test]
    fn in_flight_read_is_not_started_twice() {
        let (tx, rx) = sender_with_channel();
        let (open, gate): (Sender<()>, Receiver<()>) = channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let emu: Arc<dyn CardEmulation> = Arc::new(Blocking {
            calls: calls.clone(),
            gate: Mutex::new(gate),
        });
        let poller = leak_poller();
        let mut state = St::default();

        let far = Instant::now() + Duration::from_secs(100);
        drive(&state, &tx, &emu, poller, far);
        drive(&state, &tx, &emu, poller, far);
        drive(&state, &tx, &emu, poller, far);
        open.send(()).unwrap();
        // AID 注入（state が未注入のまま 3 回呼ばれるため複数通）+ 読み出し結果
        drain_wait(&rx, &mut state, 4);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(responses_text(&state), "応答した回数: 9（このプロセス内）");
    }

    #[test]
    fn stale_generation_does_not_override_newer_failure() {
        let mut state = St::default();
        finish_read(&mut state, 2, ReadOutcome::Failed);
        finish_read(
            &mut state,
            1,
            ReadOutcome::Status {
                nfc_enabled: true,
                default_for_aid: true,
                responses: 5,
                since_last_ms: None,
            },
        );
        assert_eq!(status_text(&state), "状態を読み出せません");
    }
}
