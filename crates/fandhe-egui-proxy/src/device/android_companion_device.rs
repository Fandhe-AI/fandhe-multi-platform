//! Android 向け付属機器のペアリング（`CompanionDeviceManager`、API 33 以上）の実装
//! （親 #143、Issue #741。契約は Issue #740 の `fandhe_device::companion_device`）。
//! `device/companion_device.rs` の Android 経路（dispatch）から呼ばれる。Java 側
//! `CompanionDeviceHelper` が OS の機器選択画面で BLE 機器を 1 台選ばせ、名前を取って関連付けを外す。
//!
//! 流れ: `pick` → JNI `MainActivity.requestCompanionDevicePick(id)` → Java が UI スレッドで
//! 端末の条件（API・機能・位置情報サービス・Bluetooth）を検査し `associate` →
//! `onAssociationPending` で `nativeTryBeginPresent`（本ファイルの export が
//! `companion_device::try_begin_present` を呼ぶ）により期限切れ・差し替えと排他にしてから
//! 選択画面を起動 → 結果の受領後に `disassociate` を済ませて `nativeOnCompanionDeviceResult(id, kind, name)`
//! → [`map_result`] が契約の結果へ写して `companion_device::complete_from_os` へ 1 回だけ渡す。
//!
//! 結果コードの写像（Java `CompanionDeviceHelper.KIND_*` と同じ値）: `KIND_SELECTED` は
//! `Ok(Selected(名前))`（名前は必ず `DeviceName::from_os` を通す）、`KIND_CANCELLED` は
//! `Ok(Cancelled)`、`KIND_UNSUPPORTED`・`KIND_LOCATION_OFF`・`KIND_BLUETOOTH_OFF`・`KIND_NOT_FOUND`・
//! `KIND_NOT_FOREGROUND`・`KIND_INTERRUPTED` は同名の失敗、`KIND_INTERNAL`（関連付けを外せない等）と
//! 未知のコードは `Internal`（成功へ倒さない）。成功値は bridge 側が表示開始を記録した要求にしか認めない。
//! `TimedOut` は bridge の期限だけが確定する。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・Activity 再生成時の旧要求の破棄
//! （`android.rs` が `reset_pending_tracking` を呼ぶ）と、旧要求の関連付けを外して捨てること。
//! 守らない範囲: API 32 以下・関連付けを残すこと・背景での起動・機器との通信とボンディング・
//! 選択画面の表示中のプロセス死・複数プロセス。機器名と OS のエラー文はログに出さない。
//! 足したのは Manifest の `uses-feature`（`required=false`）だけで、権限・依存は足さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestCompanionDevicePick` を経た後でしか
//! 呼ばれない（`android_in_app_browser.rs` と同じ判断）。

use std::sync::Arc;

use fandhe_device::companion_device::{
    CompanionDeviceFailure, CompanionDeviceOutcome, CompanionDevicePicker, DeviceName,
};

/// 結果コード（Java 側 `CompanionDeviceHelper.KIND_*` と対）。
const KIND_SELECTED: i32 = 0;
const KIND_CANCELLED: i32 = 1;
const KIND_UNSUPPORTED: i32 = 2;
const KIND_LOCATION_OFF: i32 = 3;
const KIND_BLUETOOTH_OFF: i32 = 4;
const KIND_NOT_FOUND: i32 = 5;
const KIND_NOT_FOREGROUND: i32 = 6;
const KIND_INTERRUPTED: i32 = 7;
const KIND_INTERNAL: i32 = 8;

/// 機器名として読む JNI 文字列の UTF-16 単位数の上限（超過は名前なし。最終的な長さは
/// `DeviceName::from_os` が切る）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const NAME_MAX_UNITS: i32 = 4096;

/// Java から届いた結果コードを契約の結果へ写像する。成功側は `KIND_SELECTED` と `KIND_CANCELLED`
/// だけで、未知のコードは `Internal`。名前は `KIND_SELECTED` のときだけ使い、正規化を通す。
fn map_result(
    kind: i32,
    name: Option<String>,
) -> Result<CompanionDeviceOutcome, CompanionDeviceFailure> {
    match kind {
        KIND_SELECTED => Ok(CompanionDeviceOutcome::Selected(
            name.and_then(|n| DeviceName::from_os(&n)),
        )),
        KIND_CANCELLED => Ok(CompanionDeviceOutcome::Cancelled),
        KIND_UNSUPPORTED => Err(CompanionDeviceFailure::Unsupported),
        KIND_LOCATION_OFF => Err(CompanionDeviceFailure::LocationOff),
        KIND_BLUETOOTH_OFF => Err(CompanionDeviceFailure::BluetoothOff),
        KIND_NOT_FOUND => Err(CompanionDeviceFailure::NotFound),
        KIND_NOT_FOREGROUND => Err(CompanionDeviceFailure::NotForeground),
        KIND_INTERRUPTED => Err(CompanionDeviceFailure::Interrupted),
        KIND_INTERNAL => Err(CompanionDeviceFailure::Internal),
        // 未知のコードも成功へ倒さない。
        _ => Err(CompanionDeviceFailure::Internal),
    }
}

/// Android の `CompanionDevicePicker` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn CompanionDevicePicker> {
    Arc::new(AndroidCompanionDevicePicker)
}

