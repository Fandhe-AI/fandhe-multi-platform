//! 型サンプル「メモ保存」の Web 経路（Issue #987、親 #986）。
//!
//! `fandhe_core::screens::storage_note::NoteStorage`（端末依存ゼロの同期トレイト）を
//! ブラウザの `localStorage`（同期 API）で実装する。`clipboard`/`tts` 等の他画面と違い
//! 非同期 I/O が無いため `mount_with_effect`（effect フック）は使わず、
//! `super::mount`（`mount_sample_screen` と同じ形）へ直接 `Box<dyn NoteStorage>` を
//! 注入した状態を渡すだけで、読み込みによる復元・保存・失敗表示は core 側の
//! 既存ロジック（`StorageNoteState::with_storage`/`save`）がそのまま処理する。
//!
//! `web_sys::Storage`（`JsValue` 相当、`Send`/`Sync` ではない）を保持すると
//! `NoteStorage: Send + Sync` を満たせないため、フィールドを持たないゼロサイズ型
//! [`LocalNoteStorage`] にし、呼び出しのたびに `web_sys::window()` から
//! 取り直す（デスクトップの `FsStorage` がパスを保持するのとは異なる設計）。
//!
//! # キーの名前空間
//!
//! `localStorage` のキーは [`storage_key`] で `fandhe.storage.` を接頭辞にする
//! （`biometric_auth.rs::STORAGE_KEY` の `fandhe.` 系名前空間と揃える。desktop の
//! `$HOME/.fandhe/` 配下に対応）。渡される key は core の固定定数 `NOTES_KEY` のみで
//! ユーザー入力をキーへ展開しないため（A03 対策）、パストラバーサル検査は不要
//! （ponytail: `localStorage` にパスの概念自体が無い）。
//!
//! # 失敗の写像（契約表 6「嘘の成功」対策）
//!
//! [`error_from_dom_name`] が `DOMException`/`Error` の `name` を固定文言へ写す
//! （`message` は転送しない、security.md A09。`share.rs`/`biometric_auth.rs` と
//! 同じ取り出し方）。未保存（`get_item` が `None`）は `Ok(None)` を返し失敗にしない。
//!
//! # バイト列の変換（契約表 8「入力どおりに保存」対策）
//!
//! [`bytes_to_value`] は `String::from_utf8` で厳密に変換し、非 UTF-8 は lossy 変換で
//! 黙って書き換えず `NoteStorageError::Other` へ倒す。値の trim・正規化は一切しない。
//!
//! # セキュリティ注意（A02）
//!
//! `localStorage` は平文で同一オリジンの任意スクリプトから読める。本画面はメモの
//! サンプルで秘密値は扱わない。秘密値の保存先には使わない（`secure_credential` 系を使う）。

use fandhe_core::screens::storage_note::NoteStorageError;

/// `localStorage` キーの接頭辞（モジュール doc「キーの名前空間」節参照）。native ビルドでは
/// `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`share.rs::UNAVAILABLE` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const KEY_PREFIX: &str = "fandhe.storage.";

/// core の `key`（例: `NOTES_KEY` = `"storage_note.txt"`）を実際の `localStorage` キーへ
/// 変換する純関数（native からもテストできるよう `wasm32` gate の外に置く、
/// `location.rs` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn storage_key(key: &str) -> String {
    format!("{KEY_PREFIX}{key}")
}

/// 保存容量超過のときの案内。
const QUOTA_EXCEEDED: &str = "保存容量の上限を超えました";
/// このブラウザ設定では使えないときの案内（`SecurityError`）。
const SECURITY_ERROR: &str = "このブラウザ設定では localStorage を使えません";
/// 種別を特定できない失敗の汎用案内。
const GENERIC_FAILURE: &str = "localStorage の操作に失敗しました";

/// 例外オブジェクトの `name` を `NoteStorageError` へ写す純関数（`message` は
/// 転送しない、モジュール doc「失敗の写像」節参照）。`share.rs::outcome_from_error_name`
/// と同じ理由で native からもテストできるよう `wasm32` gate の外に置く。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn error_from_dom_name(name: Option<&str>) -> NoteStorageError {
    match name {
        Some("QuotaExceededError") | Some("NS_ERROR_DOM_QUOTA_REACHED") => {
            NoteStorageError::Io(QUOTA_EXCEEDED.to_string())
        }
        Some("SecurityError") => NoteStorageError::Unsupported(SECURITY_ERROR.to_string()),
        _ => NoteStorageError::Io(GENERIC_FAILURE.to_string()),
    }
}

