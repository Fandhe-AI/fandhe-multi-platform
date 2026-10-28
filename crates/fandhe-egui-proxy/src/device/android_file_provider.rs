//! Android 向け「OS のファイル画面に出る保存先」のスタブ（Issue #1019、親 #194）。
//! `device/file_provider.rs` の Android 経路（`platform`）から呼ばれる。契約は
//! `fandhe_device::file_provider`。
//!
//! 現状は常に `Unsupported` を返す。#1020 が [`implementation`] を
//! `DirFileProvider::new(data_dir()/FOLDER_NAME, JNI の notifyChange)` に置き換える。
//! 後続が触るのは本ファイルと OS の付帯物（Java・manifest・`strings.xml`）だけで、
//! dispatch と画面には触らない。

use std::sync::Arc;

use fandhe_device::file_provider::{FileProvider, UnsupportedFileProvider};

/// Android 実装を返す。#1020 で実体に置き換える。
pub(crate) fn implementation() -> Arc<dyn FileProvider> {
    Arc::new(UnsupportedFileProvider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_never_succeeds() {
        let p = implementation();
        assert!(p.list().is_err());
        assert!(p.put("a.txt", b"x").is_err());
        assert!(p.delete("a.txt").is_err());
    }
}
