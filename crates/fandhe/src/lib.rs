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
//!
//! 各 OS の入口は [`main`]（`#[fandhe::main]`）で生成する（Issue #1200）。画面を作る関数 1 つに付けると、
//! デスクトップと iOS の `main`、Android の `android_main`、Web の `start` ができる。選んだ端末機能の起動前の登録
//! （URL scheme の受信など）・終了処理・Activity 再生成時の破棄は入口が feature に応じて行うため、利用者は書かない。
//!
//! ```ignore
//! // src/lib.rs（Android は crate-type に cdylib、iOS・デスクトップは src/main.rs から呼ぶ）
//! #[fandhe::main]
//! fn app(ctx: &fandhe::egui::Context) -> Box<dyn fandhe::app::ProxyAppDyn> {
//!     Box::new(fandhe::app::Menu::new(ctx, "メニュー"))
//! }
//!
//! // src/main.rs
//! fn main() -> Result<(), fandhe::app::LaunchError> {
//!     my_app::main()
//! }
//! ```
//!
//! 依存の向きは本 crate → 機能 crate・`fandhe-render-egui` → `fandhe-runtime` → `fandhe-core` の一方向。
//!
//! 機能ごとのモジュールは機能 crate の crate ルートを glob で再公開し、同名の非公開モジュール `driver` で
//! 機能 crate の `driver` を覆い隠す（glob より明示の定義が優先される）。次の例は feature の有無によらずコンパイルできない。
//!
//! ```compile_fail,E0603
//! use fandhe::clipboard::driver;
//! ```
//!
//! 次の例は feature `clipboard` が無いとコンパイルできない（`tests/feature_gate.rs` が feature なしで確かめる）。
//!
#![cfg_attr(feature = "clipboard", doc = "```no_run")]
#![cfg_attr(not(feature = "clipboard"), doc = "```compile_fail")]
//! let _ = fandhe::clipboard::platform_clipboard();
//! ```
// iOS の入力欄の橋渡し（`entry/ios_text_input.rs`）が ObjC の C ABI を呼ぶため、iOS だけ `deny` へ緩める
// （`unsafe` は各呼び出しに閉じて `// SAFETY:` を付ける。`.claude/rules/coding-rust.md` の FFI 例外規定）。
#![cfg_attr(not(target_os = "ios"), forbid(unsafe_code))]
#![cfg_attr(target_os = "ios", deny(unsafe_code))]

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
#[cfg(feature = "egui")]
pub use fandhe_macros::main;

/// `#[fandhe::main]` が生成するコードから呼ぶ各 OS の入口。利用者は直接使わない（互換を保証しない）。
#[cfg(feature = "egui")]
#[doc(hidden)]
#[path = "entry/mod.rs"]
pub mod __private;

#[cfg(feature = "accessibility-prefs")]
pub mod accessibility_prefs {
    pub use fandhe_cap_accessibility_prefs::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "activity-recognition")]
pub mod activity_recognition {
    pub use fandhe_cap_activity_recognition::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "alarm")]
pub mod alarm {
    pub use fandhe_cap_alarm::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "app-attest")]
pub mod app_attest {
    pub use fandhe_cap_app_attest::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "app-badge")]
pub mod app_badge {
    pub use fandhe_cap_app_badge::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "app-lifecycle")]
pub mod app_lifecycle {
    pub use fandhe_cap_app_lifecycle::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "app-progress-attention")]
pub mod app_progress_attention {
    pub use fandhe_cap_app_progress_attention::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "app-shortcuts")]
pub mod app_shortcuts {
    pub use fandhe_cap_app_shortcuts::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "ar")]
pub mod ar {
    pub use fandhe_cap_ar::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "audio-devices")]
pub mod audio_devices {
    pub use fandhe_cap_audio_devices::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "audio-session")]
pub mod audio_session {
    pub use fandhe_cap_audio_session::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "autofill-otp")]
pub mod autofill_otp {
    pub use fandhe_cap_autofill_otp::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "background-download")]
pub mod background_download {
    pub use fandhe_cap_background_download::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "background-task")]
pub mod background_task {
    pub use fandhe_cap_background_task::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "background-upload")]
