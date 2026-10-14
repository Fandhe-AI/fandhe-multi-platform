//! Windows 向け単語分割実装（Issue #1094）。`device/nlp.rs` の Windows 経路から呼ばれ、
//! WinRT `Windows.Data.Text.WordsSegmenter` で分割する（`WordSegmenter` トレイトを実装）。
//! `CreateWithLanguage` / `GetTokens` はどちらも同期 API で `.get()` 待ちが無いため、
//! ワーカースレッドは使わず呼ばれたスレッドで完結する。
//!
//! 契約の SSOT は `fandhe_device::nlp`: OS を呼ぶ前に `validate_input`、空入力は
//! `Ok(vec![])`、得たトークンは必ず `normalize_words` を通す。HRESULT・入力文字列は
//! ログにも画面にも出さず固定ラベルへ畳む（security.md A09）。`unsafe` と `RoInitialize` は
//! 使わない（既存の Windows 実装群と同じ前提）。
//!
//! # 既知の天井
//!
//! パッケージ無しの素の exe から `WordsSegmenter` が動くかは Microsoft Learn に明記が無く
//! 実機確認待ち。動かないと判明したら対応表の `done` を取り下げる。ローカルに MSVC が無く、
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use fandhe_device::nlp::{normalize_words, validate_input, WordSegmenter};
use fandhe_device::DeviceError;
use windows::core::HSTRING;
use windows::Data::Text::WordsSegmenter;

/// `WordsSegmenter` で分割する実装。
pub(crate) struct WindowsWordSegmenter;

impl WordSegmenter for WindowsWordSegmenter {
    fn segment(&self, text: &str) -> Result<Vec<String>, DeviceError> {
        validate_input(text)?;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        // ponytail: 言語は "ja" 固定（日本語規則は英数字も分割でき、規則が無ければ OS が
        // UAX #29 の `und` へ落ちる）。利用者の言語に追従するなら Globalization から取る。
        let segmenter = WordsSegmenter::CreateWithLanguage(&HSTRING::from("ja"))
            .map_err(|_| DeviceError::Internal("nlp init"))?;
        let tokens = segmenter
            .GetTokens(&HSTRING::from(text))
            .map_err(|_| DeviceError::Internal("nlp segment"))?;
        let mut raw = Vec::new();
        for token in tokens {
            let word = token
                .Text()
                .map_err(|_| DeviceError::Internal("nlp token"))?;
            raw.push(String::from_utf16(&word).map_err(|_| DeviceError::Internal("nlp token"))?);
        }
        Ok(normalize_words(raw))
    }
}
