//! 起床と完了通知・描画契約（TASK-21.2、Issue #1162・#1163）。
//!
//! 機能 crate が egui に依存しないよう、「別スレッドの完了結果を UI スレッドへ渡し、
//! 描画ループを起こす」責務だけをレンダラ非依存で切り出した crate。依存方向は
//! `fandhe-render-egui`（レンダラ実装）→ 本 crate → `fandhe-core` の一方向で、
//! 本 crate は egui / eframe / winit を参照しない。
//!
//! - レンダラ差し替えの契約は [`Renderer`]（1 フレーム分の画面描画）。描画命令
//!   [`PaintCommand`] は定義を `fandhe-core` に残したまま再公開する
//! - レンダラ実装は [`RepaintWaker`] を実装して [`CompletionSender`] へ渡す
//! - OS コールバックのように sender を持たない経路は [`request_repaint`] で起こす
//!   （レンダラ実装が起動時に [`set_repaint_waker`] で登録する）
//! - 描画層の観測値と操作（フォーカス・ファイルのドラッグ・拡大率・画面の読み戻し・ウィンドウ操作・入力欄のヒント）は [`HostView`]。
//!   [`Renderer`] には載せず、[`RepaintWaker::host_view`] で [`CompletionSender::host_view`] へ届ける
//! - [`image_store`] は effect ドライバが復号済みの画素を、識別子ごとに複数枚、描画側へ渡す保管所（容量上限と LRU の追い出しつき）
//! - [`timer`] は「一定時間後に状態を変える」予約表。effect からは [`CompletionSender::after`] で使う
//! - [`request_slot`] は request/response 型の端末機能が使う「要求 1 件の追跡」
//! - [`tick`] は時間で進む画面の effect ドライバが共用する、フレームごとの経過時間の実測
//! - [`teardown`] は終了処理・画面の破棄・Android の再生成時の破棄の登録表。機能 crate が初めて使われたときに登録する

#![forbid(unsafe_code)]

pub mod host_view;
pub mod image_store;
pub mod request_slot;
pub mod teardown;
pub mod tick;
pub mod timer;

use std::any::Any;
use std::sync::mpsc::{Receiver, SendError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub use timer::{Armed, TimerRefused, Timers, drain_timers};

/// 描画命令。定義は `fandhe-core`（`Widget::paint` が返すため）にあり、ここは再公開のみ。
pub use fandhe_core::PaintCommand;
use fandhe_core::{Screen, StateStore};
use host_view::NoHostView;
pub use host_view::{
    HostView, Screenshot, TextInputHint, TextInputHintError, WindowCommand, WindowRect, WindowState,
};

/// レンダラ差し替えの契約。1 フレーム分の `screen` を描き、UI イベントを `store.dispatch` へ流す。
///
/// 実装は `fandhe-render-egui` の `EguiRenderer` など。`PaintCommand`（`Widget::paint` が返す
/// 描画命令）をどこまで使うかは実装に任せる。`fandhe_core::RendererLifecycle`（Web の
/// canvas / DOM の排他起動）とは別物。
///
/// - UI スレッドから 1 フレームに 1 回呼ぶ。`Send` / `Sync` は要求しない
/// - 状態は `store.state()` で読み、UI イベントによる書き込みは `store.dispatch` だけで行う
///   （`&mut S` を外へ出さない）
/// - [`drain_completions`] は `render` の前に、[`EffectFn`] は後に、描画ループ側が呼ぶ。
///   `render` の仕事に含めない
/// - 起床は別契約の [`RepaintWaker`] を実装して担う
pub trait Renderer<S> {
    /// 1 フレーム分の画面を描く。
    fn render(&mut self, screen: &Screen<S>, store: &mut StateStore<S>);
}

/// 描画ループの起床口。レンダラ実装（egui 等）が実装する。
///
/// `Any` を継承するのは、レンダラ側が自分の実装へ戻す（downcast）暫定の橋渡しのため。
/// 起床以外の依存が解消したら外せる。
pub trait RepaintWaker: Any + Send + Sync {
    /// 次のフレームを直ちに要求する。
    fn wake(&self);
    /// `delay` 経過後に 1 回だけフレームを要求する。
    fn wake_after(&self, delay: Duration);
    /// 描画層の観測値と操作の口（[`HostView`]）。レンダラ実装が上書きする。既定は既定値だけを返す口。
    fn host_view(&self) -> &dyn HostView {
        &NoHostView
    }
}

/// プロセス単位の起床口。sender を持たない OS コールバックが使う。
static WAKER: Mutex<Option<Arc<dyn RepaintWaker>>> = Mutex::new(None);

/// プロセス単位の起床口を登録する。既にあれば差し替える（Android は Activity 再生成の
/// たびに起動処理が再実行され、古い描画コンテキストを残さないため）。
pub fn set_repaint_waker(waker: Arc<dyn RepaintWaker>) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
}

