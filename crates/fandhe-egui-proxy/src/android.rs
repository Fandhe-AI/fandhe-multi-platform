//! Android 向けブートストラップ（TASK-1.5-S2 / REQ-1 / M3。Activity 方式は
//! GameActivity（TASK-9.3-S1 / REQ-9 / M4、Issue #242）。
//! 決定記録: `docs/architecture/android-activity-method.md`）。
//!
//! `android-activity`（GameActivity 方式。`Cargo.toml` の
//! `cfg(target_os = "android")` 依存セクション参照）が期待する固定シンボル名
//! `android_main` を公開し、`platforms/android/app` の `MainActivity`
//! （`GameActivity` サブクラス、`System.loadLibrary` で本 cdylib をロード）から
//! 起動される。他プラットフォーム（デスクトップ = glow、Web = wasm32/glow、
//! iOS = wgpu/Metal、`main.rs` / `web.rs` 参照）と同様、実描画は `ProxyApp`
//! （`app.rs`）へ委譲する。GameActivity への切替は本関数のシグネチャ・処理内容に
//! 影響しない（`android-activity` が `AndroidApp` 型で両バックエンドの差異を吸収する）。
//!
//! Vulkan 固定: REQ-1 は「Android = Vulkan」を要求するため、wgpu の
//! `InstanceDescriptor::backends` を [`wgpu::Backends::VULKAN`] に固定した
//! `WgpuConfiguration` を `NativeOptions` へ設定する（他バックエンドへは
//! フォールバックしない）。
//!
//! `accesskit_android` との組み合わせは本クレートでは現状 `Cargo.toml` 側で
//! `accesskit` feature を有効化せず非対応のままとしている（GameActivity 方式への
//! 切替自体は #242 で完了済みだが、accesskit feature の有効化・動作整合は
//! 別 issue #243 / TASK-9.3-S2 のスコープ）。

/// `android_main` が受け取った [`AndroidApp`] を保持し、`device/android.rs`
/// （Issue #750。`DataDir` 解決・カメラ Intent 中継の JNI 呼び出し）から
/// 読み出せるようにする（Phase 8 固有ではない汎用の 3 行で、切り離し時は残置してよい。
/// docs/architecture/device-feature-removal.md参照）。
///
/// `OnceLock` ではなく `Mutex` にするのは、Manifest の `configChanges` 外の再構成
/// （例: `uiMode` 変更）で OS が `Activity` を破棄・再生成すると `android_main` が
/// 新しい [`AndroidApp`] で再度呼ばれるため（codex-review 指摘、PR #790）。`set_android_app`
/// が呼ばれるたびに保持内容を差し替え、破棄済み `Activity` を指す古い値を残さない。
///
/// [`AndroidApp`]: winit::platform::android::activity::AndroidApp
///
/// `frame-metrics` feature 有効時（計測専用ビルド、`crate::frame_metrics::run_android` が
/// 別の `android_main` 定義を使う）は `set_android_app` が呼ばれないため `android_app()` は
/// 常に `None` を返す。Issue #796（ソフトキーボード文字入力の `TextInputState` ポーリング、
/// `poll_soft_keyboard`）でも `device/android.rs` と同じく `android_app()` を使うため、
/// cfg は付けない（従来の `feature = "device"` 限定を撤廃）。
static ANDROID_APP: std::sync::Mutex<Option<winit::platform::android::activity::AndroidApp>> =
    std::sync::Mutex::new(None);

/// `android_main` が組み立てる `eframe::App` の `egui::Context`（`cc.egui_ctx.clone()`）を
/// 保持する（Issue #831 codex-review 指摘 PRRT_kwDOTl73E86hCOSM）。`device/android.rs` の
/// `nativeOnMotionChanged` は UI スレッド外（Java 側センサコールバック）から
/// [`crate::device::tilt::shared_motion`] へ傾きを push するだけでは egui の描画ループを
/// 起こせない（`tilt_ball_effect` の自走は非ゼロ傾きの `drive` 呼び出しが前提で、
/// 描画が一度止まると次に呼ばれる保証がない）ため、値更新のたびに
/// [`request_repaint`] を呼んで確実に再開させる。
///
/// `OnceLock` ではなく `Mutex` にするのは、[`ANDROID_APP`] と同じ理由（codex-review 指摘、
/// Issue #831）。`configChanges` 外の再構成で `Activity` が再生成されると `android_main` が
/// 新しい `egui::Context` を伴って再度呼ばれる。`OnceLock` のまま 2 回目以降の `set` を
/// 無視すると、破棄済み `Activity` に紐づく古い `Context` へ `request_repaint` し続け、
/// 再生成後の新しい画面が起きない（Cursor Bugbot 同一指摘 discussion_r3978309491）。
/// `android_main` の呼び出しのたびに保持内容を差し替える。
static EGUI_CTX: std::sync::Mutex<Option<egui::Context>> = std::sync::Mutex::new(None);

/// 現在表示中の画面が傾き（`tilt_ball`）を使うかどうか（codex-review 指摘 P2、Issue #831）。
/// `Java` 側の `motionListener` は画面に関わらず常にセンサを登録して通知するため、
/// 素直に毎回 `request_repaint` すると傾きを使わない画面でも `SENSOR_DELAY_GAME` の
/// 頻度で再描画要求が発生し不要な電力消費になる。`android_main` の起動時（`FANDHE_PROXY_APP`
/// で決まる画面は `Activity` 生存中は変わらない、`screens.rs` の `selected_app_name` 契約）
/// に 1 度だけ設定し、[`crate::device::android`] のセンサ JNI エクスポートが
/// `request_repaint` 呼び出し前にここを見て要否を判定する。
static ACTIVE_SCREEN_USES_TILT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// [`ACTIVE_SCREEN_USES_TILT`] を差し替える。`android_main` が起動画面を決定した直後に呼ぶ。
pub(crate) fn set_active_screen_uses_tilt(uses_tilt: bool) {
    ACTIVE_SCREEN_USES_TILT.store(uses_tilt, std::sync::atomic::Ordering::Relaxed);
}

/// [`ACTIVE_SCREEN_USES_TILT`] を読む。傾き画面以外ではセンサ通知のたびの
/// `request_repaint` を省略するためのゲートとして使う。
pub(crate) fn active_screen_uses_tilt() -> bool {
    ACTIVE_SCREEN_USES_TILT.load(std::sync::atomic::Ordering::Relaxed)
}

