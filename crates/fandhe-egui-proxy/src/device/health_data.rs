//! Phase 11（親 #149）型サンプル「ヘルスケアの歩数」（Issue #1089）の effect ドライバ。
//! `fandhe_core::screens::health_steps_read`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::health_data::HealthData` で今日の歩数を 1 回読んで `CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::health_data`。
//!
//! # スレッド
//!
//! 許可シート待ちを含む期限付きのブロッキング呼び出しのため、UI を止めないようワーカースレッドで
//! 呼ぶ。完了は世代照合つきで画面状態へ入れる（古い世代は捨てられる）。
//!
//! # 再描画
//!
//! 結果はワーカーの戻り値から `CompletionSender::send` で入り、そこで `request_repaint` されるため
//! `crate::ios::request_repaint` は不要。
//!
//! # OS 別実装の差し込み口
//!
//! [`implementation`] の cfg 分岐が最終形。iOS は `ios_health_data`（#1090 が埋める）、
//! それ以外は `UnsupportedHealthData` 固定でダミーの成功実装へは倒さない。Android は保留のため入れない。

use std::sync::Arc;

use fandhe_core::screens::health_steps_read::{
    finish_read, mark_reading, take_request, HealthStepsReadState, ReadFailure, StepsReading,
};
#[cfg(not(target_os = "ios"))]
use fandhe_device::health_data::UnsupportedHealthData;
use fandhe_device::health_data::{HealthData, HealthDataFailure, HealthSteps, READ_TIMEOUT};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `HealthData` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn HealthData> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn HealthData>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                super::ios_health_data::implementation()
            }
            #[cfg(not(target_os = "ios"))]
            {
                Arc::new(UnsupportedHealthData)
            }
        })
        .clone()
}

/// `HealthDataFailure` を画面の失敗種別へ写す。`Failed` は固定文言へ畳み、OS 固有の値を
/// 画面へ漏らさない（security.md A09）。
fn to_failure(e: HealthDataFailure) -> ReadFailure {
    match e {
        HealthDataFailure::Unsupported => ReadFailure::Unsupported,
        HealthDataFailure::Canceled => ReadFailure::Canceled,
        HealthDataFailure::Timeout => ReadFailure::Timeout,
        HealthDataFailure::Failed => ReadFailure::Failed("ヘルスケアの歩数の読み取り".to_string()),
    }
}

/// `HealthSteps` を画面の成功値へ写す。
fn to_reading(s: HealthSteps) -> StepsReading {
    match s {
        HealthSteps::Steps(n) => StepsReading::Steps(n),
        HealthSteps::NoVisibleData => StepsReading::NoVisibleData,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &HealthStepsReadState,
    sender: &CompletionSender<HealthStepsReadState>,
    health: Arc<dyn HealthData>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("health-data-read".to_string())
        .spawn(move || {
            let result = health
                .read_today_steps(READ_TIMEOUT)
                .map(to_reading)
                .map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut HealthStepsReadState| {
                finish_read(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut HealthStepsReadState| {
            finish_read(
                s,
                generation,
                Err(ReadFailure::Failed(
                    "ヘルスケアの歩数の読み取り".to_string(),
                )),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `health_steps_read` 画面用に配線される effect ドライバ。
pub fn health_steps_read_effect(
    state: &HealthStepsReadState,
    sender: &CompletionSender<HealthStepsReadState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::health_steps_read::{
        read, CANCELED_STATUS, NO_VISIBLE_DATA_STATUS, TIMEOUT_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::health_data::UnsupportedHealthData as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<HealthStepsReadState>,
        std::sync::mpsc::Receiver<crate::app::Completion<HealthStepsReadState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<HealthSteps, HealthDataFailure>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<HealthSteps, HealthDataFailure>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl HealthData for Fake {
        fn read_today_steps(&self, _t: Duration) -> Result<HealthSteps, HealthDataFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_reading と結果）をすべて適用した状態を返す。
    fn run(health: Arc<dyn HealthData>) -> HealthStepsReadState {
        let (sender, rx) = sender_with_channel();
        let mut state = HealthStepsReadState::default();
        read(&mut state);
        drive(&state, &sender, health);
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
        let fake = Fake::new(Ok(HealthSteps::Steps(1)));
        drive(&HealthStepsReadState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn zero_steps_is_success() {
        let state = run(Fake::new(Ok(HealthSteps::Steps(0))));
        assert_eq!(state.steps, Some(0));
        assert_eq!(state.error, "");
    }

    #[test]
    fn no_visible_data_is_a_success_form() {
        let state = run(Fake::new(Ok(HealthSteps::NoVisibleData)));
        assert_eq!(state.steps, None);
        assert_eq!(state.status, NO_VISIBLE_DATA_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn canceled_is_neutral() {
        let state = run(Fake::new(Err(HealthDataFailure::Canceled)));
        assert_eq!(state.status, CANCELED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let cases = [
            (HealthDataFailure::Unsupported, UNSUPPORTED_STATUS),
            (HealthDataFailure::Timeout, TIMEOUT_STATUS),
        ];
        for (f, expected) in cases {
            let state = run(Fake::new(Err(f)));
            assert_eq!(state.error, expected);
            assert_eq!(state.steps, None);
            assert_eq!(state.status, "");
        }
        let state = run(Fake::new(Err(HealthDataFailure::Failed)));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.steps, None);
    }

    #[test]
    fn second_drive_while_reading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(HealthSteps::Steps(3)));
        let mut state = HealthStepsReadState::default();
        read(&mut state);
        drive(&state, &sender, fake.clone());
        let c = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mark_reading");
        c(&mut state);
        drive(&state, &sender, fake.clone());
        let c = rx.recv_timeout(Duration::from_secs(5)).expect("finish");
        c(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_never_yields_steps() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.steps, None);
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert_eq!(
            implementation().read_today_steps(Duration::from_millis(1)),
            Err(HealthDataFailure::Unsupported)
        );
    }
}
