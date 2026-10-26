//! アクセシビリティ・SEO 要件の実ブラウザ確認用エントリポイント（`wasm32` 専用、
//! TASK-4.4-S4 / Issue #82）。
//!
//! `super::mount` は wasm32 専用の実 DOM マウント層だが、`Screen<S>` を組み立てて
//! `mount` へ渡す呼び出し元（本来はアプリ層、TASK-4.4-S3 の `RenderMode` 選択導線経由）が
//! まだ存在しないため、`scripts/web/verify-dom-a11y.sh`（`Accessibility.getFullAXTree` 相当の
//! 確認）が使う最小のエントリポイントをここに用意する。**検証専用**であり、正式な
//! 選択導線（S3、`fandhe-core::ui::RenderMode`）とは無関係（`fandhe-egui-proxy` の
//! `web.rs` が検証専用プロキシであるのと同じ位置づけ）。
//!
//! `fandhe_core::sample_screen()` は `SampleState::default()` だと `items` が空になり、
//! `<ul>` に `<li>` が 1 つも現れず AX ツリーで `listitem` ロールを観測できない
//! （`tests/a11y_seo.rs` の `empty_list_state_produces_ul_with_no_listitem` が固定化
//! 済みの前提）。そのため本エントリポイントはマウント前に項目を 2 件事前投入した状態で
//! `mount` する。

use fandhe_core::screens::accessibility_prefs_show::{
    current_screen as current_accessibility_prefs_screen, AccessibilityPrefsShowState,
};
use fandhe_core::screens::app_badge_set::{
    current_screen as current_app_badge_set_screen, AppBadgeSetState,
};
use fandhe_core::screens::app_lifecycle_show::{
    current_screen as current_app_lifecycle_show_screen, AppLifecycleShowState,
};
use fandhe_core::screens::ar_show::{
    current_screen as current_ar_screen, stop as ar_stop, ArShowState,
    STOP_BUTTON_ID as AR_STOP_BUTTON_ID, STOP_LABEL as AR_STOP_LABEL,
};
use fandhe_core::screens::audio_devices_show::{
    current_screen as current_audio_devices_screen, AudioDevicesShowState,
};
use fandhe_core::screens::audio_play::{
    current_screen as current_audio_play_screen, AudioPlayState,
};
use fandhe_core::screens::background_download::{
    current_screen as current_background_download_screen, BackgroundDownloadState,
};
use fandhe_core::screens::background_task::{
    current_screen as current_background_task_screen, BackgroundTaskState,
};
use fandhe_core::screens::barcode_scan::{
    current_screen as current_barcode_scan_screen, BarcodeScanState,
};
use fandhe_core::screens::biometric_check::{
    current_screen as current_biometric_screen, BiometricCheckState,
};
use fandhe_core::screens::ble_scan::{current_screen as current_ble_scan_screen, BleScanState};
use fandhe_core::screens::camera_capture::{
    current_screen as current_camera_capture_screen, CameraCaptureState,
};
use fandhe_core::screens::camera_preview_stream_show::{
    current_screen as current_camera_preview_stream_screen, CameraPreviewStreamShowState,
};
use fandhe_core::screens::cast_show::{current_screen as current_cast_show_screen, CastShowState};
use fandhe_core::screens::checkbox_list::{checkbox_list_screen, CheckboxListState};
use fandhe_core::screens::clipboard_text::{
    current_screen as current_clipboard_screen, ClipboardTextState,
};
use fandhe_core::screens::compass_show::{
    current_screen as current_compass_screen, stop as compass_stop, CompassShowState,
    STOP_BUTTON_ID as COMPASS_STOP_BUTTON_ID, STOP_LABEL as COMPASS_STOP_LABEL,
};
use fandhe_core::screens::compose_mail_sms::{
    current_screen as current_compose_mail_sms_screen, ComposeMailSmsState,
};
use fandhe_core::screens::contact_pick::{
    current_screen as current_contact_pick_screen, ContactPickState,
};
use fandhe_core::screens::device_info_show::{
    current_screen as current_device_info_screen, DeviceInfoShowState,
};
use fandhe_core::screens::display_refresh_hdr_show::{
    current_screen as current_display_refresh_hdr_show_screen, DisplayRefreshHdrShowState,
};
use fandhe_core::screens::drag_drop::{current_screen as current_drag_drop_screen, DragDropState};
use fandhe_core::screens::external_app_launch_show::{
    current_screen as current_external_app_launch_show_screen, ExternalAppLaunchState,
};
use fandhe_core::screens::eye_dropper_pick::{
    current_screen as current_eye_dropper_screen, EyeDropperState,
};
use fandhe_core::screens::file_association_show::{
    file_association_show_screen, FileAssociationShowState,
};
use fandhe_core::screens::file_pick::{current_screen as current_file_pick_screen, FilePickState};
use fandhe_core::screens::file_watch_show::{
    current_screen as current_file_watch_screen, stop as file_watch_stop, FileWatchShowState,
    STOP_LABEL as FILE_WATCH_STOP_LABEL,
};
use fandhe_core::screens::flashlight_toggle::{
    current_screen as current_flashlight_toggle_screen, FlashlightToggleState,
    WEB_NOTE as FLASHLIGHT_WEB_NOTE,
};
use fandhe_core::screens::foldable_posture_show::{
    foldable_posture_show_web_screen, FoldablePostureShowState,
};
use fandhe_core::screens::fullscreen_toggle::{
    current_screen as current_fullscreen_toggle_screen, exit as fullscreen_exit,
    FullscreenToggleState, EXIT_BUTTON_ID as FULLSCREEN_EXIT_BUTTON_ID,
    EXIT_LABEL as FULLSCREEN_EXIT_LABEL,
};
use fandhe_core::screens::gamepad_show::{
    current_screen as current_gamepad_screen, GamepadShowState,
};
use fandhe_core::screens::haptics_tap::{
    current_screen as current_haptics_tap_screen, HapticsTapState,
};
use fandhe_core::screens::http_fetch::{
    current_screen as current_http_fetch_screen, HttpFetchState,
};
use fandhe_core::screens::local_fonts_show::{
    current_screen as current_local_fonts_screen, LocalFontsShowState,
};
use fandhe_core::screens::location_show::{
    current_screen as current_location_screen, LocationShowState,
};
use fandhe_core::screens::media_transcode::{
    current_screen as current_media_transcode_screen, TranscodeState,
};
use fandhe_core::screens::memory_pressure_show::{
    current_screen as current_memory_pressure_show_screen, MemoryPressureShowState,
};
use fandhe_core::screens::mic_record::{
    current_screen as current_mic_record_screen, MicRecordState,
};
use fandhe_core::screens::midi_show::{
    clear as midi_clear, current_screen as current_midi_screen, stop as midi_stop, MidiShowState,
    CLEAR_LABEL as MIDI_CLEAR_LABEL, STOP_LABEL as MIDI_STOP_LABEL,
};
use fandhe_core::screens::motion_raw_sensors_show::{
    current_screen as current_motion_raw_sensors_show_screen, MotionRawSensorsShowState,
};
use fandhe_core::screens::multi_display_show::{
    current_screen as current_multi_display_screen,
    web_initial_state as multi_display_initial_state,
};
use fandhe_core::screens::network_status_show::{
    network_status_show_screen, NetworkStatusShowState,
};
use fandhe_core::screens::nlp_segment::{
    current_screen as current_nlp_segment_screen, NlpSegmentState,
};
use fandhe_core::screens::notification_send::{
    current_screen as current_notification_send_screen, NotificationSendState,
};
use fandhe_core::screens::on_device_llm::{
    current_screen as current_on_device_llm_screen, OnDeviceLlmState,
};
use fandhe_core::screens::otp_input::{fixed_screen as fixed_otp_input_screen, OtpInputState};
use fandhe_core::screens::passkey_signin::{
    current_screen as current_passkey_signin_screen, PasskeySigninState,
};
use fandhe_core::screens::payment_request::{
    current_screen as current_payment_request_screen, PaymentRequestState,
};
use fandhe_core::screens::pen_draw::{current_screen as current_pen_draw_screen, PenDrawState};
use fandhe_core::screens::photo_pick::{
    current_screen as current_photo_pick_screen, PhotoPickState,
};
use fandhe_core::screens::pip_show::{current_screen as current_pip_show_screen, PipShowState};
use fandhe_core::screens::power_state_show::{
    current_screen as current_power_state_screen, PowerStateShowState,
};
use fandhe_core::screens::print_page::{
    current_screen as current_print_page_screen, PrintPageState,
};
use fandhe_core::screens::safe_area_insets_show::{
    safe_area_insets_show_web_screen, SafeAreaInsetsShowState,
};
use fandhe_core::screens::screen_capture::{
    current_screen as current_screen_capture_screen, ScreenCaptureState,
};
use fandhe_core::screens::screen_orientation_show::{
    current_screen as current_screen_orientation_screen,
    lock_landscape as orientation_lock_landscape, lock_portrait as orientation_lock_portrait,
    reload as orientation_reload, unlock as orientation_unlock, ScreenOrientationShowState,
    LOCK_LANDSCAPE_ID, LOCK_LANDSCAPE_LABEL, LOCK_PORTRAIT_ID, LOCK_PORTRAIT_LABEL, RELOAD_ID,
    RELOAD_LABEL, UNLOCK_ID, UNLOCK_LABEL,
};
use fandhe_core::screens::serial_port_show::{
    current_screen as current_serial_port_show_screen, SerialPortShowState,
};
use fandhe_core::screens::settings::{
    get_notifications_on, set_notifications_on, settings_screen, SettingsState,
};
use fandhe_core::screens::share_receive_show::{share_receive_show_screen, ShareReceiveShowState};
use fandhe_core::screens::share_text::{current_screen as current_share_screen, ShareTextState};
use fandhe_core::screens::speech_recognize::{
    current_screen as current_speech_recognize_screen, SpeechRecognizeState,
};
use fandhe_core::screens::storage_note::{storage_note_screen, StorageNoteState};
use fandhe_core::screens::system_accent_color_show::{
    system_accent_color_show_web_screen, SystemAccentColorShowState,
};
use fandhe_core::screens::text_scale_preview::{
    current_screen as current_text_scale_screen, TextScalePreviewState,
};
use fandhe_core::screens::tilt_ball::{
    current_screen as current_tilt_ball_screen, SensorStatus, TiltBallState,
};
use fandhe_core::screens::translation::{
    current_screen as current_translation_screen, TranslationState,
};
use fandhe_core::screens::tsv_import::{
    current_screen as current_tsv_import_screen, TsvImportState,
};
use fandhe_core::screens::tts_speak::{current_screen as current_tts_screen, TtsSpeakState};
use fandhe_core::screens::url_scheme_show::{
    register, url_scheme_show_screen, UrlSchemeShowState, REGISTER_BUTTON_ID,
};
use fandhe_core::screens::usb_device_show::{
    current_screen as current_usb_device_show_screen, UsbDeviceShowState,
};
use fandhe_core::screens::user_idle_show::{
    current_screen as current_user_idle_show_screen,
    web_initial_state as user_idle_web_initial_state,
};
use fandhe_core::screens::video_capture_show::{
    current_screen as current_video_capture_show_screen, VideoCaptureShowState,
};
use fandhe_core::screens::wake_lock_toggle::{
    current_screen as current_wake_lock_toggle_screen, turn_off as wake_lock_turn_off,
    WakeLockToggleState, OFF_BUTTON_ID as WAKE_LOCK_OFF_BUTTON_ID,
    OFF_LABEL as WAKE_LOCK_OFF_LABEL,
};
use fandhe_core::screens::window_management_show::current_screen as current_window_management_screen;
use fandhe_core::{sample_screen, SampleState, WidgetId};
use wasm_bindgen::prelude::*;

