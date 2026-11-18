//! fandhe: fandhe-multi-platform の入口 crate（Issue #1199、REQ-21 / TASK-21.5）。
//!
//! 利用者はこの crate だけに依存し、使う端末機能を feature で選ぶ。feature 名は対応表の id
//! （`docs/device/capabilities/<id>.json`）で、`fandhe::<stem>`（id の `-` を `_` にした名前）に
//! 機能 crate `fandhe-cap-<id>` の crate ルートを出す。選ばなかった機能は依存グラフに載らず、
//! 呼ぶとコンパイルエラーになる。
//!
//! ```toml
//! [dependencies]
//! fandhe = { version = "0.2", features = ["clipboard", "camera-capture"] }
//! ```
//!
//! 再公開するのは契約（trait・型）と状態型に依存しない入口（`platform_<stem>()`・begin / poll / discard など）だけで、
//! カタログの画面とつなぐ effect ドライバ（`fandhe_cap_<stem>::driver`）は出さない（`docs/architecture/consumer-app-structure.md` §8.1）。
//! 画面の宣言（`Screen`・`Node`・`StateStore` など）は `fandhe-core` を、画面の組み立てと切り替えは [`app`] を使う。
//! 各 OS の入口の生成（`#[fandhe::main]`）は #1200 で足す。
//!
//! 依存の向きは本 crate → 機能 crate・`fandhe-render-egui` → `fandhe-runtime` → `fandhe-core` の一方向。
//!
//! 次の例は feature `clipboard` が無いとコンパイルできない（`tests/feature_gate.rs` が feature なしで確かめる）。
//!
#![cfg_attr(feature = "clipboard", doc = "```no_run")]
#![cfg_attr(not(feature = "clipboard"), doc = "```compile_fail")]
//! let _ = fandhe::clipboard::platform_clipboard();
//! ```
#![forbid(unsafe_code)]

pub use fandhe_core::*;
/// 起床・完了通知・要求追跡と `Renderer` trait（レンダラ非依存の実行基盤）。
pub use fandhe_runtime as runtime;

/// 画面の組み立て（egui バックエンド）。状態駆動の画面と effect を束ねる [`app::ProxyApp`]、
/// 型消去して入口へ渡す [`app::ProxyAppDyn`]、複数画面を一覧から切り替える [`app::Menu`]。
#[cfg(feature = "egui")]
pub mod app {
    pub use fandhe_render_egui::app::{
        Completion, CompletionSender, EffectFn, InsetMarginSupplier, ProxyApp, ProxyAppDyn,
    };
    pub use fandhe_render_egui::menu::BuildScreenFn;
    pub use fandhe_render_egui::{EguiApp, LaunchError, Menu};
}
/// 画面の組み立てで使う egui（`egui::Context` など）。利用者が版を合わせて依存しなくて済むよう再公開する。
#[cfg(feature = "egui")]
pub use egui;

