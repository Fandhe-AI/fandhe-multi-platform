//! Phase 11（親 #85）型サンプル「地図表示」（Issue #273）の effect ドライバ。
//! `fandhe_core::screens::map_view_show`（端末依存ゼロ）の要求を観測し、地点を
//! `fandhe_device::map_view::MapRegion` で検証してから OS の地図スナップショットへ渡し、
//! 結果を `CompletionSender` 経由で返す（`screens/registry.rs` の `effect` 区画から呼ばれる契約）。
//! 契約と守る範囲は `fandhe_device::map_view` のモジュール doc を参照。
//!
//! OS 別の置き場:
//! - macOS: `apple_map_view.rs` + `apple/fandhe_apple_map_view.m`（#273）。コールバック型のため
//!   pull 型 [`bridge`]（`RequestSlot` の facade）で毎フレーム [`drive_callback`] が結果を反映する
//! - iOS: 同じ `.m` の iOS 分岐（#274 が埋めるまでは `Unsupported`）
//! - それ以外: `UnsupportedMapSnapshotter` を同期に呼んで「非対応」を表示する
//!
//! 期限（[`SNAPSHOT_DEADLINE`]）は `start` を呼んだ時刻を起点に単調時計で測り、切れたら
//! 失敗で確定する。確定後に届いた結果は `RequestSlot` の first-wins と id 照合で捨てる。

use fandhe_core::screens::map_view_show::{
    finish, mark_loading, take_request, MapOutcome, MapViewShowState, MAP_HEIGHT_PX, MAP_WIDTH_PX,
    PRESETS,
};
use fandhe_core::RgbaPixels;
use fandhe_device::map_view::MapRegion;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 取得にかける最大時間。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
const SNAPSHOT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// 取得サイズ（共通コアの定数）。
const SIZE_PX: (u32, u32) = (MAP_WIDTH_PX, MAP_HEIGHT_PX);

/// 期限を過ぎたか。起点・現在とも単調時計 `Instant`（システム時刻の変更に影響されない）。
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
fn is_expired(
    started: std::time::Instant,
    now: std::time::Instant,
    deadline: std::time::Duration,
) -> bool {
    now.saturating_duration_since(started) >= deadline
}

/// 地点の添字から検証済みの範囲を作る。
fn region_for(preset: usize) -> Result<MapRegion, DeviceError> {
    let p = PRESETS
        .get(preset)
        .ok_or(DeviceError::InvalidInput("map_view region"))?;
    MapRegion::new(p.center_lat, p.center_lon, p.span_m)
}

/// `.m` 側の結果コード → 結果の写像（SSOT。未知のコードは失敗に倒し成功を作らない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 画像あり（画素の検査は呼び出し側の `build_snapshot`） | `Ok(())` |
/// | 1 | 非対応（iOS のスタブ等） | `Unsupported` |
/// | 2 | 通信できない（`NSURLErrorDomain` の接続なし系） | `Disabled("map_view network")` |
/// | 3 | MapKit の失敗・その他の `NSError` | `Internal("map_view mapkit")` |
/// | 4 | 画像なし・0 寸法・変換失敗 | `Internal("map_view image")` |
/// | 5 | main スレッド以外から呼ばれた | `Internal("map_view thread")` |
/// | 他 | 未知 | `Internal("map_view outcome")` |
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn outcome_from_apple_code(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported("map_view")),
        2 => Err(DeviceError::Disabled("map_view network")),
        3 => Err(DeviceError::Internal("map_view mapkit")),
        4 => Err(DeviceError::Internal("map_view image")),
        5 => Err(DeviceError::Internal("map_view thread")),
        _ => Err(DeviceError::Internal("map_view outcome")),
    }
}

/// `DeviceError` を画面用の結果へ写像する。内部構造は出さず固定文言だけにする
/// （security.md A09）。
fn to_outcome(result: Result<fandhe_device::Image, DeviceError>) -> Result<MapOutcome, String> {
    match result {
        Ok(img) => RgbaPixels::new(img.width, img.height, img.rgba)
            .map(MapOutcome::Shown)
            .ok_or_else(|| "画像が不正です".to_string()),
        Err(DeviceError::Disabled(_)) => Ok(MapOutcome::Offline),
        Err(DeviceError::Unsupported(_)) => Ok(MapOutcome::Unsupported),
        Err(_) => Err("OS の地図取得に失敗しました".to_string()),
    }
}

