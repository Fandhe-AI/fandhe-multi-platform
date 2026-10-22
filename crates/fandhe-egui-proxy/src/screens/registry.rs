//! 画面レジストリ（Issue #715）。`crates/fandhe-core/src/screens/<name>.rs` を
//! 追加したら、`screens/mod.rs` の `pub mod <name>;` と対でここに 1 行足す。
//! 端末アプリは `FANDHE_PROXY_APP=<画面名>` でこの一覧から画面を選ぶ。
//!
//! `nav "<name>" => 関数,`（`generated_screens!` の nav 区画、Issue #726）は
//! `ScreenStack<S>` のような状態駆動の画面遷移を持つ画面向け。詳細は
//! `docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」を参照。
//!
//! Phase 8（端末機能、Issue #746）の画面は
//! `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` を行頭に付けて登録する。
//! `not(target_arch = "wasm32")` を併せて要る理由は `screens.rs` のマクロ doc を参照
//! （`device` feature は既定 on で wasm32 でも有効になるが、`fandhe-device` 自体は
//! wasm32 に依存として存在しないため）。spec 要件化が否決され feature `device`
//! を削除した場合、この行自体が存在しないものとして扱われる（切り離し手順は docs/architecture/device-feature-removal.md 参照）。
//! 状態を端末保存領域から構築する画面（メモ保存等）は `init` 区画（`screens.rs` の
//! `generated_screens!` doc 参照、Issue #747）を使う。状態駆動 + 端末 I/O 起動を伴う
//! 画面（HTTP 取得・カメラ撮影等）は `effect` 区画（同 doc 参照、Issue #748/#749）を使う。

