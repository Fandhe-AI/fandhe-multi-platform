//! Android 向けドラッグ & ドロップ受け取りの JNI 受け口（Issue #1016、#1302 で機能 crate へ移動）。
//! Java 側 `DragDropHelper` が DecorView の `View.OnDragListener` で観測した 1 件ずつのイベントを、
//! iOS と同じ集約器 [`crate::drop_session`] へ橋渡しする。要求 1 件 → 応答 1 件の `RequestSlot` 型ではなく、
//! 集約器がセッション id で世代を追う（`drop_session` モジュール doc「Android 経路」参照）。
//!
//! JNI 窓口は `DragDropHelper` の static native 5 本で、署名は Java と本ファイルのエクスポート名の 2 か所でそろえる
//! （エクスポート名は `Java_com_fandhe_platform_DragDropHelper_*`）:
//!
//! - `boolean nativeDragDropIsActive()`: 表示中の画面が drag_drop か。false ならそのドラッグ全体を Java が無視し、
//!   以降の 4 本を一切呼ばない（他の画面宛てのドロップを受けないためのゲート、契約表 1）
//! - `void nativeOnDragDropHover(boolean)`: 領域内への出入り
//! - `long nativeOnDragDropBegin(int)`: ドロップ開始。払い出したセッション id を返す
//! - `boolean nativeDragDropWants(long, int)`: 1 件読む前の問い合わせ。false なら読まずに打ち切る
//! - `void nativeOnDragDropItem(long, int, int, String, int)`: 1 件ぶんの結果（code 0=ファイル / 1=フォルダ / 2=読めない）
//!
//! 画面の表示状態は `fandhe-gallery` の `android_main` が画面名を決めた時点で [`set_screen_active`] へ渡し、
//! Activity の再生成では [`reset_sessions`] が進行中のセッションと未適用イベントを捨てる
//! （どちらも最終成果物 `fandhe-gallery` が呼ぶ）。
//!
//! 受け取った内容（ファイル名・中身）はログ出力・保存しない（他アプリの機密文書になり得る、security.md A09）。

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::drop_session::DroppedItem;
use fandhe_device::file_picker::sanitize_picked_name;

/// 現在表示中の画面が `drag_drop` かどうか。`nativeDragDropIsActive` が読む。
static SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 単調時計の基準点（`Instant` は絶対値を持たないため）。
static START: OnceLock<Instant> = OnceLock::new();

/// [`mark_active`] の有効期間（ミリ秒）。iOS の `SCREEN_ACTIVE_WINDOW_MS` と揃える。
const ACTIVE_WINDOW_MS: u64 = 500;

/// 直近の [`mark_active`] 時刻（基準点からの経過ミリ秒 + 1。0 は未呼び出し）。
static LAST_ACTIVE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// 独立入口（`platform_drag_drop`）の毎フレーム呼び出しで有効期間を延ばす。呼び出しが止まれば
/// [`ACTIVE_WINDOW_MS`] 後に受け付けを止め、別画面でのドロップを積まない（契約表 1）。
pub(crate) fn mark_active() {
    LAST_ACTIVE_MS.store(now_ms() + 1, Ordering::Relaxed);
    fandhe_runtime::request_repaint_after(std::time::Duration::from_millis(ACTIVE_WINDOW_MS / 2));
}

fn is_active() -> bool {
    let last = LAST_ACTIVE_MS.load(Ordering::Relaxed);
    SCREEN_ACTIVE.load(Ordering::Relaxed)
        || (last != 0 && now_ms().saturating_sub(last - 1) < ACTIVE_WINDOW_MS)
}

/// 表示中の画面が `drag_drop` かどうかを差し替える。`fandhe-gallery` の `android_main` が起動画面を決定した直後に呼ぶ。
pub fn set_screen_active(active: bool) {
    SCREEN_ACTIVE.store(active, Ordering::Relaxed);
}

/// Activity 再生成のたびに、進行中のセッションと未適用イベントを捨てる。旧 Activity 由来の読み取りスレッドは
/// `nativeDragDropWants` で打ち切られる。`fandhe-gallery` の `android_main` が呼ぶ。
pub fn reset_sessions() {
    crate::drop_session::reset();
}

