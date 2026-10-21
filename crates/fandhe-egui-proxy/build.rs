//! iOS / macOS 向け ObjC 実装（`ios/*.m`・`apple/*.m`）のビルドスクリプト。
//!
//! `CARGO_CFG_TARGET_OS` が `ios`（`ios/*.m` 含む全機能）または `macos`（`apple/*.m` の
//! 位置情報のみ）の場合にのみ ObjC ファイルを `cc` でコンパイルする。それ以外の
//! ターゲット（デスクトップ Windows/Linux・wasm32・Android）では即座に return する
//! no-op で、既存ビルドへの影響はない。
//!
//! - `ios/fandhe_ios_text_input.m`（Issue #795、ソフトキーボード文字入力の `UITextField`
//!   ブリッジ）は feature に関わらず常にコンパイル・UIKit/Foundation をリンクする
//!   （`src/ios_text_input.rs` から呼ばれる契約。計測 feature `cold-start-probe`/`frame-metrics`
//!   有効時は Rust 側が外れるためコンパイルしない、Issue #19）。
//! - `ios/fandhe_ios_camera.m`（Issue #751、実カメラ実装）は feature `device`
//!   （既定 on）有効時のみコンパイルし、AVFoundation/CoreGraphics を追加でリンクする
//!   （`src/device/ios.rs` の `Camera` 実装から呼ばれる契約、無効時はリンクエラーを
//!   避けるため素通りする）。
//! - `ios/fandhe_ios_motion.m`（Issue #832、実センサ傾き実装）も feature `device`
//!   有効時のみコンパイルし、CoreMotion を追加でリンクする（`src/device/ios.rs` の
//!   `Motion` 実装から呼ばれる契約、camera と同じ条件分岐に相乗りする）。
//! - `ios/fandhe_ios_clipboard.m`（Issue #859、`UIPasteboard` 読み書き）も feature
//!   `device` 有効時のみコンパイルする（`src/device/ios.rs` の `Clipboard` 実装から
//!   呼ばれる契約）。UIKit/Foundation のみで足りるため追加リンクは無い。
//! - `ios/fandhe_ios_tts.m`（Issue #866、`AVSpeechSynthesizer` による読み上げ）も
//!   feature `device` 有効時のみコンパイルする（`src/device/ios.rs` の `Tts` 実装から
//!   呼ばれる契約）。AVFoundation は camera 用に既にリンク済みのため追加リンクは無い。
//! - `ios/fandhe_ios_file_picker.m`（Issue #873、`UIDocumentPickerViewController` に
//!   よるファイル選択）も feature `device` 有効時のみコンパイルする（`src/device/
//!   ios.rs` の `present_file_picker`/`fandhe_rust_file_picker_outcome` から
//!   呼ばれる契約）。`UTTypeData`（フォルダ除外のため `UTTypeItem` ではなくこちら。
//!   `.m` 側コメント参照）の解決に UniformTypeIdentifiers を追加でリンクする。
//! - `ios/fandhe_ios_photo_picker.m`（Issue #1104、`PHPickerViewController` による写真選択）も
//!   feature `device` 有効時のみコンパイルする（`src/device/ios.rs` の `present_photo_picker`/
//!   `fandhe_rust_photo_picker_outcome` から呼ばれる契約）。PhotosUI と、JPEG 変換用の
//!   ImageIO を追加でリンクする。
//! - `apple/fandhe_apple_background_download.m`（Issue #390、background `NSURLSession` による OS 管理の
//!   バックグラウンドダウンロード）は macOS・iOS の両分岐で feature `device` 有効時にコンパイルする
//!   （`src/device/apple_background_download.rs` から呼ばれる契約）。Foundation のみで追加のリンクは不要。
//! - `apple/fandhe_apple_file_association.m`（Issue #416、Apple Event `kAEOpenDocuments` によるファイルの
//!   関連付け受信）は macOS・iOS の両分岐で feature `device` 有効時にコンパイルする（`src/device/
//!   apple_file_association.rs` から呼ばれる契約。iOS 側は Issue #417 で実装済み）。追加のリンクは不要。
//! - `apple/fandhe_apple_now_playing.m`（Issue #381 macOS・#382 iOS。MediaPlayer の `MPNowPlayingInfoCenter` /
//!   `MPRemoteCommandCenter` による OS の「再生中」表示とメディアキー。`src/device/apple_now_playing.rs` から呼ばれる契約）。
//!   MediaPlayer をリンクする。
//! - `apple/fandhe_apple_picture_in_picture.m`（Issue #364、AVKit の sample buffer 経路による
//!   ピクチャ・イン・ピクチャ）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/
//!   apple_picture_in_picture.rs` から呼ばれる契約）。AVKit・CoreMedia・CoreVideo・QuartzCore をリンクする。
//! - `apple/fandhe_apple_media_cast.m`（Issue #386 macOS + #387 iOS、`AVRoutePickerView` と `AVAudioPlayer` による
//!   外部機器への再生）は macOS・iOS 双方の分岐で feature `device` 有効時にコンパイルする（`src/device/apple_media_cast.rs`
//!   から呼ばれる契約）。AVKit・AVFoundation をリンクし、CoreAudio は macOS のみ。
//! - `apple/fandhe_apple_location.m`（Issue #889、`CLLocationManager` による位置情報）は
//!   iOS・macOS の両ターゲットで feature `device` 有効時のみコンパイルする
//!   （`src/device/apple_location.rs` から呼ばれる契約）。CoreLocation をリンクする。
//!   macOS 側はさらに、素の実行体（`[[bin]]`）へ `NSLocationUsageDescription` を含む
//!   `macos/Info.plist` を `-sectcreate` で埋め込む（§ macOS 分岐のコメント参照。
//!   macOS 10.15+ は Info.plist を持たない実行体に CoreLocation の許可プロンプトを
//!   出さないため）。
//! - `apple/fandhe_apple_share.m`（Issue #897 macOS + #899 iOS、`NSSharingServicePicker`/
//!   `UIActivityViewController` による共有シート）は iOS・macOS の両ターゲットで
//!   feature `device` 有効時のみコンパイルする（`src/device/apple_share.rs` から
//!   呼ばれる契約）。macOS は AppKit を、iOS は UIKit（既にリンク済み）＋
//!   `ios/fandhe_ios_common.h`（`-I ios` で解決）を使う。
//! - `apple/fandhe_apple_biometric.m`（Issue #906、`LAContext` による生体認証）は
//!   iOS・macOS の両ターゲットで feature `device` 有効時のみコンパイルする
//!   （`src/device/apple_biometric.rs` から呼ばれる契約）。LocalAuthentication を
//!   リンクする。
//! - `apple/fandhe_apple_contact_picker.m`（Issue #1220、`CNContactPicker` による連絡先選択）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/
//!   apple_contact_picker.rs` から呼ばれる契約）。Contacts / ContactsUI をリンクする。
//!   iOS 側は #1221 で足す。
//! - `apple/fandhe_apple_calendar.m`（Issue #1224、`EKEventStore` による予定の追加）は
//!   iOS・macOS の両ターゲットで feature `device` 有効時にコンパイルする（`src/device/
//!   apple_calendar.rs` から呼ばれる契約）。EventKit をリンクし、iOS（Issue #1225、
//!   `EKEventEditViewController`）は EventKitUI も追加でリンクする。
//! - `ios/fandhe_ios_compose_mail_sms.m`（Issue #1385、MessageUI によるメール・SMS 作成画面）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルし MessageUI をリンクする
//!   （`src/device/ios_compose_mail_sms.rs` から呼ばれる契約）。
//! - `ios/fandhe_ios_print.m`（Issue #327、`UIPrintInteractionController` による印刷シート）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_print.rs` から
//!   呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_web_auth_session.m`（Issue #532、AuthenticationServices `ASWebAuthenticationSession` による
//!   OAuth 用の Web 認証セッション）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルし
//!   AuthenticationServices をリンクする（`src/device/ios_web_auth_session.rs` から呼ばれる契約）。
//! - `apple/fandhe_apple_on_device_llm.swift`（Issue #345、FoundationModels による端末内 LLM）は
//!   **macOS・iOS 両分岐**（iOS は Issue #346）で feature `device` 有効時に swiftc で静的ライブラリ化し、
//!   FoundationModels を弱リンクする（`src/device/apple_on_device_llm.rs` から呼ばれる契約）。
//! - `apple/fandhe_apple_translation.swift`（Issue #535、Translation による端末内翻訳）は
//!   **macOS・iOS 両分岐**（iOS の実体は #536）で feature `device` 有効時に swiftc で静的ライブラリ化し、
//!   Translation を弱リンクする（`src/device/apple_translation.rs` から呼ばれる契約）。
//!   swiftc 呼び出しは `compile_apple_swift` を on-device-llm と共用する。
//! - `apple/fandhe_apple_spotlight_index.m`（Issue #560、Core Spotlight による OS の検索への登録と、
//!   macOS の `continueUserActivity` 受信）は **macOS・iOS 両分岐**（iOS の実体は #561）で
//!   feature `device` 有効時にコンパイルし CoreSpotlight をリンクする
//!   （`src/device/apple_spotlight_index.rs` から呼ばれる契約）。
//! - `apple/fandhe_apple_map_view.m`（Issue #273、`MKMapSnapshotter` による地図の静止画 1 枚の取得。
//!   iOS 分岐も同じ実装）は `src/device/apple_map_view.rs` から呼ばれる契約。MapKit をリンクする。
//! - `apple/fandhe_apple_photo_library.m`（Issue #1254、`PHPhotoLibrary` による画像 1 枚の保存）は
//!   iOS（Issue #1255）・macOS の両ターゲットで feature `device` 有効時にコンパイルする
//!   （`src/device/apple_photo_library.rs` から呼ばれる契約）。Photos をリンクする。
//! - `apple/fandhe_apple_text_scale.m`（Issue #914、`UIFont.preferredFontForTextStyle:`
//!   による文字サイズ追従）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios.rs` の `IosTextScale` 実装から呼ばれる契約）。macOS は
//!   `fandhe_device::text_scale` モジュール doc の判断により恒久 `Unsupported` のため
//!   `build_macos` からは呼ばない。UIKit は上で既にリンク済みのため追加リンクは無い。
//! - `apple/fandhe_apple_secure_credential.m`（Issue #922・#924、Keychain Services
//!   による資格情報保存）は **iOS・macOS 双方**で feature `device` 有効時に
//!   コンパイルする（`src/device/apple_secure_credential.rs` の `AppleKeychain`
//!   実装から呼ばれる契約）。Security Services は iOS/macOS 同一 API のため分岐なしで
//!   そのまま流用できる（Issue #924 調査）。Security フレームワークをリンクする。
//! - `apple/fandhe_apple_gamepad.m`（Issue #934、`GCController` によるゲーム
//!   コントローラ入力）は iOS・macOS の両ターゲットで feature `device` 有効時のみ
//!   コンパイルする（`src/device/apple_gamepad.rs` の `AppleGamepad` 実装から
//!   呼ばれる契約）。GameController をリンクする。
//! - `apple/fandhe_apple_screen_capture.m`（Issue #1335、ScreenCaptureKit の `SCStream` +
//!   `SCRecordingOutput` による macOS 画面録画 + #1336 の iOS ReplayKit 録画）は
//!   `src/device/apple_screen_capture.rs` から呼ばれる契約。macOS は ScreenCaptureKit を弱リンク、
//!   iOS は ReplayKit をリンクする。
//! - `apple/fandhe_apple_mic_record.m`（Issue #954 macOS + #956 iOS、`AVAudioRecorder`
//!   によるマイク録音）は **iOS・macOS 双方**で feature `device` 有効時にコンパイル
//!   する（`src/device/apple_mic_record.rs` から呼ばれる契約）。AVFoundation
//!   （macOS 分岐では未リンク）・AudioToolbox（`kAudioFormatMPEG4AAC`）をリンクする。
//! - `ios/fandhe_ios_notification.m`（Issue #1005、`UNUserNotificationCenter` による
//!   ローカル通知）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios_notification.rs` から呼ばれる契約）。macOS は #1004 で
//!   `osascript` 経路（`apple/` ではなく `fandhe-device` クレート側）を使うため
//!   `build_macos` からは呼ばない。UserNotifications を追加でリンクする。
//! - `ios/fandhe_ios_app_badge.m`（Issue #1361、`UNUserNotificationCenter.setBadgeCount` による
//!   バッジ設定）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios_app_badge.rs` から呼ばれる契約）。追加リンクは無い。
//! - `ios/fandhe_ios_drag_drop.m`（Issue #1015、`UIDropInteraction` によるドラッグ &
//!   ドロップ受け取り）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios_drag_drop.rs` から呼ばれる契約）。macOS/Windows は #1014 で
//!   egui `RawInput` 経由の別実装（`fandhe-egui-proxy` クレート内、`ios/` ではなく
//!   `src/device/drag_drop.rs`）を使うため `build_macos` からは呼ばない。UIKit・
//!   UniformTypeIdentifiers は上で既にリンク済みのため追加リンクは無い。
//! - `apple/fandhe_apple_speech_recognition.m`（Issue #1024 macOS + #1025 iOS、
//!   `Speech`（`SFSpeechRecognizer`）＋ `AVAudioEngine` による音声認識）は
//!   **iOS・macOS 双方**で feature `device` 有効時にコンパイルする（`src/device/
//!   apple_speech_recognition.rs` から呼ばれる契約）。Android/Web は兄弟 issue の
//!   担当。Speech フレームワークをリンクする（AVFoundation は camera/mic_record 用に
//!   既にリンク済み）。
//! - `apple/fandhe_apple_bluetooth_le.m`（Issue #1060、CoreBluetooth による BLE スキャン）は
//!   **iOS・macOS 双方**で feature `device` 有効時にコンパイルする（`src/device/
//!   apple_bluetooth_le.rs` から呼ばれる契約。iOS は #1061）。CoreBluetooth をリンクする。
//! - `apple/fandhe_apple_lan_discovery.m`（Issue #486、`nw_browser` と dns_sd による LAN のサービス探索）は
//!   **iOS・macOS 双方**で feature `device` 有効時にコンパイルする（`src/device/apple_lan_discovery.rs`
//!   から呼ばれる契約）。Network をリンクする。
//! - `apple/fandhe_apple_local_peer.m`（Issue #511、MultipeerConnectivity による近くの端末との直接通信）は
//!   **iOS・macOS 双方**で feature `device` 有効時にコンパイルする（`src/device/apple_local_peer.rs`
//!   から呼ばれる契約。iOS は b-ios が委譲する）。MultipeerConnectivity をリンクする。
//! - `macos/fandhe_macos_background_task.m`（Issue #1043、`NSBackgroundActivityScheduler`
//!   によるバックグラウンド処理）は **macOS 分岐のみ**で feature `device` 有効時に
//!   コンパイルする（`src/device/macos_background_task.rs` の `MacBackgroundTask`
//!   実装から呼ばれる契約）。`NSBackgroundActivityScheduler` は macOS 専用 API
//!   （iOS に存在しない）のため `apple/` ではなく `macos/` に置く。Foundation は
//!   上で既にリンク済みのため追加リンクは無い。
//! - `macos/fandhe_macos_url_scheme.m`（Issue #1215、`NSAppleEventManager` の `kAEGetURL`
//!   によるカスタム URL scheme 受信）は **macOS 分岐のみ**で feature `device` 有効時に
//!   コンパイルする（`src/device/apple_url_scheme.rs` から呼ばれる契約）。AppKit・
//!   Foundation は既存のリンクで足り、CoreServices の定数は enum のため追加リンクは無い。
//! - `macos/fandhe_macos_screen_wake_lock.m`（Issue #1278、IOKit の電源アサーションによる
//!   画面スリープの抑止）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_screen_wake_lock.rs` から呼ばれる契約）。IOKit・CoreFoundation を
//!   追加でリンクする。
//! - `macos/fandhe_macos_app_shortcuts.m`（Issue #424、Dock メニュー。`class_addMethod` で delegate に
//!   `applicationDockMenu:` を足す）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_app_shortcuts.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_url_scheme.m`（Issue #1217、delegate の `application:openURL:options:` と
//!   起動通知の `launchOptions` によるカスタム URL scheme 受信）は **iOS 分岐のみ**で
//!   feature `device` 有効時にコンパイルする（`src/device/apple_url_scheme.rs` から呼ばれる
//!   契約）。UIKit・Foundation は既存のリンクで足りる。
//! - `macos/fandhe_macos_power_state.m`（Issue #1274、IOKit `IOPSCopyPowerSourcesInfo` と
//!   `NSProcessInfo` による電源状態の同期読み取り）は **macOS 分岐のみ**で feature `device`
//!   有効時にコンパイルする（`src/device/macos_power_state.rs` から呼ばれる契約）。IOKit を
//!   追加でリンクする。
//! - `apple/fandhe_apple_accessibility_prefs.m`（Issue #1308、`NSWorkspace` / `UIAccessibility` による
//!   視差軽減・コントラスト増加の同期読み取り）は iOS・macOS 双方の分岐で feature `device` 有効時に
//!   コンパイルする（`src/device/apple_accessibility_prefs.rs` から呼ばれる契約）。
//! - `macos/fandhe_macos_local_fonts.m`（Issue #1376、`NSFontManager` によるフォントのファミリー名の
//!   同期読み取り）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_local_fonts.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_file_watch.m`（Issue #1410、CoreServices の FSEventStream によるフォルダの
//!   変更監視）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_file_watch.rs` から呼ばれる契約）。CoreServices をリンクする。
//! - `macos/fandhe_macos_audio_devices.m`（Issue #1330、Core Audio による音声デバイスの列挙）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_audio_devices.rs` から呼ばれる契約）。CoreAudio を追加でリンクする。
//! - `macos/fandhe_macos_app_badge.m`（Issue #1360、`NSDockTile.badgeLabel` によるバッジ設定）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/macos_app_badge.rs`
//!   から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_in_app_review.m`（Issue #564、`SKStoreReviewController requestReviewInScene:` による
//!   アプリ内レビュー依頼。`src/device/ios_in_app_review.rs` から呼ばれる契約）。StoreKit を追加でリンクする。
//! - `macos/fandhe_macos_in_app_review.m`（Issue #563、`SKStoreReviewController requestReview` による
//!   レビュー依頼）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_in_app_review.rs` から呼ばれる契約）。StoreKit を追加でリンクする。
//! - `macos/fandhe_macos_compose_mail_sms.m`（Issue #1384、`NSWorkspace openURL:` による
//!   `mailto:` / `sms:` の作成画面表示）は **macOS 分岐のみ**で feature `device` 有効時に
//!   コンパイルする（`src/device/macos_compose_mail_sms.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_print.m`（Issue #325、`NSPrintPanel` + `NSPrintOperation` による印刷
//!   ダイアログ）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_print.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_device_info.m`（Issue #1304、`NSProcessInfo` + `sysctl` + `NSURL` による
//!   端末・OS 情報の同期読み取り）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_device_info.rs` から呼ばれる契約）。Foundation は既存のリンクで足りる。
//! - `macos/fandhe_macos_safe_area_insets.m`（Issue #444、窓の `contentView.safeAreaInsets` の同期読み取り）は
//!   macOS ターゲットの feature `device` 有効時のみ `cc` でコンパイルする
//!   （`src/device/macos_safe_area_insets.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_system_accent_color.m`（Issue #421、`NSColor.controlAccentColor` の同期読み取り）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_system_accent_color.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_music_library.m`（Issue #482、iTunesLibrary `ITLibrary` による曲のタイトルと
//!   アーティストの先頭 20 件の読み取り）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルし
//!   iTunesLibrary をリンクする（`src/device/macos_music_library.rs` から呼ばれる契約）。
//! - `macos/fandhe_macos_web_auth_session.m`（Issue #530、AuthenticationServices `ASWebAuthenticationSession` による
//!   OAuth 用の Web 認証セッション）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルし
//!   AuthenticationServices をリンクする（`src/device/macos_web_auth_session.rs` から呼ばれる契約）。
//! - `macos/fandhe_macos_wifi_info.m`（Issue #514、CoreWLAN `CWWiFiClient` による現在の接続・直近のスキャン結果の
//!   読み取りと、CoreLocation の許可判定）は **macOS 分岐のみ**で feature `device` 有効時にコンパイルし
//!   CoreWLAN をリンクする（`src/device/macos_wifi_info.rs` から呼ばれる契約）。
//! - `macos/fandhe_macos_usb_device.m`（Issue #505、IOKit `IOUSBHostDevice` による USB 機器の
//!   ベンダー ID・製品 ID・製品名の読み取り）は **macOS 分岐のみ**で feature `device` 有効時に
//!   コンパイルし IOKit をリンクする（`src/device/macos_usb_device.rs` から呼ばれる契約）。
//! - `macos/fandhe_macos_multi_display.m`（Issue #1341、`NSScreen.screens` による画面一覧の同期読み取り）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_multi_display.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `macos/fandhe_macos_pen_stylus.m`（Issue #355、`NSEvent` の local monitor によるペンの種類・筆圧・傾き）は
//!   **macOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/macos_pen_stylus.rs` から呼ばれる契約）。AppKit は既存のリンクで足りる。
//! - `apple/fandhe_apple_network_status.m`（Issue #1282、`NWPathMonitor` によるネットワーク接続状態）は
//!   iOS・macOS 双方の分岐で feature `device` 有効時にコンパイルする
//!   （Issue #1283。`src/device/apple_network_status.rs` から呼ばれる契約）。Network をリンクする。
//! - `ios/fandhe_ios_pedometer.m`（Issue #1248、`CMPedometer` による歩数の 1 回取得）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_pedometer.rs`
//!   から呼ばれる契約）。CoreMotion は motion 用にリンク済みで追加リンクは無い。
//! - `ios/fandhe_ios_music_library.m`（Issue #484、MediaPlayer `MPMediaQuery` による曲のタイトルとアーティスト）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_music_library.rs`
//!   から呼ばれる契約）。MediaPlayer は Now Playing 用にリンク済みで追加リンクは無い。
//! - `ios/fandhe_ios_screen_wake_lock.m`（Issue #1279、`UIApplication.idleTimerDisabled` による
//!   画面スリープの抑止）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios_screen_wake_lock.rs` から呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_fullscreen_immersive.m`（Issue #1313、winit の ViewController へ
//!   `prefersStatusBarHidden` を差し込む全画面表示）は **iOS 分岐のみ**で feature `device` 有効時に
//!   コンパイルする（`src/device/ios_fullscreen_immersive.rs` から呼ばれる契約）。UIKit と objc
//!   ランタイムは既存のリンクで足りる。
//! - `ios/fandhe_ios_power_state.m`（Issue #1275、`UIDevice` と `NSProcessInfo` による電源状態）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_power_state.rs`
//!   から呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_device_info.m`（Issue #1305、`NSProcessInfo` と `sysctl` による端末・OS 情報）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_device_info.rs`
//!   から呼ばれる契約）。Foundation は既存のリンクで足りる。
//! - `ios/fandhe_ios_safe_area_insets.m`（Issue #445、key window の `safeAreaInsets` の同期読み取り）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_safe_area_insets.rs`
//!   から呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_audio_devices.m`（Issue #1331、`AVAudioSession` の読み取りによる音声デバイス一覧）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_audio_devices.rs`
//!   から呼ばれる契約）。AVFoundation は既存のリンクで足りる。
//! - `ios/fandhe_ios_local_fonts.m`（Issue #1377、UIKit `UIFont.familyNames` によるフォントのファミリー名）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_local_fonts.rs`
//!   から呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_compass.m`（Issue #1405、CoreLocation の heading 購読による方位）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_compass.rs`
//!   から呼ばれる契約）。CoreLocation は位置情報用にリンク済みで追加リンクは無い。
//! - `ios/fandhe_ios_flashlight.m`（Issue #460、AVFoundation の torch によるライト）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_flashlight.rs`
//!   から呼ばれる契約）。AVFoundation はカメラ用にリンク済みで追加リンクは無い。
//! - `ios/fandhe_ios_pen_stylus.m`（Issue #356、観測専用 `UIGestureRecognizer` による種類・筆圧・傾き）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_pen_stylus.rs` から
//!   呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_ar.m`（Issue #308、ARKit の `ARSession` による追跡状態・位置・水平面の数）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_ar.rs` から呼ばれる契約）。
//!   ARKit を追加でリンクする。
//! - `ios/fandhe_ios_file_watch.m`（Issue #1412、`open(O_EVTONLY)` + `DISPATCH_SOURCE_TYPE_VNODE` による
//!   フォルダの変更監視）は **iOS 分岐のみ**で feature `device` 有効時にコンパイルする
//!   （`src/device/ios_file_watch.rs` から呼ばれる契約）。libdispatch は libSystem 内で追加リンクは無い。
//! - `ios/fandhe_ios_call_ui.m`（Issue #277、`CXProvider`（CallKit）による疑似着信）は **iOS 分岐のみ**で
//!   feature `device` 有効時にコンパイルする（`src/device/ios_call_ui.rs` から呼ばれる契約）。
//!   CallKit を追加でリンクする。
//! - `ios/fandhe_ios_live_activity.swift` + `ios/fandhe_live_activity_attributes.swift`（Issue #295、
//!   ActivityKit によるライブアクティビティ）は **iOS 分岐のみ**で feature `device` 有効時に swiftc で
//!   静的ライブラリ化する（`src/device/ios_live_activity.rs` から呼ばれる契約）。ActivityKit と
//!   Swift ランタイムの検索パスを追加でリンクする。
//! - `ios/fandhe_ios_scene.m`（Issue #27、UIScene 化シム）は **iOS 分岐のみ**で feature・計測ビルドに
//!   関係なく常にコンパイルする（Info.plist の `UIApplicationSceneManifest` が参照するため）。
//! - `ios/fandhe_ios_multi_display.m`（Issue #27、scene の `UIScreen` による複数ディスプレイ）は
//!   **iOS 分岐のみ**で feature `device` 有効時にコンパイルする（`src/device/ios_multi_display.rs`
//!   から呼ばれる契約）。UIKit は既存のリンクで足りる。
//! - `ios/fandhe_ios_background_task.m`（Issue #1045、`BackgroundTasks`
//!   （`BGTaskScheduler`）によるバックグラウンド処理）は **iOS 分岐のみ**で
//!   feature `device` 有効時にコンパイルする（`src/device/ios_background_task.rs`
//!   の `IosBackgroundTask` 実装から呼ばれる契約）。`BGTaskScheduler` は
//!   macOS 側の `NSBackgroundActivityScheduler`（#1043）とは別 API のため
//!   `macos/` ではなく `ios/` に置く。BackgroundTasks を追加でリンクする。

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        build_macos();
        return;
    }
    if target_os != "ios" {
        return;
    }

    // Issue #27: UIScene 化シム。Info.plist の `UIApplicationSceneManifest` はビルド種別に関係なく
    // 1 つのため、計測ビルド・feature `device` 無効でも常にコンパイルする。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_scene.m");
    cc::Build::new()
        .file("ios/fandhe_ios_scene.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_scene");

    // 計測専用ビルドでは `src/ios_text_input.rs`（`#[no_mangle]` の受け口）が `lib.rs` の
    // cfg で外れるため、呼び出し側の `.m` も同じ条件で外す（Issue #19）。
    let measure_build = std::env::var("CARGO_FEATURE_COLD_START_PROBE").is_ok()
        || std::env::var("CARGO_FEATURE_FRAME_METRICS").is_ok();
    if !measure_build {
        println!("cargo:rerun-if-changed=ios/fandhe_ios_text_input.m");
        cc::Build::new()
            .file("ios/fandhe_ios_text_input.m")
            .flag("-fobjc-arc")
            .compile("fandhe_ios_text_input");
    }

    println!("cargo:rustc-link-lib=framework=UIKit");
    println!("cargo:rustc-link-lib=framework=Foundation");

    // feature `device`（既定 on）でのみ `fandhe-device` への依存自体が有効になる
    // （`Cargo.toml` の native 分岐参照）。無効時に ObjC をコンパイルしても
    // 呼び出し元（`src/device/ios.rs`）がビルドされないため、リンクエラーを
    // 避けるべく feature 無効時は素通りする。
    if std::env::var("CARGO_FEATURE_DEVICE").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=ios/fandhe_ios_camera.m");
    // `fandhe_ios_camera.m` が `#import` する共有ヘッダ。ヘッダのみの変更でも
    // 再コンパイルされるよう明示する（ヘッダは `cc` の依存追跡に乗らないため）。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_camera.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_camera");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");

    println!("cargo:rerun-if-changed=ios/fandhe_ios_motion.m");
    cc::Build::new()
        .file("ios/fandhe_ios_motion.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_motion");

    println!("cargo:rustc-link-lib=framework=CoreMotion");

    // `ios/fandhe_ios_clipboard.m`（Issue #859、`UIPasteboard` 読み書き）も同じ
    // feature 条件に相乗りする。UIKit/Foundation は上で既にリンク済みのため
    // 追加のフレームワーク指定は不要。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_clipboard.m");
    cc::Build::new()
        .file("ios/fandhe_ios_clipboard.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_clipboard");

    // `ios/fandhe_ios_tts.m`（Issue #866、`AVSpeechSynthesizer` による読み上げ）も
    // 同じ feature 条件に相乗りする。AVFoundation は camera 用に上で既にリンク済み
    // のため追加のフレームワーク指定は不要。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_tts.m");
    cc::Build::new()
        .file("ios/fandhe_ios_tts.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_tts");

    // `ios/fandhe_ios_file_picker.m`（Issue #873、`UIDocumentPickerViewController`
    // によるファイル選択）も同じ feature 条件に相乗りする。UIKit/Foundation は
    // 上で既にリンク済みのため、`UTTypeData`（フォルダ除外のため `UTTypeItem` では
    // なくこちら。`.m` 側コメント参照）解決用の UniformTypeIdentifiers のみ
    // 追加でリンクする。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_file_picker.m");
    // `fandhe_ios_file_picker.m` が `#import` する共有ヘッダ（camera 分岐と同じ理由）。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_file_picker.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_file_picker");

    println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");

    // `ios/fandhe_ios_photo_picker.m`（Issue #1104、`PHPickerViewController`）も同じ feature
    // 条件に相乗りする。PhotosUI と、HEIC 等を JPEG へ変換する ImageIO を追加でリンクする
    // （UIKit/Foundation/UniformTypeIdentifiers は上でリンク済み）。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_photo_picker.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_photo_picker.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_photo_picker");
    println!("cargo:rustc-link-lib=framework=PhotosUI");
    println!("cargo:rustc-link-lib=framework=ImageIO");

    // `ios/fandhe_ios_notification.m`（Issue #1005、`UNUserNotificationCenter`
    // によるローカル通知）も同じ feature 条件に相乗りする（`src/device/
    // ios_notification.rs` から呼ばれる契約）。UserNotifications を追加でリンクする。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_notification.m");
    cc::Build::new()
        .file("ios/fandhe_ios_notification.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_notification");

    println!("cargo:rustc-link-lib=framework=UserNotifications");

    // `ios/fandhe_ios_app_badge.m`（Issue #1361、`setBadgeCount` によるバッジ設定）も同じ
    // feature 条件に相乗りする（`src/device/ios_app_badge.rs` から呼ばれる契約）。
    // UserNotifications は上で既にリンク済みのため追加リンクは無い。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_app_badge.m");
    cc::Build::new()
        .file("ios/fandhe_ios_app_badge.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_app_badge");

    // `ios/fandhe_ios_drag_drop.m`（Issue #1015、`UIDropInteraction` によるドラッグ &
    // ドロップ受け取り）も同じ feature 条件に相乗りする（`src/device/
    // ios_drag_drop.rs` から呼ばれる契約）。UIKit は上で既にリンク済み、
    // UniformTypeIdentifiers（`UTTypeData`/`UTTypeDirectory` の判定）も
    // file_picker 用に上で既にリンク済みのため追加リンクは無い。共有ヘッダ
    // （`fandhe_ios_common.h`）の変更でも再コンパイルされるよう明示する
    // （camera/file_picker 分岐と同じ理由）。
    println!("cargo:rerun-if-changed=ios/fandhe_ios_drag_drop.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_drag_drop.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_drag_drop");

    // `apple/fandhe_apple_location.m`（Issue #889、`CLLocationManager` による位置情報）
    // は macOS 分岐（`build_macos`）と共用のためコンパイル呼び出しを共通化する。
    compile_apple_location();

    // `apple/fandhe_apple_share.m`（Issue #897 macOS + #899 iOS）も macOS 分岐と共用
    // する。iOS 側は AppKit をリンクしない（`compile_apple_share(false)`、UIKit は
    // 上で既にリンク済み）。
    compile_apple_share(false);

    // `apple/fandhe_apple_biometric.m`（Issue #906）も macOS 分岐と共用する。
    compile_apple_biometric();
    compile_apple_contact_picker();

    // `apple/fandhe_apple_text_scale.m`（Issue #914）は iOS 分岐のみから呼ぶ
    // （`build_macos` からは呼ばない、上記 doc 参照）。
    compile_apple_text_scale();

    // `apple/fandhe_apple_secure_credential.m`（Issue #922・#924）は macOS 分岐と
    // 共用する（上記 doc 参照）。
    compile_apple_secure_credential();

    // `apple/fandhe_apple_gamepad.m`（Issue #934）は macOS 分岐と共用する
    // （上記 doc 参照）。
    compile_apple_gamepad();

    // `apple/fandhe_apple_calendar.m`（Issue #1224 macOS + #1225 iOS）も macOS 分岐と共用する。
    compile_apple_calendar(true);

    // `apple/fandhe_apple_photo_library.m`（Issue #1254 macOS + #1255 iOS）も macOS 分岐と共用する。
    compile_apple_photo_library();

    // `apple/fandhe_apple_map_view.m`（Issue #273 macOS + #274 iOS）も macOS 分岐と共用する。
    compile_apple_map_view();

    // `apple/fandhe_apple_mic_record.m`（Issue #954 macOS + #956 iOS）も macOS 分岐と
    // 共用する（上記 doc 参照）。
    compile_apple_mic_record();

    // `apple/fandhe_apple_speech_recognition.m`（Issue #1024 macOS + #1025 iOS）も
    // macOS 分岐と共用する（上記 doc 参照）。
    compile_apple_speech_recognition();

    // `apple/fandhe_apple_bluetooth_le.m`（Issue #1060 macOS + #1061 iOS）も macOS 分岐と
    // 共用する（上記 doc 参照）。
    compile_apple_bluetooth_le();
    // `apple/fandhe_apple_lan_discovery.m`（Issue #486 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。Network をリンクする。
    compile_apple_lan_discovery();
    // `apple/fandhe_apple_local_peer.m`（Issue #511 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。MultipeerConnectivity をリンクする。
    compile_apple_local_peer();

    // `apple/fandhe_apple_haptics.m`（Issue #1091 macOS + #1092 iOS）も macOS 分岐と
    // 共用する（上記 doc 参照）。
    compile_apple_haptics(false);
    compile_apple_nlp();
    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も iOS 分岐と共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_ml_inference.m`（Issue #289 macOS + b-ios iOS）も共用する。
    compile_apple_ml_inference();
    // `apple/fandhe_apple_media_transcode.m`（Issue #311 macOS + b-ios iOS）も共用する。
    compile_apple_media_transcode();
    // `apple/fandhe_apple_pdf_viewer.m`（Issue #469 macOS + Issue #471 iOS）も共用する。
    compile_apple_pdf_viewer();
    // `apple/fandhe_apple_on_device_llm.swift`（Issue #345 macOS + #346 iOS）も共用する。
    compile_apple_on_device_llm(true);
    // `apple/fandhe_apple_translation.swift`（Issue #535 macOS + #536 iOS）も共用する。
    compile_apple_translation(true);
    // `apple/fandhe_apple_spotlight_index.m`（Issue #560 macOS + #561 iOS）も共用する。
    compile_apple_spotlight_index();

    // `ios/fandhe_ios_background_task.m`（Issue #1045）は iOS 分岐のみから呼ぶ
    // （`build_macos` からは呼ばない、上記 doc 参照）。
    compile_ios_background_task();

    // `ios/fandhe_ios_url_scheme.m`（Issue #1217）。iOS 分岐のみ（macOS は別 .m）。
    compile_ios_url_scheme();

    // `ios/fandhe_ios_pedometer.m`（Issue #1248）。macOS は対応表で `no` のため iOS 分岐のみ。
    compile_ios_pedometer();
    compile_ios_screen_wake_lock();
    // `ios/fandhe_ios_fullscreen_immersive.m`（Issue #1313）。iOS 分岐のみ。
    compile_ios_fullscreen_immersive();
    // `ios/fandhe_ios_power_state.m`（Issue #1275）。iOS 分岐のみ。
    compile_ios_power_state();
    // `ios/fandhe_ios_compose_mail_sms.m`（Issue #1385）。iOS 分岐のみ。
    compile_ios_compose_mail_sms();
    // `ios/fandhe_ios_print.m`（Issue #327）。iOS 分岐のみ。
    compile_ios_print();
    // `ios/fandhe_ios_device_info.m`（Issue #1305）。iOS 分岐のみ。
    compile_ios_device_info();
    // `ios/fandhe_ios_safe_area_insets.m`（Issue #445）。iOS 分岐のみ。
    compile_ios_safe_area_insets();
    // `ios/fandhe_ios_audio_devices.m`（Issue #1331）。iOS 分岐のみ。
    compile_ios_audio_devices();
    // `ios/fandhe_ios_local_fonts.m`（Issue #1377）。iOS 分岐のみ。
    compile_ios_local_fonts();
    // `ios/fandhe_ios_music_library.m`（Issue #484）。iOS 分岐のみ。
    compile_ios_music_library();
    // `ios/fandhe_ios_compass.m`（Issue #1405）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_compass();
    // `ios/fandhe_ios_flashlight.m`（Issue #460）。iOS 分岐のみ（macOS は対応表で n-a）。
    compile_ios_flashlight();
    // `ios/fandhe_ios_pen_stylus.m`（Issue #356）。iOS 分岐のみ。
    compile_ios_pen_stylus();
    // `ios/fandhe_ios_ar.m`（Issue #308）。iOS 分岐のみ。
    compile_ios_ar();
    // `ios/fandhe_ios_file_watch.m`（Issue #1412）。iOS 分岐のみ。
    compile_ios_file_watch();
    // `ios/fandhe_ios_multi_display.m`（Issue #27）。iOS 分岐のみ。
    compile_ios_multi_display();
    // `ios/fandhe_ios_call_ui.m`（Issue #277）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_call_ui();
    // `ios/fandhe_ios_live_activity.swift`（Issue #295）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_live_activity();
    // `ios/fandhe_ios_web_auth_session.m`（Issue #532）。iOS 分岐のみ。
    compile_ios_web_auth_session();
    // `ios/fandhe_ios_in_app_review.m`（Issue #564）。iOS 分岐のみ。StoreKit をリンクする。
    compile_ios_in_app_review();
    // `apple/fandhe_apple_network_status.m`（Issue #1282 macOS + #1283 iOS）も共用する。
    compile_apple_network_status();
    // `apple/fandhe_apple_midi.m`（Issue #1398 macOS + #1399 iOS）も共用する。CoreMIDI を使う。
    compile_apple_midi();
    // `apple/fandhe_apple_media_playback.m`（Issue #267）。macOS と iOS（親 #86 の b-ios）で共用する。
    compile_apple_media_playback();
    // `apple/fandhe_apple_picture_in_picture.m`（Issue #365）。macOS 分岐と共用する。AVKit を使う。
    compile_apple_picture_in_picture(true);
    // `apple/fandhe_apple_background_download.m`（Issue #390）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_download();
    // `apple/fandhe_apple_file_association.m`（Issue #416）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_file_association();
    // `apple/fandhe_apple_media_cast.m`（Issue #387）。macOS 分岐と共用する。AVKit を使う。
    compile_apple_media_cast(true);
    // `apple/fandhe_apple_now_playing.m`（Issue #381 macOS + #382 iOS）。macOS 分岐と共用する。MediaPlayer を使う。
    compile_apple_now_playing();
    // `apple/fandhe_apple_accessibility_prefs.m`（Issue #1308 macOS + #1309 iOS）も共用する。
    compile_apple_accessibility_prefs();
    // `apple/fandhe_apple_screen_capture.m`（Issue #1336）の iOS 側（ReplayKit）。
    compile_apple_screen_capture(true);
}

/// `apple/fandhe_apple_location.m` を `cc` でコンパイルし CoreLocation をリンクする
/// （iOS・macOS 双方の feature `device` 分岐から呼ばれる共通処理）。
fn compile_apple_location() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_location.m");
    cc::Build::new()
        .file("apple/fandhe_apple_location.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_location");

    println!("cargo:rustc-link-lib=framework=CoreLocation");
}

/// `apple/fandhe_apple_share.m` を `cc` でコンパイルする（Issue #897 macOS + #899
/// iOS）。iOS 分岐（`link_appkit: false`）は `fandhe_ios_common.h`（`ios/` 配下）を
/// `#import` するため `-I ios` を追加する。macOS 分岐（`link_appkit: true`）は
/// AppKit を追加でリンクする（UIKit と異なりまだリンクされていないため）。
fn compile_apple_share(link_appkit: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_share.m");
    let mut build = cc::Build::new();
    build.file("apple/fandhe_apple_share.m").flag("-fobjc-arc");
    if !link_appkit {
        // iOS 分岐でのみ `fandhe_ios_common.h` を `#import` する（doc 参照）。
        // ヘッダのみの変更でも再コンパイルされるよう明示する。
        println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
        build.include("ios");
    }
    build.compile("fandhe_apple_share");

    if link_appkit {
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
}

/// `apple/fandhe_apple_biometric.m` を `cc` でコンパイルし LocalAuthentication を
/// リンクする（iOS・macOS 双方の feature `device` 分岐から呼ばれる共通処理、Issue #906）。
fn compile_apple_biometric() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_biometric.m");
    cc::Build::new()
        .file("apple/fandhe_apple_biometric.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_biometric");

    println!("cargo:rustc-link-lib=framework=LocalAuthentication");
}

