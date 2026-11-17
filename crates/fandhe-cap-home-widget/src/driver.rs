//! `fandhe_core::screens::home_widget_show` の effect ドライバ（home-widget 機能 crate の `driver`）。
//!
//! 画面の要求（読み出し・保存）を観測して [`HomeWidget`] を別スレッドで呼び、`CompletionSender` 経由で
//! `finish_load` / `finish_set` を送る。呼び出し元は `fandhe-gallery` の `screens/registry.rs` の
//! `effect "home_widget_show"` 行。画面の状態型を参照してよいのは本モジュールだけ
//! （契約・OS 実装は `crate` 直下・`android` / `macos`。`consumer-app-structure.md` §8.1）。

use std::sync::Arc;

use fandhe_core::screens::home_widget_show::{
    HomeWidgetRequest, HomeWidgetShowState, UNAVAILABLE_MARKER, finish_load, finish_set,
    mark_dispatched, take_request,
};
use fandhe_runtime::CompletionSender;

use crate::{HomeWidget, HomeWidgetError, platform_home_widget};

/// `Unsupported` は画面側が判定する固定文言へ、それ以外は `Display` のみ転送する
/// （内部構造・パスを漏らさない、security.md A09）。
fn to_message(e: HomeWidgetError) -> String {
    match e {
        HomeWidgetError::Unsupported => UNAVAILABLE_MARKER.to_string(),
        other => other.to_string(),
    }
}

/// 要求 1 件を別スレッドで処理して完了を送る（テストから実装を差し込めるよう分離）。
///
/// App Group コンテナへの初回アクセスで OS の確認ダイアログが出うる・`open` の待ちが
/// あるなどブロック時間に上限が無いため、別スレッドで呼ぶ。画面側は処理中ボタンを外し
/// 要求を高々 1 件にするので世代照合は不要。
fn drive(
    state: &HomeWidgetShowState,
    sender: &CompletionSender<HomeWidgetShowState>,
    widget: Arc<dyn HomeWidget>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    // 起動済みマーク（effect は毎フレーム呼ばれるため二重起動を防ぐ）。
    let _ = sender.send(Box::new(mark_dispatched));
    let sender = sender.clone();
    std::thread::spawn(move || match request {
        HomeWidgetRequest::Load => {
            let result = widget.current_text().map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut HomeWidgetShowState| {
                finish_load(s, result);
            }));
        }
        HomeWidgetRequest::Set(text) => {
            let result = widget.set_text(&text).map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut HomeWidgetShowState| {
                finish_set(s, result);
            }));
        }
    });
}

/// `registry.rs` の `effect` 区画から `home_widget_show` 画面用に配線される effect ドライバ。
pub fn home_widget_show_effect(
    state: &HomeWidgetShowState,
    sender: &CompletionSender<HomeWidgetShowState>,
) {
    drive(state, sender, platform_home_widget());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnsupportedHomeWidget;
    use fandhe_core::screens::home_widget_show::{shown_text, status_text};
    use fandhe_runtime::{Completion, RepaintWaker};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<HomeWidgetShowState>,
        Receiver<Completion<HomeWidgetShowState>>,
    ) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    #[derive(Default)]
    struct Mem(Mutex<Option<String>>);
    impl HomeWidget for Mem {
        fn set_text(&self, text: &str) -> Result<(), HomeWidgetError> {
            *self.0.lock().expect("lock") = Some(text.to_string());
            Ok(())
        }
        fn current_text(&self) -> Result<Option<String>, HomeWidgetError> {
            Ok(self.0.lock().expect("lock").clone())
        }
    }

    struct Blocking;
    impl HomeWidget for Blocking {
        fn set_text(&self, _: &str) -> Result<(), HomeWidgetError> {
            loop {
                std::thread::park();
            }
        }
        fn current_text(&self) -> Result<Option<String>, HomeWidgetError> {
            loop {
                std::thread::park();
            }
        }
    }

    #[test]
    fn load_then_set_round_trip() {
        let widget: Arc<dyn HomeWidget> = Arc::new(Mem::default());
        let (sender, rx) = sender_with_channel();
        let mut state = HomeWidgetShowState::default();
        drive(&state, &sender, widget.clone());
        rx.recv().expect("起動済みマーク")(&mut state);
        assert!(take_request(&state).is_none());
        rx.recv().expect("読み出し完了")(&mut state);
        assert_eq!(shown_text(&state), "ウィジェットの文字: 未設定");

        let screen = fandhe_core::screens::home_widget_show::current_screen(&state);
        assert!(fandhe_core::click_button(&screen, "休憩中", &mut state));
        drive(&state, &sender, widget);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("保存完了")(&mut state);
        assert_eq!(shown_text(&state), "ウィジェットの文字: 休憩中");
    }

    #[test]
    fn unsupported_is_reported_as_unavailable() {
        let (sender, rx) = sender_with_channel();
        let mut state = HomeWidgetShowState::default();
        drive(&state, &sender, Arc::new(UnsupportedHomeWidget));
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert_eq!(status_text(&state), UNAVAILABLE_MARKER);
    }

    #[test]
    fn no_respawn_while_dispatched() {
        let (sender, rx) = sender_with_channel();
        let mut state = HomeWidgetShowState::default();
        drive(&state, &sender, Arc::new(Blocking));
        rx.recv().expect("起動済みマーク")(&mut state);
        drive(&state, &sender, Arc::new(Blocking));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn no_request_is_noop() {
        let (sender, rx) = sender_with_channel();
        let mut state = HomeWidgetShowState::default();
        drive(&state, &sender, Arc::new(Mem::default()));
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        drive(&state, &sender, Arc::new(Mem::default()));
        assert!(rx.try_recv().is_err());
    }
}
