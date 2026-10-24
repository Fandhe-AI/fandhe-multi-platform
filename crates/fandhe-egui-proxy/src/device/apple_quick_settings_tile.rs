//! Issue #719（macOS）: クイック設定タイル・コントロールの Apple 向け実装。
//! `fandhe_device::quick_settings_tile::QuickSettingsTile` を、App Group コンテナ内の回数ファイル
//! 1 つを**読むだけ**で満たす。書くのは WidgetKit 拡張（`platforms/apple/FandheTextWidget` の
//! `FandheCountControl.swift`）の `AppIntent` で、別プロセス。`device/quick_settings_tile.rs` の
//! macOS 経路から呼ばれる。
//!
//! # 保存
//!
//! ファイル名は [`COUNT_FILE_NAME`]（key `COUNT_KEY` を `FsStorage` が符号化した名前）。Swift 側の
//! 定数と 2 か所に書くため、テストで `FsStorage` の実際の名前と照合してずれを検出する。内容の
//! 解釈は `fandhe_device::quick_settings_tile::parse_count`（ASCII 数字のみ、壊れていれば失敗）。
//! ファイルが無いのは拡張が一度も走っていない場合で 0 回。権限拒否・ディレクトリ・壊れた内容は
//! 0 にせず `Err(Internal)`（嘘の成功の禁止）。
//!
//! # 前提
//!
//! - コントロールは macOS 26 以降にしか置けない（互換モードで 16 と報告される場合も 26 扱い）。26 未満・判定不能なら [`macos_default`] は
//!   `None` を返し、呼び出し側が `UnsupportedQuickSettingsTile` へ倒す（0 回と見せない）
//! - Team ID は home-widget と同じ `apple_home_widget::macos_app_group_dir` から取る。取れなければ `None`

use std::path::PathBuf;

use fandhe_device::quick_settings_tile::{parse_count, QuickSettingsTile, TileSnapshot};
use fandhe_device::DeviceError;

/// 回数ファイル名（key `quick_settings_count` の 16 進符号化）。Swift 側の定数と一致させる。
pub const COUNT_FILE_NAME: &str = "717569636b5f73657474696e67735f636f756e74";

/// コントロールを置ける最小の macOS メジャーバージョン。
const MIN_CONTROLS_MAJOR: u32 = 26;

/// macOS 26 が互換モード（古い SDK でのビルド・`SYSTEM_VERSION_COMPAT=1`）で名乗るメジャー。
/// macOS 16 は実在しないため、この値は 26 と同じ OS として扱う。
const COMPAT_TAHOE_MAJOR: u32 = 16;

/// OS のメジャーバージョンがコントロールを置ける範囲か。判定できなければ `false`。
/// 互換モードの 16 は macOS 26 なので対応扱い。
pub fn supports_controls(major: Option<u32>) -> bool {
    matches!(major, Some(m) if m >= MIN_CONTROLS_MAJOR || m == COMPAT_TAHOE_MAJOR)
}

/// `"26.0.1"` 形式の OS バージョン文字列からメジャーを取る。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn major_of(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// App Group コンテナの回数ファイルを読む実装。
pub struct AppGroupQuickSettingsTile {
    dir: PathBuf,
}

