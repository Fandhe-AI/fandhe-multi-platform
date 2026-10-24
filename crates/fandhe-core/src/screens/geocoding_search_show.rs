//! Phase 11（端末機能の拡充ループ、親 #133）型サンプル「住所の座標変換」（Issue #691）。
//! 画面に埋め込んだ固定の公開住所 1 件を OS の地理コードで座標へ変換し、先頭候補の緯度・経度と
//! 候補数を表示する 1 画面。`FANDHE_PROXY_APP=geocoding_search_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod geocoding_search_show;` と対）。経路・地点検索・逆ジオコーディング・
//! 利用者の入力・現在地は扱わない。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/geocoding_search.rs`）が [`take_request`] で
//! 要求を観測し、[`mark_running`]・[`finish_geocode`] で状態を進める。全対象 OS（macOS / iOS /
//! Android）で同じ UI・文言を使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_geocode（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const GEOCODE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// 変換する固定の住所（公開の施設所在地。利用者の入力は受けない）。
pub const ADDRESS: &str = "東京都千代田区丸の内一丁目9番1号";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "端末の地理コードで、次の住所を 1 回だけ座標に変換して表示します";
/// 変換を待っている間の案内。
pub const RUNNING_STATUS: &str = "変換しています…";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 通信できないときの案内。
pub const OFFLINE_MESSAGE: &str = "通信できないため変換できません";
/// 回数上限に達したときの案内。
pub const THROTTLED_MESSAGE: &str =
    "短時間に何度も変換したため、しばらく待ってからやり直してください";
/// 該当する場所が無いときの案内。
pub const NOT_FOUND_MESSAGE: &str = "該当する場所が見つかりませんでした";
/// 座標が得られなかったときの案内。
pub const NO_COORDINATE_MESSAGE: &str = "場所は見つかりましたが座標がありません";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に変換が終わりませんでした";
/// 候補数として数える上限（`fandhe_device::geocoding_search::MAX_CANDIDATES` と同じ値）。
pub const MAX_CANDIDATES_SHOWN: u32 = 5;

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeocodeFailure {
    /// 非対応 OS・OS が古い・provider が無い。
    Unsupported,
    /// 通信できない。
    Offline,
    /// 回数上限。
    Throttled,
    /// 該当なし。
    NotFound,
    /// 座標なし。
    NoCoordinate,
    /// 期限切れ。
    Timeout,
    /// それ以外（表示用の固定文言）。
    Failed(String),
}

/// 操作の進行状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum GeocodePhase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// 変換待ち。
    Running,
    /// 成功（先頭候補の座標と候補数）。
    Done {
        /// 緯度（度）。
        latitude: f64,
        /// 経度（度）。
        longitude: f64,
        /// 候補数。
        candidates: u32,
    },
    /// 失敗。
    Failed(GeocodeFailure),
}

/// 住所の座標変換画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeocodingSearchState {
    /// 操作の進行状態。
    pub phase: GeocodePhase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
}

