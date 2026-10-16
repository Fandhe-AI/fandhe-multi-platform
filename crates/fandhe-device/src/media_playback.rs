//! Phase 11 の拡充ループ（親 #86、Issue #267）: 音声 1 本の再生・一時停止・停止の契約。
//! `fandhe-egui-proxy` の `device/media_playback.rs`（effect ドライバ）が [`MediaPlayback`] を呼び、
//! 各 OS 実装（macOS: AVAudioPlayer = #267、他 OS は後続の子）が本トレイトを実装する。
//! 素材は [`demo_wav`] が実行時に生成する固定 WAV で、URL・ファイルパスは受けない。
//!
//! 範囲は「同梱 WAV の再生・一時停止・停止・位置・長さ」のみ。動画、バックグラウンド再生、
//! プレイリスト、ネットワーク再生、シークは扱わない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: `play` が `Ok` で [`PlaybackPhase::Playing`] に至ること、最後まで鳴った [`PlaybackPhase::Ended`]
//! - 利用者の一時停止・停止は失敗ではなく成功の状態遷移（`Paused` / `Ready`）
//! - [`PlaybackPhase::Interrupted`] は失敗ではなく中断。復帰しても自動で再開しない
//! - デコード失敗は `Failed(Decode)`。終了の通知と取り違えない
//! - 非対応は `Err(Unsupported("media_playback"))`。成功値へ倒さない
//! - 期限切れ（[`START_DEADLINE`]）はドライバが `Failed(Timeout)` にして `stop` を呼ぶ
//! - Web の `play()` 拒否は `NotAllowedError` を `Blocked`、`AbortError` をキャンセル扱い（失敗にしない）
//!
//! # ライフサイクル（契約表 1）
//!
//! iOS の背景遷移、Android の `onPause`、Web の `pagehide` / `visibilitychange`（hidden）では一時停止して
//! `Interrupted` にする。Android の Activity 作り直しでは `release` まで行う。macOS / Windows は窓が
//! 背面でも再生を続け、終了時は `device::shutdown` から止める。
//!
//! # 世代（契約表 2）
//!
//! `load` ごとに世代を進め、OS コールバックは自分の世代を照合する。判定と更新は同じロックの内側。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、資源の解放。
//! 守らない範囲: OS の非同期な確定と表示の時間差（位置表示がポーリング間隔ぶん遅れる）、
//! 他アプリの音声との混ざり（音声フォーカスや `AVAudioSession` を触らない）、複数プロセス・複数タブ間。

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::Duration;

use crate::DeviceError;

/// 読み込みから `Ready`、`play` から `Playing` までの期限。
pub const START_DEADLINE: Duration = Duration::from_secs(5);
/// 再生中に画面が状態を取り直す間隔。
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 素材のサンプルレート（Hz）。
pub const SAMPLE_RATE: u32 = 16_000;
/// 素材の長さ（秒）。
pub const CLIP_SECONDS: u32 = 5;

/// 再生の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackPhase {
    /// 未読み込み。
    Idle,
    /// 読み込み中（確定待ち）。
    Loading,
    /// 読み込み済みで停止中（先頭）。
    Ready,
    /// 再生中。
    Playing,
    /// 一時停止中。
    Paused,
    /// 最後まで鳴った。
    Ended,
    /// OS による中断（背景遷移・割り込み）。失敗ではない。
    Interrupted,
    /// 失敗。
    Failed(PlaybackFailure),
}

/// 失敗の種別。OS の生コードは持たない（security.md A09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackFailure {
    /// 非対応。
    Unsupported,
    /// 自動再生がブロックされた（Web）。
    Blocked,
    /// デコードできない。
    Decode,
    /// 期限切れ。
    Timeout,
    /// それ以外。
    Internal,
}

/// 最新状態のスナップショット。`generation` は `load` が返した世代。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackSnapshot {
    /// 状態が属する世代。
    pub generation: u64,
    /// 進行状態。
    pub phase: PlaybackPhase,
    /// 再生位置（ミリ秒）。未取得は `None`。
    pub position_ms: Option<u64>,
    /// 長さ（ミリ秒）。未取得は `None`。
    pub duration_ms: Option<u64>,
}

/// 音声再生。
pub trait MediaPlayback: Send + Sync {
    /// WAV を読み込む。呼ぶたびに世代を進め、前のプレーヤーを解放してから作る。
    /// 0 バイトは `InvalidInput("media_playback wav")`。失敗経路でも作りかけの資源を解放する。
    fn load(&self, wav: Arc<[u8]>) -> Result<u64, DeviceError>;
    /// 再生する。古い世代は `InvalidInput("media_playback generation")`。
    fn play(&self, generation: u64) -> Result<(), DeviceError>;
    /// 一時停止する（位置は保つ）。古い世代は `InvalidInput`。
    fn pause(&self, generation: u64) -> Result<(), DeviceError>;
    /// 一時停止して先頭へ戻す。古い世代は `InvalidInput`。
    fn stop(&self, generation: u64) -> Result<(), DeviceError>;
    /// 最新状態。
    fn snapshot(&self) -> PlaybackSnapshot;
    /// 再生要求の進み具合 `(発行数, 確認済み数)`。`play` を非同期に実行する実装だけが `Some` を返し、
    /// 呼び出し側は「確認済み数 >= 発行数」になるまで、前の `Playing` を新しい再生要求の成功と
    /// みなさない。確認済み数は操作の実行数ではなく、要求後に再生開始を確認できた数（新しい再生開始通知など）。同期で反映する実装は `None`（既定）。
    fn play_progress(&self) -> Option<(u64, u64)> {
        None
    }
    /// アプリ終了時の解放。冪等。
    fn shutdown(&self);
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedMediaPlayback;

impl MediaPlayback for UnsupportedMediaPlayback {
    fn load(&self, _wav: Arc<[u8]>) -> Result<u64, DeviceError> {
        Err(DeviceError::Unsupported("media_playback"))
    }
    fn play(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_playback"))
    }
    fn pause(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_playback"))
    }
    fn stop(&self, _generation: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("media_playback"))
    }
    fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            generation: 0,
            phase: PlaybackPhase::Idle,
            position_ms: None,
            duration_ms: None,
        }
    }
    fn shutdown(&self) {}
}

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

/// 秒（OS の `double`）をミリ秒へ。NaN・無限大・負値は未取得として `None`（丸めの前に検証する）。
pub fn secs_to_ms(secs: f64) -> Option<u64> {
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0).floor() as u64)
}

/// ミリ秒（Android の `-1` 等の未取得表現を含む `i64`）を `u64` へ。負値は `None`。
pub fn ms_from_i64(ms: i64) -> Option<u64> {
    u64::try_from(ms).ok()
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

    #[test]
    fn secs_to_ms_rejects_invalid_before_rounding() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.001] {
            assert_eq!(secs_to_ms(bad), None);
        }
        assert_eq!(secs_to_ms(0.0), Some(0));
        assert_eq!(secs_to_ms(1.2345), Some(1234));
        assert_eq!(ms_from_i64(-1), None);
        assert_eq!(ms_from_i64(7), Some(7));
    }

    #[test]
    fn unsupported_never_succeeds() {
        let p = UnsupportedMediaPlayback;
        assert!(matches!(
            p.load(demo_wav()),
            Err(DeviceError::Unsupported("media_playback"))
        ));
        assert!(p.play(1).is_err() && p.pause(1).is_err() && p.stop(1).is_err());
        assert_eq!(p.snapshot().phase, PlaybackPhase::Idle);
    }
}
