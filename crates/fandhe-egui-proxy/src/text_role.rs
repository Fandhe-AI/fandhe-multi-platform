//! `Node::StyledText` の役割（`TextRole`）をトークン色へ解決し、egui ラベルとして
//! 描画するプラットフォーム統合レイヤ（Issue #813、親 #811 の 2/3）。
//!
//! 呼び出し元は `lib.rs` / `tv_focus.rs` の各 `render_node`（`StyledText` 分岐）。
//! `fandhe-core` の `TextRole` / `Screen::error_text`（#812）は色を持たない意味情報
//! のみを宣言し、実際の色（`color.error` 等）への解決は本モジュールが
//! `fandhe_core::token::TokenResolver` 経由で行う（DSL に色を書かせない方針、#811）。
//! 色の定数はここにも持たず、`fandhe_core::demo_resolver` が返す既定テーマ
//! （プラットフォーム別 `platform_theme`）をそのまま使う。
//!
//! 解決失敗時（未定義キー等、通常は起こらない）は `None` を返し、呼び出し元は
//! 無色ラベルへフォールバックする（`unwrap` / `expect` を本実装コードで使わない、
//! `.claude/rules/coding-rust.md`）。

use fandhe_core::{
    demo_resolver, resolve_semantic_color, SemanticColorRole, TextRole, ThemePlatform,
    ThemeVariant, TokenResolver, TokenValue,
};

/// このプラットフォーム統合レイヤが前提とする既定テーマの選択元。
///
/// `ThemePlatform` は iOS / Android の 2 値のみ（`fandhe-core` 側の設計）。
/// Apple 系（macOS/iOS）・Linux/Windows/wasm 等の desktop 系は iOS 既定
/// （`ios::RED_LIGHT` 等）に寄せる。Android ビルドのみ Android 既定を使う。
fn platform() -> ThemePlatform {
    #[cfg(target_os = "android")]
    {
        ThemePlatform::Android
    }
    #[cfg(not(target_os = "android"))]
    {
        ThemePlatform::Ios
    }
}

/// ライト/ダーク各 1 本の `TokenResolver` を初回アクセス時にのみ構築して使い回す
/// （`TokenResolver` はマップ + 整数のみを保持し `Sync`。`resolve` は毎フレーム
/// 呼んでも問題ない軽量なマップ参照のため、フレームごとの再構築を避ける）。
/// 構築失敗（`platform_base_token_set` / `platform_theme` の内部一貫性の問題、
/// 通常は起こらない）は `None` として memo し、以後も無色フォールバックに倒す。
fn resolver(dark_mode: bool) -> Option<&'static TokenResolver> {
    static LIGHT: std::sync::OnceLock<Option<TokenResolver>> = std::sync::OnceLock::new();
    static DARK: std::sync::OnceLock<Option<TokenResolver>> = std::sync::OnceLock::new();

    let variant = if dark_mode {
        ThemeVariant::Dark
    } else {
        ThemeVariant::Light
    };
    let cell = if dark_mode { &DARK } else { &LIGHT };
    cell.get_or_init(|| demo_resolver(platform(), variant).ok())
        .as_ref()
}

/// `role` に対応するトークン色を解決する。`Body` は常に `None`（無色、素の
/// `ui.label` と同じ見た目）。`Error` は `color.error`（`SemanticColorRole::Error`）
/// を解決し、失敗時は `None`（無色フォールバック）を返す。
///
/// `pub` にする理由: 結合テスト（`tests/styled_text_render.rs`）が期待色を
/// 独自の定数を持たずに本関数から取得するため（テストと実装で色定数を
/// 二重管理しない）。
pub fn text_role_color(role: TextRole, dark_mode: bool) -> Option<egui::Color32> {
    if role != TextRole::Error {
        return None;
    }
    let resolver = resolver(dark_mode)?;
    let TokenValue::Color(color) =
        resolve_semantic_color(resolver, SemanticColorRole::Error).ok()?
    else {
        return None;
    };
    Some(egui::Color32::from_rgba_unmultiplied(
        color.r, color.g, color.b, color.a,
    ))
}

/// `role` に応じた色（解決できれば）で `text` をラベル描画する。
/// `Error` かつ macOS/Android/Windows では AX ノードを live region（`Live::Polite`）として
/// 公開し、スクリーンリーダーがテキスト変化を能動的に読み上げられるようにする
/// （既存 `Node::Widget` の AX 補正と同じ `cfg`、`lib.rs` 参照。Issue #1001 で Windows を追加）。
pub(crate) fn styled_label(ui: &mut egui::Ui, role: TextRole, text: String) -> egui::Response {
    let color = text_role_color(role, ui.visuals().dark_mode);
    let response = match color {
        Some(color) => ui.label(egui::RichText::new(text).color(color)),
        None => ui.label(text),
    };

    #[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
    if role == TextRole::Error {
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_live(accesskit::Live::Polite);
        });
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_role_has_no_color() {
        assert_eq!(text_role_color(TextRole::Body, false), None);
        assert_eq!(text_role_color(TextRole::Body, true), None);
    }

    fn independent_error_color(dark_mode: bool) -> egui::Color32 {
        let variant = if dark_mode {
            ThemeVariant::Dark
        } else {
            ThemeVariant::Light
        };
        let resolver = demo_resolver(platform(), variant).expect("既定テーマは構築できる");
        let TokenValue::Color(color) = resolve_semantic_color(&resolver, SemanticColorRole::Error)
            .expect("color.error は解決できる")
        else {
            panic!("color.error は Color バリアントのはず");
        };
        egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
    }

    #[test]
    fn error_role_matches_token_resolver_value() {
        assert_eq!(
            text_role_color(TextRole::Error, false),
            Some(independent_error_color(false))
        );
        assert_eq!(
            text_role_color(TextRole::Error, true),
            Some(independent_error_color(true))
        );
    }

    #[test]
    fn error_role_is_reddish() {
        for dark_mode in [false, true] {
            let color = text_role_color(TextRole::Error, dark_mode).expect("Error は色を持つ");
            assert!(
                color.r() > color.g() && color.r() > color.b(),
                "色が赤系ではない: {color:?}"
            );
        }
    }

    #[test]
    fn light_and_dark_differ() {
        assert_ne!(
            text_role_color(TextRole::Error, false),
            text_role_color(TextRole::Error, true)
        );
    }
}
