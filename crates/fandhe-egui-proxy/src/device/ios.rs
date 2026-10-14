//! iOS 向け `fandhe-device` 実装（Issue #751）。`device.rs` の `storage_note_state` と
//! `camera_capture.rs` の `camera_capture_effect` から呼ばれ、Documents 配下への保存
//! （[`documents_dir`]）と実カメラ撮影（[`IosCamera`]）を提供する。
//!
//! カメラ実装は `unsafe extern "C"` で `ios/fandhe_ios_camera.m`（`build.rs` が
//! iOS ターゲットのみコンパイルする ObjC、UIKit の `UIImagePickerController` +
//! AVFoundation の権限確認）を呼ぶ。`unsafe` はこの FFI 呼び出し 1 関数
//! （[`IosCamera::capture`]）に閉じ、`// SAFETY:` で契約を明記する
//! （`lib.rs` の iOS `deny(unsafe_code)` 緩和、coding-rust.md の FFI 例外規定準拠）。
//!
//! 傾きセンサ実装（[`IosMotion`]、Issue #832）も同型: `ios/fandhe_ios_motion.m`
//! （CoreMotion の `CMMotionManager`）を `unsafe extern "C"` 1 関数で呼び、
//! `device/tilt.rs` の effect ドライバから毎フレーム pull される。
//!
//! クリップボード実装（[`IosClipboard`]、Issue #859）も同型:
//! `ios/fandhe_ios_clipboard.m`（UIKit の `UIPasteboard`）を `unsafe extern "C"`
//! 3 関数で呼び、`device/clipboard.rs` の effect ドライバからボタン押下時のみ
//! 呼ばれる。
//!
//! 読み上げ実装（[`IosTts`]、Issue #866）も同型: `ios/fandhe_ios_tts.m`
//! （AVFoundation の `AVSpeechSynthesizer`）を `unsafe extern "C"` 2 関数で呼び、
//! `device/tts.rs` の TTS ワーカースレッド（`worker_loop`）から呼ばれる。
//!
//! ファイル選択実装（Issue #873、案 B）も `ios/fandhe_ios_file_picker.m`
//! （`UIDocumentPickerViewController`）を呼ぶ点は同型だが、他 4 実装と異なり
//! `fandhe_device::file_picker::FilePicker` トレイトを **通らない**
//! （`device/file_pick.rs` モジュール doc「案 B」参照）。提示関数
//! [`present_file_picker`] は即時 return し、結果は非同期に
//! [`fandhe_rust_file_picker_outcome`] 経由で `device/file_pick.rs::bridge` へ
//! 直接届く。ブロッキング待ちが無いため、`camera`/`tts` のようなワーカースレッド・
//! mailbox は持たない。
//!
//! 文字サイズ追従実装（[`IosTextScale`]、Issue #914）は [`IosMotion`] と同型:
//! `apple/fandhe_apple_text_scale.m`（`UIFont.preferredFontForTextStyle:`）を
//! `unsafe extern "C"` 1 関数（out 引数 1 個の同期呼び出し）で呼び、
//! `device/text_scale.rs` の effect ドライバから毎フレーム pull される。main
//! スレッド専用の呼び出し契約も [`IosMotion`] と同じく [`is_main_thread`] で検査する。
//!
//! マイク録音（Issue #954 macOS + #956 iOS）は本モジュールを**通らない**:
//! `apple/fandhe_apple_mic_record.m`（iOS/macOS 共用）を `device/apple_mic_record.rs`
//! が呼び、`device/mic_record.rs` の iOS/macOS 経路が直接使う（ファイル選択と同じ
//! pull 型の橋渡し、`device/mic_record.rs` モジュール doc 参照）。
//!
//! 音声認識（Issue #1024 macOS + #1025 iOS）も同型で本モジュールを**通らない**:
//! `apple/fandhe_apple_speech_recognition.m`（iOS/macOS 共用）を `device/
//! apple_speech_recognition.rs` が呼び、`device/speech_recognition.rs` の
//! iOS/macOS 経路が直接使う（`device/speech_recognition.rs` モジュール doc 参照）。

use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::{Condvar, Mutex};

use fandhe_device::camera::Camera;
use fandhe_device::clipboard::Clipboard;
use fandhe_device::file_picker::{read_picked, MAX_FILE_BYTES};
use fandhe_device::motion::Motion;
use fandhe_device::text_scale::TextScale;
use fandhe_device::tts::Tts;
use fandhe_device::{DataDir, DeviceError, Image};

/// iOS アプリコンテナ内 `Documents/` を `DataDir` として返す（`storage::FsStorage::new`
/// へ渡す契約、`device.rs::storage_note_state` から呼ばれる）。
///
/// iOS では `$HOME` がアプリごとにサンドボックス化されたコンテナのルートを指すため、
/// `std::env::home_dir()`（deprecated だが単純な環境変数参照で unsafe を伴わない、
/// `fandhe-device` の `FsStorage::desktop_default` と同じ判断）に `Documents` を
/// 連結するだけで App Store の審査要件（ユーザーデータは Documents/Library 配下）に
/// 沿ったパスが得られる。
pub fn documents_dir() -> Result<DataDir, DeviceError> {
    #[allow(deprecated)]
    std::env::home_dir()
        .map(|home| DataDir(home.join("Documents")))
        .ok_or(DeviceError::Unsupported("home directory not found"))
}

/// 縮小後の画像の長辺上限（ピクセル）。`fandhe_ios_camera.m` 側の
/// `FandheEncodeImageRGBA` へそのまま渡す。上限を固定することで最大確保量を
/// 512*512*4 = 1 MiB 程度に抑える（security.md A05、メモリ枯渇対策）。
const MAX_CAPTURE_EDGE: u32 = 512;

// SAFETY: `ios/fandhe_ios_camera.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// みコンパイル・リンクする）。契約は下記 `capture` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_camera_capture(
        max_edge: u32,
        out_rgba: *mut *mut u8,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> i32;
    fn fandhe_ios_camera_free(ptr: *mut u8);
}

