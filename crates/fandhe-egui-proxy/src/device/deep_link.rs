//! Phase 11 の拡充ループ（親 #153）型サンプル「ディープリンク」（Issue #1278）の
//! effect ドライバ。`fandhe_core::screens::deep_link_show`（端末依存ゼロ）へ、
//! `fandhe_device::deep_link::DeepLink` が持つ受信状態を注入する
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`device/url_scheme.rs::drive` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # pull 型のポーリング
//!
//! URL は OS から任意のタイミングで push されるが、画面へは本ドライバが
//! [`POLL_INTERVAL`] ごとに受け口を読んで届ける。`seq` が前回と同じなら送信を省くが、
//! その分岐でも次回のポーリングは必ず予約する（`device-bridge-contracts.md`「4.」）。
//! 非対応 OS は結果が変わらないため 1 回だけ送り、再描画は予約しない。
//!
//! # 実装のある OS
//!
//! 分岐は Android だけ（`android_deep_link`。static の受信箱と JNI の受け口を持つ、
//! Issue #1279）。iOS・macOS は対応表で保留中のため fallback の
//! [`UnsupportedDeepLink`] に入る。保留が外れたときは、その再着手の (a) で分岐を足す。

use std::time::Duration;

use fandhe_core::screens::deep_link_show::{apply_snapshot, apply_unsupported, DeepLinkShowState};
use fandhe_device::deep_link::DeepLink;
#[cfg(any(not(target_os = "android"), test))]
use fandhe_device::deep_link::UnsupportedDeepLink;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 現在の OS の実装（Android のみ分岐、他は `Unsupported` 固定）。
pub(super) fn platform_deep_link() -> &'static dyn DeepLink {
    #[cfg(target_os = "android")]
    {
        super::android_deep_link::implementation()
    }
    #[cfg(not(target_os = "android"))]
    {
        &UnsupportedDeepLink
    }
}

/// 受け口を 1 回読み、変化があれば画面へ送る。
fn drive(
    state: &DeepLinkShowState,
    sender: &CompletionSender<DeepLinkShowState>,
    dl: &dyn DeepLink,
) {
    match dl.snapshot() {
        Ok(snap) => {
            if state.seq() != Some(snap.seq) {
                let last = snap.last.map(|r| (r.url, r.received_at_unix_ms));
                // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
                // （coding-rust.md: panic を API 契約にしない）。
                let _ = sender.send(Box::new(move |s: &mut DeepLinkShowState| {
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

/// `registry.rs` の `effect` 区画から `deep_link_show` 画面用に配線される effect ドライバ。
pub fn deep_link_show_effect(
    state: &DeepLinkShowState,
    sender: &CompletionSender<DeepLinkShowState>,
) {
    drive(state, sender, platform_deep_link());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::deep_link::receive;
    use fandhe_device::url_scheme::{InboxSnapshot, UrlInbox};

    struct InboxBacked(UrlInbox);
    impl DeepLink for InboxBacked {
        fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
            Ok(self.0.snapshot())
        }
    }

    struct Failing;
    impl DeepLink for Failing {
        fn snapshot(&self) -> Result<InboxSnapshot, DeviceError> {
            Err(DeviceError::Internal("x"))
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<DeepLinkShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<DeepLinkShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (crate::app::egui_sender(tx, egui::Context::default()), rx)
    }

    #[test]
    fn sends_on_first_read_and_when_seq_changes_only() {
        let dl = InboxBacked(UrlInbox::new());
        let (sender, rx) = sender_with_channel();
        let mut state = DeepLinkShowState::default();

        drive(&state, &sender, &dl);
        rx.recv().expect("初回は送る")(&mut state);
        assert_eq!(state.seq(), Some(0));

        drive(&state, &sender, &dl);
        assert!(rx.try_recv().is_err());

        receive(&dl.0, Some(b"https://example.com/a"), "example.com", 5).unwrap();
        let _ = receive(&dl.0, Some(b"https://example.org/"), "example.com", 6);
        drive(&state, &sender, &dl);
        rx.recv().expect("seq が変われば送る")(&mut state);
        assert_eq!(state.seq(), Some(2));
    }

    #[test]
    fn unsupported_is_sent_once() {
        let (sender, rx) = sender_with_channel();
        let mut state = DeepLinkShowState::default();

        drive(&state, &sender, &UnsupportedDeepLink);
        rx.recv().expect("1 回送る")(&mut state);
        assert!(state.is_unsupported());

        drive(&state, &sender, &UnsupportedDeepLink);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn other_errors_never_send_success() {
        let (sender, rx) = sender_with_channel();
        drive(&DeepLinkShowState::default(), &sender, &Failing);
        assert!(rx.try_recv().is_err());
    }
}
