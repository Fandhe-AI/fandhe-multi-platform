//! Phase 11 の拡充ループ（親 #152、Issue #1116）型サンプル「車載ディスプレイ」の effect ドライバ。
//! `fandhe_core::screens::car_display_show`（端末依存ゼロ）の `pending` 要求を観測して
//! `fandhe_device::car_display::CarDisplay::set_text` を呼び、車載画面が出ているかを
//! [`POLL_INTERVAL`] ごとに `state` で読んで `CompletionSender` 経由で返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる）。
//!
//! 実装の選択: Android は [`super::android_car_display`]（現状は骨格のスタブ、#1117 が埋める）、
//! 他 OS は `UnsupportedCarDisplay` 固定（iOS の CarPlay は承認待ちで dispatch に入れない）。
//!
//! # 並行性
//!
//! - set: 画面側が処理中のボタンを外すため高々 1 件。起動済みマークで二重起動を防ぐ
//! - state: [`Poller`] が高々 1 件に絞る（判定と状態更新は同じロックの内側）。期限は単調時計
//!   （`Instant`）で測り、期限前の分岐でも次回の再描画を予約する。完了は先に次回の期限と空きを
//!   確定してから画面へ送り、古い世代の完了は画面側が捨てる
//! - `Unsupported` は結果が変わらないため 1 回だけ送り、以後は読みも再描画の予約もしない。
//!   それ以外の失敗は `NotShowing` に畳まず `Failed` として次回も読む

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::screens::car_display_show::{
    apply_unsupported, finish_set, finish_state, mark_dispatched, take_request, CarDisplayRequest,
    CarDisplayShowState, StateOutcome, CANDIDATES, UNAVAILABLE_MARKER,
};
#[cfg(not(target_os = "android"))]
use fandhe_device::car_display::UnsupportedCarDisplay;
use fandhe_device::car_display::{CarDisplay, CarDisplayState};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 車載画面の表示状態を読み直す間隔。表示の遅れはこの間隔ぶん許容する（trait doc の守らない範囲）。
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

/// 現在の OS の実装（Android は `android_car_display`、他は `Unsupported` 固定）。
fn implementation() -> &'static Arc<dyn CarDisplay> {
    static INSTANCE: OnceLock<Arc<dyn CarDisplay>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        #[cfg(target_os = "android")]
        {
            super::android_car_display::implementation()
        }
        #[cfg(not(target_os = "android"))]
        {
            Arc::new(UnsupportedCarDisplay)
        }
    })
}

/// `Unsupported` は画面側が判定する固定文言へ、それ以外は `Display` のみ転送する
/// （入力値・内部構造を漏らさない、security.md A09）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE_MARKER.to_string(),
        other => other.to_string(),
    }
}

/// 未処理の `Set` 要求を 1 件、別スレッドで処理して完了を送る。
fn drive_set(
    state: &CarDisplayShowState,
    sender: &CompletionSender<CarDisplayShowState>,
    display: &Arc<dyn CarDisplay>,
) {
    let Some(CarDisplayRequest::Set(index)) = take_request(state) else {
        return;
    };
    // 起動済みマーク（effect は毎フレーム呼ばれるため二重起動を防ぐ）。
    let _ = sender.send(Box::new(mark_dispatched));
    let finish = {
        let sender = sender.clone();
        move |result: Result<(), String>| {
            let _ = sender.send(Box::new(move |s: &mut CarDisplayShowState| {
                finish_set(s, result);
            }));
        }
    };
    let Some(text) = CANDIDATES.get(index) else {
        finish(Err(to_message(DeviceError::InvalidInput(
            "car_display index",
        ))));
        return;
    };
    let display = Arc::clone(display);
    let worker_finish = finish.clone();
    let spawned = std::thread::Builder::new()
        .name("fandhe-car-display-set".into())
        .spawn(move || worker_finish(display.set_text(text).map_err(to_message)));
    if spawned.is_err() {
        // 起動できなければ失敗として確定する（成功を装わない）。
        finish(Err(to_message(DeviceError::Internal("car_display"))));
    }
}