/// `fandhe_ios_camera_capture` が確保したバッファを解放する（`NULL` 許容）。
/// 契約違反（戻り値と out 引数の不整合）を検出したエラー経路からまとめて呼ぶための
/// 小関数（`unsafe` を 1 箇所に集約し、`if`/`match` の枝ごとに SAFETY コメントの
/// 直前行が途切れる問題を避ける、`unsafe-audit.sh` の探索仕様に合わせた形）。
#[allow(unsafe_code)]
fn free_captured_buffer(ptr: *mut u8) {
    // SAFETY: `fandhe_ios_camera_free` は NULL でも安全（`.m` 側は `free(NULL)` に
    // 委譲するのみ）。非 NULL の場合は `fandhe_ios_camera_capture` が返した
    // malloc 領域を指す前提で、呼び出し元（`capture`）はこの呼び出し後に
    // `ptr` を再利用しない契約にする。
    unsafe { fandhe_ios_camera_free(ptr) };
}

/// iOS 実カメラ実装。`fandhe_ios_camera.m` の `UIImagePickerController` を
/// ワーカースレッド（`camera_capture.rs::spawn_capture` が呼ぶ）から同期的に起動する
/// （ObjC 側が `dispatch_async(main)` + セマフォで橋渡しする、`.m` ファイルのコメント参照）。
pub struct IosCamera;

impl Camera for IosCamera {
    #[allow(unsafe_code)]
    fn capture(&self) -> Result<Image, DeviceError> {
        let mut out_rgba: *mut u8 = std::ptr::null_mut();
        let mut out_w: u32 = 0;
        let mut out_h: u32 = 0;

        // SAFETY: `fandhe_ios_camera_capture` の契約（`.m` ファイル冒頭コメント）:
        // 戻り値 0 のときのみ `out_rgba` が `out_w * out_h * 4` バイトの有効な
        // malloc 領域を指し、所有権は呼び出し側（本関数）に移る。0 以外では
        // 3 個の out ポインタはいずれも書き換えられない（このスコープ内で
        // 初期値のまま扱う）。3 個の out 引数は上記でスタック上に確保済みの
        // 有効な `u32`/`*mut u8` へのポインタであり、`fandhe_ios_camera` 以外の
        // スレッドが同時に書き換えることはない（呼び出しはこの関数呼び出し中のみ）。
        let code = unsafe {
            fandhe_ios_camera_capture(MAX_CAPTURE_EDGE, &mut out_rgba, &mut out_w, &mut out_h)
        };

        match code {
            0 => {}
            1 => return Err(DeviceError::Unsupported("camera")),
            2 => return Err(DeviceError::PermissionDenied("camera")),
            3 => return Err(DeviceError::Cancelled("camera")),
            // 4 = 内部エラー（presenter 不在・エンコード失敗・二重呼び出し等、
            // `fandhe_ios_camera.m` 冒頭コメントの契約）。機種としてカメラが
            // 使えないわけではないため `Unsupported` にせず区別する
            // （PR #791 レビュー指摘: Internal errors reported as unsupported）。
            _ => return Err(DeviceError::Internal("camera bridge")),
        }

        if out_w == 0 || out_h == 0 || out_rgba.is_null() {
            // 契約違反（0 を返したのに寸法/ポインタが不正）を検出した経路。
            // `out_rgba` が確保済みなら解放してからエラーへ落とす
            // （`fandhe_ios_camera_free` は NULL を渡しても安全、`.m` 側は
            // `free(NULL)` に委譲するのみ。SAFETY コメントは下記関数側に集約する）。
            free_captured_buffer(out_rgba);
            return Err(DeviceError::InvalidImageSize(out_w, out_h));
        }

        let byte_len = match (out_w as usize)
            .checked_mul(out_h as usize)
            .and_then(|px| px.checked_mul(4))
        {
            Some(len) => len,
            None => {
                // 契約違反（`out_w`/`out_h` の積が u32 に収まらない）を検出した経路。
                free_captured_buffer(out_rgba);
                return Err(DeviceError::InvalidImageSize(out_w, out_h));
            }
        };

        // SAFETY: 戻り値 0 の契約により `out_rgba` は `byte_len` バイトの有効な
        // 読み取り可能領域を指す（`byte_len` は `out_w * out_h * 4` と一致するよう
        // 上で計算済み）。`to_vec` でコピーした後、直後に [`free_captured_buffer`]
        // で ObjC 側の所有権を解放する（このコピー後は `out_rgba` を再利用しない）。
        let rgba = unsafe { std::slice::from_raw_parts(out_rgba, byte_len) }.to_vec();
        free_captured_buffer(out_rgba);

        Ok(Image {
            width: out_w,
            height: out_h,
            rgba,
        })
    }
}

// SAFETY: `ios/fandhe_ios_motion.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// み・feature `device` 有効時のみコンパイル・リンクする）。契約は下記 `tilt`
// 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_motion_read(out_x: *mut f32, out_y: *mut f32) -> i32;
}

// SAFETY: `pthread_main_np` は libSystem（Darwin の全バイナリに常時リンク済み）が
// 提供する引数無しの単純な問い合わせ関数で、呼び出しスレッドを変更しない
// （呼び出し元スレッドが main スレッドなら非 0 を返す）。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn pthread_main_np() -> i32;
}

/// 呼び出しスレッドが main スレッドかどうかを返す。
///
/// `fandhe_ios_motion_read` は `UIApplication.connectedScenes` を読む
/// （`fandhe_ios_motion.m` 冒頭コメント）ため main スレッド専用の呼び出し契約を
/// 持つが、[`IosMotion`] は `Motion: Send + Sync` の公開型で誰でも構築でき、
/// ワーカースレッドから呼ばれる誤用をコンパイラは検出できない。FFI 呼び出し前に
/// スレッドを検査し、main スレッド以外では [`DeviceError::Internal`] を返して
/// unsafe な前提を呼び出し側へ漏らさない（coding-rust.md の FFI 例外規定、
/// codex-review 指摘 P1、PR #844）。
#[allow(unsafe_code)]
fn is_main_thread() -> bool {
    // SAFETY: 上記 `pthread_main_np` 宣言のコメント参照。副作用も前提条件も無く
    // 常に安全に呼び出せる。
    unsafe { pthread_main_np() != 0 }
}