/// `apple/fandhe_apple_contact_picker.m` を `cc` でコンパイルし Contacts / ContactsUI を
/// リンクする（Issue #1220 macOS + #1221 iOS）。
/// iOS 側は `fandhe_ios_common.h` を `#import` するため `-I ios` を足す（macOS では無害）。
fn compile_apple_contact_picker() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_contact_picker.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("apple/fandhe_apple_contact_picker.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_contact_picker");

    println!("cargo:rustc-link-lib=framework=Contacts");
    println!("cargo:rustc-link-lib=framework=ContactsUI");
}

/// `apple/fandhe_apple_spotlight_index.m` を `cc` でコンパイルし CoreSpotlight をリンクする
/// （Issue #560 macOS・#561 iOS。両分岐から呼ぶ）。
fn compile_apple_spotlight_index() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_spotlight_index.m");
    cc::Build::new()
        .file("apple/fandhe_apple_spotlight_index.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_spotlight_index");

    println!("cargo:rustc-link-lib=framework=CoreSpotlight");
}

/// `apple/fandhe_apple_photo_library.m` を `cc` でコンパイルし Photos をリンクする
/// （Issue #1254 macOS・#1255 iOS。両分岐から呼ぶ）。
fn compile_apple_photo_library() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_photo_library.m");
    cc::Build::new()
        .file("apple/fandhe_apple_photo_library.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_photo_library");

    println!("cargo:rustc-link-lib=framework=Photos");
}

