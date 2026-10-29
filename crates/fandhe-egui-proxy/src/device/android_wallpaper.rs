//! Android 向け壁紙設定実装（Issue #971、親 #188）。`device/wallpaper.rs` の Android 経路
//! （pull 型 `bridge`）から `begin_set` が呼ばれる。Java 側 `WallpaperHelper` が
//! `WallpaperManager.setStream` でホーム画面・ロック画面へ設定する。
//!
//! 流れ: `begin_set`（UI スレッド）→ JNI `MainActivity.requestWallpaperSet(id, png, target)` →
//! Java が単一スレッドの executor で処理し、結果コードを `nativeOnWallpaperResult(id, target, code)`
//! で返す → [`decode`] で契約の結果へ写像 → `wallpaper::bridge::deliver` が `SLOT` の id と
//! 照合して置く。
//!
//! 結果コードの SSOT は Java 側 `WallpaperHelper.RESULT_*` / `TARGET_*`。
//!
//! 守る範囲: 1 プロセス内の要求順序・非対応/禁止/失敗を成功に畳まないこと・Activity 再生成時の
//! 旧要求の破棄（Rust の `reset_pending_tracking` と Java の last-wins tracker が対）。
//! 守らない範囲: 設定後に利用者や他アプリが壁紙を変えること、Android 12 以降の壁紙変更に伴う
//! Activity 再生成で結果の表示が消えること。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestWallpaperSet` を経た後でしか呼ばれず、
//! feature `device` 無効ビルドではその経路に到達しない（app_badge と同じ）。

use fandhe_device::wallpaper::WallpaperTarget;
use fandhe_device::DeviceError;

/// 設定先コード（Java 側 `WallpaperHelper.TARGET_*` と同値）。
const TARGET_HOME: i32 = 0;
const TARGET_LOCK: i32 = 1;

/// 結果コード（SSOT は Java 側 `WallpaperHelper.RESULT_*`）。
const RESULT_SET: i32 = 0;
const RESULT_UNSUPPORTED: i32 = 1;
const RESULT_DISALLOWED: i32 = 2;

const INTERNAL: DeviceError = DeviceError::Internal("wallpaper set");

/// 設定先を Java へ渡すコードへ写す。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn target_code(target: WallpaperTarget) -> i32 {
    match target {
        WallpaperTarget::Home => TARGET_HOME,
        WallpaperTarget::Lock => TARGET_LOCK,
    }
}

/// Java の結果コードを契約の結果へ写像する。成功でも未知の target、未知のコードは `Internal`
/// （嘘の成功にしない）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn decode(target: i32, code: i32) -> Result<WallpaperTarget, DeviceError> {
    match code {
        RESULT_SET => match target {
            TARGET_HOME => Ok(WallpaperTarget::Home),
            TARGET_LOCK => Ok(WallpaperTarget::Lock),
            _ => Err(INTERNAL),
        },
        RESULT_UNSUPPORTED => Err(DeviceError::Unsupported("wallpaper")),
        RESULT_DISALLOWED => Err(DeviceError::PermissionDenied("wallpaper")),
        _ => Err(INTERNAL),
    }
}

/// OS へ設定を依頼する。`Ok` は受理済みで結果は後から `bridge::deliver` で届く。
/// `Err` は依頼自体の失敗（呼び出し元がその場で `deliver` する）。画像バイト列はログへ出さない。
/// `unsafe` 2 箇所は `android_photo_library::begin_save` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
pub(crate) fn begin_set(
    request_id: u64,
    image: &fandhe_device::photo_library::PngImage,
    target: WallpaperTarget,
) -> Result<(), DeviceError> {
    use super::android::JniCallFailed;
    let present = DeviceError::Internal("wallpaper present");
    let Some(app) = crate::android::android_app() else {
        return Err(present);
    };
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
        let j_png = env.byte_array_from_slice(image.bytes())?;
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        env.call_method(
            &activity,
            jni::jni_str!("requestWallpaperSet"),
            jni::jni_sig!((long, byte[], int) -> ()),
            &[
                jni::objects::JValue::Long(request_id as jni::sys::jlong),
                jni::objects::JValue::Object(&j_png),
                jni::objects::JValue::Int(target_code(target)),
            ],
        )?;
        Ok(())
    })
    .map_err(|_| present)
}

/// `WallpaperHelper.nativeOnWallpaperResult`（Java 側）から呼ばれる JNI エクスポート。
/// 旧要求の遅延到着は `deliver` が id 不一致で捨てる。値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名で、本クレートは cdylib として 1 つの Android 実行単位にのみリンクされる
/// ため他シンボルと衝突しない（既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_WallpaperHelper_nativeOnWallpaperResult<'caller>(
    _env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    target: jni::sys::jint,
    code: jni::sys::jint,
) {
    super::wallpaper::bridge::deliver(request_id as u64, decode(target, code));
    // 入力が無いと egui が描画せず「設定中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_maps_codes() {
        assert!(matches!(decode(0, 0), Ok(WallpaperTarget::Home)));
        assert!(matches!(decode(1, 0), Ok(WallpaperTarget::Lock)));
        assert!(matches!(decode(7, 0), Err(DeviceError::Internal(_))));
        assert!(matches!(decode(0, 1), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            decode(0, 2),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(decode(0, 3), Err(DeviceError::Internal(_))));
        assert!(matches!(decode(0, 99), Err(DeviceError::Internal(_))));
        assert!(matches!(decode(0, -1), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn target_codes_match_java() {
        assert_eq!(target_code(WallpaperTarget::Home), 0);
        assert_eq!(target_code(WallpaperTarget::Lock), 1);
    }
}
