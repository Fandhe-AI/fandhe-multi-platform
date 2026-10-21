//! Phase 11（親 #117）型サンプル「OS の検索への登録」（Issue #560）の effect ドライバ。
//! `fandhe_core::screens::spotlight_index`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::spotlight_index::SpotlightIndex` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::spotlight_index`。あわせて `url_scheme` の受け口を読み、OS の検索から開かれた
//! 項目の URL（`fandhe://spotlight/<id>`）を画面へ渡す。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch は #560 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: `apple_spotlight_index`（#560）
//! - iOS: `ios_spotlight_index`（#561）
//! - Android: `android_spotlight_index`（#562）
//! - Windows・Linux: `UnsupportedSpotlightIndex` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中で結果が未着のとき `elapsed >= deadline` なら `Timeout` を確定してスロットを空にし、
//! まだなら `poll_and_finish` の再描画予約で次の判定に回る。確定後にワーカーが返す結果は
//! 画面側の世代・状態照合（`finish`）で捨てられる。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::spotlight_index::{
    apply_opened, finish, mark_running, take_request, Action, IndexFailure, Phase,
    SpotlightIndexState, ITEM_ID, ITEM_TEXT, ITEM_TITLE,
};
use fandhe_device::spotlight_index::{
    validate_item, IndexItem, SpotlightIndex, SPOTLIGHT_DEADLINE, TIMEOUT,
};
use fandhe_device::url_scheme::UrlScheme;
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `SpotlightIndex` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn SpotlightIndex> {
    #[cfg(target_os = "macos")]
    {
        super::apple_spotlight_index::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_spotlight_index::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_spotlight_index::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Arc::new(fandhe_device::spotlight_index::UnsupportedSpotlightIndex)
    }
}

/// ドライバの可変状態。要求の追跡は [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。
/// テストが並列で干渉しないよう static から切り出して引数で渡す（スロットは leak）。
struct Session {
    slot: &'static RequestSlot<Result<(), IndexFailure>>,
    /// 進行中の要求を観測した時刻（単調時計）。
    started: Mutex<Option<Instant>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            started: Mutex::new(None),
        }
    }
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。画面状態は
/// `default()`（世代 0）に戻るため、保留が残ると次の「登録する」が飲み込まれる。古いワーカーの
/// 結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。OS の文言・内部識別子は画面にもログにも出さない。
pub(crate) fn to_failure(e: DeviceError) -> IndexFailure {
    match e {
        DeviceError::Unsupported(_) => IndexFailure::Unsupported,
        DeviceError::Internal(TIMEOUT) => IndexFailure::Timeout,
        _ => IndexFailure::Failed,
    }
}

/// ワーカーの本体: 入力検査 → OS 呼び出し → 期限の照合。
fn run_worker(
    imp: &dyn SpotlightIndex,
    action: Action,
    started: Instant,
    deadline: Duration,
) -> Result<(), IndexFailure> {
    let remaining = || deadline.saturating_sub(started.elapsed());
    let result = match action {
        Action::Index => {
            let item = IndexItem {
                id: ITEM_ID,
                title: ITEM_TITLE,
                text: ITEM_TEXT,
            };
            validate_item(&item).and_then(|()| imp.index(&item, remaining()))
        }
        Action::RemoveAll => imp.remove_all(remaining()),
    };
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(IndexFailure::Timeout);
    }
    result.map_err(to_failure)
}

