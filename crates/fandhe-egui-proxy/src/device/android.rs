//! Phase 8（端末機能、Issue #750）: Android 向け `DataDir` 解決とカメラ Intent 中継の
//! JNI 経由呼び出し。呼び出し元は `crate::device`（`storage_note_state` の Android 分岐）と
//! `crate::device::camera_capture`（`platform_camera` の Android 分岐）で、いずれも
//! `feature = "device"` 有効時のみコンパイル対象になる（`device.rs` の `mod` 宣言参照）。
//!
//! `AndroidApp` の取得は `crate::android::android_app()`（`android_main` が `eframe` へ
//! move する前に static へ複製したもの、`android.rs` 参照）を通す。`fandhe-core` は
//! 本モジュールを一切参照しない（依存方向はプロキシ → コアの一方向、coding-rust.md 準拠）。

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::drag_drop::DropItem;
use fandhe_device::camera::Camera;
use fandhe_device::clipboard::Clipboard;
use fandhe_device::file_picker::{PickedFile, MAX_FILE_BYTES};
use fandhe_device::haptics::Haptics;
use fandhe_device::nlp::{validate_input, WordSegmenter};
use fandhe_device::secure_credential::{validate_key, validate_secret, Secret, SecureCredential};
use fandhe_device::share::ShareRequest;
use fandhe_device::text_scale::TextScale;
use fandhe_device::tts::Tts;
use fandhe_device::{DataDir, DeviceError, Image};

use super::camera_capture::CaptureMailbox;

/// アプリ内部ストレージのパス（`getFilesDir()` 相当）を返す。`android-activity` の
/// `AndroidApp::internal_data_path()` が `GameActivity.internalDataPath` を返す
/// （Issue #750 実装計画の一次情報確認済み）ため、JNI を書かずに済む。
/// `crate::android::android_app()` が `None`（`android_main` 未実行、通常到達しない）
/// または OS 側の取得自体が失敗した場合は `None` を返す。
pub fn data_dir() -> Option<DataDir> {
    crate::android::android_app()?
        .internal_data_path()
        .map(DataDir)
}

/// 撮影 1 件を仲介する mailbox（プロセス内で単一飛行、`CaptureMailbox` 参照）。
/// Activity 再生成時に届く完了は旧 `ProxyApp` の `CompletionSender` へ送られ
/// 捨てられるが、Java `onCreate` が status 7 で中断決着させるため新画面が
/// 引き継ぐべき実結果は存在しない（#881、`device/camera_capture.rs` モジュール
/// doc 参照）。
static CAPTURE_MAILBOX: CaptureMailbox = CaptureMailbox::new();

/// JNI コールバック待ちの上限（ponytail: カメラアプリの起動・撮影・戻りは通常数秒
/// 〜数十秒で終わる。ユーザーがカメラアプリを開いたまま放置するケースへの安全弁で、
/// 値自体に強い実測根拠はない。実機で `Capturing` 固着の報告が出たら見直す）。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(300);

/// `nativeOnCameraResult` が JByteArray の長さを検査する上限。`fandhe_device::camera` の
/// `MAX_IMAGE_BYTES`（非公開定数）と同値（64 MiB）を本モジュールで複製する
/// （`fandhe-device` は変更しない方針、#750 実装計画 §4 参照）。
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// JNI 呼び出し中のエラーを一時的にまとめる内部専用の型。`jni::errors::Error` から
/// `From` 変換できる必要がある（`JavaVM::attach_current_thread` の型制約）が、
/// `fandhe_device::DeviceError` はこの変換を持たない（`fandhe-device` を変更しない方針）
/// ため、本モジュール内に閉じた変換先として用意する。`pub(crate)`: `android_background_task.rs`
/// も同じ JNI 呼び出し形（`AndroidClipboard::request` と同型）で再利用する
/// （Issue #1046、二重実装を避ける）。
pub(crate) struct JniCallFailed;

impl From<jni::errors::Error> for JniCallFailed {
    fn from(_: jni::errors::Error) -> Self {
        JniCallFailed
    }
}

/// `Camera::capture()` を Java 側 `MainActivity.requestCameraCapture()` へ委譲する実装
/// （Issue #750）。`device/camera_capture.rs` の `platform_camera()` から Android
/// ターゲットでのみ構築される。
pub struct AndroidCamera;

impl AndroidCamera {
    /// `AndroidCamera` を構築する。内部状態は持たず、`capture()` の都度
    /// `crate::android::android_app()` から最新の `AndroidApp` を取得するため、
    /// 構築タイミングは呼び出し側（`camera_capture::platform_camera`）が任意に選べる。
    pub fn new() -> Self {
        Self
    }
}

impl Default for AndroidCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera for AndroidCamera {
    // 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `env.as_cast_raw`）は各 `// SAFETY:`
    // コメント参照。`#[allow(unsafe_code)]` はこの 1 メソッドに閉じ、`unsafe` を安全な
    // `Camera` トレイト実装の外へ漏らさない（`.claude/rules/coding-rust.md` の FFI 例外規定）。
    #[allow(unsafe_code)]
    fn capture(&self) -> Result<Image, DeviceError> {
        // codex-review 指摘（PR #790）: 要求 ID を発行し、JNI 往復
        // （`requestCameraCapture(long)` → Java 側が保持 → `nativeOnCameraResult` で
        // 返却）に乗せて運ぶ。`deliver` はこの ID が一致する結果のみ受理するため、
        // タイムアウト後に届いた古い要求のコールバックが新しい要求の結果と
        // 取り違えられない（`CaptureMailbox` doc 参照）。
        let Some(request_id) = CAPTURE_MAILBOX.begin() else {
            // UI 側は Requested → Capturing の二段階状態機械のため通常はここへ
            // 到達しないが、多重呼び出しに対する防御として単一飛行を守る。
            return Err(DeviceError::Unsupported("camera_busy"));
        };

        let Some(app) = crate::android::android_app() else {
            CAPTURE_MAILBOX.cancel(request_id);
            return Err(DeviceError::Unsupported("android_app_unavailable"));
        };

        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` を返す。
        // `android_app()` は `crate::android::set_android_app` が保持する最新の
        // `AndroidApp`（`Activity` 再生成時は都度差し替え済み、PR #790 codex-review 指摘）を
        // 複製して返すため、ここで得た `app` は呼び出し時点で有効な `Activity` を指す
        // （android-activity 0.6.1 の `vm_as_ptr`/`activity_as_ptr` ドキュメント参照、
        // #750 実装計画 §2）。呼び出し中に OS が `Activity` を破棄しても `capture()` 内での
        // 完結（JNI 呼び出し → mailbox 待ち）はローカル変数 `app` が生存を保つため、
        // 途中で無効化される心配はない。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let request_result =
            vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `activity_as_ptr()` は `AndroidApp` が保持する未所有の JNI
                // グローバル参照で、`AndroidApp` の生存期間中（上記と同じ根拠）有効。
                // `Cast` は所有権を持たないラッパーのため、delete を試みない
                // （jni クレートの `as_cast_raw` ドキュメント「Don't wrap as Global/Auto」参照）。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                env.call_method(
                    &activity,
                    jni::jni_str!("requestCameraCapture"),
                    jni::jni_sig!((long) -> ()),
                    &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
                )?;
                Ok(())
            });

        if request_result.is_err() {
            // JNI 呼び出し自体が失敗した（メソッド未検出・例外送出等）。コールバックは
            // 来ないため `deliver` を待たず即座にスロットを解放する。
            CAPTURE_MAILBOX.cancel(request_id);
            return Err(DeviceError::Unsupported("jni_call_failed"));
        }

        CAPTURE_MAILBOX.wait(request_id, CAPTURE_TIMEOUT)
    }
}

/// Java 側から届いた撮影結果を `Image` へ変換する（`nativeOnCameraResult` 本体）。
/// `status` の意味は `MainActivity.requestCameraCapture`（Java 側実装）と対で決める:
/// 0 = 成功、1 = 権限拒否、2 = カメラアプリなし、3 = キャンセル、
/// 4 = 旧要求が未完了のため Java 側が新要求を拒否（`awaitingCameraResult` doc 参照、
/// codex-review 指摘 PR #790）、5 = 画像サイズが上限超過（Java 側 `deliverBitmap` の
/// 配列確保前検査で検出）、6 = `launch()` 自体が想定外の実行時例外で失敗
/// （cursor Bugbot 指摘 PR #790）、7 = Activity 再生成により中断（進行中の撮影要求が
/// 再生成前の Activity インスタンスに紐づいていたため結果を引き継げない。`MainActivity`
/// の `onCreate` 復元処理 doc 参照、cursor Bugbot 指摘 PR #790）。それ以外は未知の失敗。
fn decode_capture_result(
    env: &jni::Env,
    rgba: &jni::objects::JByteArray,
    width: jni::sys::jint,
    height: jni::sys::jint,
    status: jni::sys::jint,
) -> Result<Image, DeviceError> {
    let fallback_size = (width.max(0) as u32, height.max(0) as u32);
    match status {
        0 => {}
        1 => return Err(DeviceError::PermissionDenied("camera")),
        2 => return Err(DeviceError::Unsupported("camera_app_not_found")),
        3 => return Err(DeviceError::Unsupported("camera_cancelled")),
        4 => return Err(DeviceError::Unsupported("camera_busy")),
        5 => {
            return Err(DeviceError::InvalidImageSize(
                fallback_size.0,
                fallback_size.1,
            ))
        }
        6 => return Err(DeviceError::Unsupported("camera_launch_failed")),
        7 => return Err(DeviceError::Unsupported("camera_interrupted_by_recreation")),
        _ => return Err(DeviceError::Unsupported("camera_unknown_status")),
    }

    let (width, height) = match (u32::try_from(width), u32::try_from(height)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => {
            return Err(DeviceError::InvalidImageSize(
                fallback_size.0,
                fallback_size.1,
            ))
        }
    };

    // 変換前（`convert_byte_array` が Vec を確保する前）に配列長を検査する
    // （security.md A05、メモリ枯渇防止。`fandhe_device::camera::MAX_IMAGE_BYTES` と
    // 同値、上記 `MAX_IMAGE_BYTES` コメント参照）。
    let len = rgba
        .len(env)
        .map_err(|_| DeviceError::InvalidImageSize(width, height))?;
    if len > MAX_IMAGE_BYTES {
        return Err(DeviceError::InvalidImageSize(width, height));
    }

    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(DeviceError::InvalidImageSize(width, height))?;
    if len != expected_len {
        return Err(DeviceError::InvalidImageSize(width, height));
    }

    let bytes = env
        .convert_byte_array(rgba)
        .map_err(|_| DeviceError::InvalidImageSize(width, height))?;

    Ok(Image {
        width,
        height,
        rgba: bytes,
    })
}

/// `MainActivity.nativeOnCameraResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// シンボル名はパッケージ名 `com.fandhe.androidapp` とクラス名 `MainActivity` に
/// 固定で紐づく（`android.rs` の `nativeOnWindowInsetsChanged` と同じ JNI 命名規約）。
///
/// [`decode_capture_result`] で `Image`/`DeviceError` へ変換し、[`CAPTURE_MAILBOX`] へ
/// 届ける（`AndroidCamera::capture` の `wait` がこれを受け取る）。`request_id` は
/// `AndroidCamera::capture` が発行し `requestCameraCapture(long)` へ渡した値を
/// `MainActivity` がそのまま持ち回って返したもの（codex-review 指摘、PR #790）。
/// `deliver` が ID 不一致を無視するため、タイムアウト後に届いた古い要求のコールバックが
/// 新しい要求の結果を上書きしない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`android_main` と同じ根拠、
/// `.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnCameraResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    rgba: jni::objects::JByteArray<'caller>,
    width: jni::sys::jint,
    height: jni::sys::jint,
    status: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_capture_result(env, &rgba, width, height, status);
        CAPTURE_MAILBOX.deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。本関数は
    // Java 側へ戻り値も例外も返さない契約のため、結果は握りつぶす（`decode_capture_result`
    // 自体は `jni::errors::Error` を返さない設計のため通常この分岐には到達しない）。
    let _ = outcome.into_outcome();
}

/// `MainActivity.nativeOnMotionChanged`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #831、Phase 10 型サンプル「傾けると玉が転がる」の Android 実センサ経路）。
/// シンボル名の固定規約は `nativeOnCameraResult`/`android.rs` の
/// `nativeOnWindowInsetsChanged` と同じ。
///
/// `SensorEventListener#onSensorChanged`（UI スレッド）が受けた重力センサ値
/// `(x, y)` と画面回転 `rotation`（`Surface.ROTATION_*`）をそのまま受け取り、
/// [`crate::device::tilt::gravity_to_tilt`] で画面座標の傾きへ写像してから
/// [`crate::device::tilt::shared_motion`] へ push する。`tilt_ball_effect`
/// （`device/tilt.rs` の `drive`）が毎フレームここを読むだけで実機の傾きが
/// 反映される（新しい `Motion` 実装は追加しない設計、#831 実装計画 §3(a)）。
///
/// `JNIEnv`/`jclass` は本関数内で一切使わず（受け取った値を計算して static へ
/// store するのみ）、生ポインタへのアクセスを行わないため関数本体に `unsafe`
/// 操作は含まない。`#[no_mangle]` はこの 1 関数に閉じ、`unsafe` を公開 API 側へ
/// 漏らさない（`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
///
/// 値の保存自体は画面に関わらず毎回行うが、[`crate::android::request_repaint`] の呼び出しは
/// [`crate::android::active_screen_uses_tilt`] が真の場合に限る（codex-review 指摘 P2、
/// Issue #831）。`Java` 側の `motionListener` は画面によらず常にセンサを登録するため、
/// 無条件に呼ぶと傾きを使わない画面でも `SENSOR_DELAY_GAME` の頻度で再描画要求が発生し
/// 不要な電力消費になる。傾き画面が有効な場合は、静止時にセンサが厳密な `(0, 0)` を
/// 送ってきた場合（水平置き等）や初回センサ通知前に `tilt_ball_effect` の `drive` が
/// 非送信で描画が止まっていても、以後の非ゼロ値到着で確実に再開する
/// （`drive` 自体の自走に依存しない、codex-review 指摘 PRRT_kwDOTl73E86hCOSM）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib として
/// 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`nativeOnWindowInsetsChanged`・`nativeOnCameraResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnMotionChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    x: f32,
    y: f32,
    rotation: i32,
) {
    let (tilt_x, tilt_y) = crate::device::tilt::gravity_to_tilt(x, y, rotation);
    crate::device::tilt::shared_motion().set(tilt_x, tilt_y);
    if crate::android::active_screen_uses_tilt() {
        crate::android::request_repaint();
    }
}

// --------------------------------------------------------------------------
// クリップボード（Issue #859、Phase 11 型サンプル「クリップボード」）
// --------------------------------------------------------------------------
//
// `ClipboardManager` は Java 側の `Context` から取得する system service で、UI
// スレッドから触るのが Android の作法（`getPrimaryClip` は Android 10 以降
// フォーカス状態に依存する）。一方 `android_main`（eframe の event loop、effect
// ドライバが走るスレッド）は GameActivity 経路では Java の UI スレッドとは別の
// ネイティブスレッドであるため、`MainActivity.clipboardRequest` へ JNI で要求を
// 出し、Java 側が `runOnUiThread` で実行した結果を `nativeOnClipboardResult` で
// 受け取る往復にする（カメラ（#750）と同じ mailbox 方式だが、Activity result の
// ライフサイクルが絡まないぶん単純）。

/// クリップボード要求の種別（Java 側 `MainActivity.clipboardRequest` の `op` と対）。
const CLIPBOARD_OP_READ: i32 = 0;
/// 同上（書き込み）。
const CLIPBOARD_OP_WRITE: i32 = 1;

/// JNI 往復の待ち上限。クリップボードは OS 内の即時操作で、UI スレッドへの
/// `runOnUiThread` ディスパッチ待ちしか入らないため短く取る（カメラの 300 秒と違い
/// ユーザー操作を待たない）。固着防止の安全弁であり、値自体に強い実測根拠はない
/// （ponytail: 実機で timeout の報告が出たら見直す）。
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);

