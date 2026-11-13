//! Phase 11（親 #147）型サンプル「スマートホーム」（Issue #1079）の effect ドライバ。
//! `fandhe_core::screens::smart_home_list`（端末依存ゼロ）の要求を観測し、
//! `crate::contract::SmartHome` で家と機器の名前を読んで
//! `CompletionSender` 経由で結果を返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//! 契約の SSOT は本 crate の `contract`。
//!
//! # スレッド
//!
//! 許可ダイアログ・期限付きのブロッキング待ちになるため、UI を止めないよう OS を問わず
//! ワーカースレッドで呼ぶ。完了は世代照合つきで画面状態へ入れる（古い世代は捨てられる）。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_smart_home`] の cfg 分岐が最終形で、iOS は `ios_smart_home`（#1080）の
//! `platform_smart_home()` だけを埋める。Android は保留で dispatch に入れない。
//! それ以外は `UnsupportedSmartHome` 固定で、ダミーの成功実装へは倒さない。

use std::sync::Arc;

#[cfg(not(target_os = "ios"))]
use crate::contract::UnsupportedSmartHome;
use crate::contract::{AccessoryList, LIST_TIMEOUT, SmartHome, SmartHomeFailure};
use fandhe_core::screens::smart_home_list::{
    HomeListing, ListFailure, SmartHomeListState, finish_list, mark_loading, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `SmartHome` 実装を返す（プロセスで 1 個）。画面の状態型に依存しないため、
/// フレームワークの外のアプリも使える。
pub fn platform_smart_home() -> Arc<dyn SmartHome> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn SmartHome>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "ios")]
            {
                crate::ios::implementation()
            }
            #[cfg(not(target_os = "ios"))]
            {
                Arc::new(UnsupportedSmartHome)
            }
        })
        .clone()
}

/// `SmartHomeFailure` を画面の失敗種別へ写す。`Failed` は固定文言へ畳み、OS 固有の値を
/// 画面へ漏らさない（security.md A09）。
fn to_failure(e: SmartHomeFailure) -> ListFailure {
    match e {
        SmartHomeFailure::Unsupported => ListFailure::Unsupported,
        SmartHomeFailure::PermissionDenied => ListFailure::PermissionDenied,
        SmartHomeFailure::Timeout => ListFailure::Timeout,
        SmartHomeFailure::Failed => ListFailure::Failed("家と機器の一覧の読み取り".to_string()),
    }
}

/// device 側の一覧を画面の表示用へ写す（`fandhe-core` は `fandhe-device` を参照できないため）。
fn to_listing(list: AccessoryList) -> HomeListing {
    HomeListing {
        home_count: list.home_count,
        rows: list
            .accessories
            .into_iter()
            .map(|a| (a.home, a.name))
            .collect(),
        truncated: list.truncated,
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &SmartHomeListState,
    sender: &CompletionSender<SmartHomeListState>,
    home: Arc<dyn SmartHome>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_loading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("smart-home-list".to_string())
        .spawn(move || {
            let result = home
                .list_accessories(LIST_TIMEOUT)
                .map(to_listing)
                .map_err(to_failure);
            let _ = worker_sender.send(Box::new(move |s: &mut SmartHomeListState| {
                finish_list(s, generation, result);
            }));
        });
    if spawned.is_err() {
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        let _ = sender.send(Box::new(move |s: &mut SmartHomeListState| {
            finish_list(
                s,
                generation,
                Err(ListFailure::Failed("家と機器の一覧の読み取り".to_string())),
            );
        }));
    }
}

/// `registry.rs` の `effect` 区画から `smart_home_list` 画面用に配線される effect ドライバ。
pub fn smart_home_list_effect(
    state: &SmartHomeListState,
    sender: &CompletionSender<SmartHomeListState>,
) {
    drive(state, sender, platform_smart_home());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{UnsupportedSmartHome as Unsupported, accessories_from};
    use fandhe_core::screens::smart_home_list::{
        PERMISSION_DENIED_STATUS, Phase, TIMEOUT_STATUS, UNSUPPORTED_STATUS, fetch,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<SmartHomeListState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<SmartHomeListState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    struct Fake {
        result: Result<AccessoryList, SmartHomeFailure>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Result<AccessoryList, SmartHomeFailure>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl SmartHome for Fake {
        fn list_accessories(&self, _timeout: Duration) -> Result<AccessoryList, SmartHomeFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    /// 要求を立てて `drive` し、届いた完了（mark_loading と結果）をすべて適用した状態を返す。
    fn run(home: Arc<dyn SmartHome>) -> SmartHomeListState {
        let (sender, rx) = sender_with_channel();
        let mut state = SmartHomeListState::default();
        fetch(&mut state);
        drive(&state, &sender, home);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("mark_loading と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(AccessoryList::default()));
        drive(&SmartHomeListState::default(), &sender, fake.clone());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn zero_homes_is_success() {
        let state = run(Fake::new(Ok(AccessoryList::default())));
        assert_eq!(state.listing, Some(HomeListing::default()));
        assert_eq!(state.error, "");
    }

    #[test]
    fn homes_without_accessories_is_success() {
        let state = run(Fake::new(Ok(accessories_from(2, []))));
        let l = state.listing.expect("成功");
        assert_eq!(l.home_count, 2);
        assert!(l.rows.is_empty());
        assert_eq!(state.error, "");
    }

    #[test]
    fn each_failure_is_error_not_success() {
        let cases = [
            (SmartHomeFailure::Unsupported, UNSUPPORTED_STATUS),
            (SmartHomeFailure::PermissionDenied, PERMISSION_DENIED_STATUS),
            (SmartHomeFailure::Timeout, TIMEOUT_STATUS),
        ];
        for (f, expected) in cases {
            let state = run(Fake::new(Err(f)));
            assert_eq!(state.error, expected);
            assert_eq!(state.listing, None);
        }
        let state = run(Fake::new(Err(SmartHomeFailure::Failed)));
        assert!(
            state.error.starts_with("読めませんでした: "),
            "{}",
            state.error
        );
        assert_eq!(state.listing, None);
    }

    #[test]
    fn second_drive_while_loading_does_not_call_os_again() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(AccessoryList::default()));
        let mut state = SmartHomeListState::default();
        fetch(&mut state);
        drive(&state, &sender, fake.clone());
        // mark_loading を適用した状態では要求が観測されない。
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        drive(&state, &sender, fake.clone());
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_completion_does_not_overwrite_newer_request() {
        let (sender, rx) = sender_with_channel();
        let fake = Fake::new(Ok(AccessoryList::default()));
        let mut state = SmartHomeListState::default();
        fetch(&mut state);
        drive(&state, &sender, fake);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("mark_loading")(&mut state);
        // 完了が届く前に世代が進んだ状況を再現する。
        state.phase = Phase::Idle;
        fetch(&mut state);
        rx.recv_timeout(Duration::from_secs(5)).expect("finish")(&mut state);
        assert_eq!(state.generation, 2);
        assert_eq!(state.listing, None);
        assert_eq!(state.phase, Phase::Requested);
    }

    #[test]
    fn unsupported_home_never_yields_list() {
        let state = run(Arc::new(Unsupported));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.listing, None);
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert_eq!(
            platform_smart_home()
                .list_accessories(Duration::from_millis(1))
                .map(|_| ()),
            Err(SmartHomeFailure::Unsupported)
        );
    }
}