/// 登録済みの起床口で [`RepaintWaker::wake`] を呼ぶ。未登録（起動前）は何もしない。
pub fn request_repaint() {
    // ロックを放してから呼ぶ（wake 内で再入してもデッドロックしない）。
    let waker = WAKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// 登録済みの起床口で [`RepaintWaker::wake_after`] を呼ぶ。未登録（起動前）は何もしない。
pub fn request_repaint_after(delay: Duration) {
    let waker = WAKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(waker) = waker {
        waker.wake_after(delay);
    }
}

/// 外部 I/O の完了結果を状態へ適用するクロージャ（`StateStore::apply` へ渡す）。
///
/// 別スレッドから [`CompletionSender`] 経由でチャンネルへ送られ、UI スレッドの
/// [`drain_completions`] がフレーム先頭で取り出して実行する。`Send` 境界により生成元
/// スレッドが異なっても UI スレッドへ安全に渡せる。
pub type Completion<S> = Box<dyn FnOnce(&mut S) + Send>;

/// 画面非依存の汎用 I/O 起動フック。レンダラ実装がフレーム末尾で
/// `effect(store.state(), &sender)` として毎フレーム呼ぶ契約。`&S` の借用のみで状態を
/// 直接変更せず、変更は必ず [`CompletionSender::send`] → `apply` 経由（`version` はここでは
/// 進まない）。「起動が必要か」は状態側のフラグで表現する画面と組み合わせる。
pub type EffectFn<S> = fn(&S, &CompletionSender<S>);

/// 完了キューの送信ハンドル。`send` がキュー投入と描画ループの起床を一体で行う。
///
/// 素の `mpsc::Sender` だと、UI が入力待ちでアイドルのとき次の入力まで
/// [`drain_completions`] が走らない。
pub struct CompletionSender<S> {
    tx: Sender<Completion<S>>,
    waker: Arc<dyn RepaintWaker>,
    /// 予約表への**弱い**参照。表の持ち主は描画ループの持ち主（`ProxyApp`）だけ。予約のクロージャが sender の複製を
    /// 抱えても `Timers → クロージャ → sender → Timers` の循環にならず、持ち主の破棄で予約・捕まえた資源が解放される。
    timers: Weak<Timers<S>>,
}

// `#[derive(Clone)]` は `S: Clone` を要求するため手動実装する（S 自体は複製しない）。
impl<S> Clone for CompletionSender<S> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            waker: self.waker.clone(),
            timers: self.timers.clone(),
        }
    }
}

impl<S> CompletionSender<S> {
    /// 送信側と起床口から構築する。レンダラ実装（`ProxyApp::completion_sender` 等）が呼ぶ。
    ///
    /// 予約表（[`Self::after`]）の持ち主はまだ無い。描画ループの持ち主が [`drain_timers`] で流す表と
    /// 共有するときは [`Self::with_timers`] で渡す（渡さない送信側の `after` は [`TimerRefused::Closed`] で断る）。
    pub fn new(tx: Sender<Completion<S>>, waker: Arc<dyn RepaintWaker>) -> Self {
        Self {
            tx,
            waker,
            timers: Weak::new(),
        }
    }

    /// 予約表を、描画ループの持ち主が [`drain_timers`] で流す表へつなぐ。送信側は表を**弱く**参照する
    /// （持ち主が `Arc` を手放せば表は解放され、予約は取り下げられ、以後の `after` は [`TimerRefused::Closed`]）。
    pub fn with_timers(mut self, timers: &Arc<Timers<S>>) -> Self {
        self.timers = Arc::downgrade(timers);
        self
    }

