//! OS が返した文字列の整形（多数の機能 crate が共有する純関数）。
//!
//! 端末情報の契約（`DeviceInfo`・`DeviceInfoSnapshot`・`require_any`）は `fandhe-cap-device-info` が持つ。
//! ここに残すのは、2 機能以上が使う OS に触れない部品だけ（`docs/architecture/consumer-app-structure.md` §8）。
//! `fandhe-cap-device-info` の `contract` が再公開するため、どちらの経路からも同じ関数を呼べる。

/// 文字列項目の長さ上限（文字数）。
const MAX_TEXT_CHARS: usize = 128;

/// OS が返した文字列を表示可能にする。制御文字を除き、前後の空白を除き、128 文字で切り、
/// 空なら `None`。全 OS 実装が共用する。
pub fn clean_text(raw: &str) -> Option<String> {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    let s: String = s.trim().chars().take(MAX_TEXT_CHARS).collect();
    let s = s.trim_end();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_control_and_blank() {
        assert_eq!(clean_text("  Mac\u{0}\n14 "), Some("Mac14".into()));
        assert_eq!(clean_text(""), None);
        assert_eq!(clean_text(" \t\n"), None);
        assert_eq!(clean_text("端末 ✓"), Some("端末 ✓".into()));
    }

    #[test]
    fn clean_text_caps_length() {
        let long = "あ".repeat(500);
        assert_eq!(clean_text(&long).unwrap().chars().count(), 128);
    }
}
