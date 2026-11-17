//! 画像の文字認識（image-recognition）の機能 crate（REQ-21 / TASK-21.4、Issue #1355、手順は
//! `docs/operations/capability-crate-migration.md`）。写真選択（機能 crate `fandhe-cap-photo-picker`）で選んだ画像 1 枚の文字（OCR）を
//! 認識して行の一覧を返す。macOS・iOS は Vision `VNRecognizeTextRequest`、Windows は `Windows.Media.Ocr.OcrEngine`。Android は骨格
//! （対応表で `no`。`Unsupported` を返す）、Web は標準 API が無く対象外。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、iOS の写真選択との接続は
//! `ios_effect`（`driver` 内）、Windows は `windows`、Android は `unsupported_effect`（`driver` 内））を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。
//! 状態型に依存しない入口は 2 つ。同期の [`platform_image_recognition`]（macOS は Vision、Windows は `OcrEngine`、iOS・Android・Linux は
//! `Unsupported`）と、iOS の非同期の [`begin_image_recognition`] / [`poll_image_recognition`] / [`discard_image_recognition`]（写真選択を開いて
//! 選ばれた画像を認識する。画面 effect と同じ slot を持ち主で分けて共有し、進行中は busy で断る）。フレームワークの外のアプリも使える。
//!
//! 画像を選ばせる OS 機能は `fandhe-cap-photo-picker` に依存して使う（Android・wasm32 を除く。Android は骨格、wasm32 は対象外のため写真選択の宣言を混ぜない）。
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`image_recognize_effect`）。
//!
//! 守る範囲: 1 画面・1 プロセス内での要求と完了の順序（画面の世代照合）・嘘の成功の禁止（失敗・復号不能・言語データ不足を 0 行の成功へ
//! 畳まない。0 行は「文字が無い」成功）・ワーカーが持つバイト列の解放・認識した文字と画像をログへ出さないこと。守らない範囲: OS ごとの
//! 認識結果・行の分け方・順序の違い、処理時間（タイムアウトは掛けない）、実行中の OCR の取り消し（結果は世代照合で捨てる）、複数プロセス。

// unsafe は OS 実装の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::ImageRecognitionError;

/// 契約の入力型（利用者が `fandhe-device` へ直接依存せずに済むよう再公開する）。
pub use fandhe_device::photo_picker::{ImageFormat, PickedPhoto, validate_photo};
