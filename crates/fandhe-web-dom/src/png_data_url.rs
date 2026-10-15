//! `fandhe_core::RgbaPixels` を `data:image/png;base64,…` へ変換する（Issue #5）。
//!
//! `Node::RgbaImage` を `<img>` で出すための部品で、[`super::dom_tree`] の `convert_node`
//! から呼ばれる。`DomNode` は文字列しか運べないため画素は data URL に載せ、wasm の
//! `mount` と native の HTML シリアライズ（`html.rs`）で同じ出力になる。
//! 画素は縮小済みの小さなサムネイル（長辺 2048 以下、通常 160）が前提のため、
//! 圧縮はせず stored deflate ブロックで書く（新しい依存を足さず、約 60 行で済ませる。
//! 圧縮が要る画像を載せるときは `png` crate への置き換えを検討する）。

use fandhe_core::RgbaPixels;

/// 画素を無圧縮 PNG の data URL にする。出力は `data:image/png;base64,` と
/// base64 アルファベットのみで、外部由来の文字列は含まない（属性値として安全）。
pub(crate) fn png_data_url(pixels: &RgbaPixels) -> String {
    let (w, h) = (pixels.width(), pixels.height());

    // 各行の先頭にフィルタ種別 0（None）を置いた生データ。
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for row in pixels.rgba().chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    // zlib: ヘッダ + 65535 バイトごとの stored ブロック + Adler32。
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(u8::from(chunks.peek().is_none()));
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    // 8bit・カラータイプ 6（RGBA）・標準圧縮・標準フィルタ・非インターレース。
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);

    format!("data:image/png;base64,{}", base64(&png))
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn png_roundtrips_through_decoder() {
        // 70000 バイト超（stored ブロックが複数に分かれる）を含めて往復を確かめる。
        for (w, h) in [(1u32, 1u32), (3, 2), (200, 100)] {
            let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
            let px = RgbaPixels::new(w, h, rgba.clone()).unwrap();
            let url = png_data_url(&px);
            let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
            let bytes = decode_b64(b64);
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            assert_eq!((img.width(), img.height()), (w, h));
            assert_eq!(img.as_raw(), &rgba);
        }
    }

    fn decode_b64(s: &str) -> Vec<u8> {
        const T: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0;
        for ch in s.chars().filter(|c| *c != '=') {
            buf = buf << 6 | T.find(ch).unwrap() as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
}