/// iOS 実センサ実装（`CMMotionManager` の gravity、Issue #832）。
/// `device/tilt.rs` の effect ドライバから毎フレーム pull される
/// （push ではなく pull にする設計判断はモジュール doc・`tilt.rs` 冒頭コメント参照）。
pub struct IosMotion;

impl Motion for IosMotion {
    #[allow(unsafe_code)]
    fn tilt(&self) -> Result<(f32, f32), DeviceError> {
        // `fandhe_ios_motion_read` は main スレッド専用（`is_main_thread` の
        // ドキュメンテーションコメント参照）。ワーカースレッドから呼ばれた場合は
        // FFI へ進まずエラーを返す。
        if !is_main_thread() {
            return Err(DeviceError::Internal("motion: called off main thread"));
        }

        let mut out_x: f32 = 0.0;
        let mut out_y: f32 = 0.0;

        // SAFETY: `fandhe_ios_motion_read` の契約（`.m` ファイル冒頭コメント）:
        // out 引数はこの関数呼び出し中のみ有効なスタック上の `f32` への
        // ポインタで、所有権移転・malloc は無い（camera 経路と違い解放処理が
        // 不要）。戻り値が 0 以外のとき out 引数は書き換えられない契約のため、
        // 呼び出し直前の初期値 `(0.0, 0.0)` のまま扱う。
        let code = unsafe { fandhe_ios_motion_read(&mut out_x, &mut out_y) };

        match code {
            0 => {}
            // 1 = 端末が device motion 非対応（シミュレータ等）。0 以外の未知の
            // コードも安全側で同様に扱う（契約違反を検出した経路）。
            _ => return Err(DeviceError::Unsupported("motion")),
        }

        // `fandhe_ios_motion_read` は現在の UI 向き（`UIInterfaceOrientation`）基準
        // の画面右・画面上方向へ既に回転済みの値を返す（`fandhe_ios_motion.m` の
        // `FandheRotateGravityToInterfaceOrientation` 参照、Bugbot 指摘 Medium 是正:
        // portrait 前提の Y 反転のみだと landscape / iPad 回転で玉が誤った軸へ
        // 転がっていた）。ここではその画面基準の y（上方向が正）を
        // `fandhe_core::screens::tilt_ball` の座標系（x: 右方向、y: 下方向が正）へ
        // 合わせるため反転するだけでよい（画面上向き正 → 反転で下向き正、向きが
        // 変わっても常に「画面の上」基準なのでこの反転だけで全 UI 向きに対応できる）。
        // NaN → 0.0 置換 + -1.0..=1.0 クランプは `DummyMotion::set` 側でも行う
        // 二重防御（security.md A08）だが、FFI 境界を越えた値をこの関数内でも
        // 正規化しておく。
        Ok((clamp_finite(out_x), clamp_finite(-out_y)))
    }
}

/// NaN を `0.0` へ置換してから `-1.0..=1.0` へ丸める（`fandhe_device::motion` の
/// `clamp_finite` と同じ規則、FFI 境界を越えた値をここでも正規化する）。
fn clamp_finite(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

// SAFETY: `ios/fandhe_ios_clipboard.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// み・feature `device` 有効時のみコンパイル・リンクする）。契約は下記 `IosClipboard` の
// 各メソッドのコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_clipboard_read(out_text: *mut *mut core::ffi::c_char, out_len: *mut usize)
        -> i32;
    fn fandhe_ios_clipboard_free(ptr: *mut core::ffi::c_char);
    fn fandhe_ios_clipboard_write(text: *const core::ffi::c_char) -> i32;
}

/// `fandhe_ios_clipboard_read` が確保したバッファを解放する（`NULL` 許容）。
/// `unsafe` を 1 箇所へ集約するための小関数（`free_captured_buffer` と同じ形）。
#[allow(unsafe_code)]
fn free_clipboard_buffer(ptr: *mut core::ffi::c_char) {
    // SAFETY: `fandhe_ios_clipboard_free` は NULL でも安全（`.m` 側は `free(NULL)` に
    // 委譲するのみ）。非 NULL の場合は `fandhe_ios_clipboard_read` が返した malloc
    // 領域を指す前提で、呼び出し元はこの呼び出し後に `ptr` を再利用しない契約にする。
    unsafe { fandhe_ios_clipboard_free(ptr) };
}

/// iOS 実クリップボード実装（`UIPasteboard`、Issue #859）。
/// `device/clipboard.rs` の effect ドライバから「コピー」/「貼り付け」ボタン押下時に
/// 呼ばれる（毎フレームではない）。
///
/// `UIPasteboard` は UIKit の main スレッド前提 API のため、`IosMotion` と同じく
/// FFI 呼び出し前に [`is_main_thread`] で検査し、main スレッド以外では
/// [`DeviceError::Internal`] を返して unsafe な前提を呼び出し側へ漏らさない。
pub struct IosClipboard;

