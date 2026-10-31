//! Android 向け NFC カードエミュレーション（HCE）の実装（親 #151、Issue #1129）。
//! `device/nfc_card_emulation.rs` の Android 経路から呼ばれ、effect ドライバのワーカースレッドで
//! [`CardEmulation::status`] として使われる。
//!
//! 流れ: OS は `res/xml/apduservice.xml` の AID への SELECT を `NfcCardEmulationService`
//! （`HostApduService`）へ振り、サービスが固定の応答を返して `NfcCardEmulationHelper.recordResponse` が
//! プロセス内 static に回数と時刻を記録する。Rust は `MainActivity.nfcCardEmulationStatus(aid)` を
//! JNI で呼び、`long[5] = [state, nfcEnabled, defaultForAid, responses, sinceLastMs]`
//! （配置の正は Java の `NfcCardEmulationHelper`）を [`decode_status`] で契約の型へ写す。
//!
//! JNI の署名は Java 側（`MainActivity`）と本ファイルの 2 か所で一致させる:
//! `nfcCardEmulationStatus (Ljava/lang/String;)[J`。Java に `native` 宣言は無く Rust から Java への
//! 一方向なので no-op スタブは置かない。AID の正は `fandhe_device::nfc_card_emulation::AID` だけで、
//! `apduservice.xml` への複製はテストで固定する。
//!
//! `android_app()` が `None`、JNI 失敗、Java 例外、配列の形の不一致は `Internal`（`Unsupported` に
//! するとドライバがポーリングをやめるため使わない）。例外の文言は読まず、ログにも出さない。
//!
//! Activity 再生成: 本機能は同期の pull 型で要求 id も保留中の要求も持たない（`RequestSlot` /
//! `RequestTracker` は使わない）。回数と時刻は Activity ではなくプロセス内 static にあるため再生成で
//! 失われず、再生成中の JNI 失敗は `Internal` になるだけで次回のポーリングで回復する。古い完了が
//! 新しい要求を上書きしない世代管理はドライバが担う。
//!
//! 守る範囲: 1 プロセス内で回数と時刻が同じ lock で一貫すること、失敗を 0 回や成功にしないこと。
//! 守らない範囲: プロセス終了で回数が 0 に戻ること、他アプリが同じ AID を登録したときの振り先、
//! 画面オフ・ロック中、リーダーの再試行による重複計数、ポーリング間隔ぶんの遅れ。

use std::sync::Arc;

use fandhe_device::nfc_card_emulation::{CardEmulation, HceStatus};
use fandhe_device::DeviceError;

const INTERNAL: DeviceError = DeviceError::Internal("nfc_card_emulation");

/// JNI が返す配列の長さ（Java の `NfcCardEmulationHelper.status` の配置と一致させる）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const STATUS_LEN: usize = 5;

/// Android の `CardEmulation` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn CardEmulation> {
    Arc::new(AndroidCardEmulation)
}

struct AndroidCardEmulation;

impl CardEmulation for AndroidCardEmulation {
    fn status(&self) -> Result<HceStatus, DeviceError> {
        let raw = imp::read_status(fandhe_device::nfc_card_emulation::AID);
        decode_status(raw.as_ref().map(|a| &a[..]))
    }
}

/// JNI の `long[5]`（取れなければ `None`）を契約の状態へ写す。取れない・形が違う・値域外は
/// `Internal`（0 回や成功へ倒さない）。応答 0 回なら `since_last_ms=None`（番兵値を使わない）。
fn decode_status(raw: Option<&[i64]>) -> Result<HceStatus, DeviceError> {
    let v = raw.ok_or(INTERNAL)?;
    let [state, nfc, default, responses, since] = *v else {
        return Err(INTERNAL);
    };
    let flag = |x: i64| match x {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(INTERNAL),
    };
    match state {
        0 => {}
        1 => return Err(DeviceError::Unsupported("nfc_card_emulation")),
        _ => return Err(INTERNAL),
    }
    let responses = u64::try_from(responses).map_err(|_| INTERNAL)?;
    let since = u64::try_from(since).map_err(|_| INTERNAL)?;
    Ok(HceStatus {
        nfc_enabled: flag(nfc)?,
        default_for_aid: flag(default)?,
        responses,
        since_last_ms: (responses > 0).then_some(since),
    })
}

