//! ホーム画面ウィジェットに表示する 1 行テキストの機能 crate（REQ-21 / TASK-21.3、Issue #1174）。
//!
//! 契約（[`HomeWidget`]・[`HomeWidgetError`]・[`validate_text`]・[`UnsupportedHomeWidget`]）、
//! Android 実装（JNI 経由で `MainActivity.homeWidgetSetText` など）、macOS 実装（App Group コンテナ +
//! WidgetKit 拡張）、画面の状態機械（`fandhe_core::screens::home_widget_show`）へ橋渡しする effect
//! ドライバ（[`home_widget_show_effect`]）、機能宣言（`Cargo.toml` の `[package.metadata.fandhe]`）、
//! Android の Java・res（`android/`）、macOS 拡張の Swift（`apple/widget/`）を 1 つに集める。
//! iOS は有料登録が前提のため対象外（#1212）。Windows は package identity 必須のため対象外（#1208）。
//!
//! 依存方向は本 crate → `fandhe-core` / `fandhe-runtime` / `fandhe-platform-*` の一方向で、
//! egui・`fandhe-egui-proxy`・`fandhe-device` には依存しない。呼び出し元は画面カタログ
//! （`fandhe-gallery` の `screens/registry.rs` の `effect "home_widget_show"` 行）で、
//! `ProxyApp::update` のフレーム末尾から毎フレーム [`home_widget_show_effect`] が呼ばれる。
//!
//! 扱う範囲は「アプリが保存した 1 行のテキストを、ホーム画面（macOS は通知センター /
//! デスクトップ）のウィジェットに表示する」ことだけ。対象は Android / macOS。
//! その他の OS は [`UnsupportedHomeWidget`]（`Unsupported`）。複数ウィジェット・画像・操作（タップ）・
//! 定期更新は扱わない。
//!
//! 共有保存先は Apple が App Group コンテナのファイル、Android が `SharedPreferences`。ウィジェット側
//! （Swift / Java）はそこを読んで描画する。
//!
//! # 契約
//!
//! - **`set_text`**: 共有保存先へ書けたら `Ok(())`。保存後にウィジェットの再描画要求
//!   （`reloadAllTimelines` / `AppWidgetManager` の更新通知）を出すが、これはベスト
//!   エフォートで、要求の失敗・空振りは `Ok` のまま（`Ok` が約束するのは「保存できた」
//!   ことで、ウィジェットへの反映ではない。反映は OS 側で非同期・予算制限があり、
//!   要求が失敗した場合は次に再描画要求が通るまで古い表示のままになりうる＝既知の制約）。保存に失敗したら `Err`
//!   （`Internal`）で、成功を装わない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **入力検証**: 1 行・[`MAX_TEXT_CHARS`] 文字以内（[`validate_text`]）。改行
//!   （`\n` / `\r` / NEL などの制御文字と U+2028 / U+2029）を含む・長すぎる場合は `Err(InvalidInput)`。黙って切り詰めない。
//!   前後の空白は入力どおり保存する（trim しない、同「8. 秘密値」）。空文字は有効な
//!   入力で、「表示を空にする」ことを意味する
//! - **`current_text`**: 保存済みの値を読む。未保存は `Ok(None)`（空文字を番兵にしない）。
//!   読み出しに失敗したら `Err`（未保存と混同しない）
//! - **ウィジェット 0 個でも保存は成功**: ユーザーがウィジェットを置いていなくても
//!   `set_text` は成功し、`current_text` で読める（再描画要求は空振りしてよい）
//! - **対象外 OS**: 全メソッドが `Err(Unsupported)`。初期化に失敗した実装も
//!   [`UnsupportedHomeWidget`] へ倒す（ダミーの成功実装へは倒さない）
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 プロセス内の呼び出し順での保存結果と、失敗を成功と見せないこと。
//! 既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）として、
//! **ウィジェットへの実際の反映は OS 側で非同期**であり保証しない（Apple の reload 予算・
//! Android の更新タイミングで遅れる / 間引かれる）。`Ok` は「保存した」
//! までの意味で、ウィジェットが今その文字を表示していることではない。複数プロセス
//! （ウィジェット拡張プロセスを含む）が同時に保存先を触ったときの食い違いは扱わない。

// unsafe は持たない。JNI の attach と Activity 取得は `fandhe-platform-android::with_activity` に閉じる。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;

use std::sync::Arc;

use fandhe_core::screens::home_widget_show::{
    HomeWidgetRequest, HomeWidgetShowState, UNAVAILABLE_MARKER, finish_load, finish_set,
    mark_dispatched, take_request,
};
use fandhe_runtime::CompletionSender;