/// OS 側コールバックとの橋渡し（macOS / iOS、pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod bridge {
    use fandhe_core::screens::map_view_show::MapOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) type Outcome = Result<MapOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 依頼直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く。`request_id` 不一致・未登録・既に結果ありは
    /// 捨てて `false`（先に届いたものだけを採る）。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<fandhe_device::Image, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// `.m` の受け口から呼ぶ。コード → 画素の検査 → 配送を 1 か所で行う。
    pub(crate) fn deliver_rgba(
        request_id: u64,
        code: i32,
        rgba: Option<Vec<u8>>,
        width: u32,
        height: u32,
    ) -> bool {
        let result = super::outcome_from_apple_code(code).and_then(|()| {
            fandhe_device::map_view::build_snapshot(
                super::SIZE_PX,
                width,
                height,
                rgba.ok_or(DeviceError::Internal("map_view image"))?,
            )
        });
        deliver(request_id, result)
    }
}

/// 進行中の要求の id と `start` を呼んだ時刻（期限の起点）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
static STARTED: std::sync::Mutex<Option<(u64, std::time::Instant)>> = std::sync::Mutex::new(None);

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn snapshotter() -> &'static dyn fandhe_device::map_view::MapSnapshotter {
    super::apple_map_view::snapshotter()
}

/// 期限切れなら OS へ取り消しを伝えて失敗で確定する。待機中のフレームごとに呼ばれる。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn expire_if_needed(request_id: u64, now: std::time::Instant) {
    let started = *STARTED.lock().unwrap_or_else(|e| e.into_inner());
    let Some((id, at)) = started else { return };
    if id == request_id && is_expired(at, now, SNAPSHOT_DEADLINE) {
        snapshotter().cancel(request_id);
        bridge::deliver(request_id, Err(DeviceError::Internal("map_view timeout")));
    }
}

/// 要求を OS へ渡す。失敗はその場で `bridge::deliver` する。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn start_request(request_id: u64, preset: usize) {
    use fandhe_device::network_status::NetworkSnapshot;

    let region = match region_for(preset) {
        Ok(r) => r,
        Err(e) => {
            bridge::deliver(request_id, Err(e));
            return;
        }
    };
    // オフラインと分かっていれば OS を呼ばない。判定不能・失敗は止めずに進める。
    if matches!(
        super::network_status::implementation().read(),
        Ok(NetworkSnapshot::Offline)
    ) {
        bridge::deliver(request_id, Err(DeviceError::Disabled("map_view network")));
        return;
    }
    // 起点は呼び出しの開始時刻。結果が先に届く場合に備えて start より前に記録する。
    *STARTED.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((request_id, std::time::Instant::now()));
    if let Err(e) = snapshotter().start(request_id, region, SIZE_PX) {
        bridge::deliver(request_id, Err(e));
    }
}