    /// `delay` 後に `completion` を状態へ適用する予約をする。スレッドも sleep も要らない。
    ///
    /// `key` は予約の名前。同じ `key` の予約が残っている間は何もせず `Ok(Armed::AlreadyPending)` を返す（期限は延びない）ため、
    /// effect で「状態がこうなら」と毎フレーム呼んでよい。新しく積んだときは `Ok(Armed::New)`。
    /// **積まなかった**ときは `Err`（同時の予約が [`timer::MAX_PENDING`] 件に達した `Full`、期限が時計の範囲を超える `TooFar`、
    /// 表の持ち主（アプリ）が破棄済み・表につながっていない `Closed`）。
    /// `AlreadyPending` と `Err` は別物なので、`Err` は捨てずに扱う。
    /// 期限は呼んだ時点から単調時計で測り、来たらフレーム先頭で適用される（詳細と書き方は [`timer`] 参照）。
    #[must_use = "Err は予約が積まれていないことを表す"]
    pub fn after(
        &self,
        key: &str,
        delay: Duration,
        completion: impl FnOnce(&mut S) + Send + 'static,
    ) -> Result<Armed, TimerRefused> {
        let timers = self.timers.upgrade().ok_or(TimerRefused::Closed)?;
        let armed = timers.arm_at(web_time::Instant::now(), key, delay, Box::new(completion))?;
        if armed == Armed::New {
            self.waker.wake_after(delay);
        }
        Ok(armed)
    }

    /// [`Self::after`] の予約を取り下げる。あれば `true`。
    pub fn cancel_timer(&self, key: &str) -> bool {
        self.timers.upgrade().is_some_and(|t| t.cancel(key))
    }

    /// 完了クロージャをキューへ積み、直後に起床する。受信側が drop 済みのときだけ `Err`
    /// （その場合は起床しない）。
    pub fn send(&self, completion: Completion<S>) -> Result<(), SendError<Completion<S>>> {
        self.tx.send(completion)?;
        self.waker.wake();
        Ok(())
    }

    /// キューへは積まず、`duration` 後に 1 回だけ起床する予約をする。pull 方式のセンサ等が
    /// 送信を省略した後も定期的に読み直すために使う。
    pub fn request_repaint_after(&self, duration: Duration) {
        self.waker.wake_after(duration);
    }

    /// 描画層の観測値と操作の口。機能 crate が egui を知らずにフォーカス等を読むために使う。
    pub fn host_view(&self) -> &dyn HostView {
        self.waker.host_view()
    }

    /// 起床口そのもの。レンダラ実装が自分の型へ戻す暫定の橋渡し用で、機能 crate は使わない。
    pub fn waker(&self) -> &dyn RepaintWaker {
        &*self.waker
    }
}