generated_screens! {
    "schema_example" => fandhe_core::screens::schema_example_fixture::todo_screen,
    "form" => fandhe_core::screens::form::form_screen,
    "settings" => fandhe_core::screens::settings::settings_screen,
    "checkbox-list" => fandhe_core::screens::checkbox_list::checkbox_list_screen,

    nav "list_detail" => fandhe_core::screens::list_detail::current_screen,
    nav "confirm_dialog" => fandhe_core::screens::confirm_dialog::confirm_dialog_screen,

    init #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "storage_note" => fandhe_core::screens::storage_note::storage_note_screen, crate::device::storage_note_state,

    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "http_fetch" => fandhe_core::screens::http_fetch::current_screen,
    crate::device::http_fetch::http_fetch_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "camera_capture" => fandhe_core::screens::camera_capture::current_screen,
    crate::device::camera_capture::camera_capture_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "tilt_ball" => fandhe_core::screens::tilt_ball::current_screen,
    crate::device::tilt::tilt_ball_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "clipboard_text" => fandhe_core::screens::clipboard_text::current_screen,
    crate::device::clipboard::clipboard_text_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "tts_speak" => fandhe_core::screens::tts_speak::current_screen,
    crate::device::tts::tts_speak_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "file_pick" => fandhe_core::screens::file_pick::current_screen,
    crate::device::file_pick::file_pick_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "photo_pick" => fandhe_core::screens::photo_pick::current_screen,
    crate::device::photo_pick::photo_pick_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "contact_pick" => fandhe_core::screens::contact_pick::current_screen,
    crate::device::contact_pick::contact_pick_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "location_show" => fandhe_core::screens::location_show::current_screen,
    crate::device::location::location_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "geofence_show" => fandhe_core::screens::geofence_show::current_screen,
    crate::device::geofence::geofence_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "share_text" => fandhe_core::screens::share_text::current_screen,
    crate::device::share::share_text_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "biometric_check" => fandhe_core::screens::biometric_check::current_screen,
    crate::device::biometric_auth::biometric_check_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "calendar_add" => fandhe_core::screens::calendar_add::current_screen,
    crate::device::calendar::calendar_add_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "app_badge_set" => fandhe_core::screens::app_badge_set::current_screen,
    crate::device::app_badge::app_badge_set_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "compose_mail_sms" => fandhe_core::screens::compose_mail_sms::current_screen,
    crate::device::compose_mail_sms::compose_mail_sms_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "passkey_signin" => fandhe_core::screens::passkey_signin::current_screen,
    crate::device::passkey::passkey_signin_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "payment_request" => fandhe_core::screens::payment_request::current_screen,
    crate::device::payment_request::payment_request_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "web_auth_session_demo" => fandhe_core::screens::web_auth_session_demo::current_screen,
    crate::device::web_auth_session::web_auth_session_demo_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "print_page" => fandhe_core::screens::print_page::current_screen,
    crate::device::print::print_page_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "map_view_show" => fandhe_core::screens::map_view_show::current_screen,
    crate::device::map_view::map_view_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "photo_save" => fandhe_core::screens::photo_save::current_screen,
    crate::device::photo_save::photo_save_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "text_scale_preview" => fandhe_core::screens::text_scale_preview::current_screen,
    crate::device::text_scale::text_scale_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "credential_vault" => fandhe_core::screens::credential_vault::current_screen,
    crate::device::secure_credential::credential_vault_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "gamepad_show" => fandhe_core::screens::gamepad_show::current_screen,
    crate::device::gamepad::gamepad_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "mic_record" => fandhe_core::screens::mic_record::current_screen,
    crate::device::mic_record::mic_record_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "screen_capture" => fandhe_core::screens::screen_capture::current_screen,
    crate::device::screen_capture::screen_capture_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "tsv_import" => fandhe_core::screens::tsv_import::current_screen,
    crate::device::tsv_import::tsv_import_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "notification_send" => fandhe_core::screens::notification_send::current_screen,
    crate::device::notification::notification_send_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "drag_drop" => fandhe_core::screens::drag_drop::current_screen,
    crate::device::drag_drop::drag_drop_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "speech_recognize" => fandhe_core::screens::speech_recognize::current_screen,
    crate::device::speech_recognition::speech_recognize_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "system_tray" => fandhe_core::screens::system_tray::current_screen,
    crate::device::system_tray::system_tray_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "background-task" => fandhe_core::screens::background_task::current_screen,
    crate::device::background_task::background_task_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "home_widget_show" => fandhe_core::screens::home_widget_show::current_screen,
    crate::device::home_widget::home_widget_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "ble_scan" => fandhe_core::screens::ble_scan::current_screen,
    crate::device::bluetooth_le::ble_scan_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "lan_discover" => fandhe_core::screens::lan_discover::current_screen,
    crate::device::lan_discovery::lan_discover_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "peer_connect" => fandhe_core::screens::peer_connect::current_screen,
    crate::device::local_peer::peer_connect_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "haptics_tap" => fandhe_core::screens::haptics_tap::current_screen,
    crate::device::haptics::haptics_tap_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "pedometer_read" => fandhe_core::screens::pedometer_read::current_screen,
    crate::device::pedometer::pedometer_read_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "audio_play" => fandhe_core::screens::audio_play::current_screen,
    crate::device::media_playback::audio_play_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "background_download" => fandhe_core::screens::background_download::current_screen,
    crate::device::background_download::background_download_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "pip_show" => fandhe_core::screens::pip_show::current_screen,
    crate::device::picture_in_picture::pip_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "cast_show" => fandhe_core::screens::cast_show::current_screen,
    crate::device::media_cast::cast_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "midi_show" => fandhe_core::screens::midi_show::current_screen,
    crate::device::midi::midi_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "live_activity_show" => fandhe_core::screens::live_activity_show::current_screen,
    crate::device::live_activity::live_activity_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "call_ui_show" => fandhe_core::screens::call_ui_show::current_screen,
    crate::device::call_ui::call_ui_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "audio_devices_show" => fandhe_core::screens::audio_devices_show::current_screen,
    crate::device::audio_devices::audio_devices_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "local_fonts_show" => fandhe_core::screens::local_fonts_show::current_screen,
    crate::device::local_fonts::local_fonts_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "file_watch_show" => fandhe_core::screens::file_watch_show::current_screen,
    crate::device::file_watch::file_watch_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "device_info_show" => fandhe_core::screens::device_info_show::current_screen,
    crate::device::device_info::device_info_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "multi_display_show" => fandhe_core::screens::multi_display_show::current_screen,
    crate::device::multi_display::multi_display_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "power_state_show" => fandhe_core::screens::power_state_show::current_screen,
    crate::device::power_state::power_state_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "accessibility_prefs_show" => fandhe_core::screens::accessibility_prefs_show::current_screen,
    crate::device::accessibility_prefs::accessibility_prefs_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "system_accent_color_show" => fandhe_core::screens::system_accent_color_show::current_screen,
    crate::device::system_accent_color::system_accent_color_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "safe_area_insets_show" => fandhe_core::screens::safe_area_insets_show::current_screen,
    crate::device::safe_area_insets::safe_area_insets_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "focus_mode_status_show" => fandhe_core::screens::focus_mode_status_show::current_screen,
    crate::device::focus_mode_status::focus_mode_status_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "compass_show" => fandhe_core::screens::compass_show::current_screen,
    crate::device::compass::compass_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "ar_show" => fandhe_core::screens::ar_show::current_screen,
    crate::device::ar::ar_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "network_status_show" => fandhe_core::screens::network_status_show::current_screen,
    crate::device::network_status::network_status_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "pen_draw" => fandhe_core::screens::pen_draw::current_screen,
    crate::device::pen_stylus::pen_draw_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "fullscreen_toggle" => fandhe_core::screens::fullscreen_toggle::current_screen,
    crate::device::fullscreen_immersive::fullscreen_toggle_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "wake_lock_toggle" => fandhe_core::screens::wake_lock_toggle::current_screen,
    crate::device::screen_wake_lock::wake_lock_toggle_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "flashlight_toggle" => fandhe_core::screens::flashlight_toggle::current_screen,
    crate::device::flashlight::flashlight_toggle_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "nlp_segment" => fandhe_core::screens::nlp_segment::current_screen,
    crate::device::nlp::nlp_segment_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "barcode_scan" => fandhe_core::screens::barcode_scan::current_screen,
    crate::device::barcode_scan::barcode_scan_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "in_app_review" => fandhe_core::screens::in_app_review::current_screen,
    crate::device::in_app_review::in_app_review_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "image_recognize" => fandhe_core::screens::image_recognize::current_screen,
    crate::device::image_recognition::image_recognize_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "media_transcode" => fandhe_core::screens::media_transcode::current_screen,
    crate::device::media_transcode::media_transcode_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "pdf_first_page" => fandhe_core::screens::pdf_first_page::current_screen,
    crate::device::pdf_viewer::pdf_first_page_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "usb_device_show" => fandhe_core::screens::usb_device_show::current_screen,
    crate::device::usb_device::usb_device_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "music_library_list" => fandhe_core::screens::music_library_list::current_screen,
    crate::device::music_library::music_library_list_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "wifi_info_show" => fandhe_core::screens::wifi_info_show::current_screen,
    crate::device::wifi_info::wifi_info_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "ml_infer" => fandhe_core::screens::ml_infer::current_screen,
    crate::device::ml_inference::ml_infer_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "on_device_llm" => fandhe_core::screens::on_device_llm::current_screen,
    crate::device::on_device_llm::on_device_llm_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "translation" => fandhe_core::screens::translation::current_screen,
    crate::device::translation::translation_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "spotlight_index" => fandhe_core::screens::spotlight_index::current_screen,
    crate::device::spotlight_index::spotlight_index_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "url_scheme_show" => fandhe_core::screens::url_scheme_show::current_screen,
    crate::device::url_scheme::url_scheme_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "file_association_show" => fandhe_core::screens::file_association_show::current_screen,
    crate::device::file_association::file_association_show_effect,
    effect
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    "capture_protection_show" => fandhe_core::screens::capture_protection_show::current_screen,
    crate::device::capture_protection::capture_protection_show_effect,
}