pub mod background_upload {
    pub use fandhe_cap_background_upload::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "barcode-scan")]
pub mod barcode_scan {
    pub use fandhe_cap_barcode_scan::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "biometric-auth")]
pub mod biometric_auth {
    pub use fandhe_cap_biometric_auth::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "ble-peripheral")]
pub mod ble_peripheral {
    pub use fandhe_cap_ble_peripheral::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "bluetooth-classic")]
pub mod bluetooth_classic {
    pub use fandhe_cap_bluetooth_classic::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "bluetooth-le")]
pub mod bluetooth_le {
    pub use fandhe_cap_bluetooth_le::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "calendar")]
pub mod calendar {
    pub use fandhe_cap_calendar::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "call-ui")]
pub mod call_ui {
    pub use fandhe_cap_call_ui::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "camera-capture")]
pub mod camera_capture {
    pub use fandhe_cap_camera_capture::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "camera-preview-stream")]
pub mod camera_preview_stream {
    pub use fandhe_cap_camera_preview_stream::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "capture-protection")]
pub mod capture_protection {
    pub use fandhe_cap_capture_protection::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "car-display")]
pub mod car_display {
    pub use fandhe_cap_car_display::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "clipboard")]
pub mod clipboard {
    pub use fandhe_cap_clipboard::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "color-picker")]
pub mod color_picker {
    pub use fandhe_cap_color_picker::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "companion-device")]
pub mod companion_device {
    pub use fandhe_cap_companion_device::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "compass-heading")]
pub mod compass_heading {
    pub use fandhe_cap_compass_heading::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "compose-mail-sms")]
pub mod compose_mail_sms {
    pub use fandhe_cap_compose_mail_sms::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "contacts")]
pub mod contacts {
    pub use fandhe_cap_contacts::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "deep-link")]
pub mod deep_link {
    pub use fandhe_cap_deep_link::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "device-identifier")]
pub mod device_identifier {
    pub use fandhe_cap_device_identifier::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "device-info")]
pub mod device_info {
    pub use fandhe_cap_device_info::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "digital-credentials")]
pub mod digital_credentials {
    pub use fandhe_cap_digital_credentials::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "display-refresh-hdr")]
pub mod display_refresh_hdr {
    pub use fandhe_cap_display_refresh_hdr::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "document-scan")]
pub mod document_scan {
    pub use fandhe_cap_document_scan::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "drag-drop")]
pub mod drag_drop {
    pub use fandhe_cap_drag_drop::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "dynamic-type")]
pub mod dynamic_type {
    pub use fandhe_cap_dynamic_type::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "environment-sensors")]
pub mod environment_sensors {
    pub use fandhe_cap_environment_sensors::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "external-app-launch")]
pub mod external_app_launch {
    pub use fandhe_cap_external_app_launch::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "eye-dropper")]
pub mod eye_dropper {
    pub use fandhe_cap_eye_dropper::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "file-association")]
pub mod file_association {
    pub use fandhe_cap_file_association::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "file-picker")]
pub mod file_picker {
    pub use fandhe_cap_file_picker::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "file-preview")]
pub mod file_preview {
    pub use fandhe_cap_file_preview::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "file-provider")]
pub mod file_provider {
    pub use fandhe_cap_file_provider::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "file-watch")]
pub mod file_watch {
    pub use fandhe_cap_file_watch::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "flashlight")]
pub mod flashlight {
    pub use fandhe_cap_flashlight::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "focus-mode-status")]
pub mod focus_mode_status {
    pub use fandhe_cap_focus_mode_status::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "foldable-posture")]
pub mod foldable_posture {
    pub use fandhe_cap_foldable_posture::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "fullscreen-immersive")]
pub mod fullscreen_immersive {
    pub use fandhe_cap_fullscreen_immersive::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "gamepad")]
pub mod gamepad {
    pub use fandhe_cap_gamepad::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "geocoding-search")]
pub mod geocoding_search {
    pub use fandhe_cap_geocoding_search::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "geofence")]
pub mod geofence {
    pub use fandhe_cap_geofence::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "global-hotkey")]
pub mod global_hotkey {
    pub use fandhe_cap_global_hotkey::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "haptics")]
pub mod haptics {
    pub use fandhe_cap_haptics::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "health-data")]