/// 現在表示中の画面が `gamepad_show` かどうか（Bugbot 指摘、PR #939）。`ACTIVE_SCREEN_USES_TILT`
/// と同じ理由・同じ契約（Java 側は画面に関わらず `MotionEvent` を常に配送するため、
/// 表示していない画面でのスティックドリフトによる高頻度 `request_repaint` を防ぐ）。
static ACTIVE_SCREEN_USES_GAMEPAD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// [`ACTIVE_SCREEN_USES_GAMEPAD`] を差し替える。`android_main` が起動画面を決定した直後に呼ぶ。
pub(crate) fn set_active_screen_uses_gamepad(uses_gamepad: bool) {
    ACTIVE_SCREEN_USES_GAMEPAD.store(uses_gamepad, std::sync::atomic::Ordering::Relaxed);
}

/// [`ACTIVE_SCREEN_USES_GAMEPAD`] を読む。`gamepad_show` 以外ではアナログイベントの
/// たびの `request_repaint` を省略するためのゲートとして使う。
pub(crate) fn active_screen_uses_gamepad() -> bool {
    ACTIVE_SCREEN_USES_GAMEPAD.load(std::sync::atomic::Ordering::Relaxed)
}

/// 現在表示中の画面が `drag_drop` かどうか（Issue #1016）。`ACTIVE_SCREEN_USES_TILT`/
/// `_GAMEPAD` と同じ理由・同じ契約: `MainActivity.java` の `dragListener` は
/// `ACTION_DRAG_STARTED` のたびに [`Java_com_fandhe_androidapp_MainActivity_nativeDragDropIsActive`]
/// で問い合わせ、`false` ならそのドラッグの以降のイベント（Hover/Begin/Wants/Item）を
/// 一切送らない（表示していない画面が他画面宛てのドロップを受け取らないための
/// ゲート、契約表 1）。
static ACTIVE_SCREEN_USES_DRAG_DROP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// [`ACTIVE_SCREEN_USES_DRAG_DROP`] を差し替える。`android_main` が起動画面を決定した
/// 直後に呼ぶ。
pub(crate) fn set_active_screen_uses_drag_drop(uses_drag_drop: bool) {
    ACTIVE_SCREEN_USES_DRAG_DROP.store(uses_drag_drop, std::sync::atomic::Ordering::Relaxed);
}

/// [`ACTIVE_SCREEN_USES_DRAG_DROP`] を読む。
pub(crate) fn active_screen_uses_drag_drop() -> bool {
    ACTIVE_SCREEN_USES_DRAG_DROP.load(std::sync::atomic::Ordering::Relaxed)
}

/// [`EGUI_CTX`] を読み、確保済みなら `request_repaint` を呼ぶ。`android_main` 実行前
/// （通常到達しない）は何もしない。
pub(crate) fn request_repaint() {
    if let Some(ctx) = EGUI_CTX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        ctx.request_repaint();
    }
}

/// [`ANDROID_APP`] の保持内容を差し替える。`android_main`（`frame-metrics` 無効時）が
/// `eframe::run_native` を呼ぶ前に毎回呼ぶ（`Activity` 再生成のたびに `android_main` が
/// 再実行されるため、初回だけでなく毎回上書きする）。
pub(crate) fn set_android_app(app: winit::platform::android::activity::AndroidApp) {
    let mut slot = ANDROID_APP.lock().unwrap_or_else(|e| e.into_inner());
    *slot = Some(app);
}

/// [`ANDROID_APP`] を読む。`android_main`（`frame-metrics` 無効時）が
/// `eframe::run_native` を呼ぶ前に `set_android_app` 済みのため、その `android_main` 内で
/// 構築される [`crate::ProxyApp`] 等から発火する effect
/// （`device::camera_capture::camera_capture_effect` 等）や [`poll_soft_keyboard`] が
/// 呼ばれる時点では必ず `Some` になる。`AndroidApp` は `Arc` 内部実装で `Clone` が安価
/// （android-activity 0.6.1 の型定義参照）なため、`Mutex` 保持のまま `'static` 参照を
/// 返すことはできず所有値を複製して返す。
pub(crate) fn android_app() -> Option<winit::platform::android::activity::AndroidApp> {
    ANDROID_APP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Android 向け Vulkan 固定バックエンド設定を返す。
///
/// `android_main`（feature 無効時）から呼ばれる。`NativeOptions` 構築処理から
/// 値を切り出すことで、REQ-1（Android = Vulkan）の充足を単体テストで検証できるようにしている。
/// `frame-metrics` feature 有効時は `crate::frame_metrics::run_android` が Vulkan 固定を
/// 独自に持つため（dead_code 警告を避けるため cfg で揃える）、本関数もその feature では
/// コンパイル対象外にする。
#[cfg(not(feature = "frame-metrics"))]
fn android_wgpu_backends() -> eframe::wgpu::Backends {
    eframe::wgpu::Backends::VULKAN
}

/// Android アプリのエントリポイント。
///
/// `android-activity`（GameActivity 方式、TASK-9.3-S1 / #242）が
/// ライブラリロード後にこのシンボル名を検索して呼び出す（OS 側グルーコードとの契約であり、
/// Rust 側から能動的に呼ばれることはない）。`#[no_mangle]` はこの 1 関数に閉じ、`unsafe` を
/// 公開 API 側へ漏らさない（`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名 `android_main` は `android-activity` の規約上の固定名
/// （winit の `android-game-activity` feature 経由で `android_activity` crate が
/// 探索する）であり、本クレートは cdylib として 1 つの Android 実行単位にのみ
/// リンクされるため他シンボルと衝突しない。
// TASK-5.3（REQ-5 / M4、Issue #238）: `frame-metrics` feature 有効時は
// `crate::frame_metrics::run_android`（計測専用アプリ）へ委譲する。既存の `ProxyApp`
// 経路（feature 無効時）は変更しない（`frame_metrics.rs` モジュール doc 参照）。
// `#[no_mangle]` の固定シンボル名は 1 バイナリにつき 1 定義のみ許されるため、
// feature の有無で定義自体を排他的に切り替える（両方が同時にコンパイル対象になると
// シンボル重複でリンクエラーになる）。
/// # Safety
/// この関数自体は `unsafe` 操作を含まない（モジュール冒頭のシンボル名固定の説明を参照）。
#[cfg(feature = "frame-metrics")]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    crate::frame_metrics::run_android(app);
}

