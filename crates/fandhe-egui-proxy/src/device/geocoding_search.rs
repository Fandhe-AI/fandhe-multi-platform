//! Phase 11（親 #133）型サンプル「住所の座標変換」（Issue #691）の effect ドライバ。
//! `fandhe_core::screens::geocoding_search_show`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `fandhe_device::geocoding_search::ForwardGeocoder` を呼んで `CompletionSender` 経由で結果を返す
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `fandhe_device::geocoding_search`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch は #691 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`super::apple_geocoding_search::AppleForwardGeocoder`]（#691）
//! - iOS: `ios_geocoding_search`（#692。macOS と共用の `AppleForwardGeocoder` を返す）
//! - Android: `android_geocoding_search`（#693。`android.location.Geocoder`）
//! - Windows・Web・Linux: `UnsupportedForwardGeocoder` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中で結果が未着のとき `elapsed >= deadline` なら `Timeout` を確定してスロットを空にし、
//! まだなら `poll_and_finish` の再描画予約（`POLL_INTERVAL`）で次の判定に回る。要求の追跡は
//! `request_slot::RequestSlot`（pull 型）。確定後にワーカーが返す結果は画面側の世代・状態照合
//! （`finish_geocode`）で捨てられる。ワーカー自身も期限超過を `Timeout` にし、期限後に届いた成功を
//! 成功として扱わない。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fandhe_core::screens::geocoding_search_show::{
    finish_geocode, mark_running, take_request, GeocodeFailure, GeocodePhase, GeocodingSearchState,
    ADDRESS,
};
use fandhe_device::geocoding_search::{
    check_result, validate_address, ForwardGeocoder, GEOCODE_DEADLINE, NOT_FOUND, NO_COORDINATE,
    OFFLINE, THROTTLED, TIMEOUT,
};
use fandhe_device::DeviceError;

use super::request_slot::{poll_and_finish, RequestSlot, POLL_INTERVAL};
use crate::app::CompletionSender;

const FAILED_MESSAGE: &str = "座標に変換できませんでした";

/// ワーカーが返す成功値（緯度・経度・候補数）。画面側の `finish_geocode` の引数と同じ形。
type Coordinates = (f64, f64, u32);

/// 現在のプラットフォームに応じた `ForwardGeocoder` 実装を返す（dispatch の最終形）。
fn platform() -> Arc<dyn ForwardGeocoder> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::apple_geocoding_search::AppleForwardGeocoder)
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_geocoding_search::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_geocoding_search::implementation()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    {
        Arc::new(fandhe_device::geocoding_search::UnsupportedForwardGeocoder)
    }
}

/// ドライバの可変状態。要求の追跡（二重発送防止・完了照合・再生成時の破棄）は
/// [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。テストが並列で干渉しないよう static から
/// 切り出して引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Session {
    slot: &'static RequestSlot<Result<Coordinates, GeocodeFailure>>,
    /// 進行中の要求を観測した時刻（単調時計）。
    started: Mutex<Option<Instant>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            started: Mutex::new(None),
        }
    }
}

static SESSION: std::sync::OnceLock<Session> = std::sync::OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の押下が発送されず飲み込まれる。
/// 古いワーカーの結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// `DeviceError` を画面の失敗種別へ写す。内部識別子・OS コード・OS の文言は画面にもログにも出さず、
/// 上記以外は固定文言にする（security.md A09）。権限拒否・OS 側の取り消しも成功にせず固定文言の失敗。
pub(crate) fn to_failure(e: DeviceError) -> GeocodeFailure {
    match e {
        DeviceError::Unsupported(_) => GeocodeFailure::Unsupported,
        DeviceError::Internal(OFFLINE) => GeocodeFailure::Offline,
        DeviceError::Internal(THROTTLED) => GeocodeFailure::Throttled,
        DeviceError::Internal(NOT_FOUND) => GeocodeFailure::NotFound,
        DeviceError::Internal(NO_COORDINATE) => GeocodeFailure::NoCoordinate,
        DeviceError::Internal(TIMEOUT) => GeocodeFailure::Timeout,
        _ => GeocodeFailure::Failed(FAILED_MESSAGE.to_string()),
    }
}