/// `localStorage` の生の値（`Option<String>`、バイト列）を `NoteStorage::read` の
/// 戻り値へ変換する純関数。`None`（未保存）はエラーにしない契約（core 側
/// `with_storage` の「読み込み失敗時は保持しない」設計と対）。native ビルドでは
/// `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn bytes_to_value(raw: Option<String>) -> Option<Vec<u8>> {
    raw.map(String::into_bytes)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    use super::{bytes_to_value, error_from_dom_name, storage_key};
    use fandhe_core::screens::storage_note::{NoteStorage, NoteStorageError};

    /// 例外オブジェクトから `name` を取り出す（`share.rs`/`biometric_auth.rs` と同じ
    /// 取り出し方。取り出せなければ汎用失敗へ倒す）。
    fn name_of(err: &JsValue) -> Option<String> {
        Reflect::get(err, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
    }

    /// `localStorage` を保存先とする [`NoteStorage`] 実装。フィールドを持たない
    /// ゼロサイズ型（モジュール doc「`Send + Sync` のため」節参照）。
    pub struct LocalNoteStorage;

    impl NoteStorage for LocalNoteStorage {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
            let window = web_sys::window().ok_or_else(|| {
                NoteStorageError::Unsupported("localStorage がありません".to_string())
            })?;
            let storage = window
                .local_storage()
                .map_err(|e| error_from_dom_name(name_of(&e).as_deref()))?
                .ok_or_else(|| {
                    NoteStorageError::Unsupported("localStorage がありません".to_string())
                })?;
            let raw = storage
                .get_item(&storage_key(key))
                .map_err(|e| error_from_dom_name(name_of(&e).as_deref()))?;
            Ok(bytes_to_value(raw))
        }

        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), NoteStorageError> {
            // lossy 変換で黙って書き換えない（モジュール doc「バイト列の変換」節、
            // 契約表 8「入力どおりに保存」）。
            let value = String::from_utf8(bytes.to_vec()).map_err(|_| {
                NoteStorageError::Other("保存値が UTF-8 として不正です".to_string())
            })?;
            let window = web_sys::window().ok_or_else(|| {
                NoteStorageError::Unsupported("localStorage がありません".to_string())
            })?;
            let storage = window
                .local_storage()
                .map_err(|e| error_from_dom_name(name_of(&e).as_deref()))?
                .ok_or_else(|| {
                    NoteStorageError::Unsupported("localStorage がありません".to_string())
                })?;
            storage
                .set_item(&storage_key(key), &value)
                .map_err(|e| error_from_dom_name(name_of(&e).as_deref()))
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::LocalNoteStorage;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_exceeded_maps_to_io_error() {
        let err = error_from_dom_name(Some("QuotaExceededError"));
        assert!(matches!(err, NoteStorageError::Io(m) if m == QUOTA_EXCEEDED));
        let err = error_from_dom_name(Some("NS_ERROR_DOM_QUOTA_REACHED"));
        assert!(matches!(err, NoteStorageError::Io(m) if m == QUOTA_EXCEEDED));
    }

    #[test]
    fn security_error_maps_to_unsupported() {
        let err = error_from_dom_name(Some("SecurityError"));
        assert!(matches!(err, NoteStorageError::Unsupported(m) if m == SECURITY_ERROR));
    }

    #[test]
    fn unknown_or_missing_name_is_generic_io_error() {
        let err = error_from_dom_name(None);
        assert!(matches!(err, NoteStorageError::Io(m) if m == GENERIC_FAILURE));
        let err = error_from_dom_name(Some("SomethingElse"));
        assert!(matches!(err, NoteStorageError::Io(m) if m == GENERIC_FAILURE));
    }

    #[test]
    fn storage_key_has_fandhe_prefix() {
        assert_eq!(
            storage_key("storage_note.txt"),
            "fandhe.storage.storage_note.txt"
        );
    }

    #[test]
    fn missing_value_is_not_an_error() {
        assert_eq!(bytes_to_value(None), None);
    }

    #[test]
    fn round_trips_whitespace_and_newlines() {
        let value = "  前後に空白 \n改行入り  ".to_string();
        assert_eq!(
            bytes_to_value(Some(value.clone())),
            Some(value.into_bytes())
        );
    }
}
