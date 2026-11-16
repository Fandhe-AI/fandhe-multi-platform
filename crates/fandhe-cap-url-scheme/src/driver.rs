//! Phase 11 の拡充ループ（親 #1116）型サンプル「カスタム URL scheme」（Issue #1215）の
//! effect ドライバ。`fandhe_core::screens::url_scheme_show`（端末依存ゼロ）へ、
//! `crate::contract::UrlScheme` が持つ受信状態を注入する
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`fandhe-cap-gamepad` の `drive` と同型）。
//!
//! # pull 型のポーリング
//!
//! URL は OS から任意のタイミングで push されるが、画面へは本ドライバが
//! [`POLL_INTERVAL`] ごとに受け口（`UrlInbox`）を読んで届ける。`seq` が前回と同じなら
//! 送信を省くが、その分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。非対応 OS は結果が
//! 変わらないため 1 回だけ送り、再描画は予約しない（失敗状態で再描画し続けない）。
//!
//! # 実装のある OS
//!
//! macOS・iOS（`apple` mod、Issue #1215・#1217）、Android（`android` mod、Issue #1218）と Windows
//! （`windows` mod、Issue #1216）。他 OS は
//! [`UnsupportedUrlScheme`] 固定で、画面には「未対応」と出る。
//!
//! # 登録・解除（Windows のみ）
//!
//! Windows は画面のボタン操作でレジストリに scheme を登録・解除する。要求の受け渡しは
//! `background_task::drive_request` と同型で、`take_request` で観測したら先に `mark_in_flight`
//! を送って二重発行を防ぎ、別スレッドで OS を呼んで `finish_action` を送る。

use std::time::Duration;

#[cfg(any(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    test
))]
use crate::contract::UnsupportedUrlScheme;
use crate::contract::UrlScheme;
use crate::error::UrlSchemeError;
#[cfg(any(target_os = "windows", test))]
use fandhe_core::screens::url_scheme_show::{
    Action, ActionFailure, ActionOutcome, finish_action, mark_in_flight, set_registrable,
    take_request,
};
use fandhe_core::screens::url_scheme_show::{
    UrlSchemeShowState, apply_snapshot, apply_unsupported,
};

use fandhe_runtime::CompletionSender;

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 現在の OS の実装（macOS は Apple Event、iOS は delegate、Android は Intent、Windows は起動引数、
/// 他は `Unsupported` 固定）。
pub fn platform_url_scheme() -> &'static dyn UrlScheme {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        &crate::apple::AppleUrlScheme
    }
    #[cfg(target_os = "android")]
    {
        &crate::android::AndroidUrlScheme
    }
    #[cfg(target_os = "windows")]
    {
        &crate::windows::WindowsUrlScheme
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        &UnsupportedUrlScheme
    }
}

/// 登録・解除のドライブ 1 回分。登録が要る OS と画面へ知らせ（一度だけ）、未発送の要求があれば
/// 別スレッドで処理する。
#[cfg(any(target_os = "windows", test))]
fn drive_registration(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
    reg: &'static crate::windows::Registrar,
) {
    if !state.registrable() {
        let _ = sender.send(Box::new(set_registrable));
    }
    let Some(request) = take_request(state) else {
        return;
    };
    // 起動済みマーキング（二重起動防止、`background_task::drive_request` と同型）。
    let _ = sender.send(Box::new(mark_in_flight));
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result = match request.action {
            Action::Register => (reg.register)(),
            Action::Unregister => (reg.unregister)(),
        }
        .map(|()| ActionOutcome::Done)
        .map_err(|e| ActionFailure::Failed(format!("失敗しました: {e}")));
        let _ = sender.send(Box::new(move |s: &mut UrlSchemeShowState| {
            finish_action(s, request.generation, request.action, result);
        }));
    });
}

/// 受け口を 1 回読み、変化があれば画面へ送る。
fn drive(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
    us: &dyn UrlScheme,
) {
    match us.snapshot() {
        Ok(snap) => {
            if state.seq() != Some(snap.seq) {
                let last = snap.last.map(|r| (r.url, r.received_at_unix_ms));
                // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
                // （coding-rust.md: panic を API 契約にしない）。
                let _ = sender.send(Box::new(move |s: &mut UrlSchemeShowState| {
                    apply_snapshot(s, snap.seq, last, snap.rejected);
                }));
            }
            sender.request_repaint_after(POLL_INTERVAL);
        }
        Err(UrlSchemeError::Unsupported) => {
            if !state.is_unsupported() {
                let _ = sender.send(Box::new(apply_unsupported));
            }
        }
        // 受け口の読み取りが他の理由で失敗する実装は現時点に無い。来た場合は成功を
        // 装わず、状態を変えずに次回へ回す。
        Err(_) => sender.request_repaint_after(POLL_INTERVAL),
    }
}

