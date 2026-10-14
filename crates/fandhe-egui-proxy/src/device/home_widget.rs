//! Phase 11 の拡充ループ（親 #1064、Issue #1065）型サンプル「ホーム画面ウィジェット」の
//! effect ドライバ。`fandhe_core::screens::home_widget_show`（端末依存ゼロ）の `pending`
//! 要求を観測し、`fandhe_device::home_widget::HomeWidget` で保存・読み出しして
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる、`device/secure_credential.rs` と同型）。
//!
//! 実装の選択: macOS は [`super::apple_home_widget`]（Team ID が取れなければ
//! `UnsupportedHomeWidget`）、Android は [`super::android_home_widget`]（#1067）、他 OS は
//! `UnsupportedHomeWidget`（iOS は #1066）。
//!
//! App Group コンテナへの初回アクセスで OS の確認ダイアログが出うる・`open` の待ちが
//! あるなどブロック時間に上限が無いため、別スレッドで呼ぶ。画面側は処理中ボタンを外し
//! 要求を高々 1 件にするので世代照合は不要。

use std::sync::Arc;

use fandhe_core::screens::home_widget_show::{
    finish_load, finish_set, mark_dispatched, take_request, HomeWidgetRequest, HomeWidgetShowState,
    UNAVAILABLE_MARKER,
};
use fandhe_device::home_widget::HomeWidget;
#[cfg(not(target_os = "android"))]
use fandhe_device::home_widget::UnsupportedHomeWidget;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `Unsupported` は画面側が判定する固定文言へ、それ以外は `Display` のみ転送する
/// （内部構造・パスを漏らさない、security.md A09）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE_MARKER.to_string(),
        other => other.to_string(),
    }
}

/// 現在のプラットフォームの `HomeWidget` 実装。
fn implementation() -> Arc<dyn HomeWidget> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn HomeWidget>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match super::apple_home_widget::macos_default() {
                Some(w) => Arc::new(w),
                None => Arc::new(UnsupportedHomeWidget),
            })
            .clone()
    }
    #[cfg(target_os = "android")]
    {
        Arc::new(super::android_home_widget::AndroidHomeWidget)
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        Arc::new(UnsupportedHomeWidget)
    }
}

/// 要求 1 件を別スレッドで処理して完了を送る（テストから実装を差し込めるよう分離）。
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
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::home_widget_show::{shown_text, status_text};
    use std::sync::Mutex;

    fn sender_with_channel() -> (
        CompletionSender<HomeWidgetShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<HomeWidgetShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    #[derive(Default)]
    struct Mem(Mutex<Option<String>>);
    impl HomeWidget for Mem {
        fn set_text(&self, text: &str) -> Result<(), DeviceError> {
            *self.0.lock().expect("lock") = Some(text.to_string());
            Ok(())
        }
        fn current_text(&self) -> Result<Option<String>, DeviceError> {
            Ok(self.0.lock().expect("lock").clone())
        }
    }

    struct Blocking;
    impl HomeWidget for Blocking {
        fn set_text(&self, _: &str) -> Result<(), DeviceError> {
            loop {
                std::thread::park();
            }
        }
        fn current_text(&self) -> Result<Option<String>, DeviceError> {
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
