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
/// Issue #1314: Android 向け全画面表示実装（`FullscreenHelper` との JNI 橋渡し）。
/// `device/fullscreen_immersive.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_fullscreen_immersive;
/// Issue #1250: Android 向け文字認識の骨格（#1253 が ML Kit 実装に置き換える）。
/// `device/image_recognition.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_image_recognition;
/// Issue #355: Android 向けペン・筆圧入力実装。`device/pen_stylus.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_pen_stylus;
/// Issue #328: Android 向け印刷（`PrintHelper` + JNI）。`device/print.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_print;
/// Issue #446: Android 向けセーフエリア実装（既存の insets リスナーの値を pull で読む）。
/// `device/safe_area_insets.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_safe_area_insets;
/// Issue #655: Android 向け画面向き実装（#657 が埋めるスタブ）。
/// `device/screen_orientation.rs` の Android 経路から使われる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_screen_orientation;
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
/// Phase 11 の拡充ループ（親 #121、Issue #585）: ワンタイムコード入力の effect ドライバ（iOS は描画層の入力欄の橋渡し
/// （`HostView::set_text_input_hint`、実装は `ios_text_input.rs`）で実装、Android・macOS は保留、Windows は非対応で `UnsupportedAutofillOtp` 固定。
/// Web は `fandhe-web-dom` が担う）。
pub mod autofill_otp;
/// Phase 11（端末機能の拡充ループ、親 #116、Issue #557）: 画像内のバーコード・QR 読み取りの effect ドライバ。
pub mod barcode_scan;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #1133、Issue #1312）: 全画面表示の effect ドライバ（macOS/Windows は
/// eframe の viewport コマンド、iOS/Android は OS 別モジュール、他 OS は
/// `UnsupportedFullscreen` 固定。Web は `fandhe-web-dom`）。
pub mod fullscreen_immersive;
/// Phase 11 の拡充ループ（親 #1120、Issue #1250）: 画像の文字認識の effect ドライバ
/// （macOS・Windows / iOS 実装、Android は骨格のスタブ（対応表で `no`）、他 OS は
/// `UnsupportedTextRecognizer` 固定。Web は対象外）。
pub mod image_recognition;
#[cfg(target_os = "ios")]
pub mod ios;
/// Issue #557: iOS 向けバーコード・QR 読み取りの骨格（実装は #558）。
#[cfg(target_os = "ios")]
pub mod ios_barcode_scan;
/// Issue #1313: iOS 向け全画面表示実装（`ios/fandhe_ios_fullscreen_immersive.m` を呼ぶ
/// C ABI ラッパー）。
/// `device/fullscreen_immersive.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_fullscreen_immersive;
/// Issue #1252: iOS 向け文字認識（PHPicker + Vision）の effect ドライバ。
#[cfg(target_os = "ios")]
pub mod ios_image_recognition;
/// Issue #356: iOS 向けペン・筆圧入力実装（`ios/fandhe_ios_pen_stylus.m` の観測専用
/// `UIGestureRecognizer` が値を置く C ABI ラッパー）。`device/pen_stylus.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_pen_stylus;
/// Issue #327: iOS 向け印刷実装（`ios/fandhe_ios_print.m` を呼ぶ C ABI ラッパー、`UIPrintInteractionController`）。
/// `device/print.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_print;
/// Issue #444: iOS 向けセーフエリア実装。
/// `device/safe_area_insets.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_safe_area_insets;
/// Issue #655: iOS 向け画面向き実装（#656 が埋めるスタブ）。
/// `device/screen_orientation.rs` の iOS 経路から使われる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_screen_orientation;
/// Issue #355: macOS 向けペン・筆圧入力実装（`macos/fandhe_macos_pen_stylus.m` の `NSEvent` local
/// monitor を呼ぶ C ABI ラッパー）。`device/pen_stylus.rs` の macOS 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_pen_stylus;
/// Issue #325: macOS 向け印刷実装（`macos/fandhe_macos_print.m` を呼ぶ C ABI ラッパー）。
/// `device/print.rs` の macOS 経路から呼ばれる。結果コードの写像をホストでテストできるよう
/// cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_print;
/// Issue #444: macOS 向けセーフエリア実装（`macos/fandhe_macos_safe_area_insets.m` を呼ぶ
/// C ABI ラッパー）。`device/safe_area_insets.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_safe_area_insets;
/// Issue #636: macOS 向けウィンドウ管理の読み取り実装（`macos/fandhe_macos_window_management.m` を呼ぶ
/// C ABI ラッパー）。`device/window_management.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_window_management;
/// Phase 11 の拡充ループ（親 #953、Issue #954・#955・#956・#957）: マイク録音の
/// effect ドライバ（macOS/iOS/Windows/Android 実装、他 OS は `UnsupportedMicRecorder`
/// 固定）。
pub mod mic_record;
/// Phase 11 の拡充ループ（親 #95、Issue #355）: ペン・筆圧入力の effect ドライバ（macOS・iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedPenStylus` 固定。
/// Web は `fandhe-web-dom`）。
pub mod pen_stylus;
/// Phase 11 の拡充ループ（親 #1101、Issue #1103/#1104）: 写真選択の effect ドライバ（macOS /
/// Windows は `PhotoPicker` 実装、iOS/Android は pull 型 `bridge`、Linux は
/// `UnsupportedPhotoPicker` 固定。Web は `fandhe-web-dom`）。選んだ画像は復号を確かめてから
/// `image_store` 経由で描く。
pub mod photo_pick;
/// Phase 11 の拡充ループ（親 #94、Issue #325）: 印刷ダイアログの effect ドライバ（macOS 実装、
/// Windows / iOS / Android 実装、他 OS は `UnsupportedPrint` 固定。
/// Web は `fandhe-web-dom`）。画面の内容の印刷（print-screen、親 #341、Issue #1010）の取り込み段と
/// effect ドライバも同居する（macOS / Windows 実装、iOS / Android はスタブ）。
pub mod print;
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
/// Phase 11 の拡充ループ（親 #105、Issue #444）: セーフエリアの effect ドライバ
/// （macOS・iOS / Android 実装、他 OS は `UnsupportedSafeArea` 固定）。
pub mod safe_area_insets;
/// Phase 11 の拡充ループ（親 #128、Issue #655）: 画面向きの effect ドライバ
/// （iOS は #656、Android は #657 が埋めるスタブ、Web は #658 の `fandhe-web-dom`、
/// 他 OS は `UnsupportedScreenOrientation` 固定）。
pub mod screen_orientation;
/// Phase 11 の拡充ループ（親 #1023、Issue #1024・#1025）: 音声認識の effect ドライバ
/// （macOS/iOS/Windows/Android 実装、Android/Web は兄弟 issue）。
pub mod speech_recognition;
/// Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）: `device/
/// file_pick.rs` のライフサイクル処理（`PickHooks`/`drive_with`/`drive_mobile_with`）と
/// `bridge` を共有する effect ドライバ。
pub mod tsv_import;
pub mod tts;
/// Phase 11 の拡充ループ（親 #127、Issue #636）: ウィンドウ管理（位置・サイズ・最前面）の effect ドライバ
/// （macOS 実装、Windows は #637、他 OS は `UnsupportedWindowManager` 固定。Web は `fandhe-web-dom`）。
pub mod window_management;
/// Issue #1250: Windows 向け文字認識の骨格（#1251 が WinRT `OcrEngine` 実装に置き換える）。
/// `device/image_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_image_recognition;
/// Issue #955: Windows 向けマイク録音実装（WinRT `MediaCapture` ラッパー）。
/// `device/mic_record.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_mic_record;
/// Issue #359: Windows 向けペン・筆圧入力実装（`SetWindowSubclass` の観測専用サブクラスが
/// `WM_POINTER*` から値を置く）。`device/pen_stylus.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_pen_stylus;
/// Issue #326: Windows 向け印刷実装（`PrintDlgExW` + GDI）。`device/print.rs` の Windows 経路から
/// 呼ばれる。結果の写像は純関数でホストでもテストできる（ffi 部分は `cfg(target_os = "windows")`）。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_print;
/// Issue #1024: Windows 向け音声認識実装（WinRT `SpeechRecognizer` ラッパー）。
/// `device/speech_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_speech_recognition;
/// Issue #637: Windows 向けウィンドウ管理の読み取り（`NativeReads` の Windows 実装。最前面か・今いる画面の
/// 作業領域を Win32 で読む）。`device/window_management.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_window_management;

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