/// 1 件の request/response 往復を仲介する mailbox のスロット。運ぶ値の型 `T` は
/// クリップボード（`Option<String>`）・TTS（`()`）で異なるため汎用化する
/// （Issue #866、`ClipboardMailbox` を一般化したもの。挙動・doc は据え置き）。
enum Slot<T> {
    /// 要求なし。
    Idle,
    /// `id` の要求が進行中（Java 側のコールバック待ち）。
    Pending(u64),
    /// `id` の結果が到着済み。
    Ready(u64, Result<T, DeviceError>),
}

/// request/response 往復を仲介する mailbox（`CaptureMailbox` と同型だが、
/// Activity result のライフサイクルを持たない単純な往復専用）。
struct Mailbox<T> {
    slot: Mutex<Slot<T>>,
    condvar: Condvar,
    next_id: AtomicU64,
    /// `wait` がタイムアウトした際のエラー文言（用途ごとに変える。例:
    /// `"clipboard timeout"` / `"tts timeout"`）。
    timeout_label: &'static str,
}

impl<T> Mailbox<T> {
    /// 空の mailbox を構築する（`static` 初期化子から呼べるよう `const fn`）。
    const fn new(timeout_label: &'static str) -> Self {
        Self {
            slot: Mutex::new(Slot::Idle),
            condvar: Condvar::new(),
            next_id: AtomicU64::new(0),
            timeout_label,
        }
    }

    /// `Idle` なら要求 ID を発行して `Pending` へ遷移する。進行中なら `None`
    /// （ponytail: 待ち行列化はしない。画面側が `pending` で二重要求を防いでいる）。
    fn begin(&self) -> Option<u64> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Idle) {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            *slot = Slot::Pending(id);
            Some(id)
        } else {
            None
        }
    }

    /// [`begin`](Self::begin) の ID 指定版。呼び出し元が別の経路（`AndroidTts` の
    /// 共有カウンタ等）で発行済みの ID を引き継ぐ場合に使う。`Idle` のときのみ
    /// `Pending(id)` へ遷移して `true` を返し、進行中なら `false`
    /// （単一飛行の防御、`begin` と同じ意味）。
    fn begin_at_if_idle(&self, id: u64) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Idle) {
            *slot = Slot::Pending(id);
            true
        } else {
            false
        }
    }

    /// 現在の状態を無条件に `Pending(id)` で上書きする（単一飛行は呼び出し元が
    /// 別途保証している前提。`AndroidTts` の読み上げ往復（Issue #866 案A）で、
    /// 「受理」の mailbox とは独立に「最終結果」の mailbox を登録する用途に使う。
    /// `begin_at_if_idle` と違い busy を返さない点に注意。
    ///
    /// 上書きされた旧 ID を待っているスレッドがあれば `notify_all` で即座に
    /// 起こす（codex-review P1 再指摘: 通知しないと [`wait_or_ok`](Mailbox::wait_or_ok)
    /// の待機ループが世代の変化に気づくのが次の `wait_timeout` 満了（＝ここでは
    /// 実質タイムアウト全体）まで遅れ、`Tts::wait_outcome` の「置き換わり済みなら
    /// 即座に `Ok(())`」契約に反してしまう）。
    fn begin_at(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Slot::Pending(id);
        self.condvar.notify_all();
    }

    /// [`deliver`](Self::deliver) と同じだが、`id` が現在の `Pending` と一致しない
    /// 場合は届けずに `false` を返す（`result` は呼び出し元へ返さず破棄する。
    /// 破棄してよい理由は呼び出し元 [`deliver`](Self::deliver) 参照）。`id` が
    /// 「受理」と「最終結果」のどちらの mailbox 宛か呼び出し側では判別できない
    /// 場合に、まず一方へ届けを試み、届かなければもう一方へ回すルーティング用
    /// （`AndroidTts` の `nativeOnTtsResult` 受信処理、Issue #866 案A）。
    fn try_deliver(&self, id: u64, result: Result<T, DeviceError>) -> bool {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
            *slot = Slot::Ready(id, result);
            self.condvar.notify_all();
            true
        } else {
            false
        }
    }

    /// JNI 呼び出し自体が失敗してコールバックが来ないと分かった時点で `Idle` へ戻す。
    /// `id` が現在の `Pending` と一致しない場合は無視する（別要求のスロットを畳まない）。
    fn cancel(&self, id: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
            *slot = Slot::Idle;
        }
    }

    /// Java 側コールバックスレッドから結果を届ける。ID 一致時のみ受理する
    /// （タイムアウト後の遅延到着が新しい要求の結果を上書きしない）。
    /// 一致しない場合は [`try_deliver`](Self::try_deliver) の戻り値（`false`）を
    /// 読み捨てる（届け先が無い＝もう誰も待っていないため、`result` は破棄してよい）。
    fn deliver(&self, id: u64, result: Result<T, DeviceError>) {
        let _ = self.try_deliver(id, result);
    }

    /// `begin` を呼んだスレッドがその ID の結果を待つ。`timeout` 内に届かなければ
    /// スロットを `Idle` へ戻して（固着防止）エラーを返す。
    fn wait(&self, id: u64, timeout: Duration) -> Result<T, DeviceError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(*slot, Slot::Ready(ready_id, _) if ready_id == id) {
                let ready = std::mem::replace(&mut *slot, Slot::Idle);
                return match ready {
                    Slot::Ready(_, result) => result,
                    _ => unreachable!("直前に Ready であることを確認済み"),
                };
            }
            let now = Instant::now();
            if now >= deadline {
                if matches!(*slot, Slot::Pending(pending_id) if pending_id == id) {
                    *slot = Slot::Idle;
                }
                return Err(DeviceError::Internal(self.timeout_label));
            }
            let (guard, _timeout_result) = self
                .condvar
                .wait_timeout(slot, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            slot = guard;
        }
    }
}

impl Mailbox<()> {
    /// `id` を追跡していなければブロックせず直ちに `Ok(())` を返し、追跡中なら
    /// 結果が届くまで待つ。
    ///
    /// [`fandhe_device::tts::Tts::wait_outcome`] の契約「対応する処理が既に無い
    /// （未起動・空発話等で登録されていない、または別の発話へ置き換わり済みの）
    /// 場合は古い待機として即座に `Ok(())` を返す」を [`TTS_OUTCOME_MAILBOX`] 用に
    /// 実装する（Issue #866 案A）。空文字列・空白のみの発話（Java を一切呼ばない）
    /// はこの mailbox に登録されないため、この分岐で即 `Ok(())` になる。
    ///
    /// **時間による打ち切りは行わない**（codex-review P1・Bugbot Medium 是正、
    /// PR #869 3 巡目: 発話には長さの上限が無く、`CommandTts`（macOS）の
    /// `wait_outcome` も完了待ちに時間上限を持たない。固定時間で打ち切ると、
    /// 上限を超えて続く正常な発話が「まだ鳴っているのに失敗表示」になる）。
    /// 終了する条件は次のいずれかのみ:
    /// (1) 対象世代 `id` の結果が届く（[`deliver`](Mailbox::deliver)/
    /// [`try_deliver`](Mailbox::try_deliver)、成功/失敗/`stop` によるキャンセル）、
    /// (2) 対象世代が別の発話 B へ置き換わる（[`begin_at`](Mailbox::begin_at)、
    /// 後続 speak による世代置換）。呼び出し元（`device/tts.rs::run_speak` が
    /// 切り出す背景スレッド）が固まらないよう、`Tts::stop` は必ずどちらかを
    /// 引き起こす契約（`stop` は現在発話中の世代を `onStop`/`didCancel` 経由で
    /// (1) へ、後続 `speak` は (2) へ倒す）。
    ///
    /// [`wait`](Mailbox::wait) をそのまま使わず専用ループにする理由（codex-review
    /// P1 再指摘）: `wait` は「自分が `begin` した ID がそのまま `Ready` になる」
    /// 契約のみを見るため、待機中に [`begin_at`](Mailbox::begin_at) が同じ mailbox を
    /// 別の ID（後続の発話 B）へ無条件に差し替えても、それを検知せず古い ID の
    /// `Ready` をひたすら待ち続けてしまう。Java 側は新しい ID にしか応答しない
    /// ため、置き換えられた A 側の待機は目を覚まさず、`wait_outcome` の
    /// 「置き換わり済みなら即座に `Ok(())`」契約が崩れる。本ループは
    /// `notify_all`（`begin_at`/`deliver` 双方が呼ぶ）で起きるたびにスロットの ID を
    /// 再確認し、`id` と異なる `Pending`/`Ready`（＝別の世代に上書きされた）や
    /// `Idle`（＝既に誰かに消費された）を見たら古い待機として即座に `Ok(())` で抜ける。
    fn wait_or_ok(&self, id: u64) -> Result<(), DeviceError> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *slot {
                Slot::Ready(ready_id, _) if ready_id == id => {
                    let ready = std::mem::replace(&mut *slot, Slot::Idle);
                    return match ready {
                        Slot::Ready(_, result) => result,
                        _ => unreachable!("直前に Ready であることを確認済み"),
                    };
                }
                Slot::Pending(pending_id) if pending_id == id => {
                    // まだ自分の世代を追跡中。結果到着まで待つ（時間では諦めない）。
                }
                _ => {
                    // `Idle`、または `id` と異なる世代の `Pending`/`Ready`。
                    // 自分の世代はもう追跡されていない（置き換え済み・古い待機）
                    // ため、他の世代の状態には触れずそのまま即座に成功で抜ける。
                    return Ok(());
                }
            }
            slot = self.condvar.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// クリップボード往復を仲介する mailbox（プロセス内で単一飛行）。
static CLIPBOARD_MAILBOX: Mailbox<Option<String>> = Mailbox::new("clipboard timeout");

/// Android 実クリップボード実装（`ClipboardManager`、Issue #859）。
/// `device/clipboard.rs` の effect ドライバから「コピー」/「貼り付け」押下時に呼ばれる。
pub struct AndroidClipboard;

impl AndroidClipboard {
    /// Java 側 `MainActivity.clipboardRequest(long, int, String)` を呼び、結果を待つ。
    /// `op` は [`CLIPBOARD_OP_READ`] / [`CLIPBOARD_OP_WRITE`]。
    // 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は各 `// SAFETY:`
    // コメント参照。`AndroidCamera::capture` と同じ根拠・同じ形に揃えている。
    #[allow(unsafe_code)]
    fn request(&self, op: i32, text: &str) -> Result<Option<String>, DeviceError> {
        let Some(request_id) = CLIPBOARD_MAILBOX.begin() else {
            // 画面側が `pending` で二重要求を防いでいるため通常到達しないが、
            // 多重呼び出しに対する防御として単一飛行を守る。
            return Err(DeviceError::Internal("clipboard busy"));
        };

        let Some(app) = crate::android::android_app() else {
            CLIPBOARD_MAILBOX.cancel(request_id);
            return Err(DeviceError::Unsupported("clipboard"));
        };

        // SAFETY: `AndroidCamera::capture` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` を返し、`android_app()` は
        // 最新の `AndroidApp` を複製して返すため、ここで得た `app` は呼び出し時点で
        // 有効な `Activity` を指す。呼び出し中（JNI 呼び出し → mailbox 待ち）は
        // ローカル変数 `app` が生存を保つ。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let request_result =
            vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
                // クリップボードへ渡す文字列を Java String へ変換する（読み出し要求では
                // 空文字列を渡し、Java 側は `op` で無視する）。
                let j_text = env.new_string(text)?;
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `AndroidCamera::capture` と同一の根拠。`activity_as_ptr()` は
                // `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
                // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を
                // 試みない。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                env.call_method(
                    &activity,
                    jni::jni_str!("clipboardRequest"),
                    jni::jni_sig!((long, int, java.lang.String) -> ()),
                    &[
                        jni::objects::JValue::Long(request_id as jni::sys::jlong),
                        jni::objects::JValue::Int(op as jni::sys::jint),
                        jni::objects::JValue::Object(&j_text),
                    ],
                )?;
                Ok(())
            });

        if request_result.is_err() {
            // JNI 呼び出し自体が失敗した（メソッド未検出・例外送出等）。コールバックは
            // 来ないため `wait` せず即座にスロットを解放する。
            CLIPBOARD_MAILBOX.cancel(request_id);
            return Err(DeviceError::Internal("clipboard jni"));
        }

        CLIPBOARD_MAILBOX.wait(request_id, CLIPBOARD_TIMEOUT)
    }
}

impl Clipboard for AndroidClipboard {
    fn get_text(&self) -> Result<Option<String>, DeviceError> {
        self.request(CLIPBOARD_OP_READ, "")
    }

    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        self.request(CLIPBOARD_OP_WRITE, text).map(|_| ())
    }
}

/// Java 側から届いたクリップボード結果を解釈する（`nativeOnClipboardResult` 本体）。
/// `status` の意味は `MainActivity.clipboardRequest`（Java 側実装）と対で決める:
/// 0 = 成功（読み出しは `text` に内容、書き込みは `text` が null）、
/// 1 = クリップボードにテキストが無い（読み出しのみ。`Ok(None)`）、
/// 2 = `ClipboardManager` を取得できない等の内部エラー。それ以外は未知の失敗。
fn decode_clipboard_result(
    text: Option<String>,
    status: jni::sys::jint,
) -> Result<Option<String>, DeviceError> {
    match status {
        0 => Ok(text),
        1 => Ok(None),
        _ => Err(DeviceError::Internal("clipboard")),
    }
}

/// `MainActivity.nativeOnClipboardResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #859）。シンボル名の固定規約は `nativeOnCameraResult` と同じ。
///
/// 受け取った内容は [`CLIPBOARD_MAILBOX`] へ届けるだけで、ログ出力・保存は一切行わない
/// （クリップボードは他アプリのパスワード等が載り得る機密経路のため痕跡を残さない、
/// security.md A09）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnCameraResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnClipboardResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    text: jni::objects::JString<'caller>,
    status: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        // `text` が null（書き込み要求の応答・テキスト不在）なら `None` にする。
        let value = if text.is_null() {
            None
        } else {
            // 変換失敗（不正な UTF-16 等）は内容を推測せず不在扱いにする
            // （security.md A03: 検証できない入力を通さない）。
            // modified UTF-8 → Rust の `String` 変換（jni 0.22 の非 deprecated API。
            // `Env::get_string` は deprecated で `JString::try_to_string` が後継）。
            text.try_to_string(env).ok()
        };
        CLIPBOARD_MAILBOX.deliver(request_id as u64, decode_clipboard_result(value, status));
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

