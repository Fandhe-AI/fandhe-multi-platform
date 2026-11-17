//! 音声認識（speech-recognition）の機能 crate（REQ-21 / TASK-21.4、Issue #1354、手順は
//! `docs/operations/capability-crate-migration.md`）。OS の音声認識で 1 回ぶんの聞き取り（上限 30 秒）を行い、認識した文字列を受け取る。
//! macOS・iOS は `SFSpeechRecognizer`（`apple/fandhe_apple_speech_recognition.m`）、Android は `SpeechRecognizer`
//! （`SpeechRecognitionHelper`）、Windows は WinRT `SpeechRecognizer` が実装する。Web の実体は `fandhe-web-dom` に残り、
//! 契約の純関数・定数・失敗型だけをこの crate から使う。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、Android は `android`、Windows は
//! `windows`）を 1 つに集める。macOS・iOS・Android・Windows は結果が非同期に届く pull 型の bridge（`RequestSlot`）で受け、
//! `SpeechRecognizer` トレイトを通らない（入口 [`platform_speech_recognition`] は `Unsupported` 固定の同期実装を返し、成功を偽装しない）。
//! 画面の状態型を参照してよいのは `driver` だけ。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`speech_recognize_effect`）と、画面状態が再生成で初期化される構成の
//! [`reset_pending_tracking`]（Android、任意）。保留中の要求の回収（再生成の引き継ぎ・終了時の中断）は Java 側がこの crate 内で完結する。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（権限拒否・キャンセル・ライフサイクル中断・認識失敗・認識サービス不在を
//! 区別し、古い世代の結果は捨てる）・背景（`onPause`・非前景）で聞き取りを始めないこと・認識結果の文字列をログに出さないこと。
//! 守らない範囲: 連続認識・言語選択（システム言語固定）、認識サービス側のタイムアウトの個別調整、Activity 再生成をまたいだ聞き取りの継続
//! （聞き取り中の `onPause` で中断する。権限ダイアログ待ちの要求は再生成後の helper が引き継ぐ）。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios", target_os = "macos")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "android", target_os = "ios", target_os = "macos"),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

pub use contract::*;
pub use driver::*;
pub use error::SpeechRecognitionError;
