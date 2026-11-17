//! マイク録音（mic-record）の機能 crate（REQ-21 / TASK-21.4、Issue #1354、手順は `docs/operations/capability-crate-migration.md`）。
//! OS のマイクで 1 回ぶんの録音（上限 30 秒）を行い、録音データ（サイズ・MIME・長さ）を受け取る。macOS・iOS は `AVAudioRecorder`
//! （`apple/fandhe_apple_mic_record.m`）、Android は `MediaRecorder`（`MicRecordHelper`）、Windows は WinRT `MediaCapture` が実装する。
//! Web の実体は `fandhe-web-dom` に残り、契約の純関数・定数・失敗型だけをこの crate から使う。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・iOS は `apple`、Android は `android`、Windows は
//! `windows`）を 1 つに集める。macOS・iOS・Android・Windows は結果が非同期に届く pull 型の bridge（`RequestSlot`）で受け、
//! `MicRecorder` トレイトを通らない（入口 [`platform_mic_record`] は `Unsupported` 固定の同期実装を返し、成功を偽装しない）。
//! 画面の状態型を参照してよいのは `driver` だけ。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`mic_record_effect`）と、画面状態が再生成で初期化される構成の
//! [`reset_pending_tracking`]（Android、任意）。保留中の要求の回収（再生成の引き継ぎ・終了時の中断）は Java 側がこの crate 内で完結する。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（権限拒否・キャンセル・ライフサイクル中断・失敗を区別し、古い世代の
//! 結果は捨てる）・背景（`onPause`・非前景）で録音を始めないこと・録音データをログに出さないこと・サイズ上限（`MAX_RECORDING_BYTES`）。
//! 守らない範囲: 他アプリの音声入力との調停、Activity 再生成をまたいだ録音の継続（録音中の `onPause` で中断する。権限ダイアログ待ちの要求は再生成後の helper が引き継ぐ）、
//! マイク未搭載・他アプリ使用中の個別判定（Windows は `Internal` に畳む）。

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
pub use error::MicRecordError;