// --------------------------------------------------------------------------
// 読み上げ（Issue #866、Phase 11 型サンプル「読み上げ」の Android 経路）
// --------------------------------------------------------------------------
//
// `TextToSpeech` はクリップボードと同じく Java 側の system 資源だが、初期化が
// **非同期**（コンストラクタに渡す `OnInitListener` が後から呼ばれる）という
// クリップボードには無い制約を持つ。この往復では「初期化未完了時に `speak` が
// 偽の成功を返す」経路を作らないことが最重要（親 #864・#859 の再発防止点）。
//
// 設計（案A、PR #869 codex-review P1 是正）: 旧実装は `AndroidTts::speak` が
// `onInit` 完了まで同期的にブロックしていた。`speak`/`stop` は `device/tts.rs`
// の TTS ワーカースレッド（`worker_loop`）が同じ FIFO で逐次処理するため、
// 初期化が長引くと `stop` 要求がキューに残ったまま初期化完了後に
// 「キャンセル済みのテキストを発話してから停止」してしまい、Java 側
// `MainActivity` の INITIALIZING 中の停止分岐へ通常経路から到達できなかった。
//
// 是正: `speak` は Java が要求を **受理** した時点（[`nativeOnTtsAccepted`]）で
// 即座に戻り、真の結果は [`Tts::wait_outcome`] が [`TTS_OUTCOME_MAILBOX`] を
// 別スレッドから待つ（`CommandTts`/`IosTts` と同じ「`speak` は開始で戻り、
// `wait_outcome` が最終結果を持つ」契約に揃える）。これにより `speak` が
// ワーカースレッドを占有する時間は「受理されるまで」（Java の UI スレッドへ
// ポストする程度、通常はミリ秒オーダー）に短縮され、直後にキューへ積まれた
// `stop` 要求を初期化完了を待たず即座に Java へ届けられる。
//
// mailbox は 2 本に分ける（[`TTS_ACCEPT_MAILBOX`]/[`TTS_OUTCOME_MAILBOX`]）:
// - `TTS_ACCEPT_MAILBOX`: JNI 往復 1 回分の応答を仲介する短命な mailbox。
//   SPEAK では [`nativeOnTtsAccepted`] の受理シグナルのみを待つ（Java は
//   受理を送った後、実処理へ進む前にこの mailbox の役目は終わる）。STOP は
//   受理という概念を持たず、[`nativeOnTtsResult`] の最終結果をそのままここで
//   受け取る（STOP は `onInit` を待たないため二段階に分ける必要が無い）。
// - `TTS_OUTCOME_MAILBOX`: SPEAK が受理された後の真の最終結果（`onInit` の
//   成否・`speak()` 呼び出し自体の成否）を仲介する。`speak` を呼んだ ID を
//   そのまま `wait_outcome` の世代として使う（`Tts::speak` の契約「戻り値は
//   この呼び出しを識別する世代 id」）ため、2 つの mailbox は同じ ID 空間
//   （[`AndroidTts::next_id`]）を共有する。
//
// [`Java_com_fandhe_androidapp_MainActivity_nativeOnTtsResult`] は「この ID の
// 最終結果がどちらの mailbox 宛か」を判定せず、まず `TTS_ACCEPT_MAILBOX` へ
// 届けを試み（STOP の最終結果はここで受理される）、一致しなければ
// `TTS_OUTCOME_MAILBOX` へ回す（SPEAK が既に受理済みで、こちらが本来の宛先）。
// SPEAK の ID は受理シグナルの消費と同時に `TTS_ACCEPT_MAILBOX` 側が空くため、
// 受理後に届く最終結果が誤って `TTS_ACCEPT_MAILBOX` 側にマッチすることはない
// （Java は SPEAK について必ず受理 → 最終結果の順で呼ぶため、最終結果が届く
// 時点で受理は既に消費済みか届け済みであり、いずれにせよ `Pending` ではない）。
//
// Java 側（`MainActivity.ttsRequest`）は `TTS_STATE_INITIALIZING` 中に STOP が
// 届くと、保留中の SPEAK 要求をキャンセル（status 7）として畳んでから自身の
// 停止処理へ進む。キャンセルは失敗として扱わない（[`decode_tts_result`] 参照、
// `CommandTts::wait_outcome` doc の「ユーザー操作による停止は失敗として
// 扱わない」と同じ判断）。
//
// Activity 再生成時の結果配送（#881）: 再生成中の結果は旧 `ProxyApp` の
// `CompletionSender` へ送られ捨てられるが、`onDestroy` が `currentSpeakingRequestId`
// を確定させて `nativeOnTtsResult` を呼ぶため、新画面が引き継ぐべき実結果は
// 存在しない（`device/tts.rs` モジュール doc 参照）。
//
// 案A是正の続き（codex-review P1 再指摘、本 PR）: 案Aは「受理」と「結果」を
// 分離したが、Java 側 `speakNow` はなお `TextToSpeech.speak()` の戻り値
// （`SUCCESS`）＝キュー投入成功をそのまま `nativeOnTtsResult(0)` として送っており、
// これは合成完了ではない（`speak()` はキューへ積んだだけで戻る非同期 API、
// `TextToSpeech` のドキュメント）。是正: `MainActivity` は
// `setOnUtteranceProgressListener` で `onDone`/`onError`/`onStop` を購読し、
// `speak()` の utteranceId（`"fandhe-" + requestId`）から `requestId` を復元して
// `nativeOnTtsResult` を呼ぶ（`MainActivity.speakNow`/`utteranceListener` 参照）。
// `speakNow` 自身は `speak()` が `ERROR` を返した（キュー投入失敗、コールバックが
// 二度と来ない）場合のみ即座に `nativeOnTtsResult(5)` を送り、投入成功時は
// 何も送らず listener からの通知を待つ。

/// 読み上げ要求の種別（Java 側 `MainActivity.ttsRequest` の `op` と対）。
const TTS_OP_SPEAK: i32 = 0;
/// 同上（停止）。
const TTS_OP_STOP: i32 = 1;

/// JNI 往復 1 回分の応答（SPEAK は受理シグナル、STOP は最終結果）を待つ上限。
/// Java 側は `runOnUiThread` へポストした直後、実処理（`onInit` 待ち等）に
/// 入る前に必ずこれを返すため、UI スレッドの通常の込み具合以上には掛からない
/// 想定（ponytail: 実測根拠のない安全弁。実機で timeout の報告が出たら見直す）。
///
/// 「開始まで」（受理されるまで）にのみ掛ける期限であり、「発話完了まで」には
/// 掛けない（codex-review P1・Bugbot Medium 是正、PR #869 3 巡目）。受理後の
/// 真の最終結果（[`TTS_OUTCOME_MAILBOX`]、`wait_outcome`）は時間で打ち切らず、
/// 完了・失敗・キャンセル・世代置換でのみ終了する（`Mailbox::wait_or_ok` doc 参照）。
/// Java 側 `MainActivity.TTS_INIT_TIMEOUT_MS`（15 秒）は本定数とは別に、
/// 「初期化待ち中に保留した SPEAK 要求」自体の失効判定にのみ使う
/// （`expireStaleTtsRequest` 参照。これも「発話完了まで」ではなく「初期化完了 or
/// 受理まで」の期限）。この期限に達した場合、Java 側は保留を破棄するだけでなく
/// `nativeOnTtsResult(requestId, 3)` を送って [`TTS_OUTCOME_MAILBOX`] の
/// `wait_outcome` を確定させる（codex-review 指摘、PR #869 4 巡目是正: 送らないと
/// `wait_outcome` は時間で打ち切らないため、世代置換まで無期限に残ってしまう）。
const TTS_ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);

/// JNI 往復 1 回分の応答を仲介する mailbox（モジュール doc 参照）。
static TTS_ACCEPT_MAILBOX: Mailbox<()> = Mailbox::new("tts accept timeout");
/// SPEAK が受理された後の真の最終結果を仲介する mailbox（モジュール doc 参照）。
static TTS_OUTCOME_MAILBOX: Mailbox<()> = Mailbox::new("tts timeout");

/// [`TTS_ACCEPT_MAILBOX`]/[`TTS_OUTCOME_MAILBOX`] が共有する ID 空間
/// （モジュール doc 参照）。`AndroidTts` インスタンス毎ではなく static にする
/// 理由（codex-review 指摘、PR #869 5 巡目是正）: 結果の保持先
/// （`TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX`）はプロセス全体で 1 個の static
/// のため、採番だけをインスタンス毎にすると複数の `AndroidTts` インスタンス
/// （`Tts` は公開 API で誰でも構築できる）が同じ id を独立に払い出しうる。
/// その結果、無関係な 2 インスタンス A・B が同じ id で `speak`/`wait_outcome`
/// を呼ぶと、B の待機が A の結果を誤って回収してしまう（「他の世代の結果を
/// 絶対に回収しない」契約違反）。単調増加のため、mailbox をまたいで再利用されない。
static NEXT_TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Android 実読み上げ実装（`TextToSpeech`、Issue #866）。
/// `device/tts.rs` の TTS ワーカースレッド（`worker_loop`）から呼ばれる。
#[derive(Default)]
pub struct AndroidTts;

impl AndroidTts {
    fn next_id(&self) -> u64 {
        NEXT_TTS_GENERATION.fetch_add(1, Ordering::Relaxed)
    }

    /// Java 側 `MainActivity.ttsRequest(long, int, String)` を JNI で呼ぶだけの
    /// 薄いヘルパー（mailbox の登録・解除は呼び出し元が担う）。
    /// `AndroidClipboard::request` と同じ根拠・同じ形に揃えている（本体内の
    /// 2 箇所の `unsafe` は各 `// SAFETY:` 参照）。
    #[allow(unsafe_code)]
    fn call_tts_request(&self, id: u64, op: i32, text: &str) -> Result<(), DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Unsupported("tts"));
        };

        // SAFETY: `AndroidCamera::capture`/`AndroidClipboard::request` と同一の根拠。
        // `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` を返し、
        // `android_app()` は最新の `AndroidApp` を複製して返すため、ここで得た
        // `app` は呼び出し時点で有効な `Activity` を指す。呼び出し中（JNI 呼び出し
        // → mailbox 待ち）はローカル変数 `app` が生存を保つ。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let request_result =
            vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
                let j_text = env.new_string(text)?;
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `AndroidClipboard::request` と同一の根拠。`activity_as_ptr()`
                // は `AndroidApp` が保持する未所有の JNI グローバル参照で、
                // `AndroidApp` の生存期間中有効。`Cast` は所有権を持たないラッパー
                // のため delete を試みない。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                env.call_method(
                    &activity,
                    jni::jni_str!("ttsRequest"),
                    jni::jni_sig!((long, int, java.lang.String) -> ()),
                    &[
                        jni::objects::JValue::Long(id as jni::sys::jlong),
                        jni::objects::JValue::Int(op as jni::sys::jint),
                        jni::objects::JValue::Object(&j_text),
                    ],
                )?;
                Ok(())
            });

        request_result.map_err(|_| DeviceError::Internal("tts jni"))
    }
}

impl Tts for AndroidTts {
    fn speak(&self, text: &str) -> Result<u64, DeviceError> {
        // 内部 NUL を含むテキストは JNI 文字列としては渡せてしまうが、iOS 側
        // （`CString::new` が拒否）と判定を揃えるため Rust 側で先に弾く
        // （#866 実装計画 §3-5、FFI 境界の対称性）。
        if text.contains('\0') {
            return Err(DeviceError::InvalidInput("tts text"));
        }

        let id = self.next_id();

        // 空文字列・空白のみは発話するものが無いため OS を呼ばずに戻る（`Tts::speak`
        // の契約）。mailbox には一切登録しないため、`wait_outcome(id)` は
        // 「追跡していない ID」として即座に `Ok(())` を返す（`wait_or_ok` 参照）。
        if text.trim().is_empty() {
            return Ok(id);
        }

        if !TTS_ACCEPT_MAILBOX.begin_at_if_idle(id) {
            // 画面側が `speak_dispatched`/`stop_dispatched` で二重要求を防いでいる
            // ため通常到達しないが、多重呼び出しに対する防御として単一飛行を守る。
            return Err(DeviceError::Internal("tts busy"));
        }
        // 受理される前に登録する（JNI 呼び出しが返った直後に Java 側の応答が
        // 届いても取りこぼさない。呼び出しはこの後でようやく Java へ渡る）。
        TTS_OUTCOME_MAILBOX.begin_at(id);

        if let Err(e) = self.call_tts_request(id, TTS_OP_SPEAK, text) {
            // JNI 呼び出し自体が失敗した（メソッド未検出・例外送出等）。コールバックは
            // 来ないため `wait` せず即座に両 mailbox を解放する。
            TTS_ACCEPT_MAILBOX.cancel(id);
            TTS_OUTCOME_MAILBOX.cancel(id);
            return Err(e);
        }

        match TTS_ACCEPT_MAILBOX.wait(id, TTS_ACCEPT_TIMEOUT) {
            Ok(()) => Ok(id),
            Err(e) => {
                // 受理シグナルすら届かなかった（UI スレッド固着等の異常系）。
                // 真の結果を待つ相手がいなくなるため outcome 側も畳む。
                TTS_OUTCOME_MAILBOX.cancel(id);
                Err(e)
            }
        }
    }

    fn stop(&self) -> Result<(), DeviceError> {
        let id = self.next_id();
        if !TTS_ACCEPT_MAILBOX.begin_at_if_idle(id) {
            return Err(DeviceError::Internal("tts busy"));
        }
        if let Err(e) = self.call_tts_request(id, TTS_OP_STOP, "") {
            TTS_ACCEPT_MAILBOX.cancel(id);
            return Err(e);
        }
        TTS_ACCEPT_MAILBOX.wait(id, TTS_ACCEPT_TIMEOUT)
    }

    /// `speak` が返した ID に対応する真の最終結果を待つ（トレイト doc参照）。
    /// 追跡していない ID（未登録・空発話・既に消費済み）は [`Mailbox::wait_or_ok`]
    /// が即座に `Ok(())` を返す（`Tts::wait_outcome` の「対応する処理が既に無い
    /// 場合は古い待機として即座に `Ok(())`」契約）。時間による打ち切りは行わない
    /// （`Mailbox::wait_or_ok` doc 参照、codex-review P1・Bugbot Medium 是正）。
    fn wait_outcome(&self, generation: u64) -> Result<(), DeviceError> {
        TTS_OUTCOME_MAILBOX.wait_or_ok(generation)
    }
}

/// Java 側から届いた読み上げ結果を解釈する（`nativeOnTtsResult` 本体）。
/// `status` の意味は `MainActivity.ttsRequest`（Java 側実装）と対で決める
/// （#866 実装計画 §5 の表、案A 是正で 7 を追加）。
fn decode_tts_result(status: jni::sys::jint) -> Result<(), DeviceError> {
    match status {
        0 => Ok(()),
        // 1 = `TextToSpeech` を生成できない（engine なし・コンストラクタ例外）。
        // 機種として非対応。
        1 => Err(DeviceError::Unsupported("tts")),
        // 3 = 非同期初期化（`onInit`）が ERROR。機種としては対応し得るが今回は失敗。
        3 => Err(DeviceError::Internal("tts init")),
        // 4 = 初期化待ちの要求が既にある（Java 側の単一飛行防御。Rust 側の
        // mailbox が単一飛行を保証しているため通常到達しない）。
        4 => Err(DeviceError::Internal("tts busy")),
        // 5 = `speak()` 自体が `ERROR` を返した（キュー投入失敗）、または投入後に
        // `UtteranceProgressListener#onError` が届いた（発話自体の失敗、PR #869
        // 是正: 旧実装はキュー投入成功を発話成功と混同していた）。
        5 => Err(DeviceError::Internal("tts speak")),
        // 6 = `getMaxSpeechInputLength()` 超過。契約違反ではなく入力起因。
        6 => Err(DeviceError::InvalidInput("tts text")),
        // 7 = 初期化待ち中に stop が届いたことによるキャンセル（案A 是正）、または
        // 発話開始後に `onStop`（`stop()`・後続 speak の QUEUE_FLUSH による中断）が
        // 届いたことによるキャンセル（PR #869 是正）。ユーザー操作による停止のため
        // 失敗として扱わない（`CommandTts::wait_outcome` doc と同じ判断）。
        7 => Ok(()),
        // 2 = その他の内部エラー（未知の op・実行時例外・Activity 破棄で中断）。
        // 未知の status も安全側で同様に扱う。
        _ => Err(DeviceError::Internal("tts")),
    }
}

/// `MainActivity.nativeOnTtsAccepted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #866 案A）。Java が SPEAK 要求を受理した直後（実処理へ進む前）に
/// 呼ばれ、[`AndroidTts::speak`] の `TTS_ACCEPT_MAILBOX` 待ちを解放するだけの
/// 単純な仲介。生ポインタへのアクセスを行わないため `unsafe` 操作は含まない。
///
/// # Safety
/// `nativeOnTtsResult`（下記）と同じ根拠。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnTtsAccepted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    TTS_ACCEPT_MAILBOX.deliver(request_id as u64, Ok(()));
}

