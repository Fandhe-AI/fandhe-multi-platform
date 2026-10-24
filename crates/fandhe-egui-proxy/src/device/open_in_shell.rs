//! Phase 11（親 #146）型サンプル「既定アプリで開く・ファイルマネージャーで表示」（Issue #749）の
//! effect ドライバ。`fandhe_core::screens::open_in_shell`（端末依存ゼロ）の要求を観測し、一時
//! フォルダーのサンプルファイルを用意して `fandhe_device::open_in_shell::ShellPath` で検査してから
//! OS へ渡し、結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から
//! 呼ばれる契約、`device/compose_mail_sms.rs` の同期経路と同型）。契約と守る範囲は
//! `fandhe_device::open_in_shell` のモジュール doc を参照。
//!
//! OS の呼び出しは Bool を同期で返す軽い呼び出しなので UI スレッドで同期に呼ぶ。
//! **dispatch はここで最終形にしてあり、後続 issue（Windows は #750）は OS 別モジュール
//! （`windows_open_in_shell.rs`）だけを埋める**。macOS は `macos_open_in_shell.rs`、
//! それ以外は `UnsupportedOpenInShell` で「非対応」を表示する。

use std::path::{Path, PathBuf};

use fandhe_core::screens::open_in_shell::{
    apply_platform, finish_action, mark_in_flight, take_request, CoreOutcome, OpenInShellState,
    Platform, ShellAction, ShellFailure, SAMPLE_CONTENT, SAMPLE_DIR, SAMPLE_FILE,
};
#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
use fandhe_device::open_in_shell::UnsupportedOpenInShell;
use fandhe_device::open_in_shell::{OpenInShell, ShellOutcome, ShellPath};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 画面へ注入する OS 区分。
fn platform_kind() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Unknown
    }
}

/// `DeviceError` を画面用の結果へ写像する。内部構造・パスは出さず固定の種類だけにする
/// （security.md A09）。
fn to_outcome(result: Result<ShellOutcome, DeviceError>) -> Result<CoreOutcome, ShellFailure> {
    match result {
        Ok(ShellOutcome::HandedOff) => Ok(CoreOutcome::HandedOff),
        Ok(ShellOutcome::NoHandler) => Ok(CoreOutcome::NoHandler),
        Err(DeviceError::Unsupported(_)) => Ok(CoreOutcome::Unsupported),
        Err(DeviceError::InvalidInput("shell path")) => Err(ShellFailure::InvalidPath),
        Err(DeviceError::InvalidInput("shell type")) => Err(ShellFailure::RejectedType),
        Err(DeviceError::Internal("shell sample")) => Err(ShellFailure::SampleUnavailable),
        Err(_) => Err(ShellFailure::Failed),
    }
}

/// サンプルファイルを `base/<SAMPLE_DIR>/<SAMPLE_FILE>` に用意して返す。
///
/// 既にある通常のファイルはそのまま使い、無ければ作り直す（削除後の再押下に対応）。位置に
/// シンボリックリンク・ディレクトリなど通常のファイル以外があれば、たどらず書かずに失敗する
/// （他のファイルを上書きしない。security.md A08）。
fn prepare_sample_file(base: &Path) -> Result<PathBuf, DeviceError> {
    let fail = || DeviceError::Internal("shell sample");
    let dir = base.join(SAMPLE_DIR);
    // 配置先ディレクトリ自体がシンボリックリンクなら、リンク先へ書かないよう拒否する。
    // 作成直後にも再検査し、確認から作成までに差し替えられたものも弾く。
    let is_real_dir = |p: &Path| {
        std::fs::symlink_metadata(p)
            .map(|m| m.file_type().is_dir())
            .map_err(|e| e.kind())
    };
    match is_real_dir(&dir) {
        Ok(true) => {}
        Ok(false) => return Err(fail()),
        Err(std::io::ErrorKind::NotFound) => {
            std::fs::create_dir_all(&dir).map_err(|_| fail())?;
            if !matches!(is_real_dir(&dir), Ok(true)) {
                return Err(fail());
            }
        }
        Err(_) => return Err(fail()),
    }
    let file = dir.join(SAMPLE_FILE);
    match std::fs::symlink_metadata(&file) {
        Ok(meta) if meta.file_type().is_file() => Ok(file),
        Ok(_) => Err(fail()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // create_new で、確認から書き込みまでの間に現れたものを上書きしない。
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|_| fail())?;
            f.write_all(SAMPLE_CONTENT.as_bytes()).map_err(|_| fail())?;
            Ok(file)
        }
        Err(_) => Err(fail()),
    }
}

