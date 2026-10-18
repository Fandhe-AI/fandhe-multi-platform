//! Phase 11 の拡充ループ（親 #100、Issue #390）型サンプル「バックグラウンドダウンロード」の
//! effect ドライバ。`fandhe_core::screens::background_download`（端末依存ゼロ）のボタン要求を観測して
//! `fandhe_device::background_download::BackgroundDownload` を呼び、`CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から呼ばれる契約。`device/background_task.rs` と同型）。
//!
//! # OS 実装の差し込み口
//!
//! [`implementation`] は全対象 OS の最終形。macOS は `apple_background_download`（本 issue）、iOS・Android・
//! Windows は各 `*_background_download.rs` のスタブ（`Unsupported`）が返る。後続の子 issue は自分の OS の
//! モジュールだけを埋め、本ファイルと `device.rs` の `pub mod` 列には触れない。
//!
//! # ポーリング
//!
//! 転送は OS が進めるため、`status()` を毎回読み、前回送った状態と差があるときだけ送る
//! （契約表 4。差が無いのに送ると無限再描画になる）。転送中と要求処理中は、送信を省いた分岐でも
//! `request_repaint_after` で次回を予約する。完了・取り消し・失敗の後は予約しない。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fandhe_core::screens::background_download::{
    apply_snapshot, finish_action, mark_in_flight, take_request, Action, BackgroundDownloadState,
    DownloadView, Phase, UNSUPPORTED,
};
use fandhe_device::background_download::{BackgroundDownload, DownloadStatus};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// ダウンロード元。既定はローカルの確認用サーバー（`serve-brotli.mjs` で
/// `fixtures/http_fetch` を 8787 で配る）。外部 URL は既定値に入れない。ビルド時の
/// `FANDHE_DOWNLOAD_URL` で差し替えられる（`http_fetch` と同じ方式）。信頼の境界は実行時の
/// `check_url`（`start` の冒頭）で守る。
const DOWNLOAD_URL: &str = match option_env!("FANDHE_DOWNLOAD_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8787/items.json",
};

/// 転送中・要求処理中に次回ポーリングを予約する間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `Unsupported` は固定文言へ畳み、それ以外は `Display` だけを渡す（A09）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNSUPPORTED.to_string(),
        other => other.to_string(),
    }
}

fn to_view(status: Result<DownloadStatus, DeviceError>) -> DownloadView {
    match status {
        Ok(DownloadStatus::Idle) => DownloadView::Idle,
        Ok(DownloadStatus::Running { received, total }) => {
            DownloadView::Running { received, total }
        }
        Ok(DownloadStatus::Completed { bytes }) => DownloadView::Completed { bytes },
        Ok(DownloadStatus::Cancelled) => DownloadView::Cancelled,
        Ok(DownloadStatus::Failed(m)) => DownloadView::Failed(m),
        // 非対応は開始時に固定文言で出るので、開いた直後は未観測のままにする。
        Err(DeviceError::Unsupported(_)) => DownloadView::Unknown,
        // 読み出しの失敗は未開始と区別する。
        Err(e) => DownloadView::ReadFailed(to_message(e)),
    }
}

/// 現在の OS 向け実装。初期化に失敗したらダミーの成功ではなく `Unsupported` に倒す。
fn implementation() -> Arc<dyn BackgroundDownload> {
    static INSTANCE: OnceLock<Arc<dyn BackgroundDownload>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                match super::apple_background_download::instance() {
                    Some(d) => d,
                    None => {
                        Arc::new(fandhe_device::background_download::UnsupportedBackgroundDownload)
                    }
                }
            }
            #[cfg(target_os = "ios")]
            {
                super::ios_background_download::implementation()
            }
            #[cfg(target_os = "android")]
            {
                super::android_background_download::implementation()
            }
            #[cfg(target_os = "windows")]
            {
                super::windows_background_download::implementation()
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "android",
                target_os = "windows"
            )))]
            {
                Arc::new(fandhe_device::background_download::UnsupportedBackgroundDownload)
            }
        })
        .clone()
}

/// 要求 1 件を処理して別スレッドで OS を呼び、完了を送る。要求が無ければ `false`。
fn drive_request(
    state: &BackgroundDownloadState,
    sender: &CompletionSender<BackgroundDownloadState>,
    dl: Arc<dyn BackgroundDownload>,
    url: &'static str,
) -> bool {
    let Some(request) = take_request(state) else {
        return false;
    };
    let _ = sender.send(Box::new(mark_in_flight));
    let generation = request.generation;
    let action = request.action;
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result = match action {
            Action::Start => dl.start(url),
            Action::Cancel => dl.cancel(),
        }
        .map_err(to_message);
        let _ = sender.send(Box::new(move |s: &mut BackgroundDownloadState| {
            finish_action(s, generation, result);
        }));
    });
    true
}