/// `apple/fandhe_apple_map_view.m` を `cc` でコンパイルし MapKit / CoreGraphics をリンクする
/// （Issue #273 macOS・#274 iOS。両分岐から呼ぶ）。
fn compile_apple_map_view() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_map_view.m");
    cc::Build::new()
        .file("apple/fandhe_apple_map_view.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_map_view");

    println!("cargo:rustc-link-lib=framework=MapKit");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
}

/// `apple/fandhe_apple_calendar.m` を `cc` でコンパイルし EventKit をリンクする
/// （Issue #1224 macOS + #1225 iOS）。iOS 分岐（`ios: true`）は `fandhe_ios_common.h` を
/// `#import` するため `-I ios` を足し、EventKitUI も追加でリンクする。
fn compile_apple_calendar(ios: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_calendar.m");
    let mut build = cc::Build::new();
    build
        .file("apple/fandhe_apple_calendar.m")
        .flag("-fobjc-arc");
    if ios {
        println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
        build.include("ios");
    }
    build.compile("fandhe_apple_calendar");

    println!("cargo:rustc-link-lib=framework=EventKit");
    if ios {
        println!("cargo:rustc-link-lib=framework=EventKitUI");
    }
}

/// `apple/fandhe_apple_text_scale.m` を `cc` でコンパイルする（Issue #914）。iOS 分岐
/// のみから呼ばれる（doc 参照）。`TARGET_OS_IPHONE` ガードで中身を囲んでいるため、
/// ファイル自体は共有形式のまま iOS 専用のコンパイル対象にできる。UIKit は既に
/// リンク済みのため追加リンクは無い。
fn compile_apple_text_scale() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_text_scale.m");
    cc::Build::new()
        .file("apple/fandhe_apple_text_scale.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_text_scale");
}