/// Java 側から届いた `code`/`name`/`fd` を [`DroppedItem`] へ変換する（`nativeOnDragDropItem` 本体）。
/// `code` の意味は [`crate::driver::item_from_fd`] の doc と `DragDropHelper.nativeOnDragDropItem` 宣言コメントと対。
///
/// `fd` の所有権は `code` に関わらず常に先に取る（`fd >= 0` のときのみ）。Java 側が誤って `code != 0` で
/// 正の fd を渡しても取りこぼして leak させないため。所有権を取った `File` は `item_from_fd` が読み取り成否に
/// 関わらず drop 時に 1 回だけ close する。
///
/// 表示名は Java 側の DISPLAY_NAME を `sanitize_picked_name`（ファイル選択と共有する検証。パスの断片・NUL・空文字を拒否）で
/// 検証し、null または検証に失敗した場合は「（名前なし）」にする。
#[allow(unsafe_code)]
fn decode_drag_drop_item(
    env: &jni::Env,
    code: jni::sys::jint,
    name: &jni::objects::JString,
    fd: jni::sys::jint,
) -> DroppedItem {
    // SAFETY: Java 側の読み取りスレッド（`DragDropHelper.deliverDropItem`）が `ParcelFileDescriptor.detachFd()` で
    // 所有権を手放した直後に渡す有効な fd（`fd >= 0` のときのみ）で、detach 後は Java 側で close しない契約。
    // 負の値（fd なし、フォルダ・読めない経路）は所有権を取らない。ここで 1 回だけ所有に取る。
    let file = (fd >= 0).then(|| {
        // SAFETY: 上記のとおり `fd >= 0` は所有権を手放された有効な fd で、ここで 1 回だけ所有に取る。
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

    crate::driver::item_from_fd(code, file, display_name)
}

/// `DragDropHelper.nativeDragDropIsActive`。`ACTION_DRAG_STARTED` のたびに呼ばれる同期 getter。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[unsafe(no_mangle)]` は JNI 命名規約上の固定シンボル名で、
/// 本 crate は 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DragDropHelper_nativeDragDropIsActive(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) -> u8 {
    u8::from(is_active())
}

/// `DragDropHelper.nativeOnDragDropHover`。`entered`（`true` = 領域内に入った）をそのまま集約器へ渡す。
/// View のジェスチャ処理は描画ループと独立して進むため、次フレームを明示的に起こす（契約表 4）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`nativeDragDropIsActive` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DragDropHelper_nativeOnDragDropHover(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    entered: jni::sys::jboolean,
) {
    crate::drop_session::hover(entered);
    fandhe_runtime::request_repaint();
}

/// `DragDropHelper.nativeOnDragDropBegin`。`ACTION_DROP` の冒頭から呼ばれ、払い出したセッション id を返す。
/// `total` が負（契約違反）なら 0 件として扱う。iOS と異なり `to_load` は返さず、Java 側が `total` 件すべてについて
/// `nativeDragDropWants` へ 1 件ずつ問い合わせ、上限超過分・旧世代はそこで打ち切られる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`nativeDragDropIsActive` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DragDropHelper_nativeOnDragDropBegin(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    total: jni::sys::jint,
) -> jni::sys::jlong {
    let total = usize::try_from(total).unwrap_or(0);
    let (session, _to_load) = crate::drop_session::begin(total);
    fandhe_runtime::request_repaint();
    session as jni::sys::jlong
}

/// `DragDropHelper.nativeDragDropWants`。読み取りスレッドが 1 件読む **前** に呼ぶ。false なら Java 側は読まずに
/// break する（旧世代の打ち切り・上限超過分の打ち切りの両方をここで判定する）。`session` / `index` が負（契約違反）なら false。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`nativeDragDropIsActive` と同じ根拠）。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DragDropHelper_nativeDragDropWants(
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
    crate::drop_session::wants(session as u64, index)
}

/// `DragDropHelper.nativeOnDragDropItem`。読み取りスレッドが 1 件読み終える（またはフォルダ・読めないと判定する）
/// たびに呼ぶ。[`decode_drag_drop_item`] で [`DroppedItem`] へ変換し、集約器へ渡す。読み取りはロックの外
/// （本関数の中、`drop_session::item` 呼び出し前）で行い、ロック保持中に I/O をしない。
///
/// `index` が負（契約違反）の場合は `item` を呼ばないが、`fd` の所有権は decode の中で既に取っているため、
/// 変換結果の `DroppedItem` が drop されるだけでリークしない。成否に関わらず次フレームを起こす。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（`fd` の `unsafe` な受け取りは [`decode_drag_drop_item`] に閉じる）。
/// `EnvUnowned::with_env` が JNI アタッチメントの安全な取り扱い（`catch_unwind` によるパニックの遮断込み）を担う。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "system" fn Java_com_fandhe_platform_DragDropHelper_nativeOnDragDropItem<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    session: jni::sys::jlong,
    index: jni::sys::jint,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        // fd の所有権は decode 側が code に関わらず先に取るため、index 不正で `item` を呼ばない経路でも close される。
        let item = decode_drag_drop_item(env, code, &name, fd);
        if session >= 0
            && let Ok(index) = usize::try_from(index)
        {
            crate::drop_session::item(session as u64, index, item);
        }
        Ok(())
    });
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let _ = outcome.into_outcome();
    fandhe_runtime::request_repaint();
}