struct AndroidCompanionDevicePicker;

impl CompanionDevicePicker for AndroidCompanionDevicePicker {
    /// 受理は「UI スレッドへ渡した」だけを意味し、結果は後から届く。
    fn pick(&self, request_id: u64) -> Result<(), CompanionDeviceFailure> {
        imp::pick(request_id)
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::JValue;

    use super::super::android::JniCallFailed;
    use super::CompanionDeviceFailure;

    /// `MainActivity` のメソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（呼び出しの例外は握る）。
    #[allow(unsafe_code)]
    fn call_activity<T>(
        f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<T, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            f(env, &activity)
        })
        .ok()
    }

    pub(super) fn pick(id: u64) -> Result<(), CompanionDeviceFailure> {
        if crate::android::android_app().is_none() {
            return Err(CompanionDeviceFailure::NotForeground);
        }
        call_activity(|env, a| {
            env.call_method(
                a,
                jni::jni_str!("requestCompanionDevicePick"),
                jni::jni_sig!((long) -> ()),
                &[JValue::Long(id as jni::sys::jlong)],
            )?;
            Ok(())
        })
        .ok_or(CompanionDeviceFailure::Internal)
    }
}

/// `device.rs` が Android 限定で載せるため、ホストでは使われない。呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
mod imp {
    use super::CompanionDeviceFailure;

    pub(super) fn pick(_id: u64) -> Result<(), CompanionDeviceFailure> {
        Err(CompanionDeviceFailure::Unsupported)
    }
}

/// `CompanionDeviceHelper.nativeTryBeginPresent` から呼ばれる JNI エクスポート。選択画面を起動する
/// 直前に呼ばれ、`false`（期限切れ確定済み・古い要求）なら Java は起動しない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_CompanionDeviceHelper_nativeTryBeginPresent<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) -> jni::sys::jboolean {
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let outcome = unowned_env.with_env(|_env| -> Result<bool, jni::errors::Error> {
        Ok(super::companion_device::try_begin_present(
            request_id as u64,
        ))
    });
    matches!(outcome.into_outcome(), jni::Outcome::Ok(true))
}

/// `CompanionDeviceHelper.nativeOnCompanionDeviceResult` から呼ばれる JNI エクスポート。関連付けを
/// 外した後に 1 回だけ呼ばれ、結果を bridge へ渡して次フレームを起こす（契約表 4 節）。
/// 名前は null・読み取り失敗なら名前なしにし、ログには出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_CompanionDeviceHelper_nativeOnCompanionDeviceResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    name: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = super::android::read_ble_string(env, &name, NAME_MAX_UNITS);
        super::companion_device::complete_from_os(request_id as u64, map_result(kind, name));
        Ok(())
    });
    let _ = outcome.into_outcome();
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn map_result_covers_every_code() {
        assert!(matches!(
            map_result(KIND_SELECTED, name("Band")),
            Ok(CompanionDeviceOutcome::Selected(Some(_)))
        ));
        assert_eq!(
            map_result(KIND_CANCELLED, None),
            Ok(CompanionDeviceOutcome::Cancelled)
        );
        use CompanionDeviceFailure as F;
        let table = [
            (KIND_UNSUPPORTED, F::Unsupported),
            (KIND_LOCATION_OFF, F::LocationOff),
            (KIND_BLUETOOTH_OFF, F::BluetoothOff),
            (KIND_NOT_FOUND, F::NotFound),
            (KIND_NOT_FOREGROUND, F::NotForeground),
            (KIND_INTERRUPTED, F::Interrupted),
            (KIND_INTERNAL, F::Internal),
        ];
        for (kind, failure) in table {
            assert_eq!(map_result(kind, None), Err(failure));
        }
    }

    #[test]
    fn unknown_codes_never_become_success() {
        for kind in [9, 99, -1, i32::MAX, i32::MIN] {
            assert_eq!(
                map_result(kind, name("Band")),
                Err(CompanionDeviceFailure::Internal)
            );
        }
    }

    #[test]
    fn selected_without_usable_name_is_nameless() {
        for n in [None, name(""), name("   ")] {
            assert_eq!(
                map_result(KIND_SELECTED, n),
                Ok(CompanionDeviceOutcome::Selected(None))
            );
        }
    }

    #[test]
    fn selected_name_is_normalized() {
        let Ok(CompanionDeviceOutcome::Selected(Some(n))) =
            map_result(KIND_SELECTED, name("a\u{0}b\u{202E}c"))
        else {
            panic!("名前つきの選択になるはず");
        };
        assert_eq!(n.as_str(), "a\u{FFFD}b\u{FFFD}c");
    }

    #[test]
    fn name_is_dropped_for_non_selected() {
        assert_eq!(
            map_result(KIND_CANCELLED, name("Band")),
            Ok(CompanionDeviceOutcome::Cancelled)
        );
        assert_eq!(
            map_result(KIND_INTERNAL, name("Band")),
            Err(CompanionDeviceFailure::Internal)
        );
    }

    #[test]
    fn host_pick_never_succeeds() {
        assert!(implementation().pick(1).is_err());
    }

    #[test]
    fn debug_hides_name() {
        let r = map_result(KIND_SELECTED, name("SecretBand"));
        assert!(!format!("{r:?}").contains("SecretBand"));
    }
}