/// ホーム画面ウィジェットの失敗種別。保存値・パスを含めない固定の種別だけを持つ（`security.md` A09）。
/// 画面に出る文言は従来の `HomeWidgetError` と同じ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HomeWidgetError {
    /// 現在のプラットフォームではホーム画面ウィジェットを提供できない（ドライバが画面側の固定文言へ写す）。
    #[error("このプラットフォームでは未対応です: home_widget")]
    Unsupported,
    /// 入力が不正（改行・制御文字・文字数超過）。
    #[error("入力が不正です: {0}")]
    InvalidInput(&'static str),
    /// 保存・読み出し・JNI の失敗。
    #[error("内部エラーが発生しました: home_widget")]
    Internal,
}

/// 保存できるテキストの最大文字数（`char` 数）。ウィジェットの 1 行表示の目安。
pub const MAX_TEXT_CHARS: usize = 40;

/// ホーム画面ウィジェットの契約（モジュール doc の「契約」節）。
pub trait HomeWidget: Send + Sync {
    /// 表示テキストを保存して再描画を要求する。入力は [`validate_text`] で検証済みの
    /// ものだけを保存する。
    fn set_text(&self, text: &str) -> Result<(), HomeWidgetError>;
    /// 保存済みのテキストを返す。未保存は `Ok(None)`。
    fn current_text(&self) -> Result<Option<String>, HomeWidgetError>;
}

/// 対象外 OS（Windows・Web・Linux）・初期化失敗時のフォールバック。
pub struct UnsupportedHomeWidget;

impl HomeWidget for UnsupportedHomeWidget {
    fn set_text(&self, _text: &str) -> Result<(), HomeWidgetError> {
        Err(HomeWidgetError::Unsupported)
    }

    fn current_text(&self) -> Result<Option<String>, HomeWidgetError> {
        Err(HomeWidgetError::Unsupported)
    }
}

/// 入力が 1 行かつ [`MAX_TEXT_CHARS`] 文字以内かを検証する（各 OS 実装が保存前に呼ぶ）。
pub fn validate_text(text: &str) -> Result<(), HomeWidgetError> {
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(HomeWidgetError::InvalidInput(
            "home_widget text has newline",
        ));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(HomeWidgetError::InvalidInput("home_widget text too long"));
    }
    Ok(())
}

/// `Unsupported` は画面側が判定する固定文言へ、それ以外は `Display` のみ転送する
/// （内部構造・パスを漏らさない、security.md A09）。
fn to_message(e: HomeWidgetError) -> String {
    match e {
        HomeWidgetError::Unsupported => UNAVAILABLE_MARKER.to_string(),
        other => other.to_string(),
    }
}

/// 現在のプラットフォームの `HomeWidget` 実装。macOS は Team ID が取れなければ
/// `UnsupportedHomeWidget`（iOS は #1212）。
fn implementation() -> Arc<dyn HomeWidget> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn HomeWidget>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match macos::macos_default() {
                Some(w) => Arc::new(w),
                None => Arc::new(UnsupportedHomeWidget),
            })
            .clone()
    }
    #[cfg(target_os = "android")]
    {
        Arc::new(android::AndroidHomeWidget)
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        Arc::new(UnsupportedHomeWidget)
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
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn error_display_keeps_screen_wording() {
        assert_eq!(
            HomeWidgetError::InvalidInput("x").to_string(),
            "入力が不正です: x"
        );
        assert_eq!(
            HomeWidgetError::Internal.to_string(),
            "内部エラーが発生しました: home_widget"
        );
    }

    #[test]
    fn accepts_empty_and_keeps_surrounding_spaces_valid() {
        assert!(validate_text("").is_ok());
        assert!(validate_text("  hi  ").is_ok());
    }

    #[test]
    fn rejects_newline_and_too_long() {
        assert!(matches!(
            validate_text("a\nb"),
            Err(HomeWidgetError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_text("a\rb"),
            Err(HomeWidgetError::InvalidInput(_))
        ));
        for c in ['\u{85}', '\u{2028}', '\u{2029}'] {
            assert!(matches!(
                validate_text(&format!("a{c}b")),
                Err(HomeWidgetError::InvalidInput(_))
            ));
        }
        assert!(validate_text(&"あ".repeat(MAX_TEXT_CHARS)).is_ok());
        assert!(matches!(
            validate_text(&"あ".repeat(MAX_TEXT_CHARS + 1)),
            Err(HomeWidgetError::InvalidInput(_))
        ));
    }

    #[test]
    fn unsupported_is_error_for_both_methods() {
        assert!(matches!(
            UnsupportedHomeWidget.set_text("x"),
            Err(HomeWidgetError::Unsupported)
        ));
        assert!(matches!(
            UnsupportedHomeWidget.current_text(),
            Err(HomeWidgetError::Unsupported)
        ));
    }
}
