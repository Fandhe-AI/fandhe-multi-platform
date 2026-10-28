//! Phase 11 の拡充ループ（親 #194、Issue #1019）型サンプル「クラウド保存先として OS のファイル画面に出る」
//! の effect ドライバ。`fandhe_core::screens::file_provider_show`（端末依存ゼロ）の要求を
//! `take_request` で観測し、`fandhe_device::file_provider::FileProvider` をワーカーで呼んで、
//! 操作の結果と操作後に読み直した一覧を `CompletionSender` 経由で 1 件にまとめて注入する
//! （`crate::screens::registry` の `effect` 区画から呼ばれる、`app::EffectFn` 参照）。
//! 契約の SSOT は `fandhe_device::file_provider`（削除キーは再利用しない書類 ID。現行コードは
//! 名前をキーに渡しており、ID への切り替えは #1020）。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform`] の cfg 分岐が全対象 OS の最終形。Android は `android_file_provider::implementation()`
//! を #1020 が埋める。iOS・macOS・Windows は対応表で保留（`DEFERRED`）のため
//! `UnsupportedFileProvider` 固定で、画面に「保留中」と出る。それ以外は「非対応」。
//! 後続はこのファイルの dispatch に触らない。
//!
//! # 同期と再生成
//!
//! 期限・タイマー・ポーリングは持たず、OS コールバックも待たないため `RequestSlot` は使わない。
//! 実行中は `IN_FLIGHT` で二重起動を防ぎ、要求は捨てずに次フレームへ回す。Activity 再生成で
//! 状態が作り直されたときは旧世代の完了が旧 sender で捨てられ、新しい状態は開いたときに読み直す。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fandhe_core::screens::file_provider_show::{
    apply_unsupported, finish, mark_in_flight, take_request, EntryView, FileAction,
    FileProviderShowState, OpResult, SAMPLE_CONTENT, SAMPLE_NAME,
};
#[cfg(not(target_os = "android"))]
use fandhe_device::file_provider::UnsupportedFileProvider;
use fandhe_device::file_provider::{Entry, FileProvider, NOT_FOUND};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 前の操作が実行中のとき、要求を残して次に見直すまでの間隔。
const RETRY_AFTER: Duration = Duration::from_millis(100);

/// 対応表で保留の OS。非対応の結果を「保留中」と「非対応」に分ける。
const DEFERRED: bool = cfg!(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "windows"
));

/// OS 呼び出しが実行中か。
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 現在のプラットフォームに応じた実装を返す。
fn platform() -> Arc<dyn FileProvider> {
    #[cfg(target_os = "android")]
    {
        super::android_file_provider::implementation()
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(UnsupportedFileProvider)
    }
}

/// 操作の結果。`Unsupported` は成功にせず別扱いにする。
enum Done {
    Unsupported,
    Ran {
        op: Option<OpResult>,
        list: Result<Vec<EntryView>, ()>,
    },
}

fn to_view(e: Entry) -> EntryView {
    EntryView {
        name: e.name,
        size: e.size,
        modified_unix_ms: e.modified_unix_ms,
    }
}

/// エラーを固定の種類へ写す。名前・パス・OS の文言は画面にもログにも出さない。
fn put_failure(e: &DeviceError) -> OpResult {
    match e {
        DeviceError::InvalidInput(_) => OpResult::InvalidName,
        _ => OpResult::PutFailed,
    }
}

fn delete_failure(e: &DeviceError) -> OpResult {
    match e {
        DeviceError::InvalidInput(_) => OpResult::InvalidName,
        DeviceError::Internal(NOT_FOUND) => OpResult::AlreadyGone,
        _ => OpResult::DeleteFailed,
    }
}

/// 操作を 1 件実行し、続けて必ず一覧を読み直す（OS を正にする）。
fn run(imp: &dyn FileProvider, action: &FileAction) -> Done {
    let op = match action {
        FileAction::Reload => None,
        FileAction::PutSample => Some(match imp.put(SAMPLE_NAME, SAMPLE_CONTENT.as_bytes()) {
            Ok(o) => OpResult::Placed {
                notified: o.notified,
            },
            Err(DeviceError::Unsupported(_)) => return Done::Unsupported,
            Err(e) => put_failure(&e),
        }),
        FileAction::Delete(name) => Some(match imp.delete(name) {
            Ok(o) => OpResult::Deleted {
                notified: o.notified,
            },
            Err(DeviceError::Unsupported(_)) => return Done::Unsupported,
            Err(e) => delete_failure(&e),
        }),
    };
    match imp.list() {
        Ok(v) => Done::Ran {
            op,
            list: Ok(v.into_iter().map(to_view).collect()),
        },
        Err(DeviceError::Unsupported(_)) => Done::Unsupported,
        Err(_) => Done::Ran { op, list: Err(()) },
    }
}

