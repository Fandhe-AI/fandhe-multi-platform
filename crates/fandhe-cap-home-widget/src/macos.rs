//! Issue #1065（macOS）: ホーム画面ウィジェットの Apple 向け実装（機能 crate への移設は #1174）。
//! [`crate::HomeWidget`] を、App Group コンテナ内のファイル 1 つ（保存）と
//! ホストアプリ経由の `reloadAllTimelines`（再描画要求）で満たす。`lib.rs` の macOS 経路から
//! 呼ばれる。iOS（#1212）は同じ [`AppGroupHomeWidget`] に自分の再描画関数を渡して再利用する想定。
//!
//! # 保存
//!
//! 一時ファイル + rename の原子置換で App Group コンテナ直下の [`WIDGET_FILE_NAME`] へ書く
//! （一時ファイルは専用サブディレクトリ [`TMP_SUBDIR`] に置き、失敗時は消す）。WidgetKit 拡張
//! （Swift、`crates/fandhe-cap-home-widget/apple/widget`）は同じファイル名を読んで描画する。
//! 名前は 2 か所に書くため、テストで実在を確かめてずれを検出する。
//!
//! # 再描画要求
//!
//! `WidgetCenter.reloadAllTimelines` は拡張を含むアプリから呼ぶ必要があるため、小さな
//! ホストアプリ（`FandheWidgetHost`）を `open -n -g -b` で起こして呼ばせる。契約
//! （`crate` の doc）上、ウィジェット 0 個・ホストアプリ未導入でも
//! 保存さえできていれば `Ok`（再描画要求は空振りしてよく、反映は OS 側で非同期）。
//! 保存失敗は必ず `Err`（`Internal`）で、成功を装わない。
//!
//! # Team ID
//!
//! App Group ID は `<Team ID>.com.fandhe.widget`。Team ID はリポジトリに書かず、環境変数
//! `FANDHE_DEVELOPMENT_TEAM`（実行時 → ビルド時の順）から取り、`[A-Z0-9]{10}` で検証して
//! からパスへ組む（`fandhe_platform_apple::macos_app_group_dir`。quick-settings-tile と共有、
//! パストラバーサル防止、security.md A01/A03）。取れなければ呼び出し側が
//! `UnsupportedHomeWidget` へ倒す（ダミー成功へは倒さない）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{HomeWidget, HomeWidgetError, validate_text};

/// 保存先ファイル名（`widget_text` を 16 進符号化した名前）。Swift 側の定数と一致させる。
/// 旧実装（`FsStorage`）の保存済みファイルと互換にするため値は変えない。
pub const WIDGET_FILE_NAME: &str = "7769646765745f74657874";
/// 一時ファイルを保存先の名前空間から分離するサブディレクトリ名。
const TMP_SUBDIR: &str = ".fandhe-tmp";
/// 再描画要求を担うホストアプリの bundle id（`platforms/apple/FandheWidgetHost` の Info.plist と一致）。
#[cfg(target_os = "macos")]
pub const HOST_BUNDLE_ID: &str = "com.fandhe.widgethost";

/// 一時ファイル名の一意性を確保する呼び出しごとの連番（プロセス ID と組み合わせる）。
static TMP_NONCE: AtomicU64 = AtomicU64::new(0);

/// 再描画要求。成否は保存の成否に影響させない（モジュール doc）。
pub type ReloadFn = Arc<dyn Fn() -> bool + Send + Sync>;

/// App Group コンテナのファイルへ保存する `HomeWidget` 実装。
pub struct AppGroupHomeWidget {
    dir: PathBuf,
    reload: ReloadFn,
}

impl AppGroupHomeWidget {
    /// `dir`（App Group コンテナ）へ保存し、保存後に `reload` を呼ぶ実装を作る。
    pub fn new(dir: PathBuf, reload: ReloadFn) -> Self {
        Self { dir, reload }
    }
}

/// 一時ファイル + rename で `bytes` を `dir/WIDGET_FILE_NAME` へ原子的に置き換える。
/// 失敗した経路では一時ファイルを消す（削除自体の失敗は元のエラーを優先して無視）。
fn write_atomic(dir: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_dir = dir.join(TMP_SUBDIR);
    std::fs::create_dir_all(&tmp_dir)?;
    let nonce = TMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp = tmp_dir.join(format!("{}.{}.tmp", std::process::id(), nonce));
    let result = std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, dir.join(WIDGET_FILE_NAME)));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 保存・読み出しの失敗はパスを漏らさないよう `Internal` へ畳む（security.md A09）。
fn internal<E>(_: E) -> HomeWidgetError {
    HomeWidgetError::Internal
}

