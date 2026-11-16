//! ノート保存の型サンプル画面（`fandhe_core::screens::storage_note`、端末依存ゼロ）と、`fandhe-device` の
//! `Storage` を橋渡しする effect ドライバの置き場（Issue #748 以前の Phase 8、Issue #1332 で機能 crate へ移した）。
//! `fandhe-gallery` の `screens/registry.rs` の `init "storage_note"` 行から [`storage_note_state`] が呼ばれ、
//! 構築時に 1 度だけ実行される。`fandhe-core` は `fandhe-device` に依存しないため、両者の型を橋渡しするのは
//! 本モジュールの責務（依存方向は機能 crate → コアの一方向）。

use fandhe_core::screens::storage_note::{NoteStorage, NoteStorageError, StorageNoteState};
use fandhe_device::{DeviceError, storage::Storage};

use crate::contract::FsStorage;

/// `fandhe_device::DeviceError` を `fandhe_core::screens::storage_note::NoteStorageError` へ変換する。
/// `DeviceError` の内部構造（`std::io::Error` 等）は `fandhe-core` 側へ漏らさず、`Display` の文言のみを転送する
/// （security.md A09 準拠）。
fn to_note_storage_error(e: DeviceError) -> NoteStorageError {
    match e {
        // `DeviceError::Io` の `Display` は既に「入出力エラー: 」を前置するため、
        // ここでは内側の `std::io::Error` の `Display` のみを転送する
        // （`NoteStorageError::Io` 側の `#[error("入出力エラー: {0}")]` と二重にしない）。
        DeviceError::Io(io_err) => NoteStorageError::Io(io_err.to_string()),
        DeviceError::InvalidKey(key) => NoteStorageError::InvalidKey(key),
        DeviceError::Unsupported(s) => NoteStorageError::Unsupported(s.to_string()),
        DeviceError::Http(_)
        | DeviceError::InvalidImageSize(_, _)
        | DeviceError::PermissionDenied(_)
        | DeviceError::Cancelled(_)
        | DeviceError::Internal(_)
        | DeviceError::InvalidInput(_)
        | DeviceError::Disabled(_) => NoteStorageError::Other(e.to_string()),
    }
}

/// `fandhe_device::storage::Storage` 実装を `fandhe_core::screens::storage_note::NoteStorage`
/// へ橋渡しする newtype。エラーは [`to_note_storage_error`] で型付きのまま変換する
/// （`.claude/rules/coding-rust.md`「ライブラリクレート: thiserror 等による型付きエラー」準拠）。
pub struct DeviceNoteStorage(pub Box<dyn Storage>);

impl NoteStorage for DeviceNoteStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
        self.0.read(key).map_err(to_note_storage_error)
    }

    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), NoteStorageError> {
        self.0.write(key, bytes).map_err(to_note_storage_error)
    }
}

/// 任意の `Storage` 実装から `StorageNoteState` を構築する（テスト・#750/#751 が
/// Android/iOS 向け `DataDir` 注入で差し替える入口）。
pub fn storage_note_state_with(storage: Box<dyn Storage>) -> StorageNoteState {
    StorageNoteState::with_storage(Box::new(DeviceNoteStorage(storage)))
}

/// レジストリの `init "storage_note"` 行から呼ばれる状態ファクトリ（`fn() -> StorageNoteState`）。
///
/// 保存先は [`crate::data_dir`]（デスクトップは `$HOME/.fandhe`、Android は `getFilesDir()` 相当、iOS は
/// アプリコンテナの `Documents/`）。解決できなければ状態の `status` に失敗を出す（ダミーの保存先へは倒さない）。
pub fn storage_note_state() -> StorageNoteState {
    match crate::data_dir() {
        Ok(dir) => storage_note_state_with(Box::new(FsStorage::new(dir))),
        Err(e) => {
            StorageNoteState::default().with_status(format!("保存先の解決に失敗しました: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::DataDir;
    use std::sync::{Arc, Mutex};

    /// `fandhe_device::storage::Storage` のインメモリ実装。`Storage` トレイト側で
    /// 「保存 → 別状態インスタンスで復元」を満たすことを検証する（受入 (b) を
    /// `Storage` 経由でも担保する。`fandhe-core` 側の `NoteStorage` 直接実装での検証は
    /// `crates/fandhe-core/tests/storage_note.rs` を参照）。
    #[derive(Clone, Default)]
    struct MemStorage(Arc<Mutex<Option<Vec<u8>>>>);

    impl Storage for MemStorage {
        fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Ok(self.0.lock().expect("lock は成功する").clone())
        }

        fn write(&self, _key: &str, bytes: &[u8]) -> Result<(), DeviceError> {
            *self.0.lock().expect("lock は成功する") = Some(bytes.to_vec());
            Ok(())
        }
    }

    /// 不正な key を要求して `DeviceError::InvalidKey` を返す `Storage`
    /// （エラー文字列化の転送経路を検証する）。
    struct RejectingStorage;

    impl Storage for RejectingStorage {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }

        fn write(&self, key: &str, _bytes: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }
    }

    #[test]
    fn memory_storage_round_trips_through_screen() {
        use fandhe_core::click_button;
        use fandhe_core::screens::storage_note::{set_input, storage_note_screen};

        let mem = MemStorage::default();
        let screen = storage_note_screen();
        let mut state = storage_note_state_with(Box::new(mem.clone()));
        set_input(&mut state, "メモ".to_string());
        assert!(click_button(&screen, "保存", &mut state));

        let restored = storage_note_state_with(Box::new(mem));
        assert_eq!(restored.notes, vec!["メモ".to_string()]);
    }

    #[test]
    fn fs_storage_survives_state_recreation() {
        let dir = std::env::temp_dir().join(format!(
            "fandhe-cap-local-storage-kv-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("システム時刻は UNIX epoch 以降")
                .as_nanos()
        ));

        let first = storage_note_state_with(Box::new(FsStorage::new(DataDir(dir.clone()))));
        assert!(first.notes.is_empty());

        use fandhe_core::screens::storage_note::{NOTES_KEY, encode_notes};
        FsStorage::new(DataDir(dir.clone()))
            .write(NOTES_KEY, &encode_notes(&["復元されるメモ".to_string()]))
            .expect("write は成功する");

        // 新しい `FsStorage` インスタンス + 新しい状態でプロセス再起動を模す。
        let restored = storage_note_state_with(Box::new(FsStorage::new(DataDir(dir))));
        assert_eq!(restored.notes, vec!["復元されるメモ".to_string()]);
    }

    #[test]
    fn device_error_message_is_forwarded() {
        let bridged = DeviceNoteStorage(Box::new(RejectingStorage));
        let err = bridged.read("../x").unwrap_err();
        assert!(err.to_string().contains("../x"));
    }

    /// PR #787 Bugbot 指摘: `DeviceError::Io` の `Display` は既に「入出力エラー: 」を
    /// 前置するため、`to_note_storage_error` がそれをそのまま `NoteStorageError::Io` へ
    /// 格納すると「入出力エラー: 入出力エラー: ...」と二重表示になっていた。
    #[test]
    fn io_error_prefix_is_not_duplicated() {
        let io_err = std::io::Error::other("disk full");
        let err = to_note_storage_error(DeviceError::Io(io_err));
        let msg = err.to_string();
        assert_eq!(msg.matches("入出力エラー").count(), 1, "got: {msg}");
    }
}
