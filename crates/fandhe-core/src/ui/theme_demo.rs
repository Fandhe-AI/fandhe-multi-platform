//! サンプル・カタログへのテーマ適用デモ（TASK-8.4-S23、Issue #179）。
//!
//! 親 issue #156（TASK-8.4「デザイントークン基盤実装」/ REQ-8 / M3）の分解
//! sub-issue（23/24）。S1〜S19 が実装したトークンスキーマ・解決エンジン
//! （[`crate::token::resolver`]）・プラットフォーム別既定テーマ（S16、
//! [`crate::token::platform_theme`]）・ウィジェット配線（S14、
//! [`super::style_tokens`]）を用いて、「テーマがサンプル画面・ウィジェットカタログへ
//! 実際に適用され、ライト/ダークで見た目（解決値）が変わる」ことを一気通貫で
//! 示すデモを提供する。代表 1 画面サンプル（[`super::sample::sample_screen`]）と
//! ウィジェットカタログ（[`super::catalog::WidgetCatalog`]）に対し、テーマ
//! （[`crate::token::ThemeVariant`] によるライト/ダーク差分）を適用した解決結果を
//! ヘッドレスで確認できるデモ用の公開関数群を提供する。テーマ切替は
//! [`crate::token::platform_theme`]（ブリッジ関数、`platform_theme.rs` モジュール doc
//! 参照）と `TokenResolver::with_themes` / `set_active_theme` 経由で行う。
//!
//! ## 依存契約
//!
//! - [`super::sample`] / [`super::catalog`] を変更せず、それらの既存公開 API
//!   （`sample_screen` / `WidgetCatalog::with_builtins` 等）のみを利用する
//!   自己完結モジュール（`recycle.rs` / `catalog.rs` が採った前例を踏襲し、
//!   並行実装中の他 sub-issue とのコンフリクトを避ける）。
//! - すべての公開関数は `Result` を返し、panic を API 契約にしない
//!   （`.claude/rules/coding-rust.md`）。解決失敗（未定義キー・型不一致等）は
//!   `TokenError` をそのまま呼び出し元へ伝播する。
//! - 呼び出し元は `examples/theme_gallery.rs`（ヘッドレス動作確認）・
//!   `tests/theme_gallery_demo.rs`（結合テスト）を想定する。

use crate::token::{
    ThemePlatform, ThemeVariant, TokenError, TokenResolver, platform_base_token_set, platform_theme,
};

use super::catalog::WidgetCatalog;
use super::render::render_headless;
use super::sample::{SampleState, sample_screen};
use super::style_tokens::{
    StyleTokenRefs, action_style_refs, display_style_refs, input_style_refs, resolve_widget_style,
};
use super::widget::{StateStyles, VisualState};

/// カテゴリ既定スタイル（[`action_style_refs`] 等）を構築する関数ポインタの型。
///
/// [`themed_style_lines`] 内の配列要素の型注釈に使う（`fn() -> Result<_, TokenError>`
/// をそのまま書くと clippy の `type_complexity` に抵触するため、名前付き型として
/// 切り出す）。
type CategoryStyleBuilder = fn() -> Result<StateStyles<StyleTokenRefs>, TokenError>;

/// `platform` の既定テーマから、デモ用の `TokenResolver` を構築する。
///
/// `theme` に [`ThemeVariant::Light`] を渡すとライト（`platform_base_token_set`
/// のみ）、[`ThemeVariant::Dark`] を渡すとダーク（プラットフォームのダーク差分
/// を重ねたテーマ）を初期アクティブテーマとして構築する。内部では
/// [`crate::token::platform_theme`]（S5、#161 のブリッジ関数）でライト/ダーク
/// 両方の `Theme` を構築し、`TokenResolver::with_themes` へ渡す（構築後も
/// `TokenResolver::set_active_theme` で相互に切替可能）。
///
/// # Errors
///
/// `platform_base_token_set` / `platform_theme` の構築に失敗した場合に伝播する
/// （プラットフォーム別既定テーマの内部一貫性の問題であり、通常は起こらない）。
pub fn demo_resolver(
    platform: ThemePlatform,
    theme: ThemeVariant,
) -> Result<TokenResolver, TokenError> {
    let base = platform_base_token_set(platform)?;
    let light = platform_theme(platform, ThemeVariant::Light)?;
    let dark = platform_theme(platform, ThemeVariant::Dark)?;

    Ok(TokenResolver::with_themes(base, light, dark, theme))
}