/// 期限が来ていて読み出し中でなければ、状態の読み出しを 1 件起動する。
fn drive_state(
    state: &CarDisplayShowState,
    sender: &CompletionSender<CarDisplayShowState>,
    display: &Arc<dyn CarDisplay>,
    poller: &'static Poller,
    now: Instant,
) {
    // 非対応は確定済み。読み出しも再描画の予約もしない。
    if state.is_unsupported() {
        return;
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
    let display = Arc::clone(display);
    let tx = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("fandhe-car-display-state".into())
        .spawn({
            let tx = tx.clone();
            move || finish_read(poller, &tx, generation, display.state())
        });
    if spawned.is_err() {
        finish_read(
            poller,
            &tx,
            generation,
            Err(DeviceError::Internal("car_display")),
        );
    }
}

/// 読み出しの完了。先に次回の期限と空きを確定してから画面へ送る。
fn finish_read(
    poller: &Poller,
    sender: &CompletionSender<CarDisplayShowState>,
    generation: u64,
    result: Result<CarDisplayState, DeviceError>,
) {
    {
        let mut inner = match poller.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_due = Some(Instant::now() + POLL_INTERVAL);
        inner.in_flight = false;
    }
    let outcome = match result {
        Ok(CarDisplayState::Showing) => StateOutcome::Showing,
        Ok(CarDisplayState::NotShowing) => StateOutcome::NotShowing,
        // 非対応は結果が変わらないため 1 回だけ送る。再描画は予約しない。
        Err(DeviceError::Unsupported(_)) => {
            let _ = sender.send(Box::new(apply_unsupported));
            return;
        }
        // 理由は画面へ渡さない。次回も読み、復帰したら表示へ戻る。
        Err(_) => StateOutcome::Failed,
    };
    let _ = sender.send(Box::new(move |s: &mut CarDisplayShowState| {
        finish_state(s, generation, outcome);
    }));
    sender.request_repaint_after(POLL_INTERVAL);
}

/// `registry.rs` の `effect` 区画から `car_display_show` 画面用に配線される effect ドライバ。
pub fn car_display_show_effect(
    state: &CarDisplayShowState,
    sender: &CompletionSender<CarDisplayShowState>,
) {
    static POLLER: Poller = Poller::new();
    let display = implementation();
    drive_set(state, sender, display);
    drive_state(state, sender, display, &POLLER, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Completion;
    use fandhe_core::screens::car_display_show::{
        current_screen, display_state_text, shown_text, status_text, ACCEPTED_STATUS,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};

    type St = CarDisplayShowState;

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

    fn leak_poller() -> &'static Poller {
        Box::leak(Box::new(Poller::new()))
    }

    /// 受け付けた回数を数え、`state` は台本どおりに返す実装。
    struct Scripted {
        sets: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
        states: Mutex<Vec<Result<CarDisplayState, DeviceError>>>,
    }
    impl CarDisplay for Scripted {
        fn set_text(&self, _: &str) -> Result<(), DeviceError> {
            self.sets.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn state(&self) -> Result<CarDisplayState, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let mut v = self.states.lock().unwrap();
            if v.len() > 1 {
                v.remove(0)
            } else {
                clone_result(&v[0])
            }
        }
    }

    fn clone_result(
        r: &Result<CarDisplayState, DeviceError>,
    ) -> Result<CarDisplayState, DeviceError> {
        match r {
            Ok(s) => Ok(*s),
            Err(_) => Err(DeviceError::Internal("car_display")),
        }
    }

    fn scripted(
        states: Vec<Result<CarDisplayState, DeviceError>>,
    ) -> (Arc<dyn CarDisplay>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let sets = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Scripted {
                sets: sets.clone(),
                reads: reads.clone(),
                states: Mutex::new(states),
            }),
            sets,
            reads,
        )
    }

    #[test]
    fn set_round_trip_shows_accepted() {
        let (display, sets, _) = scripted(vec![Ok(CarDisplayState::Showing)]);
        let (tx, rx) = sender_with_channel();
        let mut state = St::default();
        let screen = current_screen(&state);
        assert!(fandhe_core::click_button(
            &screen,
            CANDIDATES[2],
            &mut state
        ));
        drive_set(&state, &tx, &display);
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(sets.load(Ordering::SeqCst), 1);
        assert_eq!(shown_text(&state), "車載画面に渡した文字: 休憩中");
        assert_eq!(status_text(&state), ACCEPTED_STATUS);
    }

    #[test]
    fn unsupported_set_is_reported_and_not_success() {
        let display: Arc<dyn CarDisplay> = Arc::new(UnsupportedCarDisplay);
        let (tx, rx) = sender_with_channel();
        let mut state = St::default();
        let screen = current_screen(&state);
        assert!(fandhe_core::click_button(
            &screen,
            CANDIDATES[0],
            &mut state
        ));
        drive_set(&state, &tx, &display);
        assert_eq!(drain_wait(&rx, &mut state, 2), 2);
        assert_eq!(status_text(&state), UNAVAILABLE_MARKER);
        assert_eq!(shown_text(&state), "車載画面に渡した文字: 未設定");
    }

    #[test]
    fn set_in_flight_is_not_respawned() {
        let (display, sets, _) = scripted(vec![Ok(CarDisplayState::Showing)]);
        let (tx, rx) = sender_with_channel();
        let mut state = St::default();
        let screen = current_screen(&state);
        assert!(fandhe_core::click_button(
            &screen,
            CANDIDATES[0],
            &mut state
        ));
        drive_set(&state, &tx, &display);
        // 起動済みマークだけ適用し、完了前に再度呼ぶ。
        let mark = rx.recv_timeout(Duration::from_secs(5)).expect("mark");
        mark(&mut state);
        drive_set(&state, &tx, &display);
        drain_wait(&rx, &mut state, 1);
        assert_eq!(drain_now(&rx, &mut state), 0);
        assert_eq!(sets.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_state_is_sent_once_then_silent() {
        let display: Arc<dyn CarDisplay> = Arc::new(UnsupportedCarDisplay);
        let (tx, rx) = sender_with_channel();
        let poller = leak_poller();
        let mut state = St::default();
        drive_state(&state, &tx, &display, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert!(state.is_unsupported());
        drive_state(
            &state,
            &tx,
            &display,
            poller,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(drain_now(&rx, &mut state), 0);
    }

    #[test]
    fn internal_error_is_not_not_showing_and_recovers() {
        let (display, _, _) = scripted(vec![
            Err(DeviceError::Internal("car_display")),
            Ok(CarDisplayState::Showing),
        ]);
        let (tx, rx) = sender_with_channel();
        let poller = leak_poller();
        let mut state = St::default();
        drive_state(&state, &tx, &display, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(display_state_text(&state), "車載画面: 状態を読み出せません");
        drive_state(
            &state,
            &tx,
            &display,
            poller,
            Instant::now() + POLL_INTERVAL * 2,
        );
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(display_state_text(&state), "車載画面: 表示中");
    }

    #[test]
    fn not_showing_is_shown_as_not_showing() {
        let (display, _, _) = scripted(vec![Ok(CarDisplayState::NotShowing)]);
        let (tx, rx) = sender_with_channel();
        let mut state = St::default();
        drive_state(&state, &tx, &display, leak_poller(), Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(display_state_text(&state), "車載画面: 表示していません");
    }

    #[test]
    fn before_due_does_not_read() {
        let (display, _, reads) = scripted(vec![Ok(CarDisplayState::Showing)]);
        let (tx, rx) = sender_with_channel();
        let poller = leak_poller();
        let mut state = St::default();
        drive_state(&state, &tx, &display, poller, Instant::now());
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        drive_state(&state, &tx, &display, poller, Instant::now());
        assert_eq!(drain_now(&rx, &mut state), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    /// 開始を待たせて、読み出し中の再呼び出しが二重に起動しないことを確かめる。
    struct Blocking {
        reads: Arc<AtomicUsize>,
        gate: Mutex<Receiver<()>>,
    }
    impl CarDisplay for Blocking {
        fn set_text(&self, _: &str) -> Result<(), DeviceError> {
            Ok(())
        }
        fn state(&self) -> Result<CarDisplayState, DeviceError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let _ = self
                .gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            Ok(CarDisplayState::Showing)
        }
    }

    #[test]
    fn in_flight_read_is_not_started_twice() {
        let (open, gate): (Sender<()>, Receiver<()>) = channel();
        let reads = Arc::new(AtomicUsize::new(0));
        let display: Arc<dyn CarDisplay> = Arc::new(Blocking {
            reads: reads.clone(),
            gate: Mutex::new(gate),
        });
        let (tx, rx) = sender_with_channel();
        let poller = leak_poller();
        let mut state = St::default();
        let far = Instant::now() + Duration::from_secs(100);
        for _ in 0..3 {
            drive_state(&state, &tx, &display, poller, far);
        }
        open.send(()).unwrap();
        assert_eq!(drain_wait(&rx, &mut state, 1), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }
}
