//! Phase 11 の拡充ループ（親 #887）型サンプル「位置情報」（Issue #888、Android は #890）の
//! effect ドライバ。`fandhe_core::screens::location_show`（端末依存ゼロ）へ、
//! `fandhe_device::location::SharedLocation` から読んだ最新の測位結果を渡す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # 実装スコープ（#888/#889/#890）
//!
//! Windows は [`shared_location`] が初回アクセス時に [`fandhe_device::location::CommandLocation`]
//! を起動し、書き込み先として `SHARED` を渡す（`tilt.rs` の `DummyMotion` と異なり、
//! 本モジュールでは書き手も本ファイル側で確保する）。Android は [`shared_location`] が
//! 初回アクセス時に [`crate::device::android::request_location_updates`] を 1 度だけ
//! 呼び、以後の実測位は Java 側 `LocationListener` → `nativeOnLocationChanged`/
//! `nativeOnLocationFailure` が [`shared_location_sink`] へ直接 push する
//! （継続 push 型のため Windows のような「本ファイル側が子プロセスを起動して push する」
//! 形にはならない）。
//!
//! iOS / macOS（Apple、Issue #889）は [`location_effect`] が初回呼び出し時に
//! `device::apple_location::start()` を呼ぶ（CoreLocation の `CLLocationManager` は
//! 生成したスレッドの run loop を要求するため、egui の update ループ＝main スレッドで
//! 呼ぶ必要があり、Windows のようにドライバ自体を子プロセスで起動する構成が
//! 取れない。`shared_location()` からは起動しない: `cargo test`/clippy がテストの
//! 一部として `shared_location()` を呼ぶと `CLLocationManager` を生成し TCC
//! プロンプトを出す事故になるため）。「再試行」ボタン押下時は `reset_to_waiting()`
//! してから `start()` を再度呼ぶ（`.m` 側は冪等で現在の権限状態を再評価して
//! 再 push する。`.m` ファイル冒頭コメント参照）。
//!
//! Windows/Android/iOS/macOS 以外（実質 Linux・Web）は [`shared_location`] が
//! 初回アクセス時に `push_failure(LocationFailure::Unsupported)` を 1 回入れて
//! 「使えない」表示にする（`device/clipboard.rs::shared_clipboard` の cfg
//! カスケードと同型。ここではダミー座標へフォールバックしない）。

#[cfg(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
))]
use std::sync::Mutex;
use std::sync::OnceLock;

use fandhe_core::screens::location_show::{apply_failure, apply_fix, apply_waiting, FailureKind};
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
use fandhe_device::location::LocationFailure;
use fandhe_device::location::{Location, SharedLocation};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `drive` が変化無しのときに再描画予約する間隔（`tilt.rs` の
/// `IOS_IDLE_POLL_INTERVAL` と同種の定期起床。測位は数秒間隔のため短すぎる
/// ポーリングは不要）。
pub(super) const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// プロセス内で共有する測位状態（読み手・書き手の橋渡し）。`device/apple_location.rs`
/// からも push 元として参照するため `pub(super)`（Issue #889）。
pub(super) static SHARED: OnceLock<SharedLocation> = OnceLock::new();

/// Windows のみ生存する子プロセス駆動の書き手（`SHARED` への push 元）。
/// 他ターゲットでは常に `None` のまま。`OnceLock` ではなく `Mutex` で保持する
/// （denied/disabled 後・予期しない終了後の「再試行」でドライバごと差し替えるため。
/// codex-review 指摘: `OnceLock` 固定だと初回失敗後は二度と再起動できない）。
#[cfg(target_os = "windows")]
static WINDOWS_DRIVER: Mutex<Option<fandhe_device::location::CommandLocation>> = Mutex::new(None);

