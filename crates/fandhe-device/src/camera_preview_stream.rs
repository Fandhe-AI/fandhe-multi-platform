//! カメラのライブ映像の縮小の部品（OS に触れない純関数。Phase 11 の拡充ループ、親 #168、Issue #812）。
//!
//! 機能の契約（`CameraPreview`・`PreviewSession`・イベント・ズームなど）は `fandhe-cap-camera-preview-stream` が持つ。
//! ここに残すのは、2 機能以上が使う縮小の部品だけ（`docs/architecture/consumer-app-structure.md` §8。機能 crate 同士を
//! 依存させない）。`fandhe-cap-ar` は AR の映像の縮小に、`fandhe-web-dom` は Web の `getUserMedia` の映像の縮小に使い、
//! `fandhe-cap-camera-preview-stream` は各 OS 実装の縮小に使う（再公開する）。
//!
//! 画素はプライバシー情報なので、ログ・`Debug` に出さない（[`ScaledFrame`] の `Debug` は寸法だけを出す）。
//!

use std::fmt;

/// 画面へ渡す縮小後フレームの長辺の上限（ピクセル）。
pub const PREVIEW_MAX_SIDE: u32 = 320;

/// OS バッファの寸法の上限（ピクセル）。巨大確保による DoS を防ぐ仮置き値で、実機確認で見直す。
pub const MAX_SOURCE_SIDE: u32 = 8192;

/// 元寸法 `width` x `height` を縦横比を保って長辺 [`PREVIEW_MAX_SIDE`] 以下へ縮めた寸法。
/// すでに収まる寸法はそのまま返す（冪等）。0 を含む入力は `(0, 0)`。
pub fn preview_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let long = width.max(height);
    if long <= PREVIEW_MAX_SIDE {
        return (width, height);
    }
    let scale = |v: u32| ((v as u64 * PREVIEW_MAX_SIDE as u64 / long as u64) as u32).max(1);
    (scale(width), scale(height))
}

// ---- フレーム -------------------------------------------------------------------------------

/// 縮小済みの RGBA フレーム（行優先・パディングなし。`rgba.len() == width * height * 4`）。
#[derive(Clone, PartialEq, Eq)]
pub struct ScaledFrame {
    /// 幅（ピクセル）。
    pub width: u32,
    /// 高さ（ピクセル）。
    pub height: u32,
    /// 画素。
    pub rgba: Vec<u8>,
}

impl fmt::Debug for ScaledFrame {
    /// 画素はプライバシー情報なので寸法と長さだけを出す。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScaledFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
}

/// 入力 RGBA ピクセルの取得関数でブロック平均縮小し、`ScaledFrame` を作る。
fn downscale_with(
    width: usize,
    height: usize,
    px: impl Fn(usize, usize) -> [u8; 4],
) -> ScaledFrame {
    let (ow, oh) = preview_size(width as u32, height as u32);
    let (ow, oh) = (ow as usize, oh as usize);
    let mut out = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        let y0 = oy * height / oh;
        let y1 = ((oy + 1) * height / oh).max(y0 + 1).min(height);
        for ox in 0..ow {
            let x0 = ox * width / ow;
            let x1 = ((ox + 1) * width / ow).max(x0 + 1).min(width);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = px(x, y);
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v as u64;
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| (s / count) as u8));
        }
    }
    ScaledFrame {
        width: ow as u32,
        height: oh as u32,
        rgba: out,
    }
}

fn dims_ok(width: u32, height: u32) -> bool {
    (1..=MAX_SOURCE_SIDE).contains(&width) && (1..=MAX_SOURCE_SIDE).contains(&height)
}

/// 画素の並び（1 画素 4 バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedLayout {
    /// R, G, B, A（Web の `ImageData` 等）。
    Rgba8,
    /// B, G, R, A（iOS の `kCVPixelFormatType_32BGRA`）。
    Bgra8,
}

