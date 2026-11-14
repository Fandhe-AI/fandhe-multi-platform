//! Android 向け Bluetooth Classic（Issue #661。親 #130 の b-android）。範囲はペアリング済み機器の
//! 一覧と、`DISCOVERY_WINDOW_MS` の固定窓で 1 回行う検索のみ。接続・ペアリング・Bluetooth を ON にする
//! 操作・アドレスの表示は範囲外。
//!
//! 呼び出し元: 本 crate の `driver` の Android 経路が、一覧で [`implementation`]
//! （`paired` は egui スレッドから同期で 1 回）、検索で [`present`] を呼ぶ。Activity 再生成時の保留破棄は
//! `android.rs` が `bluetooth_classic::reset_pending_tracking` で行う（配線済み）。dispatch・fallback・
//! `device.rs` の `pub mod`・共通コアの画面には触らない。契約の SSOT は `crate::contract`。
//!
//! # 役割分担
//!
//! - 一覧: `BluetoothClassicHelper.paired(Context)`（static、`(Landroid/content/Context;)Ljava/lang/String;`）が
//!   `BluetoothClassicHelper.paired` へ委譲する。文字列から契約の値への写像は純関数
//!   [`paired_from_android`] に置き、ホストでもテストする
//! - 検索: [`present`] が `BluetoothClassicHelper.request(Activity, long, int)`（static）を JNI で呼ぶだけで即座に戻る。
//!   `startDiscovery`・窓の管理・`cancelDiscovery`・onPause / onDestroy での中断は Java 側
//!   `BluetoothClassicHelper` が main Handler 1 本で行い、結果を本ファイルの JNI エクスポートで
//!   `OBSERVED`（機器 1 件ごと）と `finish`（窓が閉じたとき 1 回）へ届ける
//!
//! # 完了コード（Java と合わせる。写像は `decode_code` に一元化し、Java は値を合わせるだけ）
//!
//! | code | 意味 | 契約上のエラー |
//! |---|---|---|
//! | 0 | 窓が閉じた | 成功 |
//! | 1 | 許可なし | `PermissionDenied` |
//! | 2 | Bluetooth がオフ | `Disabled` |
//! | 3 | 非搭載 | `Unsupported` |
//! | 4 | 中断（onPause / onDestroy） | `Internal` |
//! | 5 | API 30 以下で位置情報がオフ | `Internal` |
//! | 6 | その他（開始できなかった等） | `Internal` |
//!
//! 失敗・非対応・オフを 0 件の成功へ畳まない。cod と rssi は変換の前に値域を検証する。
//! Rust の要求を経ない native 呼び出しは起きないため、no-op スタブは置かない。
//! 機器名・アドレスはログへ出さない（security.md A09）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、オフ・拒否・非対応・中断を成功と見せないこと、
//! レシーバー・タイマー・検索の解放。守らない範囲は `crate::contract` の
//! モジュール doc に加えて、API 31 以上で検索により許可を得るまで一覧が「許可されていません」になること、
//! OS や他アプリが inquiry を早く終えた・間引いた場合の結果の違い、`neverForLocation` が検索結果に
//! 与える影響（実機確認は #650）。
//!
//! 一次情報:
//! <https://developer.android.com/develop/connectivity/bluetooth/find-bluetooth-devices>、
//! <https://developer.android.com/develop/background-work/background-tasks/broadcasts>

use crate::contract::{
    ClassicDevice, DeviceKey, DeviceKind, clamp_name, decode_code, major_from_cod,
    normalize_devices, rssi_from_raw,
};
use crate::error::BluetoothClassicError;

/// Java が項目の頭に置く区切り文字（U+001E）。
const REC: char = '\u{1E}';
/// Java がレコード内の項目をつなぐ区切り文字（U+001F）。
const SEP: char = '\u{1F}';

/// Class of Device は 24 bit。範囲外（未取得の番兵 -1 を含む）は種別を取れなかった扱い。
/// 値域の検証は変換の前に行う（契約表 5 節）。
fn kind_from_android(cod: i32) -> DeviceKind {
    match u32::try_from(cod) {
        Ok(v) if v <= 0xFF_FFFF => major_from_cod(v),
        _ => DeviceKind::Unknown,
    }
}

/// OS から来た 1 件を契約の値へ写す。キーが空なら捨てる（同一機器を識別できないため）。
/// 名前は制御文字と長さを整え、電波強度は範囲外と番兵を `None` にする。
fn device_from_android(
    key: Option<String>,
    name: Option<String>,
    cod: i32,
    rssi: i32,
    paired: bool,
) -> Option<(DeviceKey, ClassicDevice)> {
    let key = DeviceKey::new(key?)?;
    Some((
        key,
        ClassicDevice {
            name: name.and_then(clamp_name),
            kind: kind_from_android(cod),
            paired,
            rssi: rssi_from_raw(rssi),
        },
    ))
}