impl AppGroupQuickSettingsTile {
    /// `dir`（App Group コンテナ）の回数ファイルを読む実装を作る。
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl QuickSettingsTile for AppGroupQuickSettingsTile {
    fn snapshot(&self) -> Result<TileSnapshot, DeviceError> {
        // パス・OS のエラー文は漏らさず種別だけにする（security.md A09）。
        match std::fs::read(self.dir.join(COUNT_FILE_NAME)) {
            Ok(bytes) => Ok(TileSnapshot {
                count: parse_count(&bytes)?,
                failed_presses: None,
            }),
            // 拡張が一度も走っていない（ディレクトリごと無い場合を含む）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TileSnapshot::default()),
            Err(_) => Err(DeviceError::Internal("quick_settings_tile")),
        }
    }
}

/// macOS 既定の実装を作る。macOS 26 未満・判定不能・Team ID なしは `None`。
#[cfg(target_os = "macos")]
pub fn macos_default() -> Option<AppGroupQuickSettingsTile> {
    let major = super::macos_device_info::implementation()
        .read()
        .ok()
        .and_then(|s| s.os_version)
        .and_then(|v| major_of(&v));
    if !supports_controls(major) {
        return None;
    }
    let dir = super::apple_home_widget::macos_app_group_dir()?;
    Some(AppGroupQuickSettingsTile::new(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::storage::{FsStorage, Storage};
    use fandhe_device::DataDir;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "fandhe-qs-tile-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn put(dir: &PathBuf, bytes: &[u8]) {
        std::fs::create_dir_all(dir).expect("作成");
        std::fs::write(dir.join(COUNT_FILE_NAME), bytes).expect("書く");
    }

    #[test]
    fn missing_file_and_dir_are_zero() {
        let dir = temp_dir("missing");
        assert_eq!(
            AppGroupQuickSettingsTile::new(dir.clone())
                .snapshot()
                .expect("ディレクトリなしは 0")
                .count,
            0
        );
        std::fs::create_dir_all(&dir).expect("作成");
        let snap = AppGroupQuickSettingsTile::new(dir.clone())
            .snapshot()
            .expect("ファイルなしは 0");
        assert_eq!((snap.count, snap.failed_presses), (0, None));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_count() {
        let dir = temp_dir("count");
        put(&dir, b"42");
        let t = AppGroupQuickSettingsTile::new(dir.clone());
        assert_eq!(t.snapshot().expect("読める").count, 42);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_content_is_internal_not_zero() {
        for (tag, bytes) in [
            ("empty", &b""[..]),
            ("text", b"abc"),
            ("newline", b"1\n"),
            ("utf8", &[0xff, 0xfe][..]),
            ("overflow", b"18446744073709551616"),
        ] {
            let dir = temp_dir(tag);
            put(&dir, bytes);
            let r = AppGroupQuickSettingsTile::new(dir.clone()).snapshot();
            assert!(
                matches!(r, Err(DeviceError::Internal("quick_settings_tile"))),
                "{tag}"
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn directory_at_file_path_is_internal() {
        let dir = temp_dir("isdir");
        std::fs::create_dir_all(dir.join(COUNT_FILE_NAME)).expect("作成");
        let r = AppGroupQuickSettingsTile::new(dir.clone()).snapshot();
        assert!(matches!(
            r,
            Err(DeviceError::Internal("quick_settings_tile"))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_is_internal_not_zero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perm");
        put(&dir, b"5");
        let file = dir.join(COUNT_FILE_NAME);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).expect("権限");
        // root 実行では 000 でも読めて前提が崩れるため、その場合は検証できないので戻る。
        if std::fs::read(&file).is_ok() {
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        let r = AppGroupQuickSettingsTile::new(dir.clone()).snapshot();
        assert!(matches!(
            r,
            Err(DeviceError::Internal("quick_settings_tile"))
        ));
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_name_matches_fs_storage_encoding() {
        let dir = temp_dir("name");
        FsStorage::new(DataDir(dir.clone()))
            .write(fandhe_device::quick_settings_tile::COUNT_KEY, b"7")
            .expect("書ける");
        assert!(dir.join(COUNT_FILE_NAME).is_file());
        assert_eq!(
            AppGroupQuickSettingsTile::new(dir.clone())
                .snapshot()
                .expect("読める")
                .count,
            7
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controls_need_macos_26() {
        assert!(!supports_controls(None));
        assert!(!supports_controls(Some(25)));
        assert!(!supports_controls(Some(15)));
        // 互換モードの macOS 26（16 を返す）は対応扱い。
        assert!(supports_controls(Some(16)));
        assert!(supports_controls(Some(26)));
        assert!(supports_controls(Some(27)));
    }

    #[test]
    fn major_parses_version_strings() {
        assert_eq!(major_of("26.0.1"), Some(26));
        assert_eq!(major_of("14.5.0"), Some(14));
        assert_eq!(major_of(""), None);
        assert_eq!(major_of("x.1"), None);
    }
}
