//! Phase 8（端末機能、親 #752）の画面ごとの effect ドライバの置き場（Issue #746/#747/
//! #748/#749）。`fandhe-device` のトレイト（`Storage`/`Http`/`Camera`）を実際に呼び出し、
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
//! `screens/registry.rs` の `init "storage_note" => ..., crate::device::storage_note_state,`
//! 行から [`storage_note_state`] が呼ばれ、`generated_screens!` マクロが構築時に
//! 1 度だけ実行する（`screens.rs` の `init` 区画 doc 参照）。Issue #748（HTTP 取得）・
//! #749（カメラ撮影）・#830（傾きで玉が転がる）の effect ドライバはそれぞれ
//! [`http_fetch`] / [`camera_capture`] / [`tilt`] サブモジュールに置く
//! （`registry.rs` の `effect` 区画から配線される）。

/// Issue #1308: アクセシビリティ設定（視差軽減・コントラスト）の effect ドライバ（Windows / macOS / iOS / Android 実装、
/// 他 OS は `UnsupportedAccessibilityPrefs` 固定。Web は `fandhe-web-dom`）。
pub mod accessibility_prefs;
#[cfg(target_os = "android")]
pub mod android;
/// Issue #1310: Android 向けアクセシビリティ設定実装（`AccessibilityPrefsHelper` を JNI で呼ぶ）。
/// `device/accessibility_prefs.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_accessibility_prefs;
/// Issue #1362: Android 向けバッジ設定実装（`AppBadgeHelper` が無音の通知 1 件で代用）。`device/app_badge.rs` の android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_app_badge;
/// Issue #1330: Android 向け音声デバイス一覧実装のスタブ（#1332 が埋める）。`device/audio_devices.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_audio_devices;
/// Issue #390: Android 向けバックグラウンドダウンロードのスタブ（b-android #392 が埋める）。
/// `device/background_download.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_background_download;
/// Issue #1046: Android 向けバックグラウンド処理実装（`WorkManager`/
/// `OneTimeWorkRequest` を呼ぶ Java 側 `MainActivity` の JNI 受け口）。状態変換の
/// 純関数はホストでテストできるよう cfg を付けない（`android_gamepad.rs` と同じ
/// 判断）。`device/background_task.rs` の Android 経路から使われる。
pub mod android_background_task;
/// Issue #276: Android 向け通話 UI 統合実装のスタブ（#278 が埋める）。`device/call_ui.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_call_ui;
/// Issue #1404: Android 向け方位実装のスタブ（後続の (b-Android) が埋める）。`device/compass.rs` の android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_compass;
/// Issue #1384: Android 向けメール・SMS 作成画面実装（`ComposeHelper` の `ACTION_SENDTO` を JNI で呼ぶ）。`device/compose_mail_sms.rs` の android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_compose_mail_sms;
/// Issue #1304: Android 向け端末・OS 情報実装のスタブ（#1306 が埋める）。`device/device_info.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_device_info;
/// Issue #1410: Android 向けフォルダ変更監視実装のスタブ（#1413 が埋める）。`device/file_watch.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_file_watch;
/// Issue #1314: Android 向け全画面表示実装（`FullscreenHelper` との JNI 橋渡し）。
/// `device/fullscreen_immersive.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_fullscreen_immersive;
/// Issue #935: Android 向けゲームコントローラ実装（`InputDevice`/`MotionEvent`/`KeyEvent`
/// を橋渡しする Java 側 `MainActivity` の JNI 受け口）。状態機械本体はターゲット非依存
/// （ホストの `cargo test` で検証、モジュール doc 参照）だが、`Gamepad` トレイト実装・
/// JNI エクスポートは Android ターゲットのみでコンパイルされる（モジュール自体の cfg は
/// 付けない。`device/tilt.rs` と同じ判断）。`device/gamepad.rs` の Android 経路から使われる。
pub mod android_gamepad;
/// Issue #1067: ホーム画面ウィジェットの Android 実装（`AppWidgetProvider` + SharedPreferences
/// を呼ぶ Java 側 `MainActivity` の JNI 受け口）。`device/home_widget.rs` の Android 経路から
/// 使われる。写像の純関数はホストでテストできるよう cfg は `any(android, test)`。
#[cfg(any(target_os = "android", test))]
pub mod android_home_widget;
/// Issue #1250: Android 向け文字認識の骨格（#1253 が ML Kit 実装に置き換える）。
/// `device/image_recognition.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_image_recognition;
/// Issue #294: Android 向けライブアクティビティ（進行中通知）実装のスタブ（b-android が埋める）。
/// `device/live_activity.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_live_activity;
/// Issue #1376: Android 向け OS 搭載フォント一覧実装のスタブ（#1378 が埋める）。`device/local_fonts.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_local_fonts;
/// Issue #267: Android 向け音声再生実装のスタブ（親 #86 の b-android が埋める）。`device/media_playback.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_media_playback;
/// Issue #311: Android 向け音声変換のスタブ（b-android の子が MediaCodec + MediaMuxer で埋める）。
/// `device/media_transcode.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_media_transcode;
/// Issue #1398: Android 向け MIDI 入力実装のスタブ（#1400 が埋める）。`device/midi.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_midi;
/// Issue #289: Android 向け機械学習推論のスタブ（b-android が NNAPI 実装で埋める）。
/// `device/ml_inference.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_ml_inference;
/// Issue #1343: Android 向け複数ディスプレイ実装（`DisplayManager` を JNI で読む）。
/// `device/multi_display.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_multi_display;
/// Issue #1284: Android 向けネットワーク接続状態実装（`ConnectivityManager` を JNI で読む）。
/// `device/network_status.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_network_status;
/// Issue #1247: Android 向け歩数実装のスタブ（#1249 が埋める）。`device/pedometer.rs` の
/// Android 経路から呼ばれる。JNI の本体はモジュール内側の `cfg(target_os = "android")` に閉じ、
/// 純関数をホストでテストできるよう cfg は `any(android, test)`。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_pedometer;
/// Issue #355: Android 向けペン・筆圧入力実装のスタブ（#357 が埋める）。`device/pen_stylus.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_pen_stylus;
/// Issue #1254（骨格）/ #1256（実装）: Android 向け写真ライブラリ保存。#1256 までスタブ。
/// `device/photo_save.rs` の Android 経路から呼ばれる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_photo_library;
/// Issue #364: Android 向けピクチャ・イン・ピクチャ実装のスタブ（親 #97 の b-android が埋める）。
/// `device/picture_in_picture.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_picture_in_picture;
/// Issue #1274: Android 向け電源状態実装のスタブ（#1276 が埋める）。`device/power_state.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_power_state;
/// Issue #328: Android 向け印刷（`PrintHelper` + JNI）。`device/print.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_print;
/// Issue #1335: Android 向け画面録画のスタブ（#1337 が埋める）。`device/screen_capture.rs` の
/// Android 経路から使われる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_screen_capture;
/// Issue #1280: Android 向け画面スリープ抑止実装（`FLAG_KEEP_SCREEN_ON`、Java は `ScreenWakeLockHelper`）。
/// `device/screen_wake_lock.rs` の Android 経路から使われる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_screen_wake_lock;
/// Issue #1218: Android 向けカスタム URL scheme 受信実装（Java 側 `MainActivity` の
/// `onCreate` / `onNewIntent` から JNI で呼ばれる）。`device/url_scheme.rs::
/// platform_url_scheme` の Android 経路から使われる。
#[cfg(target_os = "android")]
pub mod android_url_scheme;
/// Phase 11（親 #1141、Issue #1360）: アプリアイコンのバッジ（Windows はタスクバーのオーバーレイ数字）の effect ドライバ（macOS /
/// Windows は同期、iOS / Android は pull 型 `bridge`（実装は #1361 / #1362 まで `Unsupported` スタブ）、
/// 他 OS は `UnsupportedAppBadge` 固定。Web は `fandhe-web-dom`）。
pub mod app_badge;
/// Issue #1308: macOS / iOS 向けアクセシビリティ設定（`apple/fandhe_apple_accessibility_prefs.m` を
/// 呼ぶ C ABI ラッパー）。`device/accessibility_prefs.rs` の macOS / iOS 経路から呼ばれる。
/// 純関数をホストの `cargo test` で検査するため cfg に `test` を含める。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub mod apple_accessibility_prefs;
/// Issue #390（macOS）/ #391（iOS）: バックグラウンドダウンロード実装（`apple/fandhe_apple_background_download.m` の
/// background `NSURLSession` を呼ぶ C ABI ラッパー）。`device/background_download.rs` の macOS 経路から呼ばれる。
/// 状態遷移の純ロジックをホストの `cargo test` で検査するため cfg に `test` を含める。
/// iOS は b-ios が委譲するまで未使用。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub mod apple_background_download;
/// Issue #906: iOS / macOS 向け生体認証実装（`apple/fandhe_apple_biometric.m` を呼ぶ
/// C ABI ラッパー）。`device/biometric_auth.rs` の macOS/iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_biometric;
/// Issue #1060 macOS + #1061 iOS: BLE スキャン実装（`apple/fandhe_apple_bluetooth_le.m` を
/// 呼ぶ C ABI ラッパー）。`device/bluetooth_le.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_bluetooth_le;
/// Issue #1224 macOS + #1225 iOS: 予定追加実装（`apple/fandhe_apple_calendar.m` を呼ぶ
/// C ABI ラッパー）。`device/calendar.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_calendar;
/// Issue #1220 macOS + #1221 iOS: 連絡先選択実装（`apple/fandhe_apple_contact_picker.m` を
/// 呼ぶ C ABI ラッパー）。`device/contact_pick.rs` の Apple 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_contact_picker;
/// Issue #934: iOS / macOS 共通のゲームコントローラ実装（`apple/
/// fandhe_apple_gamepad.m` を呼ぶ C ABI ラッパー）。`device/gamepad.rs` の
/// Apple cfg 分岐から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_gamepad;
/// Issue #1091・#1092: macOS / iOS 向け触覚フィードバック実装（`apple/fandhe_apple_haptics.m`
/// を呼ぶ C ABI ラッパー）。`device/haptics.rs` の Apple 経路から呼ばれる。`build.rs` の
/// コンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_haptics;
/// Issue #1065（macOS）: ホーム画面ウィジェットの Apple 向け実装（App Group コンテナ
/// のファイル + ホストアプリ経由の再描画要求）。`device/home_widget.rs` の macOS 経路から
/// 呼ばれる。ホストの `cargo test` でも検証できるよう `cfg(test)` でも有効にする。
#[cfg(any(target_os = "macos", test))]
pub mod apple_home_widget;
/// Issue #1250 macOS・#1252 iOS: 文字認識実装（`apple/
/// fandhe_apple_image_recognition.m` を呼ぶ C ABI ラッパー）。`device/image_recognition.rs` の
/// macOS 経路と `ios_image_recognition` から呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_image_recognition;
/// Issue #889: iOS / macOS 共通の CoreLocation 実装（`apple/fandhe_apple_location.m`
/// を呼ぶ C ABI ラッパー）。`device/location.rs` の Apple cfg 分岐から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_location;
/// Issue #273（macOS）/ #274（iOS）: macOS / iOS 向け地図スナップショット実装
/// （`apple/fandhe_apple_map_view.m` を呼ぶ C ABI ラッパー。iOS も同じ実装を共用）。
/// `device/map_view.rs` の macOS / iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_map_view;
/// Issue #267: macOS 向け音声再生実装（`apple/fandhe_apple_media_playback.m` の `AVAudioPlayer` を呼ぶ
/// C ABI ラッパー）。`device/media_playback.rs` の macOS 経路から呼ばれる。iOS は親 #86 の b-ios が使う。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_media_playback;
/// Issue #311: macOS / iOS 向け音声変換実装（`apple/fandhe_apple_media_transcode.m` の `AVAudioFile` を
/// 呼ぶ C ABI ラッパー）。`device/media_transcode.rs` の macOS 経路と `ios_media_transcode`（Issue #313）から
/// 呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_media_transcode;
/// Issue #954（macOS）/ #956（iOS）: iOS / macOS 向けマイク録音実装（`apple/
/// fandhe_apple_mic_record.m` を呼ぶ C ABI ラッパー）。`device/mic_record.rs` の
/// iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_mic_record;
/// Issue #1398: iOS / macOS 共通の MIDI 入力実装（`apple/fandhe_apple_midi.m` を呼ぶ
/// C ABI ラッパー。iOS は 14 以上、未満は失敗コードを返す）。`device/midi.rs` の Apple 分岐から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_midi;
/// Issue #289 macOS・b-ios iOS: Core ML 推論実装（`apple/fandhe_apple_ml_inference.m` を呼ぶ
/// C ABI ラッパー）。`device/ml_inference.rs` の macOS 経路と、iOS 経路（`ios_ml_inference` 経由）から
/// 呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_ml_inference;
/// Issue #1282: macOS / iOS 向けネットワーク接続状態（`apple/fandhe_apple_network_status.m` を
/// 呼ぶ C ABI ラッパー）。`device/network_status.rs` の macOS / iOS 経路から呼ばれる。
/// 純関数をホストの `cargo test` で検査するため cfg に `test` を含める（FFI 部分は macOS 限定）。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub mod apple_network_status;
/// Issue #1094 macOS + #1095 iOS: 単語分割実装（`apple/fandhe_apple_nlp.m` を呼ぶ C ABI
/// ラッパー）。`device/nlp.rs` の macOS / iOS 経路から呼ばれる。`build.rs` のコンパイル条件と
/// 同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_nlp;
/// Issue #345 macOS・#346 iOS: 端末内 LLM 実装（`apple/fandhe_apple_on_device_llm.swift` を
/// 呼ぶ C ABI ラッパー）。`device/on_device_llm.rs` の macOS 経路と、iOS 経路（`ios_on_device_llm` 経由）から
/// 呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_on_device_llm;
/// Issue #1254（macOS）/ #1255（iOS）: iOS / macOS 向け写真ライブラリ保存実装
/// （`apple/fandhe_apple_photo_library.m` を呼ぶ C ABI ラッパー。iOS は #1255 までスタブ）。
/// `device/photo_save.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_photo_library;
/// Issue #364（macOS）/ #365（iOS）: ピクチャ・イン・ピクチャ実装（`apple/fandhe_apple_picture_in_picture.m` の AVKit を
/// 呼ぶ C ABI ラッパー）。`device/picture_in_picture.rs` の macOS・iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_picture_in_picture;
/// Issue #1335（macOS）/ #1336（iOS）: macOS 向け ScreenCaptureKit 画面録画（`apple/
/// fandhe_apple_screen_capture.m` を呼ぶ C ABI ラッパー。iOS は #1336 の ReplayKit）。
/// `device/screen_capture.rs` の macOS / iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_screen_capture;
/// Issue #922・#924: iOS / macOS 向け資格情報保存実装（`apple/
/// fandhe_apple_secure_credential.m` を呼ぶ C ABI ラッパー）。`device/
/// secure_credential.rs` の iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_secure_credential;
/// Issue #897（macOS）/ #899（iOS 横展開）: iOS / macOS 向け共有シート実装
/// （`apple/fandhe_apple_share.m` を呼ぶ C ABI ラッパー）。`device/share.rs` の
/// macOS/iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_share;
/// Issue #1024（macOS）/ #1025（iOS）: iOS / macOS 向け音声認識実装（`apple/
/// fandhe_apple_speech_recognition.m` を呼ぶ C ABI ラッパー）。`device/
/// speech_recognition.rs` の iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_speech_recognition;
/// Issue #1039: macOS 向けシステムトレイ実装（`apple/fandhe_apple_system_tray.m` を
/// 呼ぶ C ABI ラッパー）。`device/system_tray.rs::platform_tray` の macOS 経路から
/// 呼ばれる（iOS は対応表 `system-tray.json` で `n-a` のため対象外）。
#[cfg(target_os = "macos")]
pub mod apple_system_tray;
/// Issue #1215・#1217: macOS / iOS 向けカスタム URL scheme 受信（`macos/fandhe_macos_url_scheme.m`
/// の Apple Event ハンドラ、`ios/fandhe_ios_url_scheme.m` の delegate から呼ばれる C ABI
/// ラッパー）。`device/url_scheme.rs::platform_url_scheme` の macOS・iOS 経路と
/// `main.rs`（登録）から使われる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_url_scheme;
/// Issue #307（親 #90）: AR 表示の effect ドライバ（iOS は (b-ios) が `ios_ar.rs` を埋めるまでスタブ、
/// 他 OS は `UnsupportedAr` 固定。Web は `fandhe-web-dom`）。
pub mod ar;
/// Phase 11 の拡充ループ（親 #1136、Issue #1330）: 音声デバイス一覧の effect ドライバ（macOS / Windows
/// 実装、iOS・Android は #1331・#1332 で埋めるスタブ、他 OS は `UnsupportedAudioDevices` 固定）。
pub mod audio_devices;
/// Phase 11 の拡充ループ（親 #100、Issue #390）: OS 管理のバックグラウンドダウンロードの effect ドライバ
/// （macOS 実装、iOS・Android・Windows は子 issue が埋めるスタブ、他 OS は `Unsupported` 固定）。
pub mod background_download;
/// Phase 11 の拡充ループ（親 #1042、Issue #1043）: バックグラウンド処理の effect
/// ドライバ（macOS 実装、他 OS は兄弟 issue #1044〜#1047）。
pub mod background_task;
/// Phase 11 の拡充ループ（親 #905、Issue #906/#907/#908）: 生体認証の effect ドライバ
/// （macOS/iOS/Android/Windows 実装）。
pub mod biometric_auth;
/// Phase 11 の拡充ループ（親 #1059、Issue #1060）: BLE スキャンの effect ドライバ
/// （iOS/macOS/Windows/Android 実装、他 OS は `UnsupportedBluetoothLe` 固定。Web は別経路 `fandhe-web-dom`（#1063））。
pub mod bluetooth_le;
/// Issue #1224: 予定を 1 件追加する型サンプル「予定の追加」の effect ドライバ
/// （macOS は EventKit・Android は `Intent.ACTION_INSERT`（#1226）の pull 型 `bridge`、
/// それ以外は非対応表示）。
pub mod calendar;
/// Phase 11 の拡充ループ（親 #87、Issue #276）: 通話 UI 統合の effect ドライバ（iOS・Android は
/// #277・#278 で埋めるスタブ、他 OS は `UnsupportedCallUi` 固定）。
pub mod call_ui;
pub mod camera_capture;
pub mod clipboard;
/// Issue #1404（親 #1154）: 方位（コンパス）の effect ドライバ（Windows 実装、iOS・Android は後続の子が実装するまでスタブ、
/// 他 OS は `UnsupportedCompass` 固定。Web は `fandhe-web-dom`）。
pub mod compass;
/// Phase 11（親 #1142、Issue #1384）: メール・SMS 作成画面の effect ドライバ（macOS / Windows は同期、
/// iOS / Android は pull 型 `bridge`（実装は b-ios / b-android まで `Unsupported` スタブ）、他 OS は
/// `UnsupportedComposeMailSms` 固定。Web は `fandhe-web-dom`）。
pub mod compose_mail_sms;
/// Phase 11 の拡充ループ（親 #1117、Issue #1220・#1221・#1222）: 連絡先選択の effect ドライバ
/// （macOS / iOS / Android は pull 型 `bridge`、他 OS は兄弟 issue が入るまで
/// `UnsupportedContactPicker` 固定）。
pub mod contact_pick;
/// Phase 11 の拡充ループ（親 #1131、Issue #1304）: 端末・OS 情報の effect ドライバ（macOS / Windows
/// 実装、iOS・Android は #1305・#1306 で埋めるスタブ、他 OS は `UnsupportedDeviceInfo` 固定）。
pub mod device_info;
/// Phase 11 の拡充ループ（親 #1013、Issue #1014・#1015）: ドラッグ & ドロップ受け取りの
/// effect ドライバ（macOS/Windows は egui `RawInput::hovered_files`/`dropped_files` を
/// 橋渡し、iOS は [`drop_session`]/[`ios_drag_drop`] 経由で `UIDropInteraction` を橋渡し）。
pub mod drag_drop;
/// ドラッグ & ドロップ（[`drag_drop`]）の集約状態機械。ターゲット非依存
/// （`drop_session.rs` モジュール doc 参照）。iOS 経路（[`ios_drag_drop`]）専用の
/// 入力元だが、ロジック自体はホストの `cargo test` から検証できるよう cfg で
/// ゲートしない（`android_gamepad.rs` と同じ判断）。
pub mod drop_session;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #1143、Issue #1410）: フォルダの変更監視の effect ドライバ（macOS 実装、
/// Windows・iOS・Android は #1411〜#1413 で埋めるスタブ、他 OS は `UnsupportedFileWatch` 固定）。
pub mod file_watch;
/// Phase 11 の拡充ループ（親 #1133、Issue #1312）: 全画面表示の effect ドライバ（macOS/Windows は
/// eframe の viewport コマンド、iOS/Android は #1313/#1314 で埋めるスタブ、他 OS は
/// `UnsupportedFullscreen` 固定。Web は `fandhe-web-dom`）。
pub mod fullscreen_immersive;
/// Phase 11 の拡充ループ（親 #932、Issue #933・#934・#935）: ゲームコントローラの effect
/// ドライバ（Windows / iOS / macOS / Android 実装）。
pub mod gamepad;
/// Phase 11 の拡充ループ（親 #1056、Issue #1057）: ジオフェンスの effect ドライバ
/// （location の継続取得に乗せて `GeofenceTracker` で判定する）。
pub mod geofence;
/// Phase 11 の拡充ループ（親 #1090、Issue #1091）: 触覚フィードバックの effect ドライバ
/// （macOS/Windows 実装、他 OS は `UnsupportedHaptics` 固定。Web は `fandhe-web-dom`、
/// iOS/Android は #1092）。
pub mod haptics;
/// Phase 11 の拡充ループ（親 #1064、Issue #1065）: ホーム画面ウィジェットの effect
/// ドライバ（macOS 実装、iOS・Android は #1066・#1067）。
pub mod home_widget;
pub mod http_fetch;
/// Phase 11 の拡充ループ（親 #1120、Issue #1250）: 画像の文字認識の effect ドライバ
/// （macOS 実装、Windows / iOS / Android は #1251〜#1253 が埋めるスタブ、他 OS は
/// `UnsupportedTextRecognizer` 固定。Web は対象外）。
pub mod image_recognition;
#[cfg(target_os = "ios")]
pub mod ios;
/// Issue #1308: iOS 向けアクセシビリティ設定実装（#1309。`AppleAccessibilityPrefs` を返す）。
/// `device/accessibility_prefs.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_accessibility_prefs;
/// Issue #1361: iOS 向けバッジ設定実装（`ios/fandhe_ios_app_badge.m` を呼ぶ C ABI ラッパー）。`device/app_badge.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_app_badge;
/// Issue #307: iOS 向け AR 実装のスタブ（(b-ios) が ARKit で埋める）。`device/ar.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_ar;
/// Issue #1331: iOS 向け音声デバイス一覧実装（`ios/fandhe_ios_audio_devices.m` を呼ぶ C ABI ラッパー）。
/// `device/audio_devices.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_audio_devices;
/// Issue #390: iOS 向けバックグラウンドダウンロードのスタブ（b-ios #391 が埋める）。
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
/// Issue #277: iOS 向け通話 UI 統合実装（`ios/fandhe_ios_call_ui.m` の CallKit を呼ぶ C ABI ラッパー）。
/// `device/call_ui.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_call_ui;
/// Issue #1405: iOS 向け方位実装（`ios/fandhe_ios_compass.m` を呼ぶ C ABI ラッパー、CoreLocation の heading）。`device/compass.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_compass;
/// Issue #1385: iOS 向けメール・SMS 作成画面実装（`ios/fandhe_ios_compose_mail_sms.m` を呼ぶ C ABI ラッパー、MessageUI）。`device/compose_mail_sms.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_compose_mail_sms;
/// Issue #1305: iOS 向け端末・OS 情報実装（`ios/fandhe_ios_device_info.m` を呼ぶ C ABI ラッパー。
/// #1304 のスタブを置き換え）。`device/device_info.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_device_info;
/// Issue #1015: iOS 向けドラッグ & ドロップ受け取り実装（`ios/fandhe_ios_drag_drop.m`
/// を呼ぶ C ABI ラッパー）。`device/drag_drop.rs::drag_drop_effect` の iOS 経路から
/// 呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_drag_drop;
/// Issue #1410: iOS 向けフォルダ変更監視実装のスタブ（#1412 が埋める）。`device/file_watch.rs` の
/// iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_file_watch;
/// Issue #1313: iOS 向け全画面表示実装（`ios/fandhe_ios_fullscreen_immersive.m` を呼ぶ
/// C ABI ラッパー）。
/// `device/fullscreen_immersive.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_fullscreen_immersive;
/// Issue #1252: iOS 向け文字認識（PHPicker + Vision）の effect ドライバ。
#[cfg(target_os = "ios")]
pub mod ios_image_recognition;
/// Issue #294: iOS 向けライブアクティビティ（ActivityKit）実装のスタブ（b-ios が埋める）。
/// `device/live_activity.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_live_activity;
/// Issue #1377: iOS 向け OS 搭載フォント一覧実装（`ios/fandhe_ios_local_fonts.m`、UIKit
/// `UIFont.familyNames`）。`device/local_fonts.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_local_fonts;
/// Issue #267: iOS 向け音声再生実装のスタブ（親 #86 の b-ios が埋める）。`device/media_playback.rs` の
/// iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_media_playback;
/// Issue #313: iOS 向け音声変換（macOS と共用の `AppleMediaTranscode` を返す）。
/// `device/media_transcode.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_media_transcode;
/// Issue #291: iOS 向け機械学習推論（macOS と共用の `AppleMlInference` を返す）。
/// `device/ml_inference.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_ml_inference;
/// Issue #27: iOS 向け複数ディスプレイ実装（`ios/fandhe_ios_multi_display.m`、scene の `UIScreen`）。
/// `device/multi_display.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_multi_display;
/// Issue #1283: iOS 向けネットワーク接続状態実装（`apple_network_status` を返す）。
/// `device/network_status.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_network_status;
/// Issue #1005: iOS 向けローカル通知実装（`ios/fandhe_ios_notification.m` を呼ぶ
/// C ABI ラッパー）。`device/notification.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_notification;
/// Issue #346: iOS 向け端末内 LLM 実装（macOS と共用の `apple_on_device_llm` を返す）。
/// `device/on_device_llm.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_on_device_llm;
/// Issue #1248: iOS 向け歩数実装（#1247 のスタブを置き換え）。`device/pedometer.rs` の iOS
/// 経路から呼ばれる。FFI の本体はモジュール内側の `cfg(target_os = "ios")` に閉じ、
/// 純関数をホストでテストできるよう cfg は `any(ios, test)`。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_pedometer;
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
/// Issue #1275: iOS 向け電源状態実装（`ios/fandhe_ios_power_state.m` を呼ぶ C ABI ラッパー。
/// #1274 のスタブを置き換え）。`device/power_state.rs` の iOS 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(ios, test)`。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_power_state;
/// Issue #327: iOS 向け印刷実装（`ios/fandhe_ios_print.m` を呼ぶ C ABI ラッパー、`UIPrintInteractionController`）。
/// `device/print.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_print;
/// Issue #1279: iOS 向け画面スリープ抑止実装（`ios/fandhe_ios_screen_wake_lock.m` を呼ぶ C ABI
/// ラッパー。#1278 のスタブを置き換え）。
/// `device/screen_wake_lock.rs` の iOS 経路から使われる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_screen_wake_lock;
/// Phase 11 の拡充ループ（親 #88、Issue #294）: ライブアクティビティの effect ドライバ（iOS・Android は
/// b 層が埋めるスタブ、他 OS は `UnsupportedLiveActivity` 固定）。
pub mod live_activity;
/// Phase 11 の拡充ループ（親 #1145、Issue #1376）: OS 搭載フォント一覧の effect ドライバ（macOS / Windows
/// 実装、iOS・Android は #1377・#1378 で埋めるスタブ、他 OS は `UnsupportedLocalFonts` 固定）。
pub mod local_fonts;
/// Phase 11 の拡充ループ（親 #887、Issue #888）: 位置情報の effect ドライバ（Windows のみ実装）。
pub mod location;
/// Issue #1360: macOS 向けバッジ設定実装（`macos/fandhe_macos_app_badge.m` を呼ぶ C ABI ラッパー）。`device/app_badge.rs` の macos 経路から呼ばれる。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_app_badge;
/// Issue #1330: macOS 向け音声デバイス一覧実装（`macos/fandhe_macos_audio_devices.m` を呼ぶ C ABI
/// ラッパー）。`device/audio_devices.rs` の macOS 経路から呼ばれる。写像の純関数をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_audio_devices;
/// Issue #1043: macOS 向けバックグラウンド処理実装（`macos/
/// fandhe_macos_background_task.m` を呼ぶ C ABI ラッパー）。`device/
/// background_task.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_background_task;
/// Issue #1384: macOS 向けメール作成・SMS 作成画面実装（`macos/fandhe_macos_compose_mail_sms.m` を呼ぶ C ABI ラッパー）。`device/compose_mail_sms.rs` の macos 経路から呼ばれる。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_compose_mail_sms;
/// Issue #1304: macOS 向け端末・OS 情報実装（`macos/fandhe_macos_device_info.m` を呼ぶ C ABI
/// ラッパー）。`device/device_info.rs` の macOS 経路から呼ばれる。写像の純関数をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_device_info;
/// Issue #1410: macOS 向けフォルダ変更監視実装（`macos/fandhe_macos_file_watch.m`、FSEvents を呼ぶ
/// C ABI ラッパー）。`device/file_watch.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_file_watch;
/// Issue #1376: macOS 向け OS 搭載フォント一覧実装（`macos/fandhe_macos_local_fonts.m` を呼ぶ C ABI
/// ラッパー）。`device/local_fonts.rs` の macOS 経路から呼ばれる。バッファ解釈の純関数をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_local_fonts;
/// Issue #1341: macOS 向け複数ディスプレイ実装（`macos/fandhe_macos_multi_display.m` を呼ぶ C ABI
/// ラッパー）。`device/multi_display.rs` の macOS 経路から呼ばれる。写像の純関数をホストで
/// テストできるよう cfg は `any(macos, test)`。写像の純関数は iOS 実装（`ios_multi_display`）と
/// 共用するため iOS でもコンパイルする。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub mod macos_multi_display;
/// Issue #355: macOS 向けペン・筆圧入力実装（`macos/fandhe_macos_pen_stylus.m` の `NSEvent` local
/// monitor を呼ぶ C ABI ラッパー）。`device/pen_stylus.rs` の macOS 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_pen_stylus;
/// Issue #1274: macOS 向け電源状態実装（`macos/fandhe_macos_power_state.m` を呼ぶ C ABI ラッパー）。
/// `device/power_state.rs` の macOS 経路から呼ばれる。写像の純関数をホストでテストできるよう
/// cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_power_state;
/// Issue #325: macOS 向け印刷実装（`macos/fandhe_macos_print.m` を呼ぶ C ABI ラッパー）。
/// `device/print.rs` の macOS 経路から呼ばれる。結果コードの写像をホストでテストできるよう
/// cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_print;
/// Issue #1278: macOS 向け画面スリープ抑止実装（`macos/fandhe_macos_screen_wake_lock.m` を
/// 呼ぶ C ABI ラッパー）。`device/screen_wake_lock.rs` の macOS 経路から呼ばれる。状態管理
/// （`Holder`）はホストのテストで検証できるよう `test` でもコンパイルする。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
pub mod macos_screen_wake_lock;
/// Phase 11 の拡充ループ（親 #85、Issue #273）: 地図表示の effect ドライバ（macOS / iOS は
/// pull 型 `bridge`、他 OS は `UnsupportedMapSnapshotter` 固定）。
pub mod map_view;
/// Phase 11 の拡充ループ（親 #86、Issue #267）: 音声再生の effect ドライバ（macOS 実装、iOS・Android・Windows は
/// 親 #86 の子で埋めるスタブ、他 OS は `UnsupportedMediaPlayback` 固定）。
pub mod media_playback;
/// Phase 11 の拡充ループ（親 #91、Issue #311）: 音声変換（AAC）の effect ドライバ（macOS 実装、
/// iOS / Windows / Android は子 issue が埋めるスタブ、他 OS は `UnsupportedMediaTranscode` 固定。
/// Web は `fandhe-web-dom`）。
pub mod media_transcode;
/// Phase 11 の拡充ループ（親 #953、Issue #954・#955・#956・#957）: マイク録音の
/// effect ドライバ（macOS/iOS/Windows/Android 実装、他 OS は `UnsupportedMicRecorder`
/// 固定）。
pub mod mic_record;
/// Phase 11 の拡充ループ（親 #1137、Issue #1398）: MIDI 入力の effect ドライバ（macOS・iOS 実装、
/// Android・Windows は #1400・#1402 で埋めるスタブ、他 OS は `UnsupportedMidiInput` 固定）。
pub mod midi;
/// Phase 11 の拡充ループ（親 #89、Issue #289）: 機械学習モデル推論の effect ドライバ（macOS 実装、
/// iOS / Windows / Android は b-ios・windows・b-android の子 issue が埋めるスタブ、他 OS は
/// `UnsupportedMlInference` 固定。Web は対象外）。
pub mod ml_inference;
/// Phase 11 の拡充ループ（親 #1134、Issue #1341）: 複数ディスプレイの effect ドライバ（macOS / Windows
/// 実装、Android は #1343 で埋めるスタブ、他 OS は `UnsupportedMultiDisplay` 固定）。
pub mod multi_display;
/// Phase 11 の拡充ループ（親 #1130、Issue #1282）: ネットワーク接続状態の effect ドライバ
/// （Windows / macOS / iOS 実装、Android は #1284、他 OS は
/// `UnsupportedNetworkStatus` 固定。Web は `fandhe-web-dom`）。
pub mod network_status;
/// Phase 11 の拡充ループ（親 #1093、Issue #1094）: 単語分割の effect ドライバ
/// （macOS / iOS / Windows / Android 実装、他 OS は `UnsupportedWordSegmenter` 固定。Web は
/// `fandhe-web-dom`）。
pub mod nlp;
/// Phase 11 の拡充ループ（親 #1003、Issue #1004/#1005）: ローカル通知の effect
/// ドライバ（macOS/Windows/iOS 実装、Android は #1006）。
pub mod notification;
/// Phase 11 の拡充ループ（親 #93、Issue #345）: 端末内 LLM の effect ドライバ（macOS 実装、iOS は
/// b-ios が埋めるスタブ、他 OS は `UnsupportedOnDeviceLlm` 固定。Web は `fandhe-web-dom` が持つ）。
pub mod on_device_llm;
/// Phase 11 の拡充ループ（親 #92、Issue #342）: パスキーの effect ドライバ（Windows はワーカー +
/// pull 型 bridge、実装は #343 が埋める。他 OS は `UnsupportedPasskey` 固定。Web は `fandhe-web-dom`）。
pub mod passkey;
/// Phase 11 の拡充ループ（親 #1119、Issue #1247）: 歩数の effect ドライバ（Windows 実装、
/// iOS・Android は #1248・#1249 で埋めるスタブ、他 OS は `UnsupportedPedometer` 固定）。
pub mod pedometer;
/// Phase 11 の拡充ループ（親 #95、Issue #355）: ペン・筆圧入力の effect ドライバ（macOS 実装、
/// iOS・Android・Windows は #356・#357・#359 で埋めるスタブ、他 OS は `UnsupportedPenStylus` 固定。
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
/// Phase 11 の拡充ループ（親 #97、Issue #364）: ピクチャ・イン・ピクチャの effect ドライバ（macOS 実装、
/// iOS・Android は子 issue が埋めるスタブ、他 OS は `UnsupportedPictureInPicture` 固定）。
pub mod picture_in_picture;
/// Phase 11 の拡充ループ（親 #1128、Issue #1274）: 電源状態の effect ドライバ（macOS / Windows
/// ・iOS 実装、Android は #1276 で埋めるスタブ、他 OS は `UnsupportedPowerState` 固定）。
pub mod power_state;
/// Phase 11 の拡充ループ（親 #94、Issue #325）: 印刷ダイアログの effect ドライバ（macOS 実装、
/// Android は #328 で実装済み、Windows / iOS は #326・#327 が埋めるスタブ、他 OS は `UnsupportedPrint` 固定。
/// Web は `fandhe-web-dom`）。
pub mod print;
/// request/response 型端末機能共通の要求追跡（Issue #943、親 #941）。share /
/// biometric_auth / file_pick の `bridge` facade から使う。4 OS
/// （macOS/iOS/Android/Windows）以外では利用者が無いため `dead_code` を許容する。
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    allow(dead_code)
)]
pub(crate) mod request_slot;
/// Phase 11 の拡充ループ（親 #1135、Issue #1335）: 画面の録画・キャプチャの effect ドライバ
/// （macOS 実装、iOS は #1336 で実装済み・Android は #1337 で埋めるスタブ、他 OS は
/// `UnsupportedScreenCapturer` 固定。Web は `fandhe-web-dom`）。
pub mod screen_capture;
/// Phase 11 の拡充ループ（親 #1129、Issue #1278）: 画面スリープ抑止の effect ドライバ
/// （macOS/Windows/iOS 実装、Android は #1280 で埋めるスタブ、他 OS は
/// `UnsupportedScreenWakeLock` 固定。Web は `fandhe-web-dom`）。
pub mod screen_wake_lock;
/// Phase 11 の拡充ループ（親 #921、Issue #922・#924）: 資格情報保存の effect
/// ドライバ（macOS/iOS/Android 実装、Windows は #923）。
pub mod secure_credential;
/// Phase 11 の拡充ループ（親 #896、Issue #897/#898/#899）: 共有シートの effect ドライバ
/// （macOS/iOS/Android/Windows 実装）。
pub mod share;
/// Phase 11 の拡充ループ（親 #1023、Issue #1024・#1025）: 音声認識の effect ドライバ
/// （macOS/iOS/Windows/Android 実装、Android/Web は兄弟 issue）。
pub mod speech_recognition;
/// Phase 11 の拡充ループ（親 #1038、Issue #1039）: システムトレイの effect
/// ドライバ（macOS/Windows 実装、他 OS は `UnsupportedSystemTray` 固定）。
pub mod system_tray;
/// Phase 11 の拡充ループ（親 #911、Issue #912）: 文字サイズ追従の effect ドライバ。
pub mod text_scale;
pub mod tilt;
/// Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）: `device/
/// file_pick.rs` のライフサイクル処理（`PickHooks`/`drive_with`/`drive_mobile_with`）と
/// `bridge` を共有する effect ドライバ。
pub mod tsv_import;
pub mod tts;
/// Phase 11 の拡充ループ（親 #1116、Issue #1215）: カスタム URL scheme 受信の effect
/// ドライバ（macOS 実装、他 OS は `UnsupportedUrlScheme` 固定）。
pub mod url_scheme;
/// Issue #1308: Windows 向けアクセシビリティ設定実装（WinRT `UISettings` / `AccessibilitySettings`）。
/// `device/accessibility_prefs.rs` の Windows 経路から呼ばれる。写像の純関数はホストの
/// `cargo test` で検査するため cfg を付けず、WinRT 呼び出しだけをモジュール内に閉じる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_accessibility_prefs;
/// Issue #1360: Windows 向けタスクバーのオーバーレイアイコン実装（`ITaskbarList3::SetOverlayIcon`）。`device/app_badge.rs` の windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_app_badge;
/// Issue #1330: Windows 向け音声デバイス一覧実装（MMDevice `IMMDeviceEnumerator`）。
/// `device/audio_devices.rs` の Windows 経路（ワーカー）から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_audio_devices;
/// Issue #390: Windows 向けバックグラウンドダウンロードのスタブ（windows #394 が埋める）。
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
/// Issue #907: Windows 向け生体認証実装（`IUserConsentVerifierInterop` 経由の
/// Windows Hello ラッパー）。`device/biometric_auth.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_biometric;
/// Issue #1060: Windows 向け BLE スキャン実装（WinRT `BluetoothLEAdvertisementWatcher`
/// ラッパー）。`device/bluetooth_le.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_bluetooth_le;
/// Issue #1404: Windows 向け方位実装（WinRT `Compass` の pull）。`device/compass.rs` の windows 経路から呼ばれる。
/// 写像の純関数をホストでテストできるよう cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_compass;
/// Issue #1384: Windows 向けメール・SMS 作成画面実装（`ShellExecuteW` で `mailto:` / `sms:`）。`device/compose_mail_sms.rs` の windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_compose_mail_sms;
/// Issue #1304: Windows 向け端末・OS 情報実装（`RtlGetVersion` / レジストリ / `GlobalMemoryStatusEx` /
/// `GetDiskFreeSpaceExW`）。`device/device_info.rs` の Windows 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_device_info;
/// Issue #1410: Windows 向けフォルダ変更監視実装のスタブ（#1411 が埋める）。`device/file_watch.rs` の
/// Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_file_watch;
/// Issue #933: Windows 向けゲームコントローラ実装（WinRT `Windows.Gaming.Input.Gamepad`
/// ラッパー）。`device/gamepad.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_gamepad;
/// Issue #1091: Windows 向け触覚フィードバック実装（WinRT `VibrationDevice` ラッパー）。
/// `device/haptics.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_haptics;
/// Issue #1250: Windows 向け文字認識の骨格（#1251 が WinRT `OcrEngine` 実装に置き換える）。
/// `device/image_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_image_recognition;
/// Issue #1376: Windows 向け OS 搭載フォント一覧実装（DirectWrite のシステムフォントコレクション）。
/// `device/local_fonts.rs` の Windows 経路（ワーカー）から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_local_fonts;
/// Issue #267: Windows 向け音声再生実装のスタブ（親 #86 の windows が埋める）。`device/media_playback.rs` の
/// Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_media_playback;
/// Issue #312: Windows 向け音声変換の実装（MediaTranscoder）。
/// `device/media_transcode.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_media_transcode;
/// Issue #955: Windows 向けマイク録音実装（WinRT `MediaCapture` ラッパー）。
/// `device/mic_record.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_mic_record;
/// Issue #1398: Windows 向け MIDI 入力実装のスタブ（#1402 が埋める）。`device/midi.rs` の
/// Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_midi;
/// Issue #289: Windows 向け機械学習推論のスタブ（windows の子 issue が WinML 実装で埋める）。
/// `device/ml_inference.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_ml_inference;
/// Issue #1341: Windows 向け複数ディスプレイ実装（`EnumDisplayMonitors` / `GetMonitorInfoW` /
/// `GetDpiForMonitor`）。`device/multi_display.rs` の Windows 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_multi_display;
/// Issue #1282: Windows 向けネットワーク接続状態実装（WinRT `NetworkInformation` ラッパー）。
/// `device/network_status.rs` の Windows 経路から呼ばれる。写像の純関数はホストの
/// `cargo test` で検査するため cfg を付けず、WinRT 呼び出しだけをモジュール内に閉じる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_network_status;
/// Issue #1094: Windows 向け単語分割実装（WinRT `WordsSegmenter` ラッパー）。
/// `device/nlp.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_nlp;
/// Issue #1004: Windows 向けローカル通知実装（WinRT `ToastNotificationManager` ラッパー）。
/// `device/notification.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_notification;
/// Issue #343: Windows 向けパスキー実装（`webauthn.h` の MakeCredential / GetAssertion）。
/// `device/passkey.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_passkey;
/// Issue #1247: Windows 向け歩数実装（WinRT `Pedometer` ラッパー）。
/// `device/pedometer.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_pedometer;
/// Issue #359: Windows 向けペン・筆圧入力実装（`SetWindowSubclass` の観測専用サブクラスが
/// `WM_POINTER*` から値を置く）。`device/pen_stylus.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_pen_stylus;
/// Issue #1254: Windows 向け写真保存実装（`SHGetKnownFolderPath(FOLDERID_Pictures)` で解決した
/// フォルダへ `FolderPhotoLibrary` で書く）。`device/photo_save.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_photo_library;
/// Issue #1274: Windows 向け電源状態実装（Win32 `GetSystemPowerStatus`）。
/// `device/power_state.rs` の Windows 経路から呼ばれる。写像の純関数をホストでテストできるよう
/// cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_power_state;
/// Issue #326: Windows 向け印刷実装（`PrintDlgExW` + GDI）。`device/print.rs` の Windows 経路から
/// 呼ばれる。結果の写像は純関数でホストでもテストできる（ffi 部分は `cfg(target_os = "windows")`）。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_print;
/// Issue #26: Windows 向け画面録画実装（`Windows.Graphics.Capture` + `MediaTranscoder`）。
/// `device/screen_capture.rs` の Windows 経路から使われる。
#[cfg(target_os = "windows")]
pub mod windows_screen_capture;
/// Issue #1278: Windows 向け画面スリープ抑止実装（Win32 `SetThreadExecutionState`）。
/// `device/screen_wake_lock.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_screen_wake_lock;
/// Issue #923: Windows 向け資格情報保存実装（WinRT `PasswordVault` ラッパー）。
/// `device/secure_credential.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_secure_credential;
/// Issue #898: Windows 向け共有シート実装（`IDataTransferManagerInterop` 経由の
/// COM interop ラッパー）。`device/share.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_share;
/// Issue #1024: Windows 向け音声認識実装（WinRT `SpeechRecognizer` ラッパー）。
/// `device/speech_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_speech_recognition;
/// Issue #1039: Windows 向けシステムトレイ実装（`Shell_NotifyIconW` + 隠しウィンドウ
/// + `TrackPopupMenu` ラッパー）。`device/system_tray.rs::platform_tray` の Windows
/// 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_system_tray;
/// Issue #913: Windows 向け文字サイズ追従実装（`UISettings.TextScaleFactor` を
/// 読む WinRT ラッパー）。`device/text_scale.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_text_scale;
/// Issue #1216: Windows 向けカスタム URL scheme 受信（レジストリ登録・起動引数・
/// 名前付き mutex と named pipe による起動済みインスタンスへの転送）。
/// `device/url_scheme.rs` の Windows 経路と `main.rs` から使われる。純関数はホストの
/// `cargo test` で検査するため `cfg(target_os = "windows")` を付けず、Win32 呼び出しだけを
/// モジュール内に閉じる（`windows_background_task` と同じ方針）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_url_scheme;

