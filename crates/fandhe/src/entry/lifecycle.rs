//! 選んだ端末機能の起動前の登録・終了処理・Activity 再生成時の破棄（Issue #1200）。
//!
//! 利用者が機能ごとの手順を知らずに済むよう、入口がここで有効な feature の分だけ呼ぶ（機能の追加・削除は
//! feature の追加・削除だけで済ませる。`docs/architecture/consumer-app-structure.md` §4）。
//! 呼ぶ関数の網羅は `tests/features.rs` が機能 crate の crate ルートと突き合わせて検査する。
//! 機能 crate の停止は相互に独立。依存のある media-playback → audio-session の前後だけは保つ。

/// `UIApplicationMain`（iOS）・`run_native`（デスクトップ）より前に呼ぶ登録。
///
/// 起動と同じターンで届く通知（URL・ファイルを開く・背景転送の完了・検索から開く）を取りこぼさないよう、
/// イベントループの開始前に main thread で登録する。Windows のタスク スケジューラによる再起動と、
/// URL 起動を起動済みのインスタンスへ転送した場合は、画面を出さずにここでプロセスを終える。
pub(crate) fn before_launch() {
    // Issue #1044: タスク スケジューラが専用引数付きで本 exe を再起動したときは、画面を出さずに
    // 実行結果を書いて終了する（`fandhe-cap-background-task` の `windows.rs` モジュール doc 参照）。
    #[cfg(all(target_os = "windows", feature = "background-task"))]
    {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if fandhe_cap_background_task::parse_scheduled_args(&args) {
            std::process::exit(fandhe_cap_background_task::run_scheduled());
        }
    }
    // Issue #1333: ファイル起動の経路は、単一インスタンスの調停を持つ url-scheme の `startup` より前に登録する
    // （登録が無いとファイル起動は転送されない）。
    #[cfg(all(target_os = "windows", feature = "file-association"))]
    fandhe_cap_file_association::windows::register_route();
    // Issue #1216: URL 起動で既存インスタンスがあれば pipe で転送して終える。受ける側なら URL を受け口へ入れて続ける。
    #[cfg(all(target_os = "windows", feature = "url-scheme"))]
    {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if let Some(code) = fandhe_cap_url_scheme::windows::win::startup(&args) {
            std::process::exit(code);
        }
    }
    // Issue #1045: `BGTaskScheduler` の launch handler は `UIApplicationMain` より前に登録する契約。
    #[cfg(all(target_os = "ios", feature = "background-task"))]
    fandhe_cap_background_task::register();
    // Issue #1215・#1217: URL scheme の受信（macOS は Apple Event、iOS は scene の通知）。
    #[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "url-scheme"))]
    fandhe_cap_url_scheme::register();
    // Issue #1325: 背景転送の完了通知（`handleEventsForBackgroundURLSession`）は起動と同じターンで届く。
    #[cfg(all(target_os = "ios", feature = "background-upload"))]
    fandhe_cap_background_upload::register();
    #[cfg(all(target_os = "ios", feature = "background-download"))]
    fandhe_cap_background_download::register();
    // Issue #606: 共有の受け取り（サービスメニュー）の provider。
    #[cfg(all(target_os = "macos", feature = "share-receive"))]
    fandhe_cap_share_receive::register();
    // Issue #560・#561: OS の検索から開かれたときの受信。接頭辞は画面の effect が照合する
    // `fandhe_core::screens::spotlight_index::URL_PREFIX` と同じ値を渡す。
    #[cfg(all(
        any(target_os = "ios", target_os = "macos"),
        feature = "spotlight-index"
    ))]
    fandhe_cap_spotlight_index::register(fandhe_core::screens::spotlight_index::URL_PREFIX);
    // Issue #416・#417: ファイルを開く（macOS は odoc の Apple Event、iOS は scene の通知）。
    #[cfg(all(
        any(target_os = "ios", target_os = "macos"),
        feature = "file-association"
    ))]
    fandhe_cap_file_association::register();
}

