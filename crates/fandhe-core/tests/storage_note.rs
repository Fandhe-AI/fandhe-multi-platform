//! `screens::storage_note`（Phase 8 型サンプル「メモ保存」、Issue #747）のヘッドレス
//! 結合テスト。別クレートとしてコンパイルされるため、公開 API
//! （`click_button` / `render_headless` / `screens::storage_note::*`）のみを使う。
//!
//! `NoteStorage` のインメモリ実装で「保存 → 新しい状態で復元」を検証し、
//! 端末実装（`fandhe-egui-proxy` の `FsStorage` 橋渡し）に依らず永続化の意味論
//! （最新が先頭で復元される）を機械的に固定化する（受入 (b)）。

use fandhe_core::screens::storage_note::{
    EMPTY_NOTE_STATUS, LOAD_FAILED_PREFIX, NoteStorage, NoteStorageError, SAVE_FAILED_PREFIX,
    StorageNoteState, storage_note_screen,
};
use fandhe_core::{click_button, render_headless};
use std::sync::{Arc, Mutex};

/// テスト専用のインメモリ `NoteStorage`。`Arc<Mutex<..>>` で複数の `StorageNoteState`
/// から同一の保存先を共有し、「別状態インスタンスでの復元」（プロセス再起動相当）を
/// 検証できるようにする。
#[derive(Clone, Default)]
struct MemStorage(Arc<Mutex<Option<Vec<u8>>>>);

impl NoteStorage for MemStorage {
    fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
        Ok(self.0.lock().expect("lock は成功する").clone())
    }

    fn write(&self, _key: &str, bytes: &[u8]) -> Result<(), NoteStorageError> {
        *self.0.lock().expect("lock は成功する") = Some(bytes.to_vec());
        Ok(())
    }
}

/// 常に書込に失敗する `NoteStorage`（保存失敗時の挙動検証用）。
struct FailingStorage;

impl NoteStorage for FailingStorage {
    fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
        Ok(None)
    }

    fn write(&self, _key: &str, _bytes: &[u8]) -> Result<(), NoteStorageError> {
        Err(NoteStorageError::Other("disk full".to_string()))
    }
}

/// 常に読み込みに失敗する `NoteStorage`（構築時の読み込み失敗の挙動検証用。
/// 既存メモの有無に関わらず read が失敗するプラットフォーム障害を模す）。
struct FailingReadStorage;

impl NoteStorage for FailingReadStorage {
    fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
        Err(NoteStorageError::Io("permission denied".to_string()))
    }

    fn write(&self, _key: &str, _bytes: &[u8]) -> Result<(), NoteStorageError> {
        panic!("読み込み失敗時は storage を保持しないため write は呼ばれない想定");
    }
}

#[test]
fn initial_render_shows_field_button_and_empty_list() {
    let screen = storage_note_screen();
    let state = StorageNoteState::default();

    let lines = render_headless(&screen, &state);

    assert_eq!(lines[0], "# メモ保存");
    assert!(lines.contains(&"[メモ: ]".to_string()));
    assert!(lines.contains(&"(保存)".to_string()));
    assert!(lines.contains(&"メモはありません".to_string()));
}

#[test]
fn save_then_new_state_restores_newest_first() {
    let screen = storage_note_screen();
    let mem = MemStorage::default();

    let mut state = StorageNoteState::with_storage(Box::new(mem.clone()));
    fandhe_core::screens::storage_note::set_input(&mut state, "一件目".to_string());
    assert!(click_button(&screen, "保存", &mut state));
    fandhe_core::screens::storage_note::set_input(&mut state, "二件目".to_string());
    assert!(click_button(&screen, "保存", &mut state));

    assert_eq!(
        state.notes,
        vec!["二件目".to_string(), "一件目".to_string()]
    );

    // 別インスタンス（プロセス再起動相当）で同じ保存先から復元する。
    let restored = StorageNoteState::with_storage(Box::new(mem));
    assert_eq!(
        restored.notes,
        vec!["二件目".to_string(), "一件目".to_string()]
    );
    let lines = render_headless(&screen, &restored);
    let idx_first = lines.iter().position(|l| l == "二件目").unwrap();
    let idx_second = lines.iter().position(|l| l == "一件目").unwrap();
    assert!(idx_first < idx_second);
}