pub mod health_data {
    pub use fandhe_cap_health_data::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "home-widget")]
pub mod home_widget {
    pub use fandhe_cap_home_widget::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "http-get")]
pub mod http_get {
    pub use fandhe_cap_http_get::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "human-presence")]
pub mod human_presence {
    pub use fandhe_cap_human_presence::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "image-generation")]
pub mod image_generation {
    pub use fandhe_cap_image_generation::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "image-recognition")]
pub mod image_recognition {
    pub use fandhe_cap_image_recognition::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "in-app-browser")]
pub mod in_app_browser {
    pub use fandhe_cap_in_app_browser::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "in-app-review")]
pub mod in_app_review {
    pub use fandhe_cap_in_app_review::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "lan-discovery")]
pub mod lan_discovery {
    pub use fandhe_cap_lan_discovery::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "launch-at-login")]
pub mod launch_at_login {
    pub use fandhe_cap_launch_at_login::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "live-activity")]
pub mod live_activity {
    pub use fandhe_cap_live_activity::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "local-fonts")]
pub mod local_fonts {
    pub use fandhe_cap_local_fonts::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "local-peer-connect")]
pub mod local_peer_connect {
    pub use fandhe_cap_local_peer_connect::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "local-storage-kv")]
pub mod local_storage_kv {
    pub use fandhe_cap_local_storage_kv::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "location")]
pub mod location {
    pub use fandhe_cap_location::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "managed-app-config")]
pub mod managed_app_config {
    pub use fandhe_cap_managed_app_config::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "map-view")]
pub mod map_view {
    pub use fandhe_cap_map_view::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "media-cast")]
pub mod media_cast {
    pub use fandhe_cap_media_cast::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "media-playback")]
pub mod media_playback {
    pub use fandhe_cap_media_playback::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "media-transcode")]
pub mod media_transcode {
    pub use fandhe_cap_media_transcode::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "memory-pressure")]
pub mod memory_pressure {
    pub use fandhe_cap_memory_pressure::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "mic-record")]
pub mod mic_record {
    pub use fandhe_cap_mic_record::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "midi")]
pub mod midi {
    pub use fandhe_cap_midi::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "ml-inference")]
pub mod ml_inference {
    pub use fandhe_cap_ml_inference::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "motion-raw-sensors")]
pub mod motion_raw_sensors {
    pub use fandhe_cap_motion_raw_sensors::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "multi-display")]
pub mod multi_display {
    pub use fandhe_cap_multi_display::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "music-library")]
pub mod music_library {
    pub use fandhe_cap_music_library::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "network-status")]
pub mod network_status {
    pub use fandhe_cap_network_status::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "nfc")]
pub mod nfc {
    pub use fandhe_cap_nfc::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "nfc-card-emulation")]
pub mod nfc_card_emulation {
    pub use fandhe_cap_nfc_card_emulation::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "nlp")]
pub mod nlp {
    pub use fandhe_cap_nlp::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "notification")]
pub mod notification {
    pub use fandhe_cap_notification::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "now-playing-controls")]
pub mod now_playing_controls {
    pub use fandhe_cap_now_playing_controls::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "on-device-llm")]
pub mod on_device_llm {
    pub use fandhe_cap_on_device_llm::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "open-in-shell")]
pub mod open_in_shell {
    pub use fandhe_cap_open_in_shell::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "open-system-settings")]
pub mod open_system_settings {
    pub use fandhe_cap_open_system_settings::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "passkey-signin")]
pub mod passkey_signin {
    pub use fandhe_cap_passkey_signin::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "payment-request")]
pub mod payment_request {
    pub use fandhe_cap_payment_request::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "pdf-viewer")]
pub mod pdf_viewer {
    pub use fandhe_cap_pdf_viewer::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "pedometer")]
pub mod pedometer {
    pub use fandhe_cap_pedometer::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "pen-stylus")]
pub mod pen_stylus {
    pub use fandhe_cap_pen_stylus::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "per-app-language")]
pub mod per_app_language {
    pub use fandhe_cap_per_app_language::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "photo-library")]
pub mod photo_library {
    pub use fandhe_cap_photo_library::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "photo-picker")]