/// `root_id` の要素を取得する（3 エントリポイント共通の下ごしらえ）。
/// 見つからない場合はエラーを返す（panic を API 契約にしない方針、coding-rust.md 準拠）。
fn root_element(root_id: &str) -> Result<web_sys::Element, JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("window が取得できない"))?
        .document()
        .ok_or_else(|| JsValue::from_str("document が取得できない"))?;
    document
        .get_element_by_id(root_id)
        .ok_or_else(|| JsValue::from_str(&format!("id=\"{root_id}\" の要素が見つからない")))
}

/// `root_id` が指す要素へ `sample_screen()` をマウントする（`scripts/web/verify-dom-a11y.sh`
/// から `web/index.html` 経由で呼び出される検証専用エントリポイント）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す（panic を API 契約に
/// しない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_sample_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = SampleState {
        memo: "下書き".to_string(),
        items: vec!["item-1".to_string(), "item-2".to_string()],
        ..Default::default()
    };

    super::mount(root, sample_screen(), initial_state)
}

/// `root_id` が指す要素へ Phase 11 型サンプル「クリップボード」（Issue #859）を
/// マウントする検証専用エントリポイント。`super::clipboard::clipboard_text_effect` を
/// effect として渡すため、`navigator.clipboard` 経由のコピー → 貼り付けの往復が動く。
///
/// 画面は `current_screen(&ClipboardTextState::default())`（= 要求なしの状態）で
/// 固定する。`mount` は `Screen<S>` を受け取る設計で `fn(&S) -> Screen<S>` の
/// 再構築に対応しないため、処理中にボタンを隠す分岐（`current_screen` の
/// `pending` 判定）は Web では効かない。二重押しは `fandhe-core` 側の
/// `copy`/`paste` が `pending` で弾くため実害はない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_clipboard_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ClipboardTextState::default();
    super::mount::mount_with_effect(
        root,
        current_clipboard_screen(&initial_state),
        initial_state,
        super::clipboard::clipboard_text_effect,
    )
}

/// `root_id` が指す要素へ Phase 11 型サンプル「読み上げ」（Issue #867）を
/// マウントする検証専用エントリポイント。`super::tts::tts_speak_effect` を
/// effect として渡すため、`window.speechSynthesis` 経由の発話 → 停止が動く。
///
/// 画面は `current_screen(&TtsSpeakState::default())`（= 要求なしの状態）で
/// 固定する。`mount_clipboard_screen` と同じ理由で「読み上げ」「停止」両ボタンが
/// 常時表示される（`take_speak_request` / `take_stop_request` の
/// `_dispatched` フラグで二重発送は core 側が弾く）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_tts_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = TtsSpeakState::default();
    super::mount::mount_with_effect(
        root,
        current_tts_screen(&initial_state),
        initial_state,
        super::tts::tts_speak_effect,
    )
}

/// `root_id` が指す要素へ Phase 11 型サンプル「ファイル選択」（Issue #875）を
/// マウントする検証専用エントリポイント。`super::file_picker::file_pick_effect` を
/// effect として渡すため、非表示 `<input type="file">` 経由の選択 → 読み取りが動く。
///
/// 画面は `current_screen(&FilePickState::default())`（= 要求なしの状態）で固定する。
/// `mount_clipboard_screen`/`mount_tts_screen` と同じ理由でボタンが常時表示されるが、
/// `fandhe_core::screens::file_pick::request_pick` が `Idle` 以外は no-op のため
/// 二重要求は実害にならない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_file_pick_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = FilePickState::default();
    super::mount::mount_with_effect(
        root,
        current_file_pick_screen(&initial_state),
        initial_state,
        super::file_picker::file_pick_effect,
    )
}

