//! Phase 11 の拡充ループ（親 #1135、Issue #1335）: 画面の録画・キャプチャの抽象と契約。
//!
//! 録画の開始・停止（macOS の ScreenCaptureKit、iOS の ReplayKit、Android の
//! MediaProjection、Web の getDisplayMedia）はいずれも OS 側の同意 UI と完了が非同期に届く
//! ため、`mic_record` と同じ pull 型で扱う。`fandhe-egui-proxy` 側の `RequestSlot` /
//! `poll_and_finish` が `fandhe_core::screens::screen_capture` の状態機械へ直接書き戻し、
//! 本トレイトは「依頼の受理だけを返すエンジン契約」としてのみ存在する。
//!
//! # 結果の三値（契約表 10）
//!
//! - 成功: 明示停止・期限（10 秒）による自動停止・OS や利用者が OS 側 UI で止めた場合。
//!   macOS は書き出し完了通知（`recordingOutputDidFinishRecording:`）を受けた場合のみ成功
//!   （失敗通知・完了待ちタイムアウトはファイルサイズによらず失敗）。0 バイトは失敗（[`build_capture_result`]）
//! - キャンセル: 同意 UI を閉じた・拒否した（`DeviceError::Cancelled`）。失敗として見せない
//! - 失敗: 権限拒否（`PermissionDenied`）、0 バイト、エンコード失敗、ライフサイクル破棄
//!   （`Internal("screen_capture interrupted")`）、API が無い（`Unsupported`）
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス・1 画面内の順序、first-wins（結果は 1 回きり、古い世代は捨てる）、
//! 0 バイトを成功にしない、一時ファイルとネイティブハンドルの全経路解放、同意の毎回取得、
//! 映像の内容・ファイルパスをログへ出さない。
//!
//! 守らない: 映像の中身、OS 同意 UI の見た目と再確認頻度、TCC の許可主体が起動元の
//! ターミナルに付くこと、複数タブ・複数プロセス、プロセス死、端末別エンコーダ制約、
//! iOS の背景遷移で書き出し済みの分。
//!
//! 映像のバイト列は Rust 側へ読み込まない。OS が書いた一時ファイルの長さ（Web は
//! `Blob.size`）だけを [`build_capture_result`] へ渡す。上限は設けない。

use std::time::Duration;

use crate::DeviceError;

/// 録画結果。本番経路は必ず [`build_capture_result`] を通す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    /// 映像のバイト数（中身は保持しない）。
    pub size_bytes: u64,
    /// MIME タイプ（例: `"video/mp4"`）。
    pub mime: String,
    /// 録画時間（ミリ秒）。
    pub duration_ms: u64,
}

/// [`CaptureResult`] の不変条件（0 バイト・0 ミリ秒を成功にしない）の SSOT。
/// 各 OS 実装はネイティブから受け取った値を必ずここへ通す。
pub fn build_capture_result(
    size_bytes: u64,
    mime: String,
    duration_ms: u64,
) -> Result<CaptureResult, DeviceError> {
    if size_bytes == 0 {
        return Err(DeviceError::InvalidInput("screen_capture bytes"));
    }
    if duration_ms == 0 {
        return Err(DeviceError::InvalidInput("screen_capture duration"));
    }
    Ok(CaptureResult {
        size_bytes,
        mime,
        duration_ms,
    })
}

/// 画面録画エンジンの契約。`Ok` は「依頼を受理した」ことだけを意味し、結果は後から
/// bridge へ 1 回だけ届く。
pub trait ScreenCapturer: Send + Sync {
    /// 録画を依頼する。`max_duration` で実装側が自動停止する。
    fn start(&self, request_id: u64, max_duration: Duration) -> Result<(), DeviceError>;
    /// 停止を依頼する。`request_id` が進行中と一致しなければ無視される。
    fn stop(&self, request_id: u64) -> Result<(), DeviceError>;
}

/// 対応不可の環境（Linux、実装前の OS スタブ）。常に `Unsupported("screen_capture")`。
pub struct UnsupportedScreenCapturer;

impl ScreenCapturer for UnsupportedScreenCapturer {
    fn start(&self, _request_id: u64, _max_duration: Duration) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen_capture"))
    }

    fn stop(&self, _request_id: u64) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen_capture"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_bytes_and_zero_duration() {
        assert!(matches!(
            build_capture_result(0, "video/mp4".into(), 1_000),
            Err(DeviceError::InvalidInput("screen_capture bytes"))
        ));
        assert!(matches!(
            build_capture_result(10, "video/mp4".into(), 0),
            Err(DeviceError::InvalidInput("screen_capture duration"))
        ));
    }

    #[test]
    fn accepts_valid_input() {
        let r = build_capture_result(10, "video/mp4".into(), 1_500).unwrap();
        assert_eq!((r.size_bytes, r.duration_ms), (10, 1_500));
    }

    #[test]
    fn unsupported_capturer_returns_unsupported() {
        let c = UnsupportedScreenCapturer;
        assert!(matches!(
            c.start(1, Duration::from_secs(1)),
            Err(DeviceError::Unsupported("screen_capture"))
        ));
        assert!(matches!(
            c.stop(1),
            Err(DeviceError::Unsupported("screen_capture"))
        ));
    }
}
