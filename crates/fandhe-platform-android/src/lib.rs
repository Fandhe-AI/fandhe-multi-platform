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
//! - [`window_insets_px`]: `HostInsets`（Java）が decorView の insets リスナーから送る物理 px の読み出し（#1327）
//!
//! Java 側の共有クラス（`RequestTracker`・`FeatureLifecycleProvider`・`HostInsets`・`HostInput`、`android/java`）も本 crate が持つ。
//! 機能 crate の宣言ではなくこの crate の宣言（`[package.metadata.fandhe]`）が srcDirs に載せる。
//!
//! Android 以外では空（`cfg` で全体を除外）。

#![cfg(target_os = "android")]
#![deny(unsafe_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

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
///
/// `f`（または Activity 取得）が `Err` を返したとき、Java 例外が pending なら `exception_clear` で
/// 片付けてから戻る（pending のまま同じスレッドの次の JNI 呼び出しへ進ませない）。全機能 crate が
/// 通る入口なので、各機能の `?` 離脱をここで一括して受ける。`exception_clear` は `()` を返し失敗しない
/// （jni 0.22）。例外の文言は在席情報等を含みうるため `exception_describe` は呼ばない。
/// jni 0.22 の `attach_current_thread` も既定方針で戻り際に例外を回収するが、版の既定に依存しないよう明示する。
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
        let result = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity) }
            .map_err(E::from)
            .and_then(|activity| f(env, &activity));
        if result.is_err() && env.exception_check() {
            env.exception_clear();
        }
        result
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

/// JNI の文字列を、UTF-16 単位数が `max_units` 以内のときだけ Rust の `String` へ複製する
/// （null・超過・読み取り失敗は `None`）。OS や他アプリ由来の文字列は信頼境界の外なので、複製前に長さで弾く。
/// 各機能 crate の JNI コールバック（`nativeOn*`）が受けた `String` の読み取りに使う。
pub fn read_bounded_string(
    env: &mut jni::Env,
    value: &jni::objects::JString,
    max_units: i32,
) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let units = env
        .call_method(
            value,
            jni::jni_str!("length"),
            jni::jni_sig!(() -> int),
            &[],
        )
        .and_then(|v| v.i())
        .ok()?;
    if units > max_units {
        return None;
    }
    value.try_to_string(env).ok()
}

/// `HostInsets`（Java）が JNI 経由で書き込む window insets（物理 px、systemBars | displayCutout）。
///
/// `AndroidApp::content_rect()` は GameActivity 経路（android-activity 0.6.1）で insets を反映せず
/// 常に画面全体を返すため（Issue #579 の実機確認）、Java 側リスナー → JNI で受ける。呼び出し元は
/// `fandhe-egui-proxy`（描画の余白）と safe-area-insets の機能（#1327 で旧 `MainActivity` 経路から移した）。
/// 4 本を個別に読み書きするため組としての原子性は無い。UI スレッドの書き込み中に描画スレッドが読むと
/// 新旧混在になり得るが、ずれは最大 1 フレームで次の読み出しで自己修正される（見た目のみ）。
static INSET_LEFT: AtomicI32 = AtomicI32::new(0);
static INSET_TOP: AtomicI32 = AtomicI32::new(0);
static INSET_RIGHT: AtomicI32 = AtomicI32::new(0);
static INSET_BOTTOM: AtomicI32 = AtomicI32::new(0);
/// insets を 1 回でも受け取ったか。初期値 0 と「本当に 0」を区別する印（Issue #446）。
static INSET_RECEIVED: AtomicBool = AtomicBool::new(false);

/// 受信済みなら insets を物理 px の `[top, right, bottom, left]` で返し、未受信なら `None`。
// ponytail: 4 本の atomic を別々に読むため組は 1 回混ざり得る。組が要るなら `Mutex<Option<[i32; 4]>>` へ。
pub fn window_insets_px() -> Option<[i32; 4]> {
    if !INSET_RECEIVED.load(Ordering::Acquire) {
        return None;
    }
    Some([
        INSET_TOP.load(Ordering::Relaxed),
        INSET_RIGHT.load(Ordering::Relaxed),
        INSET_BOTTOM.load(Ordering::Relaxed),
        INSET_LEFT.load(Ordering::Relaxed),
    ])
}

/// `HostInsets.nativeOnWindowInsetsChanged`（Java 側）から呼ばれる JNI エクスポート関数。
/// シンボル名はパッケージ `com.fandhe.platform` とクラス `HostInsets` に固定で紐づく
/// （Java 宣言の署名 `(int, int, int, int) -> void` と 2 か所で一致させる）。
///
/// `JNIEnv`/`jclass` は使わず受けた値を static へ store するだけで、関数本体に `unsafe` 操作は無い。
/// `#[unsafe(no_mangle)]` はこの 1 関数に閉じる（`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_HostInsets_nativeOnWindowInsetsChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) {
    INSET_LEFT.store(left, Ordering::Relaxed);
    INSET_TOP.store(top, Ordering::Relaxed);
    INSET_RIGHT.store(right, Ordering::Relaxed);
    INSET_BOTTOM.store(bottom, Ordering::Relaxed);
    INSET_RECEIVED.store(true, Ordering::Release);
}
