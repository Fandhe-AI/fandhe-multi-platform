//! 画面レジストリ（Issue #715）。`crates/fandhe-core/src/screens/<name>.rs` を
//! 追加したら、`screens/mod.rs` の `pub mod <name>;` と対でここに 1 行足す。
//! 端末アプリは `FANDHE_PROXY_APP=<画面名>` でこの一覧から画面を選ぶ。
//!
//! `nav "<name>" => 関数,`（`generated_screens!` の nav 区画、Issue #726）は
//! `ScreenStack<S>` のような状態駆動の画面遷移を持つ画面向け。詳細は
//! `docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」を参照。
//!
//! 端末機能の画面は `#[cfg(not(target_arch = "wasm32"))]` を行頭に付けて登録する
//! （機能 crate への依存が native 分岐にしか無いため。理由は `screens.rs` のマクロ doc を参照）。
//! 状態を端末保存領域から構築する画面（メモ保存等）は `init` 区画（`screens.rs` の
//! `generated_screens!` doc 参照、Issue #747）を使う。状態駆動 + 端末 I/O 起動を伴う
//! 画面（HTTP 取得・カメラ撮影等）は `effect` 区画（同 doc 参照、Issue #748/#749）を使う。

/// `audio_play` 画面の effect。再生（`fandhe-cap-media-playback`）の後で OS のメディア操作
/// （`fandhe-cap-now-playing-controls`）へ曲名と状態を渡し、OS から来た操作を画面へ返す。
/// 2 機能の crate は互いに依存させない（media-playback だけを選ぶアプリに通知・MediaSession を持ち込まないため）。
/// 合成は最終成果物の責務で、ここに置く。
#[cfg(not(target_arch = "wasm32"))]
fn audio_play_effect(
    state: &fandhe_core::screens::audio_play::AudioPlayState,
    sender: &fandhe_runtime::CompletionSender<fandhe_core::screens::audio_play::AudioPlayState>,
) {
    fandhe_cap_media_playback::driver::audio_play_effect(state, sender);
    fandhe_cap_now_playing_controls::driver::sync_audio_play(state, sender);
}

/// `url_scheme_show` 画面の effect。URL の受信（`fandhe-cap-url-scheme`）の後で、OS のアプリショートカット
/// （`fandhe-cap-app-shortcuts`）の登録結果を同じ画面へ渡す。2 機能の crate は driver で結ばない
/// （url-scheme だけを選ぶアプリへ app-shortcuts を持ち込まないため）。合成は最終成果物の責務で、ここに置く。
#[cfg(not(target_arch = "wasm32"))]
fn url_scheme_show_effect(
    state: &fandhe_core::screens::url_scheme_show::UrlSchemeShowState,
    sender: &fandhe_runtime::CompletionSender<
        fandhe_core::screens::url_scheme_show::UrlSchemeShowState,
    >,
) {
    fandhe_cap_url_scheme::driver::url_scheme_show_effect(state, sender);
    fandhe_cap_app_shortcuts::driver::drive_app_shortcuts(state, sender);
}