/// `apple/fandhe_apple_secure_credential.m` を `cc` でコンパイルし Security を
/// リンクする（Issue #922・#924）。iOS・macOS 双方の feature `device` 分岐から
/// 呼ばれる共通処理（doc 参照）。
fn compile_apple_secure_credential() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_secure_credential.m");
    cc::Build::new()
        .file("apple/fandhe_apple_secure_credential.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_secure_credential");

    println!("cargo:rustc-link-lib=framework=Security");
}

/// `apple/fandhe_apple_gamepad.m` を `cc` でコンパイルし GameController を
/// リンクする（iOS・macOS 双方の feature `device` 分岐から呼ばれる共通処理、
/// Issue #934）。
fn compile_apple_gamepad() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_gamepad.m");
    cc::Build::new()
        .file("apple/fandhe_apple_gamepad.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_gamepad");

    println!("cargo:rustc-link-lib=framework=GameController");
}

/// `apple/fandhe_apple_screen_capture.m` を `cc` でコンパイルする（Issue #1335）。macOS 分岐（`ios == false`）と
/// iOS 分岐（Issue #1336、`ios == true`）から呼ばれる。macOS は ScreenCaptureKit（macOS 12.3+、`SCRecordingOutput`
/// は 15+）は実行体の `MACOSX_DEPLOYMENT_TARGET`（11.0）より新しいため**弱リンク**にする。
/// `rustc-link-arg-bins` ではなく `rustc-link-arg` を使う: cdylib とテスト実行体のリンクでも
/// `_OBJC_CLASS_$_SCStream` を解決する必要があるため（本クレートに下流の依存クレートは無い）。
fn compile_apple_screen_capture(ios: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_screen_capture.m");
    cc::Build::new()
        .file("apple/fandhe_apple_screen_capture.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_screen_capture");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    if ios {
        // iOS は ReplayKit（iOS 14+ の API は .m 側で @available ガード）。SCK の弱リンクは持ち込まない。
        println!("cargo:rustc-link-lib=framework=ReplayKit");
    } else {
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
    }
}