impl Clipboard for IosClipboard {
    #[allow(unsafe_code)]
    fn get_text(&self) -> Result<Option<String>, DeviceError> {
        if !is_main_thread() {
            return Err(DeviceError::Internal("clipboard: called off main thread"));
        }

        let mut out_text: *mut core::ffi::c_char = std::ptr::null_mut();
        let mut out_len: usize = 0;

        // SAFETY: `fandhe_ios_clipboard_read` の契約（`.m` ファイル冒頭コメント）:
        // 戻り値 0 のときのみ `out_text` が `out_len` バイトの UTF-8 バイト列を指す
        // malloc 領域へ書き換えられ（内容に NUL を含み得るが末尾にも NUL が置かれる）、
        // 所有権は呼び出し側（本関数）へ移る。0 以外では両 out 引数とも書き換えられない
        // （このスコープ内で初期値のまま扱う）。引数は上記でスタック上に確保済みの
        // 有効なポインタで、呼び出し中に他スレッドが書き換えることはない。
        let code = unsafe { fandhe_ios_clipboard_read(&mut out_text, &mut out_len) };

        match code {
            0 => {}
            // 1 = テキスト不在（空・許可拒否。`.m` の契約）。異常ではないため `None`。
            1 => return Ok(None),
            // 2 = 内部エラー（UTF-8 変換・malloc 失敗）。未知のコードも安全側で同様に扱う。
            _ => return Err(DeviceError::Internal("clipboard read")),
        }

        if out_text.is_null() {
            // 契約違反（0 を返したのにポインタが NULL）を検出した経路。
            return Err(DeviceError::Internal("clipboard read"));
        }

        // SAFETY: 戻り値 0 の契約により `out_text` は `out_len` バイトの有効な読み取り
        // 可能領域を指す。`from_raw_parts` はコピーを取らないため、`to_vec` で所有
        // バイト列へ写した後に [`free_clipboard_buffer`] で ObjC 側の所有権を解放する
        // （以降 `out_text` は再利用しない）。`strlen` ではなく `out_len` を使うため、
        // 埋め込み NUL があっても途中で切り詰めない（codex-review 指摘 P1、PR #860）。
        let bytes = unsafe { std::slice::from_raw_parts(out_text.cast::<u8>(), out_len) }.to_vec();
        free_clipboard_buffer(out_text);

        // 内部 NUL を含む文字列は受け付けない。書き込み側が `CString::new` で同じ理由
        // （C 文字列にできない）から弾いており、読み書きで判定を揃える
        // （codex-review 指摘 P1、PR #860。黙って切り詰めるより明示的に失敗させる）。
        if bytes.contains(&0) {
            return Err(DeviceError::Internal("clipboard read: embedded nul"));
        }

        match String::from_utf8(bytes) {
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            // `.m` 側が UTF-8 を保証する契約だが、破損していた場合は内容を
            // 推測せずエラーにする（security.md A03: 検証できない入力を通さない）。
            Err(_) => Err(DeviceError::Internal("clipboard read")),
        }
    }

    #[allow(unsafe_code)]
    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        if !is_main_thread() {
            return Err(DeviceError::Internal("clipboard: called off main thread"));
        }

        // 内部 NUL を含む文字列は C 文字列へできない（`fandhe-core` 側の入力欄からは
        // 到達しないが、契約として弾く）。
        let Ok(c_text) = std::ffi::CString::new(text) else {
            return Err(DeviceError::Internal("clipboard write"));
        };

        // SAFETY: `fandhe_ios_clipboard_write` の契約（`.m` ファイル冒頭コメント）:
        // 引数は NUL 終端の UTF-8 文字列へのポインタで、ObjC 側は内部でコピーを
        // 取るため本関数の戻り後に `c_text` を解放してよい（所有権は移らない）。
        // `c_text` はこの呼び出しの間スコープ内で生存する。
        let code = unsafe { fandhe_ios_clipboard_write(c_text.as_ptr()) };

        match code {
            0 => Ok(()),
            _ => Err(DeviceError::Internal("clipboard write")),
        }
    }
}

// SAFETY: `ios/fandhe_ios_tts.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// み・feature `device` 有効時のみコンパイル・リンクする）。契約は下記 `IosTts` の
// 各メソッドのコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_tts_speak(text: *const core::ffi::c_char, request_generation: u64) -> i32;
    fn fandhe_ios_tts_stop() -> i32;
}

/// [`fandhe_ios_tts_speak`] が返す世代 id ごとの「真の最終結果が届いたか」を
/// 仲介する（Android `device/android.rs` の `Mailbox` と同じ動機だが、
/// `AVSpeechSynthesizerDelegate` は成功と中断（`Ok(())` に写像、`IosTts` doc 参照）
/// しか通知しないため、運ぶ値は無く「到着したか」だけを追跡すれば足りる）。
struct TtsOutcome {
    state: Mutex<TtsOutcomeState>,
    condvar: Condvar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TtsOutcomeState {
    /// 追跡していない（未登録・既に消費済み）。
    Idle,
    /// `generation` の結果待ち。
    Pending(u64),
    /// `generation` の結果が到着済み（未消費）。
    Ready(u64),
}

impl TtsOutcome {
    const fn new() -> Self {
        Self {
            state: Mutex::new(TtsOutcomeState::Idle),
            condvar: Condvar::new(),
        }
    }

    /// `generation` の結果待ちを無条件に登録する（Android `Mailbox::begin_at` と
    /// 同じく、単一飛行の保証は呼び出し元（`IosTts::speak` は TTS ワーカースレッド
    /// 1 本に直列化済み）に委ねる）。登録済みの旧世代の待機があれば `notify_all` で
    /// 即座に起こし、「別世代に置き換わったら即座に `Ok(())`」契約
    /// （`Tts::wait_outcome` doc 参照）を満たす。
    fn begin(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = TtsOutcomeState::Pending(generation);
        self.condvar.notify_all();
    }

    /// [`begin`](Self::begin) で登録した世代の結果を待たず終わらせる
    /// （`fandhe_ios_tts_speak` 自体が同期的に失敗し、コールバックが二度と来ない
    /// と分かった場合に使う。Android `Mailbox::cancel` と同じ動機）。
    fn cancel(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == TtsOutcomeState::Pending(generation) {
            *state = TtsOutcomeState::Idle;
        }
    }

    /// `.m` 側の delegate コールバック（[`fandhe_rust_tts_outcome`]）から呼ぶ。
    /// 現在 `Pending` な世代と一致する場合のみ `Ready` へ進める。不一致（既に
    /// 別世代へ置き換わった・タイムアウトで畳まれた等）は古いコールバックとして
    /// 無視する。
    fn resolve(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state == TtsOutcomeState::Pending(generation) {
            *state = TtsOutcomeState::Ready(generation);
            self.condvar.notify_all();
        }
    }

