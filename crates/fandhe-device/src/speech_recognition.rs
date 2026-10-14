//! Phase 11 の拡充ループ（親 #1023、Issue #1024・#1026）: 音声認識（「ボタンを押して
//! 1 回話した内容を文字列で受け取る」だけの範囲。連続認識・オンデバイス認識の強制・
//! 言語選択（システム言語固定）は対象外）。
//!
//! macOS/iOS/Windows/Android（実装済み）は `mic_record`/`biometric_auth` と同じ理由（認識完了が
//! コールバック型で非同期に届く）で、本トレイトを**通らず** `fandhe_core::screens::
//! speech_recognize`（端末依存ゼロの状態機械）へ `fandhe-egui-proxy` 側の
//! `RequestSlot`/`poll_and_finish`（pull 型 bridge）から直接書き戻す
//! （`mic_record.rs` モジュール doc の判断と同じ）。本トレイトは対応不可プラットフォーム
//! （Web/Linux）向けの [`UnsupportedSpeechRecognizer`] と、
//! 将来これらが同期実装を持つ場合の受け皿として存在する。
//!
//! # 結果の値（4 種類、互いに混ぜない）
//!
//! | 状況 | 値 |
//! |---|---|
//! | 文字列を得た（空白以外を含む） | `Ok(SpeechOutcome::Recognized(text))` |
//! | 無音・結果 0 件・空文字・空白のみ・OS の「無音タイムアウト」 | `Ok(SpeechOutcome::NoMatch)` |
//! | 権限確認中（開始前）の停止・OS 側の利用者キャンセル | `Ok(SpeechOutcome::Cancelled)` |
//! | 権限拒否（マイク / 音声認識） | `Err(DeviceError::PermissionDenied("speech_recognition"))` |
//! | 認識器を作れない・言語未対応 | `Err(DeviceError::Unsupported("speech_recognition"))` |
//! | 認識器が一時的に使えない（オフライン・音声入力設定 OFF 等） | `Err(DeviceError::Disabled("speech_recognition"))` |
//! | その他（エンジン起動失敗・入力デバイス無し・未知のエラー） | `Err(DeviceError::Internal(stage))` |
//!
//! 権限拒否やエンジン無しを空文字の成功へ畳まない（`device-bridge-contracts.md`
//! 「嘘の成功」節）。無音は失敗にもキャンセルにもしない（同「三値」節）。
//!
//! # 「停止」ボタンの意味
//!
//! 停止ボタンは「話し終わった」の合図として扱い、OS に結果の確定を求める。確定した
//! 文字列があれば `Recognized`、無ければ `NoMatch` を返す。権限確認中など、まだ聞き
//! 始めていない段階の停止は `Cancelled`。停止を失敗として見せる経路は作らない。
//!
//! macOS の `SFSpeechAudioBufferRecognitionRequest` は `endAudio` を呼ぶまで結果を
//! 確定しない。停止を常にキャンセル扱いにすると、macOS では [`MAX_LISTEN_MS`] の
//! 自動停止を待たない限り結果が出なくなるため、上記の扱いにする。
//!
//! # 上限（[`MAX_LISTEN_MS`]）
//!
//! 聞き取りの上限（30 秒）は `fandhe_device::mic_record::MAX_RECORD_MS` とは別クレート
//! （`fandhe-core`）の `fandhe_core::screens::mic_record::MAX_RECORD_MS` を単一の SSOT
//! として `fandhe_core::screens::speech_recognize::MAX_LISTEN_MS` が参照する（本クレートは
//! `fandhe-core` に依存しないため、この定数自体は `fandhe-core` 側に置く、
//! `crates/fandhe-core/src/screens/speech_recognize.rs` 参照）。Rust 側（画面の `tick`）
//! と OS 側（macOS: `dispatch_after`、Windows: ワーカーの `recv_timeout`、Android:
//! `Handler.postDelayed`）の期限をこの 1 定数で揃える（`device-bridge-contracts.md`
//! 「期限・キャンセル」節）。

use std::time::Duration;

use crate::DeviceError;

/// 認識結果の文字列 1 件あたりのバイト数上限（16 KiB、メモリ枯渇防止。
/// `mic_record::MAX_RECORDING_BYTES`/`camera::MAX_IMAGE_BYTES` と同型の SSOT 定数）。
pub const MAX_TRANSCRIPT_BYTES: usize = 16 * 1024;

/// 音声認識の結果（モジュール doc の表参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechOutcome {
    /// 認識できた文字列（trim しない。空白のみは [`outcome_from_transcript`] が
    /// [`SpeechOutcome::NoMatch`] へ振り分ける）。
    Recognized(String),
    /// 無音・結果 0 件・空文字・OS の「無音タイムアウト」。
    NoMatch,
    /// 権限確認中（開始前）の停止、または OS 側の利用者キャンセル。
    Cancelled,
}

