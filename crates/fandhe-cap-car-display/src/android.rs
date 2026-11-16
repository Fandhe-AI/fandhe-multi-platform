//! Android 向け車載ディスプレイ（Android Auto）の実装（親 #152、Issue #1117、機能 crate 化 #1335）。
//! `driver.rs` の Android 経路から呼ばれ、effect ドライバのワーカースレッドで
//! [`CarDisplay::set_text`] / [`CarDisplay::state`] として使われる。
//!
//! 流れ: Rust は入力を `validate_text` で検証してから JNI で `CarDisplayScreen.setText`（static）を呼ぶ
//! → 文言をプロセス内 static へ置き、車載画面が出ていれば main へ `invalidate` を post する。
//! ホスト（Android Auto）は `CarDisplayService` を bind して `CarDisplayScreen.onGetTemplate` が返す
//! `PaneTemplate` を描く。`state` は STARTED 以上の Screen がある間 `Showing`。`Ok` が約束するのは
//! 受け付けまで（車載画面が出ていなくても成功）。
//!
//! クラスは `fandhe_platform_android::with_activity_class` でアプリの class loader から解決する
//! （利用者の `MainActivity` に受け口を書かせない）。JNI の署名は Java 側（`CarDisplayScreen`）と
//! 本ファイルの 2 か所で一致させる: `setText (Ljava/lang/String;)V` / `isShowing ()Z`。
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

use crate::contract::{CarDisplay, CarDisplayState, validate_text};
use crate::error::CarDisplayError;

const INTERNAL: CarDisplayError = CarDisplayError::Internal;

/// Android の `CarDisplay` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn CarDisplay> {
    Arc::new(AndroidCarDisplay)
}

struct AndroidCarDisplay;

impl CarDisplay for AndroidCarDisplay {
    fn set_text(&self, text: &str) -> Result<(), CarDisplayError> {
        validate_text(text)?;
        if imp::set_text(text) {
            Ok(())
        } else {
            Err(INTERNAL)
        }
    }

    fn state(&self) -> Result<CarDisplayState, CarDisplayError> {
        state_from(imp::is_showing())
    }
}

/// JNI の結果（失敗は `None`）を契約の状態へ写す。取れないときに `NotShowing` へ倒さない。
fn state_from(v: Option<bool>) -> Result<CarDisplayState, CarDisplayError> {
    match v {
        Some(true) => Ok(CarDisplayState::Showing),
        Some(false) => Ok(CarDisplayState::NotShowing),
        None => Err(INTERNAL),
    }
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};

    /// `CarDisplayScreen`（機能 crate の Java、プロセス内 static）の static メソッドへ `f` を実行する。
    /// 失敗は `None`。Java 例外は共通層が clear し、文言は読まない。
    fn with_screen<T>(
        f: impl FnOnce(
            &mut jni::Env,
            &jni::objects::JObject,
            &jni::objects::JClass,
        ) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.CarDisplayScreen"),
            f,
        )
        .ok()
    }

    pub(super) fn set_text(text: &str) -> bool {
        with_screen(|env, _activity, class| {
            let j_text = env.new_string(text)?;
            env.call_static_method(
                class,
                jni::jni_str!("setText"),
                jni::jni_sig!((java.lang.String) -> void),
                &[jni::objects::JValue::Object(&j_text)],
            )?;
            Ok(())
        })
        .is_some()
    }

    pub(super) fn is_showing() -> Option<bool> {
        with_screen(|env, _activity, class| {
            Ok(env
                .call_static_method(
                    class,
                    jni::jni_str!("isShowing"),
                    jni::jni_sig!(() -> boolean),
                    &[],
                )?
                .z()?)
        })
    }
}

/// ホストでは成功を返さない（ホストの `cargo test` でも読み込まれるため）。
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
        assert!(matches!(state_from(None), Err(CarDisplayError::Internal)));
    }

    #[test]
    fn invalid_input_is_rejected_before_jni() {
        let d = implementation();
        let long = "あ".repeat(41);
        for s in ["", "a\nb", long.as_str()] {
            assert!(
                matches!(d.set_text(s), Err(CarDisplayError::InvalidInput(_))),
                "{s:?}"
            );
        }
    }

    #[test]
    fn host_never_succeeds() {
        let d = implementation();
        assert!(matches!(d.set_text("x"), Err(CarDisplayError::Internal)));
        assert!(matches!(d.state(), Err(CarDisplayError::Internal)));
    }
}