/// `location_effect` が最後に処理した `state.retry_seq`（Windows/Android/Apple のみ）。
/// 「再試行」ボタン押下（[`fandhe_core::screens::location_show::retry`] による世代番号の
/// 加算）を検知する専用シグナルとして使う（`location_effect` の doc 参照。初期値は
/// `LocationShowState::default()` と同じ `0`）。表示状態（`view`）の遷移では判定しない:
/// OS 起点の回復通知（Android `onProviderEnabled`/`onResume` 経由の
/// `nativeOnLocationRetrying` 等）は `apply_waiting` を呼ぶだけで世代番号を進めないため、
/// 表示だけを見る旧方式（`is_retry_request`、削除済み）だと OS 起点の回復と利用者操作を
/// 区別できず、取得済みの fix を誤って破棄する経路があった
/// （codex-review P1 指摘、PR #894）。
#[cfg(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
))]
static LAST_PROCESSED_RETRY_SEQ: Mutex<u64> = Mutex::new(0);

/// Android 専用: [`crate::device::android::request_location_updates`] を初回のみ
/// 呼んだかどうか（Issue #890）。JNI 呼び出し自体の成否だけを扱う（実測位・失敗の
/// 通知は Java 側コールバックが [`shared_location_sink`] へ非同期に push する）。
#[cfg(target_os = "android")]
static ANDROID_STARTED: OnceLock<()> = OnceLock::new();

/// Android 専用: 精密位置（FINE）つきの開始要求を出したかどうか。位置情報画面が先に測位を
/// 始めていても、ジオフェンス画面の初回アクセスで FINE への昇格要求を 1 度だけ出す。
#[cfg(target_os = "android")]
static ANDROID_PRECISE_STARTED: OnceLock<()> = OnceLock::new();

/// Android 専用: [`LAST_PROCESSED_RETRY_SEQ`] を `0` へ戻す（`android_main` から
/// `Activity` 再生成のたびに呼ぶ契約、`android.rs::set_android_app` doc と同じ
/// 「再生成のたびに古い値を残さない」考え方）。
///
/// `LocationShowState`（`retry_seq` を含む）はプロセスの `static` と異なり `Activity`
/// 再生成のたびに既定値（`retry_seq: 0`）へ作り直されるが、[`LAST_PROCESSED_RETRY_SEQ`]
/// はプロセス生存中残り続ける。これを放置すると、再生成前に「再試行」を複数回押して
/// 世代番号が進んでいた場合、再生成後の新しい画面（`retry_seq` は `0` から再スタート）で
/// 利用者が改めて「再試行」を押しても、[`LAST_PROCESSED_RETRY_SEQ`] に残った大きい値を
/// 下回ったままとなり、再要求が一切発火しなくなる。呼び出し元（`android_main`）が
/// 新しい画面 `S` を構築する前に本関数で基準値を作り直しておけば、両者は必ず `0` から
/// 揃う。
#[cfg(target_os = "android")]
pub(crate) fn reset_retry_tracking() {
    let mut last_seq = LAST_PROCESSED_RETRY_SEQ
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    *last_seq = 0;
}

/// Apple（iOS/macOS）で `device::apple_location::start()` を初回呼び出し済みかどうか
/// （Issue #889。`location_effect` から呼ばれる）。
#[cfg(any(target_os = "ios", target_os = "macos"))]
static APPLE_STARTED: OnceLock<()> = OnceLock::new();

/// Windows・Android・Apple（iOS/macOS）以外で「使えない」を 1 度だけ push したかどうか。
#[cfg(not(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
)))]
static UNSUPPORTED_PUSHED: OnceLock<()> = OnceLock::new();

/// 共有 `SharedLocation` への参照を返す（Windows のみ未初期化時に書き手を起動・
/// 登録する。Apple は `location_effect` 側が起動する、モジュール doc 参照）。
/// `registry.rs` の `effect` 区画・`device.rs::shutdown` から参照される公開入口。
pub fn shared_location() -> &'static SharedLocation {
    shared_location_for(false)
}

/// [`shared_location`] のジオフェンス用。Android で精密位置（FINE）の許可を求める
/// （位置情報画面は COARSE のみ、最小権限）。他 OS では [`shared_location`] と同じ。
pub(super) fn shared_location_precise() -> &'static SharedLocation {
    shared_location_for(true)
}

