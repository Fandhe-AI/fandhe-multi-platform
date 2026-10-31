//! Android 向け車載ディスプレイ（Android Auto）の実装（親 #152、Issue #1117）。
//! `device/car_display.rs` の Android 経路から呼ばれ、effect ドライバのワーカースレッドで
//! [`CarDisplay::set_text`] / [`CarDisplay::state`] として使われる。
//!
//! 流れ: Rust は入力を `validate_text` で検証してから JNI で `MainActivity.carDisplaySetText` を呼ぶ
//! → `CarDisplayScreen.setText` が文言をプロセス内 static へ置き、車載画面が出ていれば main へ
//! `invalidate` を post する。ホスト（Android Auto）は `CarDisplayService` を bind して
//! `CarDisplayScreen.onGetTemplate` が返す `PaneTemplate` を描く。`state` は STARTED 以上の
//! Screen がある間 `Showing`。`Ok` が約束するのは受け付けまで（車載画面が出ていなくても成功）。
//!
//! JNI の署名は Java 側（`MainActivity`）と本ファイルの 2 か所で一致させる:
//! `carDisplaySetText (Ljava/lang/String;)V` / `carDisplayIsShowing ()Z`。
//! Java に `native` 宣言は無く Rust から Java への一方向なので no-op スタブは置かない。
//! 文言の上限などの定数は Rust だけを正とし、Java へは複製しない。
//!
//! `android_app()` が `None`、JNI 失敗、Java 例外は `Internal`（`Unsupported` にするとドライバが
//! ポーリングをやめるため使わない。`NotShowing` にも倒さない）。例外の文言は読まず、文言はログに出さない。
//!
//! Activity 再生成: 本機能は同期の pull 型で要求 id も保留中の要求も持たない（`RequestSlot` /
//! `RequestTracker` は使わない）。文言と表示状態は Activity ではなくプロセス内 static にあるため
//! 再生成で失われず、再生成中の JNI 失敗は `Internal` になるだけで次回のポーリングで回復する。
//! 古い完了が新しい表示を上書きしない世代管理はドライバの `Poller` / `finish_state` が担う。
//!
//! 守る範囲: 1 プロセス内で後から渡した文言が出ること、失敗を成功にしないこと、古い Screen の
//! 停止が新しい Screen の表示中を消さないこと。守らない範囲: プロセス終了後の文言、反映の遅れ、
//! DHU と実車の差、Android Auto の接続そのもの。

use std::sync::Arc;

use fandhe_device::car_display::{validate_text, CarDisplay, CarDisplayState};
use fandhe_device::DeviceError;

const INTERNAL: DeviceError = DeviceError::Internal("car_display");

/// Android の `CarDisplay` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn CarDisplay> {
    Arc::new(AndroidCarDisplay)
}

struct AndroidCarDisplay;

impl CarDisplay for AndroidCarDisplay {
    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        validate_text(text)?;
        if imp::set_text(text) {
            Ok(())
        } else {
            Err(INTERNAL)
        }
    }

    fn state(&self) -> Result<CarDisplayState, DeviceError> {
        state_from(imp::is_showing())
    }
}

/// JNI の結果（失敗は `None`）を契約の状態へ写す。取れないときに `NotShowing` へ倒さない。
fn state_from(v: Option<bool>) -> Result<CarDisplayState, DeviceError> {
    match v {
        Some(true) => Ok(CarDisplayState::Showing),
        Some(false) => Ok(CarDisplayState::NotShowing),
        None => Err(INTERNAL),
    }
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

    pub(super) fn set_text(text: &str) -> bool {
        with_activity(|env, activity| {
            let j_text = env.new_string(text)?;
            env.call_method(
                activity,
                jni::jni_str!("carDisplaySetText"),
                jni::jni_sig!((java.lang.String) -> void),
                &[jni::objects::JValue::Object(&j_text)],
            )?;
            Ok(())
        })
        .is_some()
    }

    pub(super) fn is_showing() -> Option<bool> {
        with_activity(|env, activity| {
            Ok(env
                .call_method(
                    activity,
                    jni::jni_str!("carDisplayIsShowing"),
                    jni::jni_sig!(() -> boolean),
                    &[],
                )?
                .z()?)
        })
    }
}

/// ホストでは成功を返さない（`device.rs` が Android 限定とテストで載せるため）。
#[cfg(not(target_os = "android"))]
mod imp {
    pub(super) fn set_text(_text: &str) -> bool {
        false
    }

    pub(super) fn is_showing() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_from_maps_all_branches() {
        assert!(matches!(
            state_from(Some(true)),
            Ok(CarDisplayState::Showing)
        ));
        assert!(matches!(
            state_from(Some(false)),
            Ok(CarDisplayState::NotShowing)
        ));
        assert!(matches!(
            state_from(None),
            Err(DeviceError::Internal("car_display"))
        ));
    }

    #[test]
    fn invalid_input_is_rejected_before_jni() {
        let d = implementation();
        let long = "あ".repeat(41);
        for s in ["", "a\nb", long.as_str()] {
            assert!(
                matches!(d.set_text(s), Err(DeviceError::InvalidInput(_))),
                "{s:?}"
            );
        }
    }

    #[test]
    fn host_never_succeeds() {
        let d = implementation();
        assert!(matches!(
            d.set_text("x"),
            Err(DeviceError::Internal("car_display"))
        ));
        assert!(matches!(
            d.state(),
            Err(DeviceError::Internal("car_display"))
        ));
    }
}