/// ワーカーの本体: 入力検査 → 変換 → 期限・結果の照合。
fn run_worker(
    imp: &dyn ForwardGeocoder,
    started: Instant,
    deadline: Duration,
) -> Result<Coordinates, GeocodeFailure> {
    validate_address(ADDRESS).map_err(to_failure)?;
    let result = imp.geocode(ADDRESS, deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(GeocodeFailure::Timeout);
    }
    let r = result.map_err(to_failure)?;
    // 実装が返した値も契約（範囲・件数）で再検査する。
    let r = check_result(r.latitude, r.longitude, true, r.candidates).map_err(to_failure)?;
    Ok((r.latitude, r.longitude, r.candidates))
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &GeocodingSearchState,
    sender: &CompletionSender<GeocodingSearchState>,
    imp: &Arc<dyn ForwardGeocoder>,
    session: &Session,
    deadline: Duration,
) {
    let slot = session.slot;
    if take_request(state).is_some() {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if slot.snapshot().is_some() {
            return;
        }
        let request_id = slot.begin();
        let started = Instant::now();
        *session.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
        let _ = sender.send(Box::new(mark_running));
        sender.request_repaint_after(POLL_INTERVAL);

        let worker_imp = imp.clone();
        let spawned = std::thread::Builder::new()
            .name("geocoding_search".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, started, deadline);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            slot.deliver(
                request_id,
                Err(GeocodeFailure::Failed(FAILED_MESSAGE.to_string())),
            );
        }
        return;
    }
    if state.phase != GeocodePhase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut GeocodingSearchState| {
                finish_geocode(s, generation, Err(GeocodeFailure::Timeout));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut GeocodingSearchState, result| {
            finish_geocode(s, generation, result);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `geocoding_search_show` 画面用に配線される effect ドライバ。
pub fn geocoding_search_show_effect(
    state: &GeocodingSearchState,
    sender: &CompletionSender<GeocodingSearchState>,
) {
    static IMPL: std::sync::OnceLock<Arc<dyn ForwardGeocoder>> = std::sync::OnceLock::new();
    let imp = IMPL.get_or_init(platform);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        GEOCODE_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::geocoding_search_show::request_geocode;
    use fandhe_device::geocoding_search::{GeocodeResult, UnsupportedForwardGeocoder};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<crate::app::Completion<GeocodingSearchState>>;
    type Outcome = Result<GeocodeResult, DeviceError>;

    fn channel() -> (CompletionSender<GeocodingSearchState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Outcome,
        sleep: Duration,
    }
    impl ForwardGeocoder for Fake {
        fn geocode(&self, _: &str, _: Duration) -> Outcome {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake_with(result: fn() -> Outcome, sleep: Duration) -> Arc<dyn ForwardGeocoder> {
        Arc::new(Fake { result, sleep })
    }

    fn fake(result: fn() -> Outcome) -> Arc<dyn ForwardGeocoder> {
        fake_with(result, Duration::ZERO)
    }

    fn ok() -> Outcome {
        Ok(GeocodeResult {
            latitude: 35.68,
            longitude: 139.76,
            candidates: 2,
        })
    }

    /// フレームを回して届いた完了を適用する。`Running` を抜けるか 5 秒で打ち切る。
    fn pump(
        state: &mut GeocodingSearchState,
        sender: &CompletionSender<GeocodingSearchState>,
        rx: &Rx,
        imp: &Arc<dyn ForwardGeocoder>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, GeocodePhase::Requested | GeocodePhase::Running)
                || Instant::now() > end
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 要求を立ててフレームを回し、完了まで適用した状態を返す。
    fn run(imp: Arc<dyn ForwardGeocoder>) -> GeocodingSearchState {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_secs(30),
        );
        state
    }

    fn failed(s: &GeocodingSearchState) -> &GeocodeFailure {
        match &s.phase {
            GeocodePhase::Failed(f) => f,
            p => panic!("失敗のはず: {p:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        drive(
            &GeocodingSearchState::default(),
            &sender,
            &fake(ok),
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_coordinates() {
        let s = run(fake(ok));
        assert_eq!(
            s.phase,
            GeocodePhase::Done {
                latitude: 35.68,
                longitude: 139.76,
                candidates: 2
            }
        );
    }

    #[test]
    fn errors_map_to_failures() {
        let cases: [(fn() -> Outcome, GeocodeFailure); 9] = [
            (
                || Err(DeviceError::Unsupported("geocoding_search")),
                GeocodeFailure::Unsupported,
            ),
            (
                || Err(DeviceError::Internal(OFFLINE)),
                GeocodeFailure::Offline,
            ),
            (
                || Err(DeviceError::Internal(THROTTLED)),
                GeocodeFailure::Throttled,
            ),
            (
                || Err(DeviceError::Internal(NOT_FOUND)),
                GeocodeFailure::NotFound,
            ),
            (
                || Err(DeviceError::Internal(NO_COORDINATE)),
                GeocodeFailure::NoCoordinate,
            ),
            (
                || Err(DeviceError::Internal(TIMEOUT)),
                GeocodeFailure::Timeout,
            ),
            // 権限拒否は成功にならず固定文言の失敗
            (
                || Err(DeviceError::PermissionDenied("geocoding_search")),
                GeocodeFailure::Failed(FAILED_MESSAGE.into()),
            ),
            // OS 側の取り消し・その他
            (
                || Err(DeviceError::Internal("geocoding_search")),
                GeocodeFailure::Failed(FAILED_MESSAGE.into()),
            ),
            (
                || Err(DeviceError::InvalidInput("geocoding_search address")),
                GeocodeFailure::Failed(FAILED_MESSAGE.into()),
            ),
        ];
        for (f, expect) in cases {
            let s = run(fake(f));
            assert_eq!(failed(&s), &expect);
        }
    }

    #[test]
    fn out_of_range_result_from_implementation_is_not_success() {
        let s = run(fake(|| {
            Ok(GeocodeResult {
                latitude: 91.0,
                longitude: 0.0,
                candidates: 1,
            })
        }));
        assert_eq!(failed(&s), &GeocodeFailure::NoCoordinate);
        let s = run(fake(|| {
            Ok(GeocodeResult {
                latitude: 0.0,
                longitude: 0.0,
                candidates: 0,
            })
        }));
        assert_eq!(failed(&s), &GeocodeFailure::NotFound);
    }

    #[test]
    fn internal_ids_never_reach_screen_text() {
        let s = run(fake(|| Err(DeviceError::Internal("secret os detail"))));
        let text = fandhe_core::screens::geocoding_search_show::error_text(&s);
        assert!(
            !text.contains("secret") && !text.contains("geocoding_search "),
            "{text}"
        );
    }

    #[test]
    fn worker_past_deadline_is_timeout() {
        let imp = fake_with(ok, Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        assert_eq!(failed(&state), &GeocodeFailure::Timeout);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp = fake_with(ok, Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        let deadline = Duration::from_millis(30);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failed(&state), &GeocodeFailure::Timeout);
        // 遅れて届いたワーカーの結果は無視される（スロットは空・追加の完了なし）
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failed(&state), &GeocodeFailure::Timeout);
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        mark_running(&mut state);
        finish_geocode(&mut state, 99, Ok((1.0, 2.0, 1)));
        assert_eq!(state.phase, GeocodePhase::Running);
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(ok);
        let session = Session::default();
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        // mark_running 1 件のみ（2 回目は発送されない）
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(ok);
        let session = Session::default();
        session.slot.begin();
        let mut state = GeocodingSearchState::default();
        request_geocode(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.slot.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedForwardGeocoder));
        assert_eq!(failed(&s), &GeocodeFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        assert!(matches!(
            platform().geocode("x", Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
