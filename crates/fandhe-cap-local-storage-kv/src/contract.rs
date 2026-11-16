//! local-storage-kv の契約。キー・値の保存口（`Storage`）・ファイル実装（`FsStorage`）・保存先（`DataDir`）は、
//! secure-credential・file-watch・quick-settings-tile・wallpaper-set など OS に触れずに複数機能が使うため
//! `fandhe-device` に置いたままで、ここから再公開する（`consumer-app-structure.md` §8）。
//! 保存先の解決（OS ごとの既定の場所）は [`crate::data_dir`] が持つ。

pub use fandhe_device::DataDir;
pub use fandhe_device::storage::{FsStorage, Storage, validate_key};