/// `apple/fandhe_apple_mic_record.m` を `cc` でコンパイルし AVFoundation・
/// AudioToolbox をリンクする（Issue #954 macOS + #956 iOS）。iOS・macOS 双方の
/// feature `device` 分岐から呼ばれる共通処理（doc 参照）。
fn compile_apple_mic_record() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_mic_record.m");
    cc::Build::new()
        .file("apple/fandhe_apple_mic_record.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_mic_record");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
}

/// `apple/fandhe_apple_system_tray.m` を `cc` でコンパイルする（Issue #1039、
/// macOS 専用）。`NSStatusBar`/`NSMenu` は AppKit（`compile_apple_share(true)` が
/// macOS 分岐で既にリンク済み）のみで足りるため追加リンクは無い。
fn compile_apple_system_tray() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_system_tray.m");
    cc::Build::new()
        .file("apple/fandhe_apple_system_tray.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_system_tray");
}

/// `apple/fandhe_apple_speech_recognition.m` を `cc` でコンパイルし Speech を
/// リンクする（Issue #1024 macOS + #1025 iOS）。iOS・macOS 双方の feature `device`
/// 分岐から呼ばれる共通処理（AVFoundation は上で既にリンク済み、doc 参照）。
fn compile_apple_speech_recognition() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_speech_recognition.m");
    cc::Build::new()
        .file("apple/fandhe_apple_speech_recognition.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_speech_recognition");

    println!("cargo:rustc-link-lib=framework=Speech");
}

/// `apple/fandhe_apple_haptics.m` を `cc` でコンパイルする（Issue #1091 macOS + #1092 iOS、
/// 触覚フィードバック）。iOS・macOS 双方の feature `device` 分岐から呼ぶ。macOS は AppKit、
/// iOS は CoreHaptics（`supportsHaptics` 判定）をリンクする（UIKit は iOS 分岐で既にリンク済み）。
/// Rust 側 cfg（`device.rs` の `apple_haptics`）と同じ OS 集合に保つ。
fn compile_apple_haptics(link_appkit: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_haptics.m");
    cc::Build::new()
        .file("apple/fandhe_apple_haptics.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_haptics");

    if link_appkit {
        println!("cargo:rustc-link-lib=framework=AppKit");
    } else {
        println!("cargo:rustc-link-lib=framework=CoreHaptics");
    }
}

/// `apple/fandhe_apple_nlp.m` を `cc` でコンパイルし NaturalLanguage をリンクする
/// （Issue #1094 macOS + #1095 iOS、単語分割）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_nlp`）と同じ OS 集合に保つ。
fn compile_apple_nlp() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_nlp.m");
    cc::Build::new()
        .file("apple/fandhe_apple_nlp.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_nlp");
    println!("cargo:rustc-link-lib=framework=NaturalLanguage");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

/// `apple/fandhe_apple_image_recognition.m` を `cc` でコンパイルし Vision をリンクする
/// （Issue #1250 macOS、iOS は #1252 が利用、画像の文字認識）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_image_recognition`）と
/// 同じ OS 集合に保つ。
fn compile_apple_image_recognition() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_image_recognition.m");
    cc::Build::new()
        .file("apple/fandhe_apple_image_recognition.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_image_recognition");
    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

/// `apple/fandhe_apple_pdf_viewer.m` を `cc` でコンパイルし CoreGraphics をリンクする
/// （Issue #469 macOS、iOS は親 #107 の b-ios が利用、PDF の 1 ページ目の画像化）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_pdf_viewer`）と
/// 同じ OS 集合に保つ。
fn compile_apple_pdf_viewer() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_pdf_viewer.m");
    cc::Build::new()
        .file("apple/fandhe_apple_pdf_viewer.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_pdf_viewer");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
}

/// `apple/fandhe_apple_media_transcode.m` を `cc` でコンパイルし AVFoundation をリンクする
/// （Issue #311 macOS、iOS は b-ios が利用、音声の AAC 変換）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_media_transcode`）と
/// 同じ OS 集合に保つ。
fn compile_apple_media_transcode() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_media_transcode.m");
    cc::Build::new()
        .file("apple/fandhe_apple_media_transcode.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_media_transcode");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

/// `apple/fandhe_apple_ml_inference.m` を `cc` でコンパイルし CoreML をリンクする
/// （Issue #289 macOS、iOS は b-ios が利用、機械学習モデル推論）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_ml_inference`）と
/// 同じ OS 集合に保つ。`MLModelAsset`（macOS 13 / iOS 16）は `.m` の `@available` で守る。
fn compile_apple_ml_inference() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_ml_inference.m");
    cc::Build::new()
        .file("apple/fandhe_apple_ml_inference.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_ml_inference");
    println!("cargo:rustc-link-lib=framework=CoreML");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

/// Swift 薄層 1 本ぶんのビルド指定（`compile_apple_swift` への入力）。
struct SwiftLib {
    /// `apple/` 配下の Swift ソース（`crates/fandhe-egui-proxy` からの相対パス）。
    src: &'static str,
    /// 静的ライブラリ名（`lib` 接頭辞と `.a` を除く）。
    lib: &'static str,
    /// Swift のモジュール名。
    module: &'static str,
    /// 弱リンクする framework 名（強リンクの autolink は抑止する）。
    framework: &'static str,
    /// 本番・配布ビルドでスキップを禁じる環境変数名。
    require_env: &'static str,
    /// panic・警告の接頭辞。
    tag: &'static str,
}

/// 端末内 LLM の Swift 薄層 `apple/fandhe_apple_on_device_llm.swift`（Issue #345 macOS、Issue #346 iOS）。
/// FoundationModels を弱リンクする。macOS 分岐（`false`）と iOS 分岐（`true`）の両方から呼ぶ。
fn compile_apple_on_device_llm(ios: bool) {
    compile_apple_swift(
        &SwiftLib {
            src: "apple/fandhe_apple_on_device_llm.swift",
            lib: "fandhe_apple_on_device_llm",
            module: "FandheOnDeviceLlmBridge",
            framework: "FoundationModels",
            require_env: "FANDHE_REQUIRE_ON_DEVICE_LLM",
            tag: "on_device_llm",
        },
        ios,
    );
}

/// 端末内翻訳の Swift 薄層 `apple/fandhe_apple_translation.swift`（Issue #535 macOS、iOS は #536）。
/// Translation を弱リンクする。macOS 分岐（`false`）と iOS 分岐（`true`）の両方から呼ぶ。
fn compile_apple_translation(ios: bool) {
    compile_apple_swift(
        &SwiftLib {
            src: "apple/fandhe_apple_translation.swift",
            lib: "fandhe_apple_translation",
            module: "FandheTranslationBridge",
            framework: "Translation",
            require_env: "FANDHE_REQUIRE_TRANSLATION",
            tag: "translation",
        },
        ios,
    );
}

/// Swift 薄層を `swiftc` で静的ライブラリ化し、framework を弱リンクする共通部分。`LanguageModelSession`
/// 等は Swift 専用 API で ObjC から呼べないため `cc` ではなく swiftc を使う。framework は macOS 26 /
/// iOS 26 以降のため、実行体の最低 OS（macOS 11.0）より新しく**弱リンク**にする
/// （`rustc-link-arg-bins` ではなく `rustc-link-arg`: cdylib・テスト実行体にも効かせるため）。
/// `ios` は iOS の triple 表を使う指定。
/// Apple ツール（`xcrun`）が無いホストでは警告を出してスキップする（リンクは失敗する）。
/// `spec.require_env`=1（本番・配布ビルド）ではスキップせず panic する。swiftc 自体の失敗は常に panic。
fn compile_apple_swift(spec: &SwiftLib, ios: bool) {
    use std::process::Command;

    let tag = spec.tag;
    let framework = spec.framework;
    println!("cargo:rerun-if-env-changed={}", spec.require_env);
    println!("cargo:rerun-if-changed={}", spec.src);
    let have_xcrun = Command::new("xcrun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_xcrun {
        assert!(
            std::env::var(spec.require_env).as_deref() != Ok("1"),
            "{tag}: xcrun が無い（{}=1 のためスキップ不可）",
            spec.require_env
        );
        println!("cargo:warning={tag}: xcrun が無いため Swift 薄層の生成をスキップ（型検査専用。リンクは失敗する）");
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_default();
    let (sdk, triple, lib_dir) = match (ios, target.as_str()) {
        (false, "aarch64-apple-darwin") => ("macosx", "arm64-apple-macos11.0", "macosx"),
        (false, "x86_64-apple-darwin") => ("macosx", "x86_64-apple-macos11.0", "macosx"),
        (true, "aarch64-apple-ios-sim") => (
            "iphonesimulator",
            "arm64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        (true, "aarch64-apple-ios") => ("iphoneos", "arm64-apple-ios17.0", "iphoneos"),
        (true, "x86_64-apple-ios") => (
            "iphonesimulator",
            "x86_64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        (_, other) => panic!("{tag}: 未対応のターゲット: {other}"),
    };

    let xcrun = |args: &[&str]| -> String {
        let out = Command::new("xcrun")
            .args(args)
            .output()
            .expect("xcrun を実行できない");
        assert!(out.status.success(), "xcrun {args:?} が失敗した");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let swiftc = xcrun(&["--sdk", sdk, "--find", "swiftc"]);

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let lib = format!("{out_dir}/lib{}.a", spec.lib);
    let out = Command::new("xcrun")
        .args([
            "--sdk",
            sdk,
            "swiftc",
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-O",
        ])
        .args([
            "-module-name",
            spec.module,
            "-target",
            triple,
            "-sdk",
            &sdk_path,
        ])
        // framework を強リンクとして autolink させない（macOS 11 等の旧 OS で dyld が
        // 要求して起動不能になるのを防ぐ。リンク側は下で -weak_framework を明示する）。
        .args([
            "-Xfrontend",
            "-disable-autolink-framework",
            "-Xfrontend",
            framework,
        ])
        .args(["-o", &lib])
        .arg(spec.src)
        .output()
        .expect("swiftc を実行できない");
    assert!(
        out.status.success(),
        "{tag}: swiftc が失敗した:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static={}", spec.lib);
    // SDK に framework がある場合だけ弱リンクする。無い SDK（Xcode 16 等）では Swift 側の
    // `canImport` が不成立で Unsupported 経路になり、リンクすると framework not found になる。
    if std::path::Path::new(&sdk_path)
        .join(format!("System/Library/Frameworks/{framework}.framework"))
        .exists()
    {
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,{framework}");
    }
    // Swift ランタイム（OS 同梱）の検索パスと実行時パス。
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    if let Some(bin) = std::path::Path::new(&swiftc)
        .parent()
        .and_then(|p| p.parent())
    {
        println!(
            "cargo:rustc-link-search=native={}/lib/swift/{lib_dir}",
            bin.display()
        );
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Swift の `#available` は `__isPlatformVersionAtLeast`（compiler-rt）を参照する。macOS は rustc が
    // clang_rt を自動リンクするが iOS は無いため、iOS では明示的にリンクする（Issue #346）。
    if ios {
        let resource_dir = xcrun(&["--sdk", sdk, "clang", "--print-resource-dir"]);
        let rt = if sdk == "iphoneos" { "ios" } else { "iossim" };
        println!("cargo:rustc-link-arg={resource_dir}/lib/darwin/libclang_rt.{rt}.a");
    }
}

/// `apple/fandhe_apple_accessibility_prefs.m` を `cc` でコンパイルする（Issue #1308 macOS、
/// #1309 iOS 共用）。AppKit / UIKit は各分岐の既存リンクで足りる。
fn compile_apple_accessibility_prefs() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_accessibility_prefs.m");
    cc::Build::new()
        .file("apple/fandhe_apple_accessibility_prefs.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_accessibility_prefs");
}

/// `apple/fandhe_apple_network_status.m` を `cc` でコンパイルし Network をリンクする
/// （Issue #1282 macOS、#1283 iOS 共用）。
fn compile_apple_network_status() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_network_status.m");
    cc::Build::new()
        .file("apple/fandhe_apple_network_status.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_network_status");
    println!("cargo:rustc-link-lib=framework=Network");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

/// `apple/fandhe_apple_bluetooth_le.m` を `cc` でコンパイルし CoreBluetooth をリンクする
/// （Issue #1060 macOS + #1061 iOS）。iOS・macOS 双方の feature `device` 分岐から呼ぶ。
fn compile_apple_bluetooth_le() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_bluetooth_le.m");
    cc::Build::new()
        .file("apple/fandhe_apple_bluetooth_le.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_bluetooth_le");

    println!("cargo:rustc-link-lib=framework=CoreBluetooth");
}

/// `apple/fandhe_apple_lan_discovery.m` を `cc` でコンパイルし Network をリンクする
/// （Issue #486 macOS・親 #109 の b-ios で iOS も共用）。dns_sd は libSystem に含まれる。
fn compile_apple_lan_discovery() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_lan_discovery.m");
    cc::Build::new()
        .file("apple/fandhe_apple_lan_discovery.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_lan_discovery");

    println!("cargo:rustc-link-lib=framework=Network");
}

/// `apple/fandhe_apple_local_peer.m` を `cc` でコンパイルし MultipeerConnectivity をリンクする
/// （Issue #511 macOS・親 #110 の b-ios で iOS も共用）。
fn compile_apple_local_peer() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_local_peer.m");
    cc::Build::new()
        .file("apple/fandhe_apple_local_peer.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_local_peer");

    println!("cargo:rustc-link-lib=framework=MultipeerConnectivity");
}

/// `macos/fandhe_macos_background_task.m` を `cc` でコンパイルする（Issue #1043）。
/// **macOS 分岐のみ**から呼ばれる（`NSBackgroundActivityScheduler` は macOS 専用、
/// doc 参照）。Foundation は `build_macos` 側で既にリンク済みのため追加リンクは無い。
fn compile_macos_background_task() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_background_task.m");
    cc::Build::new()
        .file("macos/fandhe_macos_background_task.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_background_task");
}