/// 列挙待ちか。モジュールが存在する macOS（とテスト）でのみ Apple 側の判定を参照し、他 OS は常に false。
#[cfg(any(target_os = "macos", test))]
fn enumeration_pending() -> bool {
    super::apple_background_download::enumeration_pending()
}

/// 列挙待ちか（Apple 側モジュールが無い OS では常に false）。
#[cfg(not(any(target_os = "macos", test)))]
fn enumeration_pending() -> bool {
    false
}

/// ポーリング 1 回分。差があるときだけ送り、転送中・処理中は次回を予約する。
fn drive_poll(
    state: &BackgroundDownloadState,
    sender: &CompletionSender<BackgroundDownloadState>,
    dl: &dyn BackgroundDownload,
) {
    let view = to_view(dl.status());
    // macOS は起動時の再接続（非同期）が終わるまで続ける。OS コールバックから再描画を起こせないため。
    let keep_polling = matches!(view, DownloadView::Running { .. })
        || state.phase != Phase::Idle
        || enumeration_pending();
    if view != state.view {
        let _ = sender.send(Box::new(move |s: &mut BackgroundDownloadState| {
            apply_snapshot(s, view);
        }));
    }
    if keep_polling {
        sender.request_repaint_after(POLL_INTERVAL);
    }
}

/// `registry.rs` の `effect` 区画から配線される effect ドライバ。
pub fn background_download_effect(
    state: &BackgroundDownloadState,
    sender: &CompletionSender<BackgroundDownloadState>,
) {
    let dl = implementation();
    if drive_request(state, sender, dl.clone(), DOWNLOAD_URL) {
        // 次フレームで進行を観測する。
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    drive_poll(state, sender, dl.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::background_download::{cancel, start};
    use fandhe_device::http::check_url;
    use std::sync::Mutex;

    fn sender_with_channel() -> (
        CompletionSender<BackgroundDownloadState>,
        std::sync::mpsc::Receiver<crate::app::Completion<BackgroundDownloadState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 結果を固定で返すモック。`unsupported` のとき 3 メソッドとも `Unsupported`。
    #[derive(Default)]
    struct Mock {
        unsupported: bool,
        status: Mutex<Option<DownloadStatus>>,
    }

    impl BackgroundDownload for Mock {
        fn start(&self, _url: &str) -> Result<(), DeviceError> {
            if self.unsupported {
                Err(DeviceError::Unsupported("background download"))
            } else {
                Ok(())
            }
        }
        fn cancel(&self) -> Result<(), DeviceError> {
            self.start("")
        }
        fn status(&self) -> Result<DownloadStatus, DeviceError> {
            self.status
                .lock()
                .expect("lock")
                .clone()
                .ok_or(DeviceError::Internal("background download status"))
        }
    }

    #[test]
    fn default_url_passes_check_url() {
        assert!(check_url(DOWNLOAD_URL).is_ok());
    }

    #[test]
    fn drive_request_is_noop_without_request() {
        let (sender, rx) = sender_with_channel();
        let state = BackgroundDownloadState::default();
        assert!(!drive_request(
            &state,
            &sender,
            Arc::new(Mock::default()),
            "u"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_start_becomes_fixed_error_not_success() {
        let (sender, rx) = sender_with_channel();
        let mut state = BackgroundDownloadState::default();
        start(&mut state);
        let dl = Arc::new(Mock {
            unsupported: true,
            ..Default::default()
        });
        assert!(drive_request(&state, &sender, dl, "u"));
        rx.recv().expect("in flight")(&mut state);
        rx.recv().expect("finish")(&mut state);
        assert_eq!(state.error, format!("失敗しました: {UNSUPPORTED}"));
        assert_eq!(state.phase, Phase::Idle);
    }

    #[test]
    fn cancel_success_clears_error() {
        let (sender, rx) = sender_with_channel();
        let mut state = BackgroundDownloadState::default();
        cancel(&mut state);
        assert!(drive_request(
            &state,
            &sender,
            Arc::new(Mock::default()),
            "u"
        ));
        rx.recv().expect("in flight")(&mut state);
        rx.recv().expect("finish")(&mut state);
        assert_eq!(state.error, "");
    }

    #[test]
    fn status_error_is_read_failed_not_idle() {
        assert!(matches!(
            to_view(Err(DeviceError::Internal("x"))),
            DownloadView::ReadFailed(_)
        ));
        assert_eq!(
            to_view(Err(DeviceError::Unsupported("x"))),
            DownloadView::Unknown
        );
    }

    #[test]
    fn poll_sends_only_on_change() {
        let (sender, rx) = sender_with_channel();
        let dl = Mock {
            status: Mutex::new(Some(DownloadStatus::Completed { bytes: 40 })),
            ..Default::default()
        };
        let mut state = BackgroundDownloadState::default();
        drive_poll(&state, &sender, &dl);
        rx.recv().expect("差分あり")(&mut state);
        assert_eq!(state.view, DownloadView::Completed { bytes: 40 });
        drive_poll(&state, &sender, &dl);
        assert!(rx.try_recv().is_err());
    }
}