/// macOS / iOS 版の effect ドライバ本体。
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn drive_callback(state: &MapViewShowState, sender: &CompletionSender<MapViewShowState>) {
    use fandhe_core::screens::map_view_show::MapPhase;

    if let Some(req) = take_request(state) {
        let _ = sender.send(Box::new(mark_loading));
        let request_id = bridge::begin();
        start_request(request_id, req.preset);
        return;
    }

    if state.phase != MapPhase::Loading {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |request_id| expire_if_needed(request_id, std::time::Instant::now()),
        move |s: &mut MapViewShowState, outcome| {
            // 反映できたとき（`Idle` へ戻っていないとき）だけ `true` を返す。
            if s.phase != MapPhase::Idle {
                finish(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `registry.rs` から `map_view_show` 画面用に配線される effect ドライバ（macOS / iOS）。
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn map_view_show_effect(state: &MapViewShowState, sender: &CompletionSender<MapViewShowState>) {
    drive_callback(state, sender);
}

/// 同期経路（macOS / iOS 以外）: 地点を検証してから [`MapSnapshotter`] を呼ぶ。
///
/// [`MapSnapshotter`]: fandhe_device::map_view::MapSnapshotter
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn drive(
    state: &MapViewShowState,
    sender: &CompletionSender<MapViewShowState>,
    snapshotter: &dyn fandhe_device::map_view::MapSnapshotter,
) {
    let Some(req) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_loading));
    // 結果は届かないため `start` の `Err` がそのまま完了になる。`Ok` は画像が無いので失敗に倒す。
    let result = region_for(req.preset).and_then(|r| {
        snapshotter.start(0, r, SIZE_PX)?;
        Err(DeviceError::Internal("map_view image"))
    });
    let outcome = to_outcome(result);
    let generation = req.generation;
    let _ = sender.send(Box::new(move |s: &mut MapViewShowState| {
        finish(s, generation, outcome);
    }));
}

/// `registry.rs` から配線される effect ドライバ（macOS / iOS 以外）。
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub fn map_view_show_effect(state: &MapViewShowState, sender: &CompletionSender<MapViewShowState>) {
    drive(
        state,
        sender,
        &fandhe_device::map_view::UnsupportedMapSnapshotter,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::Image;
    use std::time::{Duration, Instant};

    #[test]
    fn outcome_from_apple_code_table() {
        assert!(outcome_from_apple_code(0).is_ok());
        assert!(matches!(
            outcome_from_apple_code(1),
            Err(DeviceError::Unsupported("map_view"))
        ));
        assert!(matches!(
            outcome_from_apple_code(2),
            Err(DeviceError::Disabled("map_view network"))
        ));
        for code in [3, 4, 5, 6, -1, i32::MAX] {
            assert!(matches!(
                outcome_from_apple_code(code),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn to_outcome_separates_three_kinds_and_never_shows_bad_pixels() {
        let img = |w, h, len| Image {
            width: w,
            height: h,
            rgba: vec![0; len],
        };
        assert!(matches!(
            to_outcome(Ok(img(2, 2, 16))),
            Ok(MapOutcome::Shown(_))
        ));
        assert_eq!(
            to_outcome(Err(DeviceError::Disabled("map_view network"))),
            Ok(MapOutcome::Offline)
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("map_view"))),
            Ok(MapOutcome::Unsupported)
        );
        assert!(to_outcome(Err(DeviceError::Internal("map_view timeout"))).is_err());
        assert!(to_outcome(Ok(img(0, 2, 0))).is_err());
        assert!(to_outcome(Ok(img(2, 2, 3))).is_err());
    }

    #[test]
    fn is_expired_boundary() {
        let t = Instant::now();
        let d = Duration::from_secs(15);
        assert!(!is_expired(t, t + Duration::from_millis(14_999), d));
        assert!(is_expired(t, t + d, d));
        // 現在が起点より前でも負にならない。
        assert!(!is_expired(t + d, t, d));
    }

    #[test]
    fn presets_make_valid_regions() {
        for i in 0..PRESETS.len() {
            assert!(region_for(i).is_ok());
        }
        assert!(region_for(PRESETS.len()).is_err());
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    mod sync_path {
        use super::*;
        use fandhe_core::screens::map_view_show::{request, MapPhase};

        #[test]
        fn unsupported_is_shown_and_never_a_map() {
            let mut s = MapViewShowState::default();
            request(&mut s, 0);
            let (tx, rx) = std::sync::mpsc::channel();
            let sender = CompletionSender::new(tx, egui::Context::default());
            drive(
                &s,
                &sender,
                &fandhe_device::map_view::UnsupportedMapSnapshotter,
            );
            while let Ok(apply) = rx.try_recv() {
                apply(&mut s);
            }
            assert_eq!(s.phase, MapPhase::Idle);
            assert!(s.image.is_none());
            assert_eq!(
                s.status,
                fandhe_core::screens::map_view_show::UNSUPPORTED_TEXT
            );
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod bridge_tests {
    use super::*;

    #[test]
    fn stale_id_dropped_second_deliver_ignored_and_late_success_after_timeout_dropped() {
        let id = bridge::begin();
        let ok = || vec![0u8; (MAP_WIDTH_PX * MAP_HEIGHT_PX * 4) as usize];
        // 古い id は捨てる。
        assert!(!bridge::deliver_rgba(
            id + 1000,
            0,
            Some(ok()),
            MAP_WIDTH_PX,
            MAP_HEIGHT_PX
        ));
        // 期限切れで確定した後に届いた成功は捨てる。
        assert!(bridge::deliver(
            id,
            Err(DeviceError::Internal("map_view timeout"))
        ));
        assert!(!bridge::deliver_rgba(
            id,
            0,
            Some(ok()),
            MAP_WIDTH_PX,
            MAP_HEIGHT_PX
        ));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済み");
        assert!(matches!(ready, Some(Err(_))));
        bridge::SLOT.clear(id);
        deliver_rgba_rejects_bad_code_missing_or_mismatched_pixels();
    }

    /// 共有 static の `SLOT` を使うため、並列実行を避けて上のテストから順に呼ぶ。
    fn deliver_rgba_rejects_bad_code_missing_or_mismatched_pixels() {
        for (code, px, w, h) in [
            (3, Some(vec![0u8; 4]), 1, 1),
            (0, None, MAP_WIDTH_PX, MAP_HEIGHT_PX),
            (0, Some(vec![0u8; 4]), 1, 1),
            (0, Some(vec![0u8; 3]), MAP_WIDTH_PX, MAP_HEIGHT_PX),
        ] {
            let id = bridge::begin();
            assert!(bridge::deliver_rgba(id, code, px, w, h));
            let (_, ready) = bridge::SLOT.snapshot().unwrap();
            assert!(!matches!(ready, Some(Ok(MapOutcome::Shown(_)))));
            bridge::SLOT.clear(id);
        }
    }
}
