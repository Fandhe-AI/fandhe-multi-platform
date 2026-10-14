//! Phase 11 の拡充ループ（親 #953、Issue #954・#956）: マイク録音の抽象。
//!
//! `biometric_auth`/`share` と同じ理由（OS 側の権限確認・録音完了がいずれも
//! コールバック型で非同期に届く）で、iOS/macOS（実装済み）は本トレイトを
//! **通らず**、`fandhe_core::screens::mic_record`（端末依存ゼロの状態機械）へ
//! `fandhe-egui-proxy` 側の `RequestSlot`/`poll_and_finish`（pull 型 bridge）から
//! 直接書き戻す（`biometric_auth.rs` モジュール doc の「本トレイトを通らず」節と
//! 同じ判断）。Windows は同一フレーム内で `start`→`stop` を呼び切る同期呼び出しが
//! 録音時間 0 ms を生み必ず失敗するため（`fandhe-egui-proxy::device::
//! windows_mic_record` モジュール doc 参照）、本トレイトはセッション管理を持たない
//! 「OS 呼び出しのエンジン」としてのみ実装され、駆動（ワーカースレッド・期限タイマ・
//! 停止合図）は `fandhe-egui-proxy` 側の `windows_mic_record::present`/
//! `request_stop` が担う。本トレイトは対応不可プラットフォーム（Linux、現時点の
//! Android/Web）向けの [`UnsupportedMicRecorder`] と、将来 Android/Web が同期実装を
//! 持つ場合の受け皿として存在する。
//!
//! [`Recording`] の不変条件（0 バイト禁止・`duration_ms == 0` 禁止・
//! [`MAX_RECORDING_BYTES`] 超過禁止）は [`build_recording`] に一本化する（呼び出し元
//! ごとに同じ検証を書き直さない SSOT、`camera::MAX_IMAGE_BYTES`/`file_picker::
//! MAX_FILE_BYTES` と同型の方針）。macOS 実装（`fandhe-egui-proxy::device::
//! apple_mic_record`）もネイティブから受け取ったバイト列を必ずここへ通す。

use std::time::Duration;

use crate::DeviceError;

/// 録音 1 件あたりのバイト数上限（8 MiB、メモリ枯渇防止。`camera::MAX_IMAGE_BYTES`/
/// `file_picker::MAX_FILE_BYTES` と同型の SSOT 定数）。
pub const MAX_RECORDING_BYTES: usize = 8 * 1024 * 1024;

/// 録音結果。[`build_recording`] のみが構築する契約（フィールドは呼び出し元の
/// テストで直接組み立てられるよう非公開にはしないが、本番経路は必ず
/// [`build_recording`] を通す、モジュール doc 参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    /// 録音データの生バイト列（`mime` が示すエンコード方式、例: AAC）。
    pub bytes: Vec<u8>,
    /// MIME タイプ（例: `"audio/mp4"`）。
    pub mime: String,
    /// 録音時間（ミリ秒）。
    pub duration_ms: u64,
}

/// [`Recording`] の不変条件を検証しながら構築する（SSOT、モジュール doc 参照）。
/// 0 バイト・`duration_ms == 0`・[`MAX_RECORDING_BYTES`] 超過はいずれも
/// `DeviceError::InvalidInput` にする（録音結果自体の契約違反だが、
/// `file_picker::read_picked_from` 等と同様に既存バリアントを再利用し、
/// 新しいバリアントを増やさない）。
pub fn build_recording(
    bytes: Vec<u8>,
    mime: String,
    duration_ms: u64,
) -> Result<Recording, DeviceError> {
    if bytes.is_empty() {
        return Err(DeviceError::InvalidInput("mic_record bytes"));
    }
    if duration_ms == 0 {
        return Err(DeviceError::InvalidInput("mic_record duration"));
    }
    if bytes.len() > MAX_RECORDING_BYTES {
        return Err(DeviceError::InvalidInput("mic_record size"));
    }
    Ok(Recording {
        bytes,
        mime,
        duration_ms,
    })
}

