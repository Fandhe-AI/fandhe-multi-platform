//! CJK 対応フォント（Noto Sans JP）の同梱資産クレート（TASK-10.1 / REQ-10 / M3）。
//!
//! PoC-1 で判明したとおり egui は自前ラスタライザのため、既定フォント（ラテン文字のみ）
//! では日本語テキストが「豆腐（tofu）」表示になる（`docs/spec/04-requirements.md` REQ-10）。
//! 本クレートはその是正として、Noto Sans JP Regular を `include_bytes!` でバイナリ埋め込み
//! し、システムフォントの有無に依存しない配布物を実現する。
//!
//! 依存方向は「最適化レイヤ（`fandhe-egui-proxy` 等）→ 本クレート」の一方向
//! （coding-rust.md 準拠）。`fandhe-core` には依存しない・される関係もない
//! （フォント資産はレンダラ側の関心事であり、共通コアの肥大化を避ける設計判断）。
//!
//! wasm32 ターゲットでは本クレートを依存に含めない（`fandhe-egui-proxy` 側で
//! `cfg(not(target_arch = "wasm32"))` によりゲートしている）。埋め込みは配信サイズを
//! 即座に数 MB 増やし REQ-4（3MB 基準）と衝突するため、Web は `fonts/` からの fetch
//! 方式を維持し、コピー元を本クレート同梱の資産（`assets/NotoSansJP-Regular.otf`）へ
//! 差し替える（`scripts/web/build-wasm-proxy.sh` 参照）。
#![forbid(unsafe_code)]

/// Noto Sans JP Regular（静的インスタンス、CFF アウトライン・OpenType/OTF 形式）。
///
/// 出典・バージョン・SHA-256・ライセンス条件は `assets/README.md` に記録している。
/// egui/epaint が実際に使うラスタライザ（`ab_glyph` -> `owned_ttf_parser`）は CFF を
/// コア機能として扱うため、OTF のまま埋め込んでも問題なく解釈できることを
/// `tests/glyph_coverage.rs` で検証している。
pub const NOTO_SANS_JP_REGULAR: &[u8] = include_bytes!("../assets/NotoSansJP-Regular.otf");

#[cfg(test)]
mod tests {
    use super::*;

    /// 埋め込みバイト列が OpenType のマジックナンバー（OTF: "OTTO"）を持つことを
    /// 機械的に確認する（4 バイト未満であればスライス添字アクセス自体が panic するため、
    /// 空でないことも合わせて保証される）。詳細なグリフ被覆・実ラスタライズ検証は
    /// `tests/glyph_coverage.rs`（結合テスト）側で行う。
    #[test]
    fn embedded_font_is_non_empty_opentype() {
        let magic = &NOTO_SANS_JP_REGULAR[0..4];
        assert_eq!(
            magic, b"OTTO",
            "OTF（CFF アウトライン）のマジックナンバーを期待する"
        );
    }
}
