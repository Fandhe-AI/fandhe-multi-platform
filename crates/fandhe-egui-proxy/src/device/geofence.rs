//! Phase 11 の拡充ループ（親 #1056）型サンプル「ジオフェンス」（Issue #1057）の effect
//! ドライバ。`fandhe_core::screens::geofence_show`（端末依存ゼロ）へ、location の最新 Fix を
//! `fandhe_device::geofence::GeofenceTracker` で判定した結果のスナップショットを渡す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。
//!
//! オーナー判断（10/4）により OS の領域監視 API は使わず、[`super::location::shared_location`]
//! の継続取得に乗せて Rust 側で判定する。`fandhe-core` は `fandhe-device` に依存できないため、
//! 判定器は本モジュールの `static` が持つ（画面状態は表示用スナップショットだけ）。
//!
//! # 守る範囲と守らない範囲
//!
//! - 守る: 1 プロセス内の処理順（effect は egui の update ループ 1 本からのみ呼ばれ、判定と
//!   状態更新は同じロックの内側）、失敗・未取得を「中にいる」「登録済み」に見せないこと
//! - 守らない: 境界付近の揺れによる出入りの繰り返し（ヒステリシス無し）、前景のみで背景通知は
//!   しないこと
//!
//! location の起動・停止は location 画面に揃える（Apple は [`start_apple_location_once`]、
//! 停止はアプリ終了時の `stop_shared_location` のみ）。登録時に測位が失敗中または
//! 失敗中のときは、偽の現在地で登録せず [`restart_location`] で測位を再起動して案内する。
//! Fix の鮮度は判定しない（オーナー判断 10/4、`fandhe_device::geofence` 参照）。最新 Fix を
//! そのまま登録・判定に使い、受信からの経過時間を画面に出して判断を利用者に任せる。
//! Android は Activity 再生成のたびに [`reset_register_tracking`] で static 側の登録をすべて破棄する。

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fandhe_core::screens::geofence_show::{
    EventView, GeofenceShowState, GeofenceView, RegionView, RegisterNotice,
};
use fandhe_device::geofence::{GeofenceKind, GeofenceTracker};
use fandhe_device::location::Location;
use fandhe_device::location::SharedLocation;

#[cfg(any(
    target_os = "windows",
    target_os = "android",
    target_os = "ios",
    target_os = "macos"
))]
use super::location::restart_location;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use super::location::start_apple_location_once;
use super::location::{shared_location_precise, to_failure_kind, POLL_INTERVAL};
use crate::app::CompletionSender;

/// 判定器と、登録要求の処理済み世代・登録地点・案内の保持先。
struct GeofenceDriver {
    tracker: GeofenceTracker,
    last_register_seq: u64,
    region: Option<RegionView>,
    notice: Option<RegisterNotice>,
    /// 測位前に押された登録要求を、最初の Fix が届いたときに処理するための保留フラグ。
    pending_register: bool,
}

impl GeofenceDriver {
    fn new() -> Self {
        Self {
            tracker: GeofenceTracker::new(),
            last_register_seq: 0,
            region: None,
            notice: None,
            pending_register: false,
        }
    }
}

static DRIVER: Mutex<Option<GeofenceDriver>> = Mutex::new(None);

