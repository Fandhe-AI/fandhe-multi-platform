//! Phase 11（親 #1119）型サンプル「歩数」（Issue #1247）の effect ドライバ。
//! `fandhe_core::screens::pedometer_read`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::pedometer::Pedometer` で累積歩数を 1 回取得して `CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::pedometer`。
//!
//! # スレッド
//!
//! どの OS の実装も期限付きのブロッキング待ちになるため、UI を止めないよう OS を問わず
//! ワーカースレッドで呼ぶ。完了は世代照合つきで画面状態へ入れる（古い世代は捨てられる）。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形で、各 OS は `implementation()` を持つモジュールだけを
//! 埋める（Windows: `windows_pedometer`、iOS: `ios_pedometer`（#1248）、Android:
//! `android_pedometer`（#1249））。それ以外（macOS / Linux）は `UnsupportedPedometer` 固定で、
//! ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::pedometer_read::{
    finish_read, mark_reading, take_request, PedometerReadState, ReadFailure,
};
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
use fandhe_device::pedometer::UnsupportedPedometer;
use fandhe_device::pedometer::{Pedometer, PedometerFailure, READ_TIMEOUT};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `Pedometer` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn Pedometer> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn Pedometer>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_pedometer::implementation()
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_pedometer::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_pedometer::implementation()
            }
            #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
            {
                Arc::new(UnsupportedPedometer)
            }
        })
        .clone()
}

/// `PedometerFailure` を画面の失敗種別へ写す。`Failed` は固定文言へ畳み、OS 固有の値
/// （HRESULT 等）を画面へ漏らさない（security.md A09）。
fn to_failure(e: PedometerFailure) -> ReadFailure {
    match e {
        PedometerFailure::Unsupported => ReadFailure::Unsupported,
        PedometerFailure::PermissionDenied => ReadFailure::PermissionDenied,
        PedometerFailure::NoReading => ReadFailure::NoReading,
        PedometerFailure::Failed => ReadFailure::Failed("歩数の取得".to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &PedometerReadState,
    sender: &CompletionSender<PedometerReadState>,
    pedometer: Arc<dyn Pedometer>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("pedometer-read".to_string())
        .spawn(move || {
            let result = pedometer.read_steps(READ_TIMEOUT).map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut PedometerReadState| {
                finish_read(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut PedometerReadState| {
            finish_read(
                s,
                generation,
                Err(ReadFailure::Failed("歩数の取得".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `pedometer_read` 画面用に配線される effect ドライバ。
pub fn pedometer_read_effect(
    state: &PedometerReadState,
    sender: &CompletionSender<PedometerReadState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::pedometer_read::{
        read, NO_READING_STATUS, PERMISSION_DENIED_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::pedometer::UnsupportedPedometer as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<PedometerReadState>,
        std::sync::mpsc::Receiver<crate::app::Completion<PedometerReadState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<u64, PedometerFailure>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<u64, PedometerFailure>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl Pedometer for Fake {
        fn read_steps(&self, _timeout: Duration) -> Result<u64, PedometerFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_reading と結果）をすべて適用した状態を返す。
    fn run(pedometer: Arc<dyn Pedometer>) -> PedometerReadState {
        let (sender, rx) = sender_with_channel();
        let mut state = PedometerReadState::default();
        read(&mut state);
        drive(&state, &sender, pedometer);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_reading と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(1));
        drive(&PedometerReadState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn zero_steps_is_shown_as_success() {
        let state = run(Fake::new(Ok(0)));
        assert_eq!(state.steps, Some(0));
        assert_eq!(state.status, "0 歩");
        assert_eq!(state.error, "");
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let cases = [
            (PedometerFailure::Unsupported, UNSUPPORTED_STATUS),
            (PedometerFailure::PermissionDenied, PERMISSION_DENIED_STATUS),
            (PedometerFailure::NoReading, NO_READING_STATUS),
        ];
        for (f, expected) in cases {
            let state = run(Fake::new(Err(f)));
            assert_eq!(state.error, expected);
            assert_eq!(state.steps, None);
            assert_eq!(state.status, "");
        }
        let state = run(Fake::new(Err(PedometerFailure::Failed)));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.steps, None);
    }

    #[test]
    fn second_drive_while_reading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(3));
        let mut state = PedometerReadState::default();
        read(&mut state);
        drive(&state, &sender, fake.clone());
        // mark_reading を適用した状態では要求が観測されない。
        let c = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mark_reading");
        c(&mut state);
        drive(&state, &sender, fake.clone());
        // 完了（ワーカー分）だけが届く。
        let c = rx.recv_timeout(Duration::from_secs(5)).expect("finish");
        c(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_pedometer_never_yields_steps() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.steps, None);
    }

    #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert_eq!(
            implementation().read_steps(Duration::from_millis(1)),
            Err(PedometerFailure::Unsupported)
        );
    }
}
