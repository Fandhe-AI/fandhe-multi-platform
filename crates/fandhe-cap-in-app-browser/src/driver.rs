//! Phase 11 型サンプル「アプリ内ブラウザ」（親 #135、Issue #701）の effect ドライバ。
//! `fandhe_core::screens::in_app_browser_show`（端末依存ゼロ）の要求を観測し、
//! `crate::contract::InAppBrowser` に URL の表示を依頼して、OS 層から届く結果を
//! `CompletionSender` 経由で画面へ返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約（結果の三値・
//! 期限・守る範囲）の SSOT は `crate::contract`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch・bridge・本ファイルは #701 で最終形。後続の子はここを触らない）
//!
//! - iOS: `crate::ios`（#702。`SFSafariViewController`）
//! - Android: `crate::android`（#703。Custom Tabs）
//! - macOS・Windows・Linux: 対応する API を使わない（対応表で `no`）ため
//!   `UnsupportedInAppBrowser` 固定（ダミーの成功実装へは倒さない）。Web は対象外
//!
//! # OS 層が呼ぶ受け口 2 つ
//!
//! - [`try_begin_present`]: 表示を始める直前に UI スレッドから呼ぶ。`false` なら表示しない
//!   （期限切れ・古い要求・中断済み）。`true` を返した要求だけが「閉じた」を成功にできる
//! - [`complete_from_os`]: 結果を 1 回だけ送る唯一の受け口。表示前の失敗は `Err`、利用者が閉じたら
//!   `Ok(Closed)`。古い要求 id・重複は捨てられる
//!
//! # 期限（契約表 3）
//!
//! 開くまでの [`OPEN_DEADLINE`] だけを持つ。起点は要求を観測した時の `Instant`（単調時計）。
//! 期限切れの確定（`expire_if_due` と `try_begin_present` 内）は表示開始の記録と同じロックで
//! 行うため、期限切れを確定した後に OS が表示を始めることはない。表示開始後は期限を持たない。
//!
//! # スレッド
//!
//! drive は UI スレッド（毎フレーム）、`try_begin_present` と `complete_from_os` は OS 層の
//! スレッド（iOS は main queue、Android は JNI 経由の任意スレッド）から呼ばれる。判定・
//! `RequestSlot::deliver`・`active` の更新は同じロックの内側で行う。
//!
//! # 秘密値（契約表 8）
//!
//! URL と OS のエラー文はスロットにも画面にもログにも出さない。結果は理由の種別だけを持つ。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::contract::{
    InAppBrowser, InAppBrowserFailure, InAppBrowserOutcome, OPEN_DEADLINE, validate_url,
};
use fandhe_core::screens::in_app_browser_show::{
    CoreFailure, InAppBrowserShowState, Phase, finish_open, mark_running, take_request,
};

use fandhe_runtime::CompletionSender;
use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot, SlotOwner, poll_and_finish_owned};

/// OS 層から届く結果。
type Outcome = Result<InAppBrowserOutcome, InAppBrowserFailure>;

/// 現在のプラットフォームに応じた `InAppBrowser` 実装を返す（dispatch の最終形）。
pub fn platform_in_app_browser() -> Arc<dyn InAppBrowser> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Arc::new(crate::contract::UnsupportedInAppBrowser)
    }
}

/// 進行中の要求。
struct Active {
    request_id: u64,
    /// 要求を観測した時刻（単調時計）。期限の起点。
    started: Instant,
    /// OS が表示を始めたことを `try_begin_present` で記録済みか。
    presenting: bool,
}

/// 要求の追跡と期限・表示開始の排他。テストが並列で干渉しないよう static から切り出して
/// 引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Bridge {
    slot: &'static RequestSlot<Outcome>,
    /// 公開入口 [`open_url`] が追う要求。画面側はこの要求・未取得の結果を置き換えない。
    public: &'static SlotOwner,
    /// 画面 effect が追う要求。公開入口はこの要求・未取得の結果を置き換えない。
    screen: &'static SlotOwner,
    active: Mutex<Option<Active>>,
    deadline: Duration,
}