/// Android: Activity 再生成で画面の `GeofenceShowState` が未登録の既定値に戻るのに合わせ、
/// static 側の登録（判定器・登録地点・案内・履歴・処理済み世代）をまとめて破棄して未登録へ
/// 揃える（永続化はしない。`device::location::reset_retry_tracking` と同じ位置づけで
/// `android_main` から呼ぶ）。
#[cfg(any(target_os = "android", test))]
pub(crate) fn reset_register_tracking() {
    *DRIVER.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// `location` の最新結果を判定して、表示が変わったときだけ完了を送る（`location.rs::drive`
/// と同じ切り出し。変化が無い間も `request_repaint_after` で次回ポーリングを予約する）。
fn drive(
    state: &GeofenceShowState,
    sender: &CompletionSender<GeofenceShowState>,
    location: &dyn Location,
    driver: &mut GeofenceDriver,
    now_unix_ms: u64,
    fix_age: Option<Duration>,
) -> bool {
    let fix = location.last_fix();
    let mut needs_restart = false;

    // 新しい登録要求、または測位前に押されて保留していた要求を、Fix があれば処理する。
    let new_request = state.register_seq > driver.last_register_seq;
    if new_request || (driver.pending_register && matches!(fix, Ok(Some(_)))) {
        driver.last_register_seq = state.register_seq;
        driver.pending_register = false;
        driver.notice = match &fix {
            // 判定器は精度が半径を超える Fix を使わないため、登録にも使わない（登録直後に未確定のままになる）。
            Ok(Some(f)) if f.accuracy_m > state.requested_radius_m => {
                Some(RegisterNotice::Inaccurate)
            }
            Ok(Some(f)) => {
                let radius = state.requested_radius_m;
                match driver
                    .tracker
                    .register(f.latitude_deg, f.longitude_deg, radius)
                {
                    Ok(()) => {
                        driver.region = Some(RegionView {
                            lat: f.latitude_deg,
                            lon: f.longitude_deg,
                            radius_m: radius,
                        });
                        None
                    }
                    Err(_) => Some(RegisterNotice::Invalid),
                }
            }
            // 未取得・失敗のときは偽の地点で登録せず、要求を保留して Fix の到着時に処理する。
            // 失敗なら測位を再起動する（保留中の再要求では再起動を繰り返さない）。
            Ok(None) => {
                driver.pending_register = true;
                Some(RegisterNotice::NoFix)
            }
            Err(_) => {
                driver.pending_register = true;
                needs_restart = new_request;
                Some(RegisterNotice::NoFix)
            }
        };
    }

    let failure = driver
        .tracker
        .observe(now_unix_ms, fix)
        .err()
        .map(|e| to_failure_kind(&e));

    let view = GeofenceView {
        region: driver.region,
        inside: driver.tracker.is_inside(),
        events: driver
            .tracker
            .events()
            .map(|e| EventView {
                entered: e.kind == GeofenceKind::Entered,
                at_unix_ms: e.at_unix_ms,
            })
            .collect(),
        failure,
        notice: driver.notice,
        fix_age_secs: fix_age.map(|a| a.as_secs()),
    };

    if view == state.view {
        sender.request_repaint_after(POLL_INTERVAL);
        return needs_restart;
    }
    let _ = sender.send(Box::new(move |s| s.view = view));
    needs_restart
}

/// `registry.rs` の `effect` 区画から `geofence_show` 画面用に配線される effect ドライバ。
pub fn geofence_effect(state: &GeofenceShowState, sender: &CompletionSender<GeofenceShowState>) {
    let location = shared_location_precise();
    // Apple は shared_location() から起動しない方針のため、ここで起動する（location.rs 参照）。
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    start_apple_location_once();

    // 時計が取れないときは 0（ios_background_task.rs と同じ扱い）。
    let now_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut guard = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
    let driver = guard.get_or_insert_with(GeofenceDriver::new);
    let needs_restart = drive(
        state,
        sender,
        location,
        driver,
        now_unix_ms,
        location.fix_age(),
    );
    if needs_restart {
        restart_if_supported(location);
    }
}

/// 測位の再起動（対象 OS のみ。他 OS は `shared_location()` が `Unsupported` を保持済み）。
fn restart_if_supported(_location: &SharedLocation) {
    #[cfg(any(
        target_os = "windows",
        target_os = "android",
        target_os = "ios",
        target_os = "macos"
    ))]
    restart_location(_location, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::location::{Fix, LocationFailure, SharedLocation};

    type Rx = std::sync::mpsc::Receiver<crate::app::Completion<GeofenceShowState>>;

    fn sender_with_channel() -> (CompletionSender<GeofenceShowState>, Rx) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    fn fix_at(lat: f64, accuracy_m: f64) -> Fix {
        Fix {
            latitude_deg: lat,
            longitude_deg: 139.0,
            accuracy_m,
        }
    }

    /// 1 フレーム分回し、送られた完了を state へ適用する。
    fn step(
        state: &mut GeofenceShowState,
        loc: &SharedLocation,
        driver: &mut GeofenceDriver,
        now: u64,
    ) -> bool {
        let (sender, rx) = sender_with_channel();
        drive(state, &sender, loc, driver, now, loc.fix_age());
        match rx.try_recv() {
            Ok(c) => {
                c(state);
                true
            }
            Err(_) => false,
        }
    }

    fn request(state: &mut GeofenceShowState, radius: f64) {
        state.requested_radius_m = radius;
        state.register_seq += 1;
    }

    #[test]
    fn register_with_fix_confirms_inside_without_event() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        assert_eq!(s.view.inside, Some(true));
        assert!(s.view.events.is_empty());
        assert_eq!(s.view.region.map(|r| r.radius_m), Some(100.0));
    }

    #[test]
    fn pending_register_completes_when_fix_arrives() {
        let loc = SharedLocation::new();
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        assert_eq!(s.view.notice, Some(RegisterNotice::NoFix));
        loc.push_fix(fix_at(35.0, 5.0));
        step(&mut s, &loc, &mut d, 2);
        assert_eq!(s.view.notice, None);
        assert!(
            s.view.region.is_some(),
            "保留した登録要求が Fix の到着で処理される"
        );
    }

    #[test]
    fn register_without_fix_shows_notice_and_no_region() {
        let loc = SharedLocation::new();
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        assert_eq!(s.view.region, None);
        assert_eq!(s.view.notice, Some(RegisterNotice::NoFix));
    }

    #[test]
    fn location_failure_resets_inside_and_shows_failure() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        assert_eq!(s.view.inside, Some(true));
        loc.push_failure(LocationFailure::PermissionDenied);
        step(&mut s, &loc, &mut d, 2);
        assert_eq!(s.view.inside, None);
        assert!(s.view.failure.is_some());
    }

    #[test]
    fn inaccurate_fix_changes_nothing() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        // 遠方だが精度が半径より悪い Fix は使われない。
        loc.push_fix(fix_at(36.0, 500.0));
        assert!(!step(&mut s, &loc, &mut d, 2));
        assert_eq!(s.view.inside, Some(true));
        assert!(s.view.events.is_empty());
    }

    #[test]
    fn register_with_fix_less_accurate_than_radius_is_refused() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 500.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        assert_eq!(s.view.region, None);
        assert_eq!(s.view.notice, Some(RegisterNotice::Inaccurate));
    }

    #[test]
    fn no_change_sends_no_completion() {
        let loc = SharedLocation::new();
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        assert!(!step(&mut s, &loc, &mut d, 1));
    }

    #[test]
    fn moving_in_after_out_records_entered_event() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        loc.push_fix(fix_at(36.0, 5.0));
        step(&mut s, &loc, &mut d, 2);
        assert_eq!(s.view.inside, Some(false));
        loc.push_fix(fix_at(35.0, 5.0));
        step(&mut s, &loc, &mut d, 3);
        assert_eq!(s.view.inside, Some(true));
        assert!(s
            .view
            .events
            .last()
            .is_some_and(|e| e.entered && e.at_unix_ms == 3));
    }

    #[test]
    fn same_seq_does_not_register_twice() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        step(&mut s, &loc, &mut d, 1);
        // 登録後に別地点へ動いても、同じ世代では再登録されない。
        loc.push_fix(fix_at(36.0, 5.0));
        step(&mut s, &loc, &mut d, 2);
        assert_eq!(s.view.region.map(|r| r.lat), Some(35.0));
    }

    #[test]
    fn old_fix_is_used_as_is_and_age_is_exposed() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        let (sender, rx) = sender_with_channel();
        let age = Some(Duration::from_secs(3600));
        assert!(!drive(&s, &sender, &loc, &mut d, 1, age));
        rx.try_recv().expect("view 更新")(&mut s);
        assert_eq!(s.view.region.map(|r| r.lat), Some(35.0));
        assert_eq!(s.view.inside, Some(true));
        assert_eq!(s.view.fix_age_secs, Some(3600));
    }

    #[test]
    fn failure_at_register_requests_restart() {
        let loc = SharedLocation::new();
        loc.push_failure(LocationFailure::PermissionDenied);
        let mut d = GeofenceDriver::new();
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        let (sender, _rx) = sender_with_channel();
        assert!(drive(&s, &sender, &loc, &mut d, 1, None));
        assert_eq!(d.notice, Some(RegisterNotice::NoFix));
    }

    #[test]
    fn reset_register_tracking_discards_registration() {
        let loc = SharedLocation::new();
        loc.push_fix(fix_at(35.0, 5.0));
        let mut s = GeofenceShowState::default();
        request(&mut s, 100.0);
        // static の DRIVER を使う唯一のテスト（並行テストと干渉しない）。
        {
            let mut g = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
            let d = g.get_or_insert_with(GeofenceDriver::new);
            step(&mut s, &loc, d, 1);
            assert_eq!(s.view.inside, Some(true));
        }
        reset_register_tracking();
        // 再生成後の新しい画面（既定値）で最初の effect が走っても、以前の登録は現れない。
        let mut fresh = GeofenceShowState::default();
        let mut g = DRIVER.lock().unwrap_or_else(|p| p.into_inner());
        let d = g.get_or_insert_with(GeofenceDriver::new);
        step(&mut fresh, &loc, d, 2);
        assert_eq!(fresh.view.region, None);
        assert_eq!(fresh.view.inside, None);
        assert!(fresh.view.events.is_empty());
        assert_eq!(fresh.view.notice, None);
    }
}
