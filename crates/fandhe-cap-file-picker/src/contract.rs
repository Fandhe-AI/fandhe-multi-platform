//! ファイル選択ダイアログの契約（Phase 11、Issue #872）。
//!
//! `Clipboard`/`Tts` と同じ request/response の一発呼び出し型（1 回の `pick()` = 1 回のダイアログ）。[`FilePicker`] トレイトの
//! 同期契約はデスクトップ（macOS/Windows）専用。
//!
//! iOS/Android/Web は本トレイトを通らない。OS がコールバック型（ダイアログ提示 → 結果を非同期通知）であり、ワーカースレッド +
//! mailbox で同期化するとトレイト実装・mailbox・コア状態機械の 3 層管理が生まれ、再生成・プロセス死・遅延配送で継ぎ目がずれるため。
//! 代わりに `driver`（iOS/Android 橋渡し）と `fandhe-web-dom` は、コアの画面状態機械 `fandhe_core::screens::file_pick`
//! （`take_request`/`finish_pick` + `generation` 照合）へ OS コールバックから直接書き戻す。OS 側の責務は
//! 「ダイアログ提示 → 結果を渡された要求 ID（橋渡し側がプロセス内で採番。`generation` との対応は橋渡し側が持つ）付きで 1 回だけ通知 →
//! 一時ファイルの後始末」のみとし、ネイティブ（iOS/Android）の読み取り・8 MiB 判定は `fandhe-device::file_picker` の
//! [`read_picked`]（iOS: サンドボックス tmp コピーのパス）・[`read_picked_from`]（Android: SAF の fd を `OwnedFd` で借用した `File`）に
//! 一本化する。Web はパスを持たない（DOM の `File` を読む）ため [`MAX_FILE_BYTES`] と 3 値の結果契約のみを共有する
//! （3 値・上限・読み取り失敗の扱いはトレイト経由・状態機械経由のどちらでも同一）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 1 回の [`FilePicker::pick`] 呼び出し = 1 回のダイアログ。ユーザーが閉じるまで確定しない。**固定タイムアウトを掛けない**
//! - 結果は 3 値: 選択 `Ok(Some(PickedFile))` / キャンセル `Ok(None)`（失敗ではない）/ 失敗 `Err`
//! - ファイルパスは返さない・表示しない・ログに出さない（モバイル/Web ではパス自体が無意味であり、返り値に含めると呼び出し元が
//!   うっかり表示してしまう）
//! - ダイアログを閉じた時点で内容を読み切る。サイズ上限は [`MAX_FILE_BYTES`]（5 OS 共通の唯一の定数）。超過は
//!   `Err(FilePickerError::InvalidInput)`
//! - 使えない環境（Linux 等）は `Err(FilePickerError::Unsupported)`。読み取り失敗は `Err(FilePickerError::Internal)`
//! - 進行中に再度呼ばれたら `Err(FilePickerError::Internal("file_picker busy"))`（二重にダイアログを開かない）
//!
//! macOS の `NSOpenPanel` はメインスレッド必須。呼び出し元（`driver` の effect ドライバ）は UI スレッド（= 描画ループのスレッド）から
//! **同期で**呼ぶ契約とし、別スレッドへ逃がさない（`camera_capture` のようなワーカースレッド + mailbox 方式を誤って適用しないこと）。

use crate::error::FilePickerError;

pub use fandhe_device::file_picker::{
    MAX_FILE_BYTES, PickedFile, read_picked, read_picked_from, sanitize_picked_name,
};

/// ファイル選択ダイアログの契約（モジュール doc の「契約」節を参照。iOS/Android/Web の実装者が読む唯一の仕様）。
pub trait FilePicker: Send + Sync {
    /// OS 標準の「開く」ダイアログを 1 回開く。
    fn pick(&self) -> Result<Option<PickedFile>, FilePickerError>;
}

/// ダイアログを開けない環境（Linux 等）を表す実装。常に `Unsupported` を返し、成功を偽装しない。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedFilePicker;

impl FilePicker for UnsupportedFilePicker {
    fn pick(&self) -> Result<Option<PickedFile>, FilePickerError> {
        Err(FilePickerError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_picker_returns_unsupported_error() {
        let err = UnsupportedFilePicker.pick().unwrap_err();
        assert!(matches!(err, FilePickerError::Unsupported));
    }
}
