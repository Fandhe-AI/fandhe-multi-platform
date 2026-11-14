//! 素材の WAV 生成（media-playback・media-cast・media-transcode が共有する純関数）。
//!
//! 再生の契約（`MediaPlayback`・`PlaybackSnapshot`・`START_DEADLINE`・`POLL_INTERVAL`・`secs_to_ms` など）は
//! `fandhe-cap-media-playback` が持つ。ここに残すのは、2 機能以上が使う OS に触れない部品だけ
//! （`docs/architecture/consumer-app-structure.md` §8）。`fandhe-cap-media-playback` の `contract` が再公開する。

use std::f32::consts::TAU;
use std::sync::Arc;

/// 素材のサンプルレート（Hz）。
pub const SAMPLE_RATE: u32 = 16_000;
/// 素材の長さ（秒）。
pub const CLIP_SECONDS: u32 = 5;

/// 全 OS 共用の素材の PCM（16bit・モノラル・5 秒）。1 秒ごとに音程を上げる。
/// std の `sin` だけで作り、バイナリ素材をリポジトリに置かない（出典・改ざんの経路を作らない）。
/// `media_transcode` も同じ素材を別のサンプルレートで使う。
pub fn demo_pcm(sample_rate: u32) -> Vec<i16> {
    const FREQS: [f32; CLIP_SECONDS as usize] = [440.0, 494.0, 554.0, 587.0, 659.0];
    let rate = sample_rate as usize;
    let fade = rate / 200; // 5ms
    let mut pcm = Vec::with_capacity(rate * CLIP_SECONDS as usize);
    for f in FREQS {
        for i in 0..rate {
            let edge = i.min(rate - 1 - i);
            let env = if edge < fade {
                edge as f32 / fade as f32
            } else {
                1.0
            };
            let v = (TAU * f * i as f32 / sample_rate as f32).sin() * 0.3 * env;
            pcm.push((v * i16::MAX as f32) as i16);
        }
    }
    pcm
}

/// モノラル PCM 16bit を 44 バイトのヘッダ付き WAV にする。
pub fn wav_bytes(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&sample_rate.to_le_bytes());
    b.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// 全 OS 共用の素材。PCM 16bit・モノラル・16kHz・5 秒（[`demo_pcm`] を [`wav_bytes`] で包む）。
pub fn demo_wav() -> Arc<[u8]> {
    wav_bytes(&demo_pcm(SAMPLE_RATE), SAMPLE_RATE).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `demo_wav` の長さが分割前から変わっていないことの固定。`sin` の値は OS・libm で
    /// 末尾の桁が揺れるため、バイト列のハッシュでは固定しない。
    #[test]
    fn demo_wav_length_is_unchanged() {
        assert_eq!(demo_wav().len(), 160_044);
    }

    #[test]
    fn wav_bytes_header_values() {
        let w = wav_bytes(&[1, -2, 3], 48_000);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 96_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(&w[44..], &[1, 0, 0xfe, 0xff, 3, 0]);
    }

    #[test]
    fn wav_header_and_size() {
        let w = demo_wav();
        let data = (SAMPLE_RATE * CLIP_SECONDS * 2) as usize;
        assert_eq!(w.len(), 44 + data);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(&w[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            36 + data
        );
        assert_eq!(
            u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize,
            data
        );
        assert!(w[44..].iter().any(|&x| x != 0), "無音ではない");
    }
}
