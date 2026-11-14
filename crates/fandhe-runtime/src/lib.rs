//! 起床と完了通知・描画契約（TASK-21.2、Issue #1162・#1163）。
//!
//! 機能 crate が egui に依存しないよう、「別スレッドの完了結果を UI スレッドへ渡し、
//! 描画ループを起こす」責務だけをレンダラ非依存で切り出した crate。依存方向は
//! `fandhe-egui-proxy`（レンダラ実装）→ 本 crate → `fandhe-core` の一方向で、
//! 本 crate は egui / eframe / winit を参照しない。
//!
//! - レンダラ差し替えの契約は [`Renderer`]（1 フレーム分の画面描画）。描画命令
//!   [`PaintCommand`] は定義を `fandhe-core` に残したまま再公開する
//! - レンダラ実装は [`RepaintWaker`] を実装して [`CompletionSender`] へ渡す
//! - OS コールバックのように sender を持たない経路は [`request_repaint`] で起こす
//!   （レンダラ実装が起動時に [`set_repaint_waker`] で登録する）
//! - 描画層の観測値と操作（フォーカス・ファイルのドラッグ・拡大率・画面の読み戻し・ウィンドウ操作・入力欄のヒント）は [`HostView`]。
//!   [`Renderer`] には載せず、[`RepaintWaker::host_view`] で [`CompletionSender::host_view`] へ届ける
//! - [`image_store`] は effect ドライバが復号済みの画素を描画側へ渡す 1 枠の保管所
//! - [`request_slot`] は request/response 型の端末機能が使う「要求 1 件の追跡」
//! - [`tick`] は時間で進む画面の effect ドライバが共用する、フレームごとの経過時間の実測

#![forbid(unsafe_code)]

pub mod host_view;
pub mod image_store;
pub mod request_slot;
pub mod tick;

use std::any::Any;
use std::sync::mpsc::{Receiver, SendError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 描画命令。定義は `fandhe-core`（`Widget::paint` が返すため）にあり、ここは再公開のみ。
pub use fandhe_core::PaintCommand;
use fandhe_core::{Screen, StateStore};
use host_view::NoHostView;
pub use host_view::{
    HostView, Screenshot, TextInputHint, TextInputHintError, WindowCommand, WindowRect, WindowState,
};

/// レンダラ差し替えの契約。1 フレーム分の `screen` を描き、UI イベントを `store.dispatch` へ流す。
///
/// 実装は `fandhe-egui-proxy` の `EguiRenderer` など。`PaintCommand`（`Widget::paint` が返す
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
}

// `#[derive(Clone)]` は `S: Clone` を要求するため手動実装する（S 自体は複製しない）。
impl<S> Clone for CompletionSender<S> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<S> CompletionSender<S> {
    /// 送信側と起床口から構築する。レンダラ実装（`ProxyApp::completion_sender` 等）が呼ぶ。
    pub fn new(tx: Sender<Completion<S>>, waker: Arc<dyn RepaintWaker>) -> Self {
        Self { tx, waker }
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