/// `macos/fandhe_macos_url_scheme.m` を `cc` でコンパイルする（Issue #1215）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/apple_url_scheme.rs` が使う）。
/// AppKit・CoreServices は `build_macos` 側の既存リンクで足りる。
fn compile_macos_url_scheme() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_url_scheme.m");
    cc::Build::new()
        .file("macos/fandhe_macos_url_scheme.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_url_scheme");
}

/// `macos/fandhe_macos_app_shortcuts.m` を `cc` でコンパイルする（Issue #424）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_app_shortcuts.rs` が使う）。AppKit・objc runtime は既存のリンクで足りる。
fn compile_macos_app_shortcuts() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_app_shortcuts.m");
    cc::Build::new()
        .file("macos/fandhe_macos_app_shortcuts.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_app_shortcuts");
}

/// `macos/fandhe_macos_screen_wake_lock.m` を `cc` でコンパイルし IOKit をリンクする
/// （Issue #1278）。**macOS 分岐のみ**から呼ばれる（`src/device/macos_screen_wake_lock.rs` が使う）。
fn compile_macos_screen_wake_lock() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_screen_wake_lock.m");
    cc::Build::new()
        .file("macos/fandhe_macos_screen_wake_lock.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_screen_wake_lock");
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}

/// `ios/fandhe_ios_url_scheme.m` を `cc` でコンパイルする（Issue #1217）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/apple_url_scheme.rs` が使う）。UIKit・Foundation は既にリンク済み。
fn compile_ios_url_scheme() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_url_scheme.m");
    cc::Build::new()
        .file("ios/fandhe_ios_url_scheme.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_url_scheme");
}

/// `apple/fandhe_apple_background_download.m` を `cc` でコンパイルする（Issue #390）。macOS・iOS 双方の
/// 分岐から呼ばれる（`src/device/apple_background_download.rs` が使う）。Foundation は既存のリンクで足りる。
fn compile_apple_background_download() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_background_download.m");
    cc::Build::new()
        .file("apple/fandhe_apple_background_download.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_background_download");
}

/// `apple/fandhe_apple_file_association.m` を `cc` でコンパイルする（Issue #416）。macOS・iOS 双方の
/// 分岐から呼ばれる（`src/device/apple_file_association.rs` が使う）。AppKit・CoreServices は macOS 分岐で
/// 既にリンク済み。iOS は scene 通知を購読する（Issue #417）。
fn compile_apple_file_association() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_file_association.m");
    cc::Build::new()
        .file("apple/fandhe_apple_file_association.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_file_association");
}

/// `apple/fandhe_apple_media_playback.m` を `cc` でコンパイルする（Issue #267）。`AVAudioPlayer` を使い、
/// AVFoundation は既存のリンクで足りる。iOS・macOS 双方の分岐から呼ばれる
/// （`src/device/apple_media_playback.rs` が使う。iOS は親 #86 の b-ios）。
fn compile_apple_media_playback() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_media_playback.m");
    cc::Build::new()
        .file("apple/fandhe_apple_media_playback.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_media_playback");
}

/// `apple/fandhe_apple_picture_in_picture.m` を `cc` でコンパイルし AVKit・CoreMedia・CoreVideo・
/// QuartzCore をリンクする（Issue #364 macOS + #365 iOS）。iOS・macOS 双方の分岐から呼ばれる（`src/device/
/// apple_picture_in_picture.rs` が使う）。macOS 12.0+ / iOS 15.0+ の API は `.m` 側の `@available` で守る。
/// `include_ios_common` が true（iOS）のとき `ios/fandhe_ios_common.h` を `#import` できるようにする。
fn compile_apple_picture_in_picture(include_ios_common: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_picture_in_picture.m");
    let mut build = cc::Build::new();
    build
        .file("apple/fandhe_apple_picture_in_picture.m")
        .flag("-fobjc-arc");
    if include_ios_common {
        println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
        build.include("ios");
    }
    build.compile("fandhe_apple_picture_in_picture");

    for framework in ["AVKit", "CoreMedia", "CoreVideo", "QuartzCore"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// `apple/fandhe_apple_now_playing.m` を `cc` でコンパイルし MediaPlayer をリンクする（Issue #381 macOS）。
/// macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_now_playing.rs` が使う）。iOS（#382）も同じ `.m` を使う。
fn compile_apple_now_playing() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_now_playing.m");
    cc::Build::new()
        .file("apple/fandhe_apple_now_playing.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_now_playing");

    println!("cargo:rustc-link-lib=framework=MediaPlayer");
}

/// `apple/fandhe_apple_media_cast.m` を `cc` でコンパイルし AVKit・AVFoundation・CoreAudio をリンクする
/// （Issue #386 macOS + #387 iOS）。macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_media_cast.rs` が使う）。
/// CoreAudio は macOS のみ。`ios` が true のとき `ios/fandhe_ios_common.h` を `#import` できるようにする。
fn compile_apple_media_cast(ios: bool) {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_media_cast.m");
    let mut build = cc::Build::new();
    build
        .file("apple/fandhe_apple_media_cast.m")
        .flag("-fobjc-arc");
    if ios {
        println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
        build.include("ios");
    }
    build.compile("fandhe_apple_media_cast");
    let frameworks: &[&str] = if ios {
        &["AVKit", "AVFoundation"]
    } else {
        &["AVKit", "AVFoundation", "CoreAudio"]
    };
    for framework in frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// `apple/fandhe_apple_midi.m` を `cc` でコンパイルし CoreMIDI をリンクする（Issue #1398）。
/// iOS・macOS 双方の分岐から呼ばれる（`src/device/apple_midi.rs` が使う。iOS は Issue #1399）。
fn compile_apple_midi() {
    println!("cargo:rerun-if-changed=apple/fandhe_apple_midi.m");
    cc::Build::new()
        .file("apple/fandhe_apple_midi.m")
        .flag("-fobjc-arc")
        .compile("fandhe_apple_midi");

    println!("cargo:rustc-link-lib=framework=CoreMIDI");
}

/// `macos/fandhe_macos_audio_devices.m` を `cc` でコンパイルする（Issue #1330）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_audio_devices.rs` が使う）。CoreAudio を追加でリンクする。
/// Foundation は `build_macos` 側の既存リンクで足りる。
fn compile_macos_audio_devices() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_audio_devices.m");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    cc::Build::new()
        .file("macos/fandhe_macos_audio_devices.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_audio_devices");
}

/// `macos/fandhe_macos_file_watch.m` を `cc` でコンパイルする（Issue #1410）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_file_watch.rs` が使う）。FSEvents のため CoreServices をリンクする。
fn compile_macos_file_watch() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_file_watch.m");
    println!("cargo:rustc-link-lib=framework=CoreServices");
    cc::Build::new()
        .file("macos/fandhe_macos_file_watch.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_file_watch");
}

/// `macos/fandhe_macos_app_badge.m` を `cc` でコンパイルする（Issue #1360）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_app_badge.rs` が使う）。AppKit は `build_macos` 側の
/// 既存リンクで足りるため追加リンクは無い。
fn compile_macos_app_badge() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_app_badge.m");
    cc::Build::new()
        .file("macos/fandhe_macos_app_badge.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_app_badge");
}

/// `macos/fandhe_macos_in_app_review.m` を `cc` でコンパイルする（Issue #563）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_in_app_review.rs` が使う）。StoreKit を追加でリンクする。
fn compile_macos_in_app_review() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_in_app_review.m");
    println!("cargo:rustc-link-lib=framework=StoreKit");
    cc::Build::new()
        .file("macos/fandhe_macos_in_app_review.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_in_app_review");
}

/// `ios/fandhe_ios_in_app_review.m` を `cc` でコンパイルする（Issue #564）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_in_app_review.rs` が使う）。StoreKit を追加でリンクする。
fn compile_ios_in_app_review() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_in_app_review.m");
    println!("cargo:rustc-link-lib=framework=StoreKit");
    cc::Build::new()
        .file("ios/fandhe_ios_in_app_review.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_in_app_review");
}

/// `macos/fandhe_macos_local_fonts.m` を `cc` でコンパイルする（Issue #1376）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_local_fonts.rs` が使う）。AppKit は `compile_apple_share(true)` が
/// 既にリンクしているため追加リンクは無い。
fn compile_macos_local_fonts() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_local_fonts.m");
    cc::Build::new()
        .file("macos/fandhe_macos_local_fonts.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_local_fonts");
}

/// `macos/fandhe_macos_wifi_info.m` を `cc` でコンパイルし CoreWLAN をリンクする（Issue #514）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_wifi_info.rs` が使う）。CoreLocation は
/// `compile_apple_location` でリンク済み。
fn compile_macos_wifi_info() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_wifi_info.m");
    cc::Build::new()
        .file("macos/fandhe_macos_wifi_info.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_wifi_info");
    println!("cargo:rustc-link-lib=framework=CoreWLAN");
}

/// `macos/fandhe_macos_music_library.m` を `cc` でコンパイルし iTunesLibrary をリンクする（Issue #482）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_music_library.rs` が使う）。
fn compile_macos_music_library() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_music_library.m");
    cc::Build::new()
        .file("macos/fandhe_macos_music_library.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_music_library");
    println!("cargo:rustc-link-lib=framework=iTunesLibrary");
}

