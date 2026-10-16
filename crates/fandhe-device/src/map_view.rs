//! 地図表示（Phase 11 の拡充ループ、親 #85、Issue #273）の契約。
//! 決まった地点の静止画を 1 枚取得して `crate::Image`（RGBA）で返す。ライブ地図・ズーム・パン・
//! 注釈は扱わない。`fandhe-egui-proxy` の effect ドライバ（`device/map_view.rs`）が
//! [`MapSnapshotter`] を呼び、共通コアの画面（`fandhe_core::screens::map_view_show`）へ結果を渡す。
//!
//! # 結果の種類（契約表 10）
//!
//! - 成功: 画像（[`build_snapshot`] を通ったもの）
//! - 失敗: `Unsupported`（非対応 OS）/ `Disabled("map_view network")`（端末側で通信が使えない
//!   = オフライン）/ `Internal`（MapKit の失敗・画像なし・期限切れ）
//! - キャンセル: 作らない（利用者が取り消す UI が無いため）
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス・1 画面の順序、結果は 1 回きりで古い世代の完了は捨てる、画像なし・0 寸法・
//! エラー付きを成功にしない、ネイティブ資源を全経路で解放する、座標をログへ出さない。
//!
//! 守らない: タイルの内容と鮮度、Apple 側のレート制限、複数プロセス、背景にいる間の取得
//! （期限切れの失敗になる）、オンラインなのに空タイルがエラー無しで返る場合（既知の天井）。

use crate::{DeviceError, Image};

/// 取得範囲の下限（メートル）。
pub const MIN_SPAN_M: f64 = 100.0;
/// 取得範囲の上限（メートル）。
pub const MAX_SPAN_M: f64 = 1_000_000.0;

/// 取得する地図の範囲。検証は [`MapRegion::new`] の 1 か所に集約する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapRegion {
    center_lat: f64,
    center_lon: f64,
    span_m: f64,
}

impl MapRegion {
    /// 有限値・緯度 -90..=90・経度 -180..=180・範囲 100 m..=1,000 km を検証する。
    /// 外れたら `InvalidInput("map_view region")`（変換・丸めの前に検証する）。
    pub fn new(center_lat: f64, center_lon: f64, span_m: f64) -> Result<Self, DeviceError> {
        let ok = center_lat.is_finite()
            && center_lon.is_finite()
            && span_m.is_finite()
            && (-90.0..=90.0).contains(&center_lat)
            && (-180.0..=180.0).contains(&center_lon)
            && (MIN_SPAN_M..=MAX_SPAN_M).contains(&span_m);
        if ok {
            Ok(Self {
                center_lat,
                center_lon,
                span_m,
            })
        } else {
            Err(DeviceError::InvalidInput("map_view region"))
        }
    }

    /// 中心の緯度。
    pub fn center_lat(&self) -> f64 {
        self.center_lat
    }

    /// 中心の経度。
    pub fn center_lon(&self) -> f64 {
        self.center_lon
    }

    /// 範囲（メートル、縦横同じ）。
    pub fn span_m(&self) -> f64 {
        self.span_m
    }
}

/// OS が返した画素を検査して [`Image`] にする（不変条件の SSOT）。
/// 0 寸法・要求サイズとの不一致は `InvalidImageSize`、長さ不一致は `InvalidInput("map_view pixels")`。
pub fn build_snapshot(
    requested: (u32, u32),
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Result<Image, DeviceError> {
    if width == 0 || height == 0 || (width, height) != requested {
        return Err(DeviceError::InvalidImageSize(width, height));
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if expected != Some(rgba.len()) {
        return Err(DeviceError::InvalidInput("map_view pixels"));
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// 地図の静止画を取得する OS 実装。結果は非同期に 1 回だけ届く（届け方は実装側の橋渡し）。
pub trait MapSnapshotter: Send + Sync {
    /// 取得を始める。`Ok` は受理しただけを表す。`Err` は依頼自体の失敗で結果は届かない。
    fn start(
        &self,
        request_id: u64,
        region: MapRegion,
        size_px: (u32, u32),
    ) -> Result<(), DeviceError>;

    /// 進行中の要求を取り消す。`request_id` が進行中のものと違えば何もしない。
    fn cancel(&self, request_id: u64);
}

/// 非対応 OS の dispatch 先。ダミー画像は返さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedMapSnapshotter;

impl MapSnapshotter for UnsupportedMapSnapshotter {
    fn start(&self, _: u64, _: MapRegion, _: (u32, u32)) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("map_view"))
    }

    fn cancel(&self, _: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_rejects_out_of_range_and_non_finite() {
        for (a, b, c) in [
            (91.0, 0.0, 500.0),
            (-91.0, 0.0, 500.0),
            (0.0, 181.0, 500.0),
            (0.0, -181.0, 500.0),
            (0.0, 0.0, 99.0),
            (0.0, 0.0, 1_000_001.0),
            (f64::NAN, 0.0, 500.0),
            (0.0, f64::INFINITY, 500.0),
            (0.0, 0.0, f64::NAN),
        ] {
            assert!(matches!(
                MapRegion::new(a, b, c),
                Err(DeviceError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn region_accepts_boundaries() {
        for (a, b, c) in [(90.0, 180.0, 100.0), (-90.0, -180.0, 1_000_000.0)] {
            let r = MapRegion::new(a, b, c).unwrap();
            assert_eq!((r.center_lat(), r.center_lon(), r.span_m()), (a, b, c));
        }
    }

    #[test]
    fn build_snapshot_checks_invariants() {
        let px = |w: usize, h: usize| vec![0u8; w * h * 4];
        assert!(build_snapshot((2, 2), 2, 2, px(2, 2)).is_ok());
        assert!(matches!(
            build_snapshot((0, 2), 0, 2, vec![]),
            Err(DeviceError::InvalidImageSize(0, 2))
        ));
        assert!(matches!(
            build_snapshot((2, 2), 3, 2, px(3, 2)),
            Err(DeviceError::InvalidImageSize(3, 2))
        ));
        assert!(matches!(
            build_snapshot((2, 2), 2, 2, vec![0; 3]),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let r = MapRegion::new(35.0, 139.0, 500.0).unwrap();
        assert!(matches!(
            UnsupportedMapSnapshotter.start(1, r, (4, 4)),
            Err(DeviceError::Unsupported("map_view"))
        ));
    }
}