/// `MainActivity.nativeOnTtsResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #866）。シンボル名の固定規約は `nativeOnClipboardResult` と同じ。
///
/// STOP の最終結果、または SPEAK の真の最終結果のいずれかを運ぶ（モジュール doc
/// 「2 本の mailbox」参照）。まず [`TTS_ACCEPT_MAILBOX`] への配送を試み
/// （STOP はここで受理される）、一致しなければ [`TTS_OUTCOME_MAILBOX`] へ回す
/// （SPEAK が既に受理済みで、こちらが本来の宛先）。
///
/// [`TTS_ACCEPT_MAILBOX`] 側に届いた場合（＝ `nativeOnTtsAccepted` を経ない
/// 単相応答。SPEAK が `nativeOnTtsAccepted` を送らず早期 return する分岐
/// （最大文字数超過・`TTS_STATE_FAILED`・未知の op・破棄済み Activity、
/// `MainActivity.ttsRequest` 参照。`READY` を含む SPEAK の全経路は PR #869
/// 3 巡目是正で `nativeOnTtsAccepted` → `nativeOnTtsResult` の二相に統一済み
/// のため、現在ここへ届く SPEAK は上記の早期 return 分岐のみ）も、同じ結果を
/// [`TTS_OUTCOME_MAILBOX`] へ重ねて配送する（codex-review P1 再指摘）。
/// `AndroidTts::speak` は [`TTS_OUTCOME_MAILBOX`] を JNI 呼び出し前に必ず
/// 登録済み（`speak` 実装参照）のため、単相応答の場合ここで確定させないと
/// 登録だけが残り、`wait_outcome`（[`Mailbox::wait_or_ok`]、時間では終了しない
/// 契約、PR #869 3 巡目是正）が正常な発話を待ち続けたまま応答しなくなる
/// （嘘の失敗ではなく、確定漏れによる無応答）。STOP はそもそも
/// [`TTS_OUTCOME_MAILBOX`] を登録しないため、この重複配送は追跡されていない
/// ID への配送として無視される（[`Mailbox::deliver`] 参照）。
///
/// `JNIEnv` を使わず（受け取った値を計算して mailbox へ push するのみ）、生ポインタへの
/// アクセスを行わないため関数本体に `unsafe` 操作は含まない
/// （`nativeOnMotionChanged` と同じ形）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib
/// として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`nativeOnCameraResult`・`nativeOnClipboardResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnTtsResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    status: jni::sys::jint,
) {
    let id = request_id as u64;
    if TTS_ACCEPT_MAILBOX.try_deliver(id, decode_tts_result(status)) {
        // 単相応答（上記 doc 参照）。outcome 側が登録されたまま残らないよう
        // 同じ結果で確定させる（未登録の STOP 系 ID には無害に無視される）。
        TTS_OUTCOME_MAILBOX.deliver(id, decode_tts_result(status));
        return;
    }
    TTS_OUTCOME_MAILBOX.deliver(id, decode_tts_result(status));
}

/// Android 実文字サイズ追従実装（`Configuration.fontScale`、Issue #914）。
/// `device/text_scale.rs` の effect ドライバから毎フレーム pull される。
/// `AndroidClipboard`/`AndroidTts` と異なり mailbox を介さない: Java 側
/// `MainActivity.textScaleFactor()` は非同期コールバックを持たない単純な
/// getter のため、JNI 呼び出しの戻り値をそのまま同期で返す。
pub struct AndroidTextScale;

impl TextScale for AndroidTextScale {
    /// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は
    /// `AndroidClipboard::request` と同じ根拠・同じ形に揃えている（各
    /// `// SAFETY:` コメント参照）。
    #[allow(unsafe_code)]
    fn scale_factor(&self) -> Result<f32, DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Unsupported("text_scale"));
        };

        // SAFETY: `AndroidClipboard::request` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` を返し、`android_app()` は
        // 最新の `AndroidApp` を複製して返すため、ここで得た `app` は呼び出し
        // 時点で有効な `Activity` を指す。呼び出し中（JNI 呼び出し・戻り値変換）は
        // ローカル変数 `app` が生存を保つ。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let result = vm.attach_current_thread(|env: &mut jni::Env| -> Result<f32, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `AndroidClipboard::request` と同一の根拠。
            // `activity_as_ptr()` は `AndroidApp` が保持する未所有の JNI
            // グローバル参照で、`AndroidApp` の生存期間中有効。`Cast` は
            // 所有権を持たないラッパーのため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let value = env.call_method(
                &activity,
                jni::jni_str!("textScaleFactor"),
                jni::jni_sig!(() -> float),
                &[],
            )?;
            Ok(value.f()?)
        });

        // NaN/範囲外の丸めは `fandhe_device::text_scale::read_scale` に一本化する
        // ため、ここでは JNI から得た値をそのまま返す。JNI 呼び出し自体の失敗
        // （メソッド未検出・例外送出等）は「使えない」ではなく「失敗」として扱う
        // （`fandhe_device::text_scale` モジュール doc の「読めない」＝
        // `Unsupported` とは区別する契約、実装計画 §3 参照）。
        result.map_err(|_| DeviceError::Internal("text_scale: jni call failed"))
    }
}

/// Android 実触覚フィードバック実装（`Vibrator`、Issue #1092）。`device/haptics.rs` の
/// effect ドライバから egui スレッドで同期に呼ばれる。`AndroidTextScale` と同じく mailbox
/// を介さない: Java 側 `MainActivity.hapticsTap()`（`()I`）は非同期コールバックを持たない
/// ため、戻り値を `decode_haptics_result` で写すだけで済む。
pub struct AndroidHaptics;

impl Haptics for AndroidHaptics {
    /// 2 箇所の `unsafe` は `AndroidTextScale::scale_factor` と同じ根拠・同じ形。
    #[allow(unsafe_code)]
    fn tap(&self) -> Result<(), DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Internal("haptics: no activity"));
        };

        // SAFETY: `AndroidTextScale::scale_factor` と同一の根拠。`vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let result = vm.attach_current_thread(|env: &mut jni::Env| -> Result<i32, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `AndroidTextScale::scale_factor` と同一の根拠。未所有の JNI
            // グローバル参照を `Cast` で包むだけで delete は試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let value = env.call_method(
                &activity,
                jni::jni_str!("hapticsTap"),
                jni::jni_sig!(() -> int),
                &[],
            )?;
            Ok(value.i()?)
        });

        match result {
            Ok(code) => super::haptics::decode_haptics_result(code),
            Err(_) => Err(DeviceError::Internal("haptics: jni call failed")),
        }
    }
}

// --------------------------------------------------------------------------
// 単語分割（Issue #1095、Phase 11 型サンプル「単語分割」の Android 経路）
// --------------------------------------------------------------------------

/// Android 実単語分割実装（`android.icu.text.BreakIterator`、Issue #1095）。
/// `device/nlp.rs` の effect ドライバから egui スレッドで同期に呼ばれる。mailbox は介さない:
/// Java 側 `MainActivity.nlpSegment(String)`（`(Ljava/lang/String;)Ljava/lang/String;`）は
/// 同期 API で、非同期コールバックを持たない。戻り値は `decode_android_tokens` で写す。
pub struct AndroidWordSegmenter;

impl WordSegmenter for AndroidWordSegmenter {
    /// 2 箇所の `unsafe` は `AndroidHaptics::tap` と同じ根拠・同じ形。
    #[allow(unsafe_code)]
    fn segment(&self, text: &str) -> Result<Vec<String>, DeviceError> {
        validate_input(text)?;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Internal("nlp: no activity"));
        };

        // SAFETY: `AndroidHaptics::tap` と同一の根拠。`vm_as_ptr()` は android-activity が
        // 管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let result = vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<Option<String>, JniCallFailed> {
                let j_text = env.new_string(text)?;
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `AndroidHaptics::tap` と同一の根拠。未所有の JNI グローバル参照を
                // `Cast` で包むだけで delete は試みない。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let ret = env.call_method(
                    &activity,
                    jni::jni_str!("nlpSegment"),
                    jni::jni_sig!((java.lang.String) -> java.lang.String),
                    &[jni::objects::JValue::Object(&j_text)],
                );
                let ret = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // 例外は自動クリアされない（jni 0.22）ため後続の JNI 呼び出しへ
                        // 残さない。文言は読まない（security.md A09）。
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

        match result {
            Ok(raw) => super::nlp::decode_android_tokens(raw),
            Err(_) => Err(DeviceError::Internal("nlp: jni call failed")),
        }
    }
}

// --------------------------------------------------------------------------
// 資格情報（Issue #924、Phase 11 型サンプル「資格情報」の Android 経路）
// --------------------------------------------------------------------------

/// Java 側 `MainActivity.secureCredentialRequest(int, String, String)` の `op`
/// 引数（Java 側の同名定数と対で決める契約）。
const SECURE_CREDENTIAL_OP_STORE: i32 = 0;
const SECURE_CREDENTIAL_OP_LOAD: i32 = 1;
const SECURE_CREDENTIAL_OP_DELETE: i32 = 2;

/// Android 実資格情報保存実装（`EncryptedSharedPreferences`、Issue #924）。
/// `device/secure_credential.rs` の effect ドライバから別スレッドで呼ばれる
/// （`secure_credential.rs` モジュール doc「別スレッドで OS を呼ぶ理由」参照）。
/// `AndroidTextScale` と同じく mailbox を介さない: Java 側
/// `secureCredentialRequest` は `EncryptedSharedPreferences`（同期 API）を直接
/// 呼ぶだけで非同期コールバックを持たないため、JNI 呼び出しの戻り値をそのまま
/// 同期で返せる。
///
/// Java 側は保存値に固定プレフィックス `"v1:"` を付けて格納する
/// （`MainActivity.encodeSecureCredentialValue` / `decodeSecureCredentialValue`）。
/// `EncryptedSharedPreferences` の内部実装が復号後の文字列 `"__NULL__"` を
/// null 番兵として扱うため、素の値をそのまま保存すると偶然の一致で
/// `load` が `Ok(None)` を返しうる（codex-review 指摘、PR #931）。このモジュールが
/// 受け取る `Secret` は Java 側で既にプレフィックスを除去済みの値であり、
/// この契約を意識する必要はない。
pub struct AndroidSecureCredential;

impl AndroidSecureCredential {
    /// Java 側 `MainActivity.secureCredentialRequest(int, String, String)` を呼ぶ。
    /// `value` は STORE 以外では空文字列を渡す（Java 側は `op` で無視する）。
    ///
    /// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は
    /// `AndroidClipboard::request` と同じ根拠・同じ形に揃えている（各
    /// `// SAFETY:` コメント参照）。戻り値の `java.lang.String` から `JString` への
    /// 変換は `env.cast_local`（safe API、null なら `To::null()` を返す）を使うため
    /// 追加の `unsafe` は不要。
    #[allow(unsafe_code)]
    fn request(&self, op: i32, key: &str, value: &str) -> Result<Option<String>, DeviceError> {
        let Some(app) = crate::android::android_app() else {
            return Err(DeviceError::Unsupported("secure_credential"));
        };

        // SAFETY: `AndroidClipboard::request` と同一の根拠。`app.vm_as_ptr()` は
        // android-activity が管理する有効な `JavaVM*` を返し、`android_app()` は
        // 最新の `AndroidApp` を複製して返すため、ここで得た `app` は呼び出し
        // 時点で有効な `Activity` を指す。呼び出し中（JNI 呼び出し・戻り値変換）は
        // ローカル変数 `app` が生存を保つ。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

        let result = vm.attach_current_thread(
            |env: &mut jni::Env| -> Result<Option<String>, JniCallFailed> {
                let j_key = env.new_string(key)?;
                let j_value = env.new_string(value)?;
                let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                // SAFETY: `AndroidClipboard::request` と同一の根拠。
                // `activity_as_ptr()` は `AndroidApp` が保持する未所有の JNI
                // グローバル参照で、`AndroidApp` の生存期間中有効。`Cast` は
                // 所有権を持たないラッパーのため delete を試みない。
                let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                let ret = env.call_method(
                    &activity,
                    jni::jni_str!("secureCredentialRequest"),
                    jni::jni_sig!((int, java.lang.String, java.lang.String) -> java.lang.String),
                    &[
                        jni::objects::JValue::Int(op),
                        jni::objects::JValue::Object(&j_key),
                        jni::objects::JValue::Object(&j_value),
                    ],
                );
                let ret = match ret {
                    Ok(v) => v,
                    Err(e) => {
                        // Java 側が `IllegalStateException` を投げた場合（内部エラー）、
                        // 例外は自動クリアされない（jni 0.22 の仕様、env.rs doc 参照）
                        // ため、後続の JNI 呼び出しに影響しないよう明示的にクリアする。
                        // 文言は読まない（security.md A09、値・例外メッセージをログに
                        // 残さない契約）。
                        if env.exception_check() {
                            env.exception_clear();
                        }
                        return Err(e.into());
                    }
                };
                let obj = ret.l()?;
                // `env.cast_local` は null なら `JString::null()` を返す safe API
                // （runtime instanceof チェックは Java 側が `java.lang.String` を
                // 返す契約のため通常成立するが、`try_to_string` 側で null は
                // `NullPtr` エラーとして扱われるため、ここで明示的に判定する）。
                let jstring = env.cast_local::<jni::objects::JString>(obj)?;
                if jstring.is_null() {
                    Ok(None)
                } else {
                    Ok(Some(jstring.try_to_string(env)?))
                }
            },
        );

        result.map_err(|_| DeviceError::Internal("secure_credential: jni call failed"))
    }
}

impl SecureCredential for AndroidSecureCredential {
    fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        let secret = validate_secret(secret)?;
        self.request(SECURE_CREDENTIAL_OP_STORE, key, secret)
            .map(|_| ())
    }

    fn load(&self, key: &str) -> Result<Option<Secret>, DeviceError> {
        let key = validate_key(key)?;
        self.request(SECURE_CREDENTIAL_OP_LOAD, key, "")
            .map(|opt| opt.map(Secret::new))
    }

    fn delete(&self, key: &str) -> Result<(), DeviceError> {
        let key = validate_key(key)?;
        self.request(SECURE_CREDENTIAL_OP_DELETE, key, "")
            .map(|_| ())
    }
}

#[cfg(test)]
mod secure_credential_tests {
    use super::{
        SECURE_CREDENTIAL_OP_DELETE, SECURE_CREDENTIAL_OP_LOAD, SECURE_CREDENTIAL_OP_STORE,
    };

    /// op 定数が Java 側 `MainActivity` の同名定数と対で決めた値からずれていないことを
    /// 固定する（値そのものは JNI 越しに渡るだけで型検査が効かないため）。
    #[test]
    fn op_constants_match_java_side_contract() {
        assert_eq!(SECURE_CREDENTIAL_OP_STORE, 0);
        assert_eq!(SECURE_CREDENTIAL_OP_LOAD, 1);
        assert_eq!(SECURE_CREDENTIAL_OP_DELETE, 2);
    }
}

#[cfg(test)]
mod tts_tests {
    use super::{decode_tts_result, Mailbox};
    use fandhe_device::DeviceError;
    use std::sync::Mutex;

