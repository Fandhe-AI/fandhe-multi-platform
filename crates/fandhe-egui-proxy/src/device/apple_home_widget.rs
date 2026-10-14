//! Issue #1065（macOS）: ホーム画面ウィジェットの Apple 向け実装。`fandhe_device::
//! home_widget::HomeWidget` を、App Group コンテナ内のファイル 1 つ（保存）と
//! ホストアプリ経由の `reloadAllTimelines`（再描画要求）で満たす。`device/home_widget.rs`
//! の macOS 経路から呼ばれる。iOS（#1066）は同じ [`AppGroupHomeWidget`] に自分の
//! 再描画関数を渡して再利用する想定。
//!
//! # 保存
//!
//! `FsStorage`（一時ファイル + rename の原子置換）を App Group コンテナ直下に向けて
//! key [`WIDGET_KEY`] で書く。WidgetKit 拡張（Swift、`platforms/apple/FandheTextWidget`）
//! は同じファイル名 [`WIDGET_FILE_NAME`] を読んで描画する。名前は 2 か所に書くため、
//! テストで実在を確かめてずれを検出する。
//!
//! # 再描画要求
//!
//! `WidgetCenter.reloadAllTimelines` は拡張を含むアプリから呼ぶ必要があるため、小さな
//! ホストアプリ（`FandheWidgetHost`）を `open -n -g -b` で起こして呼ばせる。契約
//! （`fandhe_device::home_widget` doc）上、ウィジェット 0 個・ホストアプリ未導入でも
//! 保存さえできていれば `Ok`（再描画要求は空振りしてよく、反映は OS 側で非同期）。
//! 保存失敗は必ず `Err`（`Internal`）で、成功を装わない。
//!
//! # Team ID
//!
//! App Group ID は `<Team ID>.com.fandhe.widget`。Team ID はリポジトリに書かず、環境変数
//! `FANDHE_DEVELOPMENT_TEAM`（実行時 → ビルド時の順）から取り、`[A-Z0-9]{10}` で検証して
//! からパスへ組む（パストラバーサル防止、security.md A01/A03）。取れなければ呼び出し側が
//! `UnsupportedHomeWidget` へ倒す（ダミー成功へは倒さない）。

use std::sync::Arc;

use fandhe_device::home_widget::{validate_text, HomeWidget};
use fandhe_device::storage::{FsStorage, Storage};
use fandhe_device::{DataDir, DeviceError};

/// 保存先ファイルの key。
pub const WIDGET_KEY: &str = "widget_text";
/// [`WIDGET_KEY`] を `FsStorage` が符号化したファイル名（16 進）。Swift 側の定数と一致させる。
pub const WIDGET_FILE_NAME: &str = "7769646765745f74657874";
/// 再描画要求を担うホストアプリの bundle id（`platforms/apple/FandheWidgetHost` の Info.plist と一致）。
#[cfg(target_os = "macos")]
pub const HOST_BUNDLE_ID: &str = "com.fandhe.widgethost";

/// 再描画要求。成否は保存の成否に影響させない（モジュール doc）。
pub type ReloadFn = Arc<dyn Fn() -> bool + Send + Sync>;

/// App Group コンテナのファイルへ保存する `HomeWidget` 実装。
pub struct AppGroupHomeWidget {
    storage: FsStorage,
    reload: ReloadFn,
}

impl AppGroupHomeWidget {
    /// `dir`（App Group コンテナ）へ保存し、保存後に `reload` を呼ぶ実装を作る。
    pub fn new(dir: std::path::PathBuf, reload: ReloadFn) -> Self {
        Self {
            storage: FsStorage::new(DataDir(dir)),
            reload,
        }
    }
}

/// 保存・読み出しの失敗はパスを漏らさないよう `Internal` へ畳む（security.md A09）。
fn internal<E>(_: E) -> DeviceError {
    DeviceError::Internal("home_widget")
}

impl HomeWidget for AppGroupHomeWidget {
    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        validate_text(text)?;
        self.storage
            .write(WIDGET_KEY, text.as_bytes())
            .map_err(internal)?;
        // 空振り（ウィジェット 0 個・ホスト未導入）は契約上許容。結果は見ない。
        let _ = (self.reload)();
        Ok(())
    }

    fn current_text(&self) -> Result<Option<String>, DeviceError> {
        match self.storage.read(WIDGET_KEY).map_err(internal)? {
            None => Ok(None),
            Some(bytes) => String::from_utf8(bytes).map(Some).map_err(internal),
        }
    }
}

/// Team ID（`[A-Z0-9]{10}`）から App Group ID を作る。形式違いは `None`。
pub fn group_id_from_team(team: &str) -> Option<String> {
    let ok = team.len() == 10
        && team
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    ok.then(|| format!("{team}.com.fandhe.widget"))
}

/// macOS 既定の実装を作る。Team ID・ホームが取れなければ `None`。
#[cfg(target_os = "macos")]
pub fn macos_default() -> Option<AppGroupHomeWidget> {
    let team = std::env::var("FANDHE_DEVELOPMENT_TEAM")
        .ok()
        .or_else(|| option_env!("FANDHE_DEVELOPMENT_TEAM").map(str::to_string))?;
    let group = group_id_from_team(&team)?;
    #[allow(deprecated)]
    let home = std::env::home_dir()?;
    let dir = home.join("Library/Group Containers").join(group);
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

    fn widget(dir: std::path::PathBuf, reload_ok: bool) -> (AppGroupHomeWidget, Arc<AtomicUsize>) {
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
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            w.set_text(&"あ".repeat(41)),
            Err(DeviceError::InvalidInput(_))
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
        assert!(matches!(
            w.set_text("x"),
            Err(DeviceError::Internal("home_widget"))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_file(base);
    }

    #[test]
    fn non_utf8_content_is_internal_not_none() {
        let dir = temp_dir("utf8");
        std::fs::create_dir_all(&dir).expect("作成");
        std::fs::write(dir.join(WIDGET_FILE_NAME), [0xff, 0xfe]).expect("書く");
        let (w, _) = widget(dir.clone(), true);
        assert!(matches!(
            w.current_text(),
            Err(DeviceError::Internal("home_widget"))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn group_id_validates_team() {
        assert_eq!(
            group_id_from_team("ABCDE12345").as_deref(),
            Some("ABCDE12345.com.fandhe.widget")
        );
        for bad in [
            "",
            "abcde12345",
            "ABCDE1234",
            "ABCDE123456",
            "../ABCDEFG",
            "ABCDE/2345",
        ] {
            assert!(group_id_from_team(bad).is_none(), "{bad}");
        }
    }
}
