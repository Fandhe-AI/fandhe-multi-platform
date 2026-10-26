//! 型サンプル「画面のリフレッシュレート・HDR・広色域」の Web 経路（Issue #873、親 #177）。
//!
//! `fandhe_core::screens::display_refresh_hdr_show`（端末依存ゼロ）を [`super::mount::mount_with_effect`]
//! の effect として観測し、ボタンを押すたびに `matchMedia` を読んで `finish_read` へ書き戻す。
//! `DisplayRefreshHdr` トレイトは経由しない（`device_info` と同じ構成）。
//! `fandhe-egui-proxy` の wasm canvas 経路（`Unsupported` のまま）とは別経路。
//!
//! # 読み方
//!
//! - HDR: `(dynamic-range: high)`（否定側は `standard`）
//! - 広色域: `(color-gamut: p3)`（否定側は `srgb`）
//! - 今の Hz・最大 Hz: 標準 API が無いため常に `None`。`requestAnimationFrame` の間隔から推定しない
//!
//! 肯定・否定の両方が偽、または `"not all"` / null は「取れない」（`accessibility_prefs::pref_from`）。
//!
//! # 結果の写像
//!
//! - `window` が無い: `Unsupported`
//! - `match_media` が例外: 固定文言の `Failed`（例外の `message` は画面にも `console` にも出さない。security.md A09）
//! - HDR・広色域がともに取れない: 固定文言の `Failed`（嘘の成功の禁止。`require_any`）
//!
//! 呼び出しは同期。クエリは固定リテラルのみで、外部入力から組み立てない（A03）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（発送済み世代の記録と `finish_read` の世代照合）、
//! 取れない・いいえ・失敗・非対応の区別、Hz を推定しないこと、押下ごとに読み直すこと、
//! マウント破棄時の記録解放。
//! 守らない: 読んだ後の変化の追従（`change` は購読しない）、窓が複数画面にまたがるときの画面の選び方、
//! ブラウザと出力機器の組による「HDR」「広色域」の意味の差、複数タブ、wasm canvas 経路。

use fandhe_core::screens::display_refresh_hdr_show::{DisplayReading, ReadFailure};
use fandhe_device::display_refresh_hdr::{require_any, DisplayCapabilities};

use crate::accessibility_prefs::pref_from;

/// 失敗時の固定文言（例外の内容は含めない）。egui-proxy 側と揃える。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const READ_FAILED: &str = "画面の表示能力の読み取り";