/// 行ごとのパディング付き 4 バイト画素の画像を縮め、RGBA へ並べ替える。
///
/// `row_stride >= width * 4` と `data.len() >= row_stride * (height - 1) + width * 4` を
/// `checked_*` で検査し、寸法が 0 または [`MAX_SOURCE_SIDE`] 超なら `None`（panic を契約にしない）。
pub fn downscale_packed(
    width: u32,
    height: u32,
    row_stride: usize,
    layout: PackedLayout,
    data: &[u8],
) -> Option<ScaledFrame> {
    if !dims_ok(width, height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let row_bytes = w.checked_mul(4)?;
    if row_stride < row_bytes {
        return None;
    }
    let needed = row_stride.checked_mul(h - 1)?.checked_add(row_bytes)?;
    if data.len() < needed {
        return None;
    }
    Some(downscale_with(w, h, |x, y| {
        let i = y * row_stride + x * 4;
        match layout {
            PackedLayout::Rgba8 => [data[i], data[i + 1], data[i + 2], data[i + 3]],
            PackedLayout::Bgra8 => [data[i + 2], data[i + 1], data[i], data[i + 3]],
        }
    }))
}

/// `YUV_420_888`（Android の `ImageReader`）を BT.601 full range（JFIF）で RGBA へ変換しながら縮める。
///
/// 色差面は縦横 1/2（切り上げ）。各面の stride・長さを `checked_*` で検査し、不正なら `None`。
#[allow(clippy::too_many_arguments)]
pub fn downscale_yuv420(
    width: u32,
    height: u32,
    y: &[u8],
    y_row_stride: usize,
    u: &[u8],
    v: &[u8],
    uv_row_stride: usize,
    uv_pixel_stride: usize,
) -> Option<ScaledFrame> {
    if !dims_ok(width, height) || uv_pixel_stride == 0 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    if y_row_stride < w || y.len() < y_row_stride.checked_mul(h - 1)?.checked_add(w)? {
        return None;
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let row_span = (cw - 1).checked_mul(uv_pixel_stride)?.checked_add(1)?;
    if uv_row_stride < row_span {
        return None;
    }
    let chroma_needed = uv_row_stride.checked_mul(ch - 1)?.checked_add(row_span)?;
    if u.len() < chroma_needed || v.len() < chroma_needed {
        return None;
    }
    Some(downscale_with(w, h, |x, yy| {
        let ci = (yy / 2) * uv_row_stride + (x / 2) * uv_pixel_stride;
        let luma = y[yy * y_row_stride + x] as i32;
        let (cb, cr) = (u[ci] as i32 - 128, v[ci] as i32 - 128);
        let r = luma + ((359 * cr) >> 8);
        let g = luma - ((88 * cb + 183 * cr) >> 8);
        let b = luma + ((454 * cb) >> 8);
        let c = |v: i32| v.clamp(0, 255) as u8;
        [c(r), c(g), c(b), 255]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_size_keeps_aspect_and_is_idempotent() {
        assert_eq!(preview_size(640, 480), (320, 240));
        assert_eq!(preview_size(480, 640), (240, 320));
        assert_eq!(preview_size(100, 50), (100, 50));
        let (w, h) = preview_size(4000, 3000);
        assert_eq!(preview_size(w, h), (w, h));
        assert_eq!(preview_size(0, 10), (0, 0));
        assert_eq!(preview_size(10, 0), (0, 0));
        assert_eq!(preview_size(10000, 1), (320, 1));
    }

    #[test]
    fn packed_bgra_swaps_and_skips_padding() {
        // 2x1、stride 12（4 バイトのパディング）。
        let data = [10, 20, 30, 255, 40, 50, 60, 255, 9, 9, 9, 9];
        let f = downscale_packed(2, 1, 12, PackedLayout::Bgra8, &data).unwrap();
        assert_eq!((f.width, f.height), (2, 1));
        assert_eq!(f.rgba, vec![30, 20, 10, 255, 60, 50, 40, 255]);
        let f = downscale_packed(2, 1, 12, PackedLayout::Rgba8, &data).unwrap();
        assert_eq!(&f.rgba[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn packed_downscale_averages_blocks() {
        let mut data = Vec::new();
        for _ in 0..(640 * 480) {
            data.extend_from_slice(&[100, 100, 100, 255]);
        }
        let f = downscale_packed(640, 480, 640 * 4, PackedLayout::Rgba8, &data).unwrap();
        assert_eq!((f.width, f.height), (320, 240));
        assert_eq!(f.rgba.len(), 320 * 240 * 4);
        assert!(f.rgba.chunks(4).all(|p| p == [100, 100, 100, 255]));
    }

    #[test]
    fn packed_rejects_bad_input() {
        let data = vec![0u8; 16];
        assert!(downscale_packed(2, 2, 7, PackedLayout::Rgba8, &data).is_none());
        assert!(downscale_packed(2, 2, 8, PackedLayout::Rgba8, &data[..15]).is_none());
        assert!(downscale_packed(0, 2, 8, PackedLayout::Rgba8, &data).is_none());
        assert!(
            downscale_packed(
                MAX_SOURCE_SIDE + 1,
                1,
                usize::MAX,
                PackedLayout::Rgba8,
                &data
            )
            .is_none()
        );
        assert!(downscale_packed(2, 2, usize::MAX, PackedLayout::Rgba8, &data).is_none());
    }

    #[test]
    fn yuv_known_colors() {
        // 2x2。白・黒・赤付近を 1 画素ずつ別の呼び出しで確かめる（色差は 2x2 で共有）。
        let conv = |yv: u8, uv: u8, vv: u8| {
            let f = downscale_yuv420(2, 2, &[yv; 4], 2, &[uv], &[vv], 1, 1).unwrap();
            [f.rgba[0], f.rgba[1], f.rgba[2], f.rgba[3]]
        };
        assert_eq!(conv(255, 128, 128), [255, 255, 255, 255]);
        assert_eq!(conv(0, 128, 128), [0, 0, 0, 255]);
        let red = conv(76, 85, 255);
        assert!(red[0] > 240 && red[1] < 10 && red[2] < 10, "{red:?}");
    }

    #[test]
    fn yuv_respects_pixel_stride_and_rejects_bad_input() {
        // 色差が interleaved（pixel_stride 2）の 2x2。
        let y = [128u8; 4];
        let uv = [128u8, 0, 128, 0];
        assert!(downscale_yuv420(2, 2, &y, 2, &uv, &uv, 2, 2).is_some());
        assert!(downscale_yuv420(2, 2, &y, 1, &uv, &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y[..3], 2, &uv, &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y, 2, &uv[..0], &uv, 2, 2).is_none());
        assert!(downscale_yuv420(2, 2, &y, 2, &uv, &uv, 2, 0).is_none());
        // 4x2 で色差の行幅（3）が row_stride（2）に収まらない。
        let big = [128u8; 16];
        assert!(downscale_yuv420(4, 2, &[0u8; 8], 4, &big, &big, 2, 2).is_none());
        assert!(downscale_yuv420(0, 2, &y, 2, &uv, &uv, 2, 2).is_none());
    }
}
