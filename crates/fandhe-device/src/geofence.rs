//! ジオフェンス（領域の出入り判定）の純粋ロジック（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/geofence.json`）。
//!
//! 扱う範囲は「地点（緯度・経度）と半径（m）を 1 件だけ登録し、[`crate::location`] の
//! 継続取得から読み取った [`Fix`]（[`GeofenceTracker::observe`] に渡されたもの）ごとに内外を判定して、内外が変わったら入った / 出た
//! イベントを時刻付きで出す」ことに限る。前景中心で、アプリ停止中・背景での通知は
//! 扱わない。複数領域・滞在判定・通知の発火も扱わない。
//!
//! 設計判断として、OS の領域監視 API
//! （`CLCircularRegion` / `GeofencingClient` 等）は使わず、既存の location に乗せて
//! Rust 側で判定する。したがって OS 実装・橋渡しは持たず、判定器 [`GeofenceTracker`]
//! だけが本機能の実体になる。対象は location が動く iOS / Android / macOS / Windows。
//! Web は対応表で no のまま（本モジュールは `cfg` を持たないが、Web 向けの配線はしない）。
//! 呼び出し元（画面側）は `Location::last_fix` の結果を [`GeofenceTracker::observe`] へ
//! 渡す。
//!
//! # 契約
//!
//! - **登録は 1 件**: [`GeofenceTracker::register`] は再登録で置き換え、内外状態と
//!   イベント履歴をリセットする。不正な入力（緯度経度の範囲外・非有限・半径が
//!   `MIN_RADIUS_M..=MAX_RADIUS_M` の外）は `Err(InvalidInput)` で、既存の登録を変えない
//! - **最初の Fix は初期状態の確定**: 登録後（または失敗後）に最初に判定へ使われた
//!   Fix は内外を確定するだけでイベントにしない
//! - **内側の定義**: 大圏距離（haversine、地球半径 6371 km）が半径以下なら内側
//! - **精度が悪い Fix は使わない**: `accuracy_m` が半径より大きい Fix は判定に使わず、
//!   状態もイベントも変えない（初期状態の確定にも使わない）。範囲外・非有限の Fix も同様
//! - **失敗は握りつぶさない**: `observe` に渡された location の結果が `Err` なら
//!   判定を止め、そのエラーをそのまま返す。内外は「未確定」へ戻し、最後の内外を
//!   「今の状態」と偽らない。回復後の最初の Fix は再び初期状態の確定になる。
//!   `Ok(None)`（未取得）は何も変えず `Ok(None)` を返す
//! - **履歴**: イベントは直近 [`MAX_EVENTS`] 件だけ保持する（古い方から捨てる）
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 プロセス内・1 判定器での Fix の処理順と、失敗を成功と見せないことだけ。
//! 判定器は `&mut self` の同期 API で、共有が要る場合は呼び出し側が 1 本にまとめる。
//! 既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）として、
//! 境界付近で測位が揺れると出入りが繰り返し通知されうる（ヒステリシスは持たない）。
//! 位置の更新間隔・遅延は OS 任せで、出入りの検知時刻は Fix を観測した時刻になる。
//!
//! **Fix の鮮度は判定しない**（オーナー判断 10/4）。登録・判定には location の最新 Fix を
//! そのまま使い、古さを理由に登録を拒否したり再取得を要求したりしない。測位が止まって
//! 古い Fix が残っている間は、登録地点・内外表示がその Fix に基づくことがある（既知の
//! 制約）。代わりに画面は Fix を受け取ってからの経過時間を表示し、判断を利用者に任せる
//! （位置の継続取得からアプリ側で判定する一般的な実装と同じ扱い）。
//!
//! **判定は呼び出し側の読み取り間隔ごとの最新 Fix に対して行う**（オーナー判断 10/5）。
//! location の共有状態は最新 1 件の上書きなので、画面側が一定間隔（例: 1 秒）で
//! `last_fix` を読んで [`GeofenceTracker::observe`] へ渡す配線では、読み取りの間に
//! 届いて上書きされた Fix は判定されない。読み取り間隔より短い出入り（間隔内に入って
//! 出る）は検知しない（既知の制約）。小さい半径を車で通過する場合などに起こりうるが、
//! 今回の範囲（前景で 1 地点の出入りを表示する型サンプル）では許容する。
//!
//! **OS の測位精度が半径に届かない環境では登録・判定できない**（オーナー判断 10/5）。
//! 精度が半径より悪い Fix は使わない契約のため、Android 11 以下（API 24〜30、fused
//! provider が無く network 測位のみ）や「おおよその位置」だけ許可された場合などは、
//! 登録できない・未確定のままになることがある（既知の制約）。登録時に精度が足りなければ
//! 画面にその旨（精度不足）を出す。登録後の判定中に精度不足の Fix が届いた場合は判定に
//! 使わず、直前の内外表示のままになる（精度不足の表示はしない）。精度を上げるために測位元（GPS 等）を自前で選び分ける・複数購読することはしない。