/// # Safety
/// この関数自体は `unsafe` 操作を含まない（上の `frame-metrics` 有効時の
/// `android_main` と同じ根拠。モジュール冒頭のシンボル名固定の説明を参照）。
#[cfg(not(feature = "frame-metrics"))]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    // Issue #750 / PR #790 codex-review 指摘: `device/android.rs` が `DataDir` 解決・
    // カメラ Intent 中継のために `AndroidApp` を必要とするため、`NativeOptions` へ move
    // する前に static へ複製する（`AndroidApp` は `Arc` 内部実装で `Clone` が安価、
    // android-activity 0.6.1 の型定義参照）。`configChanges` 外の再構成で `android_main`
    // が新しい `AndroidApp` で再度呼ばれた場合は毎回上書きする（古い破棄済み `Activity` を
    // 指したまま残さない。`set_android_app` doc 参照）。Issue #796 の `poll_soft_keyboard` も
    // `device` feature 無効時（`android_app()` の cfg 撤廃済み）に本 static を必要とするため、
    // `set_android_app` 呼び出し自体は feature に関わらず常に行う。
    set_android_app(app.clone());
    // Issue #890 cursor Bugbot 指摘（PR #894 discussion_r4015098279）: `location.rs` の
    // `LAST_PROCESSED_RETRY_SEQ` はプロセス static のため `Activity` 再生成をまたいで残る。
    // 新しい画面 `S`（`LocationShowState::default()` = `retry_seq: 0`）を構築する前に
    // ここで基準値を作り直し、再生成前に「再試行」を複数回押していた場合でも、
    // 再生成後に改めて「再試行」を押したときに正しく再要求が発火するようにする
    // （`device::location::reset_retry_tracking` doc 参照）。`device` feature 無効ビルドは
    // `location` モジュール自体が存在しないため呼ばない。
    #[cfg(feature = "device")]
    crate::device::location::reset_retry_tracking();
    // geofence 画面の状態も再生成で未登録へ戻るため、static 側の登録もまとめて破棄する。
    #[cfg(feature = "device")]
    crate::device::geofence::reset_register_tracking();
    // Issue #908 codex-review 指摘（PR #917）: `biometric_auth::bridge::PENDING` も
    // プロセス static のため、上記 `location` と同じ理由で Activity 再生成をまたいで
    // 残る。新しい画面 `BiometricCheckState::default()`（`Idle`）を構築する前に
    // ここで捨て、再生成前に開始した認証が後から完了しても黙って失われず、以後
    // 確実に「未接続」（次の操作からやり直し）として扱われるようにする
    // （`device::biometric_auth::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::biometric_auth::reset_pending_tracking();
    // Issue #957: `mic_record::bridge::SLOT`/`STARTED_REQUEST_ID` も同じ理由で
    // プロセス static のため Activity 再生成をまたいで残る。新しい画面
    // `MicRecordState::default()`（`Idle`）を構築する前にここで捨て、再生成前に
    // 開始した録音が後から完了しても黙って失われず、以後確実に「未接続」（次の
    // 操作からやり直し）として扱われるようにする
    // （`device::mic_record::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::mic_record::reset_pending_tracking();
    // Issue #1335: `screen_capture::bridge` も同じ理由で再生成前に捨てる。
    #[cfg(feature = "device")]
    crate::device::screen_capture::reset_pending_tracking();
    // Issue #1026: `speech_recognition::bridge::SLOT`/`STARTED_REQUEST_ID` も同じ理由で
    // プロセス static のため Activity 再生成をまたいで残る。新しい画面
    // `SpeechRecognizeState::default()`（`Idle`）を構築する前にここで捨て、再生成前に
    // 開始した聞き取りが後から完了しても黙って失われず、以後確実に「未接続」（次の
    // 操作からやり直し）として扱われるようにする（`device::speech_recognition::
    // reset_pending_tracking` doc 参照）。Java 側（`MainActivity.onCreate`）に対になる
    // no-op スタブは置かない: Rust の `request_speech_start` を経ないと Java が
    // `nativeOnSpeechResult`/`nativeOnSpeechError` を呼ぶ経路自体が無く、
    // `feature = "device"` 無効ビルドでは到達しないため（`mic_record`/`notification` と
    // 同じ理由）。
    #[cfg(feature = "device")]
    crate::device::speech_recognition::reset_pending_tracking();
    // Issue #1062: `bluetooth_le::bridge::SLOT` / `OBSERVED` も同じ理由でプロセス static の
    // ため Activity 再生成をまたいで残る。新しい画面（`Idle`）を構築する前にここで捨てる
    // （`device::bluetooth_le::reset_pending_tracking` doc 参照）。Java 側に対になる no-op
    // スタブは置かない: Rust の `request_ble_scan` を経ないと Java が `nativeOnBleScan*`
    // を呼ぶ経路自体が無く、`feature = "device"` 無効ビルドでは到達しないため
    // （`speech_recognition`/`notification` と同じ理由）。
    #[cfg(feature = "device")]
    crate::device::bluetooth_le::reset_pending_tracking();
    // Issue #1006: `notification::bridge::SLOT` も同じ理由でプロセス static のため
    // Activity 再生成をまたいで残る。新しい画面 `NotificationSendState::default()`
    // （`Idle`）を構築する前にここで捨て、再生成前に開始した権限確認が後から完了
    // しても黙って失われず、以後確実に「未接続」（次の操作からやり直し）として
    // 扱われるようにする。Java 側（`MainActivity.onCreate`）に対になる no-op
    // スタブは置かない: Rust の `request_notification` を経ないと Java が
    // `nativeOnNotificationResult` を呼ぶ経路自体が無く、`feature = "device"`
    // 無効ビルドでは到達しないため（`location`/`mic_record` と同じ理由）
    // （`device::notification::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::notification::reset_pending_tracking();
    // Issue #1016: `drop_session::SESSIONS` も同じ理由でプロセス static のため
    // Activity 再生成をまたいで残る。進行中のセッション・未適用イベントをここで
    // 捨て、旧 Activity 由来の読み取りスレッドが `wants` で打ち切られるようにする
    // （`device::drop_session::reset` doc「Android 経路」参照）。`drop_session`
    // 自体は `feature = "device"` に関わらずコンパイルされる（`device.rs` の
    // モジュール doc 参照）が、Android での書き込み元（JNI 受け口）は
    // `device/android.rs` 側にあり feature 有効時のみ存在するため、reset もそれと
    // 揃えて feature 有効時のみ呼ぶ。
    #[cfg(feature = "device")]
    crate::device::drop_session::reset();
    // Issue #1104: `photo_pick::bridge::SLOT` もプロセス static のため再生成をまたいで残る。
    // 写真選択の契約は「保留中の要求は復元せず破棄」（`fandhe_device::photo_picker` モジュール
    // doc）なので、新しい `Idle` の画面状態を作る前に捨て、旧要求の遅延完了を id 不一致で
    // 無視させる（`device::photo_pick::bridge::reset` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::photo_pick::reset_pending_tracking();
    // Issue #1226: `calendar::bridge::SLOT` もプロセス static のため再生成をまたいで残る。
    // 保留中の要求は復元せず破棄し、旧要求の遅延完了を id 不一致で無視させる
    // （`device::calendar::reset_pending_tracking` doc 参照）。Java 側に no-op スタブは
    // 置かない（`request_calendar_insert` を経ないと `nativeOnCalendarResult` は呼ばれない）。
    #[cfg(feature = "device")]
    crate::device::calendar::reset_pending_tracking();
    // Issue #1254: `photo_save::bridge::SLOT` も同様。保留中の保存要求は復元せず破棄し、
    // 旧要求の遅延完了を id 不一致で無視させる（`device::photo_save::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::photo_save::reset_pending_tracking();
    // Issue #1360: `app_badge::bridge::SLOT` も同様に破棄する（`device::app_badge::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::app_badge::reset_pending_tracking();
    // Issue #1384: `compose_mail_sms::bridge::SLOT` も同様に破棄する（`device::compose_mail_sms::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::compose_mail_sms::reset_pending_tracking();
    // Issue #325: `print::bridge::SLOT` も同様に破棄する（Activity 再生成で印刷の保留は復元しない）。
    #[cfg(feature = "device")]
    crate::device::print::reset_pending_tracking();
    // Issue #1222: `contact_pick::bridge::SLOT` も同様。連絡先選択の契約は「復元せず破棄」
    // （`fandhe_device::contact_picker` モジュール doc）なので、旧要求の遅延完了を id 不一致で無視させる。
    #[cfg(feature = "device")]
    crate::device::contact_pick::reset_pending_tracking();
    // Issue #1400: MIDI の購読（Rust 側 epoch・sink）とドライバの処理済み世代も破棄する。画面状態は
    // 再生成で generation 0 に戻るため、残すと次の「受信開始」が処理済みとして飲み込まれる
    // （`device::midi::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::midi::reset_pending_tracking();
    // Issue #267: 音声再生も同様。画面状態が generation 0 に戻るため、処理済み世代を捨て再生を止める
    // （`device::media_playback::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::media_playback::reset_pending_tracking();
    // Issue #381: ロック画面・OS のメディア操作も同様。受付を止めて表示を取り下げ、記録を捨てる
    // （`device::now_playing::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::now_playing::reset_pending_tracking();
    // Issue #364: ピクチャ・イン・ピクチャも同様。処理済み世代を捨て、小窓を止める
    // （`device::picture_in_picture::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::picture_in_picture::reset_pending_tracking();
    // Issue #276: 通話 UI の記録（処理済み世代）と進行中の通話も破棄する。画面状態は再生成で
    // generation 0 に戻るため、残すと次の「着信を模擬する」が飲み込まれ、OS 側の着信も残る
    // （`device::call_ui::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::call_ui::reset_pending_tracking();
    // Issue #294: ライブアクティビティの保留中の要求も破棄し、表示中の通知を閉じる。再生成後の画面は
    // Idle から始まるため、残すと次の操作と OS 側の表示が食い違う
    // （`device::live_activity::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::live_activity::reset_pending_tracking();
    // Issue #292: 機械学習推論の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、
    // 残すと次の「推論する」が飲み込まれる（`device::ml_inference::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::ml_inference::reset_pending_tracking();
    // Issue #345: 端末内 LLM の処理済み世代も破棄する（理由は ml_inference と同じ。
    // `device::on_device_llm::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::on_device_llm::reset_pending_tracking();
    // Issue #311: 音声変換の処理済み世代も破棄する（理由は ml_inference と同じ。
    // `device::media_transcode::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    crate::device::media_transcode::reset_pending_tracking();

    let native_options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
                eframe::egui_wgpu::WgpuSetupCreateNew {
                    instance_descriptor: eframe::wgpu::InstanceDescriptor {
                        backends: android_wgpu_backends(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    // TASK-17.2（REQ-17）/ Issue #715: `FANDHE_PROXY_APP=<名前>` で描画対象を切り替える
    // （`main.rs`/`web.rs`/`ios.rs` と同じ契約、A03 対策・security.md 準拠の固定文字列
    // 比較のみ）。解決は `ios.rs` と同じく [`crate::screens::selected_app_name`] へ委ねる
    // （実行時 env → ビルド時 `option_env!` の順。Android は実行時に環境変数を注入できないため、
    // 実質はビルド時指定が効く経路になる。Issue #718 の PR レビュー指摘）。
    // `adb shell am start` は起動するアプリプロセスへ環境変数を注入できない
    // （Android に env var 注入の標準手段が存在しないことを実機検証で確認済み、
    // 検証記録は非公開）ため、`env::var` が
    // 空/失敗した場合は `debug.fandhe.proxy_app` システムプロパティ
    // （`adb shell setprop debug.fandhe.proxy_app <名前>` で設定可能）へフォールバックする
    // （[`read_debug_proxy_app_property`] 参照）。このフォールバックは
    // `debug-proxy-app` feature 有効時のみコンパイルされ、ストア提出物（feature 無効）
    // では常に空文字列となって既定経路へ落ちる（Issue #718、security.md A05）。
    // `tv-focus` は専用の `TvFocusApp` へ、それ以外はレジストリ（`crate::build_app`）
    // 経由で解決する。
    let requested_name =
        crate::screens::selected_app_name().unwrap_or_else(read_debug_proxy_app_property);
    let use_tv_focus = requested_name == "tv-focus";
    // codex-review 指摘 P2（Issue #831）: 傾き画面以外でのセンサ再描画を抑えるため、
    // 起動画面決定と同時にゲートを設定する（`ACTIVE_SCREEN_USES_TILT` doc 参照）。
    set_active_screen_uses_tilt(requested_name == "tilt_ball");
    // Bugbot 指摘（PR #939）: gamepad_show 以外の画面でのアナログイベント再描画を
    // 抑えるため、傾きと同じタイミングでゲートを設定する
    // （`ACTIVE_SCREEN_USES_GAMEPAD` doc 参照）。
    set_active_screen_uses_gamepad(requested_name == "gamepad_show");
    // Issue #1016: drag_drop 以外の画面が他画面宛てのドロップを受け取らないよう、
    // 傾き・ゲームパッドと同じタイミングでゲートを設定する
    // （`ACTIVE_SCREEN_USES_DRAG_DROP` doc 参照）。
    set_active_screen_uses_drag_drop(requested_name == "drag_drop");

    if let Err(err) = eframe::run_native(
        "fandhe-egui-proxy",
        native_options,
        Box::new(move |cc| {
            // Issue #831 codex-review 指摘: センサ JNI コールバックからの
            // `request_repaint` 経路確保のため static へ複製する（[`EGUI_CTX`] doc 参照）。
            // `Activity` 再生成のたびに上書きし、古い Context を残さない。
            *EGUI_CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(cc.egui_ctx.clone());
            let app: Box<dyn eframe::App> = if use_tv_focus {
                Box::new(
                    crate::TvFocusApp::new(cc).with_inset_margin_supplier(android_inset_margin),
                )
            } else if requested_name.is_empty() {
                Box::new(crate::ProxyApp::new(cc).with_inset_margin_supplier(android_inset_margin))
            } else {
                match crate::build_app(&requested_name, cc) {
                    Some(mut dyn_app) => {
                        dyn_app.set_inset_margin_supplier(Box::new(android_inset_margin));
                        dyn_app
                    }
                    None => {
                        eprintln!(
                            "[fandhe-egui-proxy] 未登録の画面名 '{requested_name}'、sample を表示する。"
                        );
                        Box::new(
                            crate::ProxyApp::new(cc)
                                .with_inset_margin_supplier(android_inset_margin),
                        )
                    }
                }
            };
            Ok(Box::new(SoftKeyboardApp { inner: app }))
        }),
    ) {
        // Android では標準エラー出力が logcat に直結しないため、本来は android_logger 等の
        // 追加導入が必要だが、ログ連携は対象外とする（#243）
        // （out-of-scope-tracking.md に基づき PR 側で報告する）。
        eprintln!("[fandhe-egui-proxy] Android 起動に失敗: {err:?}");
    }
}

/// [`android_main`] が組み立てる実描画 `Box<dyn eframe::App>` を包み、`update` の先頭で
/// [`poll_soft_keyboard`] を呼んでからフレーム本体（`inner`）に委譲する（Issue #796）。
/// `update` 以外の `eframe::App` メソッドは呼び出し元の各 App 型が上書きしていないため
/// （`ProxyApp`/`TvFocusApp`/レジストリ経由の型消去 App、いずれも `update` のみ実装）、
/// 委譲は `update` だけで足りる。
struct SoftKeyboardApp {
    inner: Box<dyn eframe::App>,
}

impl eframe::App for SoftKeyboardApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        poll_soft_keyboard(ctx);
        self.inner.update(ctx, frame);
    }
}