/// 要求 1 件を処理する: サンプルを用意 → パス検査 →（開くなら型検査）→ OS 呼び出し。
/// 検査で失敗したら OS を呼ばない。
fn run_action(
    base: &Path,
    action: ShellAction,
    imp: &dyn OpenInShell,
) -> Result<ShellOutcome, DeviceError> {
    let file = prepare_sample_file(base)?;
    let path = ShellPath::new(&file)?;
    match action {
        ShellAction::Open => {
            path.check_openable()?;
            imp.open(&path)
        }
        ShellAction::Reveal => imp.reveal(&path),
    }
}

/// 同期経路: 要求を観測して処理し、結果を画面へ返す。
fn drive(
    state: &OpenInShellState,
    sender: &CompletionSender<OpenInShellState>,
    base: &Path,
    imp: &dyn OpenInShell,
) {
    let kind = platform_kind();
    if kind != Platform::Unknown && state.platform == Platform::Unknown {
        // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい。
        let _ = sender.send(Box::new(move |s: &mut OpenInShellState| {
            apply_platform(s, kind);
        }));
    }
    let Some(view) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_in_flight));
    let result = to_outcome(run_action(base, view.action, imp));
    let generation = view.generation;
    let _ = sender.send(Box::new(move |s: &mut OpenInShellState| {
        finish_action(s, generation, result);
    }));
}

/// 現在の OS の実装を返す（macOS / Windows / それ以外）。
fn platform_open_in_shell() -> Box<dyn OpenInShell> {
    #[cfg(target_os = "macos")]
    return super::macos_open_in_shell::implementation();
    #[cfg(target_os = "windows")]
    return super::windows_open_in_shell::implementation();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(UnsupportedOpenInShell);
}

