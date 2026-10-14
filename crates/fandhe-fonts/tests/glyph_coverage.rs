//! `NOTO_SANS_JP_REGULAR` のグリフ被覆・実ラスタライズ結合テスト（TASK-10.1 / REQ-10 / M3）。
//!
//! egui/epaint が実際に使うフォントパーサ・ラスタライザ（`ab_glyph` -> `owned_ttf_parser`）を
//! そのまま使って検証する。`NotoSansJP-Regular.otf` は CFF アウトラインだが、
//! `owned_ttf_parser` は CFF をコア機能として扱うため `ab_glyph::FontRef::try_from_slice` で
//! 問題なく解釈できる。cmap 上のエントリ有無だけでなく、実際にアウトラインを描画して
//! 非空の被覆（インクされた画素）が得られることまで確認し、「cmap にはあるが
//! 実際には描画できない」= tofu の見落としを防ぐ。
//!
//! `fandhe-egui-proxy` 側の日本語ラベル（`sample_screen()`）に実在する代表文字
//! （ひらがな・カタカナ・常用漢字）を対象にする。

use ab_glyph::{Font, FontRef, ScaleFont};

/// `fandhe-prototype` の `sample_screen()` ラベル（「メモ」「カウントを増やす」等）に
/// 実在する代表文字。ひらがな・カタカナ・常用漢字を横断して選んでいる。
const REPRESENTATIVE_CHARS: &[char] = &[
    'あ', 'か', 'さ', 'た', 'な', 'ま', 'や', 'ら', 'わ', // ひらがな
    'メ', 'モ', 'カ', 'ウ', 'ン', 'ト', // カタカナ
    '日', '本', '語', '増', '設', '定', '一', '覧', '詳', '細', // 常用漢字
];

#[test]
fn embedded_font_parses_with_ab_glyph() {
    let font = FontRef::try_from_slice(fandhe_fonts::NOTO_SANS_JP_REGULAR);
    assert!(
        font.is_ok(),
        "egui が実際に使う ab_glyph/owned_ttf_parser で同梱フォントを解釈できること（CFF アウトライン対応の確認）"
    );
}

#[test]
fn representative_cjk_chars_have_glyphs_and_are_rasterizable() {
    let font = FontRef::try_from_slice(fandhe_fonts::NOTO_SANS_JP_REGULAR)
        .expect("同梱フォントは解釈可能であることを前段のテストで確認済み");
    let scaled_font = font.as_scaled(32.0);

    for &ch in REPRESENTATIVE_CHARS {
        let glyph_id = font.glyph_id(ch);
        assert_ne!(
            glyph_id.0, 0,
            "文字 '{ch}' の cmap グリフが同梱フォントに存在しないこと（tofu 表示の原因）"
        );

        let glyph = glyph_id.with_scale(scaled_font.scale());
        let outlined = font.outline_glyph(glyph);
        assert!(
            outlined.is_some(),
            "文字 '{ch}' はグリフ ID を持つがアウトラインを描画できない（空白グリフの疑い）"
        );

        // アウトラインを実際にラスタライズし、非空の被覆（インクされた画素）が
        // 得られることまで確認する。空白・欠落グリフ（幅はあるが実体がない）を
        // cmap 存在チェックだけでは検出できないため、このステップで tofu 相当の
        // 見落としを防ぐ。
        let outlined = outlined.expect("直前の assert で Some を確認済み");
        let mut covered_pixels: u32 = 0;
        outlined.draw(|_x, _y, coverage| {
            if coverage > 0.0 {
                covered_pixels += 1;
            }
        });
        assert!(
            covered_pixels > 0,
            "文字 '{ch}' のラスタライズ結果が非空の被覆を持たない（実質的に tofu と同じ見た目になる）"
        );
    }
}
