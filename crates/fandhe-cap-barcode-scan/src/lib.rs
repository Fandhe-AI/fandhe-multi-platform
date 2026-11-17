//! 画像内のバーコード・QR 読み取り（barcode-scan）の機能 crate（REQ-21 / TASK-21.4、Issue #1355、手順は
//! `docs/operations/capability-crate-migration.md`）。写真選択（機能 crate `fandhe-cap-photo-picker`）で選んだ画像 1 枚のバーコード・QR を読み、
//! 中身の文字列の一覧を返す。macOS・iOS は Vision `VNDetectBarcodesRequest`。Web は `fandhe-web-dom`（`BarcodeDetector`）が本体を持ち、
//! この crate の契約の純関数（[`normalize_payloads`]）だけを使う。Android・Windows は OS 標準の API が無く対象外（`Unsupported`）。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、iOS の写真選択との接続は
//! `ios`）を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ（iOS の drive も `driver` 内）。入口 [`platform_barcode_scan`] は状態型に依存しないため、
//! フレームワークの外のアプリも使える（macOS は Vision、それ以外は `Unsupported`）。
//!
//! 画像を選ばせる OS 機能は `fandhe-cap-photo-picker` に依存して使う（Android・wasm32 を除く）。呼び出し元は最終成果物
//! （`fandhe-gallery`）の画面レジストリ（`barcode_scan_effect`）と、`fandhe-web-dom`（契約の純関数）。
//!
//! 守る範囲: 1 画面・1 プロセス内での要求と完了の順序（画面の世代照合）・嘘の成功の禁止（失敗・復号不能を 0 件の成功へ畳まない。0 件は
//! 「コードが無い」成功）・ワーカーが持つバイト列の解放。読んだ中身は信頼できない入力として扱い、表示するだけで URL でも開かず、
//! ログにも出さない。守らない範囲: OS ごとの検出結果・順序の違い、処理時間（タイムアウトは掛けない）、実行中の検出の取り消し
//! （結果は世代照合で捨てる）、複数プロセス。

// unsafe は OS 実装の FFI に限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(not(any(target_os = "ios", target_os = "macos")), forbid(unsafe_code))]
#![cfg_attr(any(target_os = "ios", target_os = "macos"), deny(unsafe_code))]

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::BarcodeScanError;

/// 契約の入力型（利用者が `fandhe-device` へ直接依存せずに済むよう再公開する）。
pub use fandhe_device::photo_picker::{ImageFormat, PickedPhoto, validate_photo};