fn shared_location_for(_precise: bool) -> &'static SharedLocation {
    let shared = SHARED.get_or_init(SharedLocation::new);

    #[cfg(target_os = "windows")]
    ensure_windows_driver(shared, false);
    #[cfg(target_os = "android")]
    {
        // JNI 呼び出し自体が失敗（メソッド未検出・例外送出等）した場合のみここで
        // 失敗を反映する。呼び出し成功後の実測位・失敗は Java 側コールバックが
        // 別途 `shared_location_sink()` へ push する（本関数はそれを待たない）。
        if _precise {
            ANDROID_PRECISE_STARTED.get_or_init(|| {
                let _ = ANDROID_STARTED.set(());
                if !crate::device::android::request_location_updates(true) {
                    shared.push_failure(LocationFailure::Failed);
                }
            });
        } else {
            ANDROID_STARTED.get_or_init(|| {
                if !crate::device::android::request_location_updates(false) {
                    shared.push_failure(LocationFailure::Failed);
                }
            });
        }
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "android",
        target_os = "ios",
        target_os = "macos"
    )))]
    {
        UNSUPPORTED_PUSHED.get_or_init(|| {
            shared.push_failure(LocationFailure::Unsupported);
        });
    }

    shared
}

/// Android の JNI コールバック（`crate::device::android` の `nativeOnLocationChanged`/
/// `nativeOnLocationFailure`）専用の入口。[`shared_location`] と異なりドライバ起動処理
/// （`ANDROID_STARTED` の初期化）を経ない（コールバックから `shared_location()` を呼ぶと
/// 初期化処理が再入してしまうため分離する）。`SHARED` 自体は `shared_location()` と
/// 共有する（`OnceLock::get_or_init` は初期化順に関わらず同じインスタンスを返す）。
#[cfg(target_os = "android")]
pub(crate) fn shared_location_sink() -> &'static SharedLocation {
    SHARED.get_or_init(SharedLocation::new)
}

/// Windows 専用: 測位ドライバを起動・必要なら再起動する。未起動（`None`）、または
/// `force` が真（「再試行」で `location_effect` から呼ばれる）なら
/// [`fandhe_device::location::CommandLocation::start`] を呼び直す。既存ドライバは
/// `Drop`（内部で `stop()`）により子プロセス・読み取りスレッドを回収してから
/// 差し替える。`force` 時は新ドライバが初回結果を出す前に画面が旧結果へ
/// 一瞬戻らないよう、先に共有状態を「取得中」へ戻す。
#[cfg(target_os = "windows")]
fn ensure_windows_driver(shared: &SharedLocation, force: bool) {
    let mut slot = WINDOWS_DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    if slot.is_some() && !force {
        return;
    }
    *slot = None;
    if force {
        shared.reset_to_waiting();
    }
    match fandhe_device::location::CommandLocation::start(shared.clone()) {
        Ok(driver) => *slot = Some(driver),
        Err(_) => {
            // 起動不能（unpackaged PowerShell から Geolocator に到達できない等）は
            // 握り潰さず失敗として共有状態へ反映する（実装計画 §3.3 のリスク対応）。
            shared.push_failure(LocationFailure::Unsupported);
        }
    }
}

/// 測位を明示的に再起動する（location 画面の「再試行」と geofence 画面の再取得が共有する入口）。
/// 古い失敗・座標を保持したまま再起動すると、新しい結果が出る前に画面が旧結果へ一瞬戻るため、
/// 先に「取得中」へ倒してから各 OS の起動経路を呼ぶ。Apple は main スレッド（egui の update
/// ループ）から呼ぶこと。対象外の OS では何もしない。
#[cfg(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
))]
pub(super) fn restart_location(shared: &SharedLocation, _precise: bool) {
    #[cfg(target_os = "windows")]
    ensure_windows_driver(shared, true);
    #[cfg(target_os = "android")]
    {
        shared.reset_to_waiting();
        if !crate::device::android::request_location_updates(_precise) {
            shared.push_failure(LocationFailure::Failed);
        }
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        shared.reset_to_waiting();
        crate::device::apple_location::start();
    }
}