/// `root_id` が指す要素へ Phase 11 型サンプル「写真選択」（Issue #1103）をマウントする
/// 検証専用エントリポイント。`super::file_picker::photo_pick_effect` を effect として渡すため、
/// `<input type="file" accept="image/*">` 経由の選択 → `createImageBitmap` での復号確認 →
/// `<img src="blob:...">` 表示が動く。`root_id` が見つからなければエラーを返す。
#[wasm_bindgen]
pub fn mount_photo_pick_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PhotoPickState::default();
    super::mount::mount_with_effect(
        root,
        current_photo_pick_screen(&initial_state),
        initial_state,
        super::file_picker::photo_pick_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「動画の撮影」（Issue #824）をマウントする検証専用
/// エントリポイント。`super::video_capture::video_capture_show_effect` を effect として渡すため、
/// `<input type="file" accept="video/*" capture="environment">` 経由の選択 →
/// `<video>` の `loadedmetadata` での長さ・寸法の読み取りが動く。
#[wasm_bindgen]
pub fn mount_video_capture_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = VideoCaptureShowState::default();
    super::mount::mount_with_effect(
        root,
        current_video_capture_show_screen(&initial_state),
        initial_state,
        super::video_capture::video_capture_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「バーコード・QR 読み取り」（Issue #559）をマウントする
/// 検証専用エントリポイント。`super::barcode_scan::barcode_scan_effect` を effect として渡す。
#[wasm_bindgen]
pub fn mount_barcode_scan_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = BarcodeScanState::default();
    super::mount::mount_with_effect(
        root,
        current_barcode_scan_screen(&initial_state),
        initial_state,
        super::barcode_scan::barcode_scan_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「パスワード・ワンタイムコードの自動入力」（Issue #587）を
/// マウントする検証専用エントリポイント。`super::autofill_otp::otp_input_effect` を effect として渡す。
#[wasm_bindgen]
pub fn mount_otp_input_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = OtpInputState::default();
    super::mount::mount_with_effect(
        root,
        fixed_otp_input_screen(),
        initial_state,
        super::autofill_otp::otp_input_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「位置情報」（Issue #891）をマウントする検証専用
/// エントリポイント。`super::location::location_show_effect` を effect として渡すため、
/// `navigator.geolocation.watchPosition` 経由の継続測位が動く。
///
/// 画面は `current_screen(&LocationShowState::default())`（= 取得中）で固定する。
/// 他画面と同じ理由（`mount` は `Screen<S>` の再構築に対応しない）で「再試行」ボタンは
/// 常時表示されるが、押下は `location_show_effect` の再試行検知（`state.view` の変化）で
/// 拾えるため実害はない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_location_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = LocationShowState::default();
    super::mount::mount_with_effect(
        root,
        current_location_screen(&initial_state),
        initial_state,
        super::location::location_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「共有」（Issue #900）をマウントする検証専用
/// エントリポイント。`super::share::share_text_effect` を effect として渡すため、
/// `navigator.share` 経由の共有シート表示 → 結果反映が動く。
///
/// 画面は `current_screen(&ShareTextState::default())`（= 要求なしの状態）で固定する。
/// 他画面と同じ理由（`mount` は `Screen<S>` の再構築に対応しない）で「共有」ボタンは
/// 常時表示されるが、`fandhe_core::screens::share_text::share` が `Idle` 以外は no-op の
/// ため二重要求は実害にならない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_share_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ShareTextState::default();
    super::mount::mount_with_effect(
        root,
        current_share_screen(&initial_state),
        initial_state,
        super::share::share_text_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「BLE スキャン」（Issue #1063）をマウントする検証専用
/// エントリポイント。`super::bluetooth_le::ble_scan_effect` を effect として渡すため、
/// Web Bluetooth の `requestDevice` による 1 台選択 → 結果反映が動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_ble_scan_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = BleScanState::default();
    super::mount::mount_with_effect(
        root,
        current_ble_scan_screen(&initial_state),
        initial_state,
        super::bluetooth_le::ble_scan_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「USB・HID 機器」（親 #112、Issue #508）をマウントする検証専用
/// エントリポイント。`super::usb_device::usb_device_show_effect` が WebUSB / WebHID の
/// `requestDevice` を駆動する。初期状態は要求なし（2 つの選択ボタンから始まる）。
#[wasm_bindgen]
pub fn mount_usb_device_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = UsbDeviceShowState::browser_picker();
    super::mount::mount_with_effect(
        root,
        current_usb_device_show_screen(&initial_state),
        initial_state,
        super::usb_device::usb_device_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「シリアルポート」（親 #131、Issue #672）をマウントする検証専用
/// エントリポイント。`super::serial_port::serial_port_show_effect` が Web Serial API の
/// `requestPort` と固定 1 行の往復を駆動する。初期状態は要求なし（ボタン「ポートを選んで往復」から始まる）。
/// 処理中の二重押下は core 側で no-op になる。
#[wasm_bindgen]
pub fn mount_serial_port_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = SerialPortShowState::browser_picker();
    super::mount::mount_with_effect(
        root,
        current_serial_port_show_screen(&initial_state),
        initial_state,
        super::serial_port::serial_port_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「アプリの前面・背面」（親 #178、Issue #896）をマウントする検証専用
/// エントリポイント。`super::app_lifecycle::app_lifecycle_show_effect` が focus / visibilitychange 等を購読する。
#[wasm_bindgen]
pub fn mount_app_lifecycle_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = AppLifecycleShowState::default();
    super::mount::mount_with_effect(
        root,
        current_app_lifecycle_show_screen(&initial_state),
        initial_state,
        super::app_lifecycle::app_lifecycle_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「メモリ逼迫・負荷」（親 #179、Issue #914）をマウントする検証専用
/// エントリポイント。`super::memory_pressure::memory_pressure_show_effect` が Compute Pressure API を駆動する。
#[wasm_bindgen]
pub fn mount_memory_pressure_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = MemoryPressureShowState::default();
    super::mount::mount_with_effect(
        root,
        current_memory_pressure_show_screen(&initial_state),
        initial_state,
        super::memory_pressure::memory_pressure_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ジャイロ・磁気・線形加速度」（親 #180、Issue #902）をマウントする検証専用
/// エントリポイント。`super::motion_raw_sensors::motion_raw_sensors_show_effect` が `devicemotion` を駆動する。
#[wasm_bindgen]
pub fn mount_motion_raw_sensors_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = MotionRawSensorsShowState::default();
    super::mount::mount_with_effect(
        root,
        current_motion_raw_sensors_show_screen(&initial_state),
        initial_state,
        super::motion_raw_sensors::motion_raw_sensors_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「無操作時間の検知」（親 #125、Issue #632）をマウントする検証専用
/// エントリポイント。`super::user_idle::user_idle_show_effect` が Idle Detection API を駆動する。
/// 許可ボタンは固定 Screen のため全状態で表示され、`request_watch` が再要求を担う。
#[wasm_bindgen]
pub fn mount_user_idle_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = user_idle_web_initial_state();
    super::mount::mount_with_effect(
        root,
        current_user_idle_show_screen(&initial_state),
        initial_state,
        super::user_idle::user_idle_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「連絡先を選ぶ」（Issue #1223）をマウントする検証専用
/// エントリポイント。`super::contact_picker::contact_pick_effect` を effect として渡すため、
/// Contact Picker API による 1 件選択 → 結果反映が動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_contact_pick_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ContactPickState::default();
    super::mount::mount_with_effect(
        root,
        current_contact_pick_screen(&initial_state),
        initial_state,
        super::contact_picker::contact_pick_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「生体認証」（Issue #909）をマウントする検証専用
/// エントリポイント。`super::biometric_auth::biometric_check_effect` を effect として
/// 渡すため、`navigator.credentials` 経由の WebAuthn 登録・認証 → 結果反映が動く。
///
/// 画面は `current_screen(&BiometricCheckState::default())`（= 要求なしの状態）で
/// 固定する。他画面と同じ理由（`mount` は `Screen<S>` の再構築に対応しない）で
/// 「認証する」ボタンは常時表示されるが、`fandhe_core::screens::biometric_check::authenticate`
/// が `Idle` 以外は no-op のため二重要求は実害にならない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_biometric_check_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = BiometricCheckState::default();
    super::mount::mount_with_effect(
        root,
        current_biometric_screen(&initial_state),
        initial_state,
        super::biometric_auth::biometric_check_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「文字サイズ追従」（Issue #915）をマウントする
/// 検証専用エントリポイント。`super::text_scale::text_scale_effect` を effect として
/// 渡すため、`getComputedStyle` 経由の既定フォントサイズ読み取り → 状態反映と、
/// `ResizeObserver` 経由の再読込が動く。
///
/// 画面は `current_screen(&TextScalePreviewState::default())`（= 未読み取り状態）で
/// 固定する。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_text_scale_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = TextScalePreviewState::default();
    super::mount::mount_with_effect(
        root,
        current_text_scale_screen(&initial_state),
        initial_state,
        super::text_scale::text_scale_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ゲームコントローラ」（Issue #936）をマウントする
/// 検証専用エントリポイント。`super::gamepad::gamepad_show_effect` を effect として
/// 渡すため、`navigator.getGamepads()` 経由の毎フレームポーリングが動く。
///
/// 画面は `current_screen(&GamepadShowState::default())`（= 未接続・代替入力状態）で
/// 固定する。Web DOM 出力では代替入力ウィジェット（スライダー・チェックボックス）が
/// `Node::Widget` の空 `div` プレースホルダとして描画され操作できない
/// （`gamepad.rs` モジュール doc「非標準配置の扱い」節参照、既存の天井）ため、
/// 盤面キャンバスの絵も含めて実際に観測できるのは `status_text`（接続状態・玉の位置・
/// 押下中ボタン）の `<p>` のみ。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_gamepad_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = GamepadShowState::default();
    super::mount::mount_with_effect(
        root,
        current_gamepad_screen(&initial_state),
        initial_state,
        super::gamepad::gamepad_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「電源状態」（Issue #1277）をマウントする検証専用
/// エントリポイント。`super::power_state::power_state_show_effect` を effect として渡すため、
/// `navigator.getBattery()` 経由の 1 秒ごとの読み取りが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_power_state_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PowerStateShowState::default();
    super::mount::mount_with_effect(
        root,
        current_power_state_screen(&initial_state),
        initial_state,
        super::power_state::power_state_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「端末・OS 情報」（Issue #1307）をマウントする検証専用
/// エントリポイント。`super::device_info::device_info_show_effect` を effect として渡すため、
/// ボタン押下で `navigator.userAgentData` / `deviceMemory` 経由の読み取りが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_device_info_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = DeviceInfoShowState::default();
    super::mount::mount_with_effect(
        root,
        current_device_info_screen(&initial_state),
        initial_state,
        super::device_info::device_info_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「音声デバイス」（Issue #1333）をマウントする検証専用
/// エントリポイント。`super::audio_devices::audio_devices_show_effect` を effect として渡すため、
/// 起動時と「再取得」で `enumerateDevices()` の読み取りが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_audio_devices_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = AudioDevicesShowState::default();
    super::mount::mount_with_effect(
        root,
        current_audio_devices_screen(&initial_state),
        initial_state,
        super::audio_devices::audio_devices_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「画面のスポイト」（Issue #763）をマウントする検証専用
/// エントリポイント。`super::eye_dropper::eye_dropper_pick_effect` を effect として渡す。
/// 起動時には呼ばず、ボタン押下（ユーザー操作）の中で `EyeDropper.open()` が動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_eye_dropper_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = EyeDropperState::default();
    super::mount::mount_with_effect(
        root,
        current_eye_dropper_screen(&initial_state),
        initial_state,
        super::eye_dropper::eye_dropper_pick_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「OS 搭載フォント」（Issue #1379）をマウントする検証専用
/// エントリポイント。`super::local_fonts::local_fonts_show_effect` を effect として渡す。
/// 起動時には読まず、ボタン押下で `queryLocalFonts()` の許可要求と 1 回読みが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_local_fonts_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = LocalFontsShowState::default();
    super::mount::mount_with_effect(
        root,
        current_local_fonts_screen(&initial_state),
        initial_state,
        super::local_fonts::local_fonts_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「複数ディスプレイ」（Issue #1344）をマウントする検証専用
/// エントリポイント。`super::multi_display::multi_display_show_effect` を effect として渡すため、
/// ボタン押下で `getScreenDetails()` の許可要求と 1 秒ごとの読み取りが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_multi_display_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = multi_display_initial_state();
    super::mount::mount_with_effect(
        root,
        current_multi_display_screen(&initial_state),
        initial_state,
        super::multi_display::multi_display_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ウィンドウ管理」（Issue #638）をマウントする検証専用
/// エントリポイント。`super::window_management::window_management_show_effect` を effect として渡すため、
/// ボタン押下で `moveTo` / `resizeTo` の依頼と読み戻しが動く。
///
/// `mount` はボタン構成を組み直さないため、マウント時に読んだ display-mode で画面を作る。
/// PWA の窓では移動・サイズのボタンを出し、ブラウザのタブではボタンを出さず案内だけを出す。
/// 処理中の二重押下は core 側の `Idle` 以外 no-op で吸収する。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_window_management_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = super::window_management::web_initial_state();
    super::mount::mount_with_effect(
        root,
        current_window_management_screen(&initial_state),
        initial_state,
        super::window_management::window_management_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「アクセシビリティ設定の追従」（Issue #1311）をマウントする
/// 検証専用エントリポイント。`super::accessibility_prefs::accessibility_prefs_show_effect` を
/// effect として渡すため、`matchMedia` 経由の 1 秒ごとの読み取りと丸の更新が動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_accessibility_prefs_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = AccessibilityPrefsShowState::default();
    super::mount::mount_with_effect(
        root,
        current_accessibility_prefs_screen(&initial_state),
        initial_state,
        super::accessibility_prefs::accessibility_prefs_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「システムのアクセント色」（Issue #423）をマウントする
/// 検証専用エントリポイント。`super::system_accent_color::system_accent_color_show_effect` を
/// effect として渡すため、CSS `AccentColor` の 1 秒ごとの読み直しが動く。
/// 画面は「ブラウザが返した色」の注記付き（`system_accent_color_show_web_screen`）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_system_accent_color_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    super::mount::mount_with_effect(
        root,
        system_accent_color_show_web_screen(),
        SystemAccentColorShowState::default(),
        super::system_accent_color::system_accent_color_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「セーフエリア」（Issue #447）をマウントする検証専用
/// エントリポイント。`super::safe_area_insets::safe_area_insets_show_effect` を effect として渡すため、
/// CSS `env(safe-area-inset-*)` の 1 秒ごとの読み直しが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_safe_area_insets_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    super::mount::mount_with_effect(
        root,
        safe_area_insets_show_web_screen(),
        SafeAreaInsetsShowState::default(),
        super::safe_area_insets::safe_area_insets_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「折りたたみの姿勢・ヒンジ」（Issue #724）をマウントする検証専用
/// エントリポイント。`super::foldable_posture::foldable_posture_show_effect` を effect として渡すため、
/// Device Posture API と Viewport Segments API の 1 秒ごとの読み直しが動く。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_foldable_posture_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    super::mount::mount_with_effect(
        root,
        foldable_posture_show_web_screen(),
        FoldablePostureShowState::default(),
        super::foldable_posture::foldable_posture_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ファイルの関連付け」（Issue #419）をマウントする検証専用
/// エントリポイント。`super::file_association::file_association_show_effect` を effect として渡すため、
/// インストール済み PWA から `.txt` を開くと `launchQueue` 経由で受信結果が出る。
/// Web は登録操作が不要（manifest の `file_handlers` のみ）なので登録ボタンは足さない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_file_association_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    super::mount::mount_with_effect(
        root,
        file_association_show_screen(),
        FileAssociationShowState::default(),
        super::file_association::file_association_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「マイク録音」（Issue #958）をマウントする検証専用
/// エントリポイント。`super::mic_record::mic_record_effect` を effect として渡すため、
/// `navigator.mediaDevices.getUserMedia` + `MediaRecorder` 経由の録音 → 停止が動く。
///
/// 画面は `current_screen(&MicRecordState::default())`（= 待機状態、「録音開始」ボタン）で
/// 固定する（`mount` は `Screen<S>` の再構築に対応しないため、他画面と同じ理由で
/// 「停止」ボタンは表示されない。v1 は `MAX_RECORD_MS` 到達での自動停止のみを Web の
/// 検証エントリポイントで観測する、`mic_record.rs` モジュール doc「既知の天井」節参照）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_mic_record_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = MicRecordState::default();
    super::mount::mount_with_effect(
        root,
        current_mic_record_screen(&initial_state),
        initial_state,
        super::mic_record::mic_record_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「画面の録画・キャプチャ」（Issue #1338）をマウントする検証専用
/// エントリポイント。`super::screen_capture::screen_capture_effect` を effect として渡すため、
/// `getDisplayMedia()` + `MediaRecorder` 経由の録画が動く。
///
/// 「停止」ボタンは出ない（`mount` は `Screen<S>` を再構築しない、`mic_record` と同じ天井）。
/// 録画は 10 秒の自動停止、またはブラウザの「共有を停止」で終わる。
#[wasm_bindgen]
pub fn mount_screen_capture_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ScreenCaptureState::default();
    super::mount::mount_with_effect(
        root,
        current_screen_capture_screen(&initial_state),
        initial_state,
        super::screen_capture::screen_capture_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「TSV 読み込み」（親 #979、Issue #981）をマウントする
/// 検証専用エントリポイント。`super::file_picker::tsv_import_effect` を effect として
/// 渡すため、`file_pick`（Issue #875）と共有する非表示 `<input type="file">` 経由の
/// 選択 → 読み取り → `fandhe_core::tsv::parse_tsv` での解析が動く。
///
/// 画面は `current_screen(&TsvImportState::default())`（= 要求なしの状態）で固定する。
/// `mount_file_pick_screen` と同じ理由（`mount` は `Screen<S>` の再構築に対応しない）で
/// 「TSV を選ぶ」「サンプルを読み込む」両ボタンが常時表示されるが、
/// `fandhe_core::screens::tsv_import::request_import`/`load_sample` が `Idle` 以外は
/// no-op のため二重要求は実害にならない。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_tsv_import_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = TsvImportState::default();
    super::mount::mount_with_effect(
        root,
        current_tsv_import_screen(&initial_state),
        initial_state,
        super::file_picker::tsv_import_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「HTTP 取得（GET 限定）」（Issue #985）をマウントする
/// 検証専用エントリポイント。`super::http_fetch::http_fetch_effect` を effect として
/// 渡すため、ブラウザの `fetch`（Fetch API）経由の GET 取得が動く。
///
/// 画面は `current_screen(&HttpFetchState::default())`（= 要求なしの状態、「取得」
/// ボタン）で固定する。他画面と同じ理由（`mount` は `Screen<S>` の再構築に対応しない）で
/// `Failed` 時にも「取得」ボタンが表示され続けるが、
/// `fandhe_core::screens::http_fetch::start_fetch` は `Failed` からも `Requested` へ
/// 進むため、そのボタンで再試行できる。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_http_fetch_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = HttpFetchState::default();
    super::mount::mount_with_effect(
        root,
        current_http_fetch_screen(&initial_state),
        initial_state,
        super::http_fetch::http_fetch_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「メモ保存」（Issue #987）をマウントする検証専用
/// エントリポイント。`storage_note_screen()` は他画面と異なり effect を使わない
/// （`storage_note.rs` モジュール doc「同期ブリッジ」節参照）。
/// `StorageNoteState::with_storage` に `LocalNoteStorage`（`localStorage` 経由）を
/// 渡すことで、保存済みメモの読み込みによる復元・保存・失敗表示は core 側の既存ロジック
/// がそのまま処理する。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_storage_note_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state =
        StorageNoteState::with_storage(Box::new(super::storage_note::LocalNoteStorage));
    super::mount(root, storage_note_screen(), initial_state)
}

/// `root_id` が指す要素へ型サンプル「傾きで玉が転がる」（Issue #989）をマウントする
/// 検証専用エントリポイント。`super::tilt::tilt_ball_effect` を effect として渡すため、
/// `window` の `deviceorientation` 経由の継続取得と毎フレームの物理更新が動く。
///
/// 画面は `current_screen(&TiltBallState { sensor: Waiting, .. })`（= 取得中）で
/// 固定する。センサ無し・許可されていない環境では `tilt_ball_effect` が
/// `Unavailable` へ書き換える（`tilt.rs` モジュール doc「嘘の成功を出さない」節参照）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_tilt_ball_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let mut initial_state = TiltBallState::default();
    initial_state.sensor = SensorStatus::Waiting;
    super::mount::mount_with_effect(
        root,
        current_tilt_ball_screen(&initial_state),
        initial_state,
        super::tilt::tilt_ball_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「カメラ撮影」（親 #990、Issue #991）をマウントする
/// 検証専用エントリポイント。`super::camera_capture::camera_capture_effect` を effect
/// として渡すため、`navigator.mediaDevices.getUserMedia` 経由の撮影 → サムネイル反映が動く。
///
/// 画面は `current_screen(&CameraCaptureState::default())`（= 未撮影、「撮影」ボタン）で
/// 固定する（`mount` は `Screen<S>` の再構築に対応しないため、他画面と同じ理由で撮影中も
/// ボタンが表示され続けるが、`fandhe_core::screens::camera_capture::start_capture` が
/// `Requested`/`Capturing` 中は no-op のため二重要求は実害にならない）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_camera_capture_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = CameraCaptureState::default();
    super::mount::mount_with_effect(
        root,
        current_camera_capture_screen(&initial_state),
        initial_state,
        super::camera_capture::camera_capture_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「カメラのライブ映像・フレーム取得」（親 #168、Issue #815）を
/// マウントする検証専用エントリポイント。`super::camera_preview_stream::camera_preview_stream_show_effect`
/// を effect として渡すため、`getUserMedia` 経由の映像開始 → フレーム表示が動く。
///
/// 起動時には映像を始めない（「映像を始める」ボタンの操作を起点にする）。画面は
/// `current_screen(&CameraPreviewStreamShowState::default())` で固定するため、トグルの文言は
/// 「映像を始める」のまま変わらず、ズームのボタンも出ない（`mic_record` と同じ既存の制約）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_camera_preview_stream_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = CameraPreviewStreamShowState::default();
    super::mount::mount_with_effect(
        root,
        current_camera_preview_stream_screen(&initial_state),
        initial_state,
        super::camera_preview_stream::camera_preview_stream_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「通知」（親 #1003、Issue #1007）をマウントする
/// 検証専用エントリポイント。`super::notification::notification_send_effect` を effect
/// として渡すため、`Notification`（Web Notifications API）経由の権限確認 → 表示が動く。
///
/// 画面は `current_screen(&NotificationSendState::default())`（= 要求なしの状態、
/// 「通知する」ボタン）で固定する（`mount` は `Screen<S>` の再構築に対応しないため、
/// 他画面と同じ理由で送信中もボタンが表示され続けるが、
/// `fandhe_core::screens::notification_send::notify` が `Idle` 以外は no-op のため
/// 二重要求は実害にならない）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_notification_send_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = NotificationSendState::default();
    super::mount::mount_with_effect(
        root,
        current_notification_send_screen(&initial_state),
        initial_state,
        super::notification::notification_send_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ドラッグ & ドロップ受け取り」（親 #1013、
/// Issue #1017）をマウントする検証専用エントリポイント。
/// `super::drag_drop::drag_drop_effect` を effect として渡すため、`document` の
/// `dragenter`/`dragover`/`dragleave`/`drop`（HTML Drag and Drop API）経由の
/// ホバー・受け取りが動く。
///
/// 画面は `current_screen(&DragDropState::default())`（= 未操作の案内）で固定する
/// （`mount` は `Screen<S>` の再構築に対応しないが、本画面はボタンを持たず
/// `dynamic_text`/`list` のみのため他画面と異なり再構築非対応の影響を受けない）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_drag_drop_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = DragDropState::default();
    super::mount::mount_with_effect(
        root,
        current_drag_drop_screen(&initial_state),
        initial_state,
        super::drag_drop::drag_drop_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「設定」（Issue #993、UI 部品拡張の Web 配線検証用）を
/// マウントする検証専用エントリポイント。`settings_screen()` はネイティブ複製せず
/// そのまま使い、`effect` は使わない（`storage_note` と同じ理由、`toggle_switch`/
/// `Widget(Slider)`/`radio_button` の値変更は `fandhe_core::Widget::apply_value` 経由で
/// core 側の既存ロジックがそのまま処理する）。
///
/// `settings_screen()` には `Checkbox`（4 種のうち Web ブラウザ確認が未検証だった型）が
/// 含まれないため、本エントリポイントに限り「通知」トグルと同じ状態
/// （`notifications_on`）に束縛した `checkbox` を 1 行追加する。core の
/// `settings_screen()` 自体は変更しない（他プラットフォームの画面に影響しないため）。
/// 操作するとトグルと連動して「現在の設定」の集約テキストも一緒に変わる。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_settings_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let screen = settings_screen().checkbox(
        WidgetId::from_raw(7),
        "通知を受け取る（Checkbox 検証用）",
        get_notifications_on,
        set_notifications_on,
    );
    super::mount(root, screen, SettingsState::default())
}

/// `root_id` が指す要素へ型サンプル「音声認識」（親 #1023、Issue #1027）をマウントする
/// 検証専用エントリポイント。`super::speech_recognition::speech_recognize_effect` を
/// effect として渡すため、Web Speech API（`SpeechRecognition`/`webkitSpeechRecognition`）
/// 経由の聞き取り → 結果反映が動く。
///
/// 画面は `current_screen(&SpeechRecognizeState::default())`（= 要求なしの状態、「話す」
/// ボタン）で固定する（`mount` は `Screen<S>` の再構築に対応しないため、他画面と同じ理由で
/// 聞き取り中もボタンが表示され続けるが、`continuous = false` によりブラウザが発話の
/// 終わりで自動終了するため、正常系はこれで観測する、`speech_recognition.rs` モジュール
/// doc「Web 固有の天井」節参照）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_speech_recognize_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = SpeechRecognizeState::default();
    super::mount::mount_with_effect(
        root,
        current_speech_recognize_screen(&initial_state),
        initial_state,
        super::speech_recognition::speech_recognize_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「チェックボックス一覧」（親 #1036、Issue #1037）を
/// マウントする検証専用エントリポイント。`checkbox_list_screen()` は他画面と異なり
/// effect を使わない（`storage_note`/`mount_settings_screen` と同じ理由、
/// `Checkbox` の変更は `apply_value` 経由で core 側の setter がそのまま処理する）。
///
/// `Node::List` は `dom_tree.rs` で `ul > li` へ変換され、子の `Node::Widget` も
/// 再帰変換されるため（Issue #993）、追加配線なしで一覧の各行が
/// `<input type="checkbox">` として表示・操作できる。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_checkbox_list_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    super::mount(root, checkbox_list_screen(), CheckboxListState::default())
}

/// `root_id` が指す要素へ型サンプル「バックグラウンド処理」（親 #1042、Issue #1047）を
/// マウントする検証専用エントリポイント。`super::background_task::background_task_effect`
/// を effect として渡すため、「予約する」「予約を取り消す」ボタン → Background
/// Synchronization API（`web/background-task-sw.js`）経由の予約・取り消しと、
/// `Scheduled` の間の定期ポーリングが動く。
///
/// 画面は `current_screen(&BackgroundTaskState::default())`（= 要求なしの状態）で固定
/// する（`mount` は `Screen<S>` の再構築に対応しないため、他画面と同じ理由で両ボタンが
/// 表示され続けるが、`fandhe_core::screens::background_task` 側が `Idle` 以外・
/// `Scheduled` 中を no-op にするため実害は無い、`background_task.rs` モジュール doc参照）。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す
/// （panic を API 契約にしない方針、coding-rust.md 準拠）。
#[wasm_bindgen]
pub fn mount_background_task_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = BackgroundTaskState::default();
    super::mount::mount_with_effect(
        root,
        current_background_task_screen(&initial_state),
        initial_state,
        super::background_task::background_task_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「バックグラウンドダウンロード」（親 #100、Issue #393）を
/// マウントする検証専用エントリポイント。`super::background_download::background_download_effect`
/// が Background Fetch（`web/background-download-sw.js`）への依頼・取り消し・ポーリングを行う。
#[wasm_bindgen]
pub fn mount_background_download_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = BackgroundDownloadState::default();
    super::mount::mount_with_effect(
        root,
        current_background_download_screen(&initial_state),
        initial_state,
        super::background_download::background_download_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「メール・SMS の作成」（親 #1142、Issue #1387）をマウントする
/// 検証専用エントリポイント。`super::compose_mail_sms::compose_mail_sms_effect` が `mailto:` / `sms:` へ遷移する。
#[wasm_bindgen]
pub fn mount_compose_mail_sms_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ComposeMailSmsState::default();
    super::mount::mount_with_effect(
        root,
        current_compose_mail_sms_screen(&initial_state),
        initial_state,
        super::compose_mail_sms::compose_mail_sms_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「他アプリの起動」（親 #176、Issue #864）をマウントする
/// 検証専用エントリポイント。`super::external_app_launch::external_app_launch_show_effect` が `tel:` / `geo:` へ遷移する。
#[wasm_bindgen]
pub fn mount_external_app_launch_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ExternalAppLaunchState::default();
    super::mount::mount_with_effect(
        root,
        current_external_app_launch_show_screen(&initial_state),
        initial_state,
        super::external_app_launch::external_app_launch_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「画面のリフレッシュレート・HDR・広色域」（親 #177、Issue #873）をマウントする
/// 検証専用エントリポイント。`super::display_refresh_hdr::display_refresh_hdr_show_effect` が `matchMedia` を読む。
#[wasm_bindgen]
pub fn mount_display_refresh_hdr_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = DisplayRefreshHdrShowState::default();
    super::mount::mount_with_effect(
        root,
        current_display_refresh_hdr_show_screen(&initial_state),
        initial_state,
        super::display_refresh_hdr::display_refresh_hdr_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「端末内 LLM」（親 #93、Issue #347）をマウントする
/// 検証専用エントリポイント。`super::on_device_llm::on_device_llm_effect` が Prompt API を呼ぶ。
#[wasm_bindgen]
pub fn mount_on_device_llm_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = OnDeviceLlmState::default();
    super::mount::mount_with_effect(
        root,
        current_on_device_llm_screen(&initial_state),
        initial_state,
        super::on_device_llm::on_device_llm_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「端末内翻訳」（親 #115、Issue #539）をマウントする
/// 検証専用エントリポイント。`super::translation::translation_effect` が Translator API を呼ぶ。
#[wasm_bindgen]
pub fn mount_translation_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = TranslationState::default();
    super::mount::mount_with_effect(
        root,
        current_translation_screen(&initial_state),
        initial_state,
        super::translation::translation_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「パスキー・サインイン連携」（親 #92、Issue #344）をマウントする
/// 検証専用エントリポイント。`super::passkey::passkey_signin_effect` が WebAuthn を呼ぶ。
#[wasm_bindgen]
pub fn mount_passkey_signin_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PasskeySigninState::default();
    super::mount::mount_with_effect(
        root,
        current_passkey_signin_screen(&initial_state),
        initial_state,
        super::passkey::passkey_signin_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「物・サービスの決済」（親 #114、Issue #542）をマウントする
/// 検証専用エントリポイント。`super::payment_request::payment_request_effect` が Payment Request API を呼ぶ。
#[wasm_bindgen]
pub fn mount_payment_request_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PaymentRequestState::default();
    super::mount::mount_with_effect(
        root,
        current_payment_request_screen(&initial_state),
        initial_state,
        super::payment_request::payment_request_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「印刷」（親 #94、Issue #329）をマウントする
/// 検証専用エントリポイント。`super::print_page::print_page_effect` が `window.print()` を呼ぶ。
#[wasm_bindgen]
pub fn mount_print_page_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PrintPageState::default();
    super::mount::mount_with_effect(
        root,
        current_print_page_screen(&initial_state),
        initial_state,
        super::print_page::print_page_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「バッジの設定」（親 #1141、Issue #1363）をマウントする
/// 検証専用エントリポイント。`super::app_badge::app_badge_set_effect` が Badging API を呼ぶ。
#[wasm_bindgen]
pub fn mount_app_badge_set_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = AppBadgeSetState::default();
    super::mount::mount_with_effect(
        root,
        current_app_badge_set_screen(&initial_state),
        initial_state,
        super::app_badge::app_badge_set_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「触覚フィードバック」（親 #1090、Issue #1091）を
/// マウントする検証専用エントリポイント。`super::haptics::haptics_tap_effect` を effect
/// として渡すため、`navigator.vibrate` 経由の振動要求が動く。
///
/// 画面は `current_screen(&HapticsTapState::default())` で固定する（`mount` は
/// `Screen<S>` の再構築に対応しないため送信中もボタンが残るが、`tap` が `Idle` 以外は
/// no-op のため二重要求は実害にならない）。
#[wasm_bindgen]
pub fn mount_haptics_tap_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = HapticsTapState::default();
    super::mount::mount_with_effect(
        root,
        current_haptics_tap_screen(&initial_state),
        initial_state,
        super::haptics::haptics_tap_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「共有の受け取り」（親 #122、Issue #608）をマウントする
/// 検証専用エントリポイント。受信は effect が読み込み時に 1 回クエリから行う。Web には登録・解除の
/// 操作が無いためボタンは足さない。
#[wasm_bindgen]
pub fn mount_share_receive_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    super::mount::mount_with_effect(
        root,
        share_receive_show_screen(),
        ShareReceiveShowState::default(),
        super::share_receive::share_receive_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「カスタム URL scheme」（親 #1116、Issue #1219）を
/// マウントする検証専用エントリポイント。登録ボタンは Web だけに必要なためここで足す
/// （core の画面と登録 UI の状態は他 OS と共用。結果の文言は core の画面が出す）。クエリ `url` の受信は effect が読み込み時に 1 回行う。
#[wasm_bindgen]
pub fn mount_url_scheme_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let screen = url_scheme_show_screen().button(REGISTER_BUTTON_ID, "web+fandhe を登録", register);
    super::mount::mount_with_effect(
        root,
        screen,
        UrlSchemeShowState::default(),
        super::url_scheme::url_scheme_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「単語分割」（親 #1093、Issue #1094）をマウントする検証専用
/// エントリポイント。`super::nlp::nlp_segment_effect` を effect として渡すため、
/// `Intl.Segmenter` 経由の単語分割が動く。
///
/// 画面は `current_screen(&NlpSegmentState::default())` で固定する（`mount` は `Screen<S>` の
/// 再構築に対応しないためボタンは常時表示されるが、`segment` が `Idle` 以外は no-op のため
/// 二重要求は実害にならない。分割は同期で、押下の直後に `Idle` へ戻る）。
#[wasm_bindgen]
pub fn mount_nlp_segment_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = NlpSegmentState::default();
    super::mount::mount_with_effect(
        root,
        current_nlp_segment_screen(&initial_state),
        initial_state,
        super::nlp::nlp_segment_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ネットワーク接続状態」（Issue #1285）をマウントする
/// 検証専用エントリポイント。`super::network_status::network_status_show_effect` が
/// 1 秒ごとに `navigator.onLine` 等を読み直す。
///
/// 呼び出し元が渡す `root_id` の要素が見つからない場合はエラーを返す。
#[wasm_bindgen]
pub fn mount_network_status_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    super::mount::mount_with_effect(
        root,
        network_status_show_screen(),
        NetworkStatusShowState::default(),
        super::network_status::network_status_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「画面スリープの抑止」（親 #1129、Issue #1281）を
/// マウントする検証専用エントリポイント。`super::screen_wake_lock::wake_lock_toggle_effect` が
/// Screen Wake Lock API を駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、解除できるよう「抑止をやめる」を最初から足す
/// （`turn_on` / `turn_off` は `Idle` 以外 no-op のため二重要求は実害にならない）。
#[wasm_bindgen]
pub fn mount_wake_lock_toggle_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = WakeLockToggleState::default();
    let screen = current_wake_lock_toggle_screen(&initial_state).button(
        WAKE_LOCK_OFF_BUTTON_ID,
        WAKE_LOCK_OFF_LABEL,
        wake_lock_turn_off,
    );
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::screen_wake_lock::wake_lock_toggle_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ライト（トーチ）」（親 #106、Issue #462）を
/// マウントする検証専用エントリポイント。`super::flashlight::flashlight_toggle_effect` が
/// `getUserMedia` + `torch` 制約を駆動する。`Idle` の画面は点灯・消灯の両ボタンを持つため足さない。
#[wasm_bindgen]
pub fn mount_flashlight_toggle_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = FlashlightToggleState::default();
    let screen = current_flashlight_toggle_screen(&initial_state).text(FLASHLIGHT_WEB_NOTE);
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::flashlight::flashlight_toggle_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「全画面表示」（親 #1133、Issue #1315）を
/// マウントする検証専用エントリポイント。`super::fullscreen_immersive::fullscreen_toggle_effect` が
/// Fullscreen API を駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、戻せるよう「通常の表示に戻す」を最初から足す
/// （`enter` / `exit` は `Idle` 以外 no-op のため二重要求は実害にならない）。
#[wasm_bindgen]
pub fn mount_fullscreen_toggle_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = FullscreenToggleState::default();
    let screen = current_fullscreen_toggle_screen(&initial_state).button(
        FULLSCREEN_EXIT_BUTTON_ID,
        FULLSCREEN_EXIT_LABEL,
        fullscreen_exit,
    );
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::fullscreen_immersive::fullscreen_toggle_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「画面の向き」（親 #128、Issue #658）をマウントする
/// 検証専用エントリポイント。`super::screen_orientation::screen_orientation_show_effect` が
/// Screen Orientation API と Fullscreen API を駆動する。
///
/// `mount` は `Screen<S>` を再構築せず、初期状態（未読み取り）の画面にはボタンが無い。操作できるよう
/// ボタンを最初から足す（core の `request_lock` は固定不可・処理中のとき no-op、`reload` は冪等）。
#[wasm_bindgen]
pub fn mount_screen_orientation_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ScreenOrientationShowState::default();
    let screen = current_screen_orientation_screen(&initial_state)
        .button(
            LOCK_PORTRAIT_ID,
            LOCK_PORTRAIT_LABEL,
            orientation_lock_portrait,
        )
        .button(
            LOCK_LANDSCAPE_ID,
            LOCK_LANDSCAPE_LABEL,
            orientation_lock_landscape,
        )
        .button(UNLOCK_ID, UNLOCK_LABEL, orientation_unlock)
        .button(RELOAD_ID, RELOAD_LABEL, orientation_reload);
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::screen_orientation::screen_orientation_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ピクチャ・イン・ピクチャ」（親 #97、Issue #367）を
/// マウントする検証専用エントリポイント。`super::picture_in_picture::pip_show_effect` が
/// Picture-in-Picture API を駆動する（画面がボタンを常時出すため追加ボタンは不要）。
#[wasm_bindgen]
pub fn mount_pip_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PipShowState::default();
    super::mount::mount_with_effect(
        root,
        current_pip_show_screen(&initial_state),
        initial_state,
        super::picture_in_picture::pip_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「外部機器への再生」（親 #99、Issue #389）を
/// マウントする検証専用エントリポイント。`super::media_cast::cast_show_effect` が
/// Remote Playback API を駆動する（画面がボタンを常時出すため追加ボタンは不要）。
#[wasm_bindgen]
pub fn mount_cast_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = CastShowState::default();
    super::mount::mount_with_effect(
        root,
        current_cast_show_screen(&initial_state),
        initial_state,
        super::media_cast::cast_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「フォルダの変更監視」（親 #1143、Issue #1414）を
/// マウントする検証専用エントリポイント。`super::file_watch::file_watch_show_effect` が
/// `FileSystemObserver` + OPFS を駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、先頭のトグルボタンのラベルは「監視を開始」のまま固定される
/// （監視中に押すと `toggle` で停止になる）。止めたことが分かるよう「監視を止める」を最初から足す。
#[wasm_bindgen]
pub fn mount_file_watch_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = FileWatchShowState::default();
    let screen = current_file_watch_screen(&initial_state).button(
        WidgetId::from_raw(5),
        FILE_WATCH_STOP_LABEL,
        file_watch_stop,
    );
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::file_watch::file_watch_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「音声の再生」（親 #86、Issue #271）をマウントする
/// 検証専用エントリポイント。`super::media_playback::audio_play_effect` が `<audio>` を駆動する。
/// core 画面は 再生 / 一時停止 / 停止 の 3 ボタンを常に出すため、追加のボタンは不要。
#[wasm_bindgen]
pub fn mount_audio_play_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = AudioPlayState::default();
    let screen = current_audio_play_screen(&initial_state);
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::media_playback::audio_play_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「動画・音声の変換」（親 #91、Issue #315）をマウントする
/// 検証専用エントリポイント。`super::media_transcode::media_transcode_effect` が WebCodecs を駆動する。
#[wasm_bindgen]
pub fn mount_media_transcode_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;
    let initial_state = TranscodeState::default();
    let screen = current_media_transcode_screen(&initial_state);
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::media_transcode::media_transcode_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「MIDI 入力」（親 #1137、Issue #1401）をマウントする
/// 検証専用エントリポイント。`super::midi::midi_show_effect` が Web MIDI を駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、初期状態（`Idle`）の「受信開始」だけが固定される。
/// 受信中・開始中でも止められるよう、「停止」「消去」を最初から足す（ID は画面内部の値と同じ）。
#[wasm_bindgen]
pub fn mount_midi_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = MidiShowState::default();
    let screen = current_midi_screen(&initial_state)
        .button(WidgetId::from_raw(2), MIDI_STOP_LABEL, midi_stop)
        .button(WidgetId::from_raw(3), MIDI_CLEAR_LABEL, midi_clear);
    super::mount::mount_with_effect(root, screen, initial_state, super::midi::midi_show_effect)
}

/// `root_id` が指す要素へ型サンプル「AR 表示」（親 #90、Issue #309）をマウントする検証専用
/// エントリポイント。`super::ar::ar_show_effect` が WebXR セッションを駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、止められるよう「停止」を最初から足す
/// （`stop` は冪等のため二重押下は実害にならない）。
#[wasm_bindgen]
pub fn mount_ar_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = ArShowState::default();
    let screen =
        current_ar_screen(&initial_state).button(AR_STOP_BUTTON_ID, AR_STOP_LABEL, ar_stop);
    super::mount::mount_with_effect(root, screen, initial_state, super::ar::ar_show_effect)
}

/// `root_id` が指す要素へ型サンプル「方位（コンパス）」（親 #1154、Issue #1407）を
/// マウントする検証専用エントリポイント。`super::compass::compass_show_effect` が
/// 方位イベントを駆動する。
///
/// `mount` は `Screen<S>` を再構築しないため、止められるよう「停止」を最初から足す
/// （`start` は購読中 no-op、`stop` は冪等のため二重押下は実害にならない）。
#[wasm_bindgen]
pub fn mount_compass_show_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = CompassShowState::default();
    let screen = current_compass_screen(&initial_state).button(
        COMPASS_STOP_BUTTON_ID,
        COMPASS_STOP_LABEL,
        compass_stop,
    );
    super::mount::mount_with_effect(
        root,
        screen,
        initial_state,
        super::compass::compass_show_effect,
    )
}

/// `root_id` が指す要素へ型サンプル「ペン・筆圧で描く」（親 #95、Issue #358）を
/// マウントする検証専用エントリポイント。`super::pen_stylus::pen_draw_effect` が
/// Pointer Events を駆動する。
#[wasm_bindgen]
pub fn mount_pen_draw_screen(root_id: &str) -> Result<(), JsValue> {
    let root = root_element(root_id)?;

    let initial_state = PenDrawState::default();
    super::mount::mount_with_effect(
        root,
        current_pen_draw_screen(&initial_state),
        initial_state,
        super::pen_stylus::pen_draw_effect,
    )
}
