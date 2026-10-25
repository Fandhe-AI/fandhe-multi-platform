//! 登録した画面を切り替えて起動するための画面モジュールの置き場。
//!
//! `ui::mod.rs` は 600 行超で大半がドキュメンテーションコメントのため、生成 AI に
//! 全文再出力させると doc 消失・再エクスポート欠落のリスクが高い（Issue #653）。
//! そのため AI が生成した画面はここへ 1 画面 1 ファイルとして置き、本ファイルには
//! `pub mod <snake_case>;` を 1 行ずつ追加するだけにとどめる（`ui::mod.rs` には
//! 手を入れない）。画面ファイルの内部では `ui::{Screen, Node}` と `state` の
//! 公開 API のみを使う（`docs/ai/SCHEMA.md` の型制約に従う）。

// SCHEMA.md の画面本体例が実際に `screens/` 配下でコンパイルできることを固定化する
// 参照実装モジュール（PR #668 レビュー指摘）。`examples/`・`tests/` はこのクレートとは
// 別クレートとしてコンパイルされ公開 API しか参照できないため、`#[cfg(test)]` 限定
// （＝通常ビルドの公開 API に含まれない）では固定化に使えず `pub mod` にしている。
// SCHEMA.md の例は画面名 `todo` を使うため、この参照実装モジュールを `mod todo;` にすると
// AI が同じ名前で実際の画面を生成した際に `pub mod todo;` と二重定義してコンパイル
// 不能になる。生成対象と衝突しない名前にする（`schema_example_fixture.rs` 参照）。
/// Phase 11 の拡充ループ型サンプル「行動認識（歩行・乗車等）」（親 #124、Issue #613）。
pub mod activity_recognition_show;
/// Phase 11 の拡充ループ型サンプル「アラーム・正確な時刻の予約」（親 #140、Issue #721）。
pub mod alarm_show;
/// Phase 11 の拡充ループ型サンプル「端末・アプリの真正性証明」（親 #132、Issue #677）。
pub mod app_attest_show;
/// Phase 11（端末機能の型サンプル「バッジの設定」、Issue #1360）。
pub mod app_badge_set;
/// Phase 11 の拡充ループ型サンプル「AR 表示」（親 #90、Issue #307）。
pub mod ar_show;
/// Phase 8（端末機能の型サンプル「カメラ撮影」、Issue #749）。
pub mod camera_capture;
/// Phase 11 の拡充ループ型サンプル「カメラのライブ映像」（親 #168、Issue #812）。
pub mod camera_preview_stream_show;
pub mod confirm_dialog;
pub mod list_detail;
pub mod schema_example_fixture;

/// M7 型サンプル「フォーム」（Issue #728）。
pub mod form;
pub mod settings;

/// Phase 8 型サンプル「メモ保存」（Issue #747）。
pub mod storage_note;

/// Phase 8 型サンプル「HTTP 取得」（Issue #748）。
pub mod http_fetch;

/// Phase 10 型サンプル「傾きで玉が転がる」（Issue #830）。
pub mod tilt_ball;

/// Phase 11 型サンプル「クリップボード」（Issue #859）。
pub mod clipboard_text;

/// Phase 11 型サンプル「読み上げ」（Issue #865）。
pub mod tts_speak;

/// Phase 11 型サンプル「ファイル選択」（Issue #872）。
pub mod file_pick;

/// Phase 11 の拡充ループ型サンプル「位置情報」（Issue #888）。
pub mod location_show;

/// Phase 11 の拡充ループ型サンプル「住所の座標変換」（親 #133、Issue #691）。
pub mod geocoding_search_show;

/// Phase 11 の拡充ループ型サンプル「ジオフェンス」（親 #1056、Issue #1057）。
pub mod geofence_show;

/// Phase 11 の拡充ループ型サンプル「共有」（Issue #897）。
pub mod share_text;

/// Phase 11 の拡充ループ型サンプル「生体認証」（Issue #906）。
pub mod biometric_check;

/// Phase 11 の拡充ループ型サンプル「文字サイズ追従」（Issue #912）。
pub mod text_scale_preview;

/// Phase 11 の拡充ループ型サンプル「資格情報」（Issue #922）。
pub mod credential_vault;

/// Phase 11 の拡充ループ型サンプル「ゲームコントローラ」（親 #932、Issue #933）。
pub mod gamepad_show;