fn send_done(
    sender: &CompletionSender<FileProviderShowState>,
    generation: u64,
    done: Done,
    deferred: bool,
) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut FileProviderShowState| match done {
        Done::Unsupported => apply_unsupported(s, generation, deferred),
        Done::Ran { op, list } => finish(s, generation, op, list),
    }));
}

/// 要求 1 件をワーカーで処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &FileProviderShowState,
    sender: &CompletionSender<FileProviderShowState>,
    imp: Arc<dyn FileProvider>,
    in_flight: &'static AtomicBool,
    deferred: bool,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    // 前の操作が実行中なら要求は残し、次のフレームで見直す。
    if in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        sender.request_repaint_after(RETRY_AFTER);
        return;
    }
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut FileProviderShowState| {
        mark_in_flight(s, generation);
    }));
    let worker_sender = sender.clone();
    let action = request.action;
    let spawned = std::thread::Builder::new()
        .name("file-provider".to_string())
        .spawn(move || {
            let done = run(imp.as_ref(), &action);
            // 結果を届ける前に実行中を解く。
            in_flight.store(false, Ordering::SeqCst);
            send_done(&worker_sender, generation, done, deferred);
        });
    if spawned.is_err() {
        in_flight.store(false, Ordering::SeqCst);
        send_done(
            sender,
            generation,
            Done::Ran {
                op: None,
                list: Err(()),
            },
            deferred,
        );
    }
}

