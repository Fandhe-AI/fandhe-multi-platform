//! Phase 11（親 #1093）型サンプル「単語分割」（Issue #1094）の effect ドライバ。
//! `fandhe_core::screens::nlp_segment`（端末依存ゼロ）の要求を観測し、
//! `crate::contract::WordSegmenter` で OS の単語分割を呼んで `CompletionSender` 経由で
//! 結果を返す（`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる（`device.rs`
//! モジュール doc と同じ理由）。Web（wasm32）は本経路を通らず `fandhe-web-dom` の
//! `nlp.rs` が `Intl.Segmenter` へ直接配線する。契約（結果の形・失敗の区別）の SSOT は
//! `crate::contract`。
//!
//! # スレッド
//!
//! macOS / iOS の `NLTokenizer`・Windows の `WordsSegmenter`・Android の `BreakIterator`
//! （JNI 経由）はいずれも同期 API で、呼び出しごとに
//! 分割器を作り共有状態を持たない。ワーカーは使わず、呼ばれたスレッド（egui の update）で
//! 同期に呼んで完了を 1 件送る。要求は完了の適用（次フレーム先頭）までに effect が
//! もう一度走っても、世代照合つきの `finish_segment` が二重適用を防ぐ。
//!
//! # 対応 OS
//!
//! macOS / iOS: [`crate::apple::AppleWordSegmenter`]、Windows:
//! [`crate::windows::WindowsWordSegmenter`]、Android:
//! `android::AndroidWordSegmenter`（#1095）。それ以外（Linux）は
//! `UnsupportedWordSegmenter` 固定（ダミーの成功実装へは倒さない）。

use std::sync::Arc;

use crate::contract::{WordSegmenter, normalize_words};
use crate::error::NlpError;
use fandhe_core::screens::nlp_segment::{
    NlpSegmentState, SegmentFailure, finish_segment, take_request,
};

use fandhe_runtime::CompletionSender;

/// 現在のプラットフォームに応じた `WordSegmenter` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn WordSegmenter> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn WordSegmenter>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                Arc::new(crate::apple::AppleWordSegmenter)
            }
            #[cfg(target_os = "windows")]
            {
                Arc::new(crate::windows::WindowsWordSegmenter)
            }
            #[cfg(target_os = "android")]
            {
                Arc::new(crate::android::AndroidWordSegmenter)
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "ios",
                target_os = "windows",
                target_os = "android"
            )))]
            {
                Arc::new(crate::contract::UnsupportedWordSegmenter)
            }
        })
        .clone()
}

/// Android の `NlpHelper.segment` の戻り値（トークンを U+0000 で連結した文字列。
/// 入力は `validate_input` が NUL を拒否済みでトークンは入力の部分文字列のため、NUL は
/// 区切りとして曖昧にならない。null は Java 側の失敗）をトークン列へ写す。
/// 写像だけを OS 非依存の純関数にして、ホストでテストする。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn decode_android_tokens(raw: Option<String>) -> Result<Vec<String>, NlpError> {
    let raw = raw.ok_or(NlpError::Internal)?;
    Ok(normalize_words(raw.split('\0')))
}

/// `NlpError` を画面の失敗種別へ写す。内部構造は `fandhe-core` へ漏らさず、
/// `Unsupported` / `InvalidInput` 以外は `Display` の文言のみを転送する（security.md A09）。
fn to_failure(e: NlpError) -> SegmentFailure {
    match e {
        NlpError::Unsupported => SegmentFailure::Unsupported,
        NlpError::InvalidInput => SegmentFailure::InvalidInput,
        other => SegmentFailure::Failed(other.to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &NlpSegmentState,
    sender: &CompletionSender<NlpSegmentState>,
    segmenter: Arc<dyn WordSegmenter>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let result = segmenter.segment(&request.text).map_err(to_failure);
    // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut NlpSegmentState| {
        finish_segment(s, generation, result);
    }));
}

/// `registry.rs` の `effect` 区画から `nlp_segment` 画面用に配線される effect ドライバ。
pub fn nlp_segment_effect(state: &NlpSegmentState, sender: &CompletionSender<NlpSegmentState>) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::UnsupportedWordSegmenter;
    use fandhe_core::screens::nlp_segment::{INVALID_INPUT_STATUS, UNSUPPORTED_STATUS, segment};

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: std::time::Duration) {}
    }

    fn sender_with_channel() -> (
        CompletionSender<NlpSegmentState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<NlpSegmentState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(NoopWaker)),
            rx,
        )
    }

    struct Fake(Result<Vec<String>, fn() -> NlpError>);

    impl WordSegmenter for Fake {
        fn segment(&self, _text: &str) -> Result<Vec<String>, NlpError> {
            self.0.clone().map_err(|f| f())
        }
    }

    /// 入力で要求を立てて `drive` し、届いた完了を適用した状態を返す。
    fn run(input: &str, segmenter: Arc<dyn WordSegmenter>) -> NlpSegmentState {
        let (sender, rx) = sender_with_channel();
        let mut state = NlpSegmentState {
            input: input.to_string(),
            ..Default::default()
        };
        segment(&mut state);
        drive(&state, &sender, segmenter);
        let c = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("完了が届く");
        c(&mut state);
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        drive(
            &NlpSegmentState::default(),
            &sender,
            Arc::new(Fake(Ok(vec![]))),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_count_equal_to_list_length() {
        let state = run("a b", Arc::new(Fake(Ok(vec!["a".into(), "b".into()]))));
        assert_eq!(state.status, "2 語");
        assert_eq!(state.words, ["a", "b"]);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_failure_not_success_even_for_empty_input() {
        for input in ["", "hello"] {
            let state = run(input, Arc::new(UnsupportedWordSegmenter));
            assert_eq!(state.error, UNSUPPORTED_STATUS);
            assert_eq!(state.status, "");
        }
    }

    #[test]
    fn invalid_input_and_internal_are_mapped() {
        let state = run("x", Arc::new(Fake(Err(|| NlpError::InvalidInput))));
        assert_eq!(state.error, INVALID_INPUT_STATUS);
        let state = run("x", Arc::new(Fake(Err(|| NlpError::Internal))));
        assert!(state.error.starts_with("失敗しました: "));
        assert_eq!(state.status, "");
    }

    /// 実 OS 実装でも上限超過は OS を呼ぶ前に弾かれ、失敗として表示される。
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn real_implementation_rejects_over_limit_input() {
        let state = run(
            &"a".repeat(crate::contract::MAX_INPUT_CHARS + 1),
            implementation(),
        );
        assert_eq!(state.error, INVALID_INPUT_STATUS);
    }

    /// 非対応 OS の実装は入力に関わらず `Unsupported`（空入力も成功にしない）。
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn fallback_implementation_is_unsupported() {
        let state = run("", implementation());
        assert_eq!(state.error, UNSUPPORTED_STATUS);
    }

    #[test]
    fn decode_android_tokens_maps_null_and_filters() {
        assert!(matches!(
            decode_android_tokens(None),
            Err(NlpError::Internal)
        ));
        assert_eq!(decode_android_tokens(Some(String::new())).unwrap().len(), 0);
        assert_eq!(
            decode_android_tokens(Some("Hello\0,\0 \0世界".into())).unwrap(),
            ["Hello", "世界"]
        );
    }
}