/// ソフトキーボードの状態バッファ（`AndroidApp::text_input_state()`）を毎フレーム
/// ポーリングし、前回との差分を egui の入力イベントへ変換して積む（Issue #796）。
///
/// winit の Android バックエンド（0.30.13）は `InputEvent::TextEvent`（GameTextInput の
/// コンポジション更新）を処理しないため、Gboard 等のソフトキーボードの入力が
/// `InputConnection` 経由で書き換える状態バッファは winit を経由しても egui へ届かない
/// （#660 実測）。
/// `AndroidApp::text_input_state()` は `take = false` の読み出しで winit が
/// `input_events_iter` を占有していても現在値をいつでも読めるため、本関数は winit の
/// イベントループとは独立に毎フレーム状態を読み、差分だけを egui イベントへ変換する
/// （変換ロジック本体は [`crate::soft_keyboard`] の純関数へ委譲）。
///
/// [`egui::Context::wants_keyboard_input`] が `false`（テキスト編集 UI にフォーカスなし）
/// の間はポーリングしない。フォーカス中の `egui::Id`（`ctx.memory(|m| m.focused())`）が
/// 前回と異なる場合（`None → Some` の新規フォーカスに加え、`Some(a) → Some(b)` の
/// 別欄への移動も含む）は、前のフォーカス先の入力文脈が Gboard 側に残らないよう
/// 状態バッファを明示的にリセットする。`wants_keyboard_input` の true/false 遷移だけを
/// 見ていた旧実装では、preedit 表示中に Tab 等で同じ「wants = true」のまま別の
/// テキスト欄へ移動するケースを検出できなかった（レビュー指摘）。
///
/// preedit 表示中に別欄へ移った場合、旧 preedit 自体は egui 側がフォーカス喪失を検知して
/// `ime_enabled = false` にする（egui 標準動作）ため、本関数側は次回のポーリングに向けて
/// `prev` を初期化するだけでよい（旧 preedit を閉じる `Ime` イベントを明示的に送る必要はない）。
///
/// 既知の制約: このリセットにより GameTextInput バッファは空になるため、既存テキストの
/// あるフィールドへフォーカスした直後に 1 回だけ Backspace を押すと、バッファ側には
/// 消せる文字が無く `committed_diff` が差分なしと判定してイベントを発火できない
/// （実機で要確認。Gboard は空バッファに対して `KEYCODE_DEL` の `KeyEvent` を送ることが
/// 多く、その場合は本関数ではなく winit の既存 `KeyEvent` 経路で届く想定だが、
/// 未検証のため記録は実機確認後に行う）。
fn poll_soft_keyboard(ctx: &egui::Context) {
    let Some(app) = android_app() else {
        return;
    };

    let wants = ctx.wants_keyboard_input();
    if !wants {
        // ロックは状態フィールドの書き換えのみに限定し、`app.*` 呼び出しはロック外で行う
        // （codex-review 指摘: ロック区間から FFI 越しの呼び出しを外す）。
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        state.focused = None;
        return;
    }

    // フォーカス先 Id の変化判定だけ先に済ませ、ロックは一旦解放してから
    // `app.set_text_input_state` / `app.text_input_state`（いずれも JNI 越しの呼び出し）を行う。
    let focused = ctx.memory(|m| m.focused());
    let just_focused = {
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        let changed = state.focused != focused;
        state.focused = focused;
        changed
    };
    if just_focused {
        app.set_text_input_state(
            winit::platform::android::activity::input::TextInputState::default(),
        );
    }

    let raw = app.text_input_state();
    let compose = raw.compose_region.map(|span| (span.start, span.end));
    let next = crate::soft_keyboard::snapshot(&raw.text, compose);

    let events = {
        let mut state = SOFT_KEYBOARD.lock().unwrap_or_else(|e| e.into_inner());
        if just_focused {
            state.prev = crate::soft_keyboard::KeyboardSnapshot::default();
        }
        let applied = crate::soft_keyboard::keyboard_events(&state.prev, &next);
        // `applied.prev` は defer 中かどうかに関わらず「次フレームの prev として
        // そのまま使うべき値」（`keyboard_events` の doc 参照）。呼び出し側で
        // defer を判定して更新有無を切り替える必要はない。
        state.prev = applied.prev;
        applied.events
    };
    if !events.is_empty() {
        ctx.input_mut(|i| i.events.extend(events));
    }

    // ponytail: 固定 20Hz ポーリング（TextEvent 到達をフックして起こす経路は未実装）。
    // 入力レイテンシが問題になれば android-activity の `TextEvent` キューを直接監視する
    // 経路に上げる。フォーカスが外れれば `wants` が false になり早期 return するため
    // 常時ポーリングにはならない。
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

/// [`poll_soft_keyboard`] が前回フレームとの差分検出に使う状態（未確定領域含む直前の
/// [`crate::soft_keyboard::KeyboardSnapshot`] と、直前フレームでフォーカスしていた
/// `egui::Id`。`None` はテキスト編集 UI へのフォーカスなしを表す）。
struct SoftKeyboardState {
    prev: crate::soft_keyboard::KeyboardSnapshot,
    focused: Option<egui::Id>,
}

static SOFT_KEYBOARD: std::sync::Mutex<SoftKeyboardState> =
    std::sync::Mutex::new(SoftKeyboardState {
        prev: crate::soft_keyboard::KeyboardSnapshot {
            committed: String::new(),
            compose: None,
        },
        focused: None,
    });

/// Java 側（`MainActivity.onCreate` の `OnApplyWindowInsetsListener`）から
/// JNI 経由で書き込まれる window insets（物理 px、systemBars | displayCutout）。
///
/// `AndroidApp::content_rect()` は GameActivity 経路（android-activity 0.6.1）で
/// insets を反映せず常に画面全体を返すことを実機（Pixel 10a / Android 16）で
/// 実測確認したため、Rust からは取得できない。`GameActivity_getWindowInsets` C API は
/// `GameActivity*` 構造体ポインタを要求するが、Rust 公開 API（`activity_as_ptr()` =
/// JNI jobject）からは得られないため、Java 側リスナー → JNI 経由の書き込みに一本化する
/// （PR #596 是正、Issue #579）。`Ordering::Relaxed` で十分だが、4 本を個別に
/// 読み書きするため「left/top/right/bottom の組としての原子性」は保証しない:
/// UI スレッドの書き込み中にレンダースレッドが読むと新旧混在の margin になり得る。
/// ずれは最大 1 フレームで、次フレームの読み出しで自己修正されるため許容する
/// （見た目の一瞬のズレのみで、メモリ安全性には影響しない）。
static INSET_LEFT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static INSET_TOP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static INSET_RIGHT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static INSET_BOTTOM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// `MainActivity.nativeOnWindowInsetsChanged`（Java 側）から呼ばれる JNI エクスポート関数。
/// シンボル名はパッケージ名 `com.fandhe.androidapp` とクラス名 `MainActivity` に
/// 固定で紐づく（JNI 命名規約。`platforms/android/app/.../MainActivity.java` 側のクラス移動・
/// rename 時は本関数名も追随が必要）。
///
/// `JNIEnv`/`jclass` は本関数内で一切使わず（受け取った値をそのまま static へ store するのみ）、
/// 生ポインタへのアクセスを行わないため関数本体に `unsafe` 操作は含まない。
/// `#[no_mangle]` はこの 1 関数に閉じ、`unsafe` を公開 API 側へ漏らさない
/// （`.claude/rules/coding-rust.md` の FFI 例外規定準拠）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` により Rust の名前修飾を
/// 無効化するが、シンボル名は JNI 命名規約上の固定名であり、本クレートは cdylib として
/// 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （`android_main`・`read_debug_proxy_app_property` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnWindowInsetsChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) {
    use std::sync::atomic::Ordering;
    INSET_LEFT.store(left, Ordering::Relaxed);
    INSET_TOP.store(top, Ordering::Relaxed);
    INSET_RIGHT.store(right, Ordering::Relaxed);
    INSET_BOTTOM.store(bottom, Ordering::Relaxed);
}