/// 終了処理を描画層（`fandhe-render-egui`）へ登録する。描画層は端末機能を知らないため、終了（ウィンドウを閉じる経路）と
/// `ProxyApp` の破棄で呼ぶ関数をここで渡す。2 回目以降の登録は無視される（Activity 再生成で再実行されてもよい）。
pub(crate) fn register_teardown_hooks() {
    fandhe_render_egui::app::set_teardown_hooks(shutdown, on_drop);
}

/// 終了時の停止。有効な機能 crate を順に止める。
fn shutdown() {
    // 読み上げの子プロセス（`say`・PowerShell）を止める。プロセスが終了すると `static` の `Drop` は走らず、ウィンドウを閉じても
    // 発話中の子プロセスが残るため、終了フックから明示的に止める（キューがあれば発話要求と同じキューへ積み、発送済みの発話より後に止まる。
    // 公開入口から直接発話してキューが無いときは、その場で止める）。
    #[cfg(feature = "tts")]
    fandhe_cap_tts::shutdown();
    // 進行中の Web 認証（デモサーバーと OS のセッション）を閉じる。
    #[cfg(feature = "web-auth-session")]
    fandhe_cap_web_auth_session::shutdown();
    // 再生（media-playback）→ OS のメディア操作（now-playing-controls）の順に止め、どちらも audio_session より前に置く。
    #[cfg(feature = "media-playback")]
    fandhe_cap_media_playback::shutdown();
    #[cfg(feature = "now-playing-controls")]
    fandhe_cap_now_playing_controls::shutdown();
    #[cfg(feature = "bluetooth-le")]
    fandhe_cap_bluetooth_le::stop_shared_scan();
    // 公開の停止は `.m` の専用キューへの短い同期呼び出し。Rust 側のロックは握らない。
    #[cfg(feature = "ble-peripheral")]
    fandhe_cap_ble_peripheral::shutdown();
    // macOS の停止は main queue へ同期ディスパッチする。`shutdown` は eframe の `on_exit`（main thread）からロックを握らずに呼ばれるため、
    // main 上なら直接停止し、デッドロックしない。
    #[cfg(feature = "bluetooth-classic")]
    fandhe_cap_bluetooth_classic::stop_shared_discovery();
    #[cfg(feature = "system-tray")]
    fandhe_cap_system_tray::stop_shared_system_tray();
    #[cfg(feature = "global-hotkey")]
    fandhe_cap_global_hotkey::shutdown();
    #[cfg(feature = "midi")]
    fandhe_cap_midi::shutdown();
    // 位置情報の測位子プロセス（Windows）を止める。
    #[cfg(feature = "location")]
    fandhe_cap_location::stop_shared_location();
    // LAN のサービス探索の停止（Rust 側のロックを握らずに OS 側を止める）。
    #[cfg(feature = "lan-discovery")]
    fandhe_cap_lan_discovery::stop_shared_discovery();
    #[cfg(feature = "media-cast")]
    fandhe_cap_media_cast::shutdown();
    #[cfg(feature = "serial-port")]
    fandhe_cap_serial_port::shutdown();
    #[cfg(feature = "audio-session")]
    fandhe_cap_audio_session::shutdown();
    #[cfg(feature = "memory-pressure")]
    fandhe_cap_memory_pressure::shutdown();
    #[cfg(feature = "camera-preview-stream")]
    fandhe_cap_camera_preview_stream::shutdown();
    // 近くの端末との直接通信の進行中の交換を止める。OS 側の停止は専用キューへの短い同期呼び出しで、Rust 側のロックは握らない。
    #[cfg(feature = "local-peer-connect")]
    fandhe_cap_local_peer_connect::shutdown();
    // 小窓の資源（macOS はタイマー・controller・layer、Android は小窓の解除）を解放する。
    #[cfg(feature = "picture-in-picture")]
    fandhe_cap_picture_in_picture::shutdown();
}

/// `ProxyApp` の破棄で呼ぶ解放。
fn on_drop() {
    #[cfg(feature = "memory-pressure")]
    fandhe_cap_memory_pressure::shutdown();
}