impl Bridge {
    fn new(deadline: Duration) -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            public: Box::leak(Box::new(SlotOwner::new())),
            screen: Box::leak(Box::new(SlotOwner::new())),
            active: Mutex::new(None),
            deadline,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 画面側の要求が発送済み（OS 呼び出し中、または結果が画面へ未反映）か。
    fn screen_pending(&self) -> bool {
        let mine = self.screen.current();
        mine != 0 && self.slot.snapshot().is_some_and(|(id, _)| id == mine)
    }

    /// 進行中の要求も未取得の結果（画面・公開とも）も無いときだけ、`owner` の分として登録して id を返す。
    /// 先行を置き換えず `None`。OS へ渡す前に呼び、即時の完了も照合できるようにする。
    fn begin(&self, owner: &SlotOwner) -> Option<u64> {
        let mut active = self.lock();
        let request_id = owner.begin_excluding(self.slot, &[self.public, self.screen])?;
        *active = Some(Active {
            request_id,
            started: Instant::now(),
            presenting: false,
        });
        Some(request_id)
    }

    #[allow(dead_code)] // 呼び出し元は OS 層（#702・#703）と単体テスト。
    /// 表示開始の記録。現在の要求で、未確定で、期限前のときだけ `true`。
    /// 期限を過ぎていれば `TimedOut` を確定して `false`。
    fn try_begin_present(&self, request_id: u64) -> bool {
        let mut active = self.lock();
        let Some(a) = active.as_mut().filter(|a| a.request_id == request_id) else {
            return false;
        };
        if a.presenting {
            return false;
        }
        if a.started.elapsed() >= self.deadline {
            self.slot
                .deliver(request_id, Err(InAppBrowserFailure::TimedOut));
            *active = None;
            return false;
        }
        a.presenting = true;
        true
    }

    /// OS 層からの唯一の結果の受け口。
    fn complete_from_os(&self, request_id: u64, result: Outcome) {
        let mut active = self.lock();
        let Some(a) = active.as_ref().filter(|a| a.request_id == request_id) else {
            return;
        };
        // 表示開始を記録していない要求の「閉じた」は嘘の成功になるため成功にしない。
        let result = match result {
            Ok(_) if !a.presenting => Err(InAppBrowserFailure::Internal),
            r => r,
        };
        self.slot.deliver(request_id, result);
        *active = None;
    }

    /// 表示開始前に期限を過ぎていれば `TimedOut` を確定する。
    fn expire_if_due(&self) {
        let mut active = self.lock();
        let due = active
            .as_ref()
            .is_some_and(|a| !a.presenting && a.started.elapsed() >= self.deadline);
        if due && let Some(a) = active.take() {
            self.slot
                .deliver(a.request_id, Err(InAppBrowserFailure::TimedOut));
        }
    }

    fn reset(&self) {
        let mut active = self.lock();
        *active = None;
        self.slot.reset();
        self.public.adopt(0);
        self.screen.adopt(0);
    }
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| Bridge::new(OPEN_DEADLINE))
}

/// OS 層が表示を始める直前に呼ぶ。`false` なら表示しない（モジュール doc 参照）。
// OS 層（#702・#703）が呼ぶまでは未使用（スタブは呼ばない）。
#[allow(dead_code)]
pub(crate) fn try_begin_present(request_id: u64) -> bool {
    bridge().try_begin_present(request_id)
}

/// OS 層が結果を 1 回だけ送る受け口（モジュール doc 参照）。
// OS 層（#702・#703）が呼ぶまでは未使用（スタブは呼ばない）。
#[allow(dead_code)]
pub(crate) fn complete_from_os(request_id: u64, result: Outcome) {
    bridge().complete_from_os(request_id, result);
}

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の要求が発送されず飲み込まれる。
/// 旧要求の遅延完了は id 不一致で捨てられる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn reset_pending_tracking() {
    if let Some(b) = BRIDGE.get() {
        b.reset();
    }
}

