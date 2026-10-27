//! Phase 11（親 #187）型サンプル「人の在席・離席の検知」（Issue #960）の effect ドライバ。
//! `fandhe_core::screens::human_presence_read`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::human_presence::HumanPresence` で在席を 1 回読んで `CompletionSender` 経由で
//! 結果を返す（`crate::screens::registry` の `effect` 区画から呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::human_presence`。
//!
//! # スレッド
//!
//! 期限付きのブロッキング待ちになるため、UI を止めないようワーカースレッドで呼ぶ。
//! 完了は世代照合つきで画面状態へ入れる（古い世代は捨てられる）。
//!
//! # OS 別実装
//!
//! 対象は Windows のみ（`windows_human_presence`）。それ以外は `UnsupportedHumanPresence` 固定で、
//! ダミーの成功実装へは倒さない。

use std::sync::Arc;

use fandhe_core::screens::human_presence_read::{
    finish_read, mark_reading, take_request, HumanPresenceReadState, PresenceView, ReadFailure,
};
#[cfg(not(target_os = "windows"))]
use fandhe_device::human_presence::UnsupportedHumanPresence;
use fandhe_device::human_presence::{HumanPresence, HumanPresenceFailure, Presence, READ_TIMEOUT};

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `HumanPresence` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn HumanPresence> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn HumanPresence>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                super::windows_human_presence::implementation()
            }
            #[cfg(not(target_os = "windows"))]
            {
                Arc::new(UnsupportedHumanPresence)
            }
        })
        .clone()
}

/// `Presence` を画面の表示種別へ写す。
fn to_view(p: Presence) -> PresenceView {
    match p {
        Presence::Present => PresenceView::Present,
        Presence::NotPresent => PresenceView::NotPresent,
        Presence::Unknown => PresenceView::Unknown,
    }
}

/// `HumanPresenceFailure` を画面の失敗種別へ写す。`Failed` は固定文言へ畳み、OS 固有の値
/// （HRESULT 等）を画面へ漏らさない（security.md A09）。
fn to_failure(e: HumanPresenceFailure) -> ReadFailure {
    match e {
        HumanPresenceFailure::Unsupported => ReadFailure::Unsupported,
        HumanPresenceFailure::PermissionDenied => ReadFailure::PermissionDenied,
        HumanPresenceFailure::NoReading => ReadFailure::NoReading,
        HumanPresenceFailure::Failed => ReadFailure::Failed("在席の読み取り".to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &HumanPresenceReadState,
    sender: &CompletionSender<HumanPresenceReadState>,
    sensor: Arc<dyn HumanPresence>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("human-presence-read".to_string())
        .spawn(move || {
            let result = sensor
                .read_presence(READ_TIMEOUT)
                .map(to_view)
                .map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut HumanPresenceReadState| {
                finish_read(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut HumanPresenceReadState| {
            finish_read(
                s,
                generation,
                Err(ReadFailure::Failed("在席の読み取り".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `human_presence_read` 画面用に配線される effect ドライバ。
pub fn human_presence_read_effect(
    state: &HumanPresenceReadState,
    sender: &CompletionSender<HumanPresenceReadState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::human_presence_read::{
        read, NOT_PRESENT_STATUS, NO_READING_STATUS, PERMISSION_DENIED_STATUS, PRESENT_STATUS,
        UNKNOWN_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::human_presence::UnsupportedHumanPresence as Unsupported;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sender_with_channel() -> (
        CompletionSender<HumanPresenceReadState>,
        std::sync::mpsc::Receiver<crate::app::Completion<HumanPresenceReadState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake {
        result: Result<Presence, HumanPresenceFailure>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<Presence, HumanPresenceFailure>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl HumanPresence for Fake {
        fn read_presence(&self, _timeout: Duration) -> Result<Presence, HumanPresenceFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_reading と結果）をすべて適用した状態を返す。
    fn run(sensor: Arc<dyn HumanPresence>) -> HumanPresenceReadState {
        let (sender, rx) = sender_with_channel();
        let mut state = HumanPresenceReadState::default();
        read(&mut state);
        drive(&state, &sender, sensor);
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
        let fake = Fake::new(Ok(Presence::Present));
        drive(&HumanPresenceReadState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn each_presence_is_shown_as_success() {
        let cases = [
            (Presence::Present, PRESENT_STATUS),
            (Presence::NotPresent, NOT_PRESENT_STATUS),
            (Presence::Unknown, UNKNOWN_STATUS),
        ];
        for (p, expected) in cases {
            let state = run(Fake::new(Ok(p)));
            assert_eq!(state.status, expected);
            assert_eq!(state.error, "");
            assert_eq!(state.presence, Some(to_view(p)));
        }
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let cases = [
            (HumanPresenceFailure::Unsupported, UNSUPPORTED_STATUS),
            (
                HumanPresenceFailure::PermissionDenied,
                PERMISSION_DENIED_STATUS,
            ),
            (HumanPresenceFailure::NoReading, NO_READING_STATUS),
        ];
        for (f, expected) in cases {
            let state = run(Fake::new(Err(f)));
            assert_eq!(state.error, expected);
            assert_eq!(state.presence, None);
            assert_eq!(state.status, "");
        }
        let state = run(Fake::new(Err(HumanPresenceFailure::Failed)));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.presence, None);
    }

    #[test]
    fn second_drive_while_reading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(Presence::Present));
        let mut state = HumanPresenceReadState::default();
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
    fn unsupported_sensor_never_yields_presence() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.presence, None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert_eq!(
            implementation().read_presence(Duration::from_millis(1)),
            Err(HumanPresenceFailure::Unsupported)
        );
    }
}
