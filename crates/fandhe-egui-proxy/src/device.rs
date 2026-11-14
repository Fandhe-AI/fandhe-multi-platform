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
//! `screens/registry.rs` の `init "storage_note" => ..., crate::device::storage_note_state,`
//! 行から [`storage_note_state`] が呼ばれ、`generated_screens!` マクロが構築時に
//! 1 度だけ実行する（`screens.rs` の `init` 区画 doc 参照）。Issue #748（HTTP 取得）・
//! #749（カメラ撮影）・#830（傾きで玉が転がる）の effect ドライバはそれぞれ
//! [`http_fetch`] / [`tilt`] サブモジュールに置く（カメラ撮影は機能 crate `fandhe-cap-camera-capture`、#1170）
//! （`registry.rs` の `effect` 区画から配線される）。

/// Issue #613（親 #124）: 行動認識（歩行・乗車等）の effect ドライバ（Windows 実装、iOS 実装、
/// Android は b-android が埋めるスタブ、他 OS は `UnsupportedActivityRecognizer` 固定）。
pub mod activity_recognition;
/// Phase 11 の拡充ループ（親 #140、Issue #721）: アラーム・正確な時刻の予約の effect
/// ドライバ（Windows 実装、iOS は AlarmKit、Android は #1078 が埋めるスタブ、他 OS は `Unsupported` 固定）。
pub mod alarm;
#[cfg(target_os = "android")]
pub mod android;
/// Issue #1126: Android 向け行動認識の骨格（実体は b-android）。`device/activity_recognition.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_activity_recognition;
/// Issue #1076: Android 向けアラーム実装のスタブ（実装は #1078）。`device/alarm.rs` の Android 経路から呼ばれる。
/// #1078 が cfg を変えずに済むよう `any(android, test)`。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_alarm;
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
/// Issue #1123（親 #164）: Android 向け BLE の周辺機器役の実装（骨格は #1122）。`device/ble_peripheral.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_ble_peripheral;
/// Issue #659: Android 向け Bluetooth Classic の骨格（`Unsupported` のスタブ。#661 が
/// `BluetoothClassicHelper.java` と JNI で埋める）。`device/bluetooth_classic.rs` の Android 経路から呼ばれる。
/// 後続の子がホストテストを足すときに本行の cfg を広げずに済むよう test でもコンパイルする。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_bluetooth_classic;
/// Issue #812: Android 向けカメラのライブ映像実装のスタブ（#814 が埋める）。
/// `device/camera_preview_stream.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_camera_preview_stream;
/// Issue #1117: Android 向け車載ディスプレイ（Car App Library の Service/Screen を JNI 経由で操作する）。
/// `device/car_display.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_car_display;
/// Issue #1404: Android 向け方位実装。`device/compass.rs` の android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_compass;
/// Issue #1279: Android 向けディープリンク（App Links）受信（https の `ACTION_VIEW` を JNI で受ける）。
/// `device/deep_link.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_deep_link;
/// Issue #1304: Android 向け端末・OS 情報実装。`device/device_info.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_device_info;
/// Issue #609: Android 向け気圧・照度・近接センサー実装。
/// `device/environment_sensors.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_environment_sensors;
/// Issue #416: Android 向けファイルの関連付け受信のスタブ（b-android が埋める）。
/// `device/file_association.rs::platform` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_file_association;
/// Issue #1098: Android 向け折りたたみ姿勢の実装（`FoldablePostureHelper` との JNI 橋渡し）。
/// `device/foldable_posture.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_foldable_posture;
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
/// Issue #701 / #703（親 #135）: Android 向けアプリ内ブラウザ（Custom Tabs）の骨格。
/// `device/in_app_browser.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_in_app_browser;
/// Issue #489: Android 向け LAN のサービス探索（`NsdManager`。Java 側は `LanDiscoveryHelper`、親 #109 の b-android）。
/// `device/lan_discovery.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_lan_discovery;
/// Issue #511: Android 向け近くの端末との直接通信実装。
/// `device/local_peer.rs` の Android 経路から呼ばれる。
#[cfg(target_os = "android")]
pub mod android_local_peer;
/// Issue #267: Android 向け音声再生実装。`device/media_playback.rs` の
/// Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_media_playback;
/// Issue #898: Android 向けジャイロ・磁気・線形加速度の生値実装（骨格。実体は #901）。
/// `device/motion_raw_sensors.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_motion_raw_sensors;
/// Issue #485: Android 向け音楽ライブラリ実装（`MediaStore.Audio.Media` を JNI 経由で読む）。
/// `device/music_library.rs` の Android 経路から呼ばれる。純関数をホストでテストするため test でも載せる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_music_library;
/// Issue #1284: Android 向けネットワーク接続状態実装（`ConnectivityManager` を JNI で読む）。
/// `device/network_status.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_network_status;
/// Issue #383: Android 向けロック画面・OS のメディア操作実装（`MediaSession` + `Notification.MediaStyle` を JNI で操作）。
/// `device/now_playing.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_now_playing;
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
/// Issue #720（親 #139）: Android 向けクイック設定タイル。Java の `TileService` が SharedPreferences に
/// 数えた回数を JNI で読む実装。`device/quick_settings_tile.rs` の Android 経路から呼ばれる。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_quick_settings_tile;
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
/// Issue #537（親 #115）: Android 向け端末内翻訳実装（`android.view.translation`）。
/// `device/translation.rs` の Android 経路から呼ばれる。JNI の本体はモジュール内側の
/// `cfg(target_os = "android")` に閉じ、純関数をホストでテストできるよう cfg は `any(android, test)`。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_translation;
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
/// Issue #516: Android 向け Wi-Fi 情報・スキャン実装（`WifiManager`、JNI 経由の pull 型 bridge）。
/// `device/wifi_info.rs` の Android 経路から呼ばれる。純関数をホストでテストするため test でもコンパイルする。
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod android_wifi_info;
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
/// Issue #906: iOS / macOS 向け生体認証実装（`apple/fandhe_apple_biometric.m` を呼ぶ
/// C ABI ラッパー）。`device/biometric_auth.rs` の macOS/iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_biometric;
/// Issue #778 macOS・Issue #779 iOS: BLE の周辺機器役実装（`apple/fandhe_apple_ble_peripheral.m` を
/// 呼ぶ C ABI ラッパー）。`device/ble_peripheral.rs` の macOS 経路と、委譲する
/// `ios_ble_peripheral`（iOS）から呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_ble_peripheral;
/// Issue #1060 macOS + #1061 iOS: BLE スキャン実装（`apple/fandhe_apple_bluetooth_le.m` を
/// 呼ぶ C ABI ラッパー）。`device/bluetooth_le.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_bluetooth_le;
/// Issue #1220 macOS + #1221 iOS: 連絡先選択実装（`apple/fandhe_apple_contact_picker.m` を
/// 呼ぶ C ABI ラッパー）。`device/contact_pick.rs` の Apple 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_contact_picker;
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
/// Issue #486 macOS: LAN のサービス探索実装（`apple/fandhe_apple_lan_discovery.m` を呼ぶ C ABI ラッパー）。
/// `device/lan_discovery.rs` の macOS 経路と、b-ios が委譲する `ios_lan_discovery` から呼ばれる。
/// `build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_lan_discovery;
/// Issue #511 macOS: 近くの端末との直接通信実装（`apple/fandhe_apple_local_peer.m` を呼ぶ C ABI ラッパー）。
/// `device/local_peer.rs` の macOS 経路と、`ios_local_peer`（Issue #512）の委譲から呼ばれる。
/// `build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_local_peer;
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
/// Issue #954（macOS）/ #956（iOS）: iOS / macOS 向けマイク録音実装（`apple/
/// fandhe_apple_mic_record.m` を呼ぶ C ABI ラッパー）。`device/mic_record.rs` の
/// iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_mic_record;
/// Issue #1282: macOS / iOS 向けネットワーク接続状態（`apple/fandhe_apple_network_status.m` を
/// 呼ぶ C ABI ラッパー）。`device/network_status.rs` の macOS / iOS 経路から呼ばれる。
/// 純関数をホストの `cargo test` で検査するため cfg に `test` を含める（FFI 部分は macOS 限定）。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
pub mod apple_network_status;
/// Issue #381（macOS）・#382（iOS）: ロック画面・OS のメディア操作実装（`apple/fandhe_apple_now_playing.m` の
/// MediaPlayer を呼ぶ C ABI ラッパー）。`device/now_playing.rs` の macOS 経路と、iOS 経路（`ios_now_playing` 経由）から呼ばれる。
/// 写像の純関数をホストでテストできるよう `test` でもコンパイルする。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub mod apple_now_playing;
/// Issue #1254（macOS）/ #1255（iOS）: iOS / macOS 向け写真ライブラリ保存実装
/// （`apple/fandhe_apple_photo_library.m` を呼ぶ C ABI ラッパー。iOS は実装済み）。
/// `device/photo_save.rs` の iOS / macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_photo_library;
/// Issue #364（macOS）/ #365（iOS）: ピクチャ・イン・ピクチャ実装（`apple/fandhe_apple_picture_in_picture.m` の AVKit を
/// 呼ぶ C ABI ラッパー）。`device/picture_in_picture.rs` の macOS・iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_picture_in_picture;
/// Issue #719（macOS）: クイック設定コントロールの回数を App Group コンテナのファイルから読む実装。
/// `device/quick_settings_tile.rs` の macOS 経路から呼ばれる。ホストの `cargo test` でも検証できるよう
/// `cfg(test)` でも有効にする。
#[cfg(any(target_os = "macos", test))]
pub mod apple_quick_settings_tile;
/// Issue #922・#924: iOS / macOS 向け資格情報保存実装（`apple/
/// fandhe_apple_secure_credential.m` を呼ぶ C ABI ラッパー）。`device/
/// secure_credential.rs` の iOS/macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_secure_credential;
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
/// Issue #535 macOS・#536 iOS: 端末内翻訳実装（`apple/fandhe_apple_translation.swift` を呼ぶ
/// C ABI ラッパー）。`device/translation.rs` の macOS 経路と、iOS 経路（#536 で `ios_translation`
/// 経由）から呼ばれる。`build.rs` のコンパイル条件と同じ OS 集合に保つ。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_translation;
/// Issue #1215・#1217: macOS / iOS 向けカスタム URL scheme 受信（`macos/fandhe_macos_url_scheme.m`
/// の Apple Event ハンドラ、`ios/fandhe_ios_url_scheme.m` の delegate から呼ばれる C ABI
/// ラッパー）。`device/url_scheme.rs::platform_url_scheme` の macOS・iOS 経路と
/// `main.rs`（登録）から使われる。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple_url_scheme;
/// Phase 11 の拡充ループ（親 #121、Issue #585）: ワンタイムコード入力の effect ドライバ（iOS は
/// `ios_autofill_otp.rs` で実装、Android・macOS は保留、Windows は非対応で `UnsupportedAutofillOtp` 固定。
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
/// Phase 11 の拡充ループ（親 #905、Issue #906/#907/#908）: 生体認証の effect ドライバ
/// （macOS/iOS/Android/Windows 実装）。
pub mod biometric_auth;
/// Phase 11 の拡充ループ（親 #164、Issue #778）: BLE の周辺機器役の effect ドライバ
/// （macOS・iOS・Windows 実装。Android は骨格で #1123 が実装。他 OS は
/// `UnsupportedBlePeripheral` 固定）。
pub mod ble_peripheral;
/// Phase 11 の拡充ループ（親 #130、Issue #659）: Bluetooth Classic の一覧・検索の effect ドライバ
/// （macOS 実装。Windows・Android は骨格のスタブで #660 / #661 が埋める。他 OS は `UnsupportedBluetoothClassic` 固定）。
pub mod bluetooth_classic;
/// Phase 11 の拡充ループ（親 #1059、Issue #1060）: BLE スキャンの effect ドライバ
/// （iOS/macOS/Windows/Android 実装、他 OS は `UnsupportedBluetoothLe` 固定。Web は別経路 `fandhe-web-dom`（#1063））。
pub mod bluetooth_le;
/// Phase 11 の拡充ループ（親 #168、Issue #812）: カメラのライブ映像・フレーム取得の effect ドライバ
/// （iOS #813・Android #814・macOS #1066 は実装済み、Windows #1102 も実装済みで、他 OS は `UnsupportedCameraPreview`
/// 固定。Web は `fandhe-web-dom`、#815）。
pub mod camera_preview_stream;
/// Phase 11 の拡充ループ（親 #152、Issue #1116）: 車載ディスプレイの effect ドライバ。
/// Android は Car App Library 実装（#1117）、他 OS は `UnsupportedCarDisplay` 固定。
pub mod car_display;
/// Issue #1404（親 #1154）: 方位（コンパス）の effect ドライバ（Windows・iOS・Android 実装、
/// 他 OS は `UnsupportedCompass` 固定。Web は `fandhe-web-dom`）。
pub mod compass;
/// Phase 11 の拡充ループ（親 #1117、Issue #1220・#1221・#1222）: 連絡先選択の effect ドライバ
/// （macOS / iOS / Android は pull 型 `bridge`、他 OS は兄弟 issue が入るまで
/// `UnsupportedContactPicker` 固定）。
pub mod contact_pick;
/// Phase 11 の拡充ループ（親 #153、Issue #1278）: ディープリンク受信の effect ドライバ
/// （Android 分岐はスタブ、他 OS は `UnsupportedDeepLink` 固定）。
pub mod deep_link;
/// Phase 11 の拡充ループ（親 #1131、Issue #1304）: 端末・OS 情報の effect ドライバ（macOS / Windows
/// ・iOS・Android 実装、他 OS は `UnsupportedDeviceInfo` 固定）。
pub mod device_info;
/// Phase 11（親 #123、Issue #609）: 気圧・照度・近接センサーの effect ドライバ（iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedEnvironmentSensors` 固定。Web は対応表で `no`）。
pub mod environment_sensors;
/// Phase 11 の拡充ループ（親 #102、Issue #416）: ファイルの関連付けの effect ドライバ
/// （macOS・iOS・Windows 実装、Android は未実装のスタブ、他 OS は `Unsupported` 固定）。
pub mod file_association;
pub mod file_pick;
/// Phase 11 の拡充ループ（親 #141、Issue #723）: 折りたたみの姿勢の effect ドライバ
/// （Android は OS 別モジュール（#1098 が埋めるスタブ）、他ネイティブ OS は
/// `UnsupportedFoldablePosture` 固定。Web は `fandhe-web-dom`）。
pub mod foldable_posture;
/// Phase 11 の拡充ループ（親 #1133、Issue #1312）: 全画面表示の effect ドライバ（macOS/Windows は
/// eframe の viewport コマンド、iOS/Android は OS 別モジュール、他 OS は
/// `UnsupportedFullscreen` 固定。Web は `fandhe-web-dom`）。
pub mod fullscreen_immersive;
/// Phase 11 の拡充ループ（親 #1056、Issue #1057）: ジオフェンスの effect ドライバ
/// （location の継続取得に乗せて `GeofenceTracker` で判定する）。
pub mod geofence;
/// Phase 11 の拡充ループ（親 #149、Issue #1089）: ヘルスデータ（今日の歩数）の effect ドライバ
/// （iOS・Android は OS 別モジュール。他 OS は `UnsupportedHealthData` 固定）。
pub mod health_data;
/// Phase 11 の拡充ループ（親 #1120、Issue #1250）: 画像の文字認識の effect ドライバ
/// （macOS・Windows / iOS 実装、Android は骨格のスタブ（対応表で `no`）、他 OS は
/// `UnsupportedTextRecognizer` 固定。Web は対象外）。
pub mod image_recognition;
/// Phase 11 の拡充ループ（親 #135、Issue #701）: アプリ内ブラウザの effect ドライバ（iOS は
/// `SFSafariViewController`・Android は Custom Tabs で #702・#703 が実装、他 OS は
/// `UnsupportedInAppBrowser` 固定。Web は対象外）。
pub mod in_app_browser;
#[cfg(target_os = "ios")]
pub mod ios;
/// Issue #613: iOS 向け行動認識実装。`device/activity_recognition.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_activity_recognition;
/// Issue #1077: iOS 向けアラーム実装（AlarmKit を呼ぶ Swift の C ABI ラッパー）。`device/alarm.rs` の iOS 経路から呼ばれる。
/// ホストのテストでも戻りコード写像を検証できるよう `any(ios, test)`。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_alarm;
/// Issue #425: iOS 向けアプリショートカット実装（Info.plist の静的宣言。`Declared` を返す）。
/// `device/app_shortcuts.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_app_shortcuts;
/// Issue #585: iOS 向けワンタイムコード自動入力の骨格（実装は #586）。
/// `device/autofill_otp.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_autofill_otp;
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
/// Issue #779: iOS 向け BLE の周辺機器役（`apple_ble_peripheral` へ委譲する）。
/// `device/ble_peripheral.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_ble_peripheral;
/// Issue #813・#1066: iOS・macOS 向けカメラのライブ映像実装（`ios/fandhe_ios_camera_preview_stream.m` を共有）。
/// `device/camera_preview_stream.rs` の iOS・macOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
pub mod ios_camera_preview_stream;
/// Issue #1405: iOS 向け方位実装（`ios/fandhe_ios_compass.m` を呼ぶ C ABI ラッパー、CoreLocation の heading）。`device/compass.rs` の ios 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_compass;
/// Issue #1305: iOS 向け端末・OS 情報実装（`ios/fandhe_ios_device_info.m` を呼ぶ C ABI ラッパー。
/// #1304 のスタブを置き換え）。`device/device_info.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_device_info;
/// Issue #609: iOS 向け気圧・照度・近接センサー実装。
/// `device/environment_sensors.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_environment_sensors;
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
/// Issue #701 / #702（親 #135）: iOS 向けアプリ内ブラウザ（`SFSafariViewController`）の骨格。
/// `device/in_app_browser.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_in_app_browser;
/// Issue #488: iOS 向け LAN のサービス探索（`apple_lan_discovery` へ委譲する）。
/// `device/lan_discovery.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_lan_discovery;
/// Issue #512: iOS 向け近くの端末との直接通信（親 #110、`apple_local_peer` へ委譲する）。
/// `device/local_peer.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_local_peer;
/// Issue #267: iOS 向け音声再生実装。`device/media_playback.rs` の
/// iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_media_playback;
/// Issue #900: iOS 向けジャイロ・磁気・線形加速度の生値実装（`ios/fandhe_ios_motion_raw_sensors.m`、
/// CoreMotion の `CMMotionManager`）。`device/motion_raw_sensors.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_motion_raw_sensors;
/// Issue #484: iOS 向け音楽ライブラリ実装（`ios/fandhe_ios_music_library.m` の MediaPlayer を呼ぶ
/// C ABI ラッパー）。`device/music_library.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_music_library;
/// Issue #1283: iOS 向けネットワーク接続状態実装（`apple_network_status` を返す）。
/// `device/network_status.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_network_status;
/// Issue #1005: iOS 向けローカル通知実装（`ios/fandhe_ios_notification.m` を呼ぶ
/// C ABI ラッパー）。`device/notification.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_notification;
/// Issue #382: iOS 向けロック画面・OS のメディア操作実装（macOS と共用の `apple_now_playing` を返す）。
/// `device/now_playing.rs` の iOS 経路から呼ばれる。
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub mod ios_now_playing;
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
/// Issue #536: iOS 向け端末内翻訳実装（macOS と共用の `AppleTextTranslator` を返す）。`device/translation.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_translation;
/// Issue #532: iOS 向け Web 認証セッション実装（`ios/fandhe_ios_web_auth_session.m` の
/// `ASWebAuthenticationSession` を呼ぶ C ABI ラッパー）。`device/web_auth_session.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub mod ios_web_auth_session;
/// Phase 11 の拡充ループ（親 #109、Issue #486）: LAN のサービス探索の effect ドライバ
/// （macOS・iOS / Android 実装、Windows は `DnsServiceBrowse`、他 OS は `UnsupportedLanDiscovery` 固定。Web は対象外）。
pub mod lan_discovery;
/// Phase 11 の拡充ループ（親 #110、Issue #511）: 近くの端末との直接通信の effect ドライバ
/// （macOS・iOS / Android 実装、他 OS は `UnsupportedLocalPeer` 固定）。
pub mod local_peer;
/// Phase 11 の拡充ループ（親 #887、Issue #888）: 位置情報の effect ドライバ（Windows のみ実装）。
pub mod location;
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
/// Issue #659: macOS 向け Bluetooth Classic 実装（`macos/fandhe_macos_bluetooth_classic.m` の
/// IOBluetooth 呼び出しを包む C ABI ラッパー）。`device/bluetooth_classic.rs` の macOS 経路から呼ばれる。
/// 写像の純関数をホストでテストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_bluetooth_classic;
/// Issue #1304: macOS 向け端末・OS 情報実装（`macos/fandhe_macos_device_info.m` を呼ぶ C ABI
/// ラッパー）。`device/device_info.rs` の macOS 経路から呼ばれる。写像の純関数をホストで
/// テストできるよう cfg は `any(macos, test)`。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_device_info;
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
/// Issue #514: macOS 向け Wi-Fi 情報実装（`macos/fandhe_macos_wifi_info.m` の CoreWLAN と CoreLocation の
/// 許可判定を呼ぶ C ABI ラッパー）。`device/wifi_info.rs` の macOS 経路から呼ばれる。写像の純関数を
/// ホストでテストするため test でも載せる。
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod macos_wifi_info;
/// Issue #636: macOS 向けウィンドウ管理の読み取り実装（`macos/fandhe_macos_window_management.m` を呼ぶ
/// C ABI ラッパー）。`device/window_management.rs` の macOS 経路から呼ばれる。
#[cfg(target_os = "macos")]
pub mod macos_window_management;
/// Phase 11 の拡充ループ（親 #85、Issue #273）: 地図表示の effect ドライバ（macOS / iOS は
/// pull 型 `bridge`、他 OS は `UnsupportedMapSnapshotter` 固定）。
pub mod map_view;
/// Phase 11 の拡充ループ（親 #86、Issue #267）: 音声再生の effect ドライバ（macOS・iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedMediaPlayback` 固定）。
pub mod media_playback;
/// Phase 11 の拡充ループ（親 #953、Issue #954・#955・#956・#957）: マイク録音の
/// effect ドライバ（macOS/iOS/Windows/Android 実装、他 OS は `UnsupportedMicRecorder`
/// 固定）。
pub mod mic_record;
/// Phase 11（親 #180、Issue #898）: ジャイロ・磁気・線形加速度の生値の effect ドライバ（iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedMotionRawSensors` 固定。Web は `fandhe-web-dom` 側）。
pub mod motion_raw_sensors;
/// Phase 11 の拡充ループ（親 #108、Issue #482）: 音楽ライブラリの effect ドライバ（macOS・iOS 実装、
/// Windows・Android は #483・#485 の担当、他 OS は `UnsupportedMusicLibrary` 固定。
/// Web は対象外）。
pub mod music_library;
/// Phase 11 の拡充ループ（親 #1130、Issue #1282）: ネットワーク接続状態の effect ドライバ
/// （Windows / macOS / iOS 実装、Android は #1284、他 OS は
/// `UnsupportedNetworkStatus` 固定。Web は `fandhe-web-dom`）。
pub mod network_status;
/// Phase 11 の拡充ループ（親 #1003、Issue #1004/#1005）: ローカル通知の effect
/// ドライバ（macOS/Windows/iOS 実装、Android は #1006）。
pub mod notification;
/// Phase 11 の拡充ループ（親 #98、Issue #381）: ロック画面・OS のメディア操作の effect ドライバ（macOS・iOS・Android・Windows 実装、
/// 他 OS は `UnsupportedNowPlaying` 固定。
/// Web は #384 で `fandhe-web-dom`）。`media_playback` の `audio_play` 画面から呼ばれる。
pub mod now_playing;
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
/// Phase 11 の拡充ループ（親 #139、Issue #719）: クイック設定タイル・コントロールの effect ドライバ
/// （macOS は App Group の回数ファイル、Android は SharedPreferences の回数、他 OS は `UnsupportedQuickSettingsTile` 固定）。
pub mod quick_settings_tile;
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
/// Phase 11 の拡充ループ（親 #921、Issue #922・#924）: 資格情報保存の effect
/// ドライバ（macOS/iOS/Android 実装、Windows は #923）。
pub mod secure_credential;
/// Phase 11 の拡充ループ（親 #1023、Issue #1024・#1025）: 音声認識の effect ドライバ
/// （macOS/iOS/Windows/Android 実装、Android/Web は兄弟 issue）。
pub mod speech_recognition;
/// Phase 11 の拡充ループ（親 #117、Issue #560）: OS の検索への登録の effect ドライバ（macOS・iOS・
/// Android 実装、他 OS は `UnsupportedSpotlightIndex` 固定）。
pub mod spotlight_index;
pub mod tilt;
/// Phase 11 の拡充ループ（親 #115、Issue #535）: 端末内翻訳の effect ドライバ（macOS 実装、
/// iOS・Android も実装済み（#536・#537）、他 OS は `UnsupportedTextTranslator` 固定。Web は
/// `fandhe-web-dom` が持つ）。
pub mod translation;
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
/// Phase 11 の拡充ループ（親 #111、Issue #514）: Wi-Fi 情報の effect ドライバ（macOS・Windows・Android 実装、
/// iOS（deferred）・他 OS は `UnsupportedWifiInfo` 固定。Web は対象外）。
pub mod wifi_info;
/// Phase 11 の拡充ループ（親 #127、Issue #636）: ウィンドウ管理（位置・サイズ・最前面）の effect ドライバ
/// （macOS 実装、Windows は #637、他 OS は `UnsupportedWindowManager` 固定。Web は `fandhe-web-dom`）。
pub mod window_management;
/// Issue #613: Windows 向け行動認識実装（WinRT `ActivitySensor` の `ReadingChanged`）。`device/activity_recognition.rs` の windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_activity_recognition;
/// Issue #721: Windows 向けアラーム実装（`ScheduledToastNotification` +
/// `scenario="alarm"` のトースト）。`device/alarm.rs` の Windows 経路から呼ばれる。
/// 時刻変換・検証・XML 定数等の純関数はホストの `cargo test` から検証できるよう
/// `cfg(target_os = "windows")` を付けず、WinRT 呼び出しだけをモジュール内の
/// `cfg(target_os = "windows")` に閉じる。Windows 以外では利用者が無いため
/// `dead_code` を許容する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_alarm;
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
/// Issue #907: Windows 向け生体認証実装（`IUserConsentVerifierInterop` 経由の
/// Windows Hello ラッパー）。`device/biometric_auth.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_biometric;
/// Issue #1101: Windows 向け BLE の周辺機器役（WinRT `GattServiceProvider`）。
/// `device/ble_peripheral.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_ble_peripheral;
/// Issue #659: Windows 向け Bluetooth Classic の骨格（`Unsupported` のスタブ。#660 が
/// `BluetoothFindFirstDevice` で埋める）。`device/bluetooth_classic.rs` の Windows 経路から呼ばれる。
/// 後続の子がホストテストを足すときに本行の cfg を広げずに済むよう test でもコンパイルする。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_bluetooth_classic;
/// Issue #1060: Windows 向け BLE スキャン実装（WinRT `BluetoothLEAdvertisementWatcher`
/// ラッパー）。`device/bluetooth_le.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_bluetooth_le;
/// Issue #1102: Windows 向けカメラのライブ映像実装（WinRT `MediaCapture` + `MediaFrameReader`）。
/// `device/camera_preview_stream.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_camera_preview_stream;
/// Issue #1404: Windows 向け方位実装（WinRT `Compass` の pull）。`device/compass.rs` の windows 経路から呼ばれる。
/// 写像の純関数をホストでテストできるよう cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_compass;
/// Issue #1304: Windows 向け端末・OS 情報実装（`RtlGetVersion` / レジストリ / `GlobalMemoryStatusEx` /
/// `GetDiskFreeSpaceExW`）。`device/device_info.rs` の Windows 経路から呼ばれる。写像の純関数を
/// ホストでテストできるよう cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_device_info;
/// Issue #609: Windows 向け気圧・照度・近接センサー実装。
/// `device/environment_sensors.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_environment_sensors;
/// Issue #416: Windows 向けファイルの関連付け受信実装。
/// `device/file_association.rs` の Windows 経路と `main.rs` から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_file_association;
/// Issue #1250: Windows 向け文字認識の骨格（#1251 が WinRT `OcrEngine` 実装に置き換える）。
/// `device/image_recognition.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_image_recognition;
/// Issue #487: Windows 向け LAN のサービス探索（Win32 `DnsServiceBrowse`。親 #109 の windows の子）。
/// `device/lan_discovery.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_lan_discovery;
/// Issue #267: Windows 向け音声再生実装。`device/media_playback.rs` の
/// Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_media_playback;
/// Issue #955: Windows 向けマイク録音実装（WinRT `MediaCapture` ラッパー）。
/// `device/mic_record.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_mic_record;
/// Issue #898: Windows 向けジャイロ・磁気・線形加速度の生値実装（骨格。実体は #899）。
/// `device/motion_raw_sensors.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_motion_raw_sensors;
/// Issue #483: Windows 向け音楽ライブラリ実装（Music フォルダの走査 + プロパティストア）。
/// `device/music_library.rs` の Windows 経路から呼ばれる。走査の純関数をホストの `cargo test` で
/// 検査するため cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_music_library;
/// Issue #1282: Windows 向けネットワーク接続状態実装（WinRT `NetworkInformation` ラッパー）。
/// `device/network_status.rs` の Windows 経路から呼ばれる。写像の純関数はホストの
/// `cargo test` で検査するため cfg を付けず、WinRT 呼び出しだけをモジュール内に閉じる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_network_status;
/// Issue #1004: Windows 向けローカル通知実装（WinRT `ToastNotificationManager` ラッパー）。
/// `device/notification.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_notification;
/// Issue #381: Windows 向けロック画面・OS のメディア操作実装。
/// `device/now_playing.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_now_playing;
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
/// Issue #923: Windows 向け資格情報保存実装（WinRT `PasswordVault` ラッパー）。
/// `device/secure_credential.rs` の Windows 経路から呼ばれる。
#[cfg(target_os = "windows")]
pub mod windows_secure_credential;
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
/// Issue #515: Windows 向け Wi-Fi 情報実装（`wlanapi`）。
/// `device/wifi_info.rs` の Windows 経路から呼ばれる。写像の純関数をホストでテストできるよう
/// cfg は `any(windows, test)`。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_wifi_info;
/// Issue #637: Windows 向けウィンドウ管理の読み取り（`NativeReads` の Windows 実装。最前面か・今いる画面の
/// 作業領域を Win32 で読む）。`device/window_management.rs` の Windows 経路から呼ばれる。
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod windows_window_management;