#[cfg(feature = "accessibility-prefs")]
pub use fandhe_cap_accessibility_prefs as accessibility_prefs;
#[cfg(feature = "activity-recognition")]
pub use fandhe_cap_activity_recognition as activity_recognition;
#[cfg(feature = "alarm")]
pub use fandhe_cap_alarm as alarm;
#[cfg(feature = "app-attest")]
pub use fandhe_cap_app_attest as app_attest;
#[cfg(feature = "app-badge")]
pub use fandhe_cap_app_badge as app_badge;
#[cfg(feature = "app-lifecycle")]
pub use fandhe_cap_app_lifecycle as app_lifecycle;
#[cfg(feature = "app-progress-attention")]
pub use fandhe_cap_app_progress_attention as app_progress_attention;
#[cfg(feature = "app-shortcuts")]
pub use fandhe_cap_app_shortcuts as app_shortcuts;
#[cfg(feature = "ar")]
pub use fandhe_cap_ar as ar;
#[cfg(feature = "audio-devices")]
pub use fandhe_cap_audio_devices as audio_devices;
#[cfg(feature = "audio-session")]
pub use fandhe_cap_audio_session as audio_session;
#[cfg(feature = "autofill-otp")]
pub use fandhe_cap_autofill_otp as autofill_otp;
#[cfg(feature = "background-download")]
pub use fandhe_cap_background_download as background_download;
#[cfg(feature = "background-task")]
pub use fandhe_cap_background_task as background_task;
#[cfg(feature = "background-upload")]
pub use fandhe_cap_background_upload as background_upload;
#[cfg(feature = "barcode-scan")]
pub use fandhe_cap_barcode_scan as barcode_scan;
#[cfg(feature = "biometric-auth")]
pub use fandhe_cap_biometric_auth as biometric_auth;
#[cfg(feature = "ble-peripheral")]
pub use fandhe_cap_ble_peripheral as ble_peripheral;
#[cfg(feature = "bluetooth-classic")]
pub use fandhe_cap_bluetooth_classic as bluetooth_classic;
#[cfg(feature = "bluetooth-le")]
pub use fandhe_cap_bluetooth_le as bluetooth_le;
#[cfg(feature = "calendar")]
pub use fandhe_cap_calendar as calendar;
#[cfg(feature = "call-ui")]
pub use fandhe_cap_call_ui as call_ui;
#[cfg(feature = "camera-capture")]
pub use fandhe_cap_camera_capture as camera_capture;
#[cfg(feature = "camera-preview-stream")]
pub use fandhe_cap_camera_preview_stream as camera_preview_stream;
#[cfg(feature = "capture-protection")]
pub use fandhe_cap_capture_protection as capture_protection;
#[cfg(feature = "car-display")]
pub use fandhe_cap_car_display as car_display;
#[cfg(feature = "clipboard")]
pub use fandhe_cap_clipboard as clipboard;
#[cfg(feature = "color-picker")]
pub use fandhe_cap_color_picker as color_picker;
#[cfg(feature = "companion-device")]
pub use fandhe_cap_companion_device as companion_device;
#[cfg(feature = "compass-heading")]
pub use fandhe_cap_compass_heading as compass_heading;
#[cfg(feature = "compose-mail-sms")]
pub use fandhe_cap_compose_mail_sms as compose_mail_sms;
#[cfg(feature = "contacts")]
pub use fandhe_cap_contacts as contacts;
#[cfg(feature = "deep-link")]
pub use fandhe_cap_deep_link as deep_link;
#[cfg(feature = "device-identifier")]
pub use fandhe_cap_device_identifier as device_identifier;
#[cfg(feature = "device-info")]
pub use fandhe_cap_device_info as device_info;
#[cfg(feature = "digital-credentials")]
pub use fandhe_cap_digital_credentials as digital_credentials;
#[cfg(feature = "display-refresh-hdr")]
pub use fandhe_cap_display_refresh_hdr as display_refresh_hdr;
#[cfg(feature = "document-scan")]
pub use fandhe_cap_document_scan as document_scan;
#[cfg(feature = "drag-drop")]
pub use fandhe_cap_drag_drop as drag_drop;
#[cfg(feature = "dynamic-type")]
pub use fandhe_cap_dynamic_type as dynamic_type;
#[cfg(feature = "environment-sensors")]
pub use fandhe_cap_environment_sensors as environment_sensors;
#[cfg(feature = "external-app-launch")]
pub use fandhe_cap_external_app_launch as external_app_launch;
#[cfg(feature = "eye-dropper")]
pub use fandhe_cap_eye_dropper as eye_dropper;
#[cfg(feature = "file-association")]
pub use fandhe_cap_file_association as file_association;
#[cfg(feature = "file-picker")]
pub use fandhe_cap_file_picker as file_picker;
#[cfg(feature = "file-preview")]
pub use fandhe_cap_file_preview as file_preview;
#[cfg(feature = "file-provider")]
pub use fandhe_cap_file_provider as file_provider;
#[cfg(feature = "file-watch")]
pub use fandhe_cap_file_watch as file_watch;
#[cfg(feature = "flashlight")]
pub use fandhe_cap_flashlight as flashlight;
#[cfg(feature = "focus-mode-status")]
pub use fandhe_cap_focus_mode_status as focus_mode_status;
#[cfg(feature = "foldable-posture")]
pub use fandhe_cap_foldable_posture as foldable_posture;
#[cfg(feature = "fullscreen-immersive")]
pub use fandhe_cap_fullscreen_immersive as fullscreen_immersive;
#[cfg(feature = "gamepad")]
pub use fandhe_cap_gamepad as gamepad;
#[cfg(feature = "geocoding-search")]
pub use fandhe_cap_geocoding_search as geocoding_search;
#[cfg(feature = "geofence")]
pub use fandhe_cap_geofence as geofence;
#[cfg(feature = "global-hotkey")]
pub use fandhe_cap_global_hotkey as global_hotkey;
#[cfg(feature = "haptics")]
pub use fandhe_cap_haptics as haptics;
#[cfg(feature = "health-data")]
pub use fandhe_cap_health_data as health_data;
#[cfg(feature = "home-widget")]
pub use fandhe_cap_home_widget as home_widget;
#[cfg(feature = "http-get")]
pub use fandhe_cap_http_get as http_get;
#[cfg(feature = "human-presence")]
pub use fandhe_cap_human_presence as human_presence;
#[cfg(feature = "image-generation")]
pub use fandhe_cap_image_generation as image_generation;
#[cfg(feature = "image-recognition")]
pub use fandhe_cap_image_recognition as image_recognition;
#[cfg(feature = "in-app-browser")]
pub use fandhe_cap_in_app_browser as in_app_browser;
#[cfg(feature = "in-app-review")]
pub use fandhe_cap_in_app_review as in_app_review;
#[cfg(feature = "lan-discovery")]
pub use fandhe_cap_lan_discovery as lan_discovery;
#[cfg(feature = "launch-at-login")]
pub use fandhe_cap_launch_at_login as launch_at_login;
#[cfg(feature = "live-activity")]
pub use fandhe_cap_live_activity as live_activity;
#[cfg(feature = "local-fonts")]
pub use fandhe_cap_local_fonts as local_fonts;
#[cfg(feature = "local-peer-connect")]
pub use fandhe_cap_local_peer_connect as local_peer_connect;
#[cfg(feature = "local-storage-kv")]
pub use fandhe_cap_local_storage_kv as local_storage_kv;
#[cfg(feature = "location")]
pub use fandhe_cap_location as location;
#[cfg(feature = "managed-app-config")]
pub use fandhe_cap_managed_app_config as managed_app_config;
#[cfg(feature = "map-view")]
pub use fandhe_cap_map_view as map_view;
#[cfg(feature = "media-cast")]
pub use fandhe_cap_media_cast as media_cast;
#[cfg(feature = "media-playback")]
pub use fandhe_cap_media_playback as media_playback;
#[cfg(feature = "media-transcode")]
pub use fandhe_cap_media_transcode as media_transcode;
#[cfg(feature = "memory-pressure")]
pub use fandhe_cap_memory_pressure as memory_pressure;
#[cfg(feature = "mic-record")]
pub use fandhe_cap_mic_record as mic_record;
#[cfg(feature = "midi")]
pub use fandhe_cap_midi as midi;
#[cfg(feature = "ml-inference")]
pub use fandhe_cap_ml_inference as ml_inference;
#[cfg(feature = "motion-raw-sensors")]
pub use fandhe_cap_motion_raw_sensors as motion_raw_sensors;
#[cfg(feature = "multi-display")]
pub use fandhe_cap_multi_display as multi_display;
#[cfg(feature = "music-library")]
pub use fandhe_cap_music_library as music_library;
#[cfg(feature = "network-status")]
pub use fandhe_cap_network_status as network_status;
#[cfg(feature = "nfc")]
pub use fandhe_cap_nfc as nfc;
#[cfg(feature = "nfc-card-emulation")]
pub use fandhe_cap_nfc_card_emulation as nfc_card_emulation;
#[cfg(feature = "nlp")]
pub use fandhe_cap_nlp as nlp;
#[cfg(feature = "notification")]
pub use fandhe_cap_notification as notification;
#[cfg(feature = "now-playing-controls")]
pub use fandhe_cap_now_playing_controls as now_playing_controls;
#[cfg(feature = "on-device-llm")]
pub use fandhe_cap_on_device_llm as on_device_llm;
#[cfg(feature = "open-in-shell")]
pub use fandhe_cap_open_in_shell as open_in_shell;
#[cfg(feature = "open-system-settings")]
pub use fandhe_cap_open_system_settings as open_system_settings;
#[cfg(feature = "passkey-signin")]
pub use fandhe_cap_passkey_signin as passkey_signin;
#[cfg(feature = "payment-request")]
pub use fandhe_cap_payment_request as payment_request;
#[cfg(feature = "pdf-viewer")]
pub use fandhe_cap_pdf_viewer as pdf_viewer;
#[cfg(feature = "pedometer")]
pub use fandhe_cap_pedometer as pedometer;
#[cfg(feature = "pen-stylus")]
pub use fandhe_cap_pen_stylus as pen_stylus;
#[cfg(feature = "per-app-language")]
pub use fandhe_cap_per_app_language as per_app_language;
#[cfg(feature = "photo-library")]
pub use fandhe_cap_photo_library as photo_library;
#[cfg(feature = "photo-picker")]
pub use fandhe_cap_photo_picker as photo_picker;
#[cfg(feature = "picture-in-picture")]
pub use fandhe_cap_picture_in_picture as picture_in_picture;
#[cfg(feature = "power-state")]
pub use fandhe_cap_power_state as power_state;
#[cfg(feature = "print")]
pub use fandhe_cap_print as print;
#[cfg(feature = "quick-settings-tile")]
pub use fandhe_cap_quick_settings_tile as quick_settings_tile;
#[cfg(feature = "radio-control")]
pub use fandhe_cap_radio_control as radio_control;
#[cfg(feature = "recent-documents")]
pub use fandhe_cap_recent_documents as recent_documents;
#[cfg(feature = "removable-storage")]
pub use fandhe_cap_removable_storage as removable_storage;
#[cfg(feature = "safe-area-insets")]
pub use fandhe_cap_safe_area_insets as safe_area_insets;
#[cfg(feature = "scanner-devices")]
pub use fandhe_cap_scanner_devices as scanner_devices;
#[cfg(feature = "screen-brightness")]
pub use fandhe_cap_screen_brightness as screen_brightness;
#[cfg(feature = "screen-capture")]
pub use fandhe_cap_screen_capture as screen_capture;
#[cfg(feature = "screen-orientation")]
pub use fandhe_cap_screen_orientation as screen_orientation;
#[cfg(feature = "screen-wake-lock")]
pub use fandhe_cap_screen_wake_lock as screen_wake_lock;
#[cfg(feature = "secure-credential")]
pub use fandhe_cap_secure_credential as secure_credential;
#[cfg(feature = "serial-port")]
pub use fandhe_cap_serial_port as serial_port;
#[cfg(feature = "share-receive")]
pub use fandhe_cap_share_receive as share_receive;
#[cfg(feature = "share-sheet")]
pub use fandhe_cap_share_sheet as share_sheet;
#[cfg(feature = "sleep-lock-events")]
pub use fandhe_cap_sleep_lock_events as sleep_lock_events;
#[cfg(feature = "smart-home")]
pub use fandhe_cap_smart_home as smart_home;
#[cfg(feature = "sound-classification")]
pub use fandhe_cap_sound_classification as sound_classification;
#[cfg(feature = "speech-recognition")]
pub use fandhe_cap_speech_recognition as speech_recognition;
#[cfg(feature = "spell-check")]
pub use fandhe_cap_spell_check as spell_check;
#[cfg(feature = "spotlight-index")]
pub use fandhe_cap_spotlight_index as spotlight_index;
#[cfg(feature = "system-accent-color")]
pub use fandhe_cap_system_accent_color as system_accent_color;
#[cfg(feature = "system-sound")]
pub use fandhe_cap_system_sound as system_sound;
#[cfg(feature = "system-tray")]
pub use fandhe_cap_system_tray as system_tray;
#[cfg(feature = "tilt-motion")]
pub use fandhe_cap_tilt_motion as tilt_motion;
#[cfg(feature = "translation")]
pub use fandhe_cap_translation as translation;
#[cfg(feature = "tts")]
pub use fandhe_cap_tts as tts;
#[cfg(feature = "url-scheme")]
pub use fandhe_cap_url_scheme as url_scheme;
#[cfg(feature = "usb-device")]
pub use fandhe_cap_usb_device as usb_device;
#[cfg(feature = "user-idle")]
pub use fandhe_cap_user_idle as user_idle;
#[cfg(feature = "uwb-ranging")]
pub use fandhe_cap_uwb_ranging as uwb_ranging;
#[cfg(feature = "video-capture")]
pub use fandhe_cap_video_capture as video_capture;
#[cfg(feature = "video-transcode")]
pub use fandhe_cap_video_transcode as video_transcode;
#[cfg(feature = "volume-control")]
pub use fandhe_cap_volume_control as volume_control;
#[cfg(feature = "wallpaper-set")]
pub use fandhe_cap_wallpaper_set as wallpaper_set;
// web は対応表で n-a。機能 crate が wasm32 でコンパイルできないため、wasm32 では出さない（Cargo.toml 参照）。
#[cfg(all(feature = "web-auth-session", not(target_arch = "wasm32")))]
pub use fandhe_cap_web_auth_session as web_auth_session;
#[cfg(feature = "wifi-connect")]
pub use fandhe_cap_wifi_connect as wifi_connect;
#[cfg(feature = "wifi-info")]
pub use fandhe_cap_wifi_info as wifi_info;
#[cfg(feature = "window-management")]
pub use fandhe_cap_window_management as window_management;