/// 1 レコード（`address SEP name SEP cod`）を写す。項目数違い・数値でない場合は `None`。
fn parse_paired_record(rec: &str) -> Option<(DeviceKey, ClassicDevice)> {
    let mut it = rec.split(SEP);
    let address = it.next()?;
    let name = it.next()?;
    let cod: i32 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    device_from_android(
        Some(address.to_string()),
        Some(name.to_string()),
        cod,
        i32::MIN,
        true,
    )
}

/// `BluetoothClassicHelper.paired` の文字列を契約の値へ写す。形が不正なら `None`
/// （呼び出し側で `Internal`）。
///
/// - コード単独（0 以外）は `decode_code` のエラー
/// - `0 REC 件数 (REC レコード)*` は、件数とレコード数が一致するときだけ成功
fn paired_from_android(raw: &str) -> Option<Result<Vec<ClassicDevice>, BluetoothClassicError>> {
    let mut it = raw.split(REC);
    let code: i32 = it.next()?.parse().ok()?;
    let Some(count) = it.next() else {
        // コード単独。0（成功）には件数が必要なので不正。
        return decode_code(code).err().map(Err);
    };
    if code != 0 {
        return None;
    }
    let count: usize = count.parse().ok()?;
    let devices = it.map(parse_paired_record).collect::<Option<Vec<_>>>()?;
    (devices.len() == count).then(|| Ok(normalize_devices(devices)))
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::paired_from_android;
    use crate::contract::{BluetoothClassic, ClassicDevice};
    use crate::error::BluetoothClassicError;
    use fandhe_platform_android::JniCallFailed;

    /// Android の `BluetoothClassic` 実装。一覧は同期の JNI 呼び出し、検索は pull 型の
    /// [`super::present`] だけで動かすので `discover` は使わない（macOS と同じ）。
    pub(super) struct AndroidBluetoothClassic;

    impl BluetoothClassic for AndroidBluetoothClassic {
        /// `BluetoothClassicHelper.paired(Context)`（static）を JNI で呼ぶ。`unsafe` は共通層
        /// `fandhe_platform_android::with_activity_class` に閉じている。
        fn paired(&self) -> Result<Vec<ClassicDevice>, BluetoothClassicError> {
            const FAILED: BluetoothClassicError =
                BluetoothClassicError::Internal("bluetooth_classic list");
            let Some(app) = fandhe_platform_android::android_app() else {
                return Err(FAILED);
            };

            let result = fandhe_platform_android::with_activity_class(
                &app,
                jni::jni_str!("com.fandhe.platform.BluetoothClassicHelper"),
                |env, activity, class| -> Result<Option<String>, JniCallFailed> {
                    let ret = env.call_static_method(
                        class,
                        jni::jni_str!("paired"),
                        jni::jni_sig!((android.content.Context) -> java.lang.String),
                        &[jni::objects::JValue::Object(activity)],
                    );
                    let ret = match ret {
                        Ok(v) => v,
                        Err(e) => {
                            // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                            if env.exception_check() {
                                env.exception_clear();
                            }
                            return Err(e.into());
                        }
                    };
                    let jstring = env.cast_local::<jni::objects::JString>(ret.l()?)?;
                    if jstring.is_null() {
                        Ok(None)
                    } else {
                        Ok(Some(jstring.try_to_string(env)?))
                    }
                },
            );

            let raw = result.ok().flatten().ok_or(FAILED)?;
            paired_from_android(&raw).ok_or(FAILED)?
        }

        fn discover(&self) -> Result<Vec<ClassicDevice>, BluetoothClassicError> {
            Err(BluetoothClassicError::Unsupported("bluetooth_classic"))
        }
    }
}

/// Android の `BluetoothClassic` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Box<dyn crate::contract::BluetoothClassic> {
    Box::new(jni_impl::AndroidBluetoothClassic)
}

/// 検索の開始を依頼する。完了は `OBSERVED` / `finish` へ届く（契約は `crate::contract`）。
/// JNI 呼び出しに失敗したら `Err(Internal)`（dispatch が同期失敗として確定する）。`unsafe` は共通層
/// `fandhe_platform_android::with_activity_class` に閉じている。
#[cfg(target_os = "android")]
pub(crate) fn present(
    request_id: u64,
    duration: std::time::Duration,
) -> Result<(), BluetoothClassicError> {
    let duration_ms = i32::try_from(duration.as_millis())
        .map_err(|_| BluetoothClassicError::Internal("bluetooth_classic duration"))?;
    let app = fandhe_platform_android::android_app().ok_or(BluetoothClassicError::Internal(
        "bluetooth_classic app unavailable",
    ))?;

    fandhe_platform_android::with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.BluetoothClassicHelper"),
        |env, activity, class| -> Result<(), jni::errors::Error> {
            env.call_static_method(
                class,
                jni::jni_str!("request"),
                jni::jni_sig!((android.app.Activity, long, int) -> ()),
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(duration_ms),
                ],
            )?;
            Ok(())
        },
    )
    .map_err(|_| BluetoothClassicError::Internal("bluetooth_classic request"))
}