    /// JNI エクスポート関数（`nativeOnTtsResult`）を直接呼ぶテストは
    /// static `TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX` を共有するため、
    /// 並列実行で他テストの `begin_at`/`deliver` と干渉する（codex-review P2 指摘）。
    /// それ以外のテストは `Mailbox` をローカルに構築して使い、この直列化の
    /// 対象外にする（各テストが独立したインスタンスを持つため、テスト間の
    /// ID 採番が衝突しても問題ない）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn decode_tts_result_maps_status_table() {
        assert!(decode_tts_result(0).is_ok());
        assert!(matches!(
            decode_tts_result(1),
            Err(DeviceError::Unsupported("tts"))
        ));
        assert!(matches!(
            decode_tts_result(2),
            Err(DeviceError::Internal("tts"))
        ));
        assert!(matches!(
            decode_tts_result(3),
            Err(DeviceError::Internal("tts init"))
        ));
        assert!(matches!(
            decode_tts_result(4),
            Err(DeviceError::Internal("tts busy"))
        ));
        assert!(matches!(
            decode_tts_result(5),
            Err(DeviceError::Internal("tts speak"))
        ));
        assert!(matches!(
            decode_tts_result(6),
            Err(DeviceError::InvalidInput("tts text"))
        ));
        // 7 = stop による保留 speak のキャンセル。失敗として扱わない（案A 是正）。
        assert!(decode_tts_result(7).is_ok());
        // 未知の status も安全側（内部エラー）に畳む。
        assert!(matches!(
            decode_tts_result(99),
            Err(DeviceError::Internal("tts"))
        ));
    }

    // 以下は JNI・実機なしで検証できる mailbox の世代識別ロジック（案A の核心）を
    // 固定するホスト実行テスト。static を直接触らないテストはローカルな `Mailbox`
    // インスタンスを構築するため、他テストと並列実行しても干渉しない
    // （codex-review P2 指摘の是正）。

    /// stop による保留 speak のキャンセル（status 7）が `wait_outcome` 相当の
    /// 待ち（`TTS_OUTCOME_MAILBOX.wait_or_ok`）を失敗として扱わないことを固定する。
    #[test]
    fn outcome_mailbox_cancel_status_resolves_as_ok() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        outcome.begin_at(id);
        outcome.deliver(id, decode_tts_result(7));
        assert!(outcome.wait_or_ok(id).is_ok());
    }

    /// 未登録（追跡していない）ID は `wait_or_ok` がブロックせず即座に成功する
    /// （空発話が `wait_outcome` を呼んでも待たされないことの根拠、
    /// `Tts::wait_outcome` の「対応する処理が既に無い場合は即座に `Ok(())`」契約）。
    #[test]
    fn outcome_mailbox_wait_or_ok_is_immediate_for_untracked_id() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let untracked_id = 1;
        let started = std::time::Instant::now();
        assert!(outcome.wait_or_ok(untracked_id).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "追跡していない ID でブロックした: {:?}",
            started.elapsed()
        );
    }

    /// 別 ID（世代）向けに登録された結果を誤って回収しないことを固定する
    /// （codex-review P1 是正の核心: `CommandTts::wait_outcome` の
    /// `wait_outcome_ignores_result_of_a_later_speak_with_different_generation`
    /// と同型の担保）。id_a の結果を待つ前に id_b で上書きされていても、
    /// id_a の待ちは「追跡していない」として即座に `Ok(())` で抜け、id_b 自身の
    /// 結果は id_b の待ちだけが正しく受け取る。
    #[test]
    fn outcome_mailbox_does_not_collect_result_of_a_different_generation() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id_a = 1;
        let id_b = 2;
        outcome.begin_at(id_a);
        // id_a の結果が届く前に id_b へ差し替わる（`begin_at` は無条件上書き）。
        outcome.begin_at(id_b);
        outcome.deliver(id_b, Err(DeviceError::Internal("tts speak")));

        // id_a はもう追跡されていないため、id_b の失敗を誤って回収しない。
        assert!(outcome.wait_or_ok(id_a).is_ok());
    }

    /// 対象世代自身の失敗は `wait_or_ok` が正しく `Err` として返すことを固定する。
    #[test]
    fn outcome_mailbox_reports_failure_of_its_own_generation() {
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        outcome.begin_at(id);
        outcome.deliver(id, Err(DeviceError::Internal("tts speak")));
        assert!(matches!(
            outcome.wait_or_ok(id),
            Err(DeviceError::Internal("tts speak"))
        ));
    }

    /// `nativeOnTtsResult` のルーティング（`try_deliver` で accept 側へ先に届け、
    /// 一致しなければ outcome 側へ回す）を固定する: accept 側が `Pending` の
    /// ID（STOP 相当）は accept 側で受理され、outcome 側には届かない。
    #[test]
    fn accept_mailbox_receives_result_when_pending_and_outcome_is_untouched() {
        let accept: Mailbox<()> = Mailbox::new("test accept timeout");
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        accept.begin_at_if_idle(id);

        let delivered_to_accept = accept.try_deliver(id, Ok(()));
        assert!(delivered_to_accept);

        // accept 側で受理済みのため、outcome 側は未登録のまま
        // （＝待ってもブロックせず即座に成功する）。
        assert!(outcome.wait_or_ok(id).is_ok());
        assert!(accept.wait(id, std::time::Duration::from_secs(1)).is_ok());
    }

    /// accept 側が既に消費済み（`Idle`）の ID への配送は outcome 側へ回ることを
    /// 固定する（SPEAK が受理された後に真の最終結果が届く経路、モジュール doc
    /// 「2 本の mailbox」参照）。
    #[test]
    fn result_routes_to_outcome_mailbox_once_accept_already_consumed() {
        let accept: Mailbox<()> = Mailbox::new("test accept timeout");
        let outcome: Mailbox<()> = Mailbox::new("test outcome timeout");
        let id = 1;
        accept.begin_at_if_idle(id);
        // 受理シグナルを先に消費する（`AndroidTts::speak` の accept 待ちに相当）。
        assert!(accept.try_deliver(id, Ok(())));
        assert!(accept.wait(id, std::time::Duration::from_secs(1)).is_ok());

        outcome.begin_at(id);
        let delivered_to_accept = accept.try_deliver(id, Ok(()));
        assert!(
            !delivered_to_accept,
            "accept 側は既に Idle のため一致しないはず"
        );
        outcome.deliver(id, Ok(()));
        assert!(outcome.wait_or_ok(id).is_ok());
    }

    /// codex-review P1 再指摘（本 PR）の再発防止: 単相応答（`nativeOnTtsAccepted` を
    /// 経ずに `nativeOnTtsResult` が直接 `TTS_ACCEPT_MAILBOX` へ届く、SPEAK の
    /// `READY` 経路の再読み上げ成功等）が届いたとき、`TTS_OUTCOME_MAILBOX` が
    /// `Pending` のまま取り残されないことを固定する。`AndroidTts::speak` は
    /// JNI 呼び出し前に必ず `TTS_OUTCOME_MAILBOX.begin_at` 済み（`speak` 実装参照）
    /// のため、ここでも同じ順序（accept 登録 → outcome 登録 → 結果到着）を再現する。
    ///
    /// 実際の JNI エクスポート関数（`Java_..._nativeOnTtsResult`）を直接呼ぶため
    /// static `TTS_ACCEPT_MAILBOX`/`TTS_OUTCOME_MAILBOX` を経由する。並列実行での
    /// 他テストとの干渉を防ぐため [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_single_shot_success_also_resolves_outcome_mailbox() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_001;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        super::TTS_OUTCOME_MAILBOX.begin_at(id);

        // `jni::sys::jlong`/`jint` はそれぞれ `i64`/`i32` の型エイリアスのため
        // 素の整数値で呼び出せる。
        super::Java_com_fandhe_androidapp_MainActivity_nativeOnTtsResult(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            id as i64,
            0,
        );

        // accept 側（本来の宛先）が受理する。
        assert!(super::TTS_ACCEPT_MAILBOX
            .wait(id, std::time::Duration::from_secs(1))
            .is_ok());
        // outcome 側も同じ結果で確定済みのため、`wait_or_ok` はブロックせず
        // 即座に成功で戻る（`wait_or_ok` に時間による打ち切りは無いため、
        // ここで確定させ損なうと Pending のまま永久に応答しなくなる）。
        let started = std::time::Instant::now();
        assert!(super::TTS_OUTCOME_MAILBOX.wait_or_ok(id).is_ok());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "outcome 側が Pending のまま残っており待たされた: {:?}",
            started.elapsed()
        );
    }

    /// 同上・失敗版: 単相応答の失敗（例: `getMaxSpeechInputLength()` 超過、
    /// status 6）も outcome 側へ同じ `Err` が伝わることを固定する。static を
    /// 経由するため [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_single_shot_failure_also_resolves_outcome_mailbox() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_002;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        super::TTS_OUTCOME_MAILBOX.begin_at(id);

        super::Java_com_fandhe_androidapp_MainActivity_nativeOnTtsResult(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            id as i64,
            6,
        );

        assert!(matches!(
            super::TTS_ACCEPT_MAILBOX.wait(id, std::time::Duration::from_secs(1)),
            Err(DeviceError::InvalidInput("tts text"))
        ));
        assert!(matches!(
            super::TTS_OUTCOME_MAILBOX.wait_or_ok(id),
            Err(DeviceError::InvalidInput("tts text"))
        ));
    }

    /// 対照実験: STOP のように `TTS_OUTCOME_MAILBOX` を登録しない単相応答では、
    /// 重複配送（本 PR の是正）が無害であることを固定する（未登録 ID への
    /// `deliver` は無視される、`Mailbox::deliver` 参照）。static を経由するため
    /// [`TEST_LOCK`] で直列化する（codex-review P2 指摘）。
    #[test]
    fn native_on_tts_result_stop_style_single_shot_does_not_touch_unregistered_outcome() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = 2_000_003;
        super::TTS_ACCEPT_MAILBOX.begin_at_if_idle(id);
        // STOP は `TTS_OUTCOME_MAILBOX` を登録しない（`AndroidTts::stop` 参照）。

        super::Java_com_fandhe_androidapp_MainActivity_nativeOnTtsResult(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            id as i64,
            0,
        );

        assert!(super::TTS_ACCEPT_MAILBOX
            .wait(id, std::time::Duration::from_secs(1))
            .is_ok());
        // 追跡していないため即座に成功（＝重複配送で何も壊れていない）。
        assert!(super::TTS_OUTCOME_MAILBOX.wait_or_ok(id).is_ok());
    }

    /// codex-review P1 再指摘（本 PR）の再発防止: `wait_outcome(A)` が
    /// [`Mailbox::wait_or_ok`] の待機ループへ既に入った**後**に発話 B が来て
    /// [`Mailbox::begin_at`] がスロットを上書きしても、（`wait_or_ok` は時間で
    /// 打ち切らないため）`begin_at` の `notify_all` で即座に起こされて短時間で
    /// `Ok(())` を返すことを固定する（この `notify_all` が無いと、`wait_or_ok`
    /// は時間による打ち切りを持たないため B の後続結果が届くまで永久に眠り込む
    /// ことになり、`Tts::stop`/後続 `speak` による即時解決の契約が崩れる）。
    /// B 側（新しい世代）の結果は A の待機に一切影響されないことも合わせて見る。
    /// ローカル `Mailbox` を使うため static を経由せず、他テストと並列実行しても
    /// 干渉しない。
    #[test]
    fn wait_or_ok_wakes_immediately_when_begin_at_replaces_slot_while_waiting() {
        use std::sync::Arc;
        let outcome: Arc<Mailbox<()>> = Arc::new(Mailbox::new("test outcome timeout"));
        let id_a = 1;
        let id_b = 2;

        // `speak(A)` 相当: outcome 側を登録してから `wait_outcome(A)` を別スレッドで
        // 開始する（`device/tts.rs::run_speak` が背景スレッドへ切り出す形を模す）。
        outcome.begin_at(id_a);

        let waiter_a = {
            let outcome = Arc::clone(&outcome);
            std::thread::spawn(move || outcome.wait_or_ok(id_a))
        };

        // `waiter_a` が実際にロックを取ってループへ入る猶予を与える
        // （取る前に `begin_at(B)` が先行すると、そもそもループの初回チェックで
        // 「追跡していない」と判定され、この再発防止テストの意味が薄れるため）。
        std::thread::sleep(std::time::Duration::from_millis(50));

        // `speak(B)` 相当: A の待機が生きている間に B が outcome 側を上書きする。
        let started = std::time::Instant::now();
        outcome.begin_at(id_b);

        // `notify_all` により即座に起こされて「置き換え済み」を検知し `Ok(())` で
        // 戻る（`wait_or_ok` に時間による打ち切りが無いため、この通知が効いて
        // いなければ `waiter_a.join()` はテストプロセスごと永久にハングする）。
        let result_a = waiter_a.join().expect("panic しない");
        assert!(result_a.is_ok(), "{result_a:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "begin_at による置き換えから wait_or_ok が戻るまで長く掛かりすぎている              （notify_all が効いていない疑い）: {:?}",
            started.elapsed()
        );

        // B 自身の結果は A の待機に影響されず、正しく届く。
        outcome.deliver(id_b, Err(DeviceError::Internal("tts speak")));
        assert!(matches!(
            outcome.wait_or_ok(id_b),
            Err(DeviceError::Internal("tts speak"))
        ));
    }
}

// --------------------------------------------------------------------------
// ファイル選択（Issue #874、Phase 11 型サンプル「ファイル選択」の Android 経路、案 B）
// --------------------------------------------------------------------------
//
// iOS/Android は `FilePicker` trait を通らない（`fandhe_device::file_picker` モジュール
// doc の「モバイルは trait を通らない」、`device/file_pick.rs` の `bridge` モジュール
// 参照）。旧実装（trait 実装 + mailbox + ワーカースレッド）は 3 層管理が生まれ、
// Activity 再生成・プロセス死・遅延配送で継ぎ目がずれる問題があったため、案 B では
// 「提示 → requestId 付きで 1 回だけ通知 → 後始末」の 3 責務のみを Java 側に残し、
// 進行中・中断済みを表す static フラグは持たない（`filePickRequestId` doc
// （`MainActivity.java`）参照）。8 MiB 判定・読み取りは Rust 側
// `fandhe_device::file_picker::read_picked_from` に一本化する（Java 側で読み取りループを
// 書かない、仕様書「共有 Rust 部分」参照）。改訂 2 で fd をパス文字列
// （`/proc/self/fd/N`）ではなく `ParcelFileDescriptor.detachFd()` の生 fd で受け取る
// 方式に変更（pipe 型プロバイダで `/proc/self/fd/N` の再 open が権限再確認に失敗する
// ケースを避けるため）。

/// Java 側 `MainActivity.requestFilePick(long)` を呼び、ダイアログの提示だけを要求する
/// （Issue #874、案 B）。`AndroidClipboard::request`/`AndroidTts::call_tts_request` と
/// 同じ JNI 呼び出しの形だが、結果を待たずに即座に戻る点が異なる（呼び出し元
/// `device/file_pick.rs` の `bridge::begin`/`bridge::deliver` が結果の受け皿を持つ、
/// モジュール doc 参照）。渡す `request_id` は画面状態の generation ではなく
/// `bridge::begin` が発行するプロセス内一意の連番（改訂 2、モジュール doc 参照）。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。
/// 呼び出し元が [`crate::device::file_pick::bridge::deliver`] へ
/// `Err(DeviceError::Internal("file_picker present"))` を渡して畳む）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `AndroidClipboard::request`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_file_pick(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `AndroidClipboard::request`/`AndroidTts::call_tts_request` と同一の
    // 根拠。`app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` を返し、
    // `android_app()` は最新の `AndroidApp` を複製して返すため、ここで得た `app` は
    // 呼び出し時点で有効な `Activity` を指す。呼び出し中はローカル変数 `app` が
    // 生存を保つ（本関数は結果を待たず即座に戻るため、生存期間はこの呼び出し内で足りる）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `AndroidClipboard::request` と同一の根拠。`activity_as_ptr()`
            // は `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
            // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を
            // 試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestFilePick"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// DISPLAY_NAME からパスを剥がして末尾要素のみを採用し、NUL・空文字を拒否する
/// （`PickedFile.name` の不変条件「パスを含まない・NUL 無し」、`fandhe_device::file_picker`
/// モジュール doc 参照）。SAF のプロバイダが `/` を含む DISPLAY_NAME を返す場合に備え、
/// 呼び出し元へパスの断片を渡さない（security.md A03: 検証できない入力を通さない）。
fn sanitize_picked_name(raw: &str) -> Option<String> {
    let name = raw.rsplit('/').next().unwrap_or(raw);
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    Some(name.to_string())
}