/// アプリ終了時に Windows の測位子プロセスを止める（`crate::device::shutdown` →
/// `ProxyApp::on_exit` から呼ばれる契約。`static` は `Drop` が走らないため、
/// 明示停止が要る、`device/tts.rs::stop_shared_tts` と同じ理由）。
pub fn stop_shared_location() {
    #[cfg(target_os = "windows")]
    if let Some(driver) = WINDOWS_DRIVER
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        driver.stop();
    }
}

/// Apple 専用: `apple_location::start()` を初回だけ呼ぶ。初回なら真。location と geofence の
/// 両画面から呼ばれる（`CLLocationManager` は main スレッド = egui の update ループで起動する）。
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(super) fn start_apple_location_once() -> bool {
    let first = APPLE_STARTED.set(()).is_ok();
    if first {
        crate::device::apple_location::start();
    }
    first
}

/// `fandhe_device::DeviceError` を画面の [`FailureKind`] へ写す（`fandhe-core` は
/// `fandhe-device` に依存しないため、両者の型を橋渡しするのは本クレート側の責務、
/// `device.rs::to_note_storage_error` と同じ判断）。
pub(super) fn to_failure_kind(e: &DeviceError) -> FailureKind {
    match e {
        DeviceError::PermissionDenied(_) => FailureKind::PermissionDenied,
        DeviceError::Disabled(_) => FailureKind::ServiceDisabled,
        DeviceError::Unsupported(_) => FailureKind::Unsupported,
        // タイムアウト・その他の内部要因は再試行の余地がある扱いにする
        // （`fandhe_device::location` モジュール doc の対応表: Timeout/Failed → Internal）。
        _ => FailureKind::Retryable,
    }
}

/// 現在の表示状態が失敗系（`Denied`/`Disabled`/`Unavailable`/`Retryable`）なら、
/// 対応する [`FailureKind`] を返す（`drive` の変化検知が `Err` 系を `Ok` 系と
/// 同列に「変化なしなら再ポーリングのみ」で扱うための逆写像）。
fn view_failure_kind(
    view: fandhe_core::screens::location_show::LocationView,
) -> Option<FailureKind> {
    use fandhe_core::screens::location_show::LocationView;
    match view {
        LocationView::Denied => Some(FailureKind::PermissionDenied),
        LocationView::Disabled => Some(FailureKind::ServiceDisabled),
        LocationView::Unavailable => Some(FailureKind::Unsupported),
        LocationView::Retryable => Some(FailureKind::Retryable),
        LocationView::Waiting | LocationView::Fixed { .. } => None,
    }
}

/// 直前・今回の `state.retry_seq` から「再試行ボタン押下」を判定する純関数（`cfg` 無しで
/// 全ターゲットからテストできる、`drive` と同じ切り出し方針）。表示状態（`view`）の
/// 遷移では判定しない。`view` は測位の実結果（OS 起点の回復通知を含む）でも非同期に
/// 変わるため、「表示が失敗から `Waiting` に戻った」だけでは利用者操作と OS 起点の
/// 回復を区別できない。例えばサービス無効の表示中に `onProviderEnabled` が
/// `nativeOnLocationRetrying` を呼んで共有状態が `Waiting` になり、`drive` がそれを
/// 読んで `apply_waiting` をキューへ送った直後に新しい fix が JNI から届くと、次フレームで
/// `apply_waiting` 適用後の `view` 遷移だけを見る判定は「再試行」と誤認し、
/// 取得済みの fix を [`SharedLocation::reset_to_waiting`] で破棄してしまう
/// （codex-review P1 指摘、PR #894）。[`fandhe_core::screens::location_show::retry`]
/// のみが加算する世代番号 [`fandhe_core::screens::location_show::LocationShowState::retry_seq`]
/// は利用者の明示操作でしか進まないため、この誤検知経路が構造的に存在しない。
#[cfg_attr(
    not(any(
        target_os = "windows",
        target_os = "android",
        target_os = "ios",
        target_os = "macos"
    )),
    allow(dead_code)
)]
fn is_new_retry(current_seq: u64, last_processed_seq: u64) -> bool {
    current_seq > last_processed_seq
}

