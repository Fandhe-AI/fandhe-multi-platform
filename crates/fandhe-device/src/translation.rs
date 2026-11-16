//! 端末内翻訳が使う純粋な検証だけを置く（Issue #1330）。契約・OS 実装・ドライバは機能 crate
//! `fandhe-cap-translation` へ移した。ここに残すのは、他の機能 crate（`fandhe-cap-spell-check`）も使う
//! 言語タグの構文検査だけで、`fandhe-device` から機能 crate へは依存させない
//! （`docs/architecture/consumer-app-structure.md` §8、`docs/operations/capability-crate-migration.md` §7）。

/// 言語タグ（BCP 47）の最大バイト数。
pub const MAX_LANG_TAG_BYTES: usize = 35;

/// BCP 47 の構文だけを見る（主タグは ASCII 英字 2〜3 文字、続くサブタグは 1〜8 文字の英数字）。
/// 言語が実在・対応かどうかは OS に判定させる。
pub fn valid_lang_tag(tag: &str) -> bool {
    if tag.len() > MAX_LANG_TAG_BYTES {
        return false;
    }
    let mut parts = tag.split('-');
    let primary_ok = parts
        .next()
        .is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    primary_ok
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_tag_syntax() {
        for ok in ["ja", "en", "zh-Hant", "zz", "pt-BR"] {
            assert!(valid_lang_tag(ok), "{ok}");
        }
        let long = "a".repeat(MAX_LANG_TAG_BYTES + 1);
        for bad in [
            "",
            "j",
            "jaaa",
            "ja_JP",
            "ja-",
            "日本",
            "ja-toolongsub1",
            &long,
        ] {
            assert!(!valid_lang_tag(bad), "{bad:?}");
        }
    }
}
