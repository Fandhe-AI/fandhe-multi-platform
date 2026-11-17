//! `fandhe_core::screens::share_receive_show` の effect ドライバ（share-receive 機能 crate の `driver`）。
//!
//! 毎フレーム受け口（[`ShareReceive::snapshot`]）を読み、変化があれば `CompletionSender` 経由で画面へ送る。
//! 呼び出し元は `fandhe-gallery` の `screens/registry.rs` の `effect` 区画。画面の状態型
//! （`ShareGuide` を含む）を参照してよいのは本モジュールだけ（契約・OS 実装は `crate` 直下・`android` / `macos`。
//! `consumer-app-structure.md` §8.1）。

use std::time::Duration;

use fandhe_core::screens::share_receive_show::{
    ShareGuide, ShareReceiveShowState, apply_snapshot, apply_unsupported, set_guide,
};
use fandhe_runtime::CompletionSender;

use crate::{ShareReceive, ShareReceiveError, platform_share_receive};

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

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
            // 受信側（描画ループ）がドロップ済みの送信失敗は無視してよい
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
        Err(ShareReceiveError::Unsupported) => {
            if !state.is_unsupported() {
                let _ = sender.send(Box::new(apply_unsupported));
            }
        }
        // 受け口の読み取りが他の理由で失敗する実装は現時点に無い。来た場合は成功を
        // 装わず、状態を変えずに次回へ回す。
        Err(_) => sender.request_repaint_after(POLL_INTERVAL),
    }
}

/// `fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `share_receive_show` 画面用に
/// 配線される effect ドライバ（`fn(&S, &CompletionSender<S>)` の契約、`url_scheme` の `drive` と同型）。
pub fn share_receive_show_effect(
    state: &ShareReceiveShowState,
    sender: &CompletionSender<ShareReceiveShowState>,
) {
    drive(state, sender, platform_share_receive(), platform_guide());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ShareInbox, ShareSnapshot, UnsupportedShareReceive};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, channel};

    use fandhe_runtime::{Completion, RepaintWaker};

    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    struct InboxBacked(ShareInbox);
    impl ShareReceive for InboxBacked {
        fn snapshot(&self) -> Result<ShareSnapshot, ShareReceiveError> {
            Ok(self.0.snapshot())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<ShareReceiveShowState>,
        Receiver<Completion<ShareReceiveShowState>>,
    ) {
        let (tx, rx) = channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn drain(
        rx: &Receiver<Completion<ShareReceiveShowState>>,
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
