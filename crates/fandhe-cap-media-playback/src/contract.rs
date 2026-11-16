//! Phase 11 の拡充ループ（親 #86、Issue #267）: 音声 1 本の再生・一時停止・停止の契約。
//! 本 crate の `driver`（effect ドライバ）が [`MediaPlayback`] を呼び、
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

use std::sync::Arc;
use std::time::Duration;

use crate::error::MediaPlaybackError;

/// 読み込みから `Ready`、`play` から `Playing` までの期限。
pub const START_DEADLINE: Duration = Duration::from_secs(5);
/// 再生中に画面が状態を取り直す間隔。
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

// 素材（WAV）の生成は media-cast・media-transcode も使う OS に触れない部品なので `fandhe-device` に残してあり、ここから再公開する
// （consumer-app-structure.md §8）。
pub use fandhe_device::media_playback::{CLIP_SECONDS, SAMPLE_RATE, demo_pcm, demo_wav, wav_bytes};

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
    fn load(&self, wav: Arc<[u8]>) -> Result<u64, MediaPlaybackError>;
    /// 再生する。古い世代は `InvalidInput("media_playback generation")`。
    fn play(&self, generation: u64) -> Result<(), MediaPlaybackError>;
    /// 一時停止する（位置は保つ）。古い世代は `InvalidInput`。
    fn pause(&self, generation: u64) -> Result<(), MediaPlaybackError>;
    /// 一時停止して先頭へ戻す。古い世代は `InvalidInput`。
    fn stop(&self, generation: u64) -> Result<(), MediaPlaybackError>;
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
    fn load(&self, _wav: Arc<[u8]>) -> Result<u64, MediaPlaybackError> {
        Err(MediaPlaybackError::Unsupported)
    }
    fn play(&self, _generation: u64) -> Result<(), MediaPlaybackError> {
        Err(MediaPlaybackError::Unsupported)
    }
    fn pause(&self, _generation: u64) -> Result<(), MediaPlaybackError> {
        Err(MediaPlaybackError::Unsupported)
    }
    fn stop(&self, _generation: u64) -> Result<(), MediaPlaybackError> {
        Err(MediaPlaybackError::Unsupported)
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
            Err(MediaPlaybackError::Unsupported)
        ));
        assert!(p.play(1).is_err() && p.pause(1).is_err() && p.stop(1).is_err());
        assert_eq!(p.snapshot().phase, PlaybackPhase::Idle);
    }
}
