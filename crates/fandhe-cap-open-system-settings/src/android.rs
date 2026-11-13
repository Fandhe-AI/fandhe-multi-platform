//! Android 向け「OS の設定画面を開く」実装（親 #171、Issue #800）。`driver.rs` の
//! Android 経路（dispatch）から呼ばれる。Java 側 `SystemSettingsHelper` が固定の 2 頁
//! （自アプリの詳細設定・通知設定）を `startActivity` で開く。契約の SSOT は
//! `crate::contract`。
//!
//! 流れ: `begin_open` → JNI static の `SystemSettingsHelper.open(Activity, id, page)` →
//! Java が `nativeOnOpenSettingsResult(id, code)` で返す → `outcome_from_android_code` で契約の結果へ
//! 写像 → `driver::bridge::deliver` が `SLOT` の id と照合して置く。
//! 渡すのは `long` の id と `int` の頁コードだけで、Intent・URI は Java 内の固定のものを使う。
//!
//! 守る範囲: 1 プロセス内の要求順序・非前景や JNI 失敗を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄。守らない範囲: 設定画面へ渡した後の利用者の操作（結果は `HandedOff` までしか言えない）。
//! `<queries>`・権限・`resolveActivity` は使わない。開けなかった頁を別の頁で代用しない。
//! id・結果はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestOpenSystemSettings` を経た後でしか
//! 呼ばれない（compose-mail-sms と同じ）。

use crate::contract::SettingsPage;
use crate::error::OpenSystemSettingsError;

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
pub(crate) fn begin_open(
    request_id: u64,
    page: SettingsPage,
) -> Result<(), OpenSystemSettingsError> {
    if call_activity(request_id, page) {
        Ok(())
    } else {
        Err(OpenSystemSettingsError::Internal("open settings present"))
    }
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub(crate) fn begin_open(
    _request_id: u64,
    _page: SettingsPage,
) -> Result<(), OpenSystemSettingsError> {
    Err(OpenSystemSettingsError::Unsupported("open settings"))
}

/// `SystemSettingsHelper.open(Activity, long, int)` を JNI で呼ぶ。`false` は呼び出し失敗
/// （helper が無い場合を含む）。クラスは Activity の class loader で解決する
/// （`fandhe_platform_android::with_activity_class`）。
#[cfg(target_os = "android")]
fn call_activity(id: u64, page: SettingsPage) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.SystemSettingsHelper"),
        |env, activity, class| -> Result<bool, JniCallFailed> {
            let args = [
                jni::objects::JValue::Object(activity),
                jni::objects::JValue::Long(id as jni::sys::jlong),
                jni::objects::JValue::Int(page_code(page)),
            ];
            let result = env.call_static_method(
                class,
                jni::jni_str!("open"),
                jni::jni_sig!((android.app.Activity, long, int) -> boolean),
                &args,
            );
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            Ok(result?.z()?)
        },
    )
    .unwrap_or(false)
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
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_SystemSettingsHelper_nativeOnOpenSettingsResult<
    'caller,
>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::driver::bridge::deliver(
        request_id as u64,
        crate::driver::outcome_from_android_code(code),
    );
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_stub_never_succeeds() {
        for p in [SettingsPage::AppSettings, SettingsPage::Notifications] {
            assert!(matches!(
                begin_open(1, p),
                Err(OpenSystemSettingsError::Unsupported("open settings"))
            ));
        }
    }

    #[test]
    fn page_code_matches_java() {
        assert_eq!(page_code(SettingsPage::AppSettings), 0);
        assert_eq!(page_code(SettingsPage::Notifications), 1);
    }
}