/// OS 層の失敗を画面の失敗種別へ写す。URL・OS の文言は渡さない。
fn to_core(f: InAppBrowserFailure) -> CoreFailure {
    match f {
        InAppBrowserFailure::Unsupported => CoreFailure::Unsupported,
        InAppBrowserFailure::InvalidUrl => CoreFailure::InvalidUrl,
        InAppBrowserFailure::NoPresenter => CoreFailure::NoPresenter,
        InAppBrowserFailure::NoBrowser => CoreFailure::NoBrowser,
        InAppBrowserFailure::TimedOut => CoreFailure::TimedOut,
        InAppBrowserFailure::Interrupted => CoreFailure::Interrupted,
        InAppBrowserFailure::Busy | InAppBrowserFailure::Internal => CoreFailure::Internal,
    }
}

/// 要求を登録し、検査を通った URL だけを OS へ渡す。検査失敗・同期失敗はその場で確定する。
/// 登録した要求 id を返す（結果の取得に使う）。先行が進行中・未取得なら置き換えず `None`。
fn start_request(
    bridge: &Bridge,
    imp: &dyn InAppBrowser,
    owner: &SlotOwner,
    raw_url: &str,
) -> Option<u64> {
    let request_id = bridge.begin(owner)?;
    match validate_url(raw_url) {
        Err(f) => bridge.complete_from_os(request_id, Err(f)),
        Ok(url) => {
            if let Err(f) = imp.open(request_id, &url) {
                bridge.complete_from_os(request_id, Err(f));
            }
        }
    }
    Some(request_id)
}

/// 画面状態に依存しない公開入口。URL を検査して要求を登録し、現在の OS 実装へ渡して要求 id を返す。
/// 同時に追跡できる要求は 1 件。先行の要求（画面経由を含む）が進行中、または結果が未取得なら
/// 置き換えず [`InAppBrowserFailure::Busy`] を返す。結果は [`poll_open_result`] で取得する
/// （OS 層の `try_begin_present` はこの要求を受理する）。
pub fn open_url(raw_url: &str) -> Result<u64, InAppBrowserFailure> {
    static IMPL: OnceLock<Arc<dyn InAppBrowser>> = OnceLock::new();
    let b = bridge();
    start_request(
        b,
        &**IMPL.get_or_init(platform_in_app_browser),
        b.public,
        raw_url,
    )
    .ok_or(InAppBrowserFailure::Busy)
}