pub mod photo_picker {
    pub use fandhe_cap_photo_picker::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "picture-in-picture")]
pub mod picture_in_picture {
    pub use fandhe_cap_picture_in_picture::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "power-state")]
pub mod power_state {
    pub use fandhe_cap_power_state::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "print")]
pub mod print {
    pub use fandhe_cap_print::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "quick-settings-tile")]
pub mod quick_settings_tile {
    pub use fandhe_cap_quick_settings_tile::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "radio-control")]
pub mod radio_control {
    pub use fandhe_cap_radio_control::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "recent-documents")]
pub mod recent_documents {
    pub use fandhe_cap_recent_documents::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "removable-storage")]
pub mod removable_storage {
    pub use fandhe_cap_removable_storage::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "safe-area-insets")]
pub mod safe_area_insets {
    pub use fandhe_cap_safe_area_insets::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "scanner-devices")]
pub mod scanner_devices {
    pub use fandhe_cap_scanner_devices::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "screen-brightness")]
pub mod screen_brightness {
    pub use fandhe_cap_screen_brightness::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "screen-capture")]
pub mod screen_capture {
    pub use fandhe_cap_screen_capture::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "screen-orientation")]
pub mod screen_orientation {
    pub use fandhe_cap_screen_orientation::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "screen-wake-lock")]
pub mod screen_wake_lock {
    pub use fandhe_cap_screen_wake_lock::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "secure-credential")]
pub mod secure_credential {
    pub use fandhe_cap_secure_credential::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "serial-port")]
pub mod serial_port {
    pub use fandhe_cap_serial_port::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "share-receive")]
pub mod share_receive {
    pub use fandhe_cap_share_receive::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "share-sheet")]
pub mod share_sheet {
    pub use fandhe_cap_share_sheet::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "sleep-lock-events")]
pub mod sleep_lock_events {
    pub use fandhe_cap_sleep_lock_events::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "smart-home")]
pub mod smart_home {
    pub use fandhe_cap_smart_home::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "sound-classification")]
pub mod sound_classification {
    pub use fandhe_cap_sound_classification::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "speech-recognition")]
pub mod speech_recognition {
    pub use fandhe_cap_speech_recognition::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "spell-check")]
pub mod spell_check {
    pub use fandhe_cap_spell_check::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "spotlight-index")]
pub mod spotlight_index {
    pub use fandhe_cap_spotlight_index::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "system-accent-color")]
pub mod system_accent_color {
    pub use fandhe_cap_system_accent_color::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "system-sound")]
pub mod system_sound {
    pub use fandhe_cap_system_sound::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "system-tray")]
pub mod system_tray {
    pub use fandhe_cap_system_tray::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "tilt-motion")]
pub mod tilt_motion {
    pub use fandhe_cap_tilt_motion::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "translation")]
pub mod translation {
    pub use fandhe_cap_translation::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "tts")]
pub mod tts {
    pub use fandhe_cap_tts::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "url-scheme")]
pub mod url_scheme {
    pub use fandhe_cap_url_scheme::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "usb-device")]
pub mod usb_device {
    pub use fandhe_cap_usb_device::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "user-idle")]
pub mod user_idle {
    pub use fandhe_cap_user_idle::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "uwb-ranging")]
pub mod uwb_ranging {
    pub use fandhe_cap_uwb_ranging::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "video-capture")]
pub mod video_capture {
    pub use fandhe_cap_video_capture::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "video-transcode")]
pub mod video_transcode {
    pub use fandhe_cap_video_transcode::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "volume-control")]
pub mod volume_control {
    pub use fandhe_cap_volume_control::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "wallpaper-set")]
pub mod wallpaper_set {
    pub use fandhe_cap_wallpaper_set::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "web-auth-session")]
pub mod web_auth_session {
    pub use fandhe_cap_web_auth_session::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "wifi-connect")]
pub mod wifi_connect {
    pub use fandhe_cap_wifi_connect::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "wifi-info")]
pub mod wifi_info {
    pub use fandhe_cap_wifi_info::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
#[cfg(feature = "window-management")]
pub mod window_management {
    pub use fandhe_cap_window_management::*;
    #[allow(dead_code, hidden_glob_reexports)]
    mod driver {}
}
