//! Phase 11（親 #193）型サンプル「最近使った項目への登録」（Issue #988）の effect ドライバ。
//! `fandhe_core::screens::recent_documents_show`（端末依存ゼロ）の要求を観測し、一時フォルダーの
//! サンプルファイルを用意して `ShellPath` で検査してから OS へ渡し、結果を `CompletionSender` 経由で
//! 返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約、`device/open_in_shell.rs` と同型）。
//! 契約と守る範囲は `fandhe_device::recent_documents` のモジュール doc を参照。
//!
//! OS の呼び出しは同期で結果を返す軽い呼び出しなので UI スレッド（macOS は main thread）で同期に呼ぶ。
//! **dispatch はここで最終形にしてあり、後続 issue（Windows は #989）は
//! `windows_recent_documents.rs` だけを埋める**。macOS は `macos_recent_documents.rs`、
//! それ以外は `UnsupportedRecentDocuments` で「非対応」を表示する。

use std::path::Path;

use fandhe_core::screens::recent_documents_show::{
    finish_action, mark_in_flight, take_request, CoreOutcome, RecentAction, RecentDocumentsState,
    RecentFailure, SAMPLE_CONTENT, SAMPLE_DIR, SAMPLE_FILE,
};
#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
use fandhe_device::recent_documents::UnsupportedRecentDocuments;
use fandhe_device::recent_documents::{RecentDocuments, RecentOutcome, ShellPath};
use fandhe_device::DeviceError;

use super::open_in_shell::prepare_sample_file_in;
use crate::app::CompletionSender;

/// `DeviceError` を画面用の結果へ写像する。内部構造・パスは出さず固定の種類だけにする
/// （security.md A09）。
fn to_outcome(result: Result<RecentOutcome, DeviceError>) -> Result<CoreOutcome, RecentFailure> {
    match result {
        Ok(RecentOutcome::Listed) => Ok(CoreOutcome::Listed),
        Ok(RecentOutcome::NotListed) => Ok(CoreOutcome::NotListed),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::Internal("shell sample"))
        | Err(DeviceError::InvalidInput("shell path")) => Err(RecentFailure::SampleUnavailable),
        Err(_) => Err(RecentFailure::Failed),
    }
}

/// 要求 1 件を処理する: サンプルを用意 → パス検査 → OS 呼び出し。検査で失敗したら OS を呼ばない。
fn run_action(
    base: &Path,
    action: RecentAction,
    imp: &dyn RecentDocuments,
) -> Result<RecentOutcome, DeviceError> {
    let file = prepare_sample_file_in(base, SAMPLE_DIR, SAMPLE_FILE, SAMPLE_CONTENT)?;
    let path = ShellPath::new(&file)?;
    match action {
        RecentAction::Register => imp.register(&path),
        RecentAction::Check => imp.check(&path),
    }
}

/// 同期経路: 要求を観測して処理し、結果を画面へ返す。
fn drive(
    state: &RecentDocumentsState,
    sender: &CompletionSender<RecentDocumentsState>,
    base: &Path,
    imp: &dyn RecentDocuments,
) {
    let Some(view) = take_request(state) else {
        return;
    };
    // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
    let _ = sender.send(Box::new(mark_in_flight));
    let result = to_outcome(run_action(base, view.action, imp));
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut RecentDocumentsState| {
        finish_action(s, generation, result);
    }));
}

/// 現在の OS の実装を返す（macOS / Windows / それ以外）。
fn platform_recent_documents() -> Box<dyn RecentDocuments> {
    #[cfg(target_os = "macos")]
    return super::macos_recent_documents::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_recent_documents::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(UnsupportedRecentDocuments);
}