/// Phase 11 の拡充ループ型サンプル「マイク録音」（親 #953、Issue #954）。
pub mod mic_record;

/// Phase 11 の拡充ループ型サンプル「TSV 読み込み」（親 #979、Issue #980）。
pub mod tsv_import;

/// Phase 11 の拡充ループ型サンプル「通知」（親 #1003、Issue #1004）。
pub mod notification_send;

/// Phase 11 の拡充ループ型サンプル「ドラッグ & ドロップ」（親 #1013、Issue #1014）。
pub mod drag_drop;

/// Phase 11 の拡充ループ型サンプル「音声認識」（親 #1023、Issue #1024）。
pub mod speech_recognize;

/// Phase 11 の拡充ループ型サンプル「環境音の分類」（親 #142、Issue #725）。
pub mod sound_classification_show;

/// Phase 11 の拡充ループ型サンプル「チェックボックス一覧」（親 #1036、Issue #1037）。
pub mod checkbox_list;

/// Phase 11 の拡充ループ型サンプル「バックグラウンド処理」（親 #1042、Issue #1043）。
pub mod background_task;

/// Phase 11 の拡充ループ型サンプル「バックグラウンドダウンロード」（親 #100、Issue #390）。
pub mod background_download;
/// Phase 11 の拡充ループ型サンプル「ファイルの関連付け」（親 #102、Issue #416）。
pub mod file_association_show;
/// Phase 11 の拡充ループ型サンプル「クイック設定タイル・コントロール」（親 #139、Issue #719）。
pub mod quick_settings_tile_show;
/// Phase 11 の拡充ループ型サンプル「共有の受け取り」（親 #122、Issue #606）。
pub mod share_receive_show;
/// Phase 11 の拡充ループ型サンプル「OS の検索への登録」（親 #117、Issue #560）。
pub mod spotlight_index;
/// Phase 11 の拡充ループ型サンプル「システムトレイ」（親 #1038、Issue #1039）。
pub mod system_tray;
/// Phase 11 の拡充ループ型サンプル「カスタム URL scheme」（親 #1116、Issue #1215）。
pub mod url_scheme_show;
/// Phase 11 の拡充ループ型サンプル「USB 機器」（親 #112、Issue #505）。
pub mod usb_device_show;
/// Phase 11 の拡充ループ型サンプル「無操作時間の検知」（親 #125、Issue #631）。
pub mod user_idle_show;
/// Phase 11 の拡充ループ型サンプル「UWB による距離・方向」（親 #129、Issue #653）。
pub mod uwb_ranging_show;
/// Phase 11 の拡充ループ型サンプル「動画の撮影」（親 #169、Issue #821）。
pub mod video_capture_show;
/// Phase 11 の拡充ループ型サンプル「システム音量」（親 #166、Issue #785）。
pub mod volume_control_show;