    /// `generation` の結果が届くまでブロッキングで待つ。追跡していない
    /// （未登録・空発話で一度も [`begin`](Self::begin) されていない・既に別世代へ
    /// 置き換わった）場合は古い待機として即座に `Ok(())` を返す
    /// （`Tts::wait_outcome` の既定実装と同じ契約）。
    ///
    /// **時間による打ち切りは行わない**（codex-review P1・Bugbot Medium 是正、
    /// PR #869 3 巡目: 発話には長さの上限が無く、`CommandTts`（macOS）の
    /// `wait_outcome` も完了待ちに時間上限を持たない。固定時間で打ち切ると、
    /// 上限を超えて続く正常な発話が「まだ鳴っているのに失敗表示」になる）。
    /// 終了する条件は次のいずれかのみ: (1) 対象世代の結果が届く（[`resolve`]、
    /// 発話完了 or `stop`/後続 speak によるキャンセル）、(2) 対象世代が別の発話へ
    /// 置き換わる（[`begin`]、後続 speak による世代置換）。
    ///
    /// ponytail: `AVSpeechSynthesizerDelegate` のコールバック自体が欠落する
    /// ケース（オーディオセッションの強制割り込み等、理論上あり得るが未確認）への
    /// 安全網は持たない。delegate が来なくても、以後に `speak`/`stop` が
    /// 1 回でも発行されれば世代置換（(2)）で復帰する。実機で恒久的な滞留の報告が
    /// 出たら見直す。
    fn wait(&self, generation: u64) -> Result<(), DeviceError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match *state {
                TtsOutcomeState::Ready(ready) if ready == generation => {
                    *state = TtsOutcomeState::Idle;
                    return Ok(());
                }
                TtsOutcomeState::Pending(pending) if pending == generation => {
                    // まだ自分の世代を追跡中。結果到着まで待つ（時間では諦めない）。
                }
                _ => return Ok(()),
            }
            state = self.condvar.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

static TTS_OUTCOME: TtsOutcome = TtsOutcome::new();

/// `ios/fandhe_ios_tts.m` の `AVSpeechSynthesizerDelegate` から呼ばれる
/// `extern "C"` エクスポート関数（Issue #866 是正、PR #869）。
/// `AVSpeechSynthesizerDelegate` の `didFinishSpeechUtterance:`（`cancelled == 0`）/
/// `didCancelSpeechUtterance:`（`cancelled != 0`）の双方から呼ばれ、いずれも
/// [`TTS_OUTCOME`] を `Ok(())` として解決する（`AVSpeechSynthesizerDelegate` は
/// 発話失敗を通知する API を持たないため、`IosTts::speak` が返した世代の
/// 非同期失敗は存在しない前提。`.m` ファイル冒頭コメント参照）。
///
/// 呼び出しスレッドは AVFoundation の delegate 実行スレッド（メインスレッド想定だが
/// Apple のドキュメントに保証の明記が無い）。[`TtsOutcome`] は `Mutex`/`Condvar` で
/// 保護されているため、どのスレッドから呼ばれても安全（`unsafe` 操作を含まない。
/// `generation`/`cancelled` は素の整数値でポインタを経由しない）。
///
/// `#[allow(unsafe_code)]` は `#[no_mangle]`（シンボル名の固定）が `lib.rs` の
/// iOS `deny(unsafe_code)` 対象になるための宣言上の要請であり、関数本体は
/// `unsafe` 操作を一切含まない（Android 側 `device/android.rs` の JNI エクスポート
/// 関数と同じ扱い、coding-rust.md の FFI 例外規定）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_tts_outcome(generation: u64, _cancelled: i32) {
    TTS_OUTCOME.resolve(generation);
}

/// iOS 実読み上げ実装（`AVSpeechSynthesizer`、Issue #866）。
/// `device/tts.rs` の TTS ワーカースレッド（`worker_loop`、proxy の FIFO キューを
/// 処理する唯一のスレッド）から呼ばれる。Rust → `.m` の呼び出し（`speak`/`stop`）は
/// この 1 本のスレッドに直列化される契約のため、本実装自体は追加の排他制御を
/// 持たない。ただし `.m` 側は `isSpeaking` 時にメインキューへ積む遅延発話ブロック
/// （`fandhe_ios_tts_speak` 参照）が別スレッド（メインキュー）で走るため、ワーカー
/// スレッドとの間で `g_tts_lock` による排他制御を持つ（`.m` ファイルの
/// `g_tts_lock` 宣言部コメント参照）。
///
/// `IosClipboard`/`IosMotion` と違い [`is_main_thread`] 検査を付けない:
/// `UIPasteboard`/`CMMotionManager` と異なり `AVSpeechSynthesizer` は main
/// スレッド専用の契約を Apple のドキュメントに持たない。加えて、`stop_shared_tts()`
/// はアプリ終了時に main スレッドから TTS ワーカーへ要求を送りその完了を待つため
/// （`device/tts.rs::stop_shared_tts` 参照）、ここで `dispatch_sync(main, ...)` の
/// ような main スレッドへの同期委譲を挟むと、main が完了を待つ一方でワーカーの
/// 処理が main の空き待ちになる相互待ち（デッドロック）が起こり得る
/// （#866 実装計画 §3-2）。
///
/// 世代 id の採番元は `IosTts` インスタンス毎ではなく static
/// [`NEXT_TTS_GENERATION`] にする（codex-review 指摘、PR #869 5 巡目是正）:
/// 結果の保持先 [`TTS_OUTCOME`] はプロセス全体で 1 個の static のため、採番だけ
/// インスタンス毎にすると、複数の `IosTts` インスタンス（`Tts` は公開 API で
/// 誰でも構築できる）が同じ id を独立に払い出しうる。その結果、無関係な
/// 2 インスタンス A・B が同じ id で `speak`/`wait_outcome` を呼ぶと、B の待機が
/// A の結果を誤って回収してしまう（「他の世代の結果を絶対に回収しない」契約違反）。
#[derive(Default)]
pub struct IosTts;

/// [`IosTts::speak`] が払い出す世代 id の採番元（`IosTts` doc 参照）。
static NEXT_TTS_GENERATION: AtomicU64 = AtomicU64::new(0);

impl Tts for IosTts {
    #[allow(unsafe_code)]
    fn speak(&self, text: &str) -> Result<u64, DeviceError> {
        // 内部 NUL を含むテキストは C 文字列にできない（`IosClipboard::set_text` と
        // 同じ判定。`CommandTts::speak` の契約と対称に揃える）。
        if text.contains('\0') {
            return Err(DeviceError::InvalidInput("tts text"));
        }

        // 世代 id は `Tts::speak` の契約により単調増加で払い出す。`.m` 側へそのまま
        // 渡し、`AVSpeechUtterance` に紐付けた上で delegate コールバック
        // （[`fandhe_rust_tts_outcome`]）から [`TTS_OUTCOME`] を解決する識別子として
        // 使う（codex-review 指摘 PR #869 是正、`TtsOutcome` doc 参照）。
        let generation = NEXT_TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // 空文字列・空白のみは発話するものが無いため OS を呼ばずに戻る（`Tts::speak` の契約）。
        // `TTS_OUTCOME` には一切登録しないため、`wait_outcome(generation)` は
        // 「追跡していない ID」として即座に `Ok(())` を返す。
        if text.trim().is_empty() {
            return Ok(generation);
        }

        let Ok(c_text) = std::ffi::CString::new(text) else {
            // 上の NUL 検査で弾いているため到達しないはずだが、契約違反を検出した
            // 経路として安全側で扱う。
            return Err(DeviceError::InvalidInput("tts text"));
        };

        // コールバックが呼ばれる前に登録する（JNI 版 `AndroidTts::speak` と同じ理由:
        // `fandhe_ios_tts_speak` が返った直後に delegate コールバックが届いても
        // 取りこぼさない）。
        TTS_OUTCOME.begin(generation);

        // SAFETY: `fandhe_ios_tts_speak` の契約（`.m` ファイル冒頭コメント）:
        // 引数は NUL 終端の UTF-8 文字列へのポインタで、ObjC 側は内部でコピーを
        // 取るため本関数の戻り後に `c_text` を解放してよい（所有権は移らない）。
        // `c_text` はこの呼び出しの間スコープ内で生存する。
        let code = unsafe { fandhe_ios_tts_speak(c_text.as_ptr(), generation) };

        match code {
            // 0 はキュー投入成功に過ぎない。真の完了・中断は
            // `fandhe_rust_tts_outcome` が非同期に届けるため、ここでは確定させない
            // （codex-review 指摘 PR #869: 旧実装はここで即成功にしていた）。
            0 => Ok(generation),
            1 => {
                // コールバックは二度と来ないため、登録した待ちを畳んでおく
                // （Android `Mailbox::cancel` と同じ理由）。
                TTS_OUTCOME.cancel(generation);
                Err(DeviceError::Unsupported("tts"))
            }
            // 2 やその他未知のコードも安全側で内部エラーに畳む（`.m` の契約）。
            _ => {
                TTS_OUTCOME.cancel(generation);
                Err(DeviceError::Internal("tts speak"))
            }
        }
    }