/// `registry.rs` から `recent_documents_show` 画面用に配線される effect ドライバ。
pub fn recent_documents_show_effect(
    state: &RecentDocumentsState,
    sender: &CompletionSender<RecentDocumentsState>,
) {
    if take_request(state).is_none() {
        return;
    }
    drive(
        state,
        sender,
        &std::env::temp_dir(),
        platform_recent_documents().as_ref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::recent_documents_show as screen;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 呼び出し回数を数え、決めた結果を返す偽実装。
    struct Fake {
        calls: AtomicU32,
        result: fn() -> Result<RecentOutcome, DeviceError>,
    }

    impl RecentDocuments for Fake {
        fn register(&self, _p: &ShellPath) -> Result<RecentOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
        fn check(&self, _p: &ShellPath) -> Result<RecentOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<RecentOutcome, DeviceError>) -> Fake {
        Fake {
            calls: AtomicU32::new(0),
            result,
        }
    }

    fn temp_base(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fandhe-rd-drv-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 押下して 1 回ドライバを回し、反映後の状態を返す。
    fn run(action: RecentAction, base: &Path, imp: &dyn RecentDocuments) -> RecentDocumentsState {
        let mut s = RecentDocumentsState::default();
        match action {
            RecentAction::Register => screen::register_sample(&mut s),
            RecentAction::Check => screen::check_listed(&mut s),
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&s, &sender, base, imp);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut s);
        }
        s
    }

    #[test]
    fn to_outcome_covers_all_arms() {
        assert_eq!(
            to_outcome(Ok(RecentOutcome::Listed)),
            Ok(CoreOutcome::Listed)
        );
        assert_eq!(
            to_outcome(Ok(RecentOutcome::NotListed)),
            Ok(CoreOutcome::NotListed)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("recent documents"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("shell sample"))),
            Err(RecentFailure::SampleUnavailable)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("shell path"))),
            Err(RecentFailure::SampleUnavailable)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("recent documents os"))),
            Err(RecentFailure::Failed)
        );
    }

    #[test]
    fn no_request_never_calls_os() {
        let f = fake(|| Ok(RecentOutcome::Listed));
        let base = temp_base("none");
        let s = RecentDocumentsState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&s, &sender, &base, &f);
        assert!(rx.try_recv().is_err());
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn listed_shows_listed_status() {
        let base = temp_base("ok");
        let f = fake(|| Ok(RecentOutcome::Listed));
        let s = run(RecentAction::Register, &base, &f);
        assert_eq!(s.status, screen::LISTED_STATUS);
        assert!(s.error.is_empty());
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn not_listed_is_not_listed_status() {
        let base = temp_base("nl");
        let f = fake(|| Ok(RecentOutcome::NotListed));
        let s = run(RecentAction::Register, &base, &f);
        assert_eq!(s.status, screen::NOT_LISTED_AFTER_REGISTER_STATUS);
        assert!(s.error.is_empty());
        let s = run(RecentAction::Check, &base, &f);
        assert_eq!(s.status, screen::NOT_LISTED_STATUS);
        assert_ne!(s.status, screen::LISTED_STATUS);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unsupported_is_not_success() {
        let base = temp_base("unsup");
        let s = run(RecentAction::Register, &base, &UnsupportedRecentDocuments);
        assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
        assert_ne!(s.status, screen::LISTED_STATUS);
        assert!(s.error.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn internal_failure_uses_fixed_text_without_path() {
        let base = temp_base("int");
        let f = fake(|| Err(DeviceError::Internal("recent documents os")));
        let s = run(RecentAction::Check, &base, &f);
        assert_eq!(s.error, screen::CHECK_FAILED_ERROR);
        assert!(s.status.is_empty());
        assert!(!s.error.contains("recent documents") && !s.error.contains("fandhe-rd"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sample_dir_symlink_never_calls_os() {
        let base = temp_base("dirlink");
        let target = base.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, base.join(SAMPLE_DIR)).unwrap();
        let f = fake(|| Ok(RecentOutcome::Listed));
        let s = run(RecentAction::Register, &base, &f);
        assert_eq!(s.error, screen::SAMPLE_UNAVAILABLE_ERROR);
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        assert!(!target.join(SAMPLE_FILE).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut s = RecentDocumentsState::default();
        screen::register_sample(&mut s);
        mark_in_flight(&mut s);
        let stale = s.generation.wrapping_sub(1);
        finish_action(&mut s, stale, Ok(CoreOutcome::Listed));
        assert!(s.status.is_empty());
    }
}