/// Phase 11 の拡充ループ型サンプル「アクセシビリティ設定の追従」（親 #1132、Issue #1308）。
pub mod accessibility_prefs_show;
/// Phase 11 の拡充ループ型サンプル「音声デバイス」（親 #1136、Issue #1330）。
pub mod audio_devices_show;
/// Phase 11 の拡充ループ型サンプル「音声の再生」（親 #86、Issue #267）。
pub mod audio_play;
/// Phase 11 の拡充ループ型サンプル「バーコード・QR 読み取り」（親 #116、Issue #557）。
pub mod barcode_scan;
/// Phase 11 の拡充ループ型サンプル「BLE の周辺機器役」（親 #164、Issue #778）。
pub mod ble_peripheral;
/// Phase 11 の拡充ループ型サンプル「BLE スキャン」（親 #1059、Issue #1060）。
pub mod ble_scan;
/// Phase 11 の拡充ループ型サンプル「Bluetooth Classic」（親 #130、Issue #659）。
pub mod bluetooth_classic_show;
/// Phase 11 の拡充ループ型サンプル「予定の追加」（親 #1118、Issue #1224）。
pub mod calendar_add;
/// Phase 11 の拡充ループ型サンプル「通話 UI 統合」（親 #87、Issue #276）。
pub mod call_ui_show;
/// Phase 11 の拡充ループ型サンプル「画面キャプチャの防止・検知」（親 #120、Issue #582）。
pub mod capture_protection_show;
/// Phase 11 の拡充ループ型サンプル「外部機器への再生」（親 #99、Issue #386）。
pub mod cast_show;
/// Phase 11 の拡充ループ型サンプル「付属機器のペアリング」（親 #143、Issue #740）。
pub mod companion_device_show;
/// Phase 11 の拡充ループ型サンプル「方位（コンパス）」（親 #1154、Issue #1404）。
pub mod compass_show;
/// Phase 11 の拡充ループ型サンプル「メール・SMS の作成」（親 #1142、Issue #1384）。
pub mod compose_mail_sms;
/// Phase 11 の拡充ループ型サンプル「端末・OS 情報」（親 #1131、Issue #1304）。
pub mod device_info_show;
/// Phase 11 の拡充ループ型サンプル「書類スキャン」（親 #145、Issue #747）。
pub mod document_scan_show;
/// Phase 11 の拡充ループ型サンプル「気圧・照度・近接」（親 #123、Issue #609）。
pub mod environment_sensors_show;
/// Phase 11 の拡充ループ型サンプル「フォルダの変更監視」（親 #1143、Issue #1410）。
pub mod file_watch_show;
/// Phase 11 の拡充ループ型サンプル「ライト（トーチ）」（親 #106、Issue #458）。
pub mod flashlight_toggle;
/// Phase 11 の拡充ループ型サンプル「集中モードの状態」（親 #104、Issue #438）。
pub mod focus_mode_status_show;
/// Phase 11 の拡充ループ型サンプル「折りたたみの姿勢」（親 #141、Issue #723）。
pub mod foldable_posture_show;
/// Phase 11 の拡充ループ型サンプル「全画面表示」（親 #1133、Issue #1312）。
pub mod fullscreen_toggle;
/// Phase 11 の拡充ループ型サンプル「グローバルホットキー」（親 #150、Issue #766）。
pub mod global_hotkey;
/// Phase 11 の拡充ループ型サンプル「触覚フィードバック」（親 #1090、Issue #1091）。
pub mod haptics_tap;
/// Phase 11 の拡充ループ型サンプル「ホーム画面ウィジェット」（親 #1064、Issue #1065）。
pub mod home_widget_show;
/// Phase 11 の拡充ループ型サンプル「画像の文字認識」（親 #1120、Issue #1250）。
pub mod image_recognize;
/// Phase 11 の拡充ループ型サンプル「アプリ内ブラウザ」（親 #135、Issue #701）。
pub mod in_app_browser_show;
/// Phase 11 の拡充ループ型サンプル「アプリ内レビュー依頼」（Issue #563）。
pub mod in_app_review;
/// Phase 11 の拡充ループ型サンプル「LAN のサービス探索」（親 #109、Issue #486）。
pub mod lan_discover;
/// Phase 11 の拡充ループ型サンプル「ログイン時の自動起動」（親 #134、Issue #696）。
pub mod launch_at_login_show;
/// Phase 11 の拡充ループ型サンプル「ライブアクティビティ」（親 #88、Issue #294）。
pub mod live_activity_show;
/// Phase 11 の拡充ループ型サンプル「OS 搭載フォント」（親 #1145、Issue #1376）。
pub mod local_fonts_show;
/// Phase 11 の拡充ループ型サンプル「地図表示」（親 #85、Issue #273）。
pub mod map_view_show;
/// Phase 11 の拡充ループ型サンプル「動画・音声の変換」（親 #91、Issue #311）。
pub mod media_transcode;
/// Phase 11 の拡充ループ型サンプル「MIDI 入力」（親 #1137、Issue #1398）。
pub mod midi_show;
/// Phase 11 の拡充ループ型サンプル「機械学習モデル推論」（親 #89、Issue #289）。
pub mod ml_infer;
/// Phase 11 の拡充ループ型サンプル「複数ディスプレイ」（親 #1134、Issue #1341）。
pub mod multi_display_show;
/// Phase 11 の拡充ループ型サンプル「音楽ライブラリ」（親 #108、Issue #482）。
pub mod music_library_list;
/// Phase 11 の拡充ループ型サンプル「ネットワーク接続状態」（親 #1130、Issue #1282）。
pub mod network_status_show;
/// Phase 11 の拡充ループ型サンプル「単語分割」（親 #1093、Issue #1094）。
pub mod nlp_segment;
/// Phase 11 の拡充ループ型サンプル「端末内 LLM」（親 #93、Issue #345）。
pub mod on_device_llm;
/// Phase 11 の拡充ループ型サンプル「既定アプリで開く・ファイルマネージャーで表示」（親 #146、Issue #749）。
pub mod open_in_shell;
/// Phase 11 の拡充ループ型サンプル「OS の設定画面を開く」（親 #171、Issue #798）。
pub mod open_system_settings_show;
/// Phase 11 の拡充ループ型サンプル「ワンタイムコードの入力」（親 #121、Issue #585）。
pub mod otp_input;
/// Phase 11 の拡充ループ型サンプル「パスキー・サインイン連携」（親 #92、Issue #342）。
pub mod passkey_signin;
/// Phase 11 の拡充ループ型サンプル「物・サービスの決済」（親 #114、Issue #541）。
pub mod payment_request;
/// Phase 11 の拡充ループ型サンプル「PDF 表示」（親 #107、Issue #469）。
pub mod pdf_first_page;
/// Phase 11 の拡充ループ型サンプル「歩数」（親 #1119、Issue #1247）。
pub mod pedometer_read;
/// Phase 11 の拡充ループ型サンプル「近くの端末との直接通信」（親 #110、Issue #511）。
pub mod peer_connect;
/// Phase 11 の拡充ループ型サンプル「写真選択」（親 #1101、Issue #1103）。
pub mod photo_pick;
/// Phase 11 の拡充ループ型サンプル「写真の保存」（親 #1121、Issue #1254）。
pub mod photo_save;
/// Phase 11 の拡充ループ型サンプル「ピクチャ・イン・ピクチャ」（親 #97、Issue #364）。
pub mod pip_show;
/// Phase 11 の拡充ループ型サンプル「電源状態」（親 #1128、Issue #1274）。
pub mod power_state_show;
/// Phase 11 の拡充ループ型サンプル「印刷」（親 #94、Issue #325）。
pub mod print_page;
/// Phase 11 の拡充ループ型サンプル「セーフエリア」（親 #105、Issue #444）。
pub mod safe_area_insets_show;
/// Phase 11 の拡充ループ型サンプル「スペルチェック」（親 #119、Issue #578）。
pub mod spell_check;
/// Phase 11 の拡充ループ型サンプル「システムのアクセント色」（親 #103、Issue #421）。
pub mod system_accent_color_show;
/// Phase 11 の拡充ループ型サンプル「端末内翻訳」（親 #115、Issue #535）。
pub mod translation;
/// Phase 11 の拡充ループ型サンプル「画面スリープの抑止」（親 #1129、Issue #1278）。
pub mod wake_lock_toggle;
/// Phase 11 の拡充ループ型サンプル「指定した Wi-Fi への接続」（親 #165、Issue #791）。
pub mod wifi_connect_show;
/// Phase 11 の拡充ループ型サンプル「Wi-Fi 情報」（親 #111、Issue #514）。
pub mod wifi_info_show;
/// Phase 11 の拡充ループ型サンプル「ウィンドウ管理」（親 #127、Issue #636）。
pub mod window_management_show;

