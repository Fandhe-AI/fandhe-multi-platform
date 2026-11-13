//! iOS / macOS 向け ObjC・Swift 薄層（`ios/*`・`apple/*`・`macos/*`）のビルドスクリプト。
//!
//! 位置づけ: 端末機能の ObjC 実装を `fandhe-egui-proxy` がまとめてコンパイル・リンクする入口。
//! コンパイルとリンクの手順そのものは `fandhe-platform-apple`（feature `build`）の helper に集約しており、
//! 本ファイルは「どの `.m` / Swift をどの framework でビルドするか」の一覧だけを持つ
//! （機能 crate への分離が進めば、各機能の `build.rs` が同じ helper を使う。TASK-21.2 / Issue #1168）。
//!
//! - `CARGO_CFG_TARGET_OS` が `ios` / `macos` 以外（Windows・Linux・wasm32・Android）では即座に return する
//!   no-op で、既存ビルドへの影響はない。
//! - `ios/fandhe_ios_scene.m`（UIScene 化シム、Issue #27）は feature・計測ビルドに関係なく常にコンパイルする
//!   （Info.plist の `UIApplicationSceneManifest` が参照するため）。`ios/fandhe_ios_text_input.m`
//!   （Issue #795）は `src/ios_text_input.rs` が外れる計測ビルド（`cold-start-probe`・`frame-metrics`）では
//!   コンパイルしない（Issue #19）。この 2 本は winit の補完で起動経路の責務のため、本 crate に残す。
//! - それ以外の端末機能は feature `device`（既定 on）でのみコンパイルする。無効時は呼び出し元の
//!   `src/device/*` がビルドされないため、リンクエラーを避けて素通りする。
//! - framework は `.m` / Swift ごとに自己完結して宣言する（他の単位のリンクに頼らない）。
//!   各 `compile_*` の doc に、対象 OS・呼び出し元・Issue 番号を書く。
//! - macOS の `Info.plist` 埋め込み（`-sectcreate`）は最終実行体の設定。`fandhe-gallery` へ転送する。
//! - Swift 薄層の弱リンク・`-rpath`・clang_rt・`-sectcreate` は `rustc-link-arg` では依存元へ伝わらない。
//!   最終成果物は `fandhe-gallery` へ移ったため、`links` メタデータで転送し、gallery の `build.rs` が
//!   出し直す（`fandhe-platform-apple` の `build` モジュール doc 参照）。

use fandhe_platform_apple::build::{
    bin_link_arg, compile_objc, compile_swift, export_link_args, target_is_ios, target_is_macos,
    ui_framework, SwiftLib,
};

fn main() {
    build();
    // 最終成果物（`fandhe-gallery`）へ Apple のリンク指定を転送する（`Cargo.toml` の `links`）。
    export_link_args();
}