/// アドレス文字列の UTF-16 単位数の上限（`AA:BB:CC:DD:EE:FF` は 17）。
#[cfg(target_os = "android")]
const KEY_MAX_UNITS: i32 = 32;
/// 機器名の UTF-16 単位数の上限（`MAX_NAME_BYTES` 以上。超過分の切り詰めは `clamp_name` が担う）。
#[cfg(target_os = "android")]
const NAME_MAX_UNITS: i32 = 255;

/// `BluetoothClassicHelper.nativeOnClassicFound`（Java 側）から呼ばれる JNI エクスポート関数。
/// 見つかった機器 1 件を `OBSERVED` へ足す（要求 id 不一致は `Observer` が捨てる）。
/// 整形は `device_from_android` が担う。名前・アドレスはログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnBleScanResult` と同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_BluetoothClassicHelper_nativeOnClassicFound<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    address: jni::objects::JString<'caller>,
    name: jni::objects::JString<'caller>,
    cod: jni::sys::jint,
    rssi: jni::sys::jint,
    paired: jni::sys::jboolean,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let key = fandhe_platform_android::read_bounded_string(env, &address, KEY_MAX_UNITS);
        let name = fandhe_platform_android::read_bounded_string(env, &name, NAME_MAX_UNITS);
        if let Some((key, device)) = device_from_android(key, name, cod, rssi, paired) {
            crate::OBSERVED.observe(request_id as u64, key, device);
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

/// `BluetoothClassicHelper.nativeOnClassicFinished`（Java 側）から呼ばれる JNI エクスポート関数。
/// 窓が閉じたとき・中断・失敗で 1 回だけ呼ばれ、コードは `decode_code` で写す。
/// 要求 id が古ければ `finish` が捨てる。次フレームを起こして画面を更新する（契約表 4 節）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBleScanFinished` と同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_BluetoothClassicHelper_nativeOnClassicFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    crate::finish(request_id as u64, decode_code(code));
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: &str = "\u{1E}";
    const S: &str = "\u{1F}";

    fn ok(raw: &str) -> Vec<ClassicDevice> {
        paired_from_android(raw).unwrap().unwrap()
    }

    #[test]
    fn zero_devices_is_empty_success() {
        assert!(ok(&format!("0{R}0")).is_empty());
    }

    #[test]
    fn maps_paired_devices_sorted_by_name() {
        let raw = format!("0{R}2{R}AA:01{S}Zeta{S}1028{R}AA:02{S}Alpha{S}-1");
        let v = ok(&raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name.as_deref(), Some("Alpha"));
        assert_eq!(v[0].kind, DeviceKind::Unknown);
        assert_eq!(v[1].kind, DeviceKind::AudioVideo);
        assert!(v.iter().all(|d| d.paired && d.rssi.is_none()));
    }

    #[test]
    fn failure_codes_map_to_contract_errors() {
        assert!(matches!(
            paired_from_android("1"),
            Some(Err(BluetoothClassicError::PermissionDenied(_)))
        ));
        assert!(matches!(
            paired_from_android("2"),
            Some(Err(BluetoothClassicError::Disabled(_)))
        ));
        assert!(matches!(
            paired_from_android("3"),
            Some(Err(BluetoothClassicError::Unsupported(_)))
        ));
        for code in ["4", "5", "6", "9"] {
            assert!(matches!(
                paired_from_android(code),
                Some(Err(BluetoothClassicError::Internal(_)))
            ));
        }
    }

    #[test]
    fn malformed_is_none() {
        for raw in [
            "".to_string(),
            "x".to_string(),
            "0".to_string(),
            format!("0{R}"),
            format!("0{R}x"),
            format!("0{R}1"),
            format!("0{R}0{R}A{S}n{S}1"),
            format!("0{R}1{R}A{S}n"),
            format!("0{R}1{R}A{S}n{S}x"),
            format!("0{R}1{R}A{S}n{S}1{S}z"),
            format!("0{R}1{R}{S}n{S}1"),
            format!("1{R}0"),
        ] {
            assert!(paired_from_android(&raw).is_none(), "{raw:?}");
        }
    }

    #[test]
    fn kind_checks_range_before_conversion() {
        assert_eq!(kind_from_android(-1), DeviceKind::Unknown);
        assert_eq!(kind_from_android(0x100_0000), DeviceKind::Unknown);
        assert_eq!(kind_from_android(0x0204), DeviceKind::Phone);
    }

    #[test]
    fn device_from_android_cleans_fields() {
        assert!(device_from_android(Some(String::new()), None, 0, 0, false).is_none());
        assert!(device_from_android(None, None, 0, 0, false).is_none());
        let (_, d) = device_from_android(
            Some("AA".into()),
            Some("Pi\u{7}xel".into()),
            0x0204,
            -60,
            false,
        )
        .unwrap();
        assert_eq!(d.name.as_deref(), Some("Pixel"));
        assert_eq!(d.rssi, Some(-60));
        assert_eq!(d.kind, DeviceKind::Phone);
        for raw in [-32768, 127] {
            let (_, d) = device_from_android(Some("AA".into()), None, 0, raw, true).unwrap();
            assert_eq!(d.rssi, None);
        }
    }
}