/// 1 フレームぶんの処理。開いた項目の反映・要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &SpotlightIndexState,
    sender: &CompletionSender<SpotlightIndexState>,
    imp: &Arc<dyn SpotlightIndex>,
    url: &dyn UrlScheme,
    session: &Session,
    deadline: Duration,
) {
    // 受信側: seq が変わったときだけ画面へ送る。受け口が読めるなら次回のポーリングも予約する
    // （pull 型。契約表 4）。非対応（Unsupported）では予約しない。
    if let Ok(snap) = url.snapshot() {
        if state.inbox_seq != Some(snap.seq) {
            let seq = snap.seq;
            let last = snap.last.map(|r| r.url);
            let _ = sender.send(Box::new(move |s: &mut SpotlightIndexState| {
                apply_opened(s, seq, last);
            }));
        }
        sender.request_repaint_after(POLL_INTERVAL);
    }

    let slot = session.slot;
    if let Some((_, action)) = take_request(state) {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if slot.snapshot().is_some() {
            return;
        }
        let request_id = slot.begin();
        let started = Instant::now();
        *session.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
        let _ = sender.send(Box::new(mark_running));
        sender.request_repaint_after(POLL_INTERVAL);

        let worker_imp = imp.clone();
        let spawned = std::thread::Builder::new()
            .name("spotlight-index".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, action, started, deadline);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            slot.deliver(request_id, Err(IndexFailure::Failed));
        }
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut SpotlightIndexState| {
                finish(s, generation, Err(IndexFailure::Timeout));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut SpotlightIndexState, result| {
            finish(s, generation, result);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `spotlight_index` 画面用に配線される effect ドライバ。
pub fn spotlight_index_effect(
    state: &SpotlightIndexState,
    sender: &CompletionSender<SpotlightIndexState>,
) {
    static IMPL: std::sync::OnceLock<Arc<dyn SpotlightIndex>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        super::url_scheme::platform_url_scheme(),
        SESSION.get_or_init(Session::default),
        SPOTLIGHT_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::spotlight_index::request;
    use fandhe_device::spotlight_index::UnsupportedSpotlightIndex;
    use fandhe_device::url_scheme::{InboxSnapshot, ReceivedUrl};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<SpotlightIndexState>>;

    fn channel() -> (CompletionSender<SpotlightIndexState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。OS 呼び出し回数を数える。
    struct Fake {
        result: fn() -> Result<(), DeviceError>,
        sleep: Duration,
        calls: AtomicUsize,
    }
    impl SpotlightIndex for Fake {
        fn index(&self, _: &IndexItem<'_>, _: Duration) -> Result<(), DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.sleep);
            (self.result)()
        }
        fn remove_all(&self, _: Duration) -> Result<(), DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake_with(result: fn() -> Result<(), DeviceError>, sleep: Duration) -> Arc<Fake> {
        Arc::new(Fake {
            result,
            sleep,
            calls: AtomicUsize::new(0),
        })
    }

    fn fake(result: fn() -> Result<(), DeviceError>) -> Arc<dyn SpotlightIndex> {
        fake_with(result, Duration::ZERO)
    }

    /// 固定の snapshot を返す URL 受け口。
    struct FakeUrl(Option<InboxSnapshot>);
    impl UrlScheme for FakeUrl {
        fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
            self.0.clone().ok_or(DeviceError::Unsupported("url_scheme"))
        }
    }
    fn no_url() -> FakeUrl {
        FakeUrl(None)
    }

    fn pump(
        state: &mut SpotlightIndexState,
        sender: &CompletionSender<SpotlightIndexState>,
        rx: &Rx,
        imp: &Arc<dyn SpotlightIndex>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, &no_url(), session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, Phase::Requested | Phase::Running) || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn run_action(imp: Arc<dyn SpotlightIndex>, action: Action) -> SpotlightIndexState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = SpotlightIndexState::default();
        request(&mut state, action);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_secs(30),
        );
        state
    }

    fn failure(s: &SpotlightIndexState) -> IndexFailure {
        match s.phase {
            Phase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn success_for_both_actions() {
        for a in [Action::Index, Action::RemoveAll] {
            assert_eq!(run_action(fake(|| Ok(())), a).phase, Phase::Done);
        }
    }

    #[test]
    fn errors_never_become_done() {
        type R = fn() -> Result<(), DeviceError>;
        let cases: [(R, IndexFailure); 5] = [
            (
                || Err(DeviceError::Unsupported("spotlight_index")),
                IndexFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Internal(TIMEOUT)),
                IndexFailure::Timeout,
            ),
            (
                || Err(DeviceError::PermissionDenied("spotlight_index")),
                IndexFailure::Failed,
            ),
            (
                || Err(DeviceError::InvalidInput("spotlight_index id")),
                IndexFailure::Failed,
            ),
            (
                || Err(DeviceError::Internal("spotlight_index")),
                IndexFailure::Failed,
            ),
        ];
        for (f, expect) in cases {
            for a in [Action::Index, Action::RemoveAll] {
                assert_eq!(failure(&run_action(fake(f), a)), expect);
            }
        }
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run_action(Arc::new(UnsupportedSpotlightIndex), Action::Index);
        assert_eq!(failure(&s), IndexFailure::Unsupported);
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp: Arc<dyn SpotlightIndex> = fake_with(|| Ok(()), Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = SpotlightIndexState::default();
        request(&mut state, Action::Index);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        assert_eq!(failure(&state), IndexFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp: Arc<dyn SpotlightIndex> = fake_with(|| Ok(()), Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = SpotlightIndexState::default();
        request(&mut state, Action::Index);
        let deadline = Duration::from_millis(30);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failure(&state), IndexFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される（スロットは空・追加の完了なし）
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &no_url(), &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failure(&state), IndexFailure::Timeout);
    }

    #[test]
    fn double_drive_before_mark_applied_calls_os_once() {
        let (sender, rx) = channel();
        let f = fake_with(|| Ok(()), Duration::ZERO);
        let imp: Arc<dyn SpotlightIndex> = f.clone();
        let session = Session::default();
        let mut state = SpotlightIndexState::default();
        request(&mut state, Action::Index);
        drive(
            &state,
            &sender,
            &imp,
            &no_url(),
            &session,
            Duration::from_secs(30),
        );
        drive(
            &state,
            &sender,
            &imp,
            &no_url(),
            &session,
            Duration::from_secs(30),
        );
        // mark_running 1 件のみ（2 回目は発送されない）
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(()));
        let session = Session::default();
        session.slot.begin();
        let mut state = SpotlightIndexState::default();
        request(&mut state, Action::Index);
        drive(
            &state,
            &sender,
            &imp,
            &no_url(),
            &session,
            Duration::from_secs(30),
        );
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.slot.reset();
        drive(
            &state,
            &sender,
            &imp,
            &no_url(),
            &session,
            Duration::from_secs(30),
        );
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn opened_url_is_forwarded_once_per_seq() {
        let (sender, rx) = channel();
        let imp = fake(|| Ok(()));
        let session = Session::default();
        let url = FakeUrl(Some(InboxSnapshot {
            last: Some(ReceivedUrl {
                url: "fandhe://spotlight/demo-memo".into(),
                received_at_unix_ms: 0,
            }),
            rejected: 0,
            seq: 3,
        }));
        let mut state = SpotlightIndexState::default();
        drive(
            &state,
            &sender,
            &imp,
            &url,
            &session,
            Duration::from_secs(1),
        );
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state.opened.as_deref(), Some("demo-memo"));
        // seq が同じなら送らない
        drive(
            &state,
            &sender,
            &imp,
            &url,
            &session,
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        assert!(matches!(
            platform().remove_all(Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