use std::collections::VecDeque;

use crate::DeviceError;
use crate::location::Fix;

/// 半径の下限（m）。
pub const MIN_RADIUS_M: f64 = 1.0;
/// 半径の上限（m、100 km）。これを超える値は実用外として拒否する。
pub const MAX_RADIUS_M: f64 = 100_000.0;
/// 保持するイベント履歴の件数上限。
pub const MAX_EVENTS: usize = 8;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// 出入りの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeofenceKind {
    /// 外から内へ入った。
    Entered,
    /// 内から外へ出た。
    Exited,
}

/// 出入りイベント（観測時刻付き）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeofenceEvent {
    /// 入った / 出た。
    pub kind: GeofenceKind,
    /// 判定した Fix を観測した時刻（UNIX エポックからのミリ秒、呼び出し側が渡す）。
    pub at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct Region {
    latitude_deg: f64,
    longitude_deg: f64,
    radius_m: f64,
}

/// 領域 1 件の内外を判定する純粋な判定器（モジュール doc の「契約」参照）。
#[derive(Debug, Default)]
pub struct GeofenceTracker {
    region: Option<Region>,
    /// `None` は未確定（登録直後・失敗後）。
    inside: Option<bool>,
    events: VecDeque<GeofenceEvent>,
}

impl GeofenceTracker {
    /// 未登録の判定器を作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 領域を 1 件登録する。再登録は置き換えで、内外状態とイベント履歴をリセットする。
    /// 不正な入力は `Err(InvalidInput)` を返し、既存の登録を変えない。
    pub fn register(
        &mut self,
        latitude_deg: f64,
        longitude_deg: f64,
        radius_m: f64,
    ) -> Result<(), DeviceError> {
        let valid = latitude_deg.is_finite()
            && (-90.0..=90.0).contains(&latitude_deg)
            && longitude_deg.is_finite()
            && (-180.0..=180.0).contains(&longitude_deg)
            && radius_m.is_finite()
            && (MIN_RADIUS_M..=MAX_RADIUS_M).contains(&radius_m);
        if !valid {
            return Err(DeviceError::InvalidInput("geofence region"));
        }
        self.region = Some(Region {
            latitude_deg,
            longitude_deg,
            radius_m,
        });
        self.inside = None;
        self.events.clear();
        Ok(())
    }

    /// 登録を解除する（履歴も捨てる）。未登録でも何もしない。
    pub fn clear(&mut self) {
        self.region = None;
        self.inside = None;
        self.events.clear();
    }

    /// 現在の内外。未登録・未確定（まだ判定に使える Fix が無い・失敗後）は `None`。
    pub fn is_inside(&self) -> Option<bool> {
        self.inside
    }

    /// 保持中のイベント履歴（古い順、最大 [`MAX_EVENTS`] 件）。
    pub fn events(&self) -> impl Iterator<Item = &GeofenceEvent> {
        self.events.iter()
    }

    /// `Location::last_fix` の結果 1 件を判定へ流す。内外が変わったときだけ
    /// `Ok(Some(event))` を返す。未登録のときは Fix を無視して `Ok(None)`。
    pub fn observe(
        &mut self,
        now_unix_ms: u64,
        last_fix: Result<Option<Fix>, DeviceError>,
    ) -> Result<Option<GeofenceEvent>, DeviceError> {
        let fix = match last_fix {
            Ok(fix) => fix,
            Err(e) => {
                self.inside = None;
                return Err(e);
            }
        };
        let (Some(region), Some(fix)) = (self.region, fix) else {
            return Ok(None);
        };
        if !crate::location::is_valid_fix(fix) || fix.accuracy_m > region.radius_m {
            return Ok(None);
        }
        let now_inside = distance_m(&region, &fix) <= region.radius_m;
        let previous = self.inside.replace(now_inside);
        let kind = match previous {
            Some(false) if now_inside => GeofenceKind::Entered,
            Some(true) if !now_inside => GeofenceKind::Exited,
            _ => return Ok(None),
        };
        let event = GeofenceEvent {
            kind,
            at_unix_ms: now_unix_ms,
        };
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
        Ok(Some(event))
    }
}