/// `macos/fandhe_macos_web_auth_session.m` を `cc` でコンパイルし AuthenticationServices をリンクする（Issue #530）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_web_auth_session.rs` が使う）。
fn compile_macos_web_auth_session() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_web_auth_session.m");
    cc::Build::new()
        .file("macos/fandhe_macos_web_auth_session.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_web_auth_session");
    println!("cargo:rustc-link-lib=framework=AuthenticationServices");
}

/// `macos/fandhe_macos_usb_device.m` を `cc` でコンパイルし IOKit をリンクする（Issue #505）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_usb_device.rs` が使う）。
fn compile_macos_usb_device() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_usb_device.m");
    cc::Build::new()
        .file("macos/fandhe_macos_usb_device.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_usb_device");
    println!("cargo:rustc-link-lib=framework=IOKit");
}

/// `macos/fandhe_macos_compose_mail_sms.m` を `cc` でコンパイルする（Issue #1384）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_compose_mail_sms.rs` が使う）。AppKit は `build_macos` 側の
/// 既存リンクで足りるため追加リンクは無い。
fn compile_macos_compose_mail_sms() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_compose_mail_sms.m");
    cc::Build::new()
        .file("macos/fandhe_macos_compose_mail_sms.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_compose_mail_sms");
}

/// `macos/fandhe_macos_print.m` を `cc` でコンパイルする（Issue #325）。**macOS 分岐のみ**から
/// 呼ばれる（`src/device/macos_print.rs` が使う）。AppKit は `build_macos` 側の既存リンクで足りる。
fn compile_macos_print() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_print.m");
    cc::Build::new()
        .file("macos/fandhe_macos_print.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_print");
}

/// `macos/fandhe_macos_device_info.m` を `cc` でコンパイルする（Issue #1304）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_device_info.rs` が使う）。Foundation は `build_macos` 側の
/// 既存リンクで足りるため追加リンクは無い。
fn compile_macos_device_info() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_device_info.m");
    cc::Build::new()
        .file("macos/fandhe_macos_device_info.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_device_info");
}

/// `macos/fandhe_macos_safe_area_insets.m` を `cc` でコンパイルする（Issue #444）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_safe_area_insets.rs` が使う）。AppKit は既存のリンクで足りる。
fn compile_macos_safe_area_insets() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_safe_area_insets.m");
    cc::Build::new()
        .file("macos/fandhe_macos_safe_area_insets.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_safe_area_insets");
}

/// `macos/fandhe_macos_system_accent_color.m` を `cc` でコンパイルする（Issue #421）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_system_accent_color.rs` が使う）。AppKit は既存のリンクで足りる。
fn compile_macos_system_accent_color() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_system_accent_color.m");
    cc::Build::new()
        .file("macos/fandhe_macos_system_accent_color.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_system_accent_color");
}

/// `macos/fandhe_macos_multi_display.m` を `cc` でコンパイルする（Issue #1341）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_multi_display.rs` が使う）。AppKit は `build_macos` 側の
/// 既存リンクで足りるため追加リンクは無い。
fn compile_macos_multi_display() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_multi_display.m");
    cc::Build::new()
        .file("macos/fandhe_macos_multi_display.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_multi_display");
}

/// `macos/fandhe_macos_pen_stylus.m` を `cc` でコンパイルする（Issue #355）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_pen_stylus.rs` が使う）。AppKit は `build_macos` 側の
/// 既存リンクで足りるため追加リンクは無い。
fn compile_macos_pen_stylus() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_pen_stylus.m");
    cc::Build::new()
        .file("macos/fandhe_macos_pen_stylus.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_pen_stylus");
}

/// `ios/fandhe_ios_multi_display.m` を `cc` でコンパイルする（Issue #27）。**iOS 分岐のみ**から
/// 呼ばれる（`src/device/ios_multi_display.rs` が使う）。UIKit は `main` 側の既存リンクで足りる。
fn compile_ios_multi_display() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_multi_display.m");
    cc::Build::new()
        .file("ios/fandhe_ios_multi_display.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_multi_display");
}

/// `macos/fandhe_macos_power_state.m` を `cc` でコンパイルする（Issue #1274）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_power_state.rs` が使う）。IOKit（`IOPSCopyPowerSourcesInfo`）を
/// 追加でリンクする。Foundation は `build_macos` 側の既存リンクで足りる。
fn compile_macos_power_state() {
    println!("cargo:rerun-if-changed=macos/fandhe_macos_power_state.m");
    cc::Build::new()
        .file("macos/fandhe_macos_power_state.m")
        .flag("-fobjc-arc")
        .compile("fandhe_macos_power_state");
    println!("cargo:rustc-link-lib=framework=IOKit");
}

/// `ios/fandhe_ios_pedometer.m` を `cc` でコンパイルする（Issue #1248）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_pedometer.rs` が使う）。CoreMotion は motion 用に
/// 既にリンク済みのため追加リンクは無い。
fn compile_ios_pedometer() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_pedometer.m");
    cc::Build::new()
        .file("ios/fandhe_ios_pedometer.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_pedometer");
}

/// `ios/fandhe_ios_music_library.m` を `cc` でコンパイルする（Issue #484）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_music_library.rs` が使う）。MediaPlayer はリンク済みで追加リンクは無い。
fn compile_ios_music_library() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_music_library.m");
    cc::Build::new()
        .file("ios/fandhe_ios_music_library.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_music_library");
}

/// `ios/fandhe_ios_screen_wake_lock.m` を `cc` でコンパイルする（Issue #1279）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_screen_wake_lock.rs` が使う）。UIKit はリンク済み。
fn compile_ios_screen_wake_lock() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_screen_wake_lock.m");
    cc::Build::new()
        .file("ios/fandhe_ios_screen_wake_lock.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_screen_wake_lock");
}

/// `ios/fandhe_ios_fullscreen_immersive.m` を `cc` でコンパイルする（Issue #1313）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_fullscreen_immersive.rs` が使う）。`fandhe_ios_common.h` を使う。
fn compile_ios_fullscreen_immersive() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_fullscreen_immersive.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_fullscreen_immersive.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_fullscreen_immersive");
}

/// `ios/fandhe_ios_compose_mail_sms.m` を `cc` でコンパイルし MessageUI をリンクする（Issue #1385）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_compose_mail_sms.rs` が使う）。
fn compile_ios_compose_mail_sms() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_compose_mail_sms.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_compose_mail_sms.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_compose_mail_sms");
    println!("cargo:rustc-link-lib=framework=MessageUI");
}

/// `ios/fandhe_ios_web_auth_session.m` を `cc` でコンパイルし AuthenticationServices をリンクする（Issue #532）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_web_auth_session.rs` が使う）。
fn compile_ios_web_auth_session() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_web_auth_session.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_web_auth_session.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_web_auth_session");
    println!("cargo:rustc-link-lib=framework=AuthenticationServices");
}

/// `ios/fandhe_ios_print.m` を `cc` でコンパイルする（Issue #327）。**iOS 分岐のみ**から
/// 呼ばれる（`src/device/ios_print.rs` が使う）。UIKit はリンク済み。
fn compile_ios_print() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_print.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_print.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_print");
}

/// `ios/fandhe_ios_power_state.m` を `cc` でコンパイルする（Issue #1275）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_power_state.rs` が使う）。UIKit はリンク済み。
fn compile_ios_power_state() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_power_state.m");
    cc::Build::new()
        .file("ios/fandhe_ios_power_state.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_power_state");
}

/// `ios/fandhe_ios_device_info.m` を `cc` でコンパイルする（Issue #1305）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_device_info.rs` が使う）。Foundation はリンク済み。
fn compile_ios_device_info() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_device_info.m");
    cc::Build::new()
        .file("ios/fandhe_ios_device_info.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_device_info");
}

/// `ios/fandhe_ios_safe_area_insets.m` を `cc` でコンパイルする（Issue #445）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_safe_area_insets.rs` が使う）。UIKit はリンク済み。
fn compile_ios_safe_area_insets() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_safe_area_insets.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_safe_area_insets.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_safe_area_insets");
}

/// `ios/fandhe_ios_audio_devices.m` を `cc` でコンパイルする（Issue #1331）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_audio_devices.rs` が使う）。AVFoundation はリンク済み。
fn compile_ios_audio_devices() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_audio_devices.m");
    cc::Build::new()
        .file("ios/fandhe_ios_audio_devices.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_audio_devices");
}

/// `ios/fandhe_ios_local_fonts.m` を `cc` でコンパイルする（Issue #1377）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_local_fonts.rs` が使う）。UIKit はリンク済みで追加リンクは無い。
fn compile_ios_local_fonts() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_local_fonts.m");
    cc::Build::new()
        .file("ios/fandhe_ios_local_fonts.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_local_fonts");
}

/// `ios/fandhe_ios_compass.m` を `cc` でコンパイルする（Issue #1405）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_compass.rs` が使う）。CoreLocation は `compile_apple_location` が
/// 既にリンクしているため追加リンクは無い。
fn compile_ios_compass() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_compass.m");
    cc::Build::new()
        .file("ios/fandhe_ios_compass.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_compass");
}

/// `ios/fandhe_ios_flashlight.m` を `cc` でコンパイルする（Issue #460）。**iOS 分岐のみ**から
/// 呼ばれる（`src/device/ios_flashlight.rs` が使う）。AVFoundation は camera 用にリンク済み。
fn compile_ios_flashlight() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_flashlight.m");
    cc::Build::new()
        .file("ios/fandhe_ios_flashlight.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_flashlight");
}

/// `ios/fandhe_ios_pen_stylus.m` を `cc` でコンパイルする（Issue #356）。**iOS 分岐のみ**から
/// 呼ばれる（`src/device/ios_pen_stylus.rs` が使う）。UIKit は既存のリンクで足りる。
fn compile_ios_pen_stylus() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_pen_stylus.m");
    println!("cargo:rerun-if-changed=ios/fandhe_ios_common.h");
    cc::Build::new()
        .file("ios/fandhe_ios_pen_stylus.m")
        .include("ios")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_pen_stylus");
}

/// `ios/fandhe_ios_ar.m` を `cc` でコンパイルし ARKit をリンクする（Issue #308）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_ar.rs` が使う）。AVFoundation（権限確認）は既存のリンクで足りる。
fn compile_ios_ar() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_ar.m");
    println!("cargo:rustc-link-lib=framework=ARKit");
    cc::Build::new()
        .file("ios/fandhe_ios_ar.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_ar");
}

/// `ios/fandhe_ios_call_ui.m` を `cc` でコンパイルし CallKit をリンクする（Issue #277）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_call_ui.rs` が使う）。
fn compile_ios_call_ui() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_call_ui.m");
    println!("cargo:rustc-link-lib=framework=CallKit");
    cc::Build::new()
        .file("ios/fandhe_ios_call_ui.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_call_ui");
}