/// `registry.rs` の `effect` 区画から `file_provider_show` 画面用に配線される effect ドライバ。
pub fn file_provider_show_effect(
    state: &FileProviderShowState,
    sender: &CompletionSender<FileProviderShowState>,
) {
    static IMP: OnceLock<Arc<dyn FileProvider>> = OnceLock::new();
    let imp = IMP.get_or_init(platform).clone();
    drive(state, sender, imp, &IN_FLIGHT, DEFERRED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::file_provider_show::{
        delete_row, error_text, put_sample, status_text, Phase, View, ALREADY_GONE_ERROR,
        DEFERRED_ERROR, EMPTY_STATUS, UNSUPPORTED_ERROR,
    };
    use fandhe_device::file_provider::{DeleteOutcome, PutOutcome};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{mpsc, Mutex};

    /// メモリ上の偽実装。`fail` で各操作を失敗させる。
    #[derive(Default)]
    struct Fake {
        files: Mutex<Vec<String>>,
        put_err: Option<fn() -> DeviceError>,
        list_err: bool,
        calls: AtomicUsize,
    }

    impl FileProvider for Fake {
        fn list(&self) -> Result<Vec<Entry>, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.list_err {
                return Err(DeviceError::Internal("x"));
            }
            Ok(self
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|n| Entry {
                    name: n.clone(),
                    size: 1,
                    modified_unix_ms: None,
                })
                .collect())
        }
        fn put(&self, base: &str, _c: &[u8]) -> Result<PutOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.put_err {
                return Err(e());
            }
            self.files.lock().unwrap().push(base.to_string());
            Ok(PutOutcome {
                name: base.to_string(),
                notified: true,
            })
        }
        fn delete(&self, name: &str) -> Result<DeleteOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut f = self.files.lock().unwrap();
            match f.iter().position(|n| n == name) {
                Some(i) => {
                    f.remove(i);
                    Ok(DeleteOutcome { notified: true })
                }
                None => Err(DeviceError::Internal(NOT_FOUND)),
            }
        }
    }

    fn flag() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    type Rx = mpsc::Receiver<crate::app::Completion<FileProviderShowState>>;

    fn channel() -> (CompletionSender<FileProviderShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 要求を 1 件ドライブし、完了が揃うまで待って適用する。
    fn step(state: &mut FileProviderShowState, imp: Arc<dyn FileProvider>, deferred: bool) {
        let (sender, rx) = channel();
        drive(state, &sender, imp, flag(), deferred);
        for _ in 0..2 {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了");
            c(state);
        }
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Arc::new(Fake::default());
        let s = FileProviderShowState {
            phase: Phase::Idle,
            ..Default::default()
        };
        drive(&s, &sender, fake.clone(), flag(), false);
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn open_lists_then_put_and_delete_reload_list() {
        let fake = Arc::new(Fake::default());
        let mut s = FileProviderShowState::default();
        step(&mut s, fake.clone(), false);
        assert_eq!(status_text(&s), EMPTY_STATUS);
        put_sample(&mut s);
        step(&mut s, fake.clone(), false);
        assert!(matches!(&s.view, View::Listed(v) if v.len() == 1));
        delete_row(&mut s, 0);
        step(&mut s, fake.clone(), false);
        assert!(matches!(&s.view, View::Listed(v) if v.is_empty()));
    }

    #[test]
    fn busy_flag_keeps_request_and_does_not_spawn() {
        let (sender, rx) = channel();
        let fake = Arc::new(Fake::default());
        let busy = flag();
        busy.store(true, Ordering::SeqCst);
        let s = FileProviderShowState::default();
        drive(&s, &sender, fake.clone(), busy, false);
        assert!(rx.try_recv().is_err());
        assert!(take_request(&s).is_some());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unsupported_is_never_success() {
        for (deferred, text) in [(true, DEFERRED_ERROR), (false, UNSUPPORTED_ERROR)] {
            let mut s = FileProviderShowState::default();
            step(&mut s, Arc::new(UnsupportedProvider), deferred);
            assert_eq!(s.view, View::Unsupported { deferred });
            assert_eq!(error_text(&s), text);
            assert_ne!(status_text(&s), EMPTY_STATUS);
        }
    }

    struct UnsupportedProvider;
    impl FileProvider for UnsupportedProvider {
        fn list(&self) -> Result<Vec<Entry>, DeviceError> {
            Err(DeviceError::Unsupported("file_provider"))
        }
        fn put(&self, _: &str, _: &[u8]) -> Result<PutOutcome, DeviceError> {
            Err(DeviceError::Unsupported("file_provider"))
        }
        fn delete(&self, _: &str) -> Result<DeleteOutcome, DeviceError> {
            Err(DeviceError::Unsupported("file_provider"))
        }
    }

    #[test]
    fn error_mapping_covers_all_branches() {
        let inv = DeviceError::InvalidInput("x");
        let int = DeviceError::Internal("x");
        assert_eq!(put_failure(&inv), OpResult::InvalidName);
        assert_eq!(put_failure(&int), OpResult::PutFailed);
        assert_eq!(delete_failure(&inv), OpResult::InvalidName);
        assert_eq!(
            delete_failure(&DeviceError::Internal(NOT_FOUND)),
            OpResult::AlreadyGone
        );
        assert_eq!(delete_failure(&int), OpResult::DeleteFailed);
    }

    #[test]
    fn failures_are_shown_not_swallowed() {
        let fake = Arc::new(Fake {
            put_err: Some(|| DeviceError::Internal("x")),
            list_err: true,
            ..Default::default()
        });
        let mut s = FileProviderShowState::default();
        step(&mut s, fake.clone(), false);
        put_sample(&mut s);
        step(&mut s, fake, false);
        assert_eq!(s.last_op, Some(OpResult::PutFailed));
        assert_eq!(s.view, View::ListFailed);
    }

    #[test]
    fn delete_of_missing_file_is_already_gone() {
        let fake = Arc::new(Fake::default());
        let mut s = FileProviderShowState::default();
        step(&mut s, fake.clone(), false);
        fake.files.lock().unwrap().push("a".into());
        step_reload(&mut s, fake.clone());
        fake.files.lock().unwrap().clear();
        delete_row(&mut s, 0);
        step(&mut s, fake, false);
        assert_eq!(error_text(&s), ALREADY_GONE_ERROR);
    }

    fn step_reload(s: &mut FileProviderShowState, imp: Arc<dyn FileProvider>) {
        fandhe_core::screens::file_provider_show::reload(s);
        step(s, imp, false);
    }

    #[test]
    fn stale_generation_completion_does_not_overwrite() {
        let (sender, rx) = channel();
        let fake = Arc::new(Fake::default());
        let s = FileProviderShowState::default();
        drive(&s, &sender, fake, flag(), false);
        // 完了が届く前に状態が作り直され、別の世代になった想定。
        let mut fresh = FileProviderShowState {
            generation: 7,
            ..Default::default()
        };
        let before = fresh.clone();
        for _ in 0..2 {
            let c = rx.recv_timeout(Duration::from_secs(5)).expect("完了");
            c(&mut fresh);
        }
        assert_eq!(fresh, before);
    }
}
