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
/// Issue #1046: Android 向けバックグラウンド処理実装（`WorkManager`/
/// `OneTimeWorkRequest` を呼ぶ Java 側 `MainActivity` の JNI 受け口）。状態変換の
/// 純関数はホストでテストできるよう cfg を付けない（`fandhe-cap-gamepad` の `android.rs` と同じ
/// 判断）。`device/background_task.rs` の Android 経路から使われる。
pub mod android_background_task;
/// Issue #835（骨格）/ #837（実装）: Android 向けバックグラウンド送信実装。
/// `device/background_upload.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_background_upload;
/// Issue #1117: Android 向け車載ディスプレイ（Car App Library の Service/Screen を JNI 経由で操作する）。
/// `device/car_display.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_car_display;
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
/// Issue #1314: Android 向け全画面表示実装（`FullscreenHelper` との JNI 橋渡し）。
/// `device/fullscreen_immersive.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_fullscreen_immersive;
/// Issue #1099: Android 向けヘルスデータ（今日の歩数）の骨格（#1100 が Health Connect 実装に
/// 置き換える）。`device/health_data.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_health_data;
/// Issue #1250: Android 向け文字認識の骨格（#1253 が ML Kit 実装に置き換える）。
/// `device/image_recognition.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_image_recognition;
/// Issue #511: Android 向け近くの端末との直接通信実装。
/// `device/local_peer.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_local_peer;
/// Issue #485: Android 向け音楽ライブラリ実装（`MediaStore.Audio.Media` を JNI 経由で読む）。
/// `device/music_library.rs` の Android 経路から呼ばれる。純関数をホストでテストするため test でも載せる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_music_library;
/// Issue #1131（親 #114）: Android 向け決済シート（Google Pay API、TEST 環境）の実装。
/// `device/payment_request.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_payment_request;
/// Issue #355: Android 向けペン・筆圧入力実装。`device/pen_stylus.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_pen_stylus;
/// Issue #1254（骨格）/ #1256（実装）: Android 向け写真ライブラリ保存実装。
/// `device/photo_save.rs` の Android 経路から呼ばれる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_photo_library;
/// Issue #364: Android 向けピクチャ・イン・ピクチャ実装。
/// `device/picture_in_picture.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_picture_in_picture;
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
/// Issue #511 macOS: 近くの端末との直接通信実装（`apple/fandhe_apple_local_peer.m` を呼ぶ C ABI ラッパー）。
/// `device/local_peer.rs` の macOS 経路と、`ios_local_peer`（Issue #512）の委譲から呼ばれる。
/// `build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_local_peer;
/// Issue #954（macOS）/ #956（iOS）: iOS / macOS 向けマイク録音実装（`apple/
/// fandhe_apple_mic_record.m` を呼ぶ C ABI ラッパー）。`device/mic_record.rs` の
/// iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_mic_record;
/// Issue #1254（macOS）/ #1255（iOS）: iOS / macOS 向け写真ライブラリ保存実装
/// （`apple/fandhe_apple_photo_library.m` を呼ぶ C ABI ラッパー。iOS は実装済み）。
/// `device/photo_save.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_photo_library;
/// Issue #364（macOS）/ #365（iOS）: ピクチャ・イン・ピクチャ実装（`apple/fandhe_apple_picture_in_picture.m` の AVKit を
/// 呼ぶ C ABI ラッパー）。`device/picture_in_picture.rs` の macOS・iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_picture_in_picture;
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
/// Phase 11 の拡充ループ（親 #121、Issue #585）: ワンタイムコード入力の effect ドライバ（iOS は描画層の入力欄の橋渡し
/// （`HostView::set_text_input_hint`、実装は `ios_text_input.rs`）で実装、Android・macOS は保留、Windows は非対応で `UnsupportedAutofillOtp` 固定。
/// Web は `fandhe-web-dom` が担う）。
pub mod autofill_otp;
/// Phase 11 の拡充ループ（親 #100、Issue #390）: OS 管理のバックグラウンドダウンロードの effect ドライバ
/// （macOS・iOS・Android・Windows 実装、他 OS は `Unsupported` 固定）。
pub mod background_download;
/// Phase 11 の拡充ループ（親 #1042、Issue #1043）: バックグラウンド処理の effect
/// ドライバ（macOS 実装、他 OS は兄弟 issue #1044〜#1047）。
pub mod background_task;
/// Phase 11 の拡充ループ（親 #170、Issue #835）: OS 管理のバックグラウンド送信の effect ドライバ
/// （macOS #835・iOS #836 は共用実装、Android は兄弟 issue #837 が埋めるスタブ、他 OS は `Unsupported` 固定）。
pub mod background_upload;
/// Phase 11（端末機能の拡充ループ、親 #116、Issue #557）: 画像内のバーコード・QR 読み取りの effect ドライバ。
pub mod barcode_scan;
/// Phase 11 の拡充ループ（親 #152、Issue #1116）: 車載ディスプレイの effect ドライバ。
/// Android は Car App Library 実装（#1117）、他 OS は `UnsupportedCarDisplay` 固定。
pub mod car_display;
/// Phase 11 の拡充ループ（親 #153、Issue #1278）: ディープリンク受信の effect ドライバ
/// （Android 分岐はスタブ、他 OS は `UnsupportedDeepLink` 固定）。
pub mod deep_link;
/// Phase 11 の拡充ループ（親 #102、Issue #416）: ファイルの関連付けの effect ドライバ
/// （macOS・iOS・Windows 実装、Android は未実装のスタブ、他 OS は `Unsupported` 固定）。
pub mod file_association;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #1133、Issue #1312）: 全画面表示の effect ドライバ（macOS/Windows は
/// eframe の viewport コマンド、iOS/Android は OS 別モジュール、他 OS は
/// `UnsupportedFullscreen` 固定。Web は `fandhe-web-dom`）。
pub mod fullscreen_immersive;
/// Phase 11 の拡充ループ（親 #149、Issue #1089）: ヘルスデータ（今日の歩数）の effect ドライバ
/// （iOS・Android は OS 別モジュール。他 OS は `UnsupportedHealthData` 固定）。
pub mod health_data;
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
/// Issue #1045: iOS 向けバックグラウンド処理実装（`ios/
/// fandhe_ios_background_task.m` を呼ぶ C ABI ラッパー）。`device/
/// background_task.rs` の iOS 経路から呼ばれる。モジュール自体は `cfg(any(ios, test))`
/// で host の `cargo test` から状態機械を検証できる（モジュール doc 参照）。
#[cfg(any(target_os = "ios", test))]
pub mod ios_background_task;
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
/// Issue #1313: iOS 向け全画面表示実装（`ios/fandhe_ios_fullscreen_immersive.m` を呼ぶ
/// C ABI ラッパー）。
/// `device/fullscreen_immersive.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_fullscreen_immersive;
/// Issue #1089・#1090: iOS 向けヘルスデータ（歩数）実装（HealthKit の C ABI ラッパー）。
/// `device/health_data.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_health_data;
/// Issue #1252: iOS 向け文字認識（PHPicker + Vision）の effect ドライバ。
#[cfg(target_os = "ios")]
pub mod ios_image_recognition;
/// Issue #512: iOS 向け近くの端末との直接通信（親 #110、`apple_local_peer` へ委譲する）。
/// `device/local_peer.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_local_peer;
/// Issue #484: iOS 向け音楽ライブラリ実装（`ios/fandhe_ios_music_library.m` の MediaPlayer を呼ぶ
/// C ABI ラッパー）。`device/music_library.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_music_library;
/// Issue #356: iOS 向けペン・筆圧入力実装（`ios/fandhe_ios_pen_stylus.m` の観測専用
/// `UIGestureRecognizer` が値を置く C ABI ラッパー）。`device/pen_stylus.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_pen_stylus;
/// Issue #365: iOS 向けピクチャ・イン・ピクチャ実装（macOS と共用の `apple_picture_in_picture` へ委譲）。
/// `device/picture_in_picture.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_picture_in_picture;
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
/// Issue #561: iOS 向け OS の検索への登録実装（macOS と共用の `apple_spotlight_index` へ委譲）。
/// `device/spotlight_index.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_spotlight_index;
/// Issue #532: iOS 向け Web 認証セッション実装（`ios/fandhe_ios_web_auth_session.m` の
/// `ASWebAuthenticationSession` を呼ぶ C ABI ラッパー）。`device/web_auth_session.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_web_auth_session;
/// Phase 11 の拡充ループ（親 #110、Issue #511）: 近くの端末との直接通信の effect ドライバ
/// （macOS・iOS / Android 実装、他 OS は `UnsupportedLocalPeer` 固定）。
pub mod local_peer;
/// Issue #424: macOS 向けアプリショートカット実装（`macos/fandhe_macos_app_shortcuts.m` の Dock メニューを呼ぶ
/// C ABI ラッパー）。`device/app_shortcuts.rs` の macOS 経路から呼ばれる。結果コードの写像をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_app_shortcuts;
/// Issue #1043: macOS 向けバックグラウンド処理実装（`macos/
/// fandhe_macos_background_task.m` を呼ぶ C ABI ラッパー）。`device/
/// background_task.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_background_task;
/// Issue #482: macOS 向け音楽ライブラリ実装（`macos/fandhe_macos_music_library.m` の `ITLibrary` を
/// 呼ぶ C ABI ラッパー）。`device/music_library.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_music_library;
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
/// Issue #530: macOS 向け Web 認証セッション実装（`macos/fandhe_macos_web_auth_session.m` の
/// `ASWebAuthenticationSession` を呼ぶ C ABI ラッパー）。`device/web_auth_session.rs` の macOS 経路から呼ばれる。
/// 写像の純関数をホストでテストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_web_auth_session;
/// Issue #636: macOS 向けウィンドウ管理の読み取り実装（`macos/fandhe_macos_window_management.m` を呼ぶ
/// C ABI ラッパー）。`device/window_management.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_window_management;
/// Phase 11 の拡充ループ（親 #953、Issue #954・#955・#956・#957）: マイク録音の
/// effect ドライバ（macOS/iOS/Windows/Android 実装、他 OS は `UnsupportedMicRecorder`
/// 固定）。
pub mod mic_record;
/// Phase 11 の拡充ループ（親 #108、Issue #482）: 音楽ライブラリの effect ドライバ（macOS・iOS 実装、
/// Windows・Android は #483・#485 の担当、他 OS は `UnsupportedMusicLibrary` 固定。
/// Web は対象外）。
pub mod music_library;
/// Phase 11 の拡充ループ（親 #114、Issue #541・#1130）: 決済シートの effect ドライバ（Android は
/// pull 型 bridge、他のネイティブは `UnsupportedPaymentSheet` 固定の同期経路。dispatch は最終形。
/// Web は `fandhe-web-dom`）。
pub mod payment_request;
/// Phase 11 の拡充ループ（親 #95、Issue #355）: ペン・筆圧入力の effect ドライバ（macOS・iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedPenStylus` 固定。
/// Web は `fandhe-web-dom`）。
pub mod pen_stylus;
/// Phase 11 の拡充ループ（親 #1101、Issue #1103/#1104）: 写真選択の effect ドライバ（macOS /
/// Windows は `PhotoPicker` 実装、iOS/Android は pull 型 `bridge`、Linux は
/// `UnsupportedPhotoPicker` 固定。Web は `fandhe-web-dom`）。選んだ画像は復号を確かめてから
/// `image_store` 経由で描く。
pub mod photo_pick;
/// Phase 11 の拡充ループ（親 #1121、Issue #1254）: 写真の保存の effect ドライバ（macOS /
/// iOS / Android は pull 型 `bridge`、Windows は Pictures フォルダへ同期書き込み、
/// Linux は `UnsupportedPhotoLibrary` 固定。iOS / Android は兄弟 issue #1255 / #1256 まで非対応）。
pub mod photo_save;
/// Phase 11 の拡充ループ（親 #97、Issue #364）: ピクチャ・イン・ピクチャの effect ドライバ（macOS・iOS・Android 実装、
/// 他 OS は `UnsupportedPictureInPicture` 固定）。
pub mod picture_in_picture;
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
/// Phase 11 の拡充ループ（親 #127、Issue #636）: ウィンドウ管理（位置・サイズ・最前面）の effect ドライバ
/// （macOS 実装、Windows は #637、他 OS は `UnsupportedWindowManager` 固定。Web は `fandhe-web-dom`）。
pub mod window_management;
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
/// Issue #1044: Windows 向けバックグラウンド処理実装（Task Scheduler 2.0 COM
/// `ITaskService` ラッパー）。`device/background_task.rs` の Windows 経路から
/// 呼ばれる。HRESULT 写像・引数解析・時刻整形等の純関数はホストの `cargo test`
/// から検証できるよう `cfg(target_os = "windows")` を付けず、COM 呼び出しだけを
/// モジュール内の `cfg(target_os = "windows")` に閉じる（`speech_recognition.rs::
/// error_from_hresult` と同じ方針）。Windows 以外では利用者が無いため
/// `dead_code` を許容する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_background_task;
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
/// Issue #483: Windows 向け音楽ライブラリ実装（Music フォルダの走査 + プロパティストア）。
/// `device/music_library.rs` の Windows 経路から呼ばれる。走査の純関数をホストの `cargo test` で
/// 検査するため cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_music_library;
/// Issue #359: Windows 向けペン・筆圧入力実装（`SetWindowSubclass` の観測専用サブクラスが
/// `WM_POINTER*` から値を置く）。`device/pen_stylus.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_pen_stylus;
/// Issue #1254: Windows 向け写真保存実装（`SHGetKnownFolderPath(FOLDERID_Pictures)` で解決した
/// フォルダへ `FolderPhotoLibrary` で書く）。`device/photo_save.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_photo_library;
/// Issue #326: Windows 向け印刷実装（`PrintDlgExW` + GDI）。`device/print.rs` の Windows 経路から
/// 呼ばれる。結果の写像は純関数でホストでもテストできる（ffi 部分は `cfg(target_os = "windows")`）。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_print;
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
    local_peer::stop_shared_exchange();
    picture_in_picture::shutdown();
    web_auth_session::shutdown();
}