/// [`INSET_LEFT`] 等の static 4 本（Java 側リスナーが JNI 経由で書き込む物理 px）を読み、
/// [`crate::app`] の純関数 `insets_px_to_margin` へ橋渡しして、`CentralPanel` の
/// `inner_margin` へ加算する `egui::Margin`（論理 pt）を返す（Issue #579）。
///
/// [`crate::ProxyApp::with_inset_margin_supplier`] / [`crate::TvFocusApp::with_inset_margin_supplier`]
/// へ渡すクロージャから毎フレーム呼ばれる。static の読み出し自体は毎フレーム発生するが、
/// insets の値更新は OS 側リスナー（`ViewCompat.setOnApplyWindowInsetsListener`）の
/// 発火時のみ（イベント駆動）で、回転・折りたたみ等の変化にも次フレームで追従できる。
fn android_inset_margin(ctx: &egui::Context) -> egui::Margin {
    use std::sync::atomic::Ordering;
    let insets_px = (
        INSET_LEFT.load(Ordering::Relaxed) as f32,
        INSET_TOP.load(Ordering::Relaxed) as f32,
        INSET_RIGHT.load(Ordering::Relaxed) as f32,
        INSET_BOTTOM.load(Ordering::Relaxed) as f32,
    );
    crate::app::insets_px_to_margin(insets_px, ctx.pixels_per_point())
}