fn build() {
    if target_is_macos() {
        build_macos();
        return;
    }
    if !target_is_ios() {
        return;
    }

    // Issue #27: UIScene 化シム。Info.plist の `UIApplicationSceneManifest` はビルド種別に関係なく
    // 1 つのため、計測ビルド・feature `device` 無効でも常にコンパイルする。
    compile_objc("ios/fandhe_ios_scene.m", &["Foundation", "UIKit"]);

    // 計測専用ビルドでは `src/ios_text_input.rs`（`#[no_mangle]` の受け口）が `lib.rs` の
    // cfg で外れるため、呼び出し側の `.m` も同じ条件で外す（Issue #19）。
    let measure_build = std::env::var("CARGO_FEATURE_COLD_START_PROBE").is_ok()
        || std::env::var("CARGO_FEATURE_FRAME_METRICS").is_ok();
    if !measure_build {
        compile_objc("ios/fandhe_ios_text_input.m", &["Foundation", "UIKit"]);
    }

    // feature `device`（既定 on）でのみ `fandhe-device` への依存自体が有効になる
    // （`Cargo.toml` の native 分岐参照）。無効時に ObjC をコンパイルしても
    // 呼び出し元（`src/device/ios.rs`）がビルドされないため、リンクエラーを
    // 避けるべく feature 無効時は素通りする。
    if std::env::var("CARGO_FEATURE_DEVICE").is_err() {
        return;
    }

    // `ios/fandhe_ios_motion.m`（Issue #832、実センサ傾き）。
    compile_objc("ios/fandhe_ios_motion.m", &["CoreMotion", "UIKit"]);

    // `ios/fandhe_ios_clipboard.m`（Issue #859、`UIPasteboard` 読み書き）。
    compile_objc("ios/fandhe_ios_clipboard.m", &["UIKit"]);

    // `ios/fandhe_ios_tts.m`（Issue #866、`AVSpeechSynthesizer` による読み上げ）。
    compile_objc("ios/fandhe_ios_tts.m", &["AVFoundation"]);

    // `ios/fandhe_ios_file_picker.m`（Issue #873、`UIDocumentPickerViewController` によるファイル選択）。
    compile_objc(
        "ios/fandhe_ios_file_picker.m",
        &["UIKit", "UniformTypeIdentifiers"],
    );

    // `ios/fandhe_ios_photo_picker.m`（写真選択）。
    compile_objc(
        "ios/fandhe_ios_photo_picker.m",
        &["ImageIO", "PhotosUI", "UIKit", "UniformTypeIdentifiers"],
    );

    // `ios/fandhe_ios_notification.m`（通知）。
    compile_objc("ios/fandhe_ios_notification.m", &["UserNotifications"]);

    // `ios/fandhe_ios_drag_drop.m`（ドラッグ&ドロップ）。
    compile_objc(
        "ios/fandhe_ios_drag_drop.m",
        &["UIKit", "UniformTypeIdentifiers"],
    );

    compile_apple_location();

    // `apple/fandhe_apple_share.m`（Issue #897 macOS + #899 iOS）も macOS 分岐と共用する。
    compile_apple_share();

    // `apple/fandhe_apple_biometric.m`（Issue #906）も macOS 分岐と共用する。
    compile_apple_biometric();
    compile_apple_contact_picker();

    // `apple/fandhe_apple_text_scale.m`（Issue #914）は iOS 分岐のみから呼ぶ
    // （`build_macos` からは呼ばない）。
    compile_apple_text_scale();

    // `apple/fandhe_apple_secure_credential.m`（Issue #922・#924）は macOS 分岐と
    // 共用する。
    compile_apple_secure_credential();

    // `apple/fandhe_apple_photo_library.m`（Issue #1254 macOS + #1255 iOS）も macOS 分岐と共用する。
    compile_apple_photo_library();

    // `apple/fandhe_apple_map_view.m`（Issue #273 macOS + #274 iOS）も macOS 分岐と共用する。
    compile_apple_map_view();

    // `apple/fandhe_apple_scanner_devices.m`（Issue #981 macOS + #982 iOS）も macOS 分岐と共用する
    // （b-ios が本ファイルを触らずに済むよう、iOS 分岐からも (a) で呼んでおく）。
    compile_apple_scanner_devices();

    // `apple/fandhe_apple_mic_record.m`（Issue #954 macOS + #956 iOS）も macOS 分岐と
    // 共用する。
    compile_apple_mic_record();

    // `apple/fandhe_apple_speech_recognition.m`（Issue #1024 macOS + #1025 iOS）も
    // macOS 分岐と共用する。
    compile_apple_speech_recognition();

    // `apple/fandhe_apple_bluetooth_le.m`（Issue #1060 macOS + #1061 iOS）も macOS 分岐と
    // 共用する。
    compile_apple_bluetooth_le();
    // `apple/fandhe_apple_ble_peripheral.m`（Issue #778）。macOS・iOS 両分岐から呼ぶ。
    compile_apple_ble_peripheral();
    // `apple/fandhe_apple_lan_discovery.m`（Issue #486 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。Network をリンクする。
    compile_apple_lan_discovery();
    // `apple/fandhe_apple_sound_classification.m`（Issue #725 macOS + b-ios iOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。SoundAnalysis をリンクする。
    compile_apple_sound_classification();
    // `apple/fandhe_apple_local_peer.m`（Issue #511 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。MultipeerConnectivity をリンクする。
    compile_apple_local_peer();

    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も iOS 分岐と共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_barcode_scan.m`（Issue #557 macOS + #558 iOS）も共用する。
    compile_apple_barcode_scan();
    // `apple/fandhe_apple_video_transcode.m`（Issue #1014 macOS + #1016 iOS）も macOS 分岐と共用する。
    compile_apple_video_transcode();
    // `apple/fandhe_apple_file_preview.m`（Issue #917 macOS + b-ios iOS）も共用する。
    compile_apple_file_preview();
    // `apple/fandhe_apple_translation.swift`（Issue #535 macOS + #536 iOS）も共用する。
    compile_apple_translation();
    // `apple/fandhe_apple_image_generation.swift`（Issue #975 macOS + #976 iOS）も共用する。
    compile_apple_image_generation();
    // `apple/fandhe_apple_spotlight_index.m`（Issue #560 macOS + #561 iOS）も共用する。
    compile_apple_spotlight_index();

    // `ios/fandhe_ios_background_task.m`（Issue #1045）は iOS 分岐のみから呼ぶ
    // （`build_macos` からは呼ばない）。
    compile_ios_background_task();

    // `ios/fandhe_ios_url_scheme.m`（Issue #1217）。iOS 分岐のみ（macOS は別 .m）。
    compile_ios_url_scheme();

    // `ios/fandhe_ios_fullscreen_immersive.m`（Issue #1313）。iOS 分岐のみ。
    compile_ios_fullscreen_immersive();
    // `ios/fandhe_ios_screen_orientation.m`（Issue #656）。iOS 分岐のみ。
    compile_ios_screen_orientation();
    // `ios/fandhe_ios_app_lifecycle.m`（Issue #894）。iOS 分岐のみ。
    compile_ios_app_lifecycle();
    // `ios/fandhe_ios_memory_pressure.m`（Issue #912）。iOS 分岐のみ。
    compile_ios_memory_pressure();
    // `ios/fandhe_ios_compose_mail_sms.m`（Issue #1385）。iOS 分岐のみ。
    compile_ios_compose_mail_sms();
    // `ios/fandhe_ios_print.m`（Issue #327）。iOS 分岐のみ。
    compile_ios_print();
    // `ios/fandhe_ios_open_system_settings.m`（Issue #799）。iOS 分岐のみ。
    compile_ios_open_system_settings();
    // `ios/fandhe_ios_external_app_launch.m`（Issue #862）。iOS 分岐のみ。
    compile_ios_external_app_launch();
    // `ios/fandhe_ios_device_info.m`（Issue #1305）。iOS 分岐のみ。
    compile_ios_device_info();
    // `ios/fandhe_ios_managed_app_config.m`（Issue #945）。iOS 分岐のみ。
    compile_ios_managed_app_config();
    // `ios/fandhe_ios_safe_area_insets.m`（Issue #445）。iOS 分岐のみ。
    compile_ios_safe_area_insets();
    // `ios/fandhe_ios_audio_session.m`（Issue #841）。iOS 分岐のみ。
    compile_ios_audio_session();
    // `ios/fandhe_ios_music_library.m`（Issue #484）。iOS 分岐のみ。
    compile_ios_music_library();
    // `ios/fandhe_ios_smart_home.m`（Issue #1080）。iOS 分岐のみ。
    compile_ios_smart_home();
    // `ios/fandhe_ios_health_data.m`（Issue #1090）。iOS 分岐のみ（macOS は対応表で n-a）。
    compile_ios_health_data();
    // `ios/fandhe_ios_volume_control.m`（Issue #786）。iOS 分岐のみ。
    compile_ios_volume_control();
    // `ios/fandhe_ios_compass.m`（Issue #1405）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_compass();
    // `ios/fandhe_ios_environment_sensors.m`（Issue #610）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_environment_sensors();
    // `ios/fandhe_ios_motion_raw_sensors.m`（Issue #900）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_motion_raw_sensors();
    // `ios/fandhe_ios_activity_recognition.m`（Issue #614）。iOS 分岐のみ（macOS は対応表で n-a）。
    compile_ios_activity_recognition();
    // `ios/fandhe_ios_camera_preview_stream.m`（Issue #813）。iOS 分岐。macOS 分岐（`build_macos`）も同じ `.m` を共有する（Issue #1066）。
    compile_ios_camera_preview_stream();
    // `ios/fandhe_ios_flashlight.m`（Issue #460）。iOS 分岐のみ（macOS は対応表で n-a）。
    compile_ios_flashlight();
    // `ios/fandhe_ios_pen_stylus.m`（Issue #356）。iOS 分岐のみ。
    compile_ios_pen_stylus();
    // `ios/fandhe_ios_display_refresh_hdr.m`（Issue #871）。iOS 分岐のみ。
    compile_ios_display_refresh_hdr();
    // `ios/fandhe_ios_alarm.swift`（Issue #1077）。iOS 分岐のみ。AlarmKit を弱リンクする。
    compile_ios_alarm();
    // `ios/fandhe_ios_web_auth_session.m`（Issue #532）。iOS 分岐のみ。
    compile_ios_web_auth_session();
    // `ios/fandhe_ios_in_app_browser.m`（Issue #702）。iOS 分岐のみ。SafariServices をリンクする。
    compile_ios_in_app_browser();
    // `ios/fandhe_ios_video_capture.m`（Issue #822）。iOS 分岐のみ。CoreMedia をリンクする。
    compile_ios_video_capture();
    // `ios/fandhe_ios_capture_protection.m`（Issue #583）。iOS 分岐のみ（macOS は対応表で no）。
    compile_ios_capture_protection();
    // `ios/fandhe_ios_file_preview.m`（Issue #919）。iOS 分岐のみ。QuickLook をリンクする。
    compile_ios_file_preview();
    // `ios/fandhe_ios_color_picker.m`（Issue #942）。iOS 分岐のみ。
    compile_ios_color_picker();
    // `apple/fandhe_apple_network_status.m`（Issue #1282 macOS + #1283 iOS）も共用する。
    compile_apple_network_status();
    // `apple/fandhe_apple_media_playback.m`（Issue #267）。macOS と iOS（親 #86 の b-ios）で共用する。
    compile_apple_media_playback();
    // `apple/fandhe_apple_picture_in_picture.m`（Issue #365）。macOS 分岐と共用する。AVKit を使う。
    compile_apple_picture_in_picture();
    // `apple/fandhe_apple_background_download.m`（Issue #390）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_download();
    // `apple/fandhe_apple_background_upload.m`（Issue #835）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_upload();
    // `apple/fandhe_apple_file_association.m`（Issue #416）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_file_association();
    // `apple/fandhe_apple_now_playing.m`（Issue #381 macOS + #382 iOS）。macOS 分岐と共用する。MediaPlayer を使う。
    compile_apple_now_playing();
}

/// `apple/fandhe_apple_location.m` を `cc` でコンパイルし CoreLocation をリンクする
/// （iOS・macOS 双方の feature `device` 分岐から呼ばれる共通処理）。
fn compile_apple_location() {
    compile_objc("apple/fandhe_apple_location.m", &["CoreLocation"]);
}

/// `apple/fandhe_apple_share.m` を `cc` でコンパイルする（Issue #897 macOS + #899 iOS）。
/// UI は iOS が UIKit、macOS が AppKit（`ui_framework()`）。iOS・macOS 双方の分岐から呼ぶ。
fn compile_apple_share() {
    compile_objc("apple/fandhe_apple_share.m", &[ui_framework()]);
}

/// `apple/fandhe_apple_biometric.m` を `cc` でコンパイルし LocalAuthentication を
/// リンクする（iOS・macOS 双方の feature `device` 分岐から呼ばれる共通処理、Issue #906）。
fn compile_apple_biometric() {
    compile_objc("apple/fandhe_apple_biometric.m", &["LocalAuthentication"]);
}

/// `apple/fandhe_apple_contact_picker.m` を `cc` でコンパイルし Contacts / ContactsUI をリンクする（Issue #1220 macOS + #1221 iOS）。
fn compile_apple_contact_picker() {
    compile_objc(
        "apple/fandhe_apple_contact_picker.m",
        &[ui_framework(), "Contacts", "ContactsUI"],
    );
}

/// `apple/fandhe_apple_spotlight_index.m` を `cc` でコンパイルし CoreSpotlight をリンクする
/// （Issue #560 macOS・#561 iOS。両分岐から呼ぶ）。
fn compile_apple_spotlight_index() {
    compile_objc(
        "apple/fandhe_apple_spotlight_index.m",
        if target_is_ios() {
            &["CoreSpotlight", "Foundation"]
        } else {
            &["AppKit", "CoreSpotlight", "Foundation"]
        },
    );
}

/// `apple/fandhe_apple_photo_library.m` を `cc` でコンパイルし Photos をリンクする
/// （Issue #1254 macOS・#1255 iOS。両分岐から呼ぶ）。
fn compile_apple_photo_library() {
    compile_objc(
        "apple/fandhe_apple_photo_library.m",
        &["Foundation", "Photos"],
    );
}

/// `apple/fandhe_apple_map_view.m` を `cc` でコンパイルし MapKit / CoreGraphics をリンクする
/// （Issue #273 macOS・#274 iOS。両分岐から呼ぶ）。
fn compile_apple_map_view() {
    compile_objc(
        "apple/fandhe_apple_map_view.m",
        &[ui_framework(), "CoreGraphics", "Foundation", "MapKit"],
    );
}

/// `apple/fandhe_apple_scanner_devices.m` を `cc` でコンパイルし ImageCaptureCore / ImageIO /
/// UniformTypeIdentifiers をリンクする（Issue #981 macOS・#982 iOS。両分岐から呼ぶ）。
fn compile_apple_scanner_devices() {
    compile_objc(
        "apple/fandhe_apple_scanner_devices.m",
        if target_is_ios() {
            &[
                "Foundation",
                "ImageCaptureCore",
                "ImageIO",
                "UIKit",
                "UniformTypeIdentifiers",
            ]
        } else {
            &[
                "Foundation",
                "ImageCaptureCore",
                "ImageIO",
                "UniformTypeIdentifiers",
            ]
        },
    );
}

/// `apple/fandhe_apple_text_scale.m` を `cc` でコンパイルする（Issue #914）。iOS 分岐のみから呼ばれる（doc 参照）。
/// `TARGET_OS_IPHONE` ガードで中身を囲んでいるため、ファイル自体は共有形式のまま iOS 専用のコンパイル対象にできる。
fn compile_apple_text_scale() {
    compile_objc("apple/fandhe_apple_text_scale.m", &["UIKit"]);
}

/// `apple/fandhe_apple_secure_credential.m` を `cc` でコンパイルし Security を
/// リンクする（Issue #922・#924）。iOS・macOS 双方の feature `device` 分岐から
/// 呼ばれる共通処理（doc 参照）。
fn compile_apple_secure_credential() {
    compile_objc(
        "apple/fandhe_apple_secure_credential.m",
        &["Foundation", "Security"],
    );
}

/// `apple/fandhe_apple_mic_record.m` を `cc` でコンパイルし AVFoundation・
/// AudioToolbox をリンクする（Issue #954 macOS + #956 iOS）。iOS・macOS 双方の
/// feature `device` 分岐から呼ばれる共通処理（doc 参照）。
fn compile_apple_mic_record() {
    compile_objc(
        "apple/fandhe_apple_mic_record.m",
        if target_is_ios() {
            &["AVFoundation", "AudioToolbox", "UIKit"]
        } else {
            &["AVFoundation", "AudioToolbox"]
        },
    );
}

/// `apple/fandhe_apple_speech_recognition.m` を `cc` でコンパイルし Speech をリンクする（Issue #1024 macOS + #1025 iOS）。
fn compile_apple_speech_recognition() {
    compile_objc(
        "apple/fandhe_apple_speech_recognition.m",
        if target_is_ios() {
            &["AVFoundation", "Speech", "UIKit"]
        } else {
            &["AVFoundation", "Speech"]
        },
    );
}

/// `apple/fandhe_apple_barcode_scan.m` を `cc` でコンパイルし Vision をリンクする
/// （Issue #557 macOS、iOS は #558 が利用、バーコード・QR 読み取り）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_barcode_scan`）と
/// 同じ OS 集合に保つ。
fn compile_apple_barcode_scan() {
    compile_objc(
        "apple/fandhe_apple_barcode_scan.m",
        &["Foundation", "Vision"],
    );
}

/// `apple/fandhe_apple_image_recognition.m` を `cc` でコンパイルし Vision をリンクする
/// （Issue #1250 macOS、iOS は #1252 が利用、画像の文字認識）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_image_recognition`）と
/// 同じ OS 集合に保つ。
fn compile_apple_image_recognition() {
    compile_objc(
        "apple/fandhe_apple_image_recognition.m",
        &["Foundation", "Vision"],
    );
}

/// `apple/fandhe_apple_file_preview.m` を `cc` でコンパイルし QuickLookThumbnailing・CoreGraphics を
/// リンクする（Issue #917 macOS、iOS は b-ios が利用、固定ファイルのサムネイル生成）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_file_preview`）と
/// 同じ OS 集合に保つ。
fn compile_apple_file_preview() {
    compile_objc(
        "apple/fandhe_apple_file_preview.m",
        &["CoreGraphics", "Foundation", "QuickLookThumbnailing"],
    );
}

/// `apple/fandhe_apple_video_transcode.m` を `cc` でコンパイルし AVFoundation・CoreMedia・CoreVideo を
/// リンクする（Issue #1014 macOS、iOS は #1016 が利用、動画の小さい解像度への変換）。
/// macOS・iOS 双方の分岐から呼ぶ。Rust 側 cfg（`device.rs` の `apple_video_transcode`）と同じ OS 集合に保つ。
fn compile_apple_video_transcode() {
    compile_objc(
        "apple/fandhe_apple_video_transcode.m",
        &["AVFoundation", "CoreMedia", "CoreVideo", "Foundation"],
    );
}

/// 端末内での画像生成の Swift 薄層 `apple/fandhe_apple_image_generation.swift`（Issue #975 macOS、#976 iOS）。
/// ImagePlayground を弱リンクする。macOS・iOS の両分岐から呼ぶ。
fn compile_apple_image_generation() {
    compile_swift(&SwiftLib {
        srcs: &["apple/fandhe_apple_image_generation.swift"],
        lib: "fandhe_apple_image_generation",
        module: "FandheImageGenerationBridge",
        framework: "ImagePlayground",
        weak: true,
        require_env: "FANDHE_REQUIRE_IMAGE_GENERATION",
        tag: "image_generation",
    });
}

/// 端末内翻訳の Swift 薄層 `apple/fandhe_apple_translation.swift`（Issue #535 macOS、iOS は #536）。
/// Translation を弱リンクする。macOS・iOS の両分岐から呼ぶ。
fn compile_apple_translation() {
    compile_swift(&SwiftLib {
        srcs: &["apple/fandhe_apple_translation.swift"],
        lib: "fandhe_apple_translation",
        module: "FandheTranslationBridge",
        framework: "Translation",
        weak: true,
        require_env: "FANDHE_REQUIRE_TRANSLATION",
        tag: "translation",
    });
}

/// アラーム・正確な時刻の予約の Swift 薄層 `ios/fandhe_ios_alarm.swift`（Issue #1077）。
/// AlarmKit（iOS 26 以降）を弱リンクする。iOS 分岐のみから呼ぶ（`src/device/ios_alarm.rs` が使う）。
fn compile_ios_alarm() {
    compile_swift(&SwiftLib {
        srcs: &["ios/fandhe_ios_alarm.swift"],
        lib: "fandhe_ios_alarm",
        module: "FandheIosAlarmBridge",
        framework: "AlarmKit",
        weak: true,
        require_env: "FANDHE_REQUIRE_ALARM",
        tag: "alarm",
    });
}

/// （Issue #1282 macOS、#1283 iOS 共用）。
fn compile_apple_network_status() {
    compile_objc(
        "apple/fandhe_apple_network_status.m",
        &["Foundation", "Network"],
    );
}

/// `apple/fandhe_apple_ble_peripheral.m` を `cc` でコンパイルし CoreBluetooth をリンクする
/// （Issue #778 macOS。iOS は #779 で委譲済み）。iOS・macOS 双方の feature `device` 分岐から呼ぶ。
fn compile_apple_ble_peripheral() {
    compile_objc(
        "apple/fandhe_apple_ble_peripheral.m",
        if target_is_ios() {
            &["CoreBluetooth", "Foundation", "UIKit"]
        } else {
            &["CoreBluetooth", "Foundation"]
        },
    );
}

/// `apple/fandhe_apple_bluetooth_le.m` を `cc` でコンパイルし CoreBluetooth をリンクする
/// （Issue #1060 macOS + #1061 iOS）。iOS・macOS 双方の feature `device` 分岐から呼ぶ。
fn compile_apple_bluetooth_le() {
    compile_objc(
        "apple/fandhe_apple_bluetooth_le.m",
        &["CoreBluetooth", "Foundation"],
    );
}

/// `apple/fandhe_apple_lan_discovery.m` を `cc` でコンパイルし Network をリンクする
/// （Issue #486 macOS・親 #109 の b-ios で iOS も共用）。dns_sd は libSystem に含まれる。
fn compile_apple_lan_discovery() {
    compile_objc(
        "apple/fandhe_apple_lan_discovery.m",
        &["Foundation", "Network"],
    );
}

/// `apple/fandhe_apple_sound_classification.m` を `cc` でコンパイルし SoundAnalysis をリンクする（Issue #725 macOS・b-ios で iOS も共用）。
/// SoundAnalysis は macOS 10.15・iOS 13 からあるため強リンクでよい（内蔵分類器は `.m` の `@available(macOS 12.0, *)` で守る）。
fn compile_apple_sound_classification() {
    compile_objc(
        "apple/fandhe_apple_sound_classification.m",
        if target_is_ios() {
            &["AVFoundation", "Foundation", "SoundAnalysis", "UIKit"]
        } else {
            &["AVFoundation", "Foundation", "SoundAnalysis"]
        },
    );
}

/// `apple/fandhe_apple_local_peer.m` を `cc` でコンパイルし MultipeerConnectivity をリンクする
/// （Issue #511 macOS・親 #110 の b-ios で iOS も共用）。
fn compile_apple_local_peer() {
    compile_objc(
        "apple/fandhe_apple_local_peer.m",
        if target_is_ios() {
            &["Foundation", "MultipeerConnectivity", "UIKit"]
        } else {
            &["Foundation", "MultipeerConnectivity"]
        },
    );
}

/// `macos/fandhe_macos_background_task.m` を `cc` でコンパイルする（Issue #1043）。
/// **macOS 分岐のみ**から呼ばれる（`NSBackgroundActivityScheduler` は macOS 専用、doc 参照）。
fn compile_macos_background_task() {
    compile_objc("macos/fandhe_macos_background_task.m", &["Foundation"]);
}

/// `macos/fandhe_macos_url_scheme.m` を `cc` でコンパイルする（Issue #1215）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/apple_url_scheme.rs` が使う）。
fn compile_macos_url_scheme() {
    compile_objc(
        "macos/fandhe_macos_url_scheme.m",
        &["AppKit", "CoreServices", "Foundation"],
    );
}

/// `macos/fandhe_macos_app_shortcuts.m` を `cc` でコンパイルする（Issue #424）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_app_shortcuts.rs` が使う）。
fn compile_macos_app_shortcuts() {
    compile_objc(
        "macos/fandhe_macos_app_shortcuts.m",
        &["AppKit", "Foundation"],
    );
}

/// `macos/fandhe_macos_managed_app_config.m` を `cc` でコンパイルし CoreFoundation をリンクする
/// （Issue #944）。**macOS 分岐のみ**から呼ばれる（`src/device/macos_managed_app_config.rs` が使う）。
fn compile_macos_managed_app_config() {
    compile_objc(
        "macos/fandhe_macos_managed_app_config.m",
        &["CoreFoundation", "Foundation"],
    );
}

/// `ios/fandhe_ios_url_scheme.m` を `cc` でコンパイルする（Issue #1217）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/apple_url_scheme.rs` が使う）。
fn compile_ios_url_scheme() {
    compile_objc("ios/fandhe_ios_url_scheme.m", &["Foundation", "UIKit"]);
}

/// `apple/fandhe_apple_background_download.m` を `cc` でコンパイルする（Issue #390）。
/// macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_background_download.rs` が使う）。
fn compile_apple_background_download() {
    compile_objc(
        "apple/fandhe_apple_background_download.m",
        if target_is_ios() {
            &["Foundation", "UIKit"]
        } else {
            &["Foundation"]
        },
    );
}

/// `apple/fandhe_apple_background_upload.m` を `cc` でコンパイルする（Issue #835）。
/// macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_background_upload.rs` が使う）。
fn compile_apple_background_upload() {
    compile_objc("apple/fandhe_apple_background_upload.m", &["Foundation"]);
}

/// `apple/fandhe_apple_file_association.m` を `cc` でコンパイルする（Issue #416）。
/// macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_file_association.rs` が使う）。
/// iOS は scene 通知を購読する（Issue #417）。
fn compile_apple_file_association() {
    compile_objc(
        "apple/fandhe_apple_file_association.m",
        if target_is_ios() {
            &["Foundation", "UIKit"]
        } else {
            &["AppKit", "CoreServices", "Foundation"]
        },
    );
}

/// `apple/fandhe_apple_media_playback.m` を `cc` でコンパイルする（Issue #267）。
/// iOS・macOS 双方の分岐から呼ばれる（`src/device/apple_media_playback.rs` が使う。iOS は親 #86 の b-ios）。
fn compile_apple_media_playback() {
    compile_objc(
        "apple/fandhe_apple_media_playback.m",
        if target_is_ios() {
            &["AVFoundation", "Foundation", "UIKit"]
        } else {
            &["AVFoundation", "Foundation"]
        },
    );
}

/// `apple/fandhe_apple_picture_in_picture.m` を `cc` でコンパイルし AVKit・CoreMedia・CoreVideo・QuartzCore をリンクする（Issue #364 macOS + #365 iOS）。
/// iOS・macOS 双方の分岐から呼ばれる（`src/device/apple_picture_in_picture.rs` が使う）。
/// macOS 12.0+ / iOS 15.0+ の API は `.m` 側の `@available` で守る。
fn compile_apple_picture_in_picture() {
    compile_objc(
        "apple/fandhe_apple_picture_in_picture.m",
        &[
            ui_framework(),
            "AVFoundation",
            "AVKit",
            "CoreMedia",
            "CoreVideo",
            "Foundation",
            "QuartzCore",
        ],
    );
}

/// `apple/fandhe_apple_now_playing.m` を `cc` でコンパイルし MediaPlayer をリンクする（Issue #381 macOS）。
/// macOS・iOS 双方の分岐から呼ばれる（`src/device/apple_now_playing.rs` が使う）。iOS（#382）も同じ `.m` を使う。
fn compile_apple_now_playing() {
    compile_objc(
        "apple/fandhe_apple_now_playing.m",
        if target_is_ios() {
            &["Foundation", "MediaPlayer", "UIKit"]
        } else {
            &["Foundation", "MediaPlayer"]
        },
    );
}

/// `macos/fandhe_macos_volume_control.m` を `cc` でコンパイルする（Issue #785）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_volume_control.rs` が使う）。CoreAudio と AudioToolbox
/// （`kAudioHardwareServiceDeviceProperty_VirtualMainVolume`）を追加でリンクする。
fn compile_macos_volume_control() {
    compile_objc(
        "macos/fandhe_macos_volume_control.m",
        &["AudioToolbox", "CoreAudio", "Foundation"],
    );
}

/// `macos/fandhe_macos_app_lifecycle.m` を `cc` でコンパイルする（Issue #893）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_app_lifecycle.rs` が使う）。
fn compile_macos_app_lifecycle() {
    compile_objc(
        "macos/fandhe_macos_app_lifecycle.m",
        &["AppKit", "Foundation"],
    );
}

/// `macos/fandhe_macos_memory_pressure.m` を `cc` でコンパイルする（Issue #911）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_memory_pressure.rs` が使う）。
fn compile_macos_memory_pressure() {
    compile_objc("macos/fandhe_macos_memory_pressure.m", &["Foundation"]);
}

/// `macos/fandhe_macos_sleep_lock_events.m` を `cc` でコンパイルする（Issue #810）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_sleep_lock_events.rs` が使う）。
fn compile_macos_sleep_lock_events() {
    compile_objc(
        "macos/fandhe_macos_sleep_lock_events.m",
        &["AppKit", "Foundation"],
    );
}

/// `macos/fandhe_macos_removable_storage.m` を `cc` でコンパイルする（Issue #858）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_removable_storage.rs` が使う）。
fn compile_macos_removable_storage() {
    compile_objc(
        "macos/fandhe_macos_removable_storage.m",
        &["AppKit", "Foundation"],
    );
}

/// `macos/fandhe_macos_app_progress_attention.m` を `cc` でコンパイルする（Issue #860）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_app_progress_attention.rs` が使う）。
fn compile_macos_app_progress_attention() {
    compile_objc("macos/fandhe_macos_app_progress_attention.m", &["AppKit"]);
}

/// `macos/fandhe_macos_system_sound.m` を `cc` でコンパイルする（Issue #939）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_system_sound.rs` が使う）。
fn compile_macos_system_sound() {
    compile_objc("macos/fandhe_macos_system_sound.m", &["AppKit"]);
}

/// `macos/fandhe_macos_wallpaper.m` を `cc` でコンパイルする（Issue #970）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_wallpaper.rs` が使う）。
fn compile_macos_wallpaper() {
    compile_objc("macos/fandhe_macos_wallpaper.m", &["AppKit"]);
}

/// `macos/fandhe_macos_launch_at_login.m` を `cc` でコンパイルする（Issue #696）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_launch_at_login.rs` が使う）。ServiceManagement をリンクする
/// （システムフレームワークで旧 OS にも存在するため強リンクでよい。API は `@available` で守る）。
fn compile_macos_launch_at_login() {
    compile_objc(
        "macos/fandhe_macos_launch_at_login.m",
        &["Foundation", "ServiceManagement"],
    );
}

/// `ios/fandhe_ios_file_preview.m` を `cc` でコンパイルし QuickLook をリンクする（Issue #919）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_file_preview.rs` が使う）。
fn compile_ios_file_preview() {
    compile_objc("ios/fandhe_ios_file_preview.m", &["QuickLook", "UIKit"]);
}

/// `ios/fandhe_ios_color_picker.m` を `cc` でコンパイルする（Issue #942）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_color_picker.rs` が使う）。
fn compile_ios_color_picker() {
    compile_objc("ios/fandhe_ios_color_picker.m", &["UIKit"]);
}

/// `ios/fandhe_ios_capture_protection.m` を `cc` でコンパイルする（Issue #583）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_capture_protection.rs` が使う）。
fn compile_ios_capture_protection() {
    compile_objc(
        "ios/fandhe_ios_capture_protection.m",
        &["Foundation", "UIKit"],
    );
}

/// `macos/fandhe_macos_wifi_info.m` を `cc` でコンパイルし CoreWLAN をリンクする（Issue #514）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_wifi_info.rs` が使う）。
fn compile_macos_wifi_info() {
    compile_objc(
        "macos/fandhe_macos_wifi_info.m",
        &["CoreLocation", "CoreWLAN", "Foundation"],
    );
}

/// `macos/fandhe_macos_wifi_connect.m` を `cc` でコンパイルし CoreWLAN をリンクする（Issue #791）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_wifi_connect.rs` が使う）。
fn compile_macos_wifi_connect() {
    compile_objc(
        "macos/fandhe_macos_wifi_connect.m",
        &["CoreLocation", "CoreWLAN", "Foundation"],
    );
}

/// `macos/fandhe_macos_radio_control.m` を `cc` でコンパイルし CoreWLAN・IOBluetooth をリンクする（Issue #972）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_radio_control.rs` が使う）。
fn compile_macos_radio_control() {
    compile_objc(
        "macos/fandhe_macos_radio_control.m",
        &["CoreWLAN", "Foundation", "IOBluetooth"],
    );
}

/// `macos/fandhe_macos_music_library.m` を `cc` でコンパイルし iTunesLibrary をリンクする（Issue #482）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_music_library.rs` が使う）。
fn compile_macos_music_library() {
    compile_objc(
        "macos/fandhe_macos_music_library.m",
        &["Foundation", "iTunesLibrary"],
    );
}

/// `macos/fandhe_macos_web_auth_session.m` を `cc` でコンパイルし AuthenticationServices をリンクする（Issue #530）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_web_auth_session.rs` が使う）。
fn compile_macos_web_auth_session() {
    compile_objc(
        "macos/fandhe_macos_web_auth_session.m",
        &["AppKit", "AuthenticationServices", "Foundation"],
    );
}

/// `macos/fandhe_macos_bluetooth_classic.m` を `cc` でコンパイルし IOBluetooth・CoreBluetooth をリンクする
/// （Issue #659）。**macOS 分岐のみ**から呼ばれる（`src/device/macos_bluetooth_classic.rs` が使う）。
fn compile_macos_bluetooth_classic() {
    compile_objc(
        "macos/fandhe_macos_bluetooth_classic.m",
        &["CoreBluetooth", "Foundation", "IOBluetooth"],
    );
}

/// `macos/fandhe_macos_compose_mail_sms.m` を `cc` でコンパイルする（Issue #1384）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_compose_mail_sms.rs` が使う）。
fn compile_macos_compose_mail_sms() {
    compile_objc("macos/fandhe_macos_compose_mail_sms.m", &["AppKit"]);
}

/// `macos/fandhe_macos_external_app_launch.m` を `cc` でコンパイルする（Issue #861）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_external_app_launch.rs` が使う）。
fn compile_macos_external_app_launch() {
    compile_objc("macos/fandhe_macos_external_app_launch.m", &["AppKit"]);
}

/// `macos/fandhe_macos_global_hotkey.m` を `cc` でコンパイルする（Issue #766）。**macOS 分岐のみ**
/// から呼ばれる（`src/device/macos_global_hotkey.rs` が使う）。Carbon の `RegisterEventHotKey` を
/// 使うため `Carbon.framework` をここでリンクする（許可・entitlement は不要）。
fn compile_macos_global_hotkey() {
    compile_objc(
        "macos/fandhe_macos_global_hotkey.m",
        &["Carbon", "Foundation"],
    );
}

/// `macos/fandhe_macos_recent_documents.m` を `cc` でコンパイルする（Issue #988）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_recent_documents.rs` が使う）。
fn compile_macos_recent_documents() {
    compile_objc("macos/fandhe_macos_recent_documents.m", &["AppKit"]);
}

/// `macos/fandhe_macos_open_in_shell.m` を `cc` でコンパイルする（Issue #749）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_open_in_shell.rs` が使う）。
fn compile_macos_open_in_shell() {
    compile_objc("macos/fandhe_macos_open_in_shell.m", &["AppKit"]);
}

/// `macos/fandhe_macos_eye_dropper.m` を `cc` でコンパイルする（Issue #762）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_eye_dropper.rs` が使う）。
fn compile_macos_eye_dropper() {
    compile_objc("macos/fandhe_macos_eye_dropper.m", &["AppKit"]);
}

/// `macos/fandhe_macos_color_picker.m` を `cc` でコンパイルする（Issue #941）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_color_picker.rs` が使う）。
fn compile_macos_color_picker() {
    compile_objc("macos/fandhe_macos_color_picker.m", &["AppKit"]);
}

/// `macos/fandhe_macos_video_capture.m` を `cc` でコンパイルし AVFoundation・CoreMedia・QuartzCore をリンクする
/// （Issue #1071）。**macOS 分岐のみ**から呼ばれる（`src/device/macos_video_capture.rs` が使う）。
fn compile_macos_video_capture() {
    compile_objc(
        "macos/fandhe_macos_video_capture.m",
        &["AVFoundation", "AppKit", "CoreMedia", "QuartzCore"],
    );
}

/// `macos/fandhe_macos_file_preview.m` を `cc` でコンパイルし QuickLookUI をリンクする（Issue #917）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_file_preview.rs` が使う）。
fn compile_macos_file_preview() {
    compile_objc(
        "macos/fandhe_macos_file_preview.m",
        &["AppKit", "QuickLookUI"],
    );
}

/// `macos/fandhe_macos_print.m` を `cc` でコンパイルする（Issue #325）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_print.rs` が使う）。
fn compile_macos_print() {
    compile_objc("macos/fandhe_macos_print.m", &["AppKit"]);
}

/// `macos/fandhe_macos_device_info.m` を `cc` でコンパイルする（Issue #1304）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_device_info.rs` が使う）。
fn compile_macos_device_info() {
    compile_objc("macos/fandhe_macos_device_info.m", &["Foundation"]);
}

/// `macos/fandhe_macos_display_refresh_hdr.m` を `cc` でコンパイルする（Issue #870）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_display_refresh_hdr.rs` が使う）。
fn compile_macos_display_refresh_hdr() {
    compile_objc(
        "macos/fandhe_macos_display_refresh_hdr.m",
        &["AppKit", "CoreGraphics"],
    );
}

/// `macos/fandhe_macos_window_management.m` を `cc` でコンパイルする（Issue #636）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_window_management.rs` が使う）。
fn compile_macos_window_management() {
    compile_objc("macos/fandhe_macos_window_management.m", &["AppKit"]);
}

/// `macos/fandhe_macos_safe_area_insets.m` を `cc` でコンパイルする（Issue #444）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_safe_area_insets.rs` が使う）。
fn compile_macos_safe_area_insets() {
    compile_objc("macos/fandhe_macos_safe_area_insets.m", &["AppKit"]);
}

/// `macos/fandhe_macos_pen_stylus.m` を `cc` でコンパイルする（Issue #355）。
/// **macOS 分岐のみ**から呼ばれる（`src/device/macos_pen_stylus.rs` が使う）。
fn compile_macos_pen_stylus() {
    compile_objc("macos/fandhe_macos_pen_stylus.m", &["AppKit"]);
}

/// `ios/fandhe_ios_display_refresh_hdr.m` を `cc` でコンパイルする（Issue #871）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_display_refresh_hdr.rs` が使う）。
fn compile_ios_display_refresh_hdr() {
    compile_objc("ios/fandhe_ios_display_refresh_hdr.m", &["UIKit"]);
}

/// `ios/fandhe_ios_music_library.m` を `cc` でコンパイルする（Issue #484）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_music_library.rs` が使う）。
fn compile_ios_music_library() {
    compile_objc(
        "ios/fandhe_ios_music_library.m",
        &["Foundation", "MediaPlayer"],
    );
}

/// `ios/fandhe_ios_smart_home.m` を `cc` でコンパイルし HomeKit をリンクする（Issue #1080）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_smart_home.rs` が使う）。
fn compile_ios_smart_home() {
    compile_objc("ios/fandhe_ios_smart_home.m", &["Foundation", "HomeKit"]);
}

/// `ios/fandhe_ios_health_data.m` を `cc` でコンパイルし HealthKit をリンクする（Issue #1090）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_health_data.rs` が使う）。
fn compile_ios_health_data() {
    compile_objc("ios/fandhe_ios_health_data.m", &["Foundation", "HealthKit"]);
}

/// `ios/fandhe_ios_fullscreen_immersive.m` を `cc` でコンパイルする（Issue #1313）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_fullscreen_immersive.rs` が使う）。`fandhe_ios_common.h` を使う。
fn compile_ios_fullscreen_immersive() {
    compile_objc(
        "ios/fandhe_ios_fullscreen_immersive.m",
        &["Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_screen_orientation.m` を `cc` でコンパイルする（Issue #656）。**iOS 分岐のみ**
/// から呼ばれる（`src/device/ios_screen_orientation.rs` が使う）。`fandhe_ios_common.h` を使う。
fn compile_ios_screen_orientation() {
    compile_objc(
        "ios/fandhe_ios_screen_orientation.m",
        &["Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_compose_mail_sms.m` を `cc` でコンパイルし MessageUI をリンクする（Issue #1385）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_compose_mail_sms.rs` が使う）。
fn compile_ios_compose_mail_sms() {
    compile_objc("ios/fandhe_ios_compose_mail_sms.m", &["MessageUI", "UIKit"]);
}

/// `ios/fandhe_ios_web_auth_session.m` を `cc` でコンパイルし AuthenticationServices をリンクする（Issue #532）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_web_auth_session.rs` が使う）。
fn compile_ios_web_auth_session() {
    compile_objc(
        "ios/fandhe_ios_web_auth_session.m",
        &["AuthenticationServices", "Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_in_app_browser.m` を `cc` でコンパイルし SafariServices をリンクする（Issue #702）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_in_app_browser.rs` が使う）。
fn compile_ios_in_app_browser() {
    compile_objc(
        "ios/fandhe_ios_in_app_browser.m",
        &["SafariServices", "UIKit"],
    );
}

/// `ios/fandhe_ios_video_capture.m` を `cc` でコンパイルし CoreMedia をリンクする（Issue #822）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_video_capture.rs` が使う）。
fn compile_ios_video_capture() {
    compile_objc(
        "ios/fandhe_ios_video_capture.m",
        &[
            "AVFoundation",
            "CoreMedia",
            "UIKit",
            "UniformTypeIdentifiers",
        ],
    );
}

/// `ios/fandhe_ios_open_system_settings.m` を `cc` でコンパイルする（Issue #799）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_open_system_settings.rs` が使う）。
fn compile_ios_open_system_settings() {
    compile_objc("ios/fandhe_ios_open_system_settings.m", &["UIKit"]);
}

/// `ios/fandhe_ios_external_app_launch.m` を `cc` でコンパイルする（Issue #862）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_external_app_launch.rs` が使う）。
fn compile_ios_external_app_launch() {
    compile_objc("ios/fandhe_ios_external_app_launch.m", &["UIKit"]);
}

/// `ios/fandhe_ios_print.m` を `cc` でコンパイルする（Issue #327）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_print.rs` が使う）。
fn compile_ios_print() {
    compile_objc("ios/fandhe_ios_print.m", &["UIKit"]);
}

/// `ios/fandhe_ios_app_lifecycle.m` を `cc` でコンパイルする（Issue #894）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_app_lifecycle.rs` が使う）。
fn compile_ios_app_lifecycle() {
    compile_objc("ios/fandhe_ios_app_lifecycle.m", &["Foundation", "UIKit"]);
}

/// `ios/fandhe_ios_memory_pressure.m` を `cc` でコンパイルする（Issue #912）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_memory_pressure.rs` が使う）。
fn compile_ios_memory_pressure() {
    compile_objc("ios/fandhe_ios_memory_pressure.m", &["Foundation", "UIKit"]);
}

/// `ios/fandhe_ios_device_info.m` を `cc` でコンパイルする（Issue #1305）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_device_info.rs` が使う）。
fn compile_ios_device_info() {
    compile_objc("ios/fandhe_ios_device_info.m", &["Foundation"]);
}

/// `ios/fandhe_ios_managed_app_config.m` を `cc` でコンパイルする（Issue #945）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_managed_app_config.rs` が使う）。
fn compile_ios_managed_app_config() {
    compile_objc("ios/fandhe_ios_managed_app_config.m", &["Foundation"]);
}

/// `ios/fandhe_ios_safe_area_insets.m` を `cc` でコンパイルする（Issue #445）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_safe_area_insets.rs` が使う）。
fn compile_ios_safe_area_insets() {
    compile_objc("ios/fandhe_ios_safe_area_insets.m", &["UIKit"]);
}

/// `ios/fandhe_ios_audio_session.m` を `cc` でコンパイルする（Issue #841）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_audio_session.rs` が使う）。
fn compile_ios_audio_session() {
    compile_objc(
        "ios/fandhe_ios_audio_session.m",
        &["AVFoundation", "Foundation"],
    );
}

/// `ios/fandhe_ios_volume_control.m` を `cc` でコンパイルする（Issue #786）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_volume_control.rs` が使う）。
fn compile_ios_volume_control() {
    compile_objc(
        "ios/fandhe_ios_volume_control.m",
        &["AVFoundation", "Foundation", "MediaPlayer", "UIKit"],
    );
}

/// `ios/fandhe_ios_compass.m` を `cc` でコンパイルする（Issue #1405）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_compass.rs` が使う）。
fn compile_ios_compass() {
    compile_objc(
        "ios/fandhe_ios_compass.m",
        &["CoreLocation", "Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_environment_sensors.m` を `cc` でコンパイルする（Issue #610）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_environment_sensors.rs` が使う）。
fn compile_ios_environment_sensors() {
    compile_objc(
        "ios/fandhe_ios_environment_sensors.m",
        &["CoreMotion", "Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_motion_raw_sensors.m` を `cc` でコンパイルする（Issue #900）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_motion_raw_sensors.rs` が使う）。
fn compile_ios_motion_raw_sensors() {
    compile_objc(
        "ios/fandhe_ios_motion_raw_sensors.m",
        &["CoreMotion", "UIKit"],
    );
}

/// `ios/fandhe_ios_activity_recognition.m` を `cc` でコンパイルする（Issue #614）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_activity_recognition.rs` が使う）。
fn compile_ios_activity_recognition() {
    compile_objc(
        "ios/fandhe_ios_activity_recognition.m",
        &["CoreMotion", "Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_camera_preview_stream.m` を `cc` でコンパイルする（Issue #813、macOS は #1066）。
/// **iOS・macOS 双方**の分岐から呼ばれる（`src/device/ios_camera_preview_stream.rs` が使う）。
fn compile_ios_camera_preview_stream() {
    compile_objc(
        "ios/fandhe_ios_camera_preview_stream.m",
        if target_is_ios() {
            &[
                "AVFoundation",
                "CoreMedia",
                "CoreVideo",
                "Foundation",
                "UIKit",
            ]
        } else {
            &["AVFoundation", "CoreMedia", "CoreVideo", "Foundation"]
        },
    );
}

/// `ios/fandhe_ios_flashlight.m` を `cc` でコンパイルする（Issue #460）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_flashlight.rs` が使う）。
fn compile_ios_flashlight() {
    compile_objc(
        "ios/fandhe_ios_flashlight.m",
        &["AVFoundation", "Foundation", "UIKit"],
    );
}

/// `ios/fandhe_ios_pen_stylus.m` を `cc` でコンパイルする（Issue #356）。
/// **iOS 分岐のみ**から呼ばれる（`src/device/ios_pen_stylus.rs` が使う）。
fn compile_ios_pen_stylus() {
    compile_objc("ios/fandhe_ios_pen_stylus.m", &["UIKit"]);
}

/// `ios/fandhe_ios_background_task.m` を `cc` でコンパイルし BackgroundTasks を
/// リンクする（Issue #1045）。**iOS 分岐のみ**から呼ばれる（`BGTaskScheduler` は
/// iOS 専用、doc 参照）。
fn compile_ios_background_task() {
    compile_objc("ios/fandhe_ios_background_task.m", &["BackgroundTasks"]);
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
    compile_apple_share();
    compile_apple_biometric();
    compile_apple_contact_picker();
    compile_apple_secure_credential();
    compile_apple_photo_library();
    compile_apple_map_view();
    compile_apple_scanner_devices();

    // `apple/fandhe_apple_mic_record.m`（Issue #954 macOS + #956 iOS）は iOS 分岐と
    // 共用する。
    compile_apple_mic_record();

    // `apple/fandhe_apple_speech_recognition.m`（Issue #1024 macOS + #1025 iOS）も
    // iOS 分岐と共用する。
    compile_apple_speech_recognition();

    // `apple/fandhe_apple_bluetooth_le.m`（Issue #1060 macOS + #1061 iOS）も共用する。
    compile_apple_bluetooth_le();
    // `apple/fandhe_apple_ble_peripheral.m`（Issue #778）。macOS・iOS 両分岐から呼ぶ。
    compile_apple_ble_peripheral();
    // `ios/fandhe_ios_camera_preview_stream.m`（Issue #1066）は iOS 分岐と共有する（`TARGET_OS_IPHONE` で分岐）。
    compile_ios_camera_preview_stream();
    // `apple/fandhe_apple_lan_discovery.m`（Issue #486 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。Network をリンクする。
    compile_apple_lan_discovery();
    // `apple/fandhe_apple_sound_classification.m`（Issue #725 macOS + b-ios iOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。SoundAnalysis をリンクする。
    compile_apple_sound_classification();
    // `apple/fandhe_apple_local_peer.m`（Issue #511 macOS）。macOS・iOS 両分岐から呼ぶ
    // （b-ios は build.rs を触らずに済む）。MultipeerConnectivity をリンクする。
    compile_apple_local_peer();
    // `apple/fandhe_apple_image_recognition.m`（Issue #1250 macOS + #1252 iOS）も共用する。
    compile_apple_image_recognition();
    // `apple/fandhe_apple_barcode_scan.m`（Issue #557 macOS + #558 iOS）も共用する。
    compile_apple_barcode_scan();
    compile_apple_translation();
    // `apple/fandhe_apple_image_generation.swift`（Issue #975 macOS。iOS は `build_ios` からも呼ぶ）。
    compile_apple_image_generation();
    // `apple/fandhe_apple_spotlight_index.m`（Issue #560 macOS + #561 iOS）も共用する。
    compile_apple_spotlight_index();
    // `apple/fandhe_apple_video_transcode.m`（Issue #1014 macOS + #1016 iOS）も共用する。
    compile_apple_video_transcode();
    // `apple/fandhe_apple_file_preview.m`（Issue #917 macOS + b-ios iOS）も共用する。
    compile_apple_file_preview();
    // `apple/fandhe_apple_network_status.m`（Issue #1282）。iOS 分岐と共用する。
    compile_apple_network_status();
    // `macos/fandhe_macos_background_task.m`（Issue #1043）は macOS 専用（doc 参照）。
    compile_macos_background_task();
    // `macos/fandhe_macos_url_scheme.m`（Issue #1215）も macOS 専用。
    compile_macos_url_scheme();
    // `macos/fandhe_macos_app_shortcuts.m`（Issue #424）も macOS 専用。
    compile_macos_app_shortcuts();
    // `macos/fandhe_macos_device_info.m`（Issue #1304）も macOS 専用。
    compile_macos_device_info();
    // `macos/fandhe_macos_display_refresh_hdr.m`（Issue #870）も macOS 専用。
    compile_macos_display_refresh_hdr();
    // `macos/fandhe_macos_safe_area_insets.m`（Issue #444）も macOS 専用。
    compile_macos_safe_area_insets();
    // `macos/fandhe_macos_window_management.m`（Issue #636）も macOS 専用。
    compile_macos_window_management();
    // `macos/fandhe_macos_system_sound.m`（Issue #939）も macOS 専用。
    compile_macos_system_sound();
    // `macos/fandhe_macos_wallpaper.m`（Issue #970）も macOS 専用。
    compile_macos_wallpaper();
    // `macos/fandhe_macos_app_progress_attention.m`（Issue #860）も macOS 専用。
    compile_macos_app_progress_attention();
    // `macos/fandhe_macos_launch_at_login.m`（Issue #696）も macOS 専用。ServiceManagement をリンクする。
    compile_macos_launch_at_login();
    // `macos/fandhe_macos_compose_mail_sms.m`（Issue #1384）も macOS 専用。
    compile_macos_compose_mail_sms();
    // `macos/fandhe_macos_external_app_launch.m`（Issue #861）も macOS 専用。
    compile_macos_external_app_launch();
    // `macos/fandhe_macos_global_hotkey.m`（Issue #766）も macOS 専用。Carbon をリンクする。
    compile_macos_global_hotkey();
    // `macos/fandhe_macos_open_in_shell.m`（Issue #749）も macOS 専用。
    compile_macos_open_in_shell();
    // `macos/fandhe_macos_recent_documents.m`（Issue #988）も macOS 専用。
    compile_macos_recent_documents();
    // `macos/fandhe_macos_eye_dropper.m`（Issue #762）も macOS 専用。
    compile_macos_eye_dropper();
    // `macos/fandhe_macos_color_picker.m`（Issue #941）も macOS 専用。
    compile_macos_color_picker();
    // `macos/fandhe_macos_video_capture.m`（Issue #1071）も macOS 専用。AVFoundation 等をリンクする。
    compile_macos_video_capture();
    // `macos/fandhe_macos_file_preview.m`（Issue #917）も macOS 専用。QuickLookUI をリンクする。
    compile_macos_file_preview();
    // `macos/fandhe_macos_print.m`（Issue #325）も macOS 専用。
    compile_macos_print();
    // `macos/fandhe_macos_volume_control.m`（Issue #785）も macOS 専用。CoreAudio・AudioToolbox を使う。
    compile_macos_volume_control();
    // `apple/fandhe_apple_media_playback.m`（Issue #267）。iOS 分岐と共用する。
    compile_apple_media_playback();
    // `apple/fandhe_apple_picture_in_picture.m`（Issue #364 macOS + #365 iOS）。iOS 分岐と共用する。AVKit を使う。
    compile_apple_picture_in_picture();
    // `apple/fandhe_apple_background_download.m`（Issue #390）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_download();
    // `apple/fandhe_apple_background_upload.m`（Issue #835）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_background_upload();
    // `apple/fandhe_apple_file_association.m`（Issue #416）。macOS・iOS 両分岐から呼ぶ（b-ios は build.rs を触らずに済む）。
    compile_apple_file_association();
    // `apple/fandhe_apple_now_playing.m`（Issue #381）。iOS 分岐と共用する。MediaPlayer を使う。
    compile_apple_now_playing();
    // `macos/fandhe_macos_sleep_lock_events.m`（Issue #810）も macOS 専用。
    compile_macos_sleep_lock_events();
    // `macos/fandhe_macos_app_lifecycle.m`（Issue #893）も macOS 専用。
    compile_macos_app_lifecycle();
    // `macos/fandhe_macos_memory_pressure.m`（Issue #911）も macOS 専用。
    compile_macos_memory_pressure();
    // `macos/fandhe_macos_removable_storage.m`（Issue #858）も macOS 専用。
    compile_macos_removable_storage();
    // `macos/fandhe_macos_music_library.m`（Issue #482）も macOS 専用。iTunesLibrary を使う。
    compile_macos_music_library();
    // `macos/fandhe_macos_web_auth_session.m`（Issue #530）も macOS 専用。AuthenticationServices を使う。
    compile_macos_web_auth_session();
    // `macos/fandhe_macos_wifi_info.m`（Issue #514）も macOS 専用。CoreWLAN を使う。
    compile_macos_wifi_info();
    // `macos/fandhe_macos_wifi_connect.m`（Issue #791）も macOS 専用。CoreWLAN を使う。
    compile_macos_wifi_connect();
    // `macos/fandhe_macos_radio_control.m`（Issue #972）も macOS 専用。CoreWLAN・IOBluetooth を使う。
    compile_macos_radio_control();
    // `macos/fandhe_macos_bluetooth_classic.m`（Issue #659）も macOS 専用。IOBluetooth を使う。
    compile_macos_bluetooth_classic();
    compile_macos_pen_stylus();
    // `macos/fandhe_macos_managed_app_config.m`（Issue #944）も macOS 専用。CoreFoundation を使う。
    compile_macos_managed_app_config();

    // 素の実行体（`fandhe-gallery` の bin）へ `macos/Info.plist`
    // （`CFBundleIdentifier` + `NSLocationUsageDescription`）を埋め込む。macOS 10.15+
    // は Info.plist を持たない実行体に CoreLocation の許可プロンプトを一切出さない
    // ため（CoreLocationCLI の既知要件、実装計画 §3.2）。最終実行体は `fandhe-gallery` へ移ったので、
    // 本 crate は bin 専用の指定として記録するだけにし、gallery の `build.rs` が
    // `import_link_args` で `rustc-link-arg-bins` に出し直す（テスト実行体・rlib には影響しない）。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    println!("cargo:rerun-if-changed=macos/Info.plist");
    bin_link_arg(&format!(
        "-Wl,-sectcreate,__TEXT,__info_plist,{manifest_dir}/macos/Info.plist"
    ));
}