/// [`open_url`] の結果の取得口。表示開始前に期限を過ぎていれば `TimedOut` を確定し、
/// 結果が届いていれば 1 度だけ返す（未着・古い id は `None`）。
pub fn poll_open_result(
    request_id: u64,
) -> Option<Result<InAppBrowserOutcome, InAppBrowserFailure>> {
    let b = bridge();
    // 画面側の要求の結果は取らない。
    if request_id == 0 || request_id != b.public.current() {
        return None;
    }
    b.expire_if_due();
    let r = b.slot.take_ready(request_id);
    if r.is_some() {
        b.public.release(request_id);
    }
    r
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &InAppBrowserShowState,
    sender: &CompletionSender<InAppBrowserShowState>,
    imp: &Arc<dyn InAppBrowser>,
    bridge: &Bridge,
) {
    if let Some((_, url)) = take_request(state) {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if bridge.screen_pending() {
            return;
        }
        let _ = sender.send(Box::new(mark_running));
        // 公開入口などの先行を置き換えず、画面には失敗として返す。
        if start_request(bridge, &**imp, bridge.screen, url).is_none() {
            let generation = state.generation;
            let _ = sender.send(Box::new(move |s: &mut InAppBrowserShowState| {
                finish_open(s, generation, Err(CoreFailure::Internal));
            }));
            return;
        }
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    bridge.expire_if_due();
    poll_and_finish_owned(
        bridge.slot,
        bridge.screen,
        sender,
        |_| {},
        move |s: &mut InAppBrowserShowState, outcome| {
            let result = outcome.map(|_| ()).map_err(to_core);
            finish_open(s, generation, result)
        },
        move |s: &mut InAppBrowserShowState| {
            finish_open(s, generation, Err(CoreFailure::Interrupted));
        },
    );
}

/// `registry.rs` の `effect` 区画から `in_app_browser_show` 画面用に配線される effect ドライバ。
pub fn in_app_browser_show_effect(
    state: &InAppBrowserShowState,
    sender: &CompletionSender<InAppBrowserShowState>,
) {
    static IMPL: OnceLock<Arc<dyn InAppBrowser>> = OnceLock::new();
    drive(
        state,
        sender,
        IMPL.get_or_init(platform_in_app_browser),
        bridge(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{BrowserUrl, UnsupportedInAppBrowser};
    use fandhe_core::screens::in_app_browser_show::request_open;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<fandhe_runtime::Completion<InAppBrowserShowState>>;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn channel() -> (CompletionSender<InAppBrowserShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn new_bridge(deadline: Duration) -> &'static Bridge {
        Box::leak(Box::new(Bridge::new(deadline)))
    }

    #[derive(Clone, Copy)]
    enum Mode {
        /// 同期で失敗を返す。
        Fail(InAppBrowserFailure),
        /// 何も起きない（OS が応答しない）。
        Silent,
        /// 表示を始め、`ms` 待ってから「閉じた」を送る。
        PresentThenClose(u64),
        /// 表示を始めずに「閉じた」を送る（契約違反の OS）。
        CloseWithoutPresent,
        /// 表示を始めずに中断を送る。
        Interrupt,
    }

    struct Fake {
        mode: Mode,
        calls: AtomicUsize,
        bridge: &'static Bridge,
    }

    impl InAppBrowser for Fake {
        fn open(&self, request_id: u64, _: &BrowserUrl) -> Result<(), InAppBrowserFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let b = self.bridge;
            match self.mode {
                Mode::Fail(f) => return Err(f),
                Mode::Silent => {}
                Mode::PresentThenClose(ms) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            std::thread::sleep(Duration::from_millis(ms));
                            b.complete_from_os(request_id, Ok(InAppBrowserOutcome::Closed));
                        }
                    });
                }
                Mode::CloseWithoutPresent => {
                    b.complete_from_os(request_id, Ok(InAppBrowserOutcome::Closed));
                }
                Mode::Interrupt => {
                    b.complete_from_os(request_id, Err(InAppBrowserFailure::Interrupted));
                }
            }
            Ok(())
        }
    }

    fn fake(mode: Mode, bridge: &'static Bridge) -> Arc<Fake> {
        Arc::new(Fake {
            mode,
            calls: AtomicUsize::new(0),
            bridge,
        })
    }

    /// フレームを回して届いた完了を適用する。`Idle` に戻るか 5 秒で打ち切る。
    fn pump(
        state: &mut InAppBrowserShowState,
        sender: &CompletionSender<InAppBrowserShowState>,
        rx: &Rx,
        imp: &Arc<dyn InAppBrowser>,
        bridge: &Bridge,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, bridge);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if state.phase == Phase::Idle || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(mode: Mode, deadline: Duration) -> (InAppBrowserShowState, Arc<Fake>) {
        let bridge = new_bridge(deadline);
        let f = fake(mode, bridge);
        let imp: Arc<dyn InAppBrowser> = f.clone();
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        (state, f)
    }

    #[test]
    fn no_request_does_nothing() {
        let bridge = new_bridge(Duration::from_secs(1));
        let imp: Arc<dyn InAppBrowser> = fake(Mode::Silent, bridge);
        let (sender, rx) = channel();
        drive(&InAppBrowserShowState::default(), &sender, &imp, bridge);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn present_then_close_is_success() {
        let (s, f) = run(Mode::PresentThenClose(0), Duration::from_secs(30));
        assert_eq!(s.result, Some(Ok(())));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsupported_impl_is_unsupported_not_success() {
        let bridge = new_bridge(Duration::from_secs(30));
        let imp: Arc<dyn InAppBrowser> = Arc::new(UnsupportedInAppBrowser);
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Unsupported)));
    }

    #[test]
    fn fallback_platform_is_unsupported_off_mobile() {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let u = validate_url("https://example.com/").unwrap();
            assert_eq!(
                platform_in_app_browser().open(1, &u),
                Err(InAppBrowserFailure::Unsupported)
            );
        }
    }

    #[test]
    fn public_entry_registers_request_and_returns_result() {
        // 公開入口と同じ経路（登録 → OS 呼び出し → 結果取得）が画面状態なしで通る。
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::PresentThenClose(0), bridge);
        let id = start_request(bridge, &*f, bridge.public, "https://example.com/").unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let r = loop {
            if let Some(r) = bridge.slot.take_ready(id) {
                break r;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(r, Ok(InAppBrowserOutcome::Closed));
        let id = start_request(bridge, &*f, bridge.public, "file:///x").unwrap();
        assert_eq!(
            bridge.slot.take_ready(id),
            Some(Err(InAppBrowserFailure::InvalidUrl))
        );
    }

    #[test]
    fn public_open_url_unsupported_off_mobile() {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let id = open_url("https://example.com/").unwrap();
            assert_eq!(
                poll_open_result(id),
                Some(Err(InAppBrowserFailure::Unsupported))
            );
        }
    }

    #[test]
    fn second_open_while_pending_is_busy_and_keeps_first() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let id = start_request(bridge, &*f, bridge.public, "https://example.com/").unwrap();
        assert!(start_request(bridge, &*f, bridge.public, "https://example.com/").is_none());
        assert!(start_request(bridge, &*f, bridge.screen, "https://example.com/").is_none());
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert_eq!(bridge.slot.snapshot(), Some((id, None)));
    }

    #[test]
    fn screen_does_not_replace_or_consume_public_result() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Interrupt, bridge);
        let imp: Arc<dyn InAppBrowser> = f.clone();
        let id = start_request(bridge, &*f, bridge.public, "https://example.com/").unwrap();
        // 公開入口の結果が未取得のまま画面が要求しても、置き換えず・横取りせず失敗にする。
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Internal)));
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Err(InAppBrowserFailure::Interrupted))))
        );
        assert_eq!(
            bridge.slot.take_ready(id),
            Some(Err(InAppBrowserFailure::Interrupted))
        );
    }

    #[test]
    fn public_cannot_poll_screen_request() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let id = start_request(bridge, &*f, bridge.screen, "https://example.com/").unwrap();
        assert_ne!(bridge.public.current(), id);
        assert_eq!(bridge.screen.current(), id);
    }

    #[test]
    fn invalid_url_never_reaches_os() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::PresentThenClose(0), bridge);
        for raw in [
            "javascript:alert(1)",
            "https://user@example.com/",
            "file:///x",
        ] {
            let id = start_request(bridge, &*f, bridge.public, raw).unwrap();
            assert_eq!(
                bridge.slot.take_ready(id),
                Some(Err(InAppBrowserFailure::InvalidUrl))
            );
            bridge.public.release(id);
        }
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn sync_open_failures_are_reported() {
        for (f, expect) in [
            (InAppBrowserFailure::NoBrowser, CoreFailure::NoBrowser),
            (InAppBrowserFailure::NoPresenter, CoreFailure::NoPresenter),
            (InAppBrowserFailure::Internal, CoreFailure::Internal),
        ] {
            let (s, _) = run(Mode::Fail(f), Duration::from_secs(30));
            assert_eq!(s.result, Some(Err(expect)));
        }
    }

    #[test]
    fn interrupted_is_a_failure() {
        let (s, _) = run(Mode::Interrupt, Duration::from_secs(30));
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
    }

    #[test]
    fn deadline_before_present_times_out_and_blocks_later_present() {
        let bridge = new_bridge(Duration::from_millis(20));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn InAppBrowser> = f.clone();
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::TimedOut)));
        // 期限切れを確定した後は、遅れて来た OS に表示を始めさせない。
        assert!(!bridge.try_begin_present(1));
    }

    #[test]
    fn present_attempt_after_deadline_times_out_atomically() {
        let bridge = new_bridge(Duration::from_millis(10));
        let id = bridge.begin(bridge.public).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!bridge.try_begin_present(id));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Err(InAppBrowserFailure::TimedOut))))
        );
        // 期限後に届いた「閉じた」は成功にならない（active は除去済みで捨てられる）。
        bridge.complete_from_os(id, Ok(InAppBrowserOutcome::Closed));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Err(InAppBrowserFailure::TimedOut))))
        );
    }

    #[test]
    fn after_present_deadline_no_longer_applies() {
        // 期限 30ms。表示開始後に 120ms かけて閉じても TimedOut にならない。
        let (s, _) = run(Mode::PresentThenClose(120), Duration::from_millis(30));
        assert_eq!(s.result, Some(Ok(())));
    }

    #[test]
    fn closed_without_present_is_not_success() {
        let (s, _) = run(Mode::CloseWithoutPresent, Duration::from_secs(30));
        assert_eq!(s.result, Some(Err(CoreFailure::Internal)));
    }

    #[test]
    fn stale_request_id_is_dropped_and_duplicates_first_wins() {
        let bridge = new_bridge(Duration::from_secs(30));
        let id = bridge.begin(bridge.public).unwrap();
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id + 1, Err(InAppBrowserFailure::NoBrowser));
        assert_eq!(bridge.slot.snapshot(), Some((id, None)));
        bridge.complete_from_os(id, Ok(InAppBrowserOutcome::Closed));
        bridge.complete_from_os(id, Err(InAppBrowserFailure::Internal));
        assert_eq!(
            bridge.slot.snapshot(),
            Some((id, Some(Ok(InAppBrowserOutcome::Closed))))
        );
        // 表示開始は 1 回だけ。
        let id2 = {
            bridge.reset();
            bridge.begin(bridge.public).unwrap()
        };
        assert!(bridge.try_begin_present(id2));
        assert!(!bridge.try_begin_present(id2));
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        mark_running(&mut state);
        assert!(!finish_open(&mut state, 99, Ok(())));
        assert_eq!(state.phase, Phase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_opens_once() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn InAppBrowser> = f.clone();
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        drive(&state, &sender, &imp, bridge);
        drive(&state, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "mark_running は 1 件のみ");
    }

    #[test]
    fn reset_drops_old_completion_and_allows_next_dispatch() {
        let bridge = new_bridge(Duration::from_secs(30));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn InAppBrowser> = f.clone();
        let (sender, rx) = channel();
        let mut state = InAppBrowserShowState::default();
        request_open(&mut state);
        drive(&state, &sender, &imp, bridge);
        let old_id = bridge.slot.snapshot().unwrap().0;
        // Activity 再生成: 画面は default に戻り、記録を捨てる。
        bridge.reset();
        bridge.complete_from_os(old_id, Ok(InAppBrowserOutcome::Closed));
        assert_eq!(bridge.slot.snapshot(), None);
        let mut fresh = InAppBrowserShowState::default();
        request_open(&mut fresh);
        while rx.try_recv().is_ok() {}
        drive(&fresh, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failure_texts_never_contain_url() {
        for f in [
            InAppBrowserFailure::NoBrowser,
            InAppBrowserFailure::Internal,
            InAppBrowserFailure::InvalidUrl,
        ] {
            let (s, _) = run(Mode::Fail(f), Duration::from_secs(30));
            let t = fandhe_core::screens::in_app_browser_show::error_text(&s);
            assert!(!t.contains("example.com") && !t.contains("http"), "{t}");
        }
    }
}
