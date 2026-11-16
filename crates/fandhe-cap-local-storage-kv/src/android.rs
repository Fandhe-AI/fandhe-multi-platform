//! Android の保存先。`android-activity` の `AndroidApp::internal_data_path()`（`getFilesDir()` 相当）を使うため
//! JNI を書かずに済む（Issue #750）。`lib.rs` の `data_dir` から呼ばれる。

use fandhe_device::DataDir;

use crate::error::LocalStorageKvError;

/// アプリ内部ストレージのパスを返す。`android_app()` が `None`（`android_main` 未実行）か OS 側の取得が
/// 失敗したときは `Err`。
pub(crate) fn data_dir() -> Result<DataDir, LocalStorageKvError> {
    fandhe_platform_android::android_app()
        .and_then(|app| app.internal_data_path())
        .map(DataDir)
        .ok_or(LocalStorageKvError::Unsupported(
            "internal data path not found",
        ))
}
