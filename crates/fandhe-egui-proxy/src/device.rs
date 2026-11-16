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
/// Issue #426: Android 向けアプリショートカット実装（`ShortcutManager` の動的ショートカット）。
/// `device/app_shortcuts.rs` の android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_app_shortcuts;
/// Issue #392: Android 向けバックグラウンドダウンロード実装（`DownloadManager` を同期 JNI で呼ぶ）。
/// `device/background_download.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_background_download;
/// Issue #835（骨格）/ #837（実装）: Android 向けバックグラウンド送信実装。
/// `device/background_upload.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_background_upload;
/// Issue #1279: Android 向けディープリンク（App Links）受信（https の `ACTION_VIEW` を JNI で受ける）。
/// `device/deep_link.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_deep_link;
/// Issue #416: Android 向けファイルの関連付け受信のスタブ（b-android が埋める）。
/// `device/file_association.rs::platform` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_file_association;
/// Issue #1250: Android 向け文字認識の骨格（#1253 が ML Kit 実装に置き換える）。
/// `device/image_recognition.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_image_recognition;
/// Issue #562: Android 向け OS の検索への登録実装（AppSearch、API 31+、Java は `MainActivity`）。
/// `device/spotlight_index.rs` の Android 経路から呼ばれる。結果コード写像をホストでテストするため
/// test でもコンパイルする。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_spotlight_index;
/// Issue #1218: Android 向けカスタム URL scheme 受信実装（Java 側 `MainActivity` の
/// `onCreate` / `onNewIntent` から JNI で呼ばれる）。`device/url_scheme.rs::
/// platform_url_scheme` の Android 経路から使われる。
#[cfg(target_os = "android")]
pub mod android_url_scheme;
/// Issue #534: Android 向け Web 認証セッション実装（Custom Tabs、Java 側 `WebAuthSessionHelper`）。
/// `device/web_auth_session.rs` の Android 経路から呼ばれる。純関数をホストでテストするため test でもコンパイルする。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_web_auth_session;
/// Issue #424（親 #101）: アプリショートカット（長押し・ジャンプリスト・Dock メニュー）の登録ドライバ。
/// `device/url_scheme.rs::url_scheme_show_effect` から呼ばれる。実装は macOS（Dock メニュー）、
/// iOS は Info.plist の静的宣言（`Declared`、#425）、Android・Windows（ジャンプリスト）は実装済み、他 OS は `UnsupportedAppShortcuts` 固定。
pub mod app_shortcuts;
/// Issue #390（macOS）/ #391（iOS）: バックグラウンドダウンロード実装（`apple/fandhe_apple_background_download.m` の
/// background `NSURLSession` を呼ぶ C ABI ラッパー）。`device/background_download.rs` の macOS / iOS 経路から呼ばれる。
/// 状態遷移の純ロジックをホストの `cargo test` で検査するため cfg に `test` を含める。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub mod apple_background_download;
/// Issue #835（macOS）/ #836（iOS）: バックグラウンド送信実装（`apple/fandhe_apple_background_upload.m` の
/// background `NSURLSession` を呼ぶ C ABI ラッパー）。`device/background_upload.rs` の macOS 経路（iOS は
/// `ios_background_upload` 経由）から呼ばれる。結果の照合の純ロジックをホストの `cargo test` で検査するため
/// cfg に `test` を含める。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub mod apple_background_upload;
/// Issue #557 macOS・#558 iOS: バーコード・QR 読み取り実装（`apple/fandhe_apple_barcode_scan.m` を
/// 呼ぶ C ABI ラッパー）。`device/barcode_scan.rs` の macOS 経路と `ios_barcode_scan` から呼ばれる。
/// `build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_barcode_scan;
#[cfg(any(target_os = "ios", target_os = "macos"))]
/// Issue #416: macOS / iOS 向けファイルの関連付け受信（`apple/fandhe_apple_file_association.m` の
/// Apple Event `kAEOpenDocuments` ハンドラから呼ばれる C ABI ラッパー）。`device/file_association.rs::platform` の
/// macOS 経路と `main.rs`（登録）から使われる。iOS は b-ios が埋めるまで受け口だけがある。
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub mod apple_file_association;
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
/// Issue #560（macOS）・#561（iOS）: OS の検索（Core Spotlight）への登録実装（`apple/
/// fandhe_apple_spotlight_index.m` を呼ぶ C ABI ラッパー）。`device/spotlight_index.rs` の macOS 経路と、
/// iOS 経路（#561 で `ios_spotlight_index` 経由）から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_spotlight_index;
/// Issue #1215・#1217: macOS / iOS 向けカスタム URL scheme 受信（`macos/fandhe_macos_url_scheme.m`
/// の Apple Event ハンドラ、`ios/fandhe_ios_url_scheme.m` の delegate から呼ばれる C ABI
/// ラッパー）。`device/url_scheme.rs::platform_url_scheme` の macOS・iOS 経路と
/// `main.rs`（登録）から使われる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_url_scheme;
/// Phase 11 の拡充ループ（親 #100、Issue #390）: OS 管理のバックグラウンドダウンロードの effect ドライバ
/// （macOS・iOS・Android・Windows 実装、他 OS は `Unsupported` 固定）。
pub mod background_download;
/// Phase 11 の拡充ループ（親 #170、Issue #835）: OS 管理のバックグラウンド送信の effect ドライバ
/// （macOS #835・iOS #836 は共用実装、Android は兄弟 issue #837 が埋めるスタブ、他 OS は `Unsupported` 固定）。
pub mod background_upload;
/// Phase 11（端末機能の拡充ループ、親 #116、Issue #557）: 画像内のバーコード・QR 読み取りの effect ドライバ。
pub mod barcode_scan;
/// Phase 11 の拡充ループ（親 #153、Issue #1278）: ディープリンク受信の effect ドライバ
/// （Android 分岐はスタブ、他 OS は `UnsupportedDeepLink` 固定）。
pub mod deep_link;
/// Phase 11 の拡充ループ（親 #102、Issue #416）: ファイルの関連付けの effect ドライバ
/// （macOS・iOS・Windows 実装、Android は未実装のスタブ、他 OS は `Unsupported` 固定）。
pub mod file_association;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #1120、Issue #1250）: 画像の文字認識の effect ドライバ
/// （macOS・Windows / iOS 実装、Android は骨格のスタブ（対応表で `no`）、他 OS は
/// `UnsupportedTextRecognizer` 固定。Web は対象外）。
pub mod image_recognition;
#[cfg(target_os = "ios")]
pub mod ios;
/// Issue #425: iOS 向けアプリショートカット実装（Info.plist の静的宣言。`Declared` を返す）。
/// `device/app_shortcuts.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_app_shortcuts;
/// Issue #390: iOS 向けバックグラウンドダウンロード実装。
/// `device/background_download.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_background_download;
/// Issue #835（骨格）/ #836（実装）: iOS 向けバックグラウンド送信実装。
/// `device/background_upload.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_background_upload;
/// Issue #557: iOS 向けバーコード・QR 読み取りの骨格（実装は #558）。
#[cfg(target_os = "ios")]
pub mod ios_barcode_scan;
/// Issue #417: iOS 向けファイルの関連付け受信（macOS と共用の `apple_file_association` を返す）。
/// `device/file_association.rs::platform` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_file_association;
/// Issue #1252: iOS 向け文字認識（PHPicker + Vision）の effect ドライバ。
#[cfg(target_os = "ios")]
pub mod ios_image_recognition;
/// Issue #561: iOS 向け OS の検索への登録実装（macOS と共用の `apple_spotlight_index` へ委譲）。
/// `device/spotlight_index.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_spotlight_index;
/// Issue #532: iOS 向け Web 認証セッション実装（`ios/fandhe_ios_web_auth_session.m` の
/// `ASWebAuthenticationSession` を呼ぶ C ABI ラッパー）。`device/web_auth_session.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_web_auth_session;
/// Issue #424: macOS 向けアプリショートカット実装（`macos/fandhe_macos_app_shortcuts.m` の Dock メニューを呼ぶ
/// C ABI ラッパー）。`device/app_shortcuts.rs` の macOS 経路から呼ばれる。結果コードの写像をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_app_shortcuts;
/// Issue #530: macOS 向け Web 認証セッション実装（`macos/fandhe_macos_web_auth_session.m` の
/// `ASWebAuthenticationSession` を呼ぶ C ABI ラッパー）。`device/web_auth_session.rs` の macOS 経路から呼ばれる。
/// 写像の純関数をホストでテストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_web_auth_session;
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
/// Phase 11 の拡充ループ（親 #117、Issue #560）: OS の検索への登録の effect ドライバ（macOS・iOS・
/// Android 実装、他 OS は `UnsupportedSpotlightIndex` 固定）。
pub mod spotlight_index;
/// Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）: `device/
/// file_pick.rs` のライフサイクル処理（`PickHooks`/`drive_with`/`drive_mobile_with`）と
/// `bridge` を共有する effect ドライバ。
pub mod tsv_import;
pub mod tts;
/// Phase 11 の拡充ループ（親 #1116、Issue #1215）: カスタム URL scheme 受信の effect
/// ドライバ（macOS 実装、他 OS は `UnsupportedUrlScheme` 固定）。
pub mod url_scheme;
/// Phase 11 の拡充ループ（親 #169、Issue #821）: 動画の撮影の effect ドライバ
/// Phase 11 の拡充ループ（親 #113、Issue #530）: OAuth 用 Web 認証セッションの effect ドライバ（macOS・Windows・iOS・Android 実装、
/// 他 OS は `UnsupportedWebAuthSession` 固定。Web は対象外）。
pub mod web_auth_session;
/// Issue #424 / #428: Windows 向けアプリショートカット実装（ジャンプリストのタスク 1 件）。
/// `device/app_shortcuts.rs` の windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_app_shortcuts;
/// Issue #390: Windows 向けバックグラウンドダウンロード実装。
/// `device/background_download.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_background_download;
/// Issue #416: Windows 向けファイルの関連付け受信実装。
/// `device/file_association.rs` の Windows 経路と `main.rs` から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_file_association;
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
/// Issue #1216: Windows 向けカスタム URL scheme 受信（レジストリ登録・起動引数・
/// 名前付き mutex と named pipe による起動済みインスタンスへの転送）。
/// `device/url_scheme.rs` の Windows 経路と `main.rs` から使われる。純関数はホストの
/// `cargo test` で検査するため `cfg(target_os = "windows")` を付けず、Win32 呼び出しだけを
/// モジュール内に閉じる（`windows_background_task` と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_url_scheme;
/// Issue #530: Windows 向け Web 認証セッション実装。`device/web_auth_session.rs` の
/// Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_web_auth_session;

/// アプリ終了時に未移行の端末リソースを解放する（最終成果物（`fandhe-gallery`）の `register_teardown_hooks` 経由で `ProxyApp::on_exit` が呼ぶ契約、
/// 機能 crate の停止は gallery の終了フックが本関数の後に呼ぶ。停止の順序は「未移行 → 機能 crate」になるが、相互に独立している。
/// `fandhe-render-egui` の `app.rs` の `EguiApp::on_exit` 実装 doc 参照）。プロセスが終了すると
/// `static` の `Drop` は走らないため、生きている間に明示停止が要るリソースを
/// ここへ集約する。現状は TTS の子プロセス停止のみ（Issue #865 の codex-review
/// P1 指摘: 共有 TTS を `static OnceLock` で保持しているため `Drop` が実行されず、
/// ウィンドウを閉じても発話中の子プロセスが残る）。
pub fn shutdown() {
    tts::stop_shared_tts();
    web_auth_session::shutdown();
}