/// `matchMedia` の `(media(), matches())` 4 本を表示用の読み取り結果へ写す純関数。
/// native からもテストできるよう `wasm32` gate の外に置く。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn reading_from_raw(
    hdr_high: Option<(&str, bool)>,
    hdr_standard: Option<(&str, bool)>,
    gamut_p3: Option<(&str, bool)>,
    gamut_srgb: Option<(&str, bool)>,
) -> Result<DisplayReading, ReadFailure> {
    let caps = DisplayCapabilities {
        current_hz: None,
        max_hz: None,
        hdr: pref_from(hdr_high, hdr_standard),
        wide_gamut: pref_from(gamut_p3, gamut_srgb),
    };
    let c = require_any(caps).map_err(|_| ReadFailure::Failed(READ_FAILED.to_string()))?;
    Ok(DisplayReading {
        current_hz: c.current_hz,
        max_hz: c.max_hz,
        hdr: c.hdr,
        wide_gamut: c.wide_gamut,
    })
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::display_refresh_hdr_show::{
        finish_read, take_request, DisplayReading, DisplayRefreshHdrShowState, ReadFailure,
    };

    use super::{reading_from_raw, READ_FAILED};
    use crate::mount::Applier;

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に読まない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    fn mq(w: &web_sys::Window, q: &str) -> Result<Option<(String, bool)>, ReadFailure> {
        let mql = w
            .match_media(q)
            .map_err(|_| ReadFailure::Failed(READ_FAILED.to_string()))?;
        Ok(mql.map(|m| (m.media(), m.matches())))
    }

    fn read_all(w: &web_sys::Window) -> Result<DisplayReading, ReadFailure> {
        let a = mq(w, "(dynamic-range: high)")?;
        let b = mq(w, "(dynamic-range: standard)")?;
        let c = mq(w, "(color-gamut: p3)")?;
        let d = mq(w, "(color-gamut: srgb)")?;
        reading_from_raw(borrow(&a), borrow(&b), borrow(&c), borrow(&d))
    }

    fn borrow(x: &Option<(String, bool)>) -> Option<(&str, bool)> {
        x.as_ref().map(|(s, v)| (s.as_str(), *v))
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ読む。
    pub fn display_refresh_hdr_show_effect(
        state: &DisplayRefreshHdrShowState,
        applier: &Applier<DisplayRefreshHdrShowState>,
    ) {
        let Some(req) = take_request(state) else {
            return;
        };
        let generation = req.generation;
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        let result = match web_sys::window() {
            None => Err(ReadFailure::Unsupported),
            Some(w) => read_all(&w),
        };
        applier.apply(move |s| finish_read(s, generation, result));
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::display_refresh_hdr_show_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::display_refresh_hdr_show::{
        finish_read, read, take_request, DisplayRefreshHdrShowState, DONE_STATUS, FAILED_PREFIX,
    };

    const HIGH: &str = "(dynamic-range: high)";
    const STD: &str = "(dynamic-range: standard)";
    const P3: &str = "(color-gamut: p3)";
    const SRGB: &str = "(color-gamut: srgb)";

    fn failed() -> Result<DisplayReading, ReadFailure> {
        Err(ReadFailure::Failed(READ_FAILED.to_string()))
    }

    #[test]
    fn hz_is_always_none_and_true_maps_to_yes() {
        let r = reading_from_raw(
            Some((HIGH, true)),
            Some((STD, true)),
            Some((P3, true)),
            Some((SRGB, true)),
        )
        .unwrap();
        assert_eq!((r.current_hz, r.max_hz), (None, None));
        assert_eq!((r.hdr, r.wide_gamut), (Some(true), Some(true)));
    }

    #[test]
    fn standard_only_is_no() {
        let r = reading_from_raw(
            Some((HIGH, false)),
            Some((STD, true)),
            Some((P3, false)),
            Some((SRGB, true)),
        )
        .unwrap();
        assert_eq!((r.hdr, r.wide_gamut), (Some(false), Some(false)));
    }

    #[test]
    fn unknown_on_one_side_still_ok() {
        let r = reading_from_raw(
            Some((HIGH, true)),
            Some((STD, true)),
            Some(("not all", false)),
            None,
        )
        .unwrap();
        assert_eq!((r.hdr, r.wide_gamut), (Some(true), None));
    }

    #[test]
    fn nothing_readable_is_failure() {
        assert_eq!(reading_from_raw(None, None, None, None), failed());
        // 両方偽（特性未対応・sRGB 未満）は「取れない」。
        assert_eq!(
            reading_from_raw(
                Some((HIGH, false)),
                Some((STD, false)),
                Some((P3, false)),
                Some((SRGB, false)),
            ),
            failed()
        );
    }

    #[test]
    fn flow_reaches_status_or_error() {
        let mut s = DisplayRefreshHdrShowState::default();
        read(&mut s);
        let g = take_request(&s).unwrap().generation;
        finish_read(
            &mut s,
            g,
            reading_from_raw(Some((HIGH, true)), Some((STD, true)), None, None),
        );
        assert_eq!(s.status, DONE_STATUS);
        assert!(s.reading.is_some());

        read(&mut s);
        let g = take_request(&s).unwrap().generation;
        finish_read(&mut s, g, reading_from_raw(None, None, None, None));
        assert!(s.reading.is_none());
        assert!(s.error.starts_with(FAILED_PREFIX));
    }
}
