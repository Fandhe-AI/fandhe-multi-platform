//! Android 向け「OS のファイル画面に出る保存先」の実装（Issue #1020、親 #194）。
//! `driver` の Android 経路（`platform_file_provider`）から呼ばれる。契約は
//! `crate::contract`（再利用しない書類 ID 方式）。
//!
//! # 方式
//!
//! 保存先は `getFilesDir()/file-provider`（`crate::contract::FOLDER_NAME`）で、
//! 共通実装 `DirFileProvider` がそのまま使う。同じフォルダを Java 側の
//! `FileProviderDocumentsProvider`（`DocumentsProvider`、本 crate の `android/java`）が Files アプリ等へ見せる。
//! 配置は `<フォルダ>/<id>/<表示名>`。document ID は書類 ID で、表示名は識別に使わない。
//! 消した ID は二度と使われないため、古い URI の権限が後から同じ表示名で作った書類に及ばない。
//! このため世代表も、Rust と Java に共通の排他も持たない。
//!
//! # 変更通知（JNI 署名は Java 側と一致させる。native 宣言は無いので no-op スタブは不要）
//!
//! - `static boolean FileProviderDocumentsProvider.notifyChanged(Context)`。`true` は通知を投げられた
//!   ことだけを指す。`false`・JNI 失敗・例外は `notified = false` にし、成功へは倒さない。
//!
//! # Activity 再生成
//!
//! 通知は同期の JNI 呼び出しで、Java 側に保留中の要求も要求 id も持たない。再生成をまたいだ
//! 旧世代の完了は `driver` の世代照合と旧 sender で捨てられ、新しい要求の
//! 状態を消費しない。
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 初期化できなければ非対応（0 件の成功にしない）、通知失敗を成功と並べて見せる、
//! 名前・ID・パスをログ・エラー文に出さない。
//! 守らない: 他アプリが書き込み中の途中サイズ、拡張子の無い名前の MIME、通知からファイル画面の
//! 表示更新までの時間差、`lstat` と `open` の間の差し替え（私有フォルダに項目を作れるのは
//! 自プロセスだけ）。

use std::sync::Arc;

#[cfg(target_os = "android")]
use crate::contract::{DirFileProvider, FOLDER_NAME};
use crate::contract::{FileProvider, UnsupportedFileProvider};
use crate::error::FileProviderError;

/// JNI の通知結果（`Some(true)` だけが成功。`false`・JNI 失敗は `Internal`）を契約上の結果へ写す
/// （ホストの `cargo test` で固定する純関数）。
fn map_notify(r: Option<bool>) -> Result<(), FileProviderError> {
    match r {
        Some(true) => Ok(()),
        Some(false) | None => Err(FileProviderError::Internal("file_provider notify")),
    }
}

/// Android 実装を返す。保存先を解決できなければ非対応にする（0 件の成功にしない）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn FileProvider> {
    match fandhe_platform_android::android_app().and_then(|app| app.internal_data_path()) {
        Some(d) => Arc::new(DirFileProvider::new(
            d.join(FOLDER_NAME),
            Box::new(|| map_notify(imp::notify_changed())),
        )),
        None => Arc::new(UnsupportedFileProvider),
    }
}

/// ホストビルド（テスト）では常に非対応。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn FileProvider> {
    Arc::new(UnsupportedFileProvider)
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// `FileProviderDocumentsProvider.notifyChanged(Context)`（static）を呼ぶ。JNI 失敗・例外は `None`。
    /// Context には Activity を渡す（`notifyChanged` は `getPackageName` と `getContentResolver` だけを使い、保持しない）。
    /// クラスは Activity の class loader で解決する（`fandhe_platform_android::with_activity_class`）。
    pub(super) fn notify_changed() -> Option<bool> {
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.FileProviderDocumentsProvider"),
            |env, activity, class| -> Result<bool, JniCallFailed> {
                Ok(env
                    .call_static_method(
                        class,
                        jni::jni_str!("notifyChanged"),
                        jni::jni_sig!((android.content.Context) -> boolean),
                        &[jni::objects::JValue::Object(activity)],
                    )?
                    .z()?)
            },
        )
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_build_never_succeeds() {
        let p = implementation();
        assert!(p.list().is_err());
        assert!(p.put("a.txt", b"x").is_err());
        assert!(p.delete("a").is_err());
    }

    #[test]
    fn notify_only_true_is_ok() {
        assert!(map_notify(Some(true)).is_ok());
        assert!(matches!(
            map_notify(Some(false)),
            Err(FileProviderError::Internal(_))
        ));
        assert!(matches!(
            map_notify(None),
            Err(FileProviderError::Internal(_))
        ));
    }
}