/// Java 側から届いた `code`/`name`/`fd` を `PickedFile` へ変換する
/// （`nativeOnFilePickerResult` 本体）。`code` の意味は `MainActivity.
/// nativeOnFilePickerResult` 宣言コメントと対で決める: 0=選択（name/fd 必須）／
/// 1=キャンセル／2=失敗（Java 側で名前解決不能・fd 取得失敗・`launch()` 例外等を
/// 一律ここへ畳んでいる。詳細分類は行わない、仕様書「OS 側の受け口」参照）。
///
/// 8 MiB 判定・読み取り本体は [`fandhe_device::file_picker::read_picked_from`] に委譲する
/// （Java 側では読み取らない。SSOT はここではなく `fandhe-device` 側）。Android は
/// `fd` から元のファイル名を復元できないため、名前は Java から受け取った
/// DISPLAY_NAME を使う。
///
/// `fd` の所有権を受け取り、この関数内で読み切って close する（呼び出し元の
/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnFilePickerResult`] の
/// `# Safety` 節参照）。
// 本体内の 1 箇所の `unsafe`（`OwnedFd::from_raw_fd`）は関数 doc の SAFETY 記述参照。
#[allow(unsafe_code)]
fn decode_file_pick_result(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> Result<Option<PickedFile>, DeviceError> {
    match code {
        1 => return Ok(None),
        0 => {}
        _ => return Err(DeviceError::Internal("file_picker")),
    }

    if fd < 0 {
        return Err(DeviceError::Internal("file_picker"));
    }
    // 名前の検証より **先に** fd の所有権を取る。後続のどの失敗経路でも `file` の drop で
    // 必ず 1 回 close され、detach 済み fd がリークしない（codex P1）。
    //
    // SAFETY: Java 側 `deliverPickedDocument` が `ParcelFileDescriptor.detachFd()` で
    // 所有権を手放した直後に渡す有効な fd であり（`MainActivity.java` の
    // `deliverPickedDocument` 参照）、detach 後は Java 側で close しない契約
    // （detach 後の `ParcelFileDescriptor.close()` は no-op になる Android の仕様でもあり、
    // 万一 Java 側の契約が崩れても二重 close にはならない）。`OwnedFd` に包むことで
    // この関数を抜ける際（成功なら `read_picked_from` が読み切った直後、失敗なら早期
    // return 時の `File` の drop）に確実に 1 回だけ close される。呼び出し元は他の
    // どこにもこの fd を渡さないため、二重 close・use-after-close は発生しない。
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let file = std::fs::File::from(owned);

    if name.is_null() {
        return Err(DeviceError::Internal("file_picker"));
    }
    // modified UTF-8 → Rust の `String` 変換（jni 0.22 の非 deprecated API。
    // `nativeOnClipboardResult` と同じ手法）。
    let raw_name = name
        .try_to_string(env)
        .map_err(|_| DeviceError::Internal("file_picker"))?;
    let display_name =
        sanitize_picked_name(&raw_name).ok_or(DeviceError::Internal("file_picker"))?;

    let picked = fandhe_device::file_picker::read_picked_from(file, display_name, MAX_FILE_BYTES)?;
    Ok(Some(picked))
}

/// `MainActivity.nativeOnFilePickerResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #874、案 B）。シンボル名の固定規約は `nativeOnClipboardResult` と同じ。
///
/// [`decode_file_pick_result`] で `PickedFile` へ変換し、
/// [`crate::device::file_pick::bridge::deliver`] へ届ける。`request_id` が現在
/// 追跡中の要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が
/// 黙って捨てる（`bridge` モジュール doc 参照）。受け取った内容はログ出力・保存を
/// 一切行わない（選択されたファイルは他アプリの機密文書になり得る、security.md A09）。
///
/// 改訂 4（pull 型）: `bridge` は `CompletionSender` を持たないため、結果を `ready` に
/// 置くだけでは次フレームの描画が保証されない（アイドル中は `update` 自体が呼ばれない）。
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_mobile` が次フレームで [`crate::device::file_pick::bridge::snapshot`] を
/// 観測できるようにする（`nativeOnMotionChanged` と同じ経路。`deliver` が `false`
/// （不一致・重複）を返した場合でも repaint 自体は無害なので条件分岐しない）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは
/// [`decode_file_pick_result`] に閉じる）。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnClipboardResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnFilePickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_file_pick_result(env, code, &name, fd);
        crate::device::file_pick::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    // pull 型（改訂 4）: `bridge` は sender を持たないため、ここで明示的に再描画を
    // 要求しないと `drive_mobile` が次フレームで結果を観測できない（doc 参照）。
    crate::android::request_repaint();
}

// --------------------------------------------------------------------------
// 写真選択（Issue #1104、Phase 11 型サンプル「写真選択」の Android 経路）
// --------------------------------------------------------------------------
//
// ファイル選択（上）と同じ pull 型 bridge。提示は `requestPhotoPick(long)`、結果は
// `nativeOnPhotoPickerResult` で 1 度だけ届く。fd 受け取り・読み取り・上限判定は
// [`decode_file_pick_result`] / `read_picked_from` を共有する（name は Java 側の固定値
// `"photo"`。画面にもログにも出さない）。結果は `device/photo_pick.rs::bridge::deliver`
// が 2 段検査（`validate_photo` → 復号確認）まで済ませてから置く（Java のワーカー
// スレッド上で走り、UI スレッドでは復号しない）。

/// Java 側 `MainActivity.requestPhotoPick(long)` を呼び、ピッカーの提示だけを要求する。
/// [`request_file_pick`] と同じ JNI 呼び出しの形（結果を待たず即 return）。戻り値は JNI
/// 呼び出し自体の成否のみ（`false` は呼び出し元が
/// `Err(DeviceError::Internal("photo_picker present"))` として畳む）。署名 `(J)V` は
/// Java 側の定義と一致させる。
// 本体内の 2 箇所の `unsafe` は `request_file_pick` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_photo_pick(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_file_pick` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*`。ローカル変数 `app` が呼び出し中の生存を保つ。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_file_pick` と同一の根拠。`activity_as_ptr()` は `AndroidApp`
            // が保持する未所有のグローバル参照で、`Cast` は所有権を持たず delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestPhotoPick"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnPhotoPickerResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// 引数の意味は `nativeOnFilePickerResult` と同じ（`code`: 0=選択（name/fd 有効）／
/// 1=キャンセル／2=失敗。JPEG / PNG 以外の変換失敗は Java 側で 2 に畳む）。
/// [`decode_file_pick_result`] で `PickedFile` にし、写真選択の bridge へ届ける。`request_id`
/// が現在の要求と不一致（再生成後の遅延到着等）なら `deliver` が捨てる。受け取った内容は
/// ログ出力・保存しない（A09）。pull 型のため `deliver` の成否に関わらず再描画を要求する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは
/// [`decode_file_pick_result`] に閉じる）。`#[no_mangle]` は JNI 命名規約上の固定シンボル名
/// で、`nativeOnFilePickerResult` と同じ根拠で他シンボルと衝突しない。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnPhotoPickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_file_pick_result(env, code, &name, fd);
        crate::device::photo_pick::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}

// --------------------------------------------------------------------------
// 連絡先選択（Issue #1222、Phase 11 型サンプル「連絡先選択」の Android 経路、親 #1117）
// --------------------------------------------------------------------------
//
// 写真選択（上）と同じ pull 型 bridge。提示は `requestContactPick(long)`、結果は
// `nativeOnContactPickerResult` で 1 度だけ届く。コードは 0=選択（name 有効）/ 1=キャンセル
// / 3=失敗で、写像は `device/contact_pick.rs::outcome_from_code` に一本化する。

/// Java 側 `MainActivity.requestContactPick(long)` を呼び、ピッカーの提示だけを要求する。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` は呼び出し元が `Internal` に畳む）。
/// 署名 `(J)V` は Java 側の定義と一致させる。
// 本体内の 2 箇所の `unsafe` は `request_file_pick` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_contact_pick(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_file_pick` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*`。ローカル変数 `app` が呼び出し中の生存を保つ。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_file_pick` と同一の根拠。`activity_as_ptr()` は `AndroidApp`
            // が保持する未所有のグローバル参照で、`Cast` は所有権を持たず delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestContactPick"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnContactPickerResult`（Java 側）から呼ばれる JNI エクスポート関数。
/// `code`: 0=選択（`name` 有効）/ 1=キャンセル/ 3=失敗。`name` が null・変換不能なら `None`
/// とし、`outcome_from_code` が `Internal` にする（成功にしない）。`request_id` 不一致
/// （再生成後の遅延到着等）は `deliver` が捨てる。名前はログ出力しない（A09）。pull 型の
/// ため `deliver` の成否に関わらず再描画を要求する。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnContactPickerResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = if name.is_null() {
            None
        } else {
            name.try_to_string(env).ok()
        };
        let result = crate::device::contact_pick::outcome_from_code(code, name);
        crate::device::contact_pick::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}

#[cfg(test)]
mod file_pick_tests {
    use super::sanitize_picked_name;

    #[test]
    fn sanitize_picked_name_strips_leading_path() {
        assert_eq!(
            sanitize_picked_name("a/b/名前.txt").as_deref(),
            Some("名前.txt")
        );
    }

    #[test]
    fn sanitize_picked_name_rejects_nul_and_empty() {
        assert_eq!(sanitize_picked_name(""), None);
        assert_eq!(sanitize_picked_name("a/"), None);
        assert_eq!(sanitize_picked_name("na\0me"), None);
    }

    #[test]
    fn sanitize_picked_name_keeps_plain_name() {
        assert_eq!(sanitize_picked_name("a.txt").as_deref(), Some("a.txt"));
    }
}

// --------------------------------------------------------------------------
// 位置情報（Issue #890、Phase 11 型サンプル「位置情報」の Android 経路、親 #887）
// --------------------------------------------------------------------------
//
// `fandhe_device::location::SharedLocation` は継続 push 型（束 `continuous-push`）で、
// カメラ・ファイル選択のような request/response 型の mailbox を持たない
// （`device/location.rs` モジュール doc 参照）。開始要求のみ Rust → Java の片道 JNI
// 呼び出し（`request_file_pick` と同じ形）で行い、実測位は Java 側 `LocationListener`
// から `nativeOnLocationChanged`/`nativeOnLocationFailure` で継続的に届いて
// `crate::device::location::shared_location_sink()` へ直接 push される
// （`SharedLocation::push_fix`/`push_failure` の呼び手はこの 2 関数のみ）。

/// Java 側 `MainActivity.requestLocationUpdates(boolean)` を呼び、位置情報の測位開始を要求する
/// （Issue #890）。`precise` は精密位置（FINE）が要るか（ジオフェンスは真、位置情報画面は偽で
/// COARSE のみ要求、最小権限）。JNI 署名 `(Z)V` は Java 側の定義と一致させる。`request_file_pick` と同じ JNI 呼び出しの形で、結果を待たずに即座に
/// 戻る（実測位は `nativeOnLocationChanged`/`nativeOnLocationFailure` が非同期に届ける）。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// [`crate::device::location::shared_location`] が `push_failure(LocationFailure::Failed)`
/// へ畳む）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_file_pick`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_location_updates(precise: bool) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_file_pick` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*` を返し、`android_app()` は最新の `AndroidApp` を複製して
    // 返すため、ここで得た `app` は呼び出し時点で有効な `Activity` を指す。呼び出し中は
    // ローカル変数 `app` が生存を保つ（本関数は結果を待たず即座に戻るため、生存期間は
    // この呼び出し内で足りる）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_file_pick` と同一の根拠。`activity_as_ptr()` は
            // `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
            // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestLocationUpdates"),
                jni::jni_sig!((boolean) -> ()),
                &[jni::objects::JValue::Bool(precise)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `nativeOnLocationFailure` の `code` を [`fandhe_device::location::LocationFailure`] へ
/// 写す純関数（`cfg` 無しでホストからテストできる、`decode_capture_result` と同じ
/// 切り出し方針）。対応表は `MainActivity.nativeOnLocationFailure` 宣言コメントと対で
/// 決める: 1=権限拒否／2=サービス無効／それ以外=その他の失敗（未知コードもここへ丸め、
/// panic しない。security.md A03: 想定外の入力を検証せず通さない）。
fn decode_location_failure(code: jni::sys::jint) -> fandhe_device::location::LocationFailure {
    use fandhe_device::location::LocationFailure;
    match code {
        1 => LocationFailure::PermissionDenied,
        2 => LocationFailure::ServiceDisabled,
        _ => LocationFailure::Failed,
    }
}

/// `MainActivity.nativeOnLocationChanged`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #890）。シンボル名の固定規約は `nativeOnMotionChanged` と同じ。
///
/// [`crate::device::location::shared_location_sink`] へ直接 `push_fix` する
/// （`shared_location()` を呼ぶと未初期化時の起動処理（`request_location_updates` の
/// 初回呼び出し）が再入するため使わない、`location.rs` の `shared_location_sink` doc
/// 参照）。座標をログへ出力しない（security.md A09）。
///
/// `JNIEnv`/`jclass` は本関数内で一切使わず（受け取った値を計算して共有状態へ store
/// するのみ）、生ポインタへのアクセスを行わないため関数本体に `unsafe` 操作は
/// 含まない（`nativeOnMotionChanged` と同じ形）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib として
/// 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`nativeOnMotionChanged`・`nativeOnCameraResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnLocationChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    latitude: f64,
    longitude: f64,
    accuracy: f32,
) {
    crate::device::location::shared_location_sink().push_fix(fandhe_device::location::Fix {
        latitude_deg: latitude,
        longitude_deg: longitude,
        accuracy_m: accuracy as f64,
    });
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnLocationFailure`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #890）。[`decode_location_failure`] で変換し、
/// [`crate::device::location::shared_location_sink`] へ `push_failure` する。
///
/// # Safety
/// `nativeOnLocationChanged` と同一の根拠（生ポインタへのアクセスを行わない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnLocationFailure(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    code: jni::sys::jint,
) {
    crate::device::location::shared_location_sink().push_failure(decode_location_failure(code));
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnLocationRetrying`（Java 側）から呼ばれる JNI エクスポート関数
/// （cursor Bugbot 指摘、PR #894）。権限再取得・provider 再有効化で測位を再開する
/// 瞬間に呼ばれ、[`crate::device::location::shared_location_sink`] を
/// [`fandhe_device::location::SharedLocation::reset_to_waiting`] で「未取得」へ戻す。
/// `ensure_windows_driver`（Windows 経路）が明示的な再試行の直前に同じメソッドを
/// 呼ぶのと同じ契約で、coarse/network provider では再登録後も新しい fix が届くとは
/// 限らないため、この呼び出しが無いと古い Denied/Disabled 表示が残留し続ける。
///
/// # Safety
/// `nativeOnLocationChanged` と同一の根拠（生ポインタへのアクセスを行わない）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnLocationRetrying(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
    crate::device::location::shared_location_sink().reset_to_waiting();
    crate::android::request_repaint();
}

#[cfg(test)]
mod location_tests {
    use super::decode_location_failure;
    use fandhe_device::location::LocationFailure;

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

// --------------------------------------------------------------------------
// 共有シート（Issue #899、Phase 11 型サンプル「共有」の Android 経路、親 #896）
// --------------------------------------------------------------------------
//
// `fandhe_device::share::Share` trait を通らない（`device/share.rs` モジュール doc の
// 「macOS/iOS/Android は pull 型 bridge」参照）。`Intent.createChooser` は標準では
// 選択結果を返さない（`StartActivityForResult` の結果は常に `RESULT_CANCELED`）ため、
// Java 側 `MainActivity.requestShare` が `IntentSender`（`Intent.createChooser` の
// 3 引数版）で選択完了 broadcast を受け取り、受信できれば `Shared`、chooser が閉じても
// broadcast が届かなければ `Unknown`（結果不明、`device/share.rs::outcome_from_code`
// の code 4）として通知する（実装計画 §2.3 参照）。

/// Java 側 `MainActivity.requestShare(long, String, String)` を呼び、共有シートの提示を
/// 要求する（Issue #899）。`request_file_pick`/`request_location_updates` と同じ JNI
/// 呼び出しの形で、結果を待たずに即座に戻る（実結果は `nativeOnShareResult` が非同期に
/// 届ける、`device/share.rs::bridge` 参照）。`url` が無い場合は Java 側へ `null` を渡す
/// （`ShareRequest::url()` が `None` の場合。Java 側は `null` を「URL 無し」と解釈する
/// 契約、`MainActivity.requestShare` 宣言コメントと対）。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// [`crate::device::share::present`]（Android 分岐）が `false` を見て
/// `Err(DeviceError::Internal("share present"))` として `bridge::deliver` する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_file_pick`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_share(request_id: u64, request: &ShareRequest) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_file_pick` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*` を返し、`android_app()` は最新の `AndroidApp` を複製して
    // 返すため、ここで得た `app` は呼び出し時点で有効な `Activity` を指す。呼び出し中は
    // ローカル変数 `app` が生存を保つ（本関数は結果を待たず即座に戻るため、生存期間は
    // この呼び出し内で足りる）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let j_text = env.new_string(request.text())?;
            let j_url = match request.url() {
                Some(url) => jni::objects::JObject::from(env.new_string(url)?),
                None => jni::objects::JObject::null(),
            };
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_file_pick` と同一の根拠。`activity_as_ptr()` は
            // `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
            // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestShare"),
                jni::jni_sig!((long, java.lang.String, java.lang.String) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_text),
                    jni::objects::JValue::Object(&j_url),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnShareResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #899）。シンボル名の固定規約は `nativeOnFilePickerResult` と同じ。`code` の
/// 意味は `device/share.rs::outcome_from_code` の表（iOS/macOS/Android 共通）と対で
/// 決める。
///
/// [`super::share::outcome_from_code`] で変換し、[`super::share::bridge::deliver`] へ
/// 届ける。`request_id` が現在追跡中の要求と不一致（プロセス死後の再配送・旧要求の
/// 遅延到着）なら `deliver` が黙って捨てる（`bridge` モジュール doc 参照）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnFilePickerResult` と同じ理由）。共有テキスト・URL は引数に含まれないため
/// ログ出力の懸念は無い。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnFilePickerResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnShareResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = super::share::outcome_from_code(code);
    super::share::bridge::deliver(request_id as u64, outcome);
    crate::android::request_repaint();
}