impl HomeWidget for AppGroupHomeWidget {
    fn set_text(&self, text: &str) -> Result<(), HomeWidgetError> {
        validate_text(text)?;
        write_atomic(&self.dir, text.as_bytes()).map_err(internal)?;
        // 空振り（ウィジェット 0 個・ホスト未導入）は契約上許容。結果は見ない。
        let _ = (self.reload)();
        Ok(())
    }

    fn current_text(&self) -> Result<Option<String>, HomeWidgetError> {
        match std::fs::read(self.dir.join(WIDGET_FILE_NAME)) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(internal),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(internal(e)),
        }
    }
}

/// macOS 既定の実装を作る。Team ID・ホームが取れなければ `None`。
#[cfg(target_os = "macos")]
pub fn macos_default() -> Option<AppGroupHomeWidget> {
    let dir = fandhe_platform_apple::macos_app_group_dir("widget")?;
    Some(AppGroupHomeWidget::new(dir, Arc::new(reload_via_host_app)))
}

/// ホストアプリを前面に出さず新規起動し、`reloadAllTimelines` を呼ばせる。
/// `-n`: 起動中だと前面化だけで reload が走らないため。引数は固定値のみ（A03）。
#[cfg(target_os = "macos")]
fn reload_via_host_app() -> bool {
    use std::process::{Command, Stdio};
    Command::new("/usr/bin/open")
        .args(["-n", "-g", "-b", HOST_BUNDLE_ID])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "fandhe-home-widget-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn widget(dir: PathBuf, reload_ok: bool) -> (AppGroupHomeWidget, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let w = AppGroupHomeWidget::new(
            dir,
            Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                reload_ok
            }),
        );
        (w, calls)
    }

    #[test]
    fn invalid_input_is_rejected_without_write_or_reload() {
        let dir = temp_dir("invalid");
        let (w, calls) = widget(dir.clone(), true);
        assert!(matches!(
            w.set_text("a\nb"),
            Err(HomeWidgetError::InvalidInput(_))
        ));
        assert!(matches!(
            w.set_text(&"あ".repeat(41)),
            Err(HomeWidgetError::InvalidInput(_))
        ));
        assert!(!dir.exists());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unsaved_is_none() {
        let (w, _) = widget(temp_dir("none"), true);
        assert!(matches!(w.current_text(), Ok(None)));
    }

    #[test]
    fn round_trip_keeps_empty_and_surrounding_spaces_and_file_name() {
        let dir = temp_dir("rt");
        let (w, calls) = widget(dir.clone(), true);
        for t in ["", "  hi  ", "会議中"] {
            w.set_text(t).expect("保存できる");
            assert_eq!(w.current_text().expect("読める").as_deref(), Some(t));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(dir.join(WIDGET_FILE_NAME).is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reload_failure_still_ok() {
        let dir = temp_dir("reload");
        let (w, _) = widget(dir.clone(), false);
        assert!(w.set_text("x").is_ok());
        assert_eq!(w.current_text().expect("読める").as_deref(), Some("x"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_failure_is_internal_and_skips_reload() {
        let base = temp_dir("blocked");
        std::fs::write(&base, b"file").expect("通常ファイルを作る");
        let (w, calls) = widget(base.join("sub"), true);
        assert!(matches!(w.set_text("x"), Err(HomeWidgetError::Internal)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_file(base);
    }

    #[test]
    fn non_utf8_content_is_internal_not_none() {
        let dir = temp_dir("utf8");
        std::fs::create_dir_all(&dir).expect("作成");
        std::fs::write(dir.join(WIDGET_FILE_NAME), [0xff, 0xfe]).expect("書く");
        let (w, _) = widget(dir.clone(), true);
        assert!(matches!(w.current_text(), Err(HomeWidgetError::Internal)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn successful_write_leaves_no_temp_file() {
        let dir = temp_dir("tmpclean");
        let (w, _) = widget(dir.clone(), true);
        w.set_text("a").expect("保存できる");
        let left = std::fs::read_dir(dir.join(TMP_SUBDIR))
            .expect("tmp dir")
            .count();
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_rename_removes_temp_file() {
        let dir = temp_dir("rename");
        // 保存先名をディレクトリにして rename を失敗させる。
        std::fs::create_dir_all(dir.join(WIDGET_FILE_NAME)).expect("作成");
        let (w, calls) = widget(dir.clone(), true);
        assert!(matches!(w.set_text("x"), Err(HomeWidgetError::Internal)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let left = std::fs::read_dir(dir.join(TMP_SUBDIR))
            .expect("tmp dir")
            .count();
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