/// カテゴリ既定マッピング（[`action_style_refs`] / [`input_style_refs`] /
/// [`display_style_refs`]）を `resolver` で解決し、「カテゴリ → 解決済み
/// 背景/前景/タイポグラフィ」の整形済み行を返す。
///
/// いずれのカテゴリも [`VisualState::Idle`]（既定の見た目）で解決する。各行は
/// `{カテゴリ}: background={#RRGGBBAA|-} foreground={#RRGGBBAA|-}
/// typography={family}/{size}pt|-` の形式（`ColorValue::to_hex` /
/// `TypographyValue::family`+`size` を用いる。目視確認しやすい固定形式であり、
/// 呼び出し元はこれを表示用データとしてのみ扱うこと、A03 対策）。
///
/// # Errors
///
/// いずれかのカテゴリの解決（`resolve_widget_style`）が失敗した場合、最初に
/// 検出したエラーを返す（[`super::style_tokens::resolve_widget_style`] の契約を
/// そのまま引き継ぐ）。
pub fn themed_style_lines(resolver: &TokenResolver) -> Result<Vec<String>, TokenError> {
    let categories: [(&str, CategoryStyleBuilder); 3] = [
        ("action", action_style_refs),
        ("input", input_style_refs),
        ("display", display_style_refs),
    ];

    let mut lines = Vec::with_capacity(categories.len());
    for (label, build_refs) in categories {
        let refs = build_refs()?;
        let resolved = resolve_widget_style(resolver, &refs, VisualState::Idle)?;

        let background = resolved
            .background
            .map(|value| value.to_hex())
            .unwrap_or_else(|| "-".to_string());
        let foreground = resolved
            .foreground
            .map(|value| value.to_hex())
            .unwrap_or_else(|| "-".to_string());
        let typography = resolved
            .typography
            .map(|value| format!("{}/{}pt", value.family(), value.size()))
            .unwrap_or_else(|| "-".to_string());

        lines.push(format!(
            "{label}: background={background} foreground={foreground} typography={typography}"
        ));
    }

    Ok(lines)
}

/// `sample_screen` のヘッドレス行に、`resolver` で解決したテーマ適用スタイル
/// 注記（[`themed_style_lines`]）を続けて返す。
///
/// サンプル画面自体（`ui::sample`）はテーマ配線を持たないため（`sample.rs`
/// モジュール doc 参照）、本関数は「画面の構成行」と「同一テーマで解決した
/// カテゴリ別スタイル」を 1 つの行列として並べることで、同じ `resolver` を
/// 通せば画面に使われるカテゴリの見た目がどう変わるかを対応づけて確認できる
/// ようにする。
///
/// # Errors
///
/// [`themed_style_lines`] の呼び出しが失敗した場合に伝播する。
pub fn themed_sample_lines(resolver: &TokenResolver) -> Result<Vec<String>, TokenError> {
    let mut lines = render_headless(&sample_screen(), &SampleState::default());
    lines.push("--- テーマ解決済みスタイル ---".to_string());
    lines.extend(themed_style_lines(resolver)?);
    Ok(lines)
}