/// マイク録音の契約（同期実装向け。モジュール doc の「対応不可」節を参照）。
pub trait MicRecorder: Send + Sync {
    /// 録音を開始する。`max_duration` を超えたら実装側が自動停止する契約
    /// （macOS ネイティブ側は `AVAudioRecorder.recordForDuration:` を使う、
    /// モジュール doc 参照）。録音中に再度呼ばれたら `Err`（二重開始を許さない）。
    fn start(&self, max_duration: Duration) -> Result<(), DeviceError>;
    /// 録音を止めて結果を返す。録音していないときに呼ばれたら `Err`。
    fn stop(&self) -> Result<Recording, DeviceError>;
}

/// 対応不可プラットフォーム用（Linux、現時点の Android/Web）。
/// 常に `Unsupported("mic_record")` を返す。
pub struct UnsupportedMicRecorder;

impl MicRecorder for UnsupportedMicRecorder {
    fn start(&self, _max_duration: Duration) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("mic_record"))
    }

    fn stop(&self) -> Result<Recording, DeviceError> {
        Err(DeviceError::Unsupported("mic_record"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn build_recording_rejects_empty_bytes() {
        let err = build_recording(Vec::new(), "audio/mp4".to_string(), 1_000).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("mic_record bytes")));
    }

    #[test]
    fn build_recording_rejects_zero_duration() {
        let err = build_recording(vec![1, 2, 3], "audio/mp4".to_string(), 0).unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("mic_record duration")
        ));
    }

    #[test]
    fn build_recording_rejects_oversized_bytes() {
        let err = build_recording(
            vec![0u8; MAX_RECORDING_BYTES + 1],
            "audio/mp4".to_string(),
            1_000,
        )
        .unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput("mic_record size")));
    }

    #[test]
    fn build_recording_accepts_valid_input() {
        let recording = build_recording(vec![1, 2, 3], "audio/mp4".to_string(), 1_500).unwrap();
        assert_eq!(recording.bytes, vec![1, 2, 3]);
        assert_eq!(recording.mime, "audio/mp4");
        assert_eq!(recording.duration_ms, 1_500);
    }

    #[test]
    fn unsupported_mic_recorder_returns_unsupported_error() {
        let err = UnsupportedMicRecorder
            .start(Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("mic_record")));
        let err = UnsupportedMicRecorder.stop().unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("mic_record")));
    }

    /// トレイトの契約（二重 `start()`・未録音の `stop()` は `Err`）を実際に守る
    /// 参照実装（`biometric_auth::AlwaysCancelledAuth` と同じ「契約を実装で
    /// 固定する」目的のモック）。
    struct MockMicRecorder {
        recording: Mutex<bool>,
    }

    impl MockMicRecorder {
        fn new() -> Self {
            Self {
                recording: Mutex::new(false),
            }
        }
    }

    impl MicRecorder for MockMicRecorder {
        fn start(&self, _max_duration: Duration) -> Result<(), DeviceError> {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            if *recording {
                return Err(DeviceError::Internal("mic_record busy"));
            }
            *recording = true;
            Ok(())
        }

        fn stop(&self) -> Result<Recording, DeviceError> {
            let mut recording = self.recording.lock().unwrap_or_else(|e| e.into_inner());
            if !*recording {
                return Err(DeviceError::Internal("mic_record not recording"));
            }
            *recording = false;
            build_recording(vec![1, 2, 3], "audio/mp4".to_string(), 1_000)
        }
    }

    #[test]
    fn double_start_is_rejected() {
        let recorder = MockMicRecorder::new();
        recorder.start(Duration::from_secs(1)).unwrap();
        let err = recorder.start(Duration::from_secs(1)).unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)));
    }

    #[test]
    fn stop_without_start_is_rejected() {
        let recorder = MockMicRecorder::new();
        let err = recorder.stop().unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)));
    }

    #[test]
    fn stop_after_start_succeeds_and_allows_restart() {
        let recorder = MockMicRecorder::new();
        recorder.start(Duration::from_secs(1)).unwrap();
        let recording = recorder.stop().unwrap();
        assert_eq!(recording.duration_ms, 1_000);
        // 停止後は再度開始できる（二重開始ガードが停止後まで残らない）。
        recorder.start(Duration::from_secs(1)).unwrap();
    }
}
