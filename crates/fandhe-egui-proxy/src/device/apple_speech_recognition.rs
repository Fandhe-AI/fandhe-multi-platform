//! iOS/macOS 向け音声認識実装（Issue #1024 macOS + #1025 iOS）。`device/
//! speech_recognition.rs` の iOS/macOS 経路から呼ばれ、`Speech`
//! （`SFSpeechRecognizer`）実装（`apple/fandhe_apple_speech_recognition.m`、
//! `build.rs` が iOS/macOS の feature `device` 有効時にコンパイル・リンクする）を
//! `unsafe extern "C"` 2 関数（[`present`]/[`request_stop`]）で呼ぶ。`unsafe` は
//! この呼び出しに閉じ `// SAFETY:` で契約を明記する（`apple_mic_record.rs` と同型、
//! coding-rust.md の FFI 例外規定準拠）。
//!
//! `.m` 側の完了コールバックから [`fandhe_rust_speech_finished`]（本モジュールが
//! 公開する `extern "C"` 関数）が呼ばれ、`device/speech_recognition::bridge` へ
//! 結果を push する（pull 型、`apple_mic_record.rs::fandhe_rust_mic_record_finished`
//! と同じ橋渡し方式）。認識文字列をログへ出力しない（security.md A09）。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Duration;

use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_speech_recognition.m` が定義する C ABI（`build.rs` が
// macOS ターゲットの feature `device` 有効時にコンパイル・リンクする）。契約は
// `.m` ファイル冒頭コメントおよび下記関数のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_speech_start(request_id: u64, max_seconds: f64) -> i32;
    fn fandhe_apple_speech_stop(request_id: u64) -> i32;
}

/// `.m` 側が実際に聞き取りを開始した瞬間に呼ぶ `extern "C"` エクスポート関数
/// （`apple_mic_record.rs::fandhe_rust_mic_record_started` と同じ位置づけ）。
/// 権限確認待ちの間は呼ばれない。`device/speech_recognition::bridge` へ「実際に
/// 開始した request_id」を記録し、経過時間の計測（`tick`）をこの時点から
/// 始められるようにする。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_speech_started(request_id: u64) {
    super::speech_recognition::bridge::mark_started(request_id);
}

/// `.m` 側の聞き取り開始を依頼する（`device/speech_recognition.rs` の effect
/// ドライバから呼ばれる契約）。`request_id` は `device/speech_recognition::
/// bridge::begin` が払い出したプロセス内一意な ID（画面の `generation` ではない、
/// `apple_mic_record.rs::present` と同じ設計）。`.m` 側は常に 0（受理）を返す契約の
/// ため戻り値は常に `true` になるが、型を揃えるため `bool` のままにする。
#[allow(unsafe_code)]
pub(crate) fn present(request_id: u64, max: Duration) -> bool {
    // SAFETY: 引数はプリミティブのみでポインタを経由しない。戻り値 0 は依頼を
    // 受理できたことのみを意味し、実際の認識結果は非同期に
    // `fandhe_rust_speech_finished` 経由で `bridge` へ届く（`.m` ファイル冒頭
    // コメント参照）。
    let code = unsafe { fandhe_apple_speech_start(request_id, max.as_secs_f64()) };
    code == 0
}

/// `.m` 側の聞き取り停止を依頼する（`device/speech_recognition.rs` の「停止」
/// ボタンハンドラから呼ばれる契約）。`request_id` が `.m` 側の進行中セッションと
/// 一致しない場合は無視される（`.m` ファイル冒頭コメント参照）。
#[allow(unsafe_code)]
pub(crate) fn request_stop(request_id: u64) -> bool {
    // SAFETY: 引数はプリミティブのみ。戻り値は常に 0（`.m` 側は非同期に処理する）
    // で、実際の結果は `fandhe_rust_speech_finished` 経由で届く。
    let code = unsafe { fandhe_apple_speech_stop(request_id) };
    code == 0
}

/// `.m` 側の完了コールバックから呼ばれる `extern "C"` エクスポート関数（Issue #1024）。
/// 契約（`.m` ファイル冒頭コメント）:
///   `code`: 0 = 成功（`text` が有効、空文字列もあり得る）。1 = 権限拒否。
///   2 = 内部エラー。3 = 二重開始（busy）。4 = 許可確認中に利用者が「停止」で
///   キャンセル。5 = バックグラウンド遷移により破棄された（iOS のみ）。
///   6 = 認識器を作れない。7 = 認識器が一時的に使えない。
///   8 = 無音（結果 0 件）。0 以外では `text` を参照しない（NULL）。
///
/// `text` が指す領域はこの呼び出しが戻るまでのみ有効（`.m` 側のローカル変数）。
/// 本関数内で `to_string_lossy` により同期的にコピーし切ってから戻る契約
/// （`fandhe_rust_mic_record_finished` と同型）。写像は
/// `super::speech_recognition::error_from_code`（SSOT）へ一本化する。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_speech_finished(request_id: u64, code: i32, text: *const c_char) {
    let result = if code == 0 {
        if text.is_null() {
            // 契約違反（成功なのにポインタが NULL）を検出した経路。
            Err(DeviceError::Internal(
                "speech_recognition: null result on success",
            ))
        } else {
            // SAFETY: `code == 0` の契約により `text` は NUL 終端 UTF-8 文字列を
            // 指す。この呼び出しが戻るまでのみ有効なため、ここで即座にコピーする
            // （上記 doc 参照）。
            let text_str = unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned();
            fandhe_device::speech_recognition::outcome_from_transcript(text_str)
        }
    } else {
        super::speech_recognition::error_from_code(code)
    };
    super::speech_recognition::bridge::deliver(request_id, result);
}