fn is_busy(state: &GeocodingSearchState) -> bool {
    matches!(state.phase, GeocodePhase::Requested | GeocodePhase::Running)
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_geocode(state: &mut GeocodingSearchState) {
    if is_busy(state) {
        return;
    }
    state.phase = GeocodePhase::Requested;
    state.generation = state.generation.wrapping_add(1);
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_geocode`] へ
/// そのまま渡す。
pub fn take_request(state: &GeocodingSearchState) -> Option<u64> {
    matches!(state.phase, GeocodePhase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut GeocodingSearchState) {
    if state.phase == GeocodePhase::Requested {
        state.phase = GeocodePhase::Running;
    }
}

/// 結果 `(緯度, 経度, 候補数)` を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外
/// （古い世代・期限で確定済み）は無視する。判定と更新はこの関数の中で完結する。
pub fn finish_geocode(
    state: &mut GeocodingSearchState,
    generation: u64,
    result: Result<(f64, f64, u32), GeocodeFailure>,
) {
    if state.phase != GeocodePhase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok((latitude, longitude, candidates)) => GeocodePhase::Done {
            latitude,
            longitude,
            candidates,
        },
        Err(f) => GeocodePhase::Failed(f),
    };
}

/// 処理中の案内文言。
pub fn status_text(state: &GeocodingSearchState) -> String {
    if is_busy(state) {
        RUNNING_STATUS.to_string()
    } else {
        String::new()
    }
}

/// 成功時の結果文言（緯度・経度は小数 5 桁、候補数は上限まで）。
pub fn result_text(state: &GeocodingSearchState) -> String {
    match &state.phase {
        GeocodePhase::Done {
            latitude,
            longitude,
            candidates,
        } => {
            let shown = (*candidates).min(MAX_CANDIDATES_SHOWN);
            let more = if *candidates >= MAX_CANDIDATES_SHOWN {
                "以上"
            } else {
                ""
            };
            format!("緯度 {latitude:.5}、経度 {longitude:.5}（候補 {shown} 件{more}）")
        }
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &GeocodingSearchState) -> String {
    let GeocodePhase::Failed(f) = &state.phase else {
        return String::new();
    };
    let message = match f {
        GeocodeFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        GeocodeFailure::Offline => OFFLINE_MESSAGE,
        GeocodeFailure::Throttled => THROTTLED_MESSAGE,
        GeocodeFailure::NotFound => NOT_FOUND_MESSAGE,
        GeocodeFailure::NoCoordinate => NO_COORDINATE_MESSAGE,
        GeocodeFailure::Timeout => TIMEOUT_MESSAGE,
        GeocodeFailure::Failed(m) => m,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn address_line(_: &GeocodingSearchState) -> String {
    format!("住所: {ADDRESS}")
}

fn result_nodes(state: &GeocodingSearchState) -> Vec<Node<GeocodingSearchState>> {
    let t = result_text(state);
    if t.is_empty() {
        Vec::new()
    } else {
        vec![Node::Text(t)]
    }
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &GeocodingSearchState) -> Screen<GeocodingSearchState> {
    let screen = Screen::new("住所の座標変換")
        .text(IDLE_HINT)
        .dynamic_text(address_line)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(result_nodes);
    if is_busy(state) {
        return screen;
    }
    screen.button(GEOCODE_BUTTON_ID, "住所を座標に変換", request_geocode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> GeocodingSearchState {
        let mut s = GeocodingSearchState::default();
        request_geocode(&mut s);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = GeocodingSearchState::default();
        request_geocode(&mut s);
        assert_eq!(take_request(&s), Some(1));
        request_geocode(&mut s);
        assert_eq!(s.generation, 1);
        mark_running(&mut s);
        request_geocode(&mut s);
        assert_eq!(s.generation, 1);
        finish_geocode(&mut s, 1, Ok((35.0, 139.0, 2)));
        assert!(!result_text(&s).is_empty());
        request_geocode(&mut s);
        assert_eq!(s.generation, 2);
        assert_eq!(result_text(&s), "");
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish_geocode(&mut s, 0, Ok((1.0, 2.0, 1)));
        assert_eq!(s.phase, GeocodePhase::Running);
        finish_geocode(&mut s, 1, Err(GeocodeFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish_geocode(&mut s, 1, Ok((1.0, 2.0, 1)));
        assert_eq!(s.phase, GeocodePhase::Failed(GeocodeFailure::Timeout));
        let mut idle = GeocodingSearchState::default();
        finish_geocode(&mut idle, 0, Ok((1.0, 2.0, 1)));
        assert_eq!(idle.phase, GeocodePhase::Idle);
    }

    #[test]
    fn mark_running_only_from_requested() {
        let mut s = GeocodingSearchState::default();
        mark_running(&mut s);
        assert_eq!(s.phase, GeocodePhase::Idle);
    }

    #[test]
    fn every_failure_has_nonempty_text() {
        let all = [
            GeocodeFailure::Unsupported,
            GeocodeFailure::Offline,
            GeocodeFailure::Throttled,
            GeocodeFailure::NotFound,
            GeocodeFailure::NoCoordinate,
            GeocodeFailure::Timeout,
            GeocodeFailure::Failed("x".into()),
        ];
        for f in all {
            let mut s = running();
            finish_geocode(&mut s, 1, Err(f.clone()));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(result_text(&s), "");
        }
        let mut s = running();
        finish_geocode(&mut s, 1, Err(GeocodeFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
    }

    #[test]
    fn result_text_format() {
        let mut s = running();
        finish_geocode(&mut s, 1, Ok((35.681236, 139.767129, 1)));
        assert_eq!(
            result_text(&s),
            "緯度 35.68124、経度 139.76713（候補 1 件）"
        );
        let mut s = running();
        finish_geocode(&mut s, 1, Ok((-1.0, -2.5, 5)));
        assert_eq!(
            result_text(&s),
            "緯度 -1.00000、経度 -2.50000（候補 5 件以上）"
        );
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 5 ノード（案内・住所・状態・失敗・結果）+ ボタン 1 つ
        let count = |s: &GeocodingSearchState| current_screen(s).len() - 5;
        let mut s = GeocodingSearchState::default();
        assert_eq!(count(&s), 1);
        request_geocode(&mut s);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish_geocode(&mut s, 1, Err(GeocodeFailure::Timeout));
        assert_eq!(count(&s), 1);
    }
}