/// `registry.rs` から `open_in_shell` 画面用に配線される effect ドライバ。
pub fn open_in_shell_effect(state: &OpenInShellState, sender: &CompletionSender<OpenInShellState>) {
    let needs_platform =
        platform_kind() != Platform::Unknown && state.platform == Platform::Unknown;
    if take_request(state).is_none() && !needs_platform {
        return;
    }
    drive(
        state,
        sender,
        &std::env::temp_dir(),
        platform_open_in_shell().as_ref(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::open_in_shell as screen;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 呼び出し回数を数え、決めた結果を返す偽実装。
    struct Fake {
        calls: AtomicU32,
        result: fn() -> Result<ShellOutcome, DeviceError>,
    }

    impl OpenInShell for Fake {
        fn open(&self, _p: &ShellPath) -> Result<ShellOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
        fn reveal(&self, _p: &ShellPath) -> Result<ShellOutcome, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.result)()
        }
    }

    fn fake(result: fn() -> Result<ShellOutcome, DeviceError>) -> Fake {
        Fake {
            calls: AtomicU32::new(0),
            result,
        }
    }

    fn temp_base(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fandhe-ois-drv-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 押下して 1 回ドライバを回し、反映後の状態を返す。
    fn run(action: ShellAction, base: &Path, imp: &dyn OpenInShell) -> OpenInShellState {
        let mut s = OpenInShellState::default();
        match action {
            ShellAction::Open => screen::open_default(&mut s),
            ShellAction::Reveal => screen::reveal_in_manager(&mut s),
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
            to_outcome(Ok(ShellOutcome::HandedOff)),
            Ok(CoreOutcome::HandedOff)
        );
        assert_eq!(
            to_outcome(Ok(ShellOutcome::NoHandler)),
            Ok(CoreOutcome::NoHandler)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("open in shell"))),
            Ok(CoreOutcome::Unsupported)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("shell path"))),
            Err(ShellFailure::InvalidPath)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::InvalidInput("shell type"))),
            Err(ShellFailure::RejectedType)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("shell sample"))),
            Err(ShellFailure::SampleUnavailable)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Internal("shell os"))),
            Err(ShellFailure::Failed)
        );
    }

    #[test]
    fn no_request_never_calls_os() {
        let f = fake(|| Ok(ShellOutcome::HandedOff));
        let base = temp_base("none");
        let s = OpenInShellState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&s, &sender, &base, &f);
        // 届くのは OS 区分の注入だけ（要求が無いので完了は届かない）。
        let mut applied = OpenInShellState::default();
        while let Ok(apply) = rx.try_recv() {
            apply(&mut applied);
        }
        assert_eq!(applied.phase, screen::ShellPhase::Idle);
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn handed_off_shows_handed_off_status() {
        let base = temp_base("ok");
        let f = fake(|| Ok(ShellOutcome::HandedOff));
        let s = run(ShellAction::Open, &base, &f);
        assert_eq!(s.status, screen::HANDED_OFF_STATUS);
        assert!(s.error.is_empty());
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let s = run(ShellAction::Reveal, &base, &f);
        assert_eq!(s.status, screen::HANDED_OFF_STATUS);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_handler_is_not_handed_off() {
        let base = temp_base("nh");
        let f = fake(|| Ok(ShellOutcome::NoHandler));
        let s = run(ShellAction::Open, &base, &f);
        assert_eq!(s.error, screen::NO_HANDLER_ERROR);
        assert_ne!(s.status, screen::HANDED_OFF_STATUS);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unsupported_is_not_success() {
        let base = temp_base("unsup");
        let s = run(ShellAction::Open, &base, &UnsupportedOpenInShell);
        assert_eq!(s.status, screen::UNSUPPORTED_STATUS);
        assert_ne!(s.status, screen::HANDED_OFF_STATUS);
        assert!(s.error.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn internal_failure_uses_fixed_text_without_path() {
        let base = temp_base("int");
        let f = fake(|| Err(DeviceError::Internal("shell os")));
        let s = run(ShellAction::Open, &base, &f);
        assert_eq!(s.error, screen::OS_FAILED_ERROR);
        assert!(!s.error.contains("shell") && !s.error.contains("fandhe-ois"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn rejected_type_never_calls_os_for_open() {
        use std::os::unix::fs::PermissionsExt;
        let base = temp_base("exec");
        let file = prepare_sample_file(&base).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = fake(|| Ok(ShellOutcome::HandedOff));
        let s = run(ShellAction::Open, &base, &f);
        assert_eq!(s.error, screen::REJECTED_TYPE_ERROR);
        assert_eq!(f.calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let mut s = OpenInShellState::default();
        screen::open_default(&mut s);
        mark_in_flight(&mut s);
        let stale = s.generation.wrapping_sub(1);
        finish_action(&mut s, stale, Ok(CoreOutcome::HandedOff));
        assert!(s.status.is_empty());
    }

    #[test]
    fn platform_is_injected_once() {
        let base = temp_base("plat");
        let f = fake(|| Ok(ShellOutcome::HandedOff));
        let s = OpenInShellState::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&s, &sender, &base, &f);
        let mut applied = OpenInShellState::default();
        while let Ok(apply) = rx.try_recv() {
            apply(&mut applied);
        }
        assert_eq!(applied.platform, platform_kind());
        // 注入済みなら再送しない。
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, egui::Context::default());
        drive(&applied, &sender, &base, &f);
        assert!(rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sample_file_is_recreated_after_deletion() {
        let base = temp_base("recreate");
        let file = prepare_sample_file(&base).unwrap();
        std::fs::remove_file(&file).unwrap();
        let again = prepare_sample_file(&base).unwrap();
        assert_eq!(std::fs::read_to_string(again).unwrap(), SAMPLE_CONTENT);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sample_dir_symlink_is_rejected_without_writing() {
        let base = temp_base("dirlink");
        let target = base.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, base.join(SAMPLE_DIR)).unwrap();
        assert!(matches!(
            prepare_sample_file(&base),
            Err(DeviceError::Internal("shell sample"))
        ));
        assert!(!target.join(SAMPLE_FILE).exists());
        // リンク先に通常ファイルがあっても OS へ渡さない。
        std::fs::write(target.join(SAMPLE_FILE), b"x").unwrap();
        assert!(prepare_sample_file(&base).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sample_location_with_symlink_or_directory_fails_without_writing() {
        let base = temp_base("symlink");
        let dir = base.join(SAMPLE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let victim = base.join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(SAMPLE_FILE)).unwrap();
        assert!(matches!(
            prepare_sample_file(&base),
            Err(DeviceError::Internal("shell sample"))
        ));
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        std::fs::remove_file(dir.join(SAMPLE_FILE)).unwrap();
        std::fs::create_dir(dir.join(SAMPLE_FILE)).unwrap();
        assert!(prepare_sample_file(&base).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