/// 領域中心と Fix の大圏距離（m、haversine）。
fn distance_m(region: &Region, fix: &Fix) -> f64 {
    let (lat1, lat2) = (
        region.latitude_deg.to_radians(),
        fix.latitude_deg.to_radians(),
    );
    let dlat = lat2 - lat1;
    let dlon = (fix.longitude_deg - region.longitude_deg).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().min(1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 緯度 0 度では経度 1 度が約 111.19 km なので、経度 0.001 度は約 111 m。
    fn fix(lon: f64, accuracy_m: f64) -> Result<Option<Fix>, DeviceError> {
        Ok(Some(Fix {
            latitude_deg: 0.0,
            longitude_deg: lon,
            accuracy_m,
        }))
    }

    fn tracker() -> GeofenceTracker {
        let mut t = GeofenceTracker::new();
        t.register(0.0, 0.0, 200.0).unwrap();
        t
    }

    #[test]
    fn first_fix_sets_state_without_event() {
        let mut t = tracker();
        assert_eq!(t.observe(1, fix(0.0, 10.0)).unwrap(), None);
        assert_eq!(t.is_inside(), Some(true));
    }

    #[test]
    fn enter_and_exit_emit_events_with_time() {
        let mut t = tracker();
        t.observe(1, fix(0.01, 10.0)).unwrap();
        assert_eq!(t.is_inside(), Some(false));
        let e = t.observe(2, fix(0.0, 10.0)).unwrap().unwrap();
        assert_eq!((e.kind, e.at_unix_ms), (GeofenceKind::Entered, 2));
        assert_eq!(t.observe(3, fix(0.0005, 10.0)).unwrap(), None);
        let e = t.observe(4, fix(0.01, 10.0)).unwrap().unwrap();
        assert_eq!((e.kind, e.at_unix_ms), (GeofenceKind::Exited, 4));
    }

    #[test]
    fn inaccurate_fix_is_ignored() {
        let mut t = tracker();
        t.observe(1, fix(0.0, 10.0)).unwrap();
        assert_eq!(t.observe(2, fix(0.01, 500.0)).unwrap(), None);
        assert_eq!(t.is_inside(), Some(true));
    }

    #[test]
    fn invalid_register_keeps_existing_region() {
        let mut t = tracker();
        for (lat, lon, r) in [
            (91.0, 0.0, 100.0),
            (0.0, f64::NAN, 100.0),
            (0.0, 0.0, 0.0),
            (0.0, 0.0, -1.0),
            (0.0, 0.0, MAX_RADIUS_M + 1.0),
            (0.0, 0.0, f64::INFINITY),
        ] {
            assert!(matches!(
                t.register(lat, lon, r),
                Err(DeviceError::InvalidInput(_))
            ));
        }
        t.observe(1, fix(0.0, 10.0)).unwrap();
        assert_eq!(t.is_inside(), Some(true));
    }

    #[test]
    fn reregister_resets_state_and_history() {
        let mut t = tracker();
        t.observe(1, fix(0.01, 10.0)).unwrap();
        t.observe(2, fix(0.0, 10.0)).unwrap();
        t.register(0.0, 0.0, 200.0).unwrap();
        assert_eq!(t.is_inside(), None);
        assert_eq!(t.events().count(), 0);
    }

    #[test]
    fn location_failure_is_surfaced_and_state_becomes_unknown() {
        let mut t = tracker();
        t.observe(1, fix(0.0, 10.0)).unwrap();
        let r = t.observe(2, Err(DeviceError::PermissionDenied("location")));
        assert!(matches!(r, Err(DeviceError::PermissionDenied(_))));
        assert_eq!(t.is_inside(), None);
        // 回復後の最初の Fix は初期状態の確定でイベントにならない。
        assert_eq!(t.observe(3, fix(0.01, 10.0)).unwrap(), None);
        assert_eq!(t.is_inside(), Some(false));
    }

    #[test]
    fn waiting_location_changes_nothing() {
        let mut t = tracker();
        t.observe(1, fix(0.0, 10.0)).unwrap();
        assert_eq!(t.observe(2, Ok(None)).unwrap(), None);
        assert_eq!(t.is_inside(), Some(true));
    }

    #[test]
    fn history_keeps_only_latest_events() {
        let mut t = tracker();
        t.observe(0, fix(0.01, 10.0)).unwrap();
        for i in 1..=(MAX_EVENTS as u64 + 4) {
            let lon = if i % 2 == 1 { 0.0 } else { 0.01 };
            t.observe(i, fix(lon, 10.0)).unwrap();
        }
        assert_eq!(t.events().count(), MAX_EVENTS);
        assert_eq!(t.events().next().unwrap().at_unix_ms, 5);
    }
}