/// `location` から最新の測位結果を読み、変化があれば完了を送る（テストから
/// プラットフォーム cfg 抜きで検証できるよう `location_effect` 本体から切り出す）。
fn drive(
    state: &fandhe_core::screens::location_show::LocationShowState,
    sender: &CompletionSender<fandhe_core::screens::location_show::LocationShowState>,
    location: &dyn Location,
) {
    let next = location.last_fix();

    // 現在の表示と実質的に変わらないなら送信をスキップし、egui を無駄に再描画させない
    // （`device/tilt.rs::can_skip_send` と同じ考え方）。`Waiting`・`Fixed`（同一座標）・
    // 失敗状態（同一種別）のいずれも、変化が無い間は `request_repaint_after` で
    // `POLL_INTERVAL` 後に再確認するだけにとどめる。失敗状態をここに含めないと
    // `send`（即時 `request_repaint`）が毎フレーム呼ばれ続け、測位に失敗している間
    // フルフレームレートで張り付く（cursor[bot] 指摘、L111-132）。逆に `Fixed` の
    // 分岐だけ再ポーリングを予約し忘れると、初回 fix 到達後に後続の値を読みに
    // 戻らなくなる（同 L119-125）。
    use fandhe_core::screens::location_show::LocationView;
    let unchanged = match &next {
        Ok(None) => matches!(state.view, LocationView::Waiting),
        Ok(Some(fix)) => matches!(
            state.view,
            LocationView::Fixed { lat, lon, accuracy_m }
                if fix.latitude_deg == lat && fix.longitude_deg == lon && fix.accuracy_m == accuracy_m
        ),
        Err(e) => view_failure_kind(state.view) == Some(to_failure_kind(e)),
    };

    if unchanged {
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }

    let _ = sender.send(Box::new(move |s| match next {
        Ok(Some(fix)) => apply_fix(s, fix.latitude_deg, fix.longitude_deg, fix.accuracy_m),
        Ok(None) => apply_waiting(s),
        Err(e) => apply_failure(s, to_failure_kind(&e)),
    }));
}