/// Phase 11 の拡充ループ型サンプル「連絡先選択」（親 #1117、Issue #1220）。
pub mod contact_pick;

/// Phase 11 の拡充ループ型サンプル「画面のスポイト」（親 #148、Issue #762）。
pub mod eye_dropper_pick;

/// Phase 11 の拡充ループ型サンプル「画面の明るさ」（親 #126、Issue #633）。
pub mod screen_brightness_show;
/// Phase 11 の拡充ループ型サンプル「画面の録画・キャプチャ」（親 #1135、Issue #1335）。
pub mod screen_capture;
/// Phase 11 の拡充ループ型サンプル「画面の向き」（親 #128、Issue #655）。
pub mod screen_orientation_show;
/// Phase 11 の拡充ループ型サンプル「シリアルポート」（親 #131、Issue #670）。
pub mod serial_port_show;
/// Phase 11 の拡充ループ型サンプル「スリープ・ロックの通知」（親 #167、Issue #810）。
pub mod sleep_lock_events_show;

/// Phase 11 の拡充ループ型サンプル「ペン・筆圧で描く」（親 #95、Issue #355）。
pub mod pen_draw;

/// Phase 11 の拡充ループ型サンプル「OAuth 用の Web 認証セッション」（親 #113、Issue #530）。
pub mod web_auth_session_demo;
