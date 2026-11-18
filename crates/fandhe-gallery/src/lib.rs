//! fandhe-gallery: 全機能の画面を登録し、実機確認・撮影に使う非公開アプリ（`publish = false`）。
//!
//! 位置づけ（`docs/architecture/consumer-app-structure.md` §3、TASK-21.2 / Issue #1165）:
//! 画面の登録（[`screens`]）と、各 OS の最終成果物の入口を持つ。
//!
//! - デスクトップ・iOS の実行体: `main.rs`（bin `fandhe-gallery-desktop`）
//! - Android の cdylib: 本ファイルの `android_main`
//! - Web の wasm cdylib: 本ファイルの `start`（`#[wasm_bindgen(start)]`）
//!
//! 端末機能の配線と各 OS の入口関数は `fandhe-egui-proxy`（機能 crate へ分けるまで、#1194。終了時の解放と Android の画面ゲートは本 crate が呼ぶ）、
//! 描画と起動は `fandhe-render-egui` が持つ。依存の向きは本 crate → egui-proxy の一方向で、
//! 画面の登録は入口へ引数（`BuildAppFn`）で渡す。`android_main` / `start` のような
//! 最終成果物ごとに 1 つだけの固定シンボルは、重複を避けるため rlib の egui-proxy ではなく本 crate が持つ。
//!
//! Android のみ `unsafe_code` を `deny` へ緩め、`android_main` の `#[no_mangle]` 1 箇所にだけ
//! `#[allow(unsafe_code)]` を付ける（`coding-rust.md` の FFI 例外規定）。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

pub mod screens;
pub use screens::{build_app, registered_names};

/// Android アプリのエントリポイント（計測専用ビルド）。
///
/// `android-activity`（GameActivity 方式）がライブラリロード後にこのシンボル名を検索して呼ぶ
/// （OS 側グルーコードとの契約で、Rust 側から呼ばれることはない）。`frame-metrics` 有効時は
/// 計測専用アプリへ委譲する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は Rust の名前修飾を無効化するだけで、
/// シンボル名 `android_main` は `android-activity` の規約上の固定名であり、本 crate は cdylib として
/// 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない（egui-proxy は rlib で
/// 同名シンボルを持たない）。
#[cfg(all(target_os = "android", feature = "frame-metrics"))]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: fandhe_egui_proxy::android::AndroidApp) {
    fandhe_egui_proxy::frame_metrics::run_android(app);
}