generated_screens! {
    "schema_example" => fandhe_core::screens::schema_example_fixture::todo_screen,
    "form" => fandhe_core::screens::form::form_screen,
    "settings" => fandhe_core::screens::settings::settings_screen,
    "checkbox-list" => fandhe_core::screens::checkbox_list::checkbox_list_screen,
    "display-widgets" => fandhe_core::screens::display_widgets::display_widgets_screen,
    "icons" => fandhe_core::screens::icons::icons_screen,
    "containers" => fandhe_core::screens::containers::containers_screen,
    "menu-tabs" => fandhe_core::screens::menu_tabs::menu_tabs_screen,
    "row-actions" => fandhe_core::screens::row_actions::row_actions_screen,

    nav "list_detail" => fandhe_core::screens::list_detail::current_screen,
    nav "confirm_dialog" => fandhe_core::screens::confirm_dialog::confirm_dialog_screen,

    init #[cfg(not(target_arch = "wasm32"))]
    "storage_note" => fandhe_core::screens::storage_note::storage_note_screen, fandhe_cap_local_storage_kv::driver::storage_note_state,

    effect
    #[cfg(not(target_arch = "wasm32"))]
    "http_fetch" => fandhe_core::screens::http_fetch::current_screen,
    fandhe_cap_http_get::driver::http_fetch_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "camera_capture" => fandhe_core::screens::camera_capture::current_screen,
    fandhe_cap_camera_capture::driver::camera_capture_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "tilt_ball" => fandhe_core::screens::tilt_ball::current_screen,
    fandhe_cap_tilt_motion::driver::tilt_ball_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "clipboard_text" => fandhe_core::screens::clipboard_text::current_screen,
    fandhe_cap_clipboard::driver::clipboard_text_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "tts_speak" => fandhe_core::screens::tts_speak::current_screen,
    fandhe_cap_tts::driver::tts_speak_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "file_pick" => fandhe_core::screens::file_pick::current_screen,
    fandhe_cap_file_picker::driver::file_pick_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "photo_pick" => fandhe_core::screens::photo_pick::current_screen,
    fandhe_cap_photo_picker::driver::photo_pick_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "contact_pick" => fandhe_core::screens::contact_pick::current_screen,
    fandhe_cap_contacts::driver::contact_pick_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "eye_dropper" => fandhe_core::screens::eye_dropper_pick::current_screen,
    fandhe_cap_eye_dropper::driver::eye_dropper_pick_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "color_picker" => fandhe_core::screens::color_picker_pick::current_screen,
    fandhe_cap_color_picker::driver::color_picker_pick_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "location_show" => fandhe_core::screens::location_show::current_screen,
    fandhe_cap_location::driver::location_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "geofence_show" => fandhe_core::screens::geofence_show::current_screen,
    fandhe_cap_geofence::driver::geofence_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "share_text" => fandhe_core::screens::share_text::current_screen,
    fandhe_cap_share_sheet::driver::share_text_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "biometric_check" => fandhe_core::screens::biometric_check::current_screen,
    fandhe_cap_biometric_auth::driver::biometric_check_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "calendar_add" => fandhe_core::screens::calendar_add::current_screen,
    fandhe_cap_calendar::driver::calendar_add_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "alarm_show" => fandhe_core::screens::alarm_show::current_screen,
    fandhe_cap_alarm::driver::alarm_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "launch_at_login_show" => fandhe_core::screens::launch_at_login_show::current_screen,
    fandhe_cap_launch_at_login::driver::launch_at_login_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "app_badge_set" => fandhe_core::screens::app_badge_set::current_screen,
    fandhe_cap_app_badge::driver::app_badge_set_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "app_progress_attention_show" => fandhe_core::screens::app_progress_attention_show::current_screen,
    fandhe_cap_app_progress_attention::driver::app_progress_attention_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "compose_mail_sms" => fandhe_core::screens::compose_mail_sms::current_screen,
    fandhe_cap_compose_mail_sms::driver::compose_mail_sms_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "open_system_settings_show" => fandhe_core::screens::open_system_settings_show::current_screen,
    fandhe_cap_open_system_settings::driver::open_system_settings_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "external_app_launch_show" => fandhe_core::screens::external_app_launch_show::current_screen,
    fandhe_cap_external_app_launch::driver::external_app_launch_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "ble_peripheral" => fandhe_core::screens::ble_peripheral::current_screen,
    fandhe_cap_ble_peripheral::driver::ble_peripheral_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "global_hotkey" => fandhe_core::screens::global_hotkey::current_screen,
    fandhe_cap_global_hotkey::driver::global_hotkey_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "open_in_shell" => fandhe_core::screens::open_in_shell::current_screen,
    fandhe_cap_open_in_shell::driver::open_in_shell_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "recent_documents_show" => fandhe_core::screens::recent_documents_show::current_screen,
    fandhe_cap_recent_documents::driver::recent_documents_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "passkey_signin" => fandhe_core::screens::passkey_signin::current_screen,
    fandhe_cap_passkey_signin::driver::passkey_signin_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "payment_request" => fandhe_core::screens::payment_request::current_screen,
    fandhe_cap_payment_request::driver::payment_request_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "digital_credentials" => fandhe_core::screens::digital_credentials::current_screen,
    fandhe_cap_digital_credentials::driver::digital_credentials_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "web_auth_session_demo" => fandhe_core::screens::web_auth_session_demo::current_screen,
    fandhe_cap_web_auth_session::driver::web_auth_session_demo_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "print_page" => fandhe_core::screens::print_page::current_screen,
    fandhe_cap_print::driver::print_page_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "print_screen_page" => fandhe_core::screens::print_screen_page::current_screen,
    fandhe_cap_print::driver::print_screen_page_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "managed_app_config_show" => fandhe_core::screens::managed_app_config_show::current_screen,
    fandhe_cap_managed_app_config::driver::managed_app_config_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "map_view_show" => fandhe_core::screens::map_view_show::current_screen,
    fandhe_cap_map_view::driver::map_view_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "photo_save" => fandhe_core::screens::photo_save::current_screen,
    fandhe_cap_photo_library::driver::photo_save_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "wallpaper_set" => fandhe_core::screens::wallpaper_set::current_screen,
    fandhe_cap_wallpaper_set::driver::wallpaper_set_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "text_scale_preview" => fandhe_core::screens::text_scale_preview::current_screen,
    fandhe_cap_dynamic_type::driver::text_scale_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "credential_vault" => fandhe_core::screens::credential_vault::current_screen,
    fandhe_cap_secure_credential::driver::credential_vault_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "serial_port_show" => fandhe_core::screens::serial_port_show::current_screen,
    fandhe_cap_serial_port::driver::serial_port_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "gamepad_show" => fandhe_core::screens::gamepad_show::current_screen,
    fandhe_cap_gamepad::driver::gamepad_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "mic_record" => fandhe_core::screens::mic_record::current_screen,
    fandhe_cap_mic_record::driver::mic_record_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "screen_capture" => fandhe_core::screens::screen_capture::current_screen,
    fandhe_cap_screen_capture::driver::screen_capture_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "tsv_import" => fandhe_core::screens::tsv_import::current_screen,
    fandhe_cap_file_picker::driver::tsv_import_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "notification_send" => fandhe_core::screens::notification_send::current_screen,
    fandhe_cap_notification::driver::notification_send_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "drag_drop" => fandhe_core::screens::drag_drop::current_screen,
    fandhe_cap_drag_drop::driver::drag_drop_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "speech_recognize" => fandhe_core::screens::speech_recognize::current_screen,
    fandhe_cap_speech_recognition::driver::speech_recognize_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "sound_classification_show" => fandhe_core::screens::sound_classification_show::current_screen,
    fandhe_cap_sound_classification::driver::sound_classification_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "system_tray" => fandhe_core::screens::system_tray::current_screen,
    fandhe_cap_system_tray::driver::system_tray_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "background-task" => fandhe_core::screens::background_task::current_screen,
    fandhe_cap_background_task::driver::background_task_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "home_widget_show" => fandhe_core::screens::home_widget_show::current_screen,
    fandhe_cap_home_widget::driver::home_widget_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "car_display_show" => fandhe_core::screens::car_display_show::current_screen,
    fandhe_cap_car_display::driver::car_display_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "ble_scan" => fandhe_core::screens::ble_scan::current_screen,
    fandhe_cap_bluetooth_le::driver::ble_scan_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "bluetooth_classic_show" => fandhe_core::screens::bluetooth_classic_show::current_screen,
    fandhe_cap_bluetooth_classic::driver::bluetooth_classic_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "lan_discover" => fandhe_core::screens::lan_discover::current_screen,
    fandhe_cap_lan_discovery::driver::lan_discover_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "peer_connect" => fandhe_core::screens::peer_connect::current_screen,
    fandhe_cap_local_peer_connect::driver::peer_connect_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "haptics_tap" => fandhe_core::screens::haptics_tap::current_screen,
    fandhe_cap_haptics::driver::haptics_tap_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "system_sound_beep" => fandhe_core::screens::system_sound_beep::current_screen,
    fandhe_cap_system_sound::driver::system_sound_beep_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "human_presence_read" => fandhe_core::screens::human_presence_read::current_screen,
    fandhe_cap_human_presence::driver::human_presence_read_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "health_steps_read" => fandhe_core::screens::health_steps_read::current_screen,
    fandhe_cap_health_data::driver::health_steps_read_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "pedometer_read" => fandhe_core::screens::pedometer_read::current_screen,
    fandhe_cap_pedometer::driver::pedometer_read_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "audio_play" => fandhe_core::screens::audio_play::current_screen,
    audio_play_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "background_download" => fandhe_core::screens::background_download::current_screen,
    fandhe_cap_background_download::driver::background_download_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "background_upload" => fandhe_core::screens::background_upload::current_screen,
    fandhe_cap_background_upload::driver::background_upload_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "pip_show" => fandhe_core::screens::pip_show::current_screen,
    fandhe_cap_picture_in_picture::driver::pip_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "cast_show" => fandhe_core::screens::cast_show::current_screen,
    fandhe_cap_media_cast::driver::cast_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "midi_show" => fandhe_core::screens::midi_show::current_screen,
    fandhe_cap_midi::driver::midi_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "live_activity_show" => fandhe_core::screens::live_activity_show::current_screen,
    fandhe_cap_live_activity::driver::live_activity_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "call_ui_show" => fandhe_core::screens::call_ui_show::current_screen,
    fandhe_cap_call_ui::driver::call_ui_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "audio_devices_show" => fandhe_core::screens::audio_devices_show::current_screen,
    fandhe_cap_audio_devices::driver::audio_devices_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "local_fonts_show" => fandhe_core::screens::local_fonts_show::current_screen,
    fandhe_cap_local_fonts::driver::local_fonts_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "camera_preview_stream_show" => fandhe_core::screens::camera_preview_stream_show::current_screen,
    fandhe_cap_camera_preview_stream::driver::camera_preview_stream_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "file_provider_show" => fandhe_core::screens::file_provider_show::current_screen,
    fandhe_cap_file_provider::driver::file_provider_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "file_watch_show" => fandhe_core::screens::file_watch_show::current_screen,
    fandhe_cap_file_watch::driver::file_watch_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "app_lifecycle_show" => fandhe_core::screens::app_lifecycle_show::current_screen,
    fandhe_cap_app_lifecycle::driver::app_lifecycle_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "memory_pressure_show" => fandhe_core::screens::memory_pressure_show::current_screen,
    fandhe_cap_memory_pressure::driver::memory_pressure_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "sleep_lock_events_show" => fandhe_core::screens::sleep_lock_events_show::current_screen,
    fandhe_cap_sleep_lock_events::driver::sleep_lock_events_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "removable_storage_show" => fandhe_core::screens::removable_storage_show::current_screen,
    fandhe_cap_removable_storage::driver::removable_storage_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "device_info_show" => fandhe_core::screens::device_info_show::current_screen,
    fandhe_cap_device_info::driver::device_info_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "device_identifier_show" => fandhe_core::screens::device_identifier_show::current_screen,
    fandhe_cap_device_identifier::driver::device_identifier_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "display_refresh_hdr_show" => fandhe_core::screens::display_refresh_hdr_show::current_screen,
    fandhe_cap_display_refresh_hdr::driver::display_refresh_hdr_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "multi_display_show" => fandhe_core::screens::multi_display_show::current_screen,
    fandhe_cap_multi_display::driver::multi_display_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "power_state_show" => fandhe_core::screens::power_state_show::current_screen,
    fandhe_cap_power_state::driver::power_state_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "user_idle_show" => fandhe_core::screens::user_idle_show::current_screen,
    fandhe_cap_user_idle::driver::user_idle_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "accessibility_prefs_show" => fandhe_core::screens::accessibility_prefs_show::current_screen,
    fandhe_cap_accessibility_prefs::driver::accessibility_prefs_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "system_accent_color_show" => fandhe_core::screens::system_accent_color_show::current_screen,
    fandhe_cap_system_accent_color::driver::system_accent_color_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "safe_area_insets_show" => fandhe_core::screens::safe_area_insets_show::current_screen,
    fandhe_cap_safe_area_insets::driver::safe_area_insets_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "foldable_posture_show" => fandhe_core::screens::foldable_posture_show::current_screen,
    fandhe_cap_foldable_posture::driver::foldable_posture_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "focus_mode_status_show" => fandhe_core::screens::focus_mode_status_show::current_screen,
    fandhe_cap_focus_mode_status::driver::focus_mode_status_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "compass_show" => fandhe_core::screens::compass_show::current_screen,
    fandhe_cap_compass_heading::driver::compass_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "environment_sensors_show" => fandhe_core::screens::environment_sensors_show::current_screen,
    fandhe_cap_environment_sensors::driver::environment_sensors_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "motion_raw_sensors_show" => fandhe_core::screens::motion_raw_sensors_show::current_screen,
    fandhe_cap_motion_raw_sensors::driver::motion_raw_sensors_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "activity_recognition_show" => fandhe_core::screens::activity_recognition_show::current_screen,
    fandhe_cap_activity_recognition::driver::activity_recognition_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "ar_show" => fandhe_core::screens::ar_show::current_screen,
    fandhe_cap_ar::driver::ar_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "network_status_show" => fandhe_core::screens::network_status_show::current_screen,
    fandhe_cap_network_status::driver::network_status_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "pen_draw" => fandhe_core::screens::pen_draw::current_screen,
    fandhe_cap_pen_stylus::driver::pen_draw_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "per_app_language_show" => fandhe_core::screens::per_app_language_show::current_screen,
    fandhe_cap_per_app_language::driver::per_app_language_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "fullscreen_toggle" => fandhe_core::screens::fullscreen_toggle::current_screen,
    fandhe_cap_fullscreen_immersive::driver::fullscreen_toggle_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "window_management_show" => fandhe_core::screens::window_management_show::current_screen,
    fandhe_cap_window_management::driver::window_management_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "wake_lock_toggle" => fandhe_core::screens::wake_lock_toggle::current_screen,
    fandhe_cap_screen_wake_lock::driver::wake_lock_toggle_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "screen_brightness_show" => fandhe_core::screens::screen_brightness_show::current_screen,
    fandhe_cap_screen_brightness::driver::screen_brightness_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "volume_control_show" => fandhe_core::screens::volume_control_show::current_screen,
    fandhe_cap_volume_control::driver::volume_control_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "screen_orientation_show" => fandhe_core::screens::screen_orientation_show::current_screen,
    fandhe_cap_screen_orientation::driver::screen_orientation_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "uwb_ranging_show" => fandhe_core::screens::uwb_ranging_show::current_screen,
    fandhe_cap_uwb_ranging::driver::uwb_ranging_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "flashlight_toggle" => fandhe_core::screens::flashlight_toggle::current_screen,
    fandhe_cap_flashlight::driver::flashlight_toggle_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "nlp_segment" => fandhe_core::screens::nlp_segment::current_screen,
    fandhe_cap_nlp::driver::nlp_segment_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "barcode_scan" => fandhe_core::screens::barcode_scan::current_screen,
    fandhe_cap_barcode_scan::driver::barcode_scan_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "otp_input" => fandhe_core::screens::otp_input::current_screen,
    fandhe_cap_autofill_otp::driver::autofill_otp_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "in_app_review" => fandhe_core::screens::in_app_review::current_screen,
    fandhe_cap_in_app_review::driver::in_app_review_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "image_recognize" => fandhe_core::screens::image_recognize::current_screen,
    fandhe_cap_image_recognition::driver::image_recognize_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "media_transcode" => fandhe_core::screens::media_transcode::current_screen,
    fandhe_cap_media_transcode::driver::media_transcode_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "video_transcode" => fandhe_core::screens::video_transcode::current_screen,
    fandhe_cap_video_transcode::driver::video_transcode_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "pdf_first_page" => fandhe_core::screens::pdf_first_page::current_screen,
    fandhe_cap_pdf_viewer::driver::pdf_first_page_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "usb_device_show" => fandhe_core::screens::usb_device_show::current_screen,
    fandhe_cap_usb_device::driver::usb_device_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "music_library_list" => fandhe_core::screens::music_library_list::current_screen,
    fandhe_cap_music_library::driver::music_library_list_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "smart_home_list" => fandhe_core::screens::smart_home_list::current_screen,
    fandhe_cap_smart_home::driver::smart_home_list_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "wifi_info_show" => fandhe_core::screens::wifi_info_show::current_screen,
    fandhe_cap_wifi_info::driver::wifi_info_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "wifi_connect_show" => fandhe_core::screens::wifi_connect_show::current_screen,
    fandhe_cap_wifi_connect::driver::wifi_connect_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "radio_control_show" => fandhe_core::screens::radio_control_show::current_screen,
    fandhe_cap_radio_control::driver::radio_control_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "ml_infer" => fandhe_core::screens::ml_infer::current_screen,
    fandhe_cap_ml_inference::driver::ml_infer_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "on_device_llm" => fandhe_core::screens::on_device_llm::current_screen,
    fandhe_cap_on_device_llm::driver::on_device_llm_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "app_attest_show" => fandhe_core::screens::app_attest_show::current_screen,
    fandhe_cap_app_attest::driver::app_attest_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "companion_device_show" => fandhe_core::screens::companion_device_show::current_screen,
    fandhe_cap_companion_device::driver::companion_device_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "document_scan_show" => fandhe_core::screens::document_scan_show::current_screen,
    fandhe_cap_document_scan::driver::document_scan_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "image_generation_show" => fandhe_core::screens::image_generation_show::current_screen,
    fandhe_cap_image_generation::driver::image_generation_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "in_app_browser_show" => fandhe_core::screens::in_app_browser_show::current_screen,
    fandhe_cap_in_app_browser::driver::in_app_browser_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "geocoding_search_show" => fandhe_core::screens::geocoding_search_show::current_screen,
    fandhe_cap_geocoding_search::driver::geocoding_search_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "nfc_read" => fandhe_core::screens::nfc_read::current_screen,
    fandhe_cap_nfc::driver::nfc_read_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "nfc_card_emulation_show" => fandhe_core::screens::nfc_card_emulation_show::current_screen,
    fandhe_cap_nfc_card_emulation::driver::nfc_card_emulation_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "scanner_devices_import" => fandhe_core::screens::scanner_devices_import::current_screen,
    fandhe_cap_scanner_devices::driver::scanner_devices_import_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "translation" => fandhe_core::screens::translation::current_screen,
    fandhe_cap_translation::driver::translation_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "spell_check" => fandhe_core::screens::spell_check::current_screen,
    fandhe_cap_spell_check::driver::spell_check_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "spotlight_index" => fandhe_core::screens::spotlight_index::current_screen,
    fandhe_cap_spotlight_index::driver::spotlight_index_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "quick_settings_tile_show" => fandhe_core::screens::quick_settings_tile_show::current_screen,
    fandhe_cap_quick_settings_tile::driver::quick_settings_tile_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "share_receive_show" => fandhe_core::screens::share_receive_show::current_screen,
    fandhe_cap_share_receive::driver::share_receive_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "url_scheme_show" => fandhe_core::screens::url_scheme_show::current_screen,
    url_scheme_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "deep_link_show" => fandhe_core::screens::deep_link_show::current_screen,
    fandhe_cap_deep_link::driver::deep_link_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "file_preview" => fandhe_core::screens::file_preview_show::current_screen,
    fandhe_cap_file_preview::driver::file_preview_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "file_association_show" => fandhe_core::screens::file_association_show::current_screen,
    fandhe_cap_file_association::driver::file_association_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "capture_protection_show" => fandhe_core::screens::capture_protection_show::current_screen,
    fandhe_cap_capture_protection::driver::capture_protection_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "video_capture_show" => fandhe_core::screens::video_capture_show::current_screen,
    fandhe_cap_video_capture::driver::video_capture_show_effect,
    effect
    #[cfg(not(target_arch = "wasm32"))]
    "audio_session_show" => fandhe_core::screens::audio_session_show::current_screen,
    fandhe_cap_audio_session::driver::audio_session_show_effect,
}
