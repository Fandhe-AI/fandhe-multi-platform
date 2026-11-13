//! Android 向け NFC カードエミュレーション（HCE）の実装（親 #151、Issue #1129）。
//! `lib.rs` の Android 経路（`platform_nfc_card_emulation`）から呼ばれ、effect ドライバのワーカースレッドで
//! [`CardEmulation::status`] として使われる。
//!
//! 流れ: OS は `android/res/xml/apduservice.xml` の AID への SELECT を `NfcCardEmulationService`
//! （`HostApduService`）へ振り、サービスが固定の応答を返して `NfcCardEmulationHelper.recordResponse` が
//! プロセス内 static に回数と時刻を記録する。Rust は `NfcCardEmulationHelper.status(Context, aid)`（static。
//! 利用者の `MainActivity` に受け口は要らない、#1257）を JNI で呼び、`long[5] = [state, nfcEnabled, defaultForAid, responses, sinceLastMs]`
//! （配置の正は Java の `NfcCardEmulationHelper`）を [`decode_status`] で契約の型へ写す。
//!
//! JNI の署名は Java 側（`NfcCardEmulationHelper`）と本ファイルの 2 か所で一致させる:
//! `status (Landroid/content/Context;Ljava/lang/String;)[J`。Java に `native` 宣言は無く Rust から Java への
//! 一方向なので no-op スタブは置かない。AID の正は [`crate::AID`] だけで、
//! `apduservice.xml` への複製はテストで固定する。AndroidApp の取得と JavaVM への attach は
//! `fandhe-platform-android` の `with_activity` に任せ、本ファイルに `unsafe` は無い。
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

use crate::{CardEmulation, CardEmulationError, HceStatus};

/// JNI が返す配列の長さ（Java の `NfcCardEmulationHelper.status` の配置と一致させる）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const STATUS_LEN: usize = 5;

/// Android の `CardEmulation` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn CardEmulation> {
    Arc::new(AndroidCardEmulation)
}

struct AndroidCardEmulation;

impl CardEmulation for AndroidCardEmulation {
    fn status(&self) -> Result<HceStatus, CardEmulationError> {
        let raw = imp::read_status(crate::AID);
        decode_status(raw.as_ref().map(|a| &a[..]))
    }
}

/// JNI の `long[5]`（取れなければ `None`）を契約の状態へ写す。取れない・形が違う・値域外は
/// `Internal`（0 回や成功へ倒さない）。応答 0 回なら `since_last_ms=None`（番兵値を使わない）。
fn decode_status(raw: Option<&[i64]>) -> Result<HceStatus, CardEmulationError> {
    const INTERNAL: CardEmulationError = CardEmulationError::Internal;
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
        1 => return Err(CardEmulationError::Unsupported),
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
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// `NfcCardEmulationHelper.status` を呼んで `long[5]` を読む。失敗は `None`。
    /// Java 例外は clear し、文言は読まない。
    pub(super) fn read_status(aid: &str) -> Option<[i64; super::STATUS_LEN]> {
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.NfcCardEmulationHelper"),
            |env, activity, class| -> Result<_, JniCallFailed> {
                let result = (|| {
                    use jni::objects::JLongArray;
                    let j_aid = env.new_string(aid)?;
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("status"),
                        jni::jni_sig!((android.content.Context, java.lang.String) -> long[]),
                        &[
                            jni::objects::JValue::Object(activity),
                            jni::objects::JValue::Object(&j_aid),
                        ],
                    )?;
                    let arr = env.cast_local::<JLongArray>(ret.l()?)?;
                    if arr.is_null() || arr.len(env)? != super::STATUS_LEN {
                        return Ok(None);
                    }
                    let mut v = [0i64; super::STATUS_LEN];
                    arr.get_region(env, 0, &mut v)?;
                    Ok::<_, jni::errors::Error>(Some(v))
                })();
                if result.is_err() && env.exception_check() {
                    // 例外は自動クリアされない（jni 0.22）。後続 JNI に影響させない。
                    env.exception_clear();
                }
                Ok(result?)
            },
        )
        .ok()
        .flatten()
    }
}

/// ホストでは成功を返さない（`lib.rs` が Android 限定とテストで載せるため）。
#[cfg(not(target_os = "android"))]
mod imp {
    pub(super) fn read_status(_aid: &str) -> Option<[i64; super::STATUS_LEN]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AID;

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
        assert_eq!(
            decode_status(Some(&[1, 0, 0, 0, 0])),
            Err(CardEmulationError::Unsupported)
        );
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
            assert_eq!(decode_status(b), Err(CardEmulationError::Internal));
        }
    }

    #[test]
    fn host_never_succeeds() {
        assert_eq!(implementation().status(), Err(CardEmulationError::Internal));
    }

    /// `apduservice.xml` の AID が Rust の定数とずれていないこと。
    #[test]
    fn apduservice_xml_matches_aid() {
        let xml = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/android/res/xml/apduservice.xml"
        ));
        assert!(xml.contains(&format!("android:name=\"{AID}\"")));
        assert!(xml.contains("android:category=\"other\""));
    }
}