/// `registry.rs` の `effect` 区画から `url_scheme_show` 画面用に配線される effect ドライバ。
/// app-shortcuts の effect は呼ばない（機能 crate 同士を driver で結ばない。最終成果物が
/// `fandhe_cap_app_shortcuts` の effect を続けて呼ぶ）。
pub fn url_scheme_show_effect(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
) {
    #[cfg(target_os = "windows")]
    drive_registration(state, sender, &crate::windows::SYSTEM_REGISTRAR);
    drive(state, sender, platform_url_scheme());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{InboxSnapshot, SCHEME, UrlInbox};

    #[test]
    fn intent_url_passes_scheme_validation() {
        use fandhe_core::screens::url_scheme_show::INTENT_URL;
        assert!(crate::contract::validate_url(INTENT_URL, SCHEME).is_ok());
    }

    /// Swift 側（macOS の App Intents、Issue #1148）が画面側の定数と同じ文字列を持つ。
    #[test]
    fn intent_swift_matches_screen_constants() {
        use fandhe_core::screens::url_scheme_show::{INTENT_TITLE, INTENT_URL};
        let swift = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../platforms/apple/FandheWidgetHost/Sources/FandheWidgetHost/FandheShowIntent.swift"
        ));
        assert!(swift.contains(&format!("\"{INTENT_URL}\"")));
        assert!(swift.contains(&format!("\"{INTENT_TITLE}\"")));
        assert!(swift.contains("AppShortcutsProvider"));
    }

    struct InboxBacked(UrlInbox);
    impl UrlScheme for InboxBacked {
        fn snapshot(&self) -> Result<InboxSnapshot, UrlSchemeError> {
            Ok(self.0.snapshot())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<UrlSchemeShowState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<UrlSchemeShowState>>,
    ) {
        use fandhe_runtime::RepaintWaker;
        struct NoopWaker;
        impl RepaintWaker for NoopWaker {
            fn wake(&self) {}
            fn wake_after(&self, _delay: std::time::Duration) {}
        }
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    #[test]
    fn sends_on_first_read_and_when_seq_changes_only() {
        let us = InboxBacked(UrlInbox::new());
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();

        // 未適用（seq なし）なら空の受け口でも 1 回送る。
        drive(&state, &sender, &us);
        rx.recv().expect("初回は送る")(&mut state);
        assert_eq!(state.seq(), Some(0));

        // seq が同じなら送らない。
        drive(&state, &sender, &us);
        assert!(rx.try_recv().is_err());

        // 受信（弾いたものを含む）で seq が変われば送る。
        us.0.receive_bytes(Some(b"fandhe://a"), SCHEME, 5).unwrap();
        let _ = us.0.receive_bytes(Some(b"https://x"), SCHEME, 6);
        drive(&state, &sender, &us);
        rx.recv().expect("seq が変われば送る")(&mut state);
        assert_eq!(state.seq(), Some(2));
    }

    static REGISTER_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FAKE: crate::windows::Registrar = crate::windows::Registrar {
        register: || {
            REGISTER_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
        unregister: || Err(UrlSchemeError::PermissionDenied("url_scheme")),
    };

    #[test]
    fn registration_runs_once_per_request_and_reports_failure() {
        use fandhe_core::screens::url_scheme_show as show;
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();

        // 初回は registrable を一度送る。
        drive_registration(&state, &sender, &FAKE);
        rx.recv().expect("registrable")(&mut state);
        assert!(state.registrable());
        assert!(rx.try_recv().is_err());

        // 登録要求 1 件につき register は 1 回だけ。
        show::register(&mut state);
        drive_registration(&state, &sender, &FAKE);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert_eq!(REGISTER_CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
        drive_registration(&state, &sender, &FAKE);
        assert!(rx.try_recv().is_err());
        assert_eq!(show::registration_text(&state), "登録しました");

        // 失敗は Err 文言で届き、成功文言は出ない。
        show::unregister(&mut state);
        drive_registration(&state, &sender, &FAKE);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert!(show::error_text(&state).starts_with("失敗しました"));
        assert_eq!(show::registration_text(&state), "");
    }

    #[test]
    fn unsupported_is_sent_once() {
        let (sender, rx) = sender_with_channel();
        let mut state = UrlSchemeShowState::default();

        drive(&state, &sender, &UnsupportedUrlScheme);
        rx.recv().expect("1 回送る")(&mut state);
        assert!(state.is_unsupported());

        drive(&state, &sender, &UnsupportedUrlScheme);
        assert!(rx.try_recv().is_err());
    }
}