use fandhe_core::screens::storage_note::{NoteStorage, NoteStorageError, StorageNoteState};
use fandhe_device::{storage::Storage, DeviceError};

/// `fandhe_device::DeviceError` を `fandhe_core::screens::storage_note::NoteStorageError`
/// へ変換する（`fandhe-core` は `fandhe-device` に依存しないため、両者の型を橋渡しする
/// のは本クレート側の責務）。`DeviceError` の内部構造（`std::io::Error` 等）は
/// `fandhe-core` 側へ漏らさず、`Display` の文言のみを転送する
/// （`DeviceError` の内部構造を漏らさない、security.md A09 準拠）。
/// アプリ終了時に未移行の端末リソースを解放する（最終成果物（`fandhe-gallery`）の `register_teardown_hooks` 経由で `ProxyApp::on_exit` が呼ぶ契約、
/// 機能 crate の停止は gallery の終了フックが本関数の後に呼ぶ。停止の順序は「未移行 → 機能 crate」になるが、相互に独立している。
/// `fandhe-render-egui` の `app.rs` の `EguiApp::on_exit` 実装 doc 参照）。プロセスが終了すると
/// `static` の `Drop` は走らないため、生きている間に明示停止が要るリソースを
/// ここへ集約する。現状は TTS の子プロセス停止のみ（Issue #865 の codex-review
/// P1 指摘: 共有 TTS を `static OnceLock` で保持しているため `Drop` が実行されず、
/// ウィンドウを閉じても発話中の子プロセスが残る）。
pub fn shutdown() {
    tts::stop_shared_tts();
    location::stop_shared_location();
    bluetooth_le::stop_shared_scan();
    // 公開の停止は `.m` の専用キューへの短い同期呼び出し。Rust 側のロックは握らない。
    ble_peripheral::shutdown();
    // macOS の停止は main queue へ同期ディスパッチする。`shutdown` は eframe の `on_exit`（main thread）から
    // ロックを握らずに呼ばれるため、main 上なら直接停止し、デッドロックしない。
    bluetooth_classic::stop_shared_discovery();
    lan_discovery::stop_shared_discovery();
    local_peer::stop_shared_exchange();
    media_playback::shutdown();
    now_playing::shutdown();
    picture_in_picture::shutdown();
    web_auth_session::shutdown();
    camera_preview_stream::shutdown();
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
