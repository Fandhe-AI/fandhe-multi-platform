//! Android 向け「OS のファイル画面に出る保存先」の実装（Issue #1020、親 #194）。
//! `device/file_provider.rs` の Android 経路（`platform`）から呼ばれる。契約は
//! `fandhe_device::file_provider`（再利用しない書類 ID 方式）。
//!
//! # 方式
//!
//! 保存先は `getFilesDir()/file-provider`（`fandhe_device::file_provider::FOLDER_NAME`）で、
//! 共通実装 `DirFileProvider` がそのまま使う。同じフォルダを Java 側の
//! `FileProviderDocumentsProvider`（`DocumentsProvider`）が Files アプリ等へ見せる。
//! 配置は `<フォルダ>/<id>/<表示名>`。document ID は書類 ID で、表示名は識別に使わない。
//! 消した ID は二度と使われないため、古い URI の権限が後から同じ表示名で作った書類に及ばない。
//! このため世代表も、Rust と Java に共通の排他も持たない。
//!
//! # 変更通知（JNI 署名は Java 側と一致させる。native 宣言は無いので no-op スタブは不要）
//!
//! - `boolean fileProviderNotifyChanged()`（`MainActivity`）。`true` は通知を投げられた
//!   ことだけを指す。`false`・JNI 失敗・例外は `notified = false` にし、成功へは倒さない。
//!
//! # Activity 再生成
//!
//! 通知は同期の JNI 呼び出しで、Java 側に保留中の要求も要求 id も持たない。再生成をまたいだ
//! 旧世代の完了は `device/file_provider.rs` の世代照合と旧 sender で捨てられ、新しい要求の
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
use fandhe_device::file_provider::{DirFileProvider, FOLDER_NAME};
use fandhe_device::file_provider::{FileProvider, UnsupportedFileProvider};
use fandhe_device::DeviceError;

/// JNI の通知結果（`Some(true)` だけが成功。`false`・JNI 失敗は `Internal`）を契約上の結果へ写す
/// （ホストの `cargo test` で固定する純関数）。
fn map_notify(r: Option<bool>) -> Result<(), DeviceError> {
    match r {
        Some(true) => Ok(()),
        Some(false) | None => Err(DeviceError::Internal("file_provider notify")),
    }
}

/// Android 実装を返す。保存先を解決できなければ非対応にする（0 件の成功にしない）。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn FileProvider> {
    match super::android::data_dir() {
        Some(d) => Arc::new(DirFileProvider::new(
            d.0.join(FOLDER_NAME),
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
    use crate::device::android::JniCallFailed;

    /// `MainActivity.fileProviderNotifyChanged()` を呼ぶ。JNI 失敗・例外は `None`。
    /// `AndroidHaptics::tap` と同じ根拠・同じ形。
    #[allow(unsafe_code)]
    pub(super) fn notify_changed() -> Option<bool> {
        let app = crate::android::android_app()?;
        // SAFETY: `AndroidHaptics::tap` と同一の根拠。`vm_as_ptr()` は android-activity が
        // 管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<bool, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `AndroidHaptics::tap` と同一の根拠。未所有のグローバル参照を `Cast` で
            // 包むだけで delete は試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let ret = env.call_method(
                &activity,
                jni::jni_str!("fileProviderNotifyChanged"),
                jni::jni_sig!(() -> boolean),
                &[],
            );
            match ret {
                Ok(v) => Ok(v.z()?),
                Err(e) => {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に残さず、文言は読まない。
                    if env.exception_check() {
                        env.exception_clear();
                    }
                    Err(e.into())
                }
            }
        })
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
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(map_notify(None), Err(DeviceError::Internal(_))));
    }
}