/// ライブアクティビティの Swift 薄層を `swiftc` で静的ライブラリ化してリンクする（Issue #295）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_live_activity.rs` が使う）。`ActivityKit` の
/// `Activity<Attributes>` は Swift 専用 API で ObjC から呼べないため、`cc` ではなく swiftc を使う。
/// Apple ツール（`xcrun`）が無いホスト（Linux の型検査経路。`docs/operations/ios-build-pipeline.md`）
/// では警告を出して静的ライブラリの生成をスキップする。この場合 `cargo check` は通るが、
/// リンクはシンボル未解決で失敗するため成功には見えない。`FANDHE_REQUIRE_LIVE_ACTIVITY=1`
/// （本番・配布ビルド）ではスキップせず panic する。swiftc 自体の失敗は常に panic。
fn compile_ios_live_activity() {
    use std::process::Command;

    println!("cargo:rerun-if-env-changed=FANDHE_REQUIRE_LIVE_ACTIVITY");
    let have_xcrun = Command::new("xcrun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_xcrun {
        assert!(
            std::env::var("FANDHE_REQUIRE_LIVE_ACTIVITY").as_deref() != Ok("1"),
            "live_activity: xcrun が無い（FANDHE_REQUIRE_LIVE_ACTIVITY=1 のためスキップ不可）"
        );
        println!("cargo:warning=live_activity: xcrun が無いため Swift 薄層の生成をスキップ（型検査専用。リンクは失敗する）");
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_default();
    let (sdk, triple, lib_dir) = match target.as_str() {
        "aarch64-apple-ios-sim" => (
            "iphonesimulator",
            "arm64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        "aarch64-apple-ios" => ("iphoneos", "arm64-apple-ios17.0", "iphoneos"),
        "x86_64-apple-ios" => (
            "iphonesimulator",
            "x86_64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        other => panic!("live_activity: 未対応の iOS ターゲット: {other}"),
    };
    let srcs = [
        "ios/fandhe_live_activity_attributes.swift",
        "ios/fandhe_ios_live_activity.swift",
    ];
    for s in srcs {
        println!("cargo:rerun-if-changed={s}");
    }

    let xcrun = |args: &[&str]| -> String {
        let out = Command::new("xcrun")
            .args(args)
            .output()
            .expect("xcrun を実行できない");
        assert!(out.status.success(), "xcrun {args:?} が失敗した");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let swiftc = xcrun(&["--sdk", sdk, "--find", "swiftc"]);

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let lib = format!("{out_dir}/libfandhe_ios_live_activity.a");
    let out = Command::new("xcrun")
        .args([
            "--sdk",
            sdk,
            "swiftc",
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-O",
        ])
        .args([
            "-module-name",
            "FandheLiveActivityBridge",
            "-target",
            triple,
            "-sdk",
            &sdk_path,
        ])
        .args(["-o", &lib])
        .args(srcs)
        .output()
        .expect("swiftc を実行できない");
    assert!(
        out.status.success(),
        "live_activity: swiftc が失敗した:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=fandhe_ios_live_activity");
    println!("cargo:rustc-link-lib=framework=ActivityKit");
    // Swift ランタイム（OS 同梱）の検索パスと実行時パス。
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    if let Some(bin) = std::path::Path::new(&swiftc)
        .parent()
        .and_then(|p| p.parent())
    {
        println!(
            "cargo:rustc-link-search=native={}/lib/swift/{lib_dir}",
            bin.display()
        );
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

/// `ios/fandhe_ios_file_watch.m` を `cc` でコンパイルする（Issue #1412）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_file_watch.rs` が使う）。libdispatch は libSystem 内で追加リンクは無い。
fn compile_ios_file_watch() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_file_watch.m");
    cc::Build::new()
        .file("ios/fandhe_ios_file_watch.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_file_watch");
}

/// `ios/fandhe_ios_background_task.m` を `cc` でコンパイルし BackgroundTasks を
/// リンクする（Issue #1045）。**iOS 分岐のみ**から呼ばれる（`BGTaskScheduler` は
/// iOS 専用、doc 参照）。
fn compile_ios_background_task() {
    println!("cargo:rerun-if-changed=ios/fandhe_ios_background_task.m");
    cc::Build::new()
        .file("ios/fandhe_ios_background_task.m")
        .flag("-fobjc-arc")
        .compile("fandhe_ios_background_task");

    println!("cargo:rustc-link-lib=framework=BackgroundTasks");
}

/// macOS ターゲット向けビルド（Issue #889・#897）。iOS と異なり macOS では位置情報
/// （[`compile_apple_location`]）・共有シート（[`compile_apple_share`]）のみを対象と
/// する。既存の `ios/*.m` 群（文字入力・カメラ・傾き・クリップボード・読み上げ・
/// ファイル選択）は iOS 専用のため対象外。
///
/// feature `device`（既定 on）でのみ `fandhe-device` への依存自体が有効になり、
/// 呼び出し元（`src/device/apple_location.rs`・`src/device/apple_share.rs`）が
/// ビルド対象になる（iOS 分岐と同じ判断）。無効時はコンパイルもリンクも行わない。
fn build_macos() {
    if std::env::var("CARGO_FEATURE_DEVICE").is_err() {
        return;
    }

    compile_apple_location();
    compile_apple_share(true);
    compile_apple_biometric();
    compile_apple_contact_picker();
    compile_apple_secure_credential();
    compile_apple_gamepad();
    compile_apple_calendar(false);
    compile_apple_photo_library();
    compile_apple_map_view();

    // `apple/fandhe_apple_mic_record.m`（Issue #954 macOS + #956 iOS）は iOS 分岐と
    // 共用する（上記 doc 参照）。
    compile_apple_mic_record();

    // `apple/fandhe_apple_speech_recognition.m`（Issue #1024 macOS + #1025 iOS）も
    // iOS 分岐と共用する（上記 doc 参照）。
    compile_apple_speech_recognition();

    // `apple/fandhe_apple_system_tray.m`（Issue #1039）は **macOS 専用**（上記
    // `compile_apple_system_tray` doc 参照、iOS 分岐からは呼ばない）。
    compile_apple_system_tray();
    // `apple/fandhe_apple_bluetooth_le.m`（Issue #1060 macOS + #1061 iOS）も共用する。
    compile_apple_bluetooth_le();
    // `apple/fandhe_apple_lan_discovery.m`（Issue #486 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。Network をリンクする。
    compile_apple_lan_discovery();
    // `apple/fandhe_apple_local_peer.m`（Issue #511 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。MultipeerConnectivity をリンクする。
    compile_apple_local_peer();
    // `apple/fandhe_apple_haptics.m`（Issue #1091 macOS + #1092 iOS）も iOS 分岐と共用する。
    compile_apple_haptics(true);
    // `apple/fandhe_apple_nlp.m`（Issue #1094 macOS + #1095 iOS）も macOS 分岐と共用する。
    compile_apple_nlp();
    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_ml_inference.m`（Issue #289 macOS + b-ios iOS）も共用する。
    compile_apple_ml_inference();
    // `apple/fandhe_apple_on_device_llm.swift`（Issue #345 macOS。iOS は Issue #346 で `build_ios` からも呼ぶ）。
    compile_apple_on_device_llm(false);
    compile_apple_translation(false);
    // `apple/fandhe_apple_spotlight_index.m`（Issue #560 macOS + #561 iOS）も共用する。
    compile_apple_spotlight_index();
    // `apple/fandhe_apple_media_transcode.m`（Issue #311 macOS + b-ios iOS）も共用する。
    compile_apple_media_transcode();
    // `apple/fandhe_apple_pdf_viewer.m`（Issue #469 macOS + Issue #471 iOS）も共用する。
    compile_apple_pdf_viewer();
    // `apple/fandhe_apple_network_status.m`（Issue #1282）。iOS 分岐と共用する。
    compile_apple_network_status();
    // `apple/fandhe_apple_accessibility_prefs.m`（Issue #1308）。iOS 分岐と共用する。
    compile_apple_accessibility_prefs();
    // `macos/fandhe_macos_background_task.m`（Issue #1043）は macOS 専用（doc 参照）。
    compile_macos_background_task();
    // `macos/fandhe_macos_url_scheme.m`（Issue #1215）も macOS 専用。
    compile_macos_url_scheme();
    // `macos/fandhe_macos_app_shortcuts.m`（Issue #424）も macOS 専用。AppKit は既存のリンクを使う。
    compile_macos_app_shortcuts();
    // `macos/fandhe_macos_power_state.m`（Issue #1274）も macOS 専用。
    compile_macos_power_state();
    // `macos/fandhe_macos_device_info.m`（Issue #1304）も macOS 専用。
    compile_macos_device_info();
    // `macos/fandhe_macos_system_accent_color.m`（Issue #421）も macOS 専用。AppKit は既存のリンクを使う。
    compile_macos_system_accent_color();
    // `macos/fandhe_macos_safe_area_insets.m`（Issue #444）も macOS 専用。
    compile_macos_safe_area_insets();
    // `macos/fandhe_macos_app_badge.m`（Issue #1360）も macOS 専用。AppKit は既存のリンクを使う。
    compile_macos_app_badge();
    // `macos/fandhe_macos_in_app_review.m`（Issue #563）も macOS 専用。StoreKit をリンクする。
    compile_macos_in_app_review();
    // `macos/fandhe_macos_compose_mail_sms.m`（Issue #1384）も macOS 専用。AppKit は既存のリンクを使う。
    compile_macos_compose_mail_sms();
    // `macos/fandhe_macos_print.m`（Issue #325）も macOS 専用。AppKit は既存のリンクを使う。
    compile_macos_print();
    // `macos/fandhe_macos_audio_devices.m`（Issue #1330）も macOS 専用。CoreAudio を使う。
    compile_macos_audio_devices();
    // `apple/fandhe_apple_media_cast.m`（Issue #386 macOS + #387 iOS）。iOS 分岐と共用する。AVKit・CoreAudio を使う。
    compile_apple_media_cast(false);
    // `apple/fandhe_apple_midi.m`（Issue #1398）。iOS 分岐（#1399）と共用する。CoreMIDI を使う。
    compile_apple_midi();
    // `apple/fandhe_apple_media_playback.m`（Issue #267）。iOS 分岐と共用する。AVFoundation は既存のリンクで足りる。
    compile_apple_media_playback();
    // `apple/fandhe_apple_picture_in_picture.m`（Issue #364 macOS + #365 iOS）。iOS 分岐と共用する。AVKit を使う。
    compile_apple_picture_in_picture(false);
    // `apple/fandhe_apple_background_download.m`（Issue #390）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_download();
    // `apple/fandhe_apple_file_association.m`（Issue #416）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_file_association();
    // `apple/fandhe_apple_now_playing.m`（Issue #381）。iOS 分岐と共用する。MediaPlayer を使う。
    compile_apple_now_playing();
    // `macos/fandhe_macos_local_fonts.m`（Issue #1376）も macOS 専用。AppKit は既存のリンクで足りる。
    compile_macos_local_fonts();
    // `macos/fandhe_macos_file_watch.m`（Issue #1410）も macOS 専用。CoreServices を使う。
    compile_macos_file_watch();
    // `macos/fandhe_macos_multi_display.m`（Issue #1341）も macOS 専用。
    compile_macos_multi_display();
    // `macos/fandhe_macos_music_library.m`（Issue #482）も macOS 専用。iTunesLibrary を使う。
    compile_macos_music_library();
    // `macos/fandhe_macos_web_auth_session.m`（Issue #530）も macOS 専用。AuthenticationServices を使う。
    compile_macos_web_auth_session();
    // `macos/fandhe_macos_wifi_info.m`（Issue #514）も macOS 専用。CoreWLAN を使う。
    compile_macos_wifi_info();
    // `macos/fandhe_macos_usb_device.m`（Issue #505）も macOS 専用。IOKit を使う。
    compile_macos_usb_device();
    compile_macos_pen_stylus();
    // `macos/fandhe_macos_screen_wake_lock.m`（Issue #1278）も macOS 専用。IOKit を使う。
    compile_macos_screen_wake_lock();
    // `apple/fandhe_apple_screen_capture.m`（Issue #1335）の macOS 側（ScreenCaptureKit）。
    compile_apple_screen_capture(false);

    // 素の実行体（`[[bin]] fandhe-egui-proxy-desktop`）へ `macos/Info.plist`
    // （`CFBundleIdentifier` + `NSLocationUsageDescription`）を埋め込む。macOS 10.15+
    // は Info.plist を持たない実行体に CoreLocation の許可プロンプトを一切出さない
    // ため（CoreLocationCLI の既知要件、実装計画 §3.2）。`rustc-link-arg-bins` は
    // `[[bin]]` のみを対象にし、テスト実行体・rlib（`tests/desktop_render.rs` 等）には
    // 影響しない。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    println!("cargo:rerun-if-changed=macos/Info.plist");
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{manifest_dir}/macos/Info.plist"
    );
}