/// bionic の `__system_property_get` を最小 FFI で呼び、
/// `debug.fandhe.proxy_app` システムプロパティの現在値を返す（TASK-17.2）。
///
/// 呼び出し元: [`android_main`] の `FANDHE_PROXY_APP` 環境変数フォールバック
/// （上記コメント参照）。`adb shell setprop debug.fandhe.proxy_app tv-focus` で
/// 値を設定できる（`scripts/android/verify-tv-focus.sh` が使う）。プロパティ未設定・
/// 取得失敗時は空文字列を返す（`android_main` 側は空文字列を "tv-focus" と
/// 比較するだけで安全に既定経路（`ProxyApp`）へフォールバックする）。
///
/// # Safety
/// 本関数が含む `unsafe` 操作は下記 FFI 呼び出し 1 箇所のみで、それ以外の
/// `unsafe` 操作（生ポインタ経由の追加読み書き等）は行わない（根拠は下記
/// `// SAFETY:` コメント参照。`#[allow(unsafe_code)]` は `android_main` と同じ
/// FFI 例外規定（`.claude/rules/coding-rust.md`）に基づき本関数 1 箇所に閉じる）。
#[allow(unsafe_code)]
#[cfg(feature = "debug-proxy-app")]
fn read_debug_proxy_app_property() -> String {
    const PROPERTY_NAME: &[u8] = b"debug.fandhe.proxy_app\0";
    // bionic のプロパティ値は PROP_VALUE_MAX（92 バイト、NUL 終端込み）を超えない。
    let mut buf = [0u8; 92];

    // SAFETY: `PROPERTY_NAME` は NUL 終端済みの `'static` バイト列を指すポインタで、
    // `__system_property_get` はこれを読み取り専用の C 文字列として扱う（書き込まない）。
    // `buf` は 92 バイト（bionic が定義する PROP_VALUE_MAX の上限）確保済みで、
    // `__system_property_get` はこの上限を超えて書き込まない契約（bionic
    // `<sys/system_properties.h>` の仕様）。戻り値は書き込んだバイト数（NUL 終端含まず、
    // 負値なし）であり、`buf` の範囲外を読み書きすることはない。
    // 注意: この 92 バイト仮定はレガシー API `__system_property_get` 固有の
    // 後方互換 ABI 契約（長い値は切り詰められる）であり、可変長対応の代替 API
    // （`__system_property_read_callback` 等）へは流用不可。
    let len = unsafe {
        libc::__system_property_get(PROPERTY_NAME.as_ptr().cast(), buf.as_mut_ptr().cast())
    };

    if len <= 0 {
        return String::new();
    }
    let len = usize::try_from(len).unwrap_or(0).min(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// `debug-proxy-app` feature 無効時（ストア提出物を含む既定ビルド）の
/// [`read_debug_proxy_app_property`] 代替。システムプロパティを一切読まず空文字列を返し、
/// [`android_main`] を既定経路（レジストリ解決 / `ProxyApp`）へ落とす（Issue #718）。
#[cfg(not(feature = "debug-proxy-app"))]
fn read_debug_proxy_app_property() -> String {
    String::new()
}

/// `device` feature 無効ビルド（`--no-default-features` 等）向けの
/// `nativeOnMotionChanged` no-op フォールバック（codex-review 指摘、Issue #831）。
///
/// `MainActivity.java` の `motionListener` は feature 構成に関わらず常にセンサを
/// 登録して `nativeOnMotionChanged` を呼ぶ（Java 側はビルド feature を知らない）。
/// 本来の実装（`crate::device::android::Java_..._nativeOnMotionChanged`）は
/// `device` feature 内にしかないため、feature 無効ビルドではこのシンボルが
/// 未定義のままリンクされ、初回センサ通知時に `UnsatisfiedLinkError` で
/// クラッシュする。ここで同名シンボルを feature 無効時のみ定義し、値を捨てるだけの
/// no-op にして落ちないようにする（feature 有効時は `device/android.rs` 側の定義と
/// 重複しないよう `cfg(not(feature = "device"))` で排他にする）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnWindowInsetsChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnMotionChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _x: f32,
    _y: f32,
    _rotation: i32,
) {
}

/// `device` feature 無効ビルド向けの `nativeOnUrlSchemeReceived` no-op フォールバック
/// （Issue #1218）。`MainActivity.java` は feature を知らず `onCreate` / `onNewIntent` から
/// 必ず呼ぶため、スタブが無いと feature 無効ビルドで `UnsatisfiedLinkError` になる。
/// 引数の並びは Java 宣言・`device/android_url_scheme.rs` の実体と 3 か所で一致させる。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnMotionChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnUrlSchemeReceived(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _url: *mut core::ffi::c_void,
) {
}