use fandhe_core::screens::storage_note::{NoteStorage, NoteStorageError, StorageNoteState};
use fandhe_device::{storage::Storage, DeviceError};

/// `fandhe_device::DeviceError` を `fandhe_core::screens::storage_note::NoteStorageError`
/// へ変換する（`fandhe-core` は `fandhe-device` に依存しないため、両者の型を橋渡しする
/// のは本クレート側の責務）。`DeviceError` の内部構造（`std::io::Error` 等）は
/// `fandhe-core` 側へ漏らさず、`Display` の文言のみを転送する
/// （`DeviceError` の内部構造を漏らさない、security.md A09 準拠）。
/// アプリ終了時に端末リソースを解放する（`ProxyApp::on_exit` から呼ばれる契約、
/// `app.rs` の `eframe::App::on_exit` 実装 doc 参照）。プロセスが終了すると
/// `static` の `Drop` は走らないため、生きている間に明示停止が要るリソースを
/// ここへ集約する。現状は TTS の子プロセス停止のみ（Issue #865 の codex-review
/// P1 指摘: 共有 TTS を `static OnceLock` で保持しているため `Drop` が実行されず、
/// ウィンドウを閉じても発話中の子プロセスが残る）。
pub fn shutdown() {
    tts::stop_shared_tts();
    location::stop_shared_location();
    system_tray::stop_shared_system_tray();
    bluetooth_le::stop_shared_scan();
    midi::shutdown();
    media_playback::shutdown();
    picture_in_picture::shutdown();
}