#[test]
fn empty_input_is_not_saved() {
    let screen = storage_note_screen();
    let mut state = StorageNoteState::default();
    fandhe_core::screens::storage_note::set_input(&mut state, "   ".to_string());

    assert!(click_button(&screen, "保存", &mut state));

    assert!(state.notes.is_empty());
    assert_eq!(state.status, EMPTY_NOTE_STATUS);
}

#[test]
fn write_failure_keeps_note_and_shows_error() {
    let screen = storage_note_screen();
    let mut state = StorageNoteState::with_storage(Box::new(FailingStorage));
    fandhe_core::screens::storage_note::set_input(&mut state, "メモ".to_string());

    assert!(click_button(&screen, "保存", &mut state));

    assert_eq!(state.notes, vec!["メモ".to_string()]);
    assert!(state.status.starts_with(SAVE_FAILED_PREFIX));
}

#[test]
fn newline_in_note_is_flattened() {
    let screen = storage_note_screen();
    let mem = MemStorage::default();
    let mut state = StorageNoteState::with_storage(Box::new(mem.clone()));
    fandhe_core::screens::storage_note::set_input(&mut state, "a\nb".to_string());

    assert!(click_button(&screen, "保存", &mut state));

    assert_eq!(state.notes, vec!["a b".to_string()]);
    let restored = StorageNoteState::with_storage(Box::new(mem));
    assert_eq!(restored.notes, vec!["a b".to_string()]);
}

/// codex-review P1 の回帰テスト: 構築時の読み込み失敗後に save しても、
/// 保存先へは書き込まない（＝既存メモを上書きしない）。`storage` を保持しない
/// ことで、次の save は書き込みを試みず新規メモをメモリ内にのみ残す。
#[test]
fn read_failure_disables_persistence_and_does_not_overwrite() {
    let screen = storage_note_screen();
    let mut state = StorageNoteState::with_storage(Box::new(FailingReadStorage));
    assert!(state.status.starts_with(LOAD_FAILED_PREFIX));

    fandhe_core::screens::storage_note::set_input(&mut state, "新規メモ".to_string());
    assert!(click_button(&screen, "保存", &mut state));

    // FailingReadStorage::write は panic するため、ここまで到達していれば
    // 書き込みが試みられなかったことの証明になる。
    assert_eq!(state.notes, vec!["新規メモ".to_string()]);
}

/// Issue #987 の回帰テスト: 前後の空白は trim せずそのまま保存・復元される
/// （空判定のみ trim する、契約表 8「入力どおりに保存」）。
#[test]
fn leading_and_trailing_whitespace_is_preserved() {
    let screen = storage_note_screen();
    let mem = MemStorage::default();
    let mut state = StorageNoteState::with_storage(Box::new(mem.clone()));
    fandhe_core::screens::storage_note::set_input(&mut state, "  メモ  ".to_string());

    assert!(click_button(&screen, "保存", &mut state));

    assert_eq!(state.notes, vec!["  メモ  ".to_string()]);
    let restored = StorageNoteState::with_storage(Box::new(mem));
    assert_eq!(restored.notes, vec!["  メモ  ".to_string()]);
}

/// P2 / Bugbot の回帰テスト: 保存先なし（メモリ内のみ）の save は成功として
/// status を更新し、直前のエラー文言を残さない。
#[test]
fn memory_only_save_updates_status_after_previous_error() {
    let screen = storage_note_screen();
    let mut state = StorageNoteState::default().with_status("前回のエラー");
    fandhe_core::screens::storage_note::set_input(&mut state, "メモ".to_string());

    assert!(click_button(&screen, "保存", &mut state));

    assert!(!state.status.contains("前回のエラー"));
    assert!(state.status.contains('1'));
}