// 生体認証（Issue #908）。Java 側は `MainActivity.requestBiometric(long, String)`（提示
// 依頼）/ `nativeOnBiometricResult`（結果通知）で `device/biometric_auth.rs::bridge` と
// pull 型で橋渡しする（`共有` 節と同型、Issue #899 と同じ設計判断）。

/// Java 側 `MainActivity.requestBiometric(long, String)` を呼び、生体認証プロンプトの
/// 提示を要求する（Issue #908）。`request_share` と同じ JNI 呼び出しの形で、結果を
/// 待たずに即座に戻る（実結果は `nativeOnBiometricResult` が非同期に届ける、
/// `device/biometric_auth.rs::bridge` 参照）。`reason` は呼び出し元
/// （`device/biometric_auth.rs::present`）で `validate_reason` 済みの非空文字列。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// （`device/biometric_auth.rs::present`、Android 分岐）が `false` を見て
/// `Err(DeviceError::Internal("biometric present"))` として `bridge::deliver` する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_share`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_biometric(request_id: u64, reason: &str) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_share` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*` を返し、`android_app()` は最新の `AndroidApp` を複製して
    // 返すため、ここで得た `app` は呼び出し時点で有効な `Activity` を指す。呼び出し中は
    // ローカル変数 `app` が生存を保つ（本関数は結果を待たず即座に戻るため、生存期間は
    // この呼び出し内で足りる）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let j_reason = env.new_string(reason)?;
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_share` と同一の根拠。`activity_as_ptr()` は
            // `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
            // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestBiometric"),
                jni::jni_sig!((long, java.lang.String) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_reason),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnBiometricResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #908）。シンボル名の固定規約は `nativeOnShareResult` と同じ。`code` の意味は
/// `device/biometric_auth.rs::outcome_from_android_code` の doc（Java 側 `MainActivity`
/// の code 定数と対）を参照。
///
/// [`super::biometric_auth::outcome_from_android_code`] で変換し、
/// [`super::biometric_auth::bridge::deliver`] へ届ける。`request_id` が現在追跡中の要求と
/// 不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnShareResult` と同じ理由）。生体データ・識別子は引数に含まれないため
/// ログ出力の懸念は無い（security.md A09）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnShareResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnBiometricResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = super::biometric_auth::outcome_from_android_code(code);
    super::biometric_auth::bridge::deliver(request_id as u64, outcome);
    crate::android::request_repaint();
}

// マイク録音（Issue #957）。Java 側は `MainActivity.requestMicRecordStart(long, int)`/
// `requestMicRecordStop(long)`（提示依頼）/ `nativeOnMicRecordStarted`・
// `nativeOnMicRecordResult`（結果通知）で `device/mic_record.rs::bridge` と pull 型で
// 橋渡しする（`request_biometric`/`nativeOnBiometricResult` と同型、Issue #908 と
// 同じ設計判断）。

/// Java 側 `MainActivity.requestMicRecordStart(long, int)` を呼び、録音開始（権限確認
/// 込み）を要求する（Issue #957）。`request_biometric` と同じ JNI 呼び出しの形で、
/// 結果を待たずに即座に戻る（実結果は `nativeOnMicRecordStarted`/
/// `nativeOnMicRecordResult` が非同期に届ける、`device/mic_record.rs::bridge` 参照）。
/// `max_duration_ms` は呼び出し元（`device/mic_record.rs::present`、Android 分岐）が
/// `MAX_RECORD_MS`（30 秒固定）から変換済み。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// が `false` を見て `Err(DeviceError::Internal("mic_record present"))` として
/// `bridge::deliver` する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_biometric`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_mic_record_start(request_id: u64, max_duration_ms: i32) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestMicRecordStart"),
                jni::jni_sig!((long, int) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_duration_ms),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// Java 側 `MainActivity.requestMicRecordStop(long)` を呼び、録音停止を要求する
/// （Issue #957）。`request_mic_record_start` と同じ形。`request_id` が Java 側の
/// 追跡と不一致（権限確認中の別要求・旧要求）なら Java 側が no-op として無視する
/// 契約（`MainActivity.requestMicRecordStop` 参照）。
// 本体内の `unsafe` は `request_mic_record_start` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_mic_record_stop(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestMicRecordStop"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// Java 側から届いた録音結果を `Recording` へ変換する（`nativeOnMicRecordResult` 本体）。
/// `code` の意味は `device/mic_record.rs::error_from_code`（Java 側 `MIC_RESULT_*`
/// 定数が SSOT、`MainActivity.java` のコメント参照）を参照。`code == 0`（成功）以外は
/// `bytes`/`mime` を参照しない（Java 側は null を渡す契約、`MainActivity` 参照）。
fn decode_mic_record_result(
    env: &jni::Env,
    code: jni::sys::jint,
    bytes: &jni::objects::JByteArray,
    mime: &jni::objects::JString,
    duration_ms: jni::sys::jlong,
) -> Result<fandhe_device::mic_record::Recording, DeviceError> {
    if code != 0 {
        return Err(super::mic_record::error_from_code(code));
    }
    if bytes.is_null() || mime.is_null() {
        // 契約違反（成功なのに null）を検出した経路。
        return Err(DeviceError::Internal("mic_record: null result on success"));
    }
    // 変換前（`convert_byte_array` が Vec を確保する前）に配列長を検査する
    // （security.md A05、メモリ枯渇防止。`decode_capture_result` の `MAX_IMAGE_BYTES`
    // 検査と同型。`build_recording` 側の検査は防御の二重化として残す）。
    let len = bytes
        .len(env)
        .map_err(|_| DeviceError::Internal("mic_record"))?;
    if len > fandhe_device::mic_record::MAX_RECORDING_BYTES {
        return Err(DeviceError::Internal("mic_record size"));
    }
    let owned_bytes = env
        .convert_byte_array(bytes)
        .map_err(|_| DeviceError::Internal("mic_record"))?;
    let mime_str = mime
        .try_to_string(env)
        .map_err(|_| DeviceError::Internal("mic_record"))?;
    fandhe_device::mic_record::build_recording(owned_bytes, mime_str, duration_ms.max(0) as u64)
}

/// `MainActivity.nativeOnMicRecordStarted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #957）。シンボル名の固定規約は `nativeOnBiometricResult` と同じ。
/// `MediaRecorder.start()` が成功した瞬間に Java 側から呼ばれ、
/// [`super::mic_record::bridge::mark_started`] へ橋渡しする（`device/mic_record.rs::
/// drive_callback` が経過時間の計測を開始する合図、モジュール doc 参照）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBiometricResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnMicRecordStarted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    super::mic_record::bridge::mark_started(request_id as u64);
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnMicRecordResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #957）。シンボル名の固定規約は `nativeOnFilePickerResult` と同じ。
///
/// [`decode_mic_record_result`] で `Recording`/`DeviceError` へ変換し、
/// [`super::mic_record::bridge::deliver`] へ届ける。`request_id` が現在追跡中の要求と
/// 不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。録音データ・例外メッセージはログへ出力しない
/// （security.md A09）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnFilePickerResult` と同じ理由）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnFilePickerResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnMicRecordResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    bytes: jni::objects::JByteArray<'caller>,
    mime: jni::objects::JString<'caller>,
    duration_ms: jni::sys::jlong,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_mic_record_result(env, code, &bytes, &mime, duration_ms);
        super::mic_record::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}

// `decode_mic_record_result`（code 表の写像）は `android.rs` 自体が Android
// ターゲットのみでコンパイルされる（`device.rs` の `#[cfg(target_os = "android")]`
// 参照）ため host `cargo test` では検証できない。写像表の SSOT
// （`device/mic_record.rs::error_from_code`）は host で常時コンパイル・テストされる
// （`mic_record.rs::tests::error_from_code_maps_table` 参照）ため、二重にテストを
// 置かない。

// 音声認識（Issue #1026）。Java 側は `MainActivity.requestSpeechRecognitionStart(long,
// int)`/`requestSpeechRecognitionStop(long)`（提示依頼）/ `nativeOnSpeechStarted`・
// `nativeOnSpeechResult`・`nativeOnSpeechError`（結果通知）で `device/
// speech_recognition.rs::bridge` と pull 型で橋渡しする（`request_mic_record_start`/
// `nativeOnMicRecordResult` と同型、Issue #957 と同じ設計判断）。`onError` の生値は
// Java 側で分岐せず `nativeOnSpeechError` でそのまま渡し、写像は
// `device/speech_recognition.rs::outcome_from_android_error`（SSOT、host でテスト済み）
// に一元化する（device-bridge-contracts.md「FFI 境界」節）。

/// Java 側 `MainActivity.requestSpeechRecognitionStart(long, int)` を呼び、聞き取り開始
/// （権限確認込み）を要求する（Issue #1026）。`request_mic_record_start` と同じ JNI
/// 呼び出しの形で、結果を待たずに即座に戻る（実結果は `nativeOnSpeechStarted`/
/// `nativeOnSpeechResult`/`nativeOnSpeechError` が非同期に届ける、`device/
/// speech_recognition.rs::bridge` 参照）。`max_listen_ms` は呼び出し元
/// （`device/speech_recognition.rs::present`、Android 分岐）が `MAX_LISTEN_MS`
/// （30 秒固定）から変換済み。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// が `false` を見て `Err(DeviceError::Internal("speech_recognition present"))` として
/// `bridge::deliver` する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_biometric`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_speech_start(request_id: u64, max_listen_ms: i32) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestSpeechRecognitionStart"),
                jni::jni_sig!((long, int) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(max_listen_ms),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// Java 側 `MainActivity.requestSpeechRecognitionStop(long)` を呼び、聞き取り停止
/// （＝「話し終わった」の合図）を要求する（Issue #1026）。`request_speech_start` と
/// 同じ形。`request_id` が Java 側の追跡と不一致（権限確認中の別要求・旧要求）なら
/// Java 側が no-op として無視する契約（`MainActivity.requestSpeechRecognitionStop`
/// 参照）。
// 本体内の `unsafe` は `request_speech_start` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_speech_stop(request_id: u64) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestSpeechRecognitionStop"),
                jni::jni_sig!((long) -> ()),
                &[jni::objects::JValue::Long(request_id as jni::sys::jlong)],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// Java 側から届いた聞き取り結果を `SpeechOutcome` へ変換する（`nativeOnSpeechResult`
/// 本体）。`code == 0`（成功）以外は `text` を参照せず
/// `device/speech_recognition.rs::error_from_code`（Java 側の独自コード体系、
/// `RecognitionListener.onError` の生値とは別の SSOT）へ委譲する（`decode_mic_record_result`
/// と同じ形）。
fn decode_speech_result(
    env: &mut jni::Env,
    code: jni::sys::jint,
    text: &jni::objects::JString,
) -> Result<fandhe_device::speech_recognition::SpeechOutcome, DeviceError> {
    if code != 0 {
        return super::speech_recognition::error_from_code(code);
    }
    if text.is_null() {
        // 契約違反（成功なのに null）を検出した経路。
        return Err(DeviceError::Internal(
            "speech_recognition: null result on success",
        ));
    }
    // 全文を Rust の `String` へ複製する前に長さで弾く（PR #1030 codex 指摘）。
    // `String.length()` は UTF-16 単位数で、UTF-8 では 1 単位あたり 1 バイト以上に
    // なるため、単位数が上限を超えていれば UTF-8 でも必ず超える。上限以内なら複製は
    // 高々 3 倍で収まり、厳密なバイト数の判定は `outcome_from_transcript` が行う。
    let units = env
        .call_method(text, jni::jni_str!("length"), jni::jni_sig!(() -> int), &[])
        .and_then(|v| v.i())
        .map_err(|_| DeviceError::Internal("speech_recognition"))?;
    if usize::try_from(units).unwrap_or(usize::MAX)
        > fandhe_device::speech_recognition::MAX_TRANSCRIPT_BYTES
    {
        return Err(DeviceError::InvalidInput("speech_recognition size"));
    }
    let text_str = text
        .try_to_string(env)
        .map_err(|_| DeviceError::Internal("speech_recognition"))?;
    fandhe_device::speech_recognition::outcome_from_transcript(text_str)
}

/// `MainActivity.nativeOnSpeechStarted`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。シンボル名の固定規約は `nativeOnMicRecordStarted` と同じ。
/// `SpeechRecognizer.startListening()` に対応する `onReadyForSpeech` が届いた瞬間に
/// Java 側から呼ばれ、[`super::speech_recognition::bridge::mark_started`] へ橋渡しする
/// （`device/speech_recognition.rs::drive_callback` が経過時間の計測を開始する合図、
/// モジュール doc 参照）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnMicRecordStarted` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnSpeechStarted(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
) {
    super::speech_recognition::bridge::mark_started(request_id as u64);
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnSpeechResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。シンボル名の固定規約は `nativeOnMicRecordResult` と同じ。
///
/// [`decode_speech_result`] で `SpeechOutcome`/`DeviceError` へ変換し、
/// [`super::speech_recognition::bridge::deliver`] へ届ける。`request_id` が現在追跡中の
/// 要求と不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。認識結果の文字列はログへ出力しない
/// （security.md A09）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnMicRecordResult` と同じ理由）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnMicRecordResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnSpeechResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
    text: jni::objects::JString<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let result = decode_speech_result(env, code, &text);
        super::speech_recognition::bridge::deliver(request_id as u64, result);
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}

/// `MainActivity.nativeOnSpeechError`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1026）。`RecognitionListener.onError(int)` の生値をそのまま受け取り、
/// `device/speech_recognition.rs::outcome_from_android_error`（SSOT、host でテスト済み）
/// で `SpeechOutcome`/`DeviceError` へ変換してから
/// [`super::speech_recognition::bridge::deliver`] へ届ける。`JString` を扱わないため
/// `nativeOnBiometricResult` と同じ生ポインタ形式で十分（`EnvUnowned` 不要）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBiometricResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnSpeechError(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    error: jni::sys::jint,
) {
    let outcome = super::speech_recognition::outcome_from_android_error(error);
    super::speech_recognition::bridge::deliver(request_id as u64, outcome);
    crate::android::request_repaint();
}

// BLE スキャン（Issue #1062）。Java 側は `MainActivity.requestBleScan(long, int)`（開始依頼）/
// `nativeOnBleScanResult`（広告ごと）・`nativeOnBleScanFinished`（終了 1 回）で
// `device/bluetooth_le.rs` の `bridge` / `OBSERVED`（macOS / Windows と共通）へ pull 型で
// 橋渡しする。完了コードの写像は `device/bluetooth_le.rs::error_from_android_code`
// （SSOT、host でテスト済み）に一元化し、Java 側では分岐しない。周辺機器の名前・
// アドレスはログへ出さない（security.md A09）。

/// BLE の識別子（アドレス）の UTF-16 単位数の上限（MAC は 17。これを超える値は捨てる）。
const BLE_ID_MAX_UNITS: i32 = 64;
/// BLE の広告名の UTF-16 単位数の上限（GAP の Local Name は最大 248 バイト）。
const BLE_NAME_MAX_UNITS: i32 = 248;

