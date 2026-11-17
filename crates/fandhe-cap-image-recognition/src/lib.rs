//! 画像の文字認識（image-recognition）の機能 crate（REQ-21 / TASK-21.4、Issue #1355、手順は
//! `docs/operations/capability-crate-migration.md`）。写真選択（機能 crate `fandhe-cap-photo-picker`）で選んだ画像 1 枚の文字（OCR）を
//! 認識して行の一覧を返す。macOS・iOS は Vision `VNRecognizeTextRequest`、Windows は `Windows.Media.Ocr.OcrEngine`。Android は骨格
//! （対応表で `no`。`Unsupported` を返す）、Web は標準 API が無く対象外。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、iOS の写真選択との接続は
//! `ios`、Windows は `windows`、Android は `android`）を 1 つに集める。画面の状態型を参照してよいのは `driver`・`ios`・`android` だけ。
//! 入口 [`platform_image_recognition`] は状態型に依存しないため、フレームワークの外のアプリも使える（macOS は Vision、Windows は
//! `OcrEngine`、iOS・Android・Linux は `Unsupported`）。
//!
//! 画像を選ばせる OS 機能は `fandhe-cap-photo-picker` に依存して使う（Android を除く。Android は骨格のため写真選択の宣言を混ぜない）。
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`image_recognize_effect`）。
//!
//! 守る範囲: 1 画面・1 プロセス内での要求と完了の順序（画面の世代照合）・嘘の成功の禁止（失敗・復号不能・言語データ不足を 0 行の成功へ
//! 畳まない。0 行は「文字が無い」成功）・ワーカーが持つバイト列の解放・認識した文字と画像をログへ出さないこと。守らない範囲: OS ごとの
//! 認識結果・行の分け方・順序の違い、処理時間（タイムアウトは掛けない）、実行中の OCR の取り消し（結果は世代照合で捨てる）、複数プロセス。

// unsafe は OS 実装の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ImageRecognitionError;
