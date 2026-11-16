//! iOS の保存先。`$HOME` がアプリごとにサンドボックス化されたコンテナのルートを指すため、`Documents` を連結する
//! だけで App Store の審査要件（ユーザーデータは Documents/Library 配下）に沿う（Issue #751）。
//! `lib.rs` の `data_dir` から呼ばれる。

use fandhe_device::DataDir;

use crate::error::LocalStorageKvError;

/// アプリコンテナ内の `Documents/` を返す。`std::env::home_dir()` は deprecated だが単純な環境変数参照で
/// `unsafe` を伴わない（`FsStorage::desktop_default` と同じ判断）。
pub(crate) fn documents_dir() -> Result<DataDir, LocalStorageKvError> {
    #[allow(deprecated)]
    std::env::home_dir()
        .map(|home| DataDir(home.join("Documents")))
        .ok_or(LocalStorageKvError::Unsupported("home directory not found"))
}