/// Android アプリのエントリポイント。
///
/// 実処理は egui-proxy の `android::run` で、画面の登録（[`build_app`]）を渡す。
///
/// # Safety
/// 上の `frame-metrics` 有効時の `android_main` と同じ根拠（この関数自体は `unsafe` 操作を含まない）。
#[cfg(all(target_os = "android", not(feature = "frame-metrics")))]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: fandhe_egui_proxy::android::AndroidApp) {
    // 片付けの前に新しい `AndroidApp` を登録する。通知の閉じ込み等は `android_app()` を使うため、
    // 新プロセスで未登録のまま呼ぶと何も消えない（`run` 内の登録と同じ値で、そちらは上書きになるだけ）。
    fandhe_platform_android::set_android_app(app.clone());
    // Activity 再生成では画面状態が作り直される（世代 0）。保留中の書類スキャン要求が残ると
    // 次の要求が飲み込まれるため、新しい画面状態を作る前に破棄する（TASK-21.3 / Issue #1171。
    // 以前は egui-proxy の `android::run` が呼んでいた。機能 crate を egui-proxy が知らないようにするため移した）。
    fandhe_cap_document_scan::reset_pending_tracking();
    // 写真選択の保留中の要求も破棄する（契約は「復元せず破棄」。以前は egui-proxy の `android::run` が呼んでいた。旧要求の遅延完了を
    // id 不一致で無視させる。`fandhe_cap_photo_picker::reset_pending_tracking` doc 参照）。
    fandhe_cap_photo_picker::reset_pending_tracking();
    // 写真ライブラリ保存の保留中の要求も破棄する（保留中の保存要求は復元せず破棄し、旧要求の遅延完了を id 不一致で無視させる。`fandhe_cap_photo_library::reset_pending_tracking` doc 参照）。
    fandhe_cap_photo_library::reset_pending_tracking();
    // 位置情報の再試行の処理済み世代を基準値へ戻す（画面状態は再生成で retry_seq 0 へ戻る。`fandhe_cap_location::reset_retry_tracking` doc 参照）。
    fandhe_cap_location::reset_retry_tracking();
    // ジオフェンス画面の状態も再生成で未登録へ戻るため、static 側の登録もまとめて破棄する。
    fandhe_cap_geofence::reset_register_tracking();
    // LAN のサービス探索の保留中の要求も破棄する（プロセス static の旧要求を再生成後へ持ち越さない。`fandhe_cap_lan_discovery::reset_pending_tracking` doc 参照）。
    fandhe_cap_lan_discovery::reset_pending_tracking();
    // 機械学習推論の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の
    // 「推論する」が飲み込まれる（`fandhe_cap_ml_inference::reset_pending_tracking` doc 参照）。
    fandhe_cap_ml_inference::reset_pending_tracking();
    // 端末内翻訳の処理済み世代も破棄する（画面状態は世代 0 に戻るため。`fandhe_cap_translation::reset_pending_tracking` doc 参照）。
    fandhe_cap_translation::reset_pending_tracking();
    // 生体認証の保留中の要求も破棄する。画面状態は再生成で `Idle` に戻るため、残すと結果が画面へ反映されず失われる
    // （`fandhe_cap_biometric_auth::reset_pending_tracking` doc 参照）。
    fandhe_cap_biometric_auth::reset_pending_tracking();
    // 連絡先選択の保留中の要求も破棄する（契約は「復元せず破棄」。旧要求の遅延完了を id 不一致で無視させる。
    // `fandhe_cap_contacts::reset_pending_tracking` doc 参照）。
    fandhe_cap_contacts::reset_pending_tracking();
    // 付属機器のペアリングの保留中の要求も破棄する（理由は ml_inference と同じ。
    // `fandhe_cap_companion_device::reset_pending_tracking` doc 参照）。
    fandhe_cap_companion_device::reset_pending_tracking();
    // 通話 UI の記録（処理済み世代）と進行中の通話も破棄する。画面状態は再生成で generation 0 に戻るため、
    // 残すと次の「着信を模擬する」が飲み込まれ、OS 側の着信も残る（`fandhe_cap_call_ui::reset_pending_tracking` doc 参照）。
    fandhe_cap_call_ui::reset_pending_tracking();
    // ライブアクティビティの保留中の要求も破棄し、表示中の通知を閉じる。再生成後の画面は Idle から始まるため、
    // 残すと次の操作と OS 側の表示が食い違う（`fandhe_cap_live_activity::reset_pending_tracking` doc 参照）。
    fandhe_cap_live_activity::reset_pending_tracking();
    // 予定追加の保留中の要求も破棄する（`SLOT` はプロセス static のため再生成をまたいで残る。
    // `fandhe_cap_calendar::reset_pending_tracking` doc 参照）。
    fandhe_cap_calendar::reset_pending_tracking();
    // 第 3 束の機能: 端末内 LLM・音声変換・アイコンバッジ・MIDI・パスキー・画面録画の保留中の要求と処理済み世代も
    // 破棄する（理由は ml_inference と同じ。各 crate の `reset_pending_tracking` doc 参照）。
    fandhe_cap_on_device_llm::reset_pending_tracking();
    fandhe_cap_media_transcode::reset_pending_tracking();
    fandhe_cap_app_badge::reset_pending_tracking();
    fandhe_cap_midi::reset_pending_tracking();
    // 印刷の保留中の要求と画面の取り込み待ちも破棄する（Activity 再生成で印刷の保留は復元しない。
    // `fandhe_cap_print::reset_pending_tracking` doc 参照）。
    fandhe_cap_print::reset_pending_tracking();
    fandhe_cap_passkey_signin::reset_pending_tracking();
    fandhe_cap_screen_capture::reset_pending_tracking();
    // PDF 表示の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の
    // 「表示する」が飲み込まれる（`fandhe_cap_pdf_viewer::reset_pending_tracking` doc 参照）。
    fandhe_cap_pdf_viewer::reset_pending_tracking();
    // 端末・アプリの真正性証明の処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の取得が飲み込まれる）（`fandhe_cap_app_attest::reset_pending_tracking` doc 参照）。
    fandhe_cap_app_attest::reset_pending_tracking();
    // スペルチェックの保留中の要求も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の検査が飲み込まれる）（`fandhe_cap_spell_check::reset_pending_tracking` doc 参照）。
    fandhe_cap_spell_check::reset_pending_tracking();
    // 住所の座標変換の保留中の要求も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の変換が飲み込まれる）（`fandhe_cap_geocoding_search::reset_pending_tracking` doc 参照）。
    fandhe_cap_geocoding_search::reset_pending_tracking();
    // 音声セッションの保持中の購読と処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の開始が飲み込まれる（`fandhe_cap_audio_session::reset_pending_tracking` doc 参照）。
    fandhe_cap_audio_session::reset_pending_tracking();
    // 他アプリ起動の保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_external_app_launch::reset_pending_tracking` doc 参照）。
    fandhe_cap_external_app_launch::reset_pending_tracking();
    // 設定画面を開く保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_open_system_settings::reset_pending_tracking` doc 参照）。
    fandhe_cap_open_system_settings::reset_pending_tracking();
    // 壁紙設定の保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_wallpaper_set::reset_pending_tracking` doc 参照）。
    fandhe_cap_wallpaper_set::reset_pending_tracking();
    // ファイルプレビューの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_file_preview::reset_pending_tracking` doc 参照）。
    fandhe_cap_file_preview::reset_pending_tracking();
    // 動画の撮影の保留中の要求も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の要求が飲み込まれ、
    // 旧要求の遅延完了は id 不一致で捨てさせる（`fandhe_cap_video_capture::reset_pending_tracking` doc 参照）。
    fandhe_cap_video_capture::reset_pending_tracking();
    // 無線の切り替えの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_radio_control::reset_pending_tracking` doc 参照）。
    fandhe_cap_radio_control::reset_pending_tracking();
    // 動画変換の処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の変換が飲み込まれる）（`fandhe_cap_video_transcode::reset_pending_tracking` doc 参照）。
    fandhe_cap_video_transcode::reset_pending_tracking();
    // NFC タグ読み取りの保留中の要求と処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の読み取りが飲み込まれる）（`fandhe_cap_nfc::reset_pending_tracking` doc 参照）。
    fandhe_cap_nfc::reset_pending_tracking();
    // 機器からの取り込みの保留中の要求と処理済み世代も破棄する（画面状態は再生成で generation 0 に戻るため、残すと次の取り込みが飲み込まれる）（`fandhe_cap_scanner_devices::reset_pending_tracking` doc 参照）。
    fandhe_cap_scanner_devices::reset_pending_tracking();
    // メール・SMS 作成の保留中の要求も破棄する（`bridge::SLOT` はプロセス static のため再生成をまたいで残る。`fandhe_cap_compose_mail_sms::reset_pending_tracking` doc 参照）。
    fandhe_cap_compose_mail_sms::reset_pending_tracking();
    // クリップボードの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない）（`fandhe_cap_clipboard::reset_pending_tracking` doc 参照）。
    fandhe_cap_clipboard::reset_pending_tracking();
    // カメラのライブ映像も同様。画面状態が generation 0 に戻るため、動いたままのセッションを止めて `ACTIVE` を解放する（`fandhe_cap_camera_preview_stream::reset_pending_tracking` doc 参照）。
    fandhe_cap_camera_preview_stream::reset_pending_tracking();
    // ローカル通知の保留中の要求も破棄する。`bridge::SLOT` はプロセス static のため再生成をまたいで残り、画面状態は `Idle` で
    // 作り直されるため、再生成前に開始した権限確認が後から完了しても黙って失われる（`fandhe_cap_notification::reset_pending_tracking` doc 参照）。
    fandhe_cap_notification::reset_pending_tracking();
    // BLE スキャンの保留中の要求と観測バッファも同様に破棄する（`fandhe_cap_bluetooth_le::reset_pending_tracking` doc 参照）。
    fandhe_cap_bluetooth_le::reset_pending_tracking();
    // Android の BLE 周辺機器役の通知先も破棄する（`fandhe_cap_ble_peripheral::reset_pending_tracking` の doc 参照）。
    fandhe_cap_ble_peripheral::reset_pending_tracking();
    // Bluetooth Classic の保留中の要求と観測バッファも同様に破棄する（`fandhe_cap_bluetooth_classic::reset_pending_tracking` doc 参照）。
    fandhe_cap_bluetooth_classic::reset_pending_tracking();
    // 音声再生も同様。画面状態が generation 0 に戻るため、処理済み世代を捨て再生を止める
    // （`fandhe_cap_media_playback::reset_pending_tracking` doc 参照）。
    fandhe_cap_media_playback::reset_pending_tracking();
    // ロック画面・OS のメディア操作も同様。受付を止めて表示を取り下げ、記録を捨てる
    // （`fandhe_cap_now_playing_controls::reset_pending_tracking` doc 参照）。
    fandhe_cap_now_playing_controls::reset_pending_tracking();
    // アプリ内ブラウザの保留中の要求も破棄する（Activity 再生成で保留中の要求を復元しない。`fandhe_cap_in_app_browser::reset_pending_tracking` doc 参照）。
    fandhe_cap_in_app_browser::reset_pending_tracking();
    // マイク録音・音声認識の保留中の要求も破棄する。`bridge::SLOT`/`STARTED_REQUEST_ID` はプロセス static のため再生成をまたいで残り、
    // 画面状態は `Idle` で作り直されるため、残すと結果が画面へ反映されず次の操作が詰まる（各 crate の `reset_pending_tracking` doc 参照）。
    // 各 crate の reset は Java 側の保留・実行中の要求も止める（結果は届けない）。再生成の引き継ぎ・終了時の中断は helper が自分で行い、ここには依存しない。
    fandhe_cap_mic_record::reset_pending_tracking();
    fandhe_cap_speech_recognition::reset_pending_tracking();
    // 決済シートの保留中の要求を破棄し、旧 request_id の完了を不一致で捨てさせる（`fandhe_cap_payment_request::reset_pending_tracking` doc 参照）。
    fandhe_cap_payment_request::reset_pending_tracking();
    // 近くの端末との直接通信の保留中の要求と受信バッファも破棄する（`bridge::SLOT` / `PENDING` はプロセス static のため再生成をまたいで残る。`fandhe_cap_local_peer_connect::reset_pending_tracking` doc 参照）。
    fandhe_cap_local_peer_connect::reset_pending_tracking();
    // ピクチャ・イン・ピクチャも同様。処理済み世代を捨て、小窓を止める（`fandhe_cap_picture_in_picture::reset_pending_tracking` doc 参照）。
    fandhe_cap_picture_in_picture::reset_pending_tracking();
    // ドラッグ & ドロップの集約器もプロセス static のため再生成をまたいで残る。進行中のセッション・未適用イベントを
    // 新しい画面状態を作る前に捨てる（`fandhe_cap_drag_drop::reset_sessions` doc 参照。以前は egui-proxy の `run` が呼んでいた）。
    fandhe_cap_drag_drop::reset_sessions();
    // 他画面でのアナログ入力・他画面宛てのドロップによる再描画を抑える画面ゲートを、画面の構築前に設定する
    // （`fandhe_cap_gamepad::set_screen_active`・`fandhe_cap_drag_drop::set_screen_active` doc 参照）。
    {
        let name = fandhe_egui_proxy::android::requested_app_name();
        fandhe_cap_gamepad::set_screen_active(name == "gamepad_show");
        fandhe_cap_tilt_motion::set_screen_active(name == "tilt_ball");
        fandhe_cap_drag_drop::set_screen_active(name == "drag_drop");
    }
    // 終了時の解放処理を描画層へ登録する（再生成で再実行されても 2 回目以降は無視される）。
    register_teardown_hooks();
    fandhe_egui_proxy::android::run(app, build_app);
}

