//! Phase 11 の拡充ループ（親 #122）型サンプル「共有の受け取り」（Issue #606）の effect
//! ドライバ。`fandhe_core::screens::share_receive_show`（端末依存ゼロ）へ、
//! `fandhe_device::share_receive::ShareReceive` が持つ受信状態と受け取り方の案内を注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`device/url_scheme.rs::drive` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # pull 型のポーリング
//!
//! テキストは OS から任意のタイミングで push されるが、画面へは本ドライバが
//! [`POLL_INTERVAL`] ごとに受け口を読んで届ける。`seq` が前回と同じなら送信を省くが、その
//! 分岐でも次回のポーリングは必ず予約する（`device-bridge-contracts.md`「4. ポーリング・
//! 再描画」）。非対応 OS は結果が変わらないため 1 回だけ送り、再描画は予約しない。
//!
//! # 実装のある OS
//!
//! macOS（[`MacosShareReceive`](super::macos_share_receive)、Issue #606）と Android
//! （`android_share_receive`、Issue #607）。iOS・Windows は子 issue を切らず
//! `UnsupportedShareReceive` 固定で、画面には「未対応」と出る。

use std::time::Duration;

use fandhe_core::screens::share_receive_show::{
    apply_snapshot, apply_unsupported, set_guide, ShareGuide, ShareReceiveShowState,
};
use fandhe_device::share_receive::ShareReceive;
#[cfg(any(not(any(target_os = "macos", target_os = "android")), test))]
use fandhe_device::share_receive::UnsupportedShareReceive;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 現在の OS の実装（macOS はサービスメニュー、Android は `android_share_receive`、他は `Unsupported` 固定）。
pub(super) fn platform_share_receive() -> &'static dyn ShareReceive {
    #[cfg(target_os = "macos")]
    {
        &super::macos_share_receive::MacosShareReceive
    }
    #[cfg(target_os = "android")]
    {
        super::android_share_receive::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        &UnsupportedShareReceive
    }
}

/// 現在の OS の受け取り方の案内（`platform_share_receive` と同じ cfg 集合）。
fn platform_guide() -> ShareGuide {
    #[cfg(target_os = "macos")]
    {
        ShareGuide::MacServices
    }
    #[cfg(target_os = "android")]
    {
        ShareGuide::AndroidShare
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    {
        ShareGuide::None
    }
}

/// 受け口を 1 回読み、変化があれば画面へ送る。
fn drive(
    state: &ShareReceiveShowState,
    sender: &CompletionSender<ShareReceiveShowState>,
    sr: &dyn ShareReceive,
    guide: ShareGuide,
) {
    match sr.snapshot() {
        Ok(snap) => {
            // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
            // （coding-rust.md: panic を API 契約にしない）。
            if state.guide() != guide {
                let _ = sender.send(Box::new(move |s: &mut ShareReceiveShowState| {
                    set_guide(s, guide);
                }));
            }
            if state.seq() != Some(snap.seq) {
                let last = snap.last.map(|r| (r.text, r.received_at_unix_ms));
                let _ = sender.send(Box::new(move |s: &mut ShareReceiveShowState| {
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

/// `registry.rs` の `effect` 区画から `share_receive_show` 画面用に配線される effect ドライバ。
pub fn share_receive_show_effect(
    state: &ShareReceiveShowState,
    sender: &CompletionSender<ShareReceiveShowState>,
) {
    drive(state, sender, platform_share_receive(), platform_guide());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::share_receive::{ShareInbox, ShareSnapshot};

    struct InboxBacked(ShareInbox);
    impl ShareReceive for InboxBacked {
        fn snapshot(&self) -> Result<ShareSnapshot, DeviceError> {
            Ok(self.0.snapshot())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<ShareReceiveShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<ShareReceiveShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    fn drain(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<ShareReceiveShowState>>,
        state: &mut ShareReceiveShowState,
    ) -> usize {
        let mut n = 0;
        while let Ok(c) = rx.try_recv() {
            c(state);
            n += 1;
        }
        n
    }

    #[test]
    fn sends_on_first_read_then_only_when_seq_changes() {
        let (tx, rx) = sender_with_channel();
        let src = InboxBacked(ShareInbox::new());
        let mut state = ShareReceiveShowState::default();

        drive(&state, &tx, &src, ShareGuide::None);
        assert_eq!(drain(&rx, &mut state), 1);
        assert_eq!(state.seq(), Some(0));

        drive(&state, &tx, &src, ShareGuide::None);
        assert_eq!(drain(&rx, &mut state), 0);

        // 拒否でも seq が変われば送る。
        let _ = src.0.receive_bytes(Some(&[0xff]), 1);
        drive(&state, &tx, &src, ShareGuide::None);
        assert_eq!(drain(&rx, &mut state), 1);
        assert_eq!(state.seq(), Some(1));
    }

    #[test]
    fn guide_is_sent_once() {
        let (tx, rx) = sender_with_channel();
        let src = InboxBacked(ShareInbox::new());
        let mut state = ShareReceiveShowState::default();

        drive(&state, &tx, &src, ShareGuide::MacServices);
        // guide + snapshot
        assert_eq!(drain(&rx, &mut state), 2);
        assert_eq!(state.guide(), ShareGuide::MacServices);

        drive(&state, &tx, &src, ShareGuide::MacServices);
        assert_eq!(drain(&rx, &mut state), 0);
    }

    #[test]
    fn unsupported_is_sent_once() {
        let (tx, rx) = sender_with_channel();
        let mut state = ShareReceiveShowState::default();

        drive(&state, &tx, &UnsupportedShareReceive, ShareGuide::None);
        assert_eq!(drain(&rx, &mut state), 1);
        assert!(state.is_unsupported());

        drive(&state, &tx, &UnsupportedShareReceive, ShareGuide::None);
        assert_eq!(drain(&rx, &mut state), 0);
    }
}