/// `device` feature 無効ビルド向けの `nativeAccepts` no-op（常に true で読み取りへ進め、
/// 後続の `nativeOnFileOpened` no-op が fd を閉じる）。引数は Java 宣言・実体と一致させる。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FileAssociationHelper_nativeAccepts(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _name: *mut core::ffi::c_void,
    _mime: *mut core::ffi::c_void,
) -> u8 {
    1
}

/// `device` feature 無効ビルド向けの `nativeOnFileOpened` no-op フォールバック（Issue #418）。
/// `FileAssociationHelper` は feature を知らず `ACTION_VIEW` の `content:` で必ず呼ぶため、
/// スタブが無いと `UnsatisfiedLinkError` になる。detach 済みの fd は `OwnedFd` で包んで drop し
/// close する（漏らさない）。引数の並びは Java 宣言・`device/android_file_association.rs` と一致させる。
///
/// # Safety
/// `fd >= 0` は Java 側 `detachFd()` 済みの有効な fd で、他に渡さないため二重 close は無い。
/// `#[no_mangle]` は JNI 命名規約上の固定シンボル名（`nativeOnMotionChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FileAssociationHelper_nativeOnFileOpened(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _code: i32,
    _name: *mut core::ffi::c_void,
    _mime: *mut core::ffi::c_void,
    fd: i32,
) {
    use std::os::fd::{FromRawFd, OwnedFd};
    if fd >= 0 {
        // SAFETY: 上の Safety 節のとおり。
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }
}

// `nativeOnLocationChanged`/`nativeOnLocationFailure`（Issue #890）は上記
// `nativeOnMotionChanged` と異なり feature 無効ビルド向けの no-op フォールバックを
// 追加しない。`motionListener` は `onResume` で無条件にセンサ登録するため Java 側は
// ビルド feature を問わず必ず呼ぶが、位置情報は Rust 側 `request_location_updates()`
// （feature `device` 内のみ存在、`device/location.rs::shared_location` から呼ばれる）
// の呼び出しを経て初めて `locationWanted` が真になり、Java 側もそれより前に
// `nativeOnLocationChanged`/`nativeOnLocationFailure` を呼ぶ経路を持たない。
// よって feature 無効ビルドではこれらのシンボルへ到達しえず、
// `UnsatisfiedLinkError` の心配がない（codex-review の同種再指摘の予防）。