/// Activity 再生成の前に、保留中の要求と処理済み世代を破棄する。
///
/// 再生成では画面状態が作り直される（世代 0・`Idle`）。プロセスの static に残った旧要求が残ると、
/// 次の要求が飲み込まれたり、遅延した完了が新しい画面へ結びついたりするため、新しい画面状態を作る前に捨てる。
#[cfg(target_os = "android")]
pub(crate) fn reset_android() {
    // Activity 再生成では画面状態が作り直される（世代 0）。保留中の書類スキャン要求が残ると
    // 次の要求が飲み込まれるため、新しい画面状態を作る前に破棄する（TASK-21.3 / Issue #1171）。
    #[cfg(feature = "document-scan")]
    fandhe_cap_document_scan::reset_pending_tracking();
    // 写真選択の保留中の要求も破棄する（契約は「復元せず破棄」。旧要求の遅延完了を
    // id 不一致で無視させる。`fandhe_cap_photo_picker::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "photo-picker")]
    fandhe_cap_photo_picker::reset_pending_tracking();
    // 写真ライブラリ保存の保留中の要求も破棄する（保留中の保存要求は復元せず破棄し、旧要求の遅延完了を id 不一致で無視させる。`fandhe_cap_photo_library::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "photo-library")]
    fandhe_cap_photo_library::reset_pending_tracking();
    // 位置情報の再試行の処理済み世代を基準値へ戻す（画面状態は再生成で retry_seq 0 へ戻る。`fandhe_cap_location::reset_retry_tracking` doc 参照）。
    #[cfg(feature = "location")]
    fandhe_cap_location::reset_retry_tracking();
    // ジオフェンス画面の状態も再生成で未登録へ戻るため、static 側の登録もまとめて破棄する。
    #[cfg(feature = "geofence")]
    fandhe_cap_geofence::reset_register_tracking();
    // LAN のサービス探索の保留中の要求も破棄する（プロセス static の旧要求を再生成後へ持ち越さない。`fandhe_cap_lan_discovery::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "lan-discovery")]
    fandhe_cap_lan_discovery::reset_pending_tracking();
    // 機械学習推論の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の
    // 「推論する」が飲み込まれる（`fandhe_cap_ml_inference::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "ml-inference")]
    fandhe_cap_ml_inference::reset_pending_tracking();
    // 端末内翻訳の処理済み世代も破棄する（画面状態は世代 0 に戻るため。`fandhe_cap_translation::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "translation")]
    fandhe_cap_translation::reset_pending_tracking();
    // 生体認証の保留中の要求も破棄する。画面状態は再生成で `Idle` に戻るため、残すと結果が画面へ反映されず失われる
    // （`fandhe_cap_biometric_auth::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "biometric-auth")]
    fandhe_cap_biometric_auth::reset_pending_tracking();
    // 連絡先選択の保留中の要求も破棄する（契約は「復元せず破棄」。旧要求の遅延完了を id 不一致で無視させる。
    // `fandhe_cap_contacts::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "contacts")]
    fandhe_cap_contacts::reset_pending_tracking();
    // 付属機器のペアリングの保留中の要求も破棄する（理由は ml_inference と同じ。
    // `fandhe_cap_companion_device::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "companion-device")]
    fandhe_cap_companion_device::reset_pending_tracking();
    // 通話 UI の記録（処理済み世代）と進行中の通話も破棄する。画面状態は再生成で generation 0 に戻るため、
    // 残すと次の「着信を模擬する」が飲み込まれ、OS 側の着信も残る（`fandhe_cap_call_ui::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "call-ui")]
    fandhe_cap_call_ui::reset_pending_tracking();
    // ライブアクティビティの保留中の要求も破棄し、表示中の通知を閉じる。再生成後の画面は Idle から始まるため、
    // 残すと次の操作と OS 側の表示が食い違う（`fandhe_cap_live_activity::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "live-activity")]
    fandhe_cap_live_activity::reset_pending_tracking();
    // 予定追加の保留中の要求も破棄する（`SLOT` はプロセス static のため再生成をまたいで残る。
    // `fandhe_cap_calendar::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "calendar")]
    fandhe_cap_calendar::reset_pending_tracking();
    // 第 3 束の機能: 端末内 LLM・音声変換・アイコンバッジ・MIDI・パスキー・画面録画の保留中の要求と処理済み世代も
    // 破棄する（理由は ml_inference と同じ。各 crate の `reset_pending_tracking` doc 参照）。
    #[cfg(feature = "on-device-llm")]
    fandhe_cap_on_device_llm::reset_pending_tracking();
    #[cfg(feature = "media-transcode")]
    fandhe_cap_media_transcode::reset_pending_tracking();
    #[cfg(feature = "app-badge")]
    fandhe_cap_app_badge::reset_pending_tracking();
    #[cfg(feature = "midi")]
    fandhe_cap_midi::reset_pending_tracking();
    // 印刷の保留中の要求と画面の取り込み待ちも破棄する（Activity 再生成で印刷の保留は復元しない。
    // `fandhe_cap_print::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "print")]
    fandhe_cap_print::reset_pending_tracking();
    #[cfg(feature = "passkey-signin")]
    fandhe_cap_passkey_signin::reset_pending_tracking();
    #[cfg(feature = "screen-capture")]
    fandhe_cap_screen_capture::reset_pending_tracking();
    // PDF 表示の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の
    // 「表示する」が飲み込まれる（`fandhe_cap_pdf_viewer::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "pdf-viewer")]
    fandhe_cap_pdf_viewer::reset_pending_tracking();
    // 端末・アプリの真正性証明の処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の取得が飲み込まれる）（`fandhe_cap_app_attest::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "app-attest")]
    fandhe_cap_app_attest::reset_pending_tracking();
    // スペルチェックの保留中の要求も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の検査が飲み込まれる）（`fandhe_cap_spell_check::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "spell-check")]
    fandhe_cap_spell_check::reset_pending_tracking();
    // 住所の座標変換の保留中の要求も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の変換が飲み込まれる）（`fandhe_cap_geocoding_search::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "geocoding-search")]
    fandhe_cap_geocoding_search::reset_pending_tracking();
    // 音声セッションの保持中の購読と処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の開始が飲み込まれる（`fandhe_cap_audio_session::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "audio-session")]
    fandhe_cap_audio_session::reset_pending_tracking();
    // 他アプリ起動の保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_external_app_launch::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "external-app-launch")]
    fandhe_cap_external_app_launch::reset_pending_tracking();
    // 設定画面を開く保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_open_system_settings::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "open-system-settings")]
    fandhe_cap_open_system_settings::reset_pending_tracking();
    // 壁紙設定の保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_wallpaper_set::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "wallpaper-set")]
    fandhe_cap_wallpaper_set::reset_pending_tracking();
    // ファイルプレビューの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_file_preview::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "file-preview")]
    fandhe_cap_file_preview::reset_pending_tracking();
    // 動画の撮影の保留中の要求も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の要求が飲み込まれ、
    // 旧要求の遅延完了は id 不一致で捨てさせる（`fandhe_cap_video_capture::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "video-capture")]
    fandhe_cap_video_capture::reset_pending_tracking();
    // 無線の切り替えの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_radio_control::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "radio-control")]
    fandhe_cap_radio_control::reset_pending_tracking();
    // 動画変換の処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の変換が飲み込まれる）（`fandhe_cap_video_transcode::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "video-transcode")]
    fandhe_cap_video_transcode::reset_pending_tracking();
    // NFC タグ読み取りの保留中の要求と処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の読み取りが飲み込まれる）（`fandhe_cap_nfc::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "nfc")]
    fandhe_cap_nfc::reset_pending_tracking();
    // 機器からの取り込みの保留中の要求と処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の取り込みが飲み込まれる）（`fandhe_cap_scanner_devices::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "scanner-devices")]
    fandhe_cap_scanner_devices::reset_pending_tracking();
    // メール・SMS 作成の保留中の要求も破棄する（`bridge::SLOT` はプロセス static のため再生成をまたいで残る。`fandhe_cap_compose_mail_sms::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "compose-mail-sms")]
    fandhe_cap_compose_mail_sms::reset_pending_tracking();
    // クリップボードの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_clipboard::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "clipboard")]
    fandhe_cap_clipboard::reset_pending_tracking();
    // カメラのライブ映像も同様。画面状態が generation 0 に戻るため、動いたままのセッションを止めて `ACTIVE` を解放する（`fandhe_cap_camera_preview_stream::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "camera-preview-stream")]
    fandhe_cap_camera_preview_stream::reset_pending_tracking();
    // ローカル通知の保留中の要求も破棄する。`bridge::SLOT` はプロセス static のため再生成をまたいで残り、画面状態は `Idle` で
    // 作り直されるため、再生成前に開始した権限確認が後から完了しても黙って失われる（`fandhe_cap_notification::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "notification")]
    fandhe_cap_notification::reset_pending_tracking();
    // BLE スキャンの保留中の要求と観測バッファも同様に破棄する（`fandhe_cap_bluetooth_le::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "bluetooth-le")]
    fandhe_cap_bluetooth_le::reset_pending_tracking();
    // Android の BLE 周辺機器役の通知先も破棄する（`fandhe_cap_ble_peripheral::reset_pending_tracking` の doc 参照）。
    #[cfg(feature = "ble-peripheral")]
    fandhe_cap_ble_peripheral::reset_pending_tracking();
    // Bluetooth Classic の保留中の要求と観測バッファも同様に破棄する（`fandhe_cap_bluetooth_classic::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "bluetooth-classic")]
    fandhe_cap_bluetooth_classic::reset_pending_tracking();
    // 音声再生も同様。画面状態が generation 0 に戻るため、処理済み世代を捨て再生を止める
    // （`fandhe_cap_media_playback::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "media-playback")]
    fandhe_cap_media_playback::reset_pending_tracking();
    // ロック画面・OS のメディア操作も同様。受付を止めて表示を取り下げ、記録を捨てる
    // （`fandhe_cap_now_playing_controls::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "now-playing-controls")]
    fandhe_cap_now_playing_controls::reset_pending_tracking();
    // アプリ内ブラウザの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない。`fandhe_cap_in_app_browser::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "in-app-browser")]
    fandhe_cap_in_app_browser::reset_pending_tracking();
    // マイク録音・音声認識の保留中の要求も破棄する。`bridge::SLOT`/`STARTED_REQUEST_ID` はプロセス static のため再生成をまたいで残り、
    // 画面状態は `Idle` で作り直されるため、残すと結果が画面へ反映されず次の操作が詰まる（各 crate の `reset_pending_tracking` doc 参照）。
    // 各 crate の reset は Java 側の保留・実行中の要求も止める（結果は届けない）。再生成の引き継ぎ・終了時の中断は helper が自分で行い、ここには依存しない。
    #[cfg(feature = "mic-record")]
    fandhe_cap_mic_record::reset_pending_tracking();
    #[cfg(feature = "speech-recognition")]
    fandhe_cap_speech_recognition::reset_pending_tracking();
    // 決済シートの保留中の要求を破棄し、旧 request_id の完了を不一致で捨てさせる（`fandhe_cap_payment_request::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "payment-request")]
    fandhe_cap_payment_request::reset_pending_tracking();
    // 近くの端末との直接通信の保留中の要求と受信バッファも破棄する（`bridge::SLOT` / `PENDING` はプロセス static のため再生成をまたいで残る。`fandhe_cap_local_peer_connect::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "local-peer-connect")]
    fandhe_cap_local_peer_connect::reset_pending_tracking();
    // ピクチャ・イン・ピクチャも同様。処理済み世代を捨て、小窓を止める（`fandhe_cap_picture_in_picture::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "picture-in-picture")]
    fandhe_cap_picture_in_picture::reset_pending_tracking();
    // ドラッグ & ドロップの集約器もプロセス static のため再生成をまたいで残る。進行中のセッション・未適用イベントを
    // 新しい画面状態を作る前に捨てる（`fandhe_cap_drag_drop::reset_sessions` doc 参照。以前は egui-proxy の `run` が呼んでいた）。
    #[cfg(feature = "drag-drop")]
    fandhe_cap_drag_drop::reset_sessions();
}