/// キューに溜まった完了クロージャを非ブロッキングで取り出し、`store.apply` へ順に渡す
/// （FIFO）。レンダラ実装がフレーム先頭で呼ぶ。空なら何もせず `version` も進まない。
pub fn drain_completions<S>(store: &mut StateStore<S>, rx: &Receiver<Completion<S>>) {
    while let Ok(completion) = rx.try_recv() {
        store.apply(completion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::channel;

    #[derive(Default)]
    struct Counter {
        count: u32,
        note: String,
    }

    #[test]
    fn paint_command_is_reexported_from_core() {
        let c: PaintCommand = fandhe_core::PaintCommand::Rect;
        assert_eq!(c, PaintCommand::Rect);
    }

    /// egui なしで `Renderer` を実装でき、`dyn` 越しに呼べる。
    struct Headless(Vec<String>);

    impl Renderer<Counter> for Headless {
        fn render(&mut self, screen: &Screen<Counter>, store: &mut StateStore<Counter>) {
            self.0 = fandhe_core::render_headless(screen, store.state());
        }
    }

    #[test]
    fn renderer_is_implementable_without_egui() {
        let screen = Screen::new("題").text("本文");
        let mut store = StateStore::new(Counter::default());
        let mut r = Headless(Vec::new());
        let dynr: &mut dyn Renderer<Counter> = &mut r;
        dynr.render(&screen, &mut store);
        assert_eq!(r.0, vec!["# 題".to_string(), "本文".to_string()]);
    }

    #[derive(Default)]
    struct FakeWaker {
        wakes: AtomicUsize,
        afters: Mutex<Vec<Duration>>,
    }

    impl RepaintWaker for FakeWaker {
        fn wake(&self) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_after(&self, delay: Duration) {
            self.afters.lock().unwrap().push(delay);
        }
    }

    /// 口を提供しない起床口の `host_view` は「観測していない」既定値を返す。
    #[test]
    fn host_view_defaults_when_waker_has_none() {
        let (tx, _rx) = channel::<Completion<Counter>>();
        let sender = CompletionSender::new(tx, Arc::new(FakeWaker::default()));
        let v = sender.host_view();
        assert!(v.focused());
        assert_eq!(v.file_drag(), (false, Vec::new()));
        assert!(!v.pointer_primary_down());
        v.set_zoom_factor(2.0);
        assert_eq!(v.applied_zoom_factor(), None);
        v.request_screenshot(1);
        assert!(v.tagged_screenshot(1).is_none());
        assert_eq!(v.viewport_size(), None);
        assert_eq!(v.window_state(), WindowState::default());
        v.request_window(WindowCommand::AlwaysOnTop(true));
        v.request_window(WindowCommand::Fullscreen(true));
        assert_eq!(v.native_pixels_per_point(), None);
        let id = fandhe_core::WidgetId::from_raw(1);
        assert_eq!(v.canvas_pointer(id), None);
        assert!(!v.canvas_pressed(id));
        assert_eq!(v.canvas_released(id), None);
        assert_eq!(
            v.set_text_input_hint(TextInputHint::OneTimeCode),
            Err(TextInputHintError::Unsupported)
        );
    }

    struct ViewWaker;
    impl RepaintWaker for ViewWaker {
        fn wake(&self) {}
        fn wake_after(&self, _: Duration) {}
        fn host_view(&self) -> &dyn HostView {
            self
        }
    }
    impl HostView for ViewWaker {
        fn focused(&self) -> bool {
            false
        }
    }

    /// waker が上書きした `host_view` の値が sender から見える。
    #[test]
    fn host_view_override_is_visible_from_sender() {
        let (tx, _rx) = channel::<Completion<Counter>>();
        let sender = CompletionSender::new(tx, Arc::new(ViewWaker));
        assert!(!sender.host_view().focused());
    }

    #[test]
    fn drain_applies_queued_completion_and_bumps_version() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = channel::<Completion<Counter>>();
        tx.send(Box::new(|s: &mut Counter| s.count += 1)).unwrap();
        drain_completions(&mut store, &rx);
        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    #[test]
    fn drain_applies_in_fifo_order() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = channel::<Completion<Counter>>();
        tx.send(Box::new(|s: &mut Counter| s.note.push('a')))
            .unwrap();
        tx.send(Box::new(|s: &mut Counter| s.note.push('b')))
            .unwrap();
        drain_completions(&mut store, &rx);
        assert_eq!(store.state().note, "ab");
        assert_eq!(store.version(), 2);
    }

    #[test]
    fn drain_is_noop_when_queue_empty() {
        let mut store = StateStore::new(Counter::default());
        let (_tx, rx) = channel::<Completion<Counter>>();
        drain_completions(&mut store, &rx);
        assert_eq!(store.version(), 0);
    }

    #[test]
    fn send_from_another_thread_is_applied() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = channel::<Completion<Counter>>();
        std::thread::spawn(move || {
            tx.send(Box::new(|s: &mut Counter| s.count += 1)).unwrap();
        })
        .join()
        .unwrap();
        drain_completions(&mut store, &rx);
        assert_eq!(store.state().count, 1);
    }

    #[test]
    fn send_wakes_once_and_clone_shares_waker() {
        let waker = Arc::new(FakeWaker::default());
        let (tx, _rx) = channel::<Completion<Counter>>();
        let sender = CompletionSender::new(tx, waker.clone());
        sender
            .send(Box::new(|s: &mut Counter| s.count += 1))
            .unwrap();
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 1);
        sender.clone().send(Box::new(|_| {})).unwrap();
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn send_does_not_wake_when_receiver_dropped() {
        let waker = Arc::new(FakeWaker::default());
        let (tx, rx) = channel::<Completion<Counter>>();
        drop(rx);
        let sender = CompletionSender::new(tx, waker.clone());
        assert!(sender.send(Box::new(|_| {})).is_err());
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 0);
    }

    /// `after` は同じキーの二重予約を積まず、起床も 1 回だけ予約する。取り下げたキーは積み直せる。
    #[test]
    fn after_arms_once_per_key_and_cancel_rearms() {
        let waker = Arc::new(FakeWaker::default());
        let (tx, _rx) = channel::<Completion<Counter>>();
        let timers = Arc::new(Timers::default());
        let sender = CompletionSender::new(tx, waker.clone()).with_timers(&timers);
        let d = Duration::from_secs(60);
        assert_eq!(
            sender.after("k", d, |s: &mut Counter| s.count += 1),
            Ok(Armed::New)
        );
        assert_eq!(
            sender.after("k", d, |s: &mut Counter| s.count += 10),
            Ok(Armed::AlreadyPending)
        );
        assert_eq!(*waker.afters.lock().unwrap(), vec![d]);
        assert!(
            sender.clone().cancel_timer("k"),
            "複製の送信側とも予約表を共有する"
        );
        assert_eq!(sender.after("k", d, |_: &mut Counter| {}), Ok(Armed::New));
        assert_eq!(waker.afters.lock().unwrap().len(), 2);
        // 溢れる期限は積まず、起床も予約しない。
        assert_eq!(
            sender.after("huge", Duration::MAX, |_: &mut Counter| {}),
            Err(TimerRefused::TooFar)
        );
        assert_eq!(waker.afters.lock().unwrap().len(), 2);
    }

    /// 予約のクロージャが sender の複製と資源を抱えても、持ち主（表の `Arc`）を手放せば解放される（循環しない）。
    /// 外のワーカーが sender を持ち続けても、破棄後の `after` は断られ、予約は残らない。
    #[test]
    fn dropping_the_timer_owner_frees_closures_even_if_they_hold_the_sender() {
        let waker = Arc::new(FakeWaker::default());
        let (tx, _rx) = channel::<Completion<Counter>>();
        let owner = Arc::new(Timers::default());
        let sender = CompletionSender::new(tx, waker).with_timers(&owner);
        let resource = Arc::new(());

        let (captured_sender, captured_res) = (sender.clone(), resource.clone());
        let armed = sender.after("k", Duration::from_secs(60), move |_: &mut Counter| {
            let _ = (&captured_sender, &captured_res);
        });
        assert_eq!(armed, Ok(Armed::New));
        assert_eq!(Arc::strong_count(&resource), 2, "予約が資源を抱えている");

        let outside = sender.clone(); // 外のワーカーが持ち続ける sender
        drop(sender);
        drop(owner); // アプリの破棄
        assert_eq!(
            Arc::strong_count(&resource),
            1,
            "持ち主の破棄で予約・捕まえた資源が解放される"
        );
        assert_eq!(
            outside.after("again", Duration::from_secs(1), |_: &mut Counter| {}),
            Err(TimerRefused::Closed)
        );
        assert!(!outside.cancel_timer("k"));
    }

    /// 表につないでいない sender の `after` は断る（誰も流さない予約を黙って溜めない）。
    #[test]
    fn after_without_an_owner_is_closed() {
        let (tx, _rx) = channel::<Completion<Counter>>();
        let sender = CompletionSender::new(tx, Arc::new(FakeWaker::default()));
        assert_eq!(
            sender.after("k", Duration::from_secs(1), |_: &mut Counter| {}),
            Err(TimerRefused::Closed)
        );
    }

    #[test]
    fn request_repaint_after_forwards_delay() {
        let waker = Arc::new(FakeWaker::default());
        let (tx, _rx) = channel::<Completion<Counter>>();
        let sender = CompletionSender::new(tx, waker.clone());
        sender.request_repaint_after(Duration::from_millis(30));
        assert_eq!(
            *waker.afters.lock().unwrap(),
            vec![Duration::from_millis(30)]
        );
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 0);
    }

    /// static を触るため 1 テストにまとめる（並列実行で互いに干渉させない）。
    #[test]
    fn process_waker_is_noop_until_set_and_replaced_by_set() {
        request_repaint();
        let first = Arc::new(FakeWaker::default());
        set_repaint_waker(first.clone());
        request_repaint();
        assert_eq!(first.wakes.load(Ordering::SeqCst), 1);
        let second = Arc::new(FakeWaker::default());
        set_repaint_waker(second.clone());
        request_repaint();
        assert_eq!(first.wakes.load(Ordering::SeqCst), 1);
        assert_eq!(second.wakes.load(Ordering::SeqCst), 1);
    }
}
