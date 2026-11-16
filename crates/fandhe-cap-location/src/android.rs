//! Android 向け位置情報（Issue #890。ジオフェンス #1058 も共用）の JNI 橋渡し。
//!
//! 呼び出し元: `driver.rs`（開始要求）。`SharedLocation` は継続 push 型で request/response の mailbox を持たない
//! ため、開始要求だけ Rust → Java の片道 JNI 呼び出し（static の `LocationHelper.requestLocationUpdates(Activity,
//! boolean)`）で行い、実測位は Java 側 `LocationListener` から `nativeOnLocationChanged` /
//! `nativeOnLocationFailure` / `nativeOnLocationRetrying` で継続的に届いて [`crate::driver::shared_location_sink`]
//! へ直接 push される。座標はログへ出力しない（security.md A09）。
//! 受け口のシンボル名は Helper のクラス名（`com.fandhe.platform.LocationHelper`）に従う。

use crate::contract::{Fix, LocationFailure};

/// Java 側 `LocationHelper.requestLocationUpdates(Activity, boolean)` を呼び、位置情報の測位開始を要求する。
/// `precise` は精密位置（FINE）が要るか（ジオフェンスは真、位置情報画面は偽で COARSE のみ要求、最小権限）。
/// 結果を待たずに即座に戻る。戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。
/// 呼び出し元 `driver.rs` が `push_failure(LocationFailure::Failed)` へ畳む）。
/// JavaVM と Activity の取得は `with_activity_class` に任せ、ここは `unsafe` を持たない。
pub(crate) fn request_location_updates(precise: bool) -> bool {
    use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
    use jni::objects::JValue;
    let Some(app) = android_app() else {
        return false;
    };
    with_activity_class(
        &app,
        jni::jni_str!("com.fandhe.platform.LocationHelper"),
        |env, activity, class| -> Result<(), JniCallFailed> {
            env.call_static_method(
                class,
                jni::jni_str!("requestLocationUpdates"),
                jni::jni_sig!((android.app.Activity, boolean) -> ()),
                &[JValue::Object(activity), JValue::Bool(precise)],
            )?;
            Ok(())
        },
    )
    .is_ok()
}

/// `nativeOnLocationFailure` の `code` を [`LocationFailure`] へ写す純関数（`cfg` 無しでホストからテストできる）。
/// 対応表は `LocationHelper.nativeOnLocationFailure` 宣言コメントと対で決める: 1=権限拒否／2=サービス無効／
/// それ以外=その他の失敗（未知コードもここへ丸め、panic しない。security.md A03）。
fn decode_location_failure(code: jni::sys::jint) -> LocationFailure {
    match code {
        1 => LocationFailure::PermissionDenied,
        2 => LocationFailure::ServiceDisabled,
        _ => LocationFailure::Failed,
    }
}

/// `LocationHelper.nativeOnLocationChanged`（Java 側）から呼ばれる JNI エクスポート関数。
///
/// [`crate::driver::shared_location_sink`] へ直接 `push_fix` する（`shared_location()` を呼ぶと未初期化時の起動処理
/// （`request_location_updates` の初回呼び出し）が再入するため使わない）。`JNIEnv`/`jclass` は使わず、
/// 生ポインタへのアクセスを行わないため関数本体に `unsafe` 操作は含まない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` により Rust の名前修飾を無効化するが、
/// シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LocationHelper_nativeOnLocationChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    latitude: f64,
    longitude: f64,
    accuracy: f32,
) {
    crate::driver::shared_location_sink().push_fix(Fix {
        latitude_deg: latitude,
        longitude_deg: longitude,
        accuracy_m: accuracy as f64,
    });
    fandhe_runtime::request_repaint();
}

/// `LocationHelper.nativeOnLocationFailure`（Java 側）から呼ばれる JNI エクスポート関数。
/// [`decode_location_failure`] で変換して `push_failure` する。
///
/// # Safety
/// `nativeOnLocationChanged` と同一の根拠（生ポインタへのアクセスを行わない）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LocationHelper_nativeOnLocationFailure(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    code: jni::sys::jint,
) {
    crate::driver::shared_location_sink().push_failure(decode_location_failure(code));
    fandhe_runtime::request_repaint();
}

/// `LocationHelper.nativeOnLocationRetrying`（Java 側）から呼ばれる JNI エクスポート関数。
/// 権限再取得・provider 再有効化で測位を再開する瞬間に呼ばれ、共有状態を `reset_to_waiting` で「未取得」へ戻す
/// （coarse/network provider では再登録後も新しい fix が届くとは限らず、無いと古い Denied/Disabled 表示が残留する）。
///
/// # Safety
/// `nativeOnLocationChanged` と同一の根拠（生ポインタへのアクセスを行わない）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_LocationHelper_nativeOnLocationRetrying(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    crate::driver::shared_location_sink().reset_to_waiting();
    fandhe_runtime::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::decode_location_failure;
    use crate::contract::LocationFailure;

    #[test]
    fn decode_location_failure_maps_known_codes() {
        assert_eq!(
            decode_location_failure(1),
            LocationFailure::PermissionDenied
        );
        assert_eq!(decode_location_failure(2), LocationFailure::ServiceDisabled);
        assert_eq!(decode_location_failure(3), LocationFailure::Failed);
    }

    #[test]
    fn decode_location_failure_maps_unknown_code_to_failed() {
        assert_eq!(decode_location_failure(99), LocationFailure::Failed);
    }
}