    #[allow(unsafe_code)]
    fn stop(&self) -> Result<(), DeviceError> {
        // SAFETY: `fandhe_ios_tts_stop` は引数を取らず、副作用は発話停止のみ
        // （`.m` ファイル冒頭コメント）。停止によって中断された発話があれば
        // `AVSpeechSynthesizerDelegate` の `didCancelSpeechUtterance:` が自動的に
        // 発火し、その世代の `TTS_OUTCOME` を解決する（`.m` 側 doc 参照）。
        match unsafe { fandhe_ios_tts_stop() } {
            0 => Ok(()),
            _ => Err(DeviceError::Internal("tts stop")),
        }
    }

    /// `speak` が返した世代の真の最終結果（発話完了 or 中断）を待つ
    /// （`Tts::wait_outcome` の契約）。追跡していない世代（未登録・空発話・既に
    /// 別世代へ置き換わり済み）は [`TtsOutcome::wait`] が即座に `Ok(())` を返す。
    /// 時間による打ち切りは行わない（`TtsOutcome::wait` doc 参照、codex-review
    /// P1・Bugbot Medium 是正）。
    fn wait_outcome(&self, generation: u64) -> Result<(), DeviceError> {
        TTS_OUTCOME.wait(generation)
    }
}

// SAFETY: `ios/fandhe_ios_file_picker.m` が定義する C ABI（`build.rs` が iOS
// ターゲットのみ・feature `device` 有効時のみコンパイル・リンクする）。契約は
// 下記 [`present_file_picker`]・[`fandhe_rust_file_picker_outcome`] のコメントに
// 集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_file_picker_pick(request_id: u64) -> i32;
}

/// `ios/fandhe_ios_file_picker.m` の delegate から呼ばれる `extern "C"` エクスポート
/// 関数（Issue #873、案 B）。契約（`.m` ファイル冒頭コメント）:
///   `code`: 0 = 選択（`path_ptr`/`path_len` が tmp コピーの絶対パスを指す UTF-8
///   バイト列）・1 = キャンセル・それ以外 = 内部エラー。0 以外では `path_ptr` を
///   参照しない。
///
/// 選択時は本関数内で [`read_picked`] を同期に呼び切ってから戻る契約
/// （`.m` 側はこの呼び出しが戻ってから tmp コピーを削除する。読み取り・8 MiB 判定・
/// EOF の扱いは `read_picked` に一本化し、`.m` 側で読み取りループを持たない、
/// `device/file_pick.rs` モジュール doc 参照）。iOS の `asCopy:YES` コピーは
/// 元のファイル名を保つため、`read_picked` が `path` から導く名前をそのまま使う
/// （Android のように表示名を別途上書きする必要が無い）。
///
/// 呼び出しスレッドは `.m` 側の global queue（main ではない、選択時の同期読み取りで
/// main を塞がないため）。[`crate::device::file_pick::bridge::deliver`] は
/// スレッド安全なため、どのスレッドから呼ばれても安全（`unsafe` はポインタを
/// 読む 1 箇所のみに閉じる）。
///
/// `#[allow(unsafe_code)]` は `#[no_mangle]` が `lib.rs` の iOS `deny(unsafe_code)`
/// 対象になるための宣言上の要請（[`fandhe_rust_tts_outcome`] と同じ扱い）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_file_picker_outcome(
    request_id: u64,
    code: i32,
    path_ptr: *const u8,
    path_len: usize,
) {
    let result = picked_from_path(code, path_ptr, path_len, "file_picker");
    crate::device::file_pick::bridge::deliver(request_id, result);
    // 改訂 4: pull 型 bridge は sender も画面の `generation` も持たないため、
    // 結果を置いただけでは次の描画が起きる保証が無い（`file_pick.rs::drive_mobile`
    // は毎フレームの effect 呼び出しでしか拾えない）。無条件に呼んで確実に起こす
    // （`crate::ios::EGUI_CTX` doc 参照）。
    crate::ios::request_repaint();
}

