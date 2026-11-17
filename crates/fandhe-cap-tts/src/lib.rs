//! 読み上げ（TTS）の機能 crate（REQ-21 / TASK-21.4、Issue #1354、手順は `docs/operations/capability-crate-migration.md`）。
//! 任意テキストを OS の音声合成で読み上げ、停止する。macOS・Windows は OS 同梱の音声合成コマンド（`CommandTts`、子プロセス）、
//! iOS は `AVSpeechSynthesizer`、Android は `TextToSpeech` が実装する。Web の実体は `fandhe-web-dom` に残る。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（iOS は `ios`、Android は `android`）を
//! 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。入口 [`platform_tts`] は画面の状態型に依存しないため、
//! フレームワークの外のアプリも使える。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`tts_speak_effect`）と、終了時の [`shutdown`]。
//!
//! 守る範囲: 1 プロセス内での発話と停止の順序（唯一のワーカースレッドの FIFO）・嘘の成功の禁止（初期化中・キュー投入成功を
//! 発話成功にしない。接続できないときにダミー実装へ倒さない）・古い世代の結果を回収しない・読み上げ内容をログに出さないこと。
//! 守らない範囲: 他アプリの音声出力との調停、Activity 再生成をまたいだ発話の継続（破棄時は `TtsHelper.detach` が保留中を内部エラー、
//! 発話中をキャンセルで確定する）、サンドボックス配布での子プロセス起動の拒否（`contract` の doc 参照）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;

pub use contract::*;
pub use driver::*;
pub use error::TtsError;