/// `catalog.gallery_screen()` のヘッドレス行に、`resolver` で解決したテーマ
/// 適用スタイル注記（[`themed_style_lines`]）を続けて返す。
///
/// [`themed_sample_lines`] と同型の構成（「カタログの全エントリ行」+「同一
/// テーマで解決したカテゴリ別スタイル」）。`catalog` はどの組み込みカタログ
/// （`WidgetCatalog::with_builtins` 等）を渡しても、登録済み全エントリの行が
/// 欠けず先頭部分に含まれる。
///
/// # Errors
///
/// [`themed_style_lines`] の呼び出しが失敗した場合に伝播する。
pub fn themed_catalog_lines(
    catalog: &WidgetCatalog,
    resolver: &TokenResolver,
) -> Result<Vec<String>, TokenError> {
    let mut lines = render_headless(&catalog.gallery_screen(), &catalog.gallery_state());
    lines.push("--- テーマ解決済みスタイル ---".to_string());
    lines.extend(themed_style_lines(resolver)?);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_resolver_light_builds_without_error_for_both_platforms() {
        demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
            .expect("iOS ライトテーマの resolver 構築は成功する");
        demo_resolver(ThemePlatform::Android, ThemeVariant::Light)
            .expect("Android ライトテーマの resolver 構築は成功する");
    }

    #[test]
    fn demo_resolver_dark_builds_without_error_for_both_platforms() {
        demo_resolver(ThemePlatform::Ios, ThemeVariant::Dark)
            .expect("iOS ダークテーマの resolver 構築は成功する");
        demo_resolver(ThemePlatform::Android, ThemeVariant::Dark)
            .expect("Android ダークテーマの resolver 構築は成功する");
    }

    #[test]
    fn themed_style_lines_differ_between_light_and_dark() {
        let light = demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
            .expect("ライト resolver の構築は成功する");
        let dark = demo_resolver(ThemePlatform::Ios, ThemeVariant::Dark)
            .expect("ダーク resolver の構築は成功する");

        let light_lines = themed_style_lines(&light).expect("ライトの解決は成功する");
        let dark_lines = themed_style_lines(&dark).expect("ダークの解決は成功する");

        assert_eq!(light_lines.len(), 3);
        assert_eq!(dark_lines.len(), 3);
        assert_ne!(
            light_lines, dark_lines,
            "iOS のライト/ダークでカテゴリ別スタイル行は異なる解決結果になる必要がある"
        );
    }

    #[test]
    fn themed_style_lines_are_deterministic_for_repeated_light_resolvers() {
        let first = demo_resolver(ThemePlatform::Android, ThemeVariant::Light)
            .expect("1 回目の構築は成功する");
        let second = demo_resolver(ThemePlatform::Android, ThemeVariant::Light)
            .expect("2 回目の構築は成功する");

        let first_lines = themed_style_lines(&first).expect("1 回目の解決は成功する");
        let second_lines = themed_style_lines(&second).expect("2 回目の解決は成功する");

        assert_eq!(
            first_lines, second_lines,
            "同一プラットフォーム・同一テーマの再構築は決定的な出力になる必要がある"
        );
    }

    #[test]
    fn themed_sample_lines_includes_sample_screen_rows_and_style_annotation() {
        let resolver = demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
            .expect("resolver の構築は成功する");

        let lines = themed_sample_lines(&resolver).expect("サンプル画面の解決は成功する");

        assert!(lines.contains(&"# サンプル画面".to_string()));
        assert!(lines.contains(&"代表 1 画面サンプル".to_string()));
        assert!(lines.contains(&"(カウントを増やす)".to_string()));
        assert!(lines.contains(&"(項目を追加)".to_string()));
        assert!(lines.iter().any(|line| line.starts_with("action: ")));
    }

    #[test]
    fn themed_catalog_lines_includes_all_builtin_entries() {
        let catalog = WidgetCatalog::with_builtins();
        let resolver = demo_resolver(ThemePlatform::Android, ThemeVariant::Dark)
            .expect("resolver の構築は成功する");

        let lines = themed_catalog_lines(&catalog, &resolver).expect("カタログの解決は成功する");

        for entry in catalog.entries() {
            assert!(
                lines.iter().any(|line| line.contains(entry.name)),
                "エントリ {} の行が欠けている",
                entry.name
            );
        }
        assert!(lines.iter().any(|line| line.starts_with("action: ")));
        assert!(lines.iter().any(|line| line.starts_with("input: ")));
        assert!(lines.iter().any(|line| line.starts_with("display: ")));
    }
}