/// `device` feature 無効ビルド向けの `nativeOnGamepadPresenceScanBegin`/`ScanId`/`ScanEnd`/
/// `nativeOnGamepadMotion`/`nativeOnGamepadKey` no-op フォールバック（Issue #935）。
/// `MainActivity` の `reportGamepadPresence()`/`dispatchGenericMotionEvent`/`dispatchKeyEvent`
/// はビルド feature を知らずゲームパッド由来のイベントであれば常にこれらを呼ぶため、
/// `nativeOnMotionChanged` と同じ理由（feature 無効ビルドでの `UnsatisfiedLinkError`
/// 防止）で値を捨てるだけの no-op を用意する（feature 有効時は
/// `device/android_gamepad.rs` 側の定義と重複しないよう `cfg(not(feature =
/// "device"))` で排他にする）。`ScanBegin`/`ScanId`/`ScanEnd` の 3 分割・シグネチャは
/// `device/android_gamepad.rs` 側の実装と必ず一致させる（JNI は署名不一致を実行時にしか
/// 検出しないため、PR #939 で `ScanId` から `controllerNumber` 引数を削った際も両側を
/// 揃えた）。
///
/// # Safety
/// この 5 関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 5 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnMotionChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
}

/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ根拠の no-op。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanId(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _device_id: i32,
) {
}

/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ根拠の no-op。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanEnd(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
}

/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ根拠の no-op。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code, clippy::too_many_arguments)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadMotion(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _device_id: i32,
    _source: i32,
    _x: f32,
    _y: f32,
    _z: f32,
    _rz: f32,
    _left_trigger: f32,
    _right_trigger: f32,
    _hat_x: f32,
    _hat_y: f32,
    _flat_x: f32,
    _flat_y: f32,
    _flat_z: f32,
    _flat_rz: f32,
) {
}

/// [`Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadPresenceScanBegin`] と同じ根拠の no-op。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeOnGamepadKey(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _device_id: i32,
    _source: i32,
    _key_code: i32,
    _down: u8,
) {
}

/// `MainActivity.java` の `dragListener`（`View.OnDragListener`）が
/// `ACTION_DRAG_STARTED` のたびに呼ぶ同期 getter（Issue #1016 実装計画 §3-2）。
/// 現在表示中の画面が `drag_drop` かどうかを返し、`false` ならこのドラッグの
/// 以降のイベント（`ACTION_DRAG_ENTERED`/`_EXITED`/`_DROP`/`_ENDED`）を Java 側は
/// Rust へ一切送らない（契約表 1「画面外イベントを破棄する」）。
///
/// 他の JNI 受け口（`nativeOnMotionChanged`/`nativeOnGamepad*`）と異なり、`device`
/// feature 無効ビルド向けの no-op スタブを別途置かない: 本関数を feature に関わらず
/// ここへ 1 つだけ定義し、返す値に `cfg!(feature = "device")` を織り込むことで
/// 二重定義を避ける（`device/android.rs` は `feature = "device"` のときのみ
/// コンパイルされるため、同名シンボルをそちらに置くと feature 無効ビルドで
/// `UnsatisfiedLinkError` になる。実装計画 §3-2「スタブを置かない理由」）。
/// `false` を返すこの経路のおかげで、`nativeOnDragDropHover`/`Begin`/`Wants`/`Item`
/// （`device/android.rs`、feature 有効時のみ存在）は feature 無効ビルドでは
/// Java 側から一切呼ばれず、対になる no-op スタブが不要になる。
///
/// jni クレートの型を使わず生ポインタ/プリミティブで受ける（`nativeOnMotionChanged`
/// no-op フォールバックと同じ理由: 本ファイルは `device` feature 無効時も常に
/// コンパイルされるため、`jni` クレートへの依存を増やさない）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnMotionChanged` と同じ根拠）。
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_MainActivity_nativeDragDropIsActive(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) -> u8 {
    u8::from(cfg!(feature = "device") && active_screen_uses_drag_drop())
}

/// `device` feature 無効ビルド向けの `nativeOnFullscreenChanged` no-op フォールバック
/// （Issue #1314）。`FullscreenHelper` は insets リスナーから必ず呼ぶため、スタブが無いと
/// feature 無効ビルドで `UnsatisfiedLinkError` になる。JNI 署名は Java 宣言・
/// `device/android_fullscreen_immersive.rs` の実体と 3 か所で `(int) -> void` に揃える。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnMotionChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FullscreenHelper_nativeOnFullscreenChanged(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _state: i32,
) {
}

/// `device` feature 無効ビルド向けの `nativeOnPenSample` no-op フォールバック（Issue #357）。
/// `PenStylusHelper` は画面に関係なくタッチのたびに呼ぶため、スタブが無いと
/// `UnsatisfiedLinkError` になる。JNI 署名は Java 宣言・`device/android_pen_stylus.rs` の実体と
/// 3 か所で `(int, float, float, boolean) -> void` に揃える。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名（`nativeOnMotionChanged` と同じ根拠）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PenStylusHelper_nativeOnPenSample(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _tool_type: i32,
    _pressure: f32,
    _tilt_rad: f32,
    _has_tilt: u8,
) {
}

/// `device` feature 無効ビルド向けの `nativeOnPenClear` no-op フォールバック（Issue #357）。
///
/// # Safety
/// `nativeOnPenSample` の no-op と同じ（unsafe 操作なし、固定シンボル名）。
#[cfg(not(feature = "device"))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_PenStylusHelper_nativeOnPenClear(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-1（Android = Vulkan）の充足を機械的に検証する。
    /// 実機起動の確認自体は TASK-1.6（#230、人間担当）のスコープであり、
    /// ここでは初期化コードが Vulkan 以外へフォールバックしないことのみを保証する。
    /// `frame-metrics` feature 有効時は `android_wgpu_backends` 自体が
    /// コンパイル対象外になるため（上記 cfg 参照）、本テストも同様に除外する。
    #[cfg(not(feature = "frame-metrics"))]
    #[test]
    fn vulkan_backend_is_fixed() {
        assert_eq!(android_wgpu_backends(), eframe::wgpu::Backends::VULKAN);
    }

    /// `read_debug_proxy_app_property` はエミュレータ実機での
    /// `adb shell setprop` 前提の関数であり、ホスト側単体テストでは値の内容を
    /// 検証できない（実機検証は `scripts/android/verify-tv-focus.sh`、TASK-17.2）。
    /// ここでは panic せず戻り値を返すことのみを最小確認する（`cargo ndk test` で
    /// Android ターゲット向けにコンパイル・実行できることの確認を兼ねる）。
    /// `debug-proxy-app` feature 無効時は FFI を含まないスタブ（空文字列）が対象になる
    /// （Issue #718）。FFI 版の確認には `--features debug-proxy-app` が要る。
    #[test]
    fn read_debug_proxy_app_property_does_not_panic() {
        let _ = read_debug_proxy_app_property();
    }
}
