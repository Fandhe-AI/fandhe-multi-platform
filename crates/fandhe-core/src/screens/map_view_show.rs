//! Phase 11（端末機能の拡充ループ、親 #85）型サンプル「地図表示」（Issue #273）。
//! 地点を選ぶと OS が決まった範囲の地図を静止画 1 枚で返し、画面に描く。
//! `FANDHE_PROXY_APP=map_view_show` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod map_view_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。座標の検証と OS 呼び出しは effect
//! ドライバ（`device/map_view.rs`）が [`take_request`] で要求を観測して行い、[`finish`] で結果を
//! 注入する。本画面は二重要求防止・世代照合・文言だけを持つ。全 OS 分の文言は (a) で確定する。
//!
//! # 結果を混同しない理由
//!
//! オフライン・非対応・失敗を黙って地図表示に丸めない。画像は [`MapOutcome::Shown`] のときだけ
//! 持つ。OS が判定した正常な結果（オフライン・非対応）は `status`、失敗は `error`
//! （`TextRole::Error`）へ書き分ける。世代番号で古い完了や `Idle` で届いた完了を無視する。

use crate::{RgbaPixels, Screen, WidgetId};

/// 取得する画像の幅（ピクセル）。ドライバと OS 側の受け口が同じ値を使う。
pub const MAP_WIDTH_PX: u32 = 480;
/// 取得する画像の高さ（ピクセル）。
pub const MAP_HEIGHT_PX: u32 = 320;

/// 読み込み中の案内。
pub const LOADING_TEXT: &str = "地図を読み込み中…";
/// 地図データの帰属表示（固定文言。ストア配布前に HIG・規約を人が確認する）。
pub const ATTRIBUTION_TEXT: &str = "地図データ: Apple マップ";
/// オフラインの案内。
pub const OFFLINE_TEXT: &str = "ネットワークに接続されていないため地図を取得できません";
/// 非対応環境の案内。
pub const UNSUPPORTED_TEXT: &str = "この環境では地図を表示できません";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "地図を取得できませんでした: ";

/// 選べる地点（公開の地名）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapPreset {
    /// ボタンと代替テキストに使う地名。
    pub label: &'static str,
    /// 中心の緯度。
    pub center_lat: f64,
    /// 中心の経度。
    pub center_lon: f64,
    /// 範囲（メートル）。
    pub span_m: f64,
}

/// 地点の一覧。
pub const PRESETS: [MapPreset; 3] = [
    MapPreset {
        label: "東京駅",
        center_lat: 35.6812,
        center_lon: 139.7671,
        span_m: 1000.0,
    },
    MapPreset {
        label: "大阪城",
        center_lat: 34.6873,
        center_lon: 135.5262,
        span_m: 1000.0,
    },
    MapPreset {
        label: "札幌市時計台",
        center_lat: 43.0629,
        center_lon: 141.3537,
        span_m: 1000.0,
    },
];

const PRESET_BUTTON_IDS: [WidgetId; 3] = [
    WidgetId::from_raw(1),
    WidgetId::from_raw(2),
    WidgetId::from_raw(3),
];
const RETRY_BUTTON_ID: WidgetId = WidgetId::from_raw(4);

/// 取得操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapPhase {
    /// 未操作・前回の操作が完了済み。
    #[default]
    Idle,
    /// 地点が選ばれ、effect ドライバへまだ渡していない。
    Requested,
    /// effect ドライバが処理中（[`mark_loading`] 済み）。
    Loading,
}

/// OS が判定した結果（写像は `fandhe-egui-proxy` 側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapOutcome {
    /// 地図の画像が得られた。
    Shown(RgbaPixels),
    /// 端末側で通信が使えない。
    Offline,
    /// この環境では非対応。
    Unsupported,
}

/// effect ドライバへ渡す要求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRequest {
    /// [`finish`] へそのまま渡す世代番号。
    pub generation: u64,
    /// [`PRESETS`] の添字。
    pub preset: usize,
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapViewShowState {
    /// 進行状態。
    pub phase: MapPhase,
    /// 直近の要求の世代番号。
    pub generation: u64,
    /// 直近に選んだ地点の添字。
    pub selected: usize,
    /// 取得できた地図（[`MapOutcome::Shown`] のときだけ持つ）。
    pub image: Option<RgbaPixels>,
    /// OS が判定した結果の案内（通常色）。
    pub status: String,
    /// 失敗（`TextRole::Error`）。
    pub error: String,
}

