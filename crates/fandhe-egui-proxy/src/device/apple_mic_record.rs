//! iOS / macOS 向けマイク録音実装（Issue #954 macOS + #956 iOS）。
//! `device/mic_record.rs` の iOS/macOS 経路から呼ばれ、`AVAudioRecorder` 実装
//! （`apple/fandhe_apple_mic_record.m`、`build.rs` が iOS/macOS の feature
//! `device` 有効時にコンパイル・リンクする）を `unsafe extern "C"` 2 関数（[`present`]/
//! [`request_stop`]）で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を
//! 明記する（`apple_biometric.rs` と同型、coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の完了コールバックから [`fandhe_rust_mic_record_finished`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、`device/mic_record::bridge` へ結果を push
//! する（pull 型、`apple_biometric.rs::fandhe_rust_biometric_outcome` と同じ
//! 橋渡し方式）。録音データの中身をログへ出力しない（security.md A09）。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Duration;

use fandhe_device::mic_record::build_recording;
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_mic_record.m` が定義する C ABI（`build.rs` が
// iOS/macOS ターゲットの feature `device` 有効時にコンパイル・リンクする）。
// 契約は `.m` ファイル冒頭コメントおよび下記関数のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_mic_record_start(request_id: u64, max_duration_seconds: f64) -> i32;
    fn fandhe_apple_mic_record_stop(request_id: u64) -> i32;
}

/// `.m` 側が実際に `recordForDuration:` を開始した瞬間に呼ぶ `extern "C"`
/// エクスポート関数（Issue #954 レビュー指摘）。権限確認待ちの間は呼ばれない。
/// `device/mic_record::bridge` へ「実際に開始した request_id」を記録し、
/// 経過時間の計測（`tick`）をこの時点から始められるようにする
/// （`.m` ファイル冒頭 `fandhe_rust_mic_record_started` 節参照）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_mic_record_started(request_id: u64) {
    super::mic_record::bridge::mark_started(request_id);
}

/// `.m` 側の録音開始を依頼する（`device/mic_record.rs` の effect ドライバから
/// 呼ばれる契約）。`request_id` は `device/mic_record::bridge::begin` が払い出した
/// プロセス内一意な ID（画面の `generation` ではない、`apple_biometric.rs::present`
/// と同じ設計）。`.m` 側は常に 0（受理）を返す契約のため、戻り値は常に `true`
/// になるが、`apple_biometric.rs::present` と型を揃えるため `bool` のままにする。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, max_duration: Duration) -> bool {
    // SAFETY: 引数はプリミティブのみでポインタを経由しない。戻り値 0 は依頼を
    // 受理できたことのみを意味し、実際の録音結果は非同期に
    // `fandhe_rust_mic_record_finished` 経由で `bridge` へ届く（`.m` ファイル
    // 冒頭コメント参照）。
    let code = unsafe { fandhe_apple_mic_record_start(request_id, max_duration.as_secs_f64()) };
    code == 0
}

/// `.m` 側の録音停止を依頼する（`device/mic_record.rs` の「停止」ボタンハンドラから
/// 呼ばれる契約）。`request_id` が `.m` 側の進行中セッションと一致しない場合は
/// 無視される（`.m` ファイル冒頭コメント参照）。
#[allow(unsafe_code)]
pub(crate) fn request_stop(request_id: u64) -> bool {
    // SAFETY: 引数はプリミティブのみ。戻り値は常に 0（`.m` 側は非同期に
    // 処理する）で、実際の結果は `fandhe_rust_mic_record_finished` 経由で届く。
    let code = unsafe { fandhe_apple_mic_record_stop(request_id) };
    code == 0
}

/// `.m` 側から届く結果コード（`code`）→ `DeviceError` の写像は Android と共有の
/// SSOT（`device/mic_record.rs::error_from_code`、Issue #957 で移動）へ一本化した。
///
/// `.m` 側の完了コールバックから呼ばれる `extern "C"` エクスポート関数（Issue #954・
/// #956）。契約（`.m` ファイル冒頭コメント）:
///   `code`: 0 = 成功（`bytes`/`bytes_len`/`mime`/`duration_ms` が有効）。
///   1 = 権限拒否。2 = 内部エラー。3 = 二重開始（busy）。4 = 許可確認中に
///   利用者が「停止」でキャンセル。5 = バックグラウンド遷移により破棄
///   （iOS のみ）。0 以外では `bytes`/`mime` を参照しない（NULL）。
///
/// `bytes`/`mime` が指す領域はこの呼び出しが戻るまでのみ有効（`.m` 側のローカル
/// 変数）。本関数内で `to_vec`/`to_string_lossy` により同期的にコピーし切ってから
/// 戻る契約（`fandhe_rust_file_picker_outcome` と同型）。バイト列の不変条件
/// （0 バイト禁止・`duration_ms == 0` 禁止・上限超過禁止）は
/// [`fandhe_device::mic_record::build_recording`] に一本化する（SSOT）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_mic_record_finished(
    request_id: u64,
    code: i32,
    bytes: *const u8,
    bytes_len: u64,
    mime: *const c_char,
    duration_ms: u64,
) {
    let result = if code == 0 {
        if bytes.is_null() || mime.is_null() {
            // 契約違反（成功なのにポインタが NULL）を検出した経路。
            Err(DeviceError::Internal("mic_record: null result on success"))
        } else {
            // SAFETY: `code == 0` の契約により `bytes` は `bytes_len` バイトの
            // 有効な読み取り可能領域、`mime` は NUL 終端 UTF-8 文字列を指す。
            // いずれもこの呼び出しが戻るまでのみ有効なため、ここで即座に
            // コピーする（上記 doc 参照）。
            let owned_bytes =
                unsafe { std::slice::from_raw_parts(bytes, bytes_len as usize) }.to_vec();
            let mime_str = unsafe { CStr::from_ptr(mime) }
                .to_string_lossy()
                .into_owned();
            build_recording(owned_bytes, mime_str, duration_ms)
        }
    } else {
        Err(super::mic_record::error_from_code(code))
    };
    super::mic_record::bridge::deliver(request_id, result);
}
