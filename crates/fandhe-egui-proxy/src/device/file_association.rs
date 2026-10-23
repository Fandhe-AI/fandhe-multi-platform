//! Phase 11 の拡充ループ（親 #102、Issue #416）型サンプル「ファイルの関連付け」の
//! effect ドライバ。`fandhe_core::screens::file_association_show`（端末依存ゼロ）へ、
//! `fandhe_device::file_association::FileAssociation` が持つ受信状態を注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`device/url_scheme.rs` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # pull 型のポーリング
//!
//! ファイルは OS から任意のタイミングで push されるが、画面へは本ドライバが
//! [`POLL_INTERVAL`] ごとに受け口（`FileInbox`）を読んで届ける。`seq` が前回と同じなら
//! 送信を省くが、その分岐でも次回のポーリングは必ず予約する
//! （`device-bridge-contracts.md`「4. ポーリング・再描画」）。非対応 OS は結果が
//! 変わらないため 1 回だけ送り、再描画は予約しない。
//!
//! # 実装のある OS
//!
//! [`platform`] が全 OS の最終形。macOS は `apple_file_association`（Issue #416）、iOS・Windows は
//! 各 `*_file_association.rs` の実装、Android は `android_file_association.rs` のスタブ
//! （`Unsupported`）が返る。
//!
//! # 登録・解除（Windows のみ）
//!
//! `url_scheme` と同じ形。`take_request` で観測したら先に `mark_in_flight` を送って二重発行を
//! 防ぎ、別スレッドで OS を呼んで `finish_action` を送る。

use std::time::Duration;

use fandhe_core::screens::file_association_show::{
    apply_snapshot, apply_unsupported, FileAssociationShowState, OpenFailureKind,
};
#[cfg(any(target_os = "windows", test))]
use fandhe_core::screens::file_association_show::{
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
use fandhe_device::file_association::UnsupportedFileAssociation;
use fandhe_device::file_association::{FileAssociation, OpenFailure};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 受け口を読み直す間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 現在の OS の実装。振り分けは全 OS の最終形で、後続の子 issue は触らない。
fn platform() -> &'static dyn FileAssociation {
    #[cfg(target_os = "macos")]
    {
        &super::apple_file_association::AppleFileAssociation
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_file_association::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_file_association::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        super::windows_file_association::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        &UnsupportedFileAssociation
    }
}

/// 登録・解除のドライブ 1 回分。登録が要る OS と画面へ知らせ（一度だけ）、未発送の要求があれば
/// 別スレッドで処理する。
#[cfg(any(target_os = "windows", test))]
fn drive_registration(
    state: &FileAssociationShowState,
    sender: &CompletionSender<FileAssociationShowState>,
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
        let _ = sender.send(Box::new(move |s: &mut FileAssociationShowState| {
            finish_action(s, request.generation, request.action, result);
        }));
    });
}

fn failure_kind(f: OpenFailure) -> OpenFailureKind {
    match f {
        OpenFailure::TooLarge => OpenFailureKind::TooLarge,
        OpenFailure::Unreadable => OpenFailureKind::Unreadable,
        OpenFailure::Folder => OpenFailureKind::Folder,
    }
}

/// 受け口を 1 回読み、変化があれば画面へ送る。
fn drive(
    state: &FileAssociationShowState,
    sender: &CompletionSender<FileAssociationShowState>,
    fa: &dyn FileAssociation,
) {
    match fa.snapshot() {
        Ok(snap) => {
            if state.seq() != Some(snap.seq) {
                let latest = snap.latest.map(|r| match r {
                    Ok(f) => Ok((f.name, f.size, f.head, f.received_at_unix_ms)),
                    Err(e) => Err(failure_kind(e)),
                });
                // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
                // （coding-rust.md: panic を API 契約にしない）。
                let _ = sender.send(Box::new(move |s: &mut FileAssociationShowState| {
                    apply_snapshot(s, snap.seq, latest, snap.rejected);
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

/// `registry.rs` の `effect` 区画から `file_association_show` 画面用に配線される effect ドライバ。
pub fn file_association_show_effect(
    state: &FileAssociationShowState,
    sender: &CompletionSender<FileAssociationShowState>,
) {
    #[cfg(target_os = "windows")]
    if let Some(reg) = super::windows_file_association::registrar() {
        drive_registration(state, sender, reg);
    }
    drive(state, sender, platform());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::file_association::{FileInbox, FileSnapshot};

    struct InboxBacked(FileInbox);
    impl FileAssociation for InboxBacked {
        fn snapshot(&self) -> Result<FileSnapshot, DeviceError> {
            Ok(self.0.snapshot())
        }
    }

    fn sender_with_channel() -> (
        CompletionSender<FileAssociationShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<FileAssociationShowState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    #[test]
    fn sends_on_first_read_and_when_seq_changes_only() {
        let fa = InboxBacked(FileInbox::new());
        let (sender, rx) = sender_with_channel();
        let mut state = FileAssociationShowState::default();

        drive(&state, &sender, &fa);
        rx.recv().expect("初回は送る")(&mut state);
        assert_eq!(state.seq(), Some(0));

        drive(&state, &sender, &fa);
        assert!(rx.try_recv().is_err());

        // 拒否も seq を進めるので送る。
        fa.0.reject();
        drive(&state, &sender, &fa);
        rx.recv().expect("seq が変われば送る")(&mut state);
        assert_eq!(state.seq(), Some(1));
    }

    static REGISTER_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FAKE: crate::device::windows_url_scheme::Registrar =
        crate::device::windows_url_scheme::Registrar {
            register: || {
                REGISTER_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            unregister: || Err(DeviceError::PermissionDenied("file_association")),
        };

    #[test]
    fn registration_runs_once_per_request_and_reports_failure() {
        use fandhe_core::screens::file_association_show as show;
        let (sender, rx) = sender_with_channel();
        let mut state = FileAssociationShowState::default();

        drive_registration(&state, &sender, &FAKE);
        rx.recv().expect("registrable")(&mut state);
        assert!(state.registrable());
        assert!(rx.try_recv().is_err());

        show::register(&mut state);
        drive_registration(&state, &sender, &FAKE);
        rx.recv().expect("起動済みマーク")(&mut state);
        rx.recv().expect("完了")(&mut state);
        assert_eq!(REGISTER_CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
        drive_registration(&state, &sender, &FAKE);
        assert!(rx.try_recv().is_err());
        assert_eq!(show::registration_text(&state), "登録しました");

        // 権限拒否は失敗文言で届き、成功文言は出ない。
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
        let mut state = FileAssociationShowState::default();

        drive(&state, &sender, &UnsupportedFileAssociation);
        rx.recv().expect("1 回送る")(&mut state);
        assert!(state.is_unsupported());

        drive(&state, &sender, &UnsupportedFileAssociation);
        assert!(rx.try_recv().is_err());
    }
}