/// Java 側 `MainActivity.requestBleScan(long, int)` を呼び、スキャン開始（権限確認込み）を
/// 要求する（Issue #1062）。`request_speech_start` と同じ JNI 呼び出しの形で、結果を待たず
/// 即座に戻る。`duration_ms` は呼び出し元が `SCAN_DURATION_MS`（SSOT）から変換済みで、
/// Java はそのまま期限に使う（期限を複製しない、device-bridge-contracts.md 3 節）。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` なら呼び出し元が `Internal` で完了させる）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_biometric`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_ble_scan(request_id: u64, duration_ms: i32) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestBleScan"),
                jni::jni_sig!((long, int) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Int(duration_ms),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// JNI の文字列を、UTF-16 単位数が `max_units` 以内のときだけ Rust の `String` へ複製する
/// （null・超過・読み取り失敗は `None`。OS 由来でも信頼境界の外として複製前に弾く）。
fn read_ble_string(
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

/// `MainActivity.nativeOnBleScanResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1062）。広告 1 件を `OBSERVED` へ足す（要求 id 不一致は `Observer` が捨てる）。
/// `address` が読めない・長すぎる広告は捨てる。`rssi` は Java の生値（取得不能の番兵も
/// 127 のまま）を `rssi_from_apple` で `None` へ写す。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnSpeechResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnBleScanResult<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    address: jni::objects::JString<'caller>,
    name: jni::objects::JString<'caller>,
    rssi: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        if let Some(id) = read_ble_string(env, &address, BLE_ID_MAX_UNITS) {
            let name = read_ble_string(env, &name, BLE_NAME_MAX_UNITS);
            super::bluetooth_le::OBSERVED.observe(
                request_id as u64,
                fandhe_device::bluetooth_le::BleDevice {
                    id,
                    name,
                    rssi_dbm: super::bluetooth_le::rssi_from_apple(rssi),
                },
            );
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
}

/// `MainActivity.nativeOnBleScanFinished`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1062）。`code == 0` は成功、それ以外は `error_from_android_code` で写す。
/// 要求 id が古ければ `finish` が捨てる。`JString` を扱わないため生ポインタ形式で足りる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnSpeechError` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnBleScanFinished(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let result = if code == 0 {
        Ok(())
    } else {
        Err(super::bluetooth_le::error_from_android_code(code))
    };
    super::bluetooth_le::finish(request_id as u64, result);
    crate::android::request_repaint();
}

// ローカル通知（Issue #1006）。Java 側は `MainActivity.requestNotification(long, String,
// String)`（提示依頼）/ `nativeOnNotificationResult`（結果通知）で `device/
// notification.rs::bridge` と pull 型で橋渡しする（`request_biometric`/
// `nativeOnBiometricResult` と同型、Issue #908 と同じ設計判断）。

/// Java 側 `MainActivity.requestNotification(long, String, String)` を呼び、通知の
/// 表示（権限確認込み）を要求する（Issue #1006）。`request_biometric` と同じ JNI
/// 呼び出しの形で、結果を待たずに即座に戻る（実結果は `nativeOnNotificationResult`
/// が非同期に届ける、`device/notification.rs::bridge` 参照）。`title`/`body` は
/// 呼び出し元（`device/notification.rs::present`、Android 分岐）で
/// `NotificationRequest::new` 検証済みの文字列（ログには出さない、security.md A09）。
///
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` はメソッド未検出・例外送出等。呼び出し元
/// が `false` を見て `Err(DeviceError::Internal("notification present"))` として
/// `bridge::deliver` する）。
// 本体内の 2 箇所の `unsafe`（`JavaVM::from_raw` / `as_cast_raw`）は `request_biometric`
// と同一の根拠（各 `// SAFETY:` 参照）。
#[allow(unsafe_code)]
pub(crate) fn request_notification(request_id: u64, title: &str, body: &str) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_biometric` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*` を返し、`android_app()` は最新の `AndroidApp` を複製して
    // 返すため、ここで得た `app` は呼び出し時点で有効な `Activity` を指す。呼び出し中は
    // ローカル変数 `app` が生存を保つ（本関数は結果を待たず即座に戻るため、生存期間は
    // この呼び出し内で足りる）。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let j_title = env.new_string(title)?;
            let j_body = env.new_string(body)?;
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_biometric` と同一の根拠。`activity_as_ptr()` は
            // `AndroidApp` が保持する未所有の JNI グローバル参照で、`AndroidApp` の
            // 生存期間中有効。`Cast` は所有権を持たないラッパーのため delete を試みない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            env.call_method(
                &activity,
                jni::jni_str!("requestNotification"),
                jni::jni_sig!((long, java.lang.String, java.lang.String) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_title),
                    jni::objects::JValue::Object(&j_body),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnNotificationResult`（Java 側）から呼ばれる JNI エクスポート
/// 関数（Issue #1006）。シンボル名の固定規約は `nativeOnBiometricResult` と同じ。
/// `code` の意味は `device/notification.rs::outcome_from_code` の doc（Java 側
/// `NOTIFICATION_RESULT_*` 定数と対）を参照。
///
/// [`super::notification::outcome_from_code`] で変換し、
/// [`super::notification::bridge::deliver`] へ届ける。`request_id` が現在追跡中の要求と
/// 不一致（プロセス死後の再配送・旧要求の遅延到着）なら `deliver` が黙って捨てる
/// （`bridge` モジュール doc 参照）。
///
/// `deliver` の成否に関わらず無条件で [`crate::android::request_repaint`] を呼び、
/// `drive_callback` が次フレームで `bridge::snapshot` を観測できるようにする
/// （`nativeOnBiometricResult` と同じ理由）。タイトル・本文は引数に含まれないため
/// ログ出力の懸念は無い（security.md A09）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnBiometricResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnNotificationResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = super::notification::outcome_from_code(code);
    super::notification::bridge::deliver(request_id as u64, outcome);
    crate::android::request_repaint();
}

// 予定の追加（Issue #1226）。Java 側 `MainActivity.requestCalendarInsert` /
// `nativeOnCalendarResult` で `device/calendar.rs::bridge` と pull 型で橋渡しする
// （`request_notification` と同型）。Java 側に対になる no-op スタブは置かない:
// この関数を経ないと `nativeOnCalendarResult` を呼ぶ経路自体が無いため。

/// Java 側 `MainActivity.requestCalendarInsert(long, String, int x10)` を呼び、カレンダー
/// アプリの編集画面を開く（Issue #1226）。結果は `nativeOnCalendarResult` が非同期に届ける。
/// 件名・日時は `EventDraft::parse` 検証済みで、ログには出さない（security.md A09）。
/// 戻り値は JNI 呼び出し自体の成否のみ（`false` は呼び出し元が失敗として畳む）。
// 本体内の 2 箇所の `unsafe` は `request_notification` と同一の根拠。
#[allow(unsafe_code)]
pub(crate) fn request_calendar_insert(
    request_id: u64,
    draft: &fandhe_device::calendar::EventDraft,
) -> bool {
    let Some(app) = crate::android::android_app() else {
        return false;
    };

    // SAFETY: `request_notification` と同一の根拠。`app.vm_as_ptr()` は android-activity が
    // 管理する有効な `JavaVM*` で、`app` は本呼び出しの間生存する。
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

    let (s, e) = (draft.start(), draft.end());
    let request_result =
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<(), JniCallFailed> {
            let j_title = env.new_string(draft.title())?;
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: `request_notification` と同一の根拠。未所有のグローバル参照を
            // 所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            let i = |v: u8| jni::objects::JValue::Int(v as jni::sys::jint);
            env.call_method(
                &activity,
                jni::jni_str!("requestCalendarInsert"),
                jni::jni_sig!((long, java.lang.String, int, int, int, int, int, int, int, int, int, int) -> ()),
                &[
                    jni::objects::JValue::Long(request_id as jni::sys::jlong),
                    jni::objects::JValue::Object(&j_title),
                    jni::objects::JValue::Int(s.year as jni::sys::jint),
                    i(s.month),
                    i(s.day),
                    i(s.hour),
                    i(s.minute),
                    jni::objects::JValue::Int(e.year as jni::sys::jint),
                    i(e.month),
                    i(e.day),
                    i(e.hour),
                    i(e.minute),
                ],
            )?;
            Ok(())
        });

    request_result.is_ok()
}

/// `MainActivity.nativeOnCalendarResult`（Java 側）から呼ばれる JNI エクスポート関数
/// （Issue #1226）。`code` の意味は `device/calendar.rs::outcome_from_android_code` の doc
/// （Java 側 `CALENDAR_RESULT_*` と対）。旧要求の遅延到着は `deliver` が id 不一致で捨てる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名
/// （`nativeOnNotificationResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnCalendarResult(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    request_id: jni::sys::jlong,
    code: jni::sys::jint,
) {
    let outcome = super::calendar::outcome_from_android_code(code);
    super::calendar::bridge::deliver(request_id as u64, outcome);
    crate::android::request_repaint();
}

// ドラッグ & ドロップ受け取り（Issue #1016）。Java 側の `dragListener`
// （`View.OnDragListener`、`MainActivity.java`）が `View.setOnDragListener` 経由で push
// する各イベントを、iOS（`ios_drag_drop.rs`）と同じ集約器 [`super::drop_session`] へ
// 橋渡しする。file-pick/notification と異なり `RequestSlot`/`bridge` は使わない
// （`drop_session.rs` モジュール doc「Android 経路」参照。集約器自体がセッション id で
// 世代を追う）。`nativeDragDropIsActive`（`crate::android` 側、feature 無条件で 1 つだけ
// 定義）が `false` を返す画面では、Java 側はここから先の 4 関数を一切呼ばない
// （実装計画 §3-2「スタブを置かない理由」）。

/// Java 側から届いた `code`/`name`/`fd` を [`DropItem`] へ変換する
/// （`nativeOnDragDropItem` 本体）。`code` の意味は [`super::drag_drop::item_from_fd`]
/// の doc（`MainActivity.nativeOnDragDropItem` 宣言コメントと対）を参照。
///
/// `fd` の所有権は `code` に関わらず常に先に取る（`fd >= 0` のときのみ）。Java 側が
/// 誤って `code != 0` で正の fd を渡した場合でも取りこぼして leak させないため
/// （`decode_file_pick_result` と同じ判断、codex P1 対応の踏襲）。所有権を取った
/// `File` は [`super::drag_drop::item_from_fd`] が読み取り成否に関わらず drop 時に
/// 必ず 1 回だけ close する。
///
/// 表示名は Java 側 DISPLAY_NAME（`MainActivity.resolveDisplayName`）を
/// [`sanitize_picked_name`] で検証する（パスの断片・NUL・空文字を拒否）。`name` が
/// null、または検証に失敗した場合は「（名前なし）」にする（`file_pick` と同じ
/// フォールバック）。
// 本体内の 1 箇所の `unsafe`（`OwnedFd::from_raw_fd`）は関数 doc の SAFETY 記述参照。
#[allow(unsafe_code)]
fn decode_drag_drop_item(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> DropItem {
    // SAFETY: Java 側 `dragListener` の読み取りスレッド（`MainActivity.java`
    // `deliverDropItem`）が `ParcelFileDescriptor.detachFd()` で所有権を手放した
    // 直後に渡す有効な fd（`fd >= 0` のときのみ）であり、detach 後は Java 側で
    // close しない契約（`decode_file_pick_result` と同一の根拠）。負の値（fd なし、
    // フォルダ/読めない経路）は所有権を取らない。
    let file = (fd >= 0).then(|| {
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        std::fs::File::from(owned)
    });

    let display_name = if name.is_null() {
        None
    } else {
        name.try_to_string(env).ok()
    }
    .and_then(|raw| sanitize_picked_name(&raw))
    .unwrap_or_else(|| "（名前なし）".to_string());

    super::drag_drop::item_from_fd(code, file, display_name)
}

/// `MainActivity.java` の `dragListener` が `ACTION_DRAG_ENTERED`/`ACTION_DRAG_EXITED`
/// から呼ぶ JNI エクスポート関数（Issue #1016）。`entered`（`true` = 領域内に入った）を
/// そのまま [`super::drop_session::hover`] へ委譲する。
///
/// View のジェスチャ処理は egui の描画ループとは独立して進むため、
/// [`crate::android::request_repaint`] で次フレームを明示的に起こす
/// （`nativeOnFilePickerResult` と同じ理由）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定
/// シンボル名であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない（`nativeOnFilePickerResult` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnDragDropHover(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    entered: jni::sys::jboolean,
) {
    super::drop_session::hover(entered);
    crate::android::request_repaint();
}

/// `MainActivity.java` の `dragListener` が `ACTION_DROP` の冒頭から呼ぶ JNI
/// エクスポート関数（Issue #1016）。[`super::drop_session::begin`] へ委譲し、払い出した
/// セッション id を返す。`total` が負（契約違反）なら 0 件として扱う。
///
/// iOS の `fandhe_rust_drag_drop_begin`（out パラメータで `to_load` も返す）と異なり、
/// `to_load` は返さない: Java 側の読み取りスレッドは `total` 件すべてについて
/// [`Java_com_fandhe_androidapp_MainActivity_nativeDragDropWants`] へ 1 件ずつ問い合わせ、
/// 上限超過分・旧世代はそこで `false` として打ち切られる（実装計画 §3-1 ステップ 5、
/// `drop_session::wants` doc 参照）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`nativeOnDragDropHover` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnDragDropBegin(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    total: jni::sys::jint,
) -> jni::sys::jlong {
    let total = usize::try_from(total).unwrap_or(0);
    let (session, _to_load) = super::drop_session::begin(total);
    crate::android::request_repaint();
    session as jni::sys::jlong
}

/// `MainActivity.java` の読み取りスレッドが、1 件読む **前** に呼ぶ JNI エクスポート
/// 関数（Issue #1016）。[`super::drop_session::wants`] へ委譲する。`false` が返れば
/// Java 側は読み取りを行わず break する（旧世代の打ち切り・上限超過分の打ち切りの
/// 両方をここで判定する、実装計画 §3-3）。`session`/`index` が負（契約違反）なら
/// 無条件で `false`。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`nativeOnDragDropHover` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeDragDropWants(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    session: jni::sys::jlong,
    index: jni::sys::jint,
) -> jni::sys::jboolean {
    if session < 0 {
        return false;
    }
    let Ok(index) = usize::try_from(index) else {
        return false;
    };
    super::drop_session::wants(session as u64, index)
}

/// `MainActivity.java` の読み取りスレッドが、1 件読み終える（またはフォルダ/読めない
/// と判定する）たびに呼ぶ JNI エクスポート関数（Issue #1016）。[`decode_drag_drop_item`]
/// で [`DropItem`] へ変換し、[`super::drop_session::item`] へ渡す。
///
/// 読み取り（[`decode_drag_drop_item`] 内の [`super::drag_drop::item_from_fd`]）は
/// ロックの外（本関数の中、`drop_session::item` 呼び出し前）で行う（`drop_session.rs`
/// モジュール doc「判定と更新は同じロックの内側」参照。ロック保持中に I/O を行わない）。
///
/// `index` が負（契約違反）の場合は `drop_session::item` を呼ばないが、`fd` の所有権は
/// [`decode_drag_drop_item`] の中で既に取っているため、変換結果の `DropItem` が drop
/// されるだけでリークしない。
///
/// 受け取った内容（ファイル名・中身）はログ出力・保存を一切行わない（他アプリの
/// 機密文書になり得る、security.md A09）。`deliver` に相当する経路の成否に関わらず
/// 無条件で [`crate::android::request_repaint`] を呼ぶ（`nativeOnFilePickerResult` と
/// 同じ理由。アイドル中でも次フレームで `drag_drop_effect` が
/// `drop_session::take_events` を観測できるようにする、契約表 4）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは
/// [`decode_drag_drop_item`] に閉じる）。`EnvUnowned::with_env` が JNI アタッチメントの
/// 安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。`#[no_mangle]` は
/// JNI 命名規約上の固定シンボル名であり、本クレートは cdylib として 1 つの Android
/// 実行単位にのみリンクされるため他シンボルと衝突しない（`nativeOnFilePickerResult` と
/// 同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnDragDropItem<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    session: jni::sys::jlong,
    index: jni::sys::jint,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        // fd の所有権は decode 側が code に関わらず先に取るため、index 不正で
        // `item` を呼ばない経路でも close されリークしない（関数 doc 参照）。
        let item = decode_drag_drop_item(env, code, &name, fd);
        if session >= 0 {
            if let Ok(index) = usize::try_from(index) {
                super::drop_session::item(session as u64, index, item);
            }
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    crate::android::request_repaint();
}