fn to_note_storage_error(e: DeviceError) -> NoteStorageError {
    match e {
        // `DeviceError::Io` の `Display` は既に「入出力エラー: 」を前置するため、
        // ここでは内側の `std::io::Error` の `Display` のみを転送する
        // （`NoteStorageError::Io` 側の `#[error("入出力エラー: {0}")]` と二重にしない）。
        DeviceError::Io(io_err) => NoteStorageError::Io(io_err.to_string()),
        DeviceError::InvalidKey(key) => NoteStorageError::InvalidKey(key),
        DeviceError::Unsupported(s) => NoteStorageError::Unsupported(s.to_string()),
        DeviceError::Http(_)
        | DeviceError::InvalidImageSize(_, _)
        | DeviceError::PermissionDenied(_)
        | DeviceError::Cancelled(_)
        | DeviceError::Internal(_)
        | DeviceError::InvalidInput(_)
        | DeviceError::Disabled(_) => NoteStorageError::Other(e.to_string()),
    }
}

/// `fandhe_device::storage::Storage` 実装を `fandhe_core::screens::storage_note::NoteStorage`
/// へ橋渡しする newtype。エラーは [`to_note_storage_error`] で型付きのまま変換する
/// （`.claude/rules/coding-rust.md`「ライブラリクレート: thiserror 等による型付きエラー」準拠）。
pub struct DeviceNoteStorage(pub Box<dyn Storage>);