/// ファイル選択・写真選択の `.m` が共有する「code + 一時ファイルのパス → `PickedFile`」変換
/// （Issue #1104 で `fandhe_rust_file_picker_outcome` から切り出し。`unsafe` はここ 1 箇所）。
/// `code`: 0 = 選択（`path_ptr`/`path_len` が UTF-8 の絶対パス）・1 = キャンセル・それ以外 =
/// 内部エラー。`what` はエラー識別子（利用者向け文言には出ない）。
/// 読み取り・上限判定は同期に終える（呼び出し元の `.m` は戻ってから一時ファイルを消す）。
#[allow(unsafe_code)]
fn picked_from_path(
    code: i32,
    path_ptr: *const u8,
    path_len: usize,
    what: &'static str,
) -> Result<Option<fandhe_device::file_picker::PickedFile>, DeviceError> {
    match code {
        0 => {
            if path_ptr.is_null() {
                // 契約違反（0 を返したのに path_ptr が NULL）を検出した経路。
                return Err(DeviceError::Internal(what));
            }
            // SAFETY: `code == 0` の契約により `path_ptr` は `path_len` バイトの
            // 有効な読み取り可能領域を指す UTF-8 バイト列。`read_picked` はこの関数内で
            // 読み切って戻るため、以降このポインタを再利用しない。
            let path_bytes = unsafe { std::slice::from_raw_parts(path_ptr, path_len) };
            match std::str::from_utf8(path_bytes) {
                Ok(path_str) => read_picked(Path::new(path_str), MAX_FILE_BYTES).map(Some),
                // パスが UTF-8 破損（実務上ほぼ到達しない）の場合は内容を
                // 推測せず失敗にする（`IosClipboard::get_text` と同じ判断）。
                Err(_) => Err(DeviceError::Internal(what)),
            }
        }
        1 => Ok(None),
        _ => Err(DeviceError::Internal(what)),
    }
}

/// iOS 提示関数（Issue #873、案 B）。`device/file_pick.rs` のモバイル版
/// `drive_mobile` から呼ばれる。`request_id` は `bridge::begin` が払い出す
/// プロセス内一意な ID（画面の `generation` ではない、`device/file_pick.rs::bridge`
/// のモジュール doc 参照）。即時 return（`.m` 側が `dispatch_async(main)` へ
/// 提示処理を積んで戻る）ため、呼び出し元をブロックしない。iOS は常に `true`
/// （`.m` 側の `fandhe_ios_file_picker_pick` は常に 0）を返し、提示失敗
/// （top view controller 不在等）も code 2 で非同期に通知する（`.m` ファイル
/// 冒頭コメント参照）。戻り値 `false` は Android の JNI 失敗専用（同期的に
/// 提示依頼自体が失敗しコールバックが二度と来ないケース）で、iOS では到達しない。
pub(crate) fn present_file_picker(request_id: u64) -> bool {
    #[allow(unsafe_code)]
    // SAFETY: `fandhe_ios_file_picker_pick` の契約（`.m` ファイル冒頭コメント）:
    // 引数はプリミティブ整数のみでポインタを経由しない。戻り値 0 は提示を
    // 依頼できたことのみを意味し、実際の選択結果は非同期に
    // `fandhe_rust_file_picker_outcome` 経由で `bridge` へ届く。
    unsafe {
        fandhe_ios_file_picker_pick(request_id) == 0
    }
}

// SAFETY: `ios/fandhe_ios_photo_picker.m` が定義する C ABI（`build.rs` が iOS
// ターゲットのみ・feature `device` 有効時のみコンパイル・リンクする）。契約は
// 下記 [`present_photo_picker`]・[`fandhe_rust_photo_picker_outcome`] のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_photo_picker_pick(request_id: u64) -> i32;
}

/// `ios/fandhe_ios_photo_picker.m` の完了ハンドラから呼ばれる `extern "C"` エクスポート
/// （Issue #1104）。`code` の意味・パスの寿命は [`fandhe_rust_file_picker_outcome`] と同じ
/// （0 = 選択でパスは `NSItemProvider` が渡した一時ファイル、1 = キャンセル、
/// それ以外 = 変換失敗等の内部エラー）。選択時は [`picked_from_path`] が同期に読み切って
/// から戻り、続けて 2 段検査（`validate_photo` → 復号確認）を **この呼び出しスレッド**
/// （`NSItemProvider` のバックグラウンドキュー）で行う。UI スレッドでは復号しない。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_photo_picker_outcome(
    request_id: u64,
    code: i32,
    path_ptr: *const u8,
    path_len: usize,
) {
    let result = picked_from_path(code, path_ptr, path_len, "photo_picker pick");
    // 同じ PHPicker の受け口を写真選択と文字認識（#1252）で共有し、id のタグで振り分ける。
    if crate::device::ios_image_recognition::owns(request_id) {
        crate::device::ios_image_recognition::deliver(request_id, result);
    } else {
        crate::device::photo_pick::bridge::deliver(request_id, result);
    }
    // pull 型 bridge は sender を持たないため、結果を置いたら描画を起こす
    // （`fandhe_rust_file_picker_outcome` と同じ）。
    crate::ios::request_repaint();
}

