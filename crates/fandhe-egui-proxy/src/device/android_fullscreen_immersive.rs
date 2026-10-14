//! Android 向け全画面表示実装（Issue #1314、親 #1133）。`device/fullscreen_immersive.rs` の
//! Android 経路が [`implementation`] を呼ぶ。Java 側の SSOT は `FullscreenHelper`
//! （結果コード・UI スレッドへの橋渡し・insets からの状態判定）。
//!
//! 切り替え: `request(on)` は JNI `MainActivity.fullscreenSet(boolean)` を同期で呼ぶ。Java 側は
//! UI スレッドで `WindowInsetsControllerCompat.hide/show(systemBars())` を呼び終えるまで
//! 最長 1 秒待つ。`Ok` は「呼び終えた」だけを意味し、全画面になったかは `Fullscreen::is_fullscreen`
//! の読み戻しで確定する（期限 3 秒の判定はドライバ側）。期限切れ後に遅れて適用されることはない。
//!
//! 読み戻し: 既存の insets リスナーが状態（statusBars / navigationBars がともに非表示か）を
//! `nativeOnFullscreenChanged` で push し、[`STATE`] に保持する。`is_fullscreen` は atomic を
//! 読むだけ。-1 は「読めない」で `None`（`false` で代用しない）。変化時は次フレームを起こす。
//!
//! Activity 再生成: Java の `onDestroy` が -1 を送り、新しい Window は通常表示から始まる。
//! 要求は持ち越さない。Rust 側に残った確認待ちはドライバが世代不一致で捨てる。
//!
//! 守る範囲: 1 プロセス内で表示が実際のバー状態に追従すること、失敗・読めない状態を全画面と
//! 見せないこと、期限（1 秒）後に適用しないこと。守らない範囲: スワイプでバーが一時表示される間の
//! 表示ずれ、再生成・背景復帰で全画面を自動再適用しないこと、分割画面等で OS が隠さない場合
//! （期限切れの失敗として出る）、プロセス終了時の後片付け（OS が片付ける）。
//! `PermissionDenied` / `Cancelled` は返さない（権限不要）。

use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::Arc;

use fandhe_device::fullscreen_immersive::Fullscreen;
use fandhe_device::DeviceError;

/// 結果コード（SSOT は Java 側 `FullscreenHelper.RESULT_*`）。0 以外はすべて失敗。
const RESULT_OK: i32 = 0;
#[cfg(test)]
const RESULT_TIMEOUT: i32 = 1;
#[cfg(test)]
const RESULT_FAILED: i32 = 2;

/// 現在の表示状態（-1 = 読めない / 0 = 通常 / 1 = 全画面）。Java 側 `FullscreenHelper.STATE_*` と同値。
static STATE: AtomicI8 = AtomicI8::new(-1);

/// 結果コードを契約の結果へ写す純関数。0 以外と未知の値はすべて `Internal`。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn decode(code: i32) -> Result<(), DeviceError> {
    if code == RESULT_OK {
        Ok(())
    } else {
        Err(DeviceError::Internal("fullscreen-immersive"))
    }
}

/// 生の状態値を契約の `Option<bool>` へ写す純関数。範囲外は読めない扱い。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn state_from_raw(raw: i8) -> Option<bool> {
    match raw {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// 状態を差し替え、値が変わったかを返す。範囲外は -1（読めない）へ正規化する。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn store_state(raw: i32) -> bool {
    let v: i8 = match raw {
        0 => 0,
        1 => 1,
        _ => -1,
    };
    STATE.swap(v, Ordering::SeqCst) != v
}

/// `FullscreenHelper.nativeOnFullscreenChanged(int)` の実体。変化があったときだけ次フレームを起こす
/// （外部変化でも表示が追従する。変化が無ければ起こさない）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// `device/android_url_scheme.rs` の同種エクスポートと同じ根拠。
#[cfg(all(target_os = "android", feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FullscreenHelper_nativeOnFullscreenChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    state: i32,
) {
    if store_state(state) {
        crate::android::request_repaint();
    }
}

#[cfg(target_os = "android")]
struct AndroidFullscreen;

/// `MainActivity.fullscreenSet(boolean)` を JNI で呼ぶ。`None` は呼び出し失敗。
/// `unsafe` 2 箇所は `android_screen_wake_lock.rs::call_activity` と同一の根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
fn call_activity(on: bool) -> Option<i32> {
    use super::android::JniCallFailed;
    let app = crate::android::android_app()?;
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        Ok(env
            .call_method(
                &activity,
                jni::jni_str!("fullscreenSet"),
                jni::jni_sig!((boolean) -> int),
                &[jni::objects::JValue::Bool(on)],
            )?
            .i()?)
    })
    .ok()
}

#[cfg(target_os = "android")]
impl Fullscreen for AndroidFullscreen {
    fn request(&self, on: bool) -> Result<(), DeviceError> {
        match call_activity(on) {
            Some(code) => decode(code),
            None => Err(DeviceError::Internal("fullscreen-immersive")),
        }
    }

    fn is_fullscreen(&self) -> Option<bool> {
        state_from_raw(STATE.load(Ordering::SeqCst))
    }
}

/// Android の `Fullscreen` 実装を返す（`device/fullscreen_immersive.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "android")]
pub fn implementation() -> Arc<dyn Fullscreen> {
    Arc::new(AndroidFullscreen)
}

/// `device.rs` の `cfg(any(android, test))` でホストのテストビルドにも載るための形。
/// dispatch は Android でしか呼ばない。非対応を返し、成功へは倒さない。
#[cfg(not(target_os = "android"))]
pub fn implementation() -> Arc<dyn Fullscreen> {
    Arc::new(fandhe_device::fullscreen_immersive::UnsupportedFullscreen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_ok_only_for_zero() {
        assert!(decode(RESULT_OK).is_ok());
        for c in [RESULT_TIMEOUT, RESULT_FAILED, -1, 99] {
            assert!(matches!(decode(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn constants_match_java_side_contract() {
        // Java 側 `FullscreenHelper.RESULT_*` と同じ値であること。
        assert_eq!((RESULT_OK, RESULT_TIMEOUT, RESULT_FAILED), (0, 1, 2));
    }

    #[test]
    fn state_from_raw_maps_unknown_to_none() {
        assert_eq!(state_from_raw(-1), None);
        assert_eq!(state_from_raw(2), None);
        assert_eq!(state_from_raw(0), Some(false));
        assert_eq!(state_from_raw(1), Some(true));
    }

    #[test]
    fn store_state_reports_change_only() {
        store_state(-1);
        assert!(store_state(1));
        assert!(!store_state(1));
        assert!(store_state(0));
        assert!(store_state(7)); // 範囲外は -1 へ正規化され、0 から変わる
        assert!(!store_state(-1));
    }
}
