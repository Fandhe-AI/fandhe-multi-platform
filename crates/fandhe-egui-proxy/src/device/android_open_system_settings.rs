//! Android 向け「OS の設定画面を開く」実装（親 #171、Issue #800）。`device/open_system_settings.rs` の
//! Android 経路（dispatch）から呼ばれる。Java 側 `SystemSettingsHelper` が固定の 2 頁
//! （自アプリの詳細設定・通知設定）を `startActivity` で開く。契約の SSOT は
//! `fandhe_device::open_system_settings`。
//!
//! 流れ: `begin_open` → JNI `MainActivity.requestOpenSystemSettings(id, page)` →
//! Java が `nativeOnOpenSettingsResult(id, code)` で返す → `outcome_from_android_code` で契約の結果へ
//! 写像 → `open_system_settings::bridge::deliver` が `SLOT` の id と照合して置く。
//! 渡すのは `long` の id と `int` の頁コードだけで、Intent・URI は Java 内の固定のものを使う。
//!
//! 守る範囲: 1 プロセス内の要求順序・非前景や JNI 失敗を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄。守らない範囲: 設定画面へ渡した後の利用者の操作（結果は `HandedOff` までしか言えない）。
//! `<queries>`・権限・`resolveActivity` は使わない。開けなかった頁を別の頁で代用しない。
//! id・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestOpenSystemSettings` を経た後でしか
//! 呼ばれない（compose_mail_sms と同じ）。

use fandhe_device::open_system_settings::SettingsPage;
use fandhe_device::DeviceError;

/// 頁コード（Java 側 `SystemSettingsHelper.PAGE_*` と対）。
const PAGE_APP: i32 = 0;
const PAGE_NOTIFICATIONS: i32 = 1;

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn page_code(page: SettingsPage) -> i32 {
    match page {
        SettingsPage::AppSettings => PAGE_APP,
        SettingsPage::Notifications => PAGE_NOTIFICATIONS,
    }
}

/// OS へ設定画面の表示を依頼する。`Ok` は受理済みで、結果は後から `nativeOnOpenSettingsResult` で届く。
#[cfg(target_os = "android")]
pub(crate) fn begin_open(request_id: u64, page: SettingsPage) -> Result<(), DeviceError> {
    if call_activity(request_id, page) {
        Ok(())
    } else {
        Err(DeviceError::Internal("open settings present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_open(_request_id: u64, _page: SettingsPage) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("open settings"))
}

/// `MainActivity.requestOpenSystemSettings(long, int)` を JNI で呼ぶ。
/// `false` は呼び出し失敗。`unsafe` 2 箇所は `android_compose_mail_sms::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(id: u64, page: SettingsPage) -> bool {
    use super::android::JniCallFailed;
    let Some(app) = crate::android::android_app() else {
        return false;
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        let args = [
            jni::objects::JValue::Long(id as jni::sys::jlong),
            jni::objects::JValue::Int(page_code(page)),
        ];
        env.call_method(
            &activity,
            jni::jni_str!("requestOpenSystemSettings"),
            jni::jni_sig!((long, int) -> ()),
            &args,
        )?;
        Ok(())
    })
    .is_ok()
}

/// `SystemSettingsHelper.nativeOnOpenSettingsResult`（Java 側）から呼ばれる JNI エクスポート。
/// id・結果は一切ログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_SystemSettingsHelper_nativeOnOpenSettingsResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    super::open_system_settings::bridge::deliver(
        request_id as u64,
        super::open_system_settings::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stub_never_succeeds() {
        for p in [SettingsPage::AppSettings, SettingsPage::Notifications] {
            assert!(matches!(
                begin_open(1, p),
                Err(DeviceError::Unsupported("open settings"))
            ));
        }
    }

    #[test]
    fn page_code_matches_java() {
        assert_eq!(page_code(SettingsPage::AppSettings), 0);
        assert_eq!(page_code(SettingsPage::Notifications), 1);
    }
}
