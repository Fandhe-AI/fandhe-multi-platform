//! Android 向け「OS のファイル画面に出る保存先」の実装（Issue #1020、親 #194）。
//! `device/file_provider.rs` の Android 経路（`platform`）から呼ばれる。契約は
//! `fandhe_device::file_provider`。
//!
//! 方式: OS のファイル画面（Files アプリ・SAF）への公開は manifest 宣言の Java クラス
//! `FileProviderDocumentsProvider`（`DocumentsProvider`）が担い、native を要さず Activity が無くても
//! 同じプロセスで動く。本ファイルは同じフォルダ（`data_dir()/FOLDER_NAME` = `getFilesDir()/file-provider`）
//! を [`DirFileProvider`] で読み書きし、置く・消した後の変更通知だけを JNI
//! （`MainActivity.fileProviderNotifyChanged()`、署名 `()Z`）で Java へ頼む。`false` と JNI 失敗は
//! `Internal`（`PutOutcome::notified = false` になり、成功へ倒さない）。
//!
//! Activity 再生成: 通知は同期の JNI 呼び出しで、Java 側に保留中の要求も id も持たない。再生成を
//! またいで引き継ぐ要求も、旧結果が新しい要求の id を消費する経路も無い。旧世代の完了は
//! `device/file_provider.rs` の世代照合と旧 sender で捨てられる。
//!
//! 初期化できない（`android_app()` が `None`）ときは `Unsupported` に倒す（0 件にしない）。
//!
//! 守る範囲: 1 プロセス内の操作順、嘘の成功の禁止、パスの閉じ込め。守らない範囲: OS のファイル画面の
//! 表示の遅れ、他アプリの書き込み中の途中サイズ、複数プロセス。名前・中身・パスはログに出さない。
//! 新しい権限・依存は足さない。

use std::sync::Arc;

#[cfg(target_os = "android")]
use fandhe_device::file_provider::{DirFileProvider, FOLDER_NAME};
use fandhe_device::file_provider::{FileProvider, UnsupportedFileProvider};
use fandhe_device::DeviceError;

/// Android 実装を返す。
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

/// ホストのテストビルドでは非対応（成功を返さない）。
#[cfg(not(target_os = "android"))]
pub(crate) fn implementation() -> Arc<dyn FileProvider> {
    Arc::new(UnsupportedFileProvider)
}

/// Java の通知結果を契約の値へ写す。`false` と JNI 失敗は `Internal`（成功へ倒さない）。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn map_notify(r: Option<bool>) -> Result<(), DeviceError> {
    match r {
        Some(true) => Ok(()),
        _ => Err(DeviceError::Internal("file_provider notify")),
    }
}

#[cfg(target_os = "android")]
mod imp {
    /// `MainActivity.fileProviderNotifyChanged()` を呼ぶ。JNI 失敗・Java 例外は例外を clear して `None`。
    #[allow(unsafe_code)]
    pub(super) fn notify_changed() -> Option<bool> {
        let app = crate::android::android_app()?;
        // SAFETY: `android_quick_settings_tile::imp::read` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` で、`app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<bool, super::super::android::JniCallFailed> {
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない
                // （`app` の生存中有効）。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let result = (|| {
                    Ok(env
                        .call_method(
                            &activity,
                            jni::jni_str!("fileProviderNotifyChanged"),
                            jni::jni_sig!(() -> boolean),
                            &[],
                        )?
                        .z()?)
                })();
                if result.is_err() && env.exception_check() {
                    env.exception_clear();
                }
                result
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
        assert!(p.delete("a.txt").is_err());
    }

    #[test]
    fn map_notify_does_not_turn_failure_into_ok() {
        assert!(map_notify(Some(true)).is_ok());
        assert!(matches!(
            map_notify(Some(false)),
            Err(DeviceError::Internal("file_provider notify"))
        ));
        assert!(matches!(
            map_notify(None),
            Err(DeviceError::Internal("file_provider notify"))
        ));
    }
}
