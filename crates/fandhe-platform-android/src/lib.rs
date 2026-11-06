//! Android の共通層（TASK-21.2、Issue #1166）。
//!
//! 機能 crate が egui に依存しないよう、Android の機能実装が共通で要る 3 点だけを
//! レンダラ非依存で切り出した crate。依存方向は `fandhe-egui-proxy`（レンダラ実装）→ 本 crate
//! の一方向で、egui / eframe / winit は参照しない。
//!
//! - [`set_android_app`] / [`android_app`]: `android_main` が受け取る [`AndroidApp`] の保持
//! - [`JniCallFailed`]: JNI 呼び出し中のエラーの受け皿
//! - [`with_activity`]: JavaVM への attach と Activity 取得の定型（`unsafe` はここに閉じる）
//! - [`with_activity_class`]: 上に加え、機能の Java クラスを解決して static メソッドを呼ぶ定型
//!
//! Java 側の共有クラス（`RequestTracker`・`FeatureLifecycleProvider`、`android/java`）も本 crate が持つ。
//! 機能 crate の宣言ではなくこの crate の宣言（`[package.metadata.fandhe]`）が srcDirs に載せる。
//!
//! Android 以外では空（`cfg` で全体を除外）。

#![cfg(target_os = "android")]
#![deny(unsafe_code)]

use std::sync::Mutex;

pub use android_activity::AndroidApp;

/// `android_main` が受け取った [`AndroidApp`] を保持し、JNI 呼び出しや `internal_data_path` が
/// 読み出せるようにする。
///
/// `OnceLock` ではなく `Mutex` にするのは、Manifest の `configChanges` で止められない再生成
/// （例: 壁紙によるテーマ色変更）で OS が `Activity` を破棄・再生成すると `android_main` が
/// 新しい [`AndroidApp`] で再度呼ばれるため。[`set_android_app`] が呼ばれるたびに差し替え、
/// 破棄済み `Activity` を指す古い値を残さない。
///
/// 計測専用ビルド（`fandhe-egui-proxy` の `frame-metrics`）は別の `android_main` を使い
/// [`set_android_app`] を呼ばないので、[`android_app`] は常に `None` を返す。
static ANDROID_APP: Mutex<Option<AndroidApp>> = Mutex::new(None);

/// [`ANDROID_APP`] の保持内容を差し替える。`android_main` が毎回呼ぶ（Activity 再生成のたびに
/// `android_main` が再実行されるため、初回だけでなく毎回上書きする）。
pub fn set_android_app(app: AndroidApp) {
    *ANDROID_APP.lock().unwrap_or_else(|e| e.into_inner()) = Some(app);
}

/// [`ANDROID_APP`] を読む。`AndroidApp` は `Arc` 内部実装で `Clone` が安価なため所有値を返す。
/// `android_main` 実行前は `None`。
pub fn android_app() -> Option<AndroidApp> {
    ANDROID_APP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// JNI 呼び出し中のエラーを一時的にまとめる型。`jni` の `attach_current_thread` のクロージャは
/// `From<jni::errors::Error>` を満たすエラー型を要求するため、その変換先として用意する
/// （詳細は持たない。呼び出し側が契約上のエラー種別へ写す）。
pub struct JniCallFailed;

impl From<jni::errors::Error> for JniCallFailed {
    fn from(_: jni::errors::Error) -> Self {
        JniCallFailed
    }
}

/// 現在のスレッドを JavaVM に attach し、`app` の Activity を `f` に渡す。
///
/// `Activity` は未所有の借用で、`f` の外へ持ち出せない（delete されない）。`app` を借用で
/// 受けるのは、JavaVM と Activity のポインタが `app` の生存期間中だけ有効という根拠を
/// 型に現すため。attach 自体の失敗は `E::from` 経由で返る。
#[allow(unsafe_code)]
pub fn with_activity<T, E: From<jni::errors::Error>>(
    app: &AndroidApp,
    f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, E>,
) -> Result<T, E> {
    // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
    // `app` は本呼び出しの間（借用により）生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env: &mut jni::Env| -> Result<T, E> {
        let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
        // SAFETY: `activity_as_ptr()` は `AndroidApp` が保持する未所有のグローバル参照で、
        // `app` の生存期間中有効。`Cast` は所有権を持たないラッパーのため delete しない
        // （jni の `as_cast_raw` 文書「Don't wrap as Global/Auto」参照）。
        let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
        f(env, &activity)
    })
}

/// [`with_activity`] に加えて、機能の Java クラスをアプリの class loader で解決し、`f` に渡す。
///
/// 機能の Rust は利用者の `MainActivity` のメソッドを呼ばず、機能 crate の Java が持つ static メソッドを
/// 呼ぶ（Activity は第 1 引数で渡す）。これで利用者の Activity に受け口を書かせずに済む。
/// ネイティブスレッドからの `FindClass` はシステム class loader を使いアプリのクラスを見つけられないため、
/// Activity の class loader（`LoaderContext::FromObject`）で解決する。`class_name` は呼び出し側の固定の
/// 文字列リテラル（`"com.fandhe.platform.X"` のドット区切り）だけを渡し、外部入力を混ぜない。
/// 呼び出し元は各機能 crate の `android.rs`（効果ドライバのワーカースレッド）。
pub fn with_activity_class<T, E: From<jni::errors::Error>>(
    app: &AndroidApp,
    class_name: &jni::strings::JNIStr,
    f: impl FnOnce(&mut jni::Env, &jni::objects::JObject, &jni::objects::JClass) -> Result<T, E>,
) -> Result<T, E> {
    with_activity(app, |env, activity| {
        let class =
            jni::refs::LoaderContext::FromObject(activity).load_class(env, class_name, false)?;
        f(env, activity, &class)
    })
}