/// iOS 提示関数（Issue #1104）。`device/photo_pick.rs::drive_mobile` から呼ばれ、
/// 即時 return する（`.m` 側が `dispatch_async(main)` で提示する）。提示失敗も code 2 で
/// 非同期に通知するため、iOS では常に `true`。
pub(crate) fn present_photo_picker(request_id: u64) -> bool {
    #[allow(unsafe_code)]
    // SAFETY: `fandhe_ios_photo_picker_pick` の契約（`.m` ファイル冒頭コメント）:
    // 引数はプリミティブ整数のみでポインタを経由しない。戻り値 0 は提示を依頼できた
    // ことのみを意味し、結果は非同期に `fandhe_rust_photo_picker_outcome` へ届く。
    unsafe {
        fandhe_ios_photo_picker_pick(request_id) == 0
    }
}

// SAFETY: `apple/fandhe_apple_text_scale.m` が定義する C ABI（`build.rs` が iOS
// ターゲットのみ・feature `device` 有効時のみコンパイル・リンクする）。契約は
// 下記 [`IosTextScale::scale_factor`] のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_text_scale_read(out_scale: *mut f32) -> i32;
}

/// iOS 実文字サイズ追従実装（`UIFont.preferredFontForTextStyle:`、Issue #914）。
/// `device/text_scale.rs` の effect ドライバから毎フレーム pull される
/// （push ではなく pull にする設計判断は `fandhe_device::text_scale` モジュール doc・
/// `tilt.rs` 冒頭コメント参照）。
pub struct IosTextScale;

impl TextScale for IosTextScale {
    #[allow(unsafe_code)]
    fn scale_factor(&self) -> Result<f32, DeviceError> {
        // `fandhe_apple_text_scale_read` は main スレッド専用（[`IosMotion::tilt`]
        // と同じ `is_main_thread` ガード、`.m` ファイル冒頭コメント参照）。
        if !is_main_thread() {
            return Err(DeviceError::Internal("text_scale: called off main thread"));
        }

        let mut out_scale: f32 = 0.0;

        // SAFETY: `fandhe_apple_text_scale_read` の契約（`.m` ファイル冒頭コメント）:
        // out 引数はこの関数呼び出し中のみ有効なスタック上の `f32` へのポインタで、
        // 所有権移転・malloc は無い（`IosMotion::tilt` と同じ形）。戻り値が 0 以外の
        // とき out 引数は書き換えられない契約のため、呼び出し直前の初期値のまま扱う。
        let code = unsafe { fandhe_apple_text_scale_read(&mut out_scale) };

        match code {
            0 => Ok(out_scale),
            // 0 以外はすべて「読めない」（`.m` 側の異常系、`pointSize` が 0 以下等）。
            // NaN/範囲外の丸めは `fandhe_device::text_scale::read_scale` に一本化する
            // ため、ここでは FFI 直後の値をそのまま返す。
            _ => Err(DeviceError::Unsupported("text_scale")),
        }
    }
}

#[cfg(test)]
mod tts_outcome_tests {
    use super::TtsOutcome;
    use std::time::Duration;

    /// ObjC 側の delegate コールバックを経由しない、`TtsOutcome` 単体のロジック
    /// 固定テスト（`.m` のビルドを要さず iOS ターゲットでの `cargo test` で回せる）。
    /// static `TTS_OUTCOME` は使わずローカルに構築する（Android `tts_tests` の
    /// P2 是正と同じ理由: 並列実行時の他テストとの干渉を避ける）。

    #[test]
    fn untracked_generation_resolves_immediately() {
        let outcome = TtsOutcome::new();
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn resolve_completes_matching_generation() {
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.resolve(1);
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn cancel_stops_tracking_without_resolve() {
        // 同期失敗（`fandhe_ios_tts_speak` が非 0）時の経路: コールバックが二度と
        // 来ないため `cancel` で畳み、以後の `wait` は「追跡していない」として
        // 即座に成功する。
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.cancel(1);
        assert!(outcome.wait(1).is_ok());
    }

    #[test]
    fn resolve_ignores_mismatched_generation() {
        // 古いコールバック（既に別世代へ置き換わった後に届く finish/cancel）は
        // 無視され、新しい世代の追跡状態を壊さない。
        let outcome = TtsOutcome::new();
        outcome.begin(1);
        outcome.begin(2);
        outcome.resolve(1);
        assert!(
            outcome.wait(1).is_ok(),
            "1 は追跡されていないため即座に成功"
        );
        // 2 はまだ resolve されていないため、別スレッドで待つ。
    }

    #[test]
    fn begin_wakes_up_a_waiter_blocked_on_the_replaced_generation() {
        // `wait_outcome(A)` が待機中に `speak(B)` が来て `begin` が世代を
        // 上書きした場合、（`wait` は時間で打ち切らないため）`notify_all` により
        // 即座に起こされて `Ok(())` で戻ることを固定する（Android
        // `wait_or_ok_wakes_immediately...` と同型の担保。この通知が効いて
        // いなければ `waiter.join()` はテストプロセスごと永久にハングする）。
        use std::sync::Arc;
        let outcome = Arc::new(TtsOutcome::new());
        outcome.begin(1);

        let waiter = {
            let outcome = Arc::clone(&outcome);
            std::thread::spawn(move || outcome.wait(1))
        };
        std::thread::sleep(Duration::from_millis(50));

        let started = std::time::Instant::now();
        outcome.begin(2);
        assert!(waiter.join().expect("panic しない").is_ok());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "begin による置き換えから wait が戻るまで長く掛かりすぎている: {:?}",
            started.elapsed()
        );
    }
}

#[cfg(test)]
mod motion_tests {
    use super::clamp_finite;

    #[test]
    fn clamp_finite_replaces_nan_with_zero() {
        assert_eq!(clamp_finite(f32::NAN), 0.0);
    }

    #[test]
    fn clamp_finite_clamps_out_of_range_values() {
        assert_eq!(clamp_finite(2.0), 1.0);
        assert_eq!(clamp_finite(-2.0), -1.0);
    }

    #[test]
    fn clamp_finite_passes_through_in_range_values() {
        assert_eq!(clamp_finite(0.25), 0.25);
    }
}