/// OS から届いた認識文字列 → [`SpeechOutcome`] への変換（SSOT、モジュール doc の
/// 表参照）。空白のみは [`SpeechOutcome::NoMatch`]、上限超過は `InvalidInput` にする
/// （録音結果自体の契約違反だが、`mic_record::build_recording` と同様に既存
/// バリアントを再利用し、新しいバリアントを増やさない）。text は trim せずそのまま
/// 保持する（`device-bridge-contracts.md` 「秘密値」節と同じ「入力どおりに保存する」
/// 方針を認識結果にも適用する）。
pub fn outcome_from_transcript(text: String) -> Result<SpeechOutcome, DeviceError> {
    // サイズ検査を空白判定より先に行う。空白のみ判定を先にすると、上限超過の
    // 空白のみ文字列（例: 16 KiB を超える半角スペース列）が `NoMatch` として
    // すり抜け、認識結果のサイズ上限という契約に違反する
    // （device-bridge-contracts.md「対応表・権限」節と同じ契約遵守の考え方）。
    if text.len() > MAX_TRANSCRIPT_BYTES {
        return Err(DeviceError::InvalidInput("speech_recognition size"));
    }
    if text.trim().is_empty() {
        return Ok(SpeechOutcome::NoMatch);
    }
    Ok(SpeechOutcome::Recognized(text))
}

/// 音声認識の契約（同期実装向け。モジュール doc の「対応不可」節を参照）。
pub trait SpeechRecognizer: Send + Sync {
    /// 聞き取りを開始する。`max` を超えたら実装側が自動停止する契約
    /// （モジュール doc「上限」節参照）。聞き取り中に再度呼ばれたら
    /// `Err`（二重開始を許さない）。
    fn start(&self, max: Duration) -> Result<(), DeviceError>;
    /// 聞き取りを止めて結果を返す。聞き取っていないときに呼ばれたら `Err`。
    fn stop(&self) -> Result<SpeechOutcome, DeviceError>;
}

/// 対応不可プラットフォーム用（本 Issue の時点では iOS/Android/Web/Linux）。
/// 常に `Unsupported("speech_recognition")` を返す。
pub struct UnsupportedSpeechRecognizer;

impl SpeechRecognizer for UnsupportedSpeechRecognizer {
    fn start(&self, _max: Duration) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("speech_recognition"))
    }

    fn stop(&self) -> Result<SpeechOutcome, DeviceError> {
        Err(DeviceError::Unsupported("speech_recognition"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_from_transcript_recognizes_non_blank_text() {
        assert_eq!(
            outcome_from_transcript("こんにちは".to_string()).unwrap(),
            SpeechOutcome::Recognized("こんにちは".to_string())
        );
    }

    #[test]
    fn outcome_from_transcript_treats_empty_as_no_match() {
        assert_eq!(
            outcome_from_transcript(String::new()).unwrap(),
            SpeechOutcome::NoMatch
        );
    }

    #[test]
    fn outcome_from_transcript_treats_whitespace_only_as_no_match() {
        assert_eq!(
            outcome_from_transcript("   \n\t".to_string()).unwrap(),
            SpeechOutcome::NoMatch
        );
    }

    #[test]
    fn outcome_from_transcript_does_not_trim_recognized_text() {
        // 入力どおりに保存する（trim で前後の空白を意図せず除去しない、
        // device-bridge-contracts.md「秘密値」節と同じ方針）。
        assert_eq!(
            outcome_from_transcript(" こんにちは ".to_string()).unwrap(),
            SpeechOutcome::Recognized(" こんにちは ".to_string())
        );
    }

    #[test]
    fn outcome_from_transcript_rejects_oversized_text() {
        let err = outcome_from_transcript("あ".repeat(MAX_TRANSCRIPT_BYTES + 1)).unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("speech_recognition size")
        ));
    }

    #[test]
    fn outcome_from_transcript_rejects_oversized_whitespace_only_text() {
        // サイズ検査は空白判定より先に行う契約（PR #1028 codex 指摘）。
        // 空白のみでも上限超過なら NoMatch へすり抜けさせず InvalidInput にする。
        let err = outcome_from_transcript(" ".repeat(MAX_TRANSCRIPT_BYTES + 1)).unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("speech_recognition size")
        ));
    }

    #[test]
    fn unsupported_speech_recognizer_returns_unsupported_error() {
        let err = UnsupportedSpeechRecognizer
            .start(Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(
            err,
            DeviceError::Unsupported("speech_recognition")
        ));
        let err = UnsupportedSpeechRecognizer.stop().unwrap_err();
        assert!(matches!(
            err,
            DeviceError::Unsupported("speech_recognition")
        ));
    }
}