/// `registry.rs` の `effect` 区画から `location_show` 画面用に配線される effect ドライバ。
///
/// `retry`（`location_show::retry`）は画面状態を `Waiting` へ戻し `retry_seq` を進める
/// だけで、実際の測位再開は本関数が担う契約（`fandhe-core` は端末依存ゼロのため）。
/// 「再試行ボタン押下」の検知には `state.view` の遷移を使わず、直前呼び出しで処理済みの
/// `retry_seq`（[`LAST_PROCESSED_RETRY_SEQ`]）より今回の `state.retry_seq` が
/// 大きいことだけを条件にする（[`is_new_retry`] の doc 参照）。OS 起点の回復通知
/// （Android `onProviderEnabled`/`onResume` 等）は世代番号を進めないため、これだけで
/// 「利用者が明示的に retry を押した」ときにのみ再要求・共有状態のリセットが走る
/// （codex-review P1 指摘、PR #894）。
pub fn location_effect(
    state: &fandhe_core::screens::location_show::LocationShowState,
    sender: &CompletionSender<fandhe_core::screens::location_show::LocationShowState>,
) {
    let shared = shared_location();
    #[cfg(any(target_os = "windows", target_os = "android"))]
    {
        let previous_seq = {
            let mut last_seq = LAST_PROCESSED_RETRY_SEQ
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut *last_seq, state.retry_seq)
        };
        if is_new_retry(state.retry_seq, previous_seq) {
            restart_location(shared, false);
        }
    }
    // Apple（iOS/macOS、Issue #889）: `CLLocationManager` は生成したスレッドの run loop
    // を要求するため、egui の update ループ（main スレッド）から呼ばれる本関数で起動する
    // （モジュール doc 参照。Windows のように `shared_location()` 側では起動しない）。
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        let previous_seq = {
            let mut last_seq = LAST_PROCESSED_RETRY_SEQ
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut *last_seq, state.retry_seq)
        };
        // 初回起動のフレームでは再試行判定を行わない（従来の if / else if と同じ）。
        if !start_apple_location_once() && is_new_retry(state.retry_seq, previous_seq) {
            restart_location(shared, false);
        }
    }
    drive(state, sender, shared);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::location_show::LocationShowState;
    use fandhe_device::location::{Fix, LocationFailure};

    fn sender_with_channel() -> (
        CompletionSender<LocationShowState>,
        std::sync::mpsc::Receiver<crate::app::Completion<LocationShowState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[test]
    fn is_new_retry_true_only_when_seq_increases() {
        // 「再試行」ボタン押下（retry_seq の加算）でのみ真になる（codex-review P1
        // 指摘、PR #894）。
        assert!(is_new_retry(1, 0));
        assert!(is_new_retry(5, 4));
    }

    #[test]
    fn is_new_retry_false_when_seq_unchanged_or_smaller() {
        assert!(!is_new_retry(0, 0));
        assert!(!is_new_retry(3, 3));
        assert!(!is_new_retry(2, 3));
    }

    #[test]
    fn retry_signal_ignores_async_view_recovery_without_seq_increase() {
        // モジュール doc: OS 起点の回復通知（例: Android onProviderEnabled が
        // nativeOnLocationRetrying を呼ぶ）は表示を Waiting へ戻すだけで retry_seq を
        // 進めない。取得済みの fix が届いた直後に表示が Fixed→Waiting とすれ違っても、
        // seq 比較のみを見る判定は再要求を出さない（codex-review P1 指摘、PR #894）。
        let mut last_processed = 0u64;
        for _ in 0..3 {
            // 複数フレームにわたり seq は動かない（OS 起点の回復通知のみが挟まる想定）。
            let current_seq = 0u64;
            let previous = std::mem::replace(&mut last_processed, current_seq);
            assert!(!is_new_retry(current_seq, previous));
        }
    }

    #[test]
    fn retry_signal_fires_only_on_frames_where_button_advances_seq() {
        // ボタン押下（retry_seq 加算）のフレームだけ true、それ以外は false。
        let mut last_processed = 0u64;
        let frames = [0u64, 0, 1, 1, 1, 2];
        let expected = [false, false, true, false, false, true];
        for (current_seq, want) in frames.into_iter().zip(expected) {
            let previous = std::mem::replace(&mut last_processed, current_seq);
            assert_eq!(is_new_retry(current_seq, previous), want);
        }
    }

    #[test]
    fn retry_signal_does_not_fire_right_after_activity_recreation_reset() {
        // Activity 再生成直後は reset_retry_tracking が last_processed を 0 へ戻し、
        // 新しい画面状態の retry_seq も既定値 0 から再スタートする
        // （reset_retry_tracking の doc 参照）。両者が揃った直後の呼び出しで
        // 誤って再要求しない。
        // 再生成前に last_processed = 5（複数回「再試行」を押していた想定）まで進んで
        // いたとしても、`reset_retry_tracking` が呼ばれた直後は 0 から再スタートする。
        let mut last_processed = 0u64;
        let fresh_state_seq = 0u64; // LocationShowState::default() 相当
        let previous = std::mem::replace(&mut last_processed, fresh_state_seq);
        assert!(!is_new_retry(fresh_state_seq, previous));
    }

    #[test]
    fn drive_sends_fix_when_state_is_waiting() {
        let location = SharedLocation::new();
        location.push_fix(Fix {
            latitude_deg: 1.0,
            longitude_deg: 2.0,
            accuracy_m: 3.0,
        });
        let (sender, rx) = sender_with_channel();
        let state = LocationShowState::default();

        drive(&state, &sender, &location);

        let completion = rx.recv().expect("完了が送られる");
        let mut applied = state;
        completion(&mut applied);
        assert_eq!(
            applied.view,
            fandhe_core::screens::location_show::LocationView::Fixed {
                lat: 1.0,
                lon: 2.0,
                accuracy_m: 3.0
            }
        );
    }

    #[test]
    fn drive_skips_send_when_fix_unchanged() {
        let location = SharedLocation::new();
        location.push_fix(Fix {
            latitude_deg: 1.0,
            longitude_deg: 2.0,
            accuracy_m: 3.0,
        });
        let (sender, rx) = sender_with_channel();
        let state = LocationShowState {
            view: fandhe_core::screens::location_show::LocationView::Fixed {
                lat: 1.0,
                lon: 2.0,
                accuracy_m: 3.0,
            },
            ..LocationShowState::default()
        };

        drive(&state, &sender, &location);

        assert!(rx.try_recv().is_err(), "同じ値なら完了を送らない");
    }

    #[test]
    fn drive_skips_send_while_still_waiting() {
        let location = SharedLocation::new();
        let (sender, rx) = sender_with_channel();
        let state = LocationShowState::default();

        drive(&state, &sender, &location);

        assert!(rx.try_recv().is_err(), "未取得のままなら完了を送らない");
    }

    #[test]
    fn drive_maps_unsupported_to_unavailable() {
        let location = SharedLocation::new();
        location.push_failure(LocationFailure::Unsupported);
        let (sender, rx) = sender_with_channel();
        let state = LocationShowState::default();

        drive(&state, &sender, &location);

        let completion = rx.recv().expect("完了が送られる");
        let mut applied = state;
        completion(&mut applied);
        assert_eq!(
            applied.view,
            fandhe_core::screens::location_show::LocationView::Unavailable
        );
    }

    #[test]
    fn drive_maps_permission_denied_and_discards_previous_fix_display() {
        let location = SharedLocation::new();
        location.push_failure(LocationFailure::PermissionDenied);
        let (sender, rx) = sender_with_channel();
        let state = LocationShowState {
            view: fandhe_core::screens::location_show::LocationView::Fixed {
                lat: 1.0,
                lon: 2.0,
                accuracy_m: 3.0,
            },
            ..LocationShowState::default()
        };

        drive(&state, &sender, &location);

        let completion = rx.recv().expect("完了が送られる");
        let mut applied = state;
        completion(&mut applied);
        assert_eq!(
            applied.view,
            fandhe_core::screens::location_show::LocationView::Denied
        );
    }

    #[test]
    fn shared_location_is_unavailable_on_non_windows_non_apple_host() {
        // CI は Linux/macOS のみ（Windows/Android 実機は Issue #888/#890 の
        // スコープ外、実装計画 §3.3「Windows 実機では確認できない」参照）。macOS は
        // Apple 分岐（下記テスト）のため対象外にする（Issue #889）。
        #[cfg(not(any(
            target_os = "windows",
            target_os = "android",
            target_os = "ios",
            target_os = "macos"
        )))]
        {
            let shared = shared_location();
            assert!(matches!(
                shared.last_fix(),
                Err(DeviceError::Unsupported("location"))
            ));
        }
    }

    #[test]
    fn shared_location_is_waiting_on_apple_host_without_starting_core_location() {
        // Apple（iOS/macOS）は `shared_location()` 単独では `CLLocationManager` を
        // 起動しない（モジュール doc 参照。`cargo test`/clippy が本関数を呼んでも
        // TCC プロンプトを出さないことを保証する）。起動は `location_effect` 側の
        // 責務のため、ここでは呼ばない。
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            let shared = shared_location();
            assert!(matches!(shared.last_fix(), Ok(None)));
        }
    }
}
