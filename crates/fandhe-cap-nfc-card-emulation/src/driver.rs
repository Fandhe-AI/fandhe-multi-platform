//! `fandhe_core::screens::nfc_card_emulation_show` の effect ドライバ（nfc-card-emulation 機能 crate の `driver`）。
//!
//! 毎フレーム呼ばれ、期限が来ていれば [`CardEmulation::status`] をワーカーで読み、`CompletionSender` 経由で
//! 画面へ届ける（二重起動防止・世代照合は `Poller`）。呼び出し元は `fandhe-gallery` の `screens/registry.rs` の
//! `effect "nfc_card_emulation_show"` 行。画面の状態型を参照してよいのは本モジュールだけ
//! （契約・OS 実装は `crate` 直下・`android`。`consumer-app-structure.md` §8.1）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::nfc_card_emulation_show::{
    NfcCardEmulationShowState, ReadOutcome, apply_unsupported, finish_read, set_aid,
};
use fandhe_runtime::CompletionSender;

use crate::{AID, CardEmulation, CardEmulationError, HceStatus, platform_nfc_card_emulation};

/// 状態を読み直す間隔。表示の遅れはこの間隔ぶん許容する（crate doc の守らない範囲）。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 読み出しの二重起動防止・世代・次回期限をまとめて持つ。
struct Poller {
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
    const fn new() -> Self {
        Self {
            inner: Mutex::new(PollerInner {
                next_due: None,
                generation: 0,
                in_flight: false,
            }),
        }
    }
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
        if let Some(due) = inner.next_due
            && now < due
        {
            // 送信を省く分岐でも次回を予約する。
            sender.request_repaint_after(due - now);
            return;
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
        finish(poller, &tx, generation, Err(CardEmulationError::Internal));
    }
}

/// 読み出しの完了。先に次回の期限と空きを確定してから画面へ送る。
fn finish(
    poller: &Poller,
    sender: &CompletionSender<NfcCardEmulationShowState>,
    generation: u64,
    result: Result<HceStatus, CardEmulationError>,
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
        Err(CardEmulationError::Unsupported) => {
            let _ = sender.send(Box::new(apply_unsupported));
        }
        // 理由（OS の文言）は画面へ渡さない。次回も読み、復帰したら状態へ戻る。
        Err(CardEmulationError::Internal) => {
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
    drive(
        state,
        sender,
        platform_nfc_card_emulation(),
        &POLLER,
        Instant::now(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnsupportedCardEmulation;
    use fandhe_core::screens::nfc_card_emulation_show::{responses_text, status_text};
    use fandhe_runtime::{Completion, RepaintWaker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};

    type St = NfcCardEmulationShowState;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn sender_with_channel() -> (CompletionSender<St>, Receiver<Completion<St>>) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
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
        results: Mutex<Vec<Result<HceStatus, CardEmulationError>>>,
    }
    impl CardEmulation for Scripted {
        fn status(&self) -> Result<HceStatus, CardEmulationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut r = self.results.lock().unwrap();
            if r.len() > 1 { r.remove(0) } else { r[0] }
        }
    }

    fn scripted(
        results: Vec<Result<HceStatus, CardEmulationError>>,
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

    fn st(n: u64) -> Result<HceStatus, CardEmulationError> {
        Ok(HceStatus {
            nfc_enabled: true,
            default_for_aid: true,
            responses: n,
            since_last_ms: None,
        })
    }

    fn fail() -> Result<HceStatus, CardEmulationError> {
        Err(CardEmulationError::Internal)
    }

    #[test]
    fn unsupported_is_sent_once_then_silent() {
        let (tx, rx) = sender_with_channel();
        let emu: Arc<dyn CardEmulation> = Arc::new(UnsupportedCardEmulation);
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
        fn status(&self) -> Result<HceStatus, CardEmulationError> {
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
