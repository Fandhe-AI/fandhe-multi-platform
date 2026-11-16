//! Phase 8（端末機能、親 #752）の画面ごとの effect ドライバの置き場（Issue #746/#747/
//! #748/#749）。`fandhe-device` のトレイト（`Storage`/`Http` 等）を実際に呼び出し、
//! 結果を `CompletionSender`（`crate::app`）経由で画面状態へ渡すのはこのモジュール以下に
//! 限る（`fandhe-core` は端末依存ゼロ、`docs/ai/SCHEMA.md`「新規画面ファイルの置き場」節
//! 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`fandhe-device` への依存自体が `Cargo.toml` の native 分岐にしか無いため。
//! wasm32 は feature を on にしても依存が入らず本モジュール自体が存在しない、`lib.rs` の
//! `mod` 宣言参照）。`fandhe-core` は `fandhe-device` を参照しない（依存方向はプロキシ →
//! コアの一方向、coding-rust.md 準拠）ため、両者の橋渡しは本クレート側の責務にする。
//!
//! ノート保存（`storage_note`）の橋渡しは機能 crate `fandhe-cap-local-storage-kv`（Issue #1332）へ移した。
//! Issue #748（HTTP 取得）・
//! #749（カメラ撮影）・#830（傾きで玉が転がる）の effect ドライバはそれぞれ
//! [`http_fetch`] / [`tilt`] サブモジュールに置く（カメラ撮影は機能 crate `fandhe-cap-camera-capture`、#1170）
//! （`registry.rs` の `effect` 区画から配線される）。

#[cfg(target_os = "android")]
pub mod android;
/// Issue #1250: Android 向け文字認識の骨格（#1253 が ML Kit 実装に置き換える）。
/// `device/image_recognition.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_image_recognition;
/// Issue #557 macOS・#558 iOS: バーコード・QR 読み取り実装（`apple/fandhe_apple_barcode_scan.m` を
/// 呼ぶ C ABI ラッパー）。`device/barcode_scan.rs` の macOS 経路と `ios_barcode_scan` から呼ばれる。
/// `build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_barcode_scan;
/// Issue #1250 macOS・#1252 iOS: 文字認識実装（`apple/
/// fandhe_apple_image_recognition.m` を呼ぶ C ABI ラッパー）。`device/image_recognition.rs` の
/// macOS 経路と `ios_image_recognition` から呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_image_recognition;
/// Issue #954（macOS）/ #956（iOS）: iOS / macOS 向けマイク録音実装（`apple/
/// fandhe_apple_mic_record.m` を呼ぶ C ABI ラッパー）。`device/mic_record.rs` の
/// iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_mic_record;
/// Issue #1024（macOS）/ #1025（iOS）: iOS / macOS 向け音声認識実装（`apple/
/// fandhe_apple_speech_recognition.m` を呼ぶ C ABI ラッパー）。`device/
/// speech_recognition.rs` の iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_speech_recognition;
/// Phase 11（端末機能の拡充ループ、親 #116、Issue #557）: 画像内のバーコード・QR 読み取りの effect ドライバ。
pub mod barcode_scan;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #1120、Issue #1250）: 画像の文字認識の effect ドライバ
/// （macOS・Windows / iOS 実装、Android は骨格のスタブ（対応表で `no`）、他 OS は
/// `UnsupportedTextRecognizer` 固定。Web は対象外）。
pub mod image_recognition;
#[cfg(target_os = "ios")]
pub mod ios;
/// Issue #557: iOS 向けバーコード・QR 読み取りの骨格（実装は #558）。
#[cfg(target_os = "ios")]
pub mod ios_barcode_scan;
/// Issue #1252: iOS 向け文字認識（PHPicker + Vision）の effect ドライバ。
#[cfg(target_os = "ios")]
pub mod ios_image_recognition;
/// Phase 11 の拡充ループ（親 #953、Issue #954・#955・#956・#957）: マイク録音の
/// effect ドライバ（macOS/iOS/Windows/Android 実装、他 OS は `UnsupportedMicRecorder`
/// 固定）。
pub mod mic_record;
/// Phase 11 の拡充ループ（親 #1101、Issue #1103/#1104）: 写真選択の effect ドライバ（macOS /
/// Windows は `PhotoPicker` 実装、iOS/Android は pull 型 `bridge`、Linux は
/// `UnsupportedPhotoPicker` 固定。Web は `fandhe-web-dom`）。選んだ画像は復号を確かめてから
/// `image_store` 経由で描く。
pub mod photo_pick;
/// request/response 型端末機能共通の要求追跡（Issue #943、親 #941）。share /
/// biometric_auth / file_pick の `bridge` facade から使う。
/// `RequestSlot` / `poll_and_finish` の実体は `fandhe-runtime`（Issue #1166）。呼び出し側のパスを保つ再公開。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    allow(unused_imports)
)]
pub(crate) use fandhe_runtime::request_slot;
/// Phase 11 の拡充ループ（親 #1023、Issue #1024・#1025）: 音声認識の effect ドライバ
/// （macOS/iOS/Windows/Android 実装、Android/Web は兄弟 issue）。
pub mod speech_recognition;
/// Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）: `device/
/// file_pick.rs` のライフサイクル処理（`PickHooks`/`drive_with`/`drive_mobile_with`）と
/// `bridge` を共有する effect ドライバ。
pub mod tsv_import;
pub mod tts;
/// Issue #1250: Windows 向け文字認識の骨格（#1251 が WinRT `OcrEngine` 実装に置き換える）。
/// `device/image_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_image_recognition;
/// Issue #955: Windows 向けマイク録音実装（WinRT `MediaCapture` ラッパー）。
/// `device/mic_record.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_mic_record;
/// Issue #1024: Windows 向け音声認識実装（WinRT `SpeechRecognizer` ラッパー）。
/// `device/speech_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_speech_recognition;

/// アプリ終了時に未移行の端末リソースを解放する（最終成果物（`fandhe-gallery`）の `register_teardown_hooks` 経由で `ProxyApp::on_exit` が呼ぶ契約、
/// 機能 crate の停止は gallery の終了フックが本関数の後に呼ぶ。停止の順序は「未移行 → 機能 crate」になるが、相互に独立している。
/// `fandhe-render-egui` の `app.rs` の `EguiApp::on_exit` 実装 doc 参照）。プロセスが終了すると
/// `static` の `Drop` は走らないため、生きている間に明示停止が要るリソースを
/// ここへ集約する。現状は TTS の子プロセス停止のみ（Issue #865 の codex-review
/// P1 指摘: 共有 TTS を `static OnceLock` で保持しているため `Drop` が実行されず、
/// ウィンドウを閉じても発話中の子プロセスが残る）。
pub fn shutdown() {
    tts::stop_shared_tts();
}