/// アプリ終了時の解放処理を描画層（`fandhe-render-egui`）へ登録する。各 OS の入口（`main.rs`・`android_main`）が
/// 起動前に 1 回呼ぶ。描画層は端末機能を知らないため、終了（ウィンドウを閉じる経路）と `ProxyApp` の破棄で
/// 呼ぶ関数をここで渡す。`device` 無効のビルド・wasm32 では何もしない。
pub fn register_teardown_hooks() {
    #[cfg(not(target_arch = "wasm32"))]
    fandhe_render_egui::app::set_teardown_hooks(shutdown, fandhe_cap_memory_pressure::shutdown);
}

/// 終了時の停止。機能 crate を順に止める。機能 crate の停止は相互に独立で、依存のある media_playback → audio_session の前後は
/// 保っている。
#[cfg(not(target_arch = "wasm32"))]
fn shutdown() {
    // 読み上げの子プロセス（`say`・PowerShell）を止める。プロセスが終了すると `static` の `Drop` は走らず、ウィンドウを閉じても
    // 発話中の子プロセスが残るため、終了フックから明示的に止める（キューがあれば発話要求と同じキューへ積み、発送済みの発話より後に止まる。
    // 公開入口から直接発話してキューが無いときは、その場で止める）。
    fandhe_cap_tts::shutdown();
    // 進行中の Web 認証（デモサーバーと OS のセッション）を閉じる。
    fandhe_cap_web_auth_session::shutdown();
    // 再生（media-playback）→ OS のメディア操作（now-playing-controls）の順に止め、どちらも audio_session より前に置く。
    fandhe_cap_media_playback::shutdown();
    fandhe_cap_now_playing_controls::shutdown();
    fandhe_cap_bluetooth_le::stop_shared_scan();
    // 公開の停止は `.m` の専用キューへの短い同期呼び出し。Rust 側のロックは握らない。
    fandhe_cap_ble_peripheral::shutdown();
    // macOS の停止は main queue へ同期ディスパッチする。`shutdown` は eframe の `on_exit`（main thread）からロックを握らずに呼ばれるため、
    // main 上なら直接停止し、デッドロックしない。
    fandhe_cap_bluetooth_classic::stop_shared_discovery();
    fandhe_cap_system_tray::stop_shared_system_tray();
    fandhe_cap_global_hotkey::shutdown();
    fandhe_cap_midi::shutdown();
    // 位置情報の測位子プロセス（Windows）を止める。
    fandhe_cap_location::stop_shared_location();
    // LAN のサービス探索の停止（Rust 側のロックを握らずに OS 側を止める）。
    fandhe_cap_lan_discovery::stop_shared_discovery();
    fandhe_cap_media_cast::shutdown();
    fandhe_cap_serial_port::shutdown();
    fandhe_cap_audio_session::shutdown();
    fandhe_cap_memory_pressure::shutdown();
    fandhe_cap_camera_preview_stream::shutdown();
    // 近くの端末との直接通信の進行中の交換を止める。OS 側の停止は専用キューへの短い同期呼び出しで、Rust 側のロックは握らない。
    fandhe_cap_local_peer_connect::shutdown();
    // 小窓の資源（macOS はタイマー・controller・layer、Android は小窓の解除）を解放する。
    fandhe_cap_picture_in_picture::shutdown();
}

/// Web のエントリポイント。wasm-bindgen の glue が読み込み時に呼ぶ。実処理は egui-proxy の
/// `web::start` で、画面の登録（[`build_app`]）を渡す。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    fandhe_egui_proxy::web::start(build_app)
}