impl NoteStorage for DeviceNoteStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, NoteStorageError> {
        self.0.read(key).map_err(to_note_storage_error)
    }

    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), NoteStorageError> {
        self.0.write(key, bytes).map_err(to_note_storage_error)
    }
}

/// 任意の `Storage` 実装から `StorageNoteState` を構築する（テスト・#750/#751 が
/// Android/iOS 向け `DataDir` 注入で差し替える入口）。
pub fn storage_note_state_with(storage: Box<dyn Storage>) -> StorageNoteState {
    StorageNoteState::with_storage(Box::new(DeviceNoteStorage(storage)))
}

/// レジストリの `init "storage_note"` 行から呼ばれる状態ファクトリ
/// （`fn() -> StorageNoteState`）。
///
/// デスクトップ（macOS/Windows/Linux）は `FsStorage::desktop_default()`
/// （`$HOME/.fandhe`）を保存先にする。Android は `AndroidApp::internal_data_path()`
/// （`getFilesDir()` 相当、`device::android::data_dir()`、Issue #750）を起点にし、
/// iOS は [`ios::documents_dir`]（アプリコンテナ内 `Documents/`、Issue #751）を
/// 保存先にする。
pub fn storage_note_state() -> StorageNoteState {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        use fandhe_device::storage::FsStorage;
        match FsStorage::desktop_default() {
            Ok(dir) => storage_note_state_with(Box::new(FsStorage::new(dir))),
            Err(e) => {
                StorageNoteState::default().with_status(format!("保存先の解決に失敗しました: {e}"))
            }
        }
    }
    #[cfg(target_os = "android")]
    {
        use fandhe_device::storage::FsStorage;
        match android::data_dir() {
            Some(dir) => storage_note_state_with(Box::new(FsStorage::new(dir))),
            None => StorageNoteState::default()
                .with_status("保存先（内部ストレージ）を取得できませんでした"),
        }
    }
    #[cfg(target_os = "ios")]
    {
        use fandhe_device::storage::FsStorage;
        match ios::documents_dir() {
            Ok(dir) => storage_note_state_with(Box::new(FsStorage::new(dir))),
            Err(e) => {
                StorageNoteState::default().with_status(format!("保存先の解決に失敗しました: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::{DataDir, DeviceError};
    use std::sync::{Arc, Mutex};

    /// `fandhe_device::storage::Storage` のインメモリ実装。`Storage` トレイト側で
    /// 「保存 → 別状態インスタンスで復元」を満たすことを検証する（受入 (b) を
    /// `Storage` 経由でも担保する。`fandhe-core` 側の `NoteStorage` 直接実装での検証は
    /// `crates/fandhe-core/tests/storage_note.rs` を参照）。
    #[derive(Clone, Default)]
    struct MemStorage(Arc<Mutex<Option<Vec<u8>>>>);

    impl Storage for MemStorage {
        fn read(&self, _key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Ok(self.0.lock().expect("lock は成功する").clone())
        }

        fn write(&self, _key: &str, bytes: &[u8]) -> Result<(), DeviceError> {
            *self.0.lock().expect("lock は成功する") = Some(bytes.to_vec());
            Ok(())
        }
    }

    /// 不正な key を要求して `DeviceError::InvalidKey` を返す `Storage`
    /// （エラー文字列化の転送経路を検証する）。
    struct RejectingStorage;

    impl Storage for RejectingStorage {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }

        fn write(&self, key: &str, _bytes: &[u8]) -> Result<(), DeviceError> {
            Err(DeviceError::InvalidKey(key.to_string()))
        }
    }

    #[test]
    fn memory_storage_round_trips_through_screen() {
        use fandhe_core::click_button;
        use fandhe_core::screens::storage_note::{set_input, storage_note_screen};

        let mem = MemStorage::default();
        let screen = storage_note_screen();
        let mut state = storage_note_state_with(Box::new(mem.clone()));
        set_input(&mut state, "メモ".to_string());
        assert!(click_button(&screen, "保存", &mut state));

        let restored = storage_note_state_with(Box::new(mem));
        assert_eq!(restored.notes, vec!["メモ".to_string()]);
    }

    #[test]
    fn fs_storage_survives_state_recreation() {
        use fandhe_device::storage::FsStorage;

        let dir = std::env::temp_dir().join(format!(
            "fandhe-egui-proxy-device-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("システム時刻は UNIX epoch 以降")
                .as_nanos()
        ));

        let first = storage_note_state_with(Box::new(FsStorage::new(DataDir(dir.clone()))));
        assert!(first.notes.is_empty());

        use fandhe_core::screens::storage_note::{encode_notes, NOTES_KEY};
        FsStorage::new(DataDir(dir.clone()))
            .write(NOTES_KEY, &encode_notes(&["復元されるメモ".to_string()]))
            .expect("write は成功する");

        // 新しい `FsStorage` インスタンス + 新しい状態でプロセス再起動を模す。
        let restored = storage_note_state_with(Box::new(FsStorage::new(DataDir(dir))));
        assert_eq!(restored.notes, vec!["復元されるメモ".to_string()]);
    }

    #[test]
    fn device_error_message_is_forwarded() {
        let bridged = DeviceNoteStorage(Box::new(RejectingStorage));
        let err = bridged.read("../x").unwrap_err();
        assert!(err.to_string().contains("../x"));
    }

    /// PR #787 Bugbot 指摘: `DeviceError::Io` の `Display` は既に「入出力エラー: 」を
    /// 前置するため、`to_note_storage_error` がそれをそのまま `NoteStorageError::Io` へ
    /// 格納すると「入出力エラー: 入出力エラー: ...」と二重表示になっていた。
    #[test]
    fn io_error_prefix_is_not_duplicated() {
        let io_err = std::io::Error::other("disk full");
        let err = to_note_storage_error(DeviceError::Io(io_err));
        let msg = err.to_string();
        assert_eq!(msg.matches("入出力エラー").count(), 1, "got: {msg}");
    }
}