#[cfg(target_os = "android")]
mod imp {
    /// Activity インスタンスへ `f` を実行する（唯一の `unsafe` 集約点）。失敗は `None`。
    /// Java 例外は clear し、文言は読まない。
    #[allow(unsafe_code)]
    fn with_activity<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JObject,
        ) -> Result<T, super::super::android::JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `android_home_widget` の `with_activity` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` で、`app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない
            // （`app` の生存中有効）。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let result = f(env, &activity);
            if result.is_err() && env.exception_check() {
                // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                env.exception_clear();
            }
            result
        })
        .ok()
    }

    pub(super) fn read_status(aid: &str) -> Option<[i64; super::STATUS_LEN]> {
        with_activity(|env, activity| {
            use jni::objects::JLongArray;
            let j_aid = env.new_string(aid)?;
            let ret = env.call_method(
                activity,
                jni::jni_str!("nfcCardEmulationStatus"),
                jni::jni_sig!((java.lang.String) -> long[]),
                &[jni::objects::JValue::Object(&j_aid)],
            )?;
            let arr = env.cast_local::<JLongArray>(ret.l()?)?;
            if arr.is_null() || arr.len(env)? != super::STATUS_LEN {
                return Ok(None);
            }
            let mut v = [0i64; super::STATUS_LEN];
            arr.get_region(env, 0, &mut v)?;
            Ok(Some(v))
        })
        .flatten()
    }
}

/// ホストでは成功を返さない（`device.rs` が Android 限定とテストで載せるため）。
#[cfg(not(target_os = "android"))]
mod imp {
    pub(super) fn read_status(_aid: &str) -> Option<[i64; super::STATUS_LEN]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::nfc_card_emulation::AID;

    #[test]
    fn decode_ok_without_responses_has_no_since() {
        let s = decode_status(Some(&[0, 1, 0, 0, 0])).unwrap();
        assert!(s.nfc_enabled && !s.default_for_aid);
        assert_eq!((s.responses, s.since_last_ms), (0, None));
    }

    #[test]
    fn decode_ok_with_responses_has_since() {
        let s = decode_status(Some(&[0, 0, 1, 3, 250])).unwrap();
        assert!(!s.nfc_enabled && s.default_for_aid);
        assert_eq!((s.responses, s.since_last_ms), (3, Some(250)));
    }

    #[test]
    fn decode_state_one_is_unsupported() {
        assert!(matches!(
            decode_status(Some(&[1, 0, 0, 0, 0])),
            Err(DeviceError::Unsupported("nfc_card_emulation"))
        ));
    }

    #[test]
    fn decode_bad_shapes_are_internal() {
        let bad: [Option<&[i64]>; 7] = [
            None,
            Some(&[0, 0, 0, 0]),
            Some(&[0, 0, 0, 0, 0, 0]),
            Some(&[2, 0, 0, 0, 0]),
            Some(&[0, 2, 0, 0, 0]),
            Some(&[0, 0, 0, -1, 0]),
            Some(&[0, 0, 0, 1, -1]),
        ];
        for b in bad {
            assert!(matches!(
                decode_status(b),
                Err(DeviceError::Internal("nfc_card_emulation"))
            ));
        }
    }

    #[test]
    fn host_never_succeeds() {
        assert!(matches!(
            implementation().status(),
            Err(DeviceError::Internal("nfc_card_emulation"))
        ));
    }

    /// `apduservice.xml` の AID が Rust の定数とずれていないこと。
    #[test]
    fn apduservice_xml_matches_aid() {
        let xml = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../platforms/android/app/src/main/res/xml/apduservice.xml"
        ));
        assert!(xml.contains(&format!("android:name=\"{AID}\"")));
        assert!(xml.contains("android:category=\"other\""));
    }
}