/// 地点を選んで取得を要求する。`Idle` 以外は二重要求防止のため no-op。範囲外の添字も no-op。
pub fn request(state: &mut MapViewShowState, preset: usize) {
    if state.phase != MapPhase::Idle || preset >= PRESETS.len() {
        return;
    }
    state.phase = MapPhase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.selected = preset;
    state.image = None;
    state.status = String::new();
    state.error = String::new();
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &MapViewShowState) -> Option<MapRequest> {
    (state.phase == MapPhase::Requested).then_some(MapRequest {
        generation: state.generation,
        preset: state.selected,
    })
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_loading(state: &mut MapViewShowState) {
    if state.phase == MapPhase::Requested {
        state.phase = MapPhase::Loading;
    }
}

/// 結果を反映する。`Idle` で届いた完了・世代が違う完了は無視する。
pub fn finish(state: &mut MapViewShowState, generation: u64, result: Result<MapOutcome, String>) {
    if state.phase == MapPhase::Idle || state.generation != generation {
        return;
    }
    state.phase = MapPhase::Idle;
    state.image = None;
    state.status = String::new();
    state.error = String::new();
    match result {
        Ok(MapOutcome::Shown(pixels)) => state.image = Some(pixels),
        Ok(MapOutcome::Offline) => state.status = OFFLINE_TEXT.to_string(),
        Ok(MapOutcome::Unsupported) => state.status = UNSUPPORTED_TEXT.to_string(),
        Err(m) => state.error = format!("{FAILED_PREFIX}{m}"),
    }
}

/// 案内文言（`DynamicText` の `get`）。処理中は読み込み中の文言を返す。
pub fn status_text(state: &MapViewShowState) -> String {
    if state.phase != MapPhase::Idle {
        return LOADING_TEXT.to_string();
    }
    state.status.clone()
}

/// エラー文言（`error_text` の `get`）。
pub fn error_text(state: &MapViewShowState) -> String {
    state.error.clone()
}

/// 画面を組み立てる。処理中はボタンを外して二重要求を防ぐ。
pub fn current_screen(state: &MapViewShowState) -> Screen<MapViewShowState> {
    let mut screen = Screen::new("地図表示")
        .text("選んだ地点の地図を、OS の地図機能から静止画で 1 枚取得して表示します")
        .dynamic_text(status_text)
        .error_text(error_text);
    if let Some(pixels) = &state.image {
        let label = PRESETS.get(state.selected).map_or("", |p| p.label);
        screen = screen
            .rgba_image(pixels.clone(), format!("{label} 周辺の地図"))
            .text(ATTRIBUTION_TEXT);
    }
    if state.phase != MapPhase::Idle {
        return screen;
    }
    screen
        .button(PRESET_BUTTON_IDS[0], PRESETS[0].label, |s| request(s, 0))
        .button(PRESET_BUTTON_IDS[1], PRESETS[1].label, |s| request(s, 1))
        .button(PRESET_BUTTON_IDS[2], PRESETS[2].label, |s| request(s, 2))
        .button(RETRY_BUTTON_ID, "取り直す", |s| {
            let i = s.selected;
            request(s, i)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loading() -> MapViewShowState {
        let mut s = MapViewShowState::default();
        request(&mut s, 1);
        mark_loading(&mut s);
        s
    }

    fn pixels() -> RgbaPixels {
        RgbaPixels::new(1, 1, vec![0u8; 4]).unwrap()
    }

    fn shown(r: Result<MapOutcome, String>) -> MapViewShowState {
        let mut s = loading();
        let g = s.generation;
        finish(&mut s, g, r);
        assert_eq!(s.phase, MapPhase::Idle);
        s
    }

    #[test]
    fn results_have_distinct_text_and_image_only_when_shown() {
        let all = [
            shown(Ok(MapOutcome::Shown(pixels()))),
            shown(Ok(MapOutcome::Offline)),
            shown(Ok(MapOutcome::Unsupported)),
            shown(Err("x".into())),
        ];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(a.image.is_some(), i == 0, "index {i}");
            for b in &all[i + 1..] {
                assert_ne!((&a.status, &a.error), (&b.status, &b.error));
            }
        }
        assert!(all[3].error.starts_with(FAILED_PREFIX));
    }

    #[test]
    fn stale_and_idle_completions_are_ignored() {
        let mut s = loading();
        let g = s.generation + 1;
        finish(&mut s, g, Ok(MapOutcome::Shown(pixels())));
        assert_eq!(s.phase, MapPhase::Loading);
        assert!(s.image.is_none());

        let mut idle = MapViewShowState::default();
        finish(&mut idle, 0, Ok(MapOutcome::Shown(pixels())));
        assert!(idle.image.is_none());
    }

    #[test]
    fn request_is_noop_in_flight_and_clears_previous() {
        let mut s = loading();
        let g = s.generation;
        request(&mut s, 2);
        assert_eq!(
            (s.phase, s.generation, s.selected),
            (MapPhase::Loading, g, 1)
        );

        let mut s = MapViewShowState {
            image: Some(pixels()),
            status: "a".into(),
            error: "e".into(),
            ..Default::default()
        };
        request(&mut s, 0);
        assert!(s.image.is_none() && s.status.is_empty() && s.error.is_empty());
        assert_eq!(s.phase, MapPhase::Requested);

        let mut s = MapViewShowState::default();
        request(&mut s, PRESETS.len());
        assert_eq!(s.phase, MapPhase::Idle);
    }

    #[test]
    fn take_request_only_when_requested() {
        let mut s = MapViewShowState::default();
        assert!(take_request(&s).is_none());
        request(&mut s, 2);
        let r = take_request(&s).unwrap();
        assert_eq!((r.generation, r.preset), (s.generation, 2));
        mark_loading(&mut s);
        assert!(take_request(&s).is_none());
        assert_eq!(status_text(&s), LOADING_TEXT);
    }

    #[test]
    fn presets_are_within_valid_region_range() {
        for p in PRESETS {
            assert!((-90.0..=90.0).contains(&p.center_lat));
            assert!((-180.0..=180.0).contains(&p.center_lon));
            assert!((100.0..=1_000_000.0).contains(&p.span_m));
        }
    }
}
