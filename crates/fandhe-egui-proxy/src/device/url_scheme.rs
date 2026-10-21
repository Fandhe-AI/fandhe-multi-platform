//! Phase 11 の拡充ループ（親 #1116）型サンプル「カスタム URL scheme」（Issue #1215）の
//! effect ドライバ。`fandhe_core::screens::url_scheme_show`（端末依存ゼロ）へ、
//! `fandhe_device::url_scheme::UrlScheme` が持つ受信状態を注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`device/gamepad.rs::drive` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
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
//! macOS・iOS（[`AppleUrlScheme`](super::apple_url_scheme)、Issue #1215・#1217）、Android
//! （`android_url_scheme::AndroidUrlScheme`、Issue #1218）と Windows
//! （[`WindowsUrlScheme`](super::windows_url_scheme)、Issue #1216）。他 OS は
//! [`UnsupportedUrlScheme`] 固定で、画面には「未対応」と出る。
//!
//! # 登録・解除（Windows のみ）
//!
//! Windows は画面のボタン操作でレジストリに scheme を登録・解除する。要求の受け渡しは
//! `background_task::drive_request` と同型で、`take_request` で観測したら先に `mark_in_flight`
//! を送って二重発行を防ぎ、別スレッドで OS を呼んで `finish_action` を送る。

use std::time::Duration;

use fandhe_core::screens::url_scheme_show::{
    apply_snapshot, apply_unsupported, UrlSchemeShowState,
};
#[cfg(any(target_os = "windows", test))]
use fandhe_core::screens::url_scheme_show::{
    finish_action, mark_in_flight, set_registrable, take_request, Action, ActionFailure,
    ActionOutcome,
};
#[cfg(any(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    test
))]
use fandhe_device::url_scheme::UnsupportedUrlScheme;
use fandhe_device::url_scheme::UrlScheme;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 現在の OS の実装（macOS は Apple Event、iOS は delegate、Android は Intent、Windows は起動引数、
/// 他は `Unsupported` 固定）。
pub(super) fn platform_url_scheme() -> &'static dyn UrlScheme {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        &super::apple_url_scheme::AppleUrlScheme
    }
    #[cfg(target_os = "android")]
    {
        &super::android_url_scheme::AndroidUrlScheme
    }
    #[cfg(target_os = "windows")]
    {
        &super::windows_url_scheme::WindowsUrlScheme
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
    reg: &'static super::windows_url_scheme::Registrar,
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
        Err(DeviceError::Unsupported(_)) => {
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
pub fn url_scheme_show_effect(
    state: &UrlSchemeShowState,
    sender: &CompletionSender<UrlSchemeShowState>,
) {
    #[cfg(target_os = "windows")]
    drive_registration(state, sender, &super::windows_url_scheme::SYSTEM_REGISTRAR);
    drive(state, sender, platform_url_scheme());
    super::app_shortcuts::drive_app_shortcuts(state, sender);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::url_scheme::{InboxSnapshot, UrlInbox, SCHEME};

    struct InboxBacked(UrlInbox);
    impl UrlScheme for InboxBacked {
        fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
            Ok(self.0.snapshot())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<UrlSchemeShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<UrlSchemeShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
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
    static FAKE: crate::device::windows_url_scheme::Registrar =
        crate::device::windows_url_scheme::Registrar {
            register: || {
                REGISTER_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            unregister: || Err(DeviceError::PermissionDenied("url_scheme")),
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
