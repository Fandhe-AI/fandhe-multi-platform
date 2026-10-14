//! デザイントークンの Markdown リファレンス自動生成器（TASK-8.4-S20、#176）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4 の
//! スキーマ・命名規則を機械可読化し、コード上の `TokenSet` から Markdown 文書を
//! 生成する純粋なデータ変換として実装する。二層アーキテクチャ（REQ-2）上は
//! 共通コアに閉じ、ファイル I/O を一切持たない（ファイル書き出しは
//! `examples/generate_token_docs.rs` が、生成物の鮮度検証は
//! `tests/token_docgen.rs` が担う契約。両者は本モジュールが提供する
//! [`default_token_doc_sections`] を共有し、セクション構築ロジックを二重管理
//! しない）。
//!
//! 生成物 `docs/tokens/design-token-reference.md` は S24（TASK-8.4-S24、#180、
//! 使用ガイド）が参照先とする契約であり、本モジュール（生成器）を変更する際は
//! S24 側の参照が壊れないか確認する。

use super::animation::{EasingValue, animation_token_set, duration_token_set};
use super::border::{BorderStyleValue, border_token_set};
use super::color::palette_token_set;
use super::dimension::{elevation_token_set, radius_token_set, size_token_set};
use super::error::TokenError;
use super::platform_theme::{ThemePlatform, platform_base_token_set, platform_dark_overrides};
use super::resolver::TokenResolver;
use super::schema::{TokenCategory, TokenDefinition, TokenPath, TokenSet, TokenValue};
use super::shadow::shadow_token_set;
use super::spacing::spacing_token_set;
use super::typography::{FontStyleValue, typography_token_set};

/// 生成文書の 1 セクション（見出し + 対象トークンセット）。
///
/// `default_token_doc_sections` が既定の並び（共通カテゴリ → プラットフォーム
/// 別テーマ）で構築する想定だが、本型自体はどのような `TokenSet` の集合も
/// 受け取れる（テスト・将来の外部定義ファイル文書化（S17 拡張、スコープ外）
/// からの再利用を妨げない）。
pub struct TokenDocSection {
    /// セクション見出し（Markdown の `##` に使う）。
    pub title: String,
    /// セクションが束ねるトークン定義。
    pub tokens: TokenSet,
}

/// 生成物冒頭の固定ヘッダ（自動生成・手動編集禁止・再生成コマンドの明示）。
///
/// 整合性の不備（OWASP A08、security.md）対策として、生成経路を文書自身に
/// 埋め込み、鮮度検証テスト（`tests/token_docgen.rs`）と合わせてコードと
/// 文書の乖離を検出可能にする。
const HEADER: &str = "# デザイントークンリファレンス（自動生成）\n\n\
**このファイルは自動生成されたものです。手動で編集しないでください。**\n\n\
再生成コマンド:\n\n\
```sh\n\
cargo run -p fandhe-core --example generate_token_docs\n\
```\n\n\
`crates/fandhe-core/src/token/docgen.rs`（TASK-8.4-S20、#176）がコード上の\n\
トークン定義から本ファイルを生成します。コードとドキュメントの乖離は\n\
`cargo test -p fandhe-core --test token_docgen` の鮮度検証テストが検出します。\n";

/// Markdown テーブルセルへ入れる自由文字列をエスケープする。
///
/// フォントファミリ名等のトークン値に含まれ得る `|`（列区切りとの衝突）・
/// `` ` ``（コードスパンとの衝突）・改行（行区切りとの衝突）をエスケープし、
/// 表構造・意図しないマークアップの混入を防ぐ（OWASP A03 対策、security.md。
/// 外部定義ファイル（S17、`token-file` feature）由来の `TokenSet` を将来
/// 文書化する場合の防御にもなる）。
fn escape_cell(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('`', "\\`")
        .replace('\n', "<br>")
}

fn render_font_style(style: FontStyleValue) -> &'static str {
    match style {
        FontStyleValue::Normal => "normal",
        FontStyleValue::Italic => "italic",
    }
}

fn render_border_style(style: BorderStyleValue) -> &'static str {
    match style {
        BorderStyleValue::Solid => "solid",
        BorderStyleValue::Dashed => "dashed",
    }
}

fn render_easing(easing: EasingValue) -> &'static str {
    match easing {
        EasingValue::Linear => "linear",
        EasingValue::EaseIn => "ease-in",
        EasingValue::EaseOut => "ease-out",
        EasingValue::EaseInOut => "ease-in-out",
    }
}

/// `Border`/`Shadow` が内包する色参照 `TokenRef` を解決し、表示用の文字列へ
/// 変換する。参照先が見つかれば `rgba(...)` の実値を、未定義参照
/// （呼び出し元がテスト用に不完全な `TokenSet` しか渡していない等）であれば
/// 解決失敗を示す文言をそのまま表示する（`render_value` の他バリアントと
/// 異なりここではエラーを `Result` で伝播しない。border/shadow の色参照は
/// `TokenDefinition::Reference` を経由しない値内部の埋め込みフィールドで
/// あり、`generate_token_reference` の「未定義参照は握り潰さない」契約は
/// トップレベルの `TokenDefinition::Reference` に対して適用する設計とし、
/// 値内部の色参照はベストエフォートの補助表示に留める）。
fn render_embedded_color_ref(
    resolver: &TokenResolver,
    reference: &super::schema::TokenRef,
) -> String {
    match resolver.resolve(reference) {
        Ok(resolved) => render_value(resolved.value(), resolver),
        Err(_) => format!("`{reference}`（未解決）"),
    }
}

/// f32 値を設計値として読める表記へ整形する（小数第 3 位で丸めた上で末尾の
/// 冗長なゼロ・小数点を取り除く）。
///
/// `f32` は 2 進数表現のため、`36.0_f32 * 1.4` のような設計上「割り切れる」
/// 計算でも `50.399998` のような 2 進浮動小数点誤差が実値に残る（`typography`
/// の `line_height` 等）。デフォルトの `{}` 表示（`Display for f32`）はこの
/// 誤差をそのまま桁数一杯に出してしまい、生成される Markdown リファレンスが
/// 設計値を誤って伝える（`docs/tokens/design-token-reference.md` は S24
/// （#180）が使用ガイドの参照元とする契約のため、ここでの表記崩れは下流へ
/// 伝播する）。用途はドキュメント表示のみで、値そのもの（`TokenValue` 側）は
/// 一切変更しない。
fn format_f32(value: f32) -> String {
    let rounded = format!("{value:.3}");
    let trimmed = rounded.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// トークン値を人間可読な表記へレンダリングする。
///
/// バリアント間の暗黙変換を持ち込まない `TokenValue` の設計（schema.rs 参照）
/// に合わせ、各バリアントごとに専用の表記へ変換する（`Color` は 16 進表記 +
/// RGBA、`Typography` はフィールド列挙、`Border`/`Shadow` は幅・オフセット等 +
/// 色参照の解決値、等）。`Border`/`Shadow` は色を `TokenRef`（`resolve_border_color`
/// / `resolve_shadow_color` が解決する埋め込み参照）として保持するため、
/// `resolver` を受け取り解決後の値まで表示する。数値フィールドはすべて
/// `format_f32` / `f64` は `Dimension` のみの素通し表示とし、`f32` 系は
/// 2 進浮動小数点誤差の露出を防ぐ（`format_f32` doc 参照）。
fn render_value(value: &TokenValue, resolver: &TokenResolver) -> String {
    match value {
        TokenValue::Color(color) => {
            format!(
                "`{}`（rgba {}, {}, {}, {}）",
                color.to_hex(),
                color.r,
                color.g,
                color.b,
                color.a
            )
        }
        TokenValue::Dimension(value) => format!("{value}"),
        TokenValue::Text(text) => escape_cell(text),
        TokenValue::Typography(typography) => format!(
            "family={}, size={}px, line_height={}px, weight={}, style={}",
            escape_cell(typography.family()),
            format_f32(typography.size()),
            format_f32(typography.line_height()),
            typography.weight().value(),
            render_font_style(typography.style())
        ),
        TokenValue::Spacing(spacing) => format!("{}px", format_f32(spacing.value())),
        TokenValue::Size(size) => format!("{}px", format_f32(size.value())),
        TokenValue::Radius(radius) => format!("{}px", format_f32(radius.value())),
        TokenValue::Elevation(elevation) => format!("level {}", elevation.level()),
        TokenValue::Border(border) => format!(
            "width={}px, style={}, color={}（参照元: `{}`）",
            format_f32(border.width()),
            render_border_style(border.style()),
            render_embedded_color_ref(resolver, border.color()),
            border.color()
        ),
        TokenValue::Shadow(shadow) => format!(
            "offset=({}, {})px, blur={}px, color={}（参照元: `{}`）",
            format_f32(shadow.offset_x()),
            format_f32(shadow.offset_y()),
            format_f32(shadow.blur_radius()),
            render_embedded_color_ref(resolver, shadow.color()),
            shadow.color()
        ),
        TokenValue::Duration(duration) => format!("{}ms", format_f32(duration.milliseconds())),
        TokenValue::Animation(animation) => format!(
            "duration={}ms, easing={}",
            format_f32(animation.duration().milliseconds()),
            render_easing(animation.easing())
        ),
    }
}

/// カテゴリの表示順位を返す（`TokenCategory::ALL` の宣言順に一致）。
///
/// 未知のカテゴリ（現状は起こり得ないが、将来バリアントが増えて `ALL` の
/// 更新漏れが起きた場合の保険）は末尾へ回す。
fn category_rank(category: TokenCategory) -> usize {
    TokenCategory::ALL
        .iter()
        .position(|c| *c == category)
        .unwrap_or(usize::MAX)
}

/// トークン 1 件分を Markdown テーブルの 1 行へレンダリングする。
///
/// 直接値（`TokenDefinition::Value`）はそのまま表記し、参照
/// （`TokenDefinition::Reference`）は参照先パス・期待カテゴリに加え、
/// `resolver` による解決後の値を併記する（設計文書 §4.4 の参照意味論を
/// 人間可読な形で提示する）。解決に失敗した場合はエラーを握り潰さず
/// 呼び出し元へ伝播する（既定トークンセットの実装不備を検出するため、
/// coding-rust.md の panic 禁止方針とは独立に「フォールバックしない」）。
fn render_row(
    path: &TokenPath,
    definition: &TokenDefinition,
    resolver: &TokenResolver,
) -> Result<String, TokenError> {
    let category = definition.category().name();
    let (definition_cell, resolved_cell) = match definition {
        TokenDefinition::Value { value } => (render_value(value, resolver), "(直接値)".to_string()),
        TokenDefinition::Reference {
            category: expected,
            reference,
        } => {
            let resolved = resolver.resolve(reference)?;
            (
                format!("→ `{reference}`（期待カテゴリ: {}）", expected.name()),
                render_value(resolved.value(), resolver),
            )
        }
    };
    Ok(format!(
        "| `{path}` | {category} | {definition_cell} | {resolved_cell} |"
    ))
}

/// 全カテゴリ・全プラットフォームのライト既定値を束ねた「解決宇宙」を構築する。
///
/// 参照（エイリアス）の解決は、それが属するセクション単体では完結しない
/// （例: `border.default` の色参照は `Palette` セクションの
/// `color.palette.black` を指す、iOS 基本テーマの `color.primary` は同じ
/// 基本テーマ内の `color.palette.ios-blue` を指す）。そのため各セクションの
/// 個別 `TokenSet` ではなく、共通カテゴリ + 各プラットフォームのライト基本
/// テーマを合成した単一の `TokenSet` から 1 つの `TokenResolver` を構築し、
/// 生成文書内の全参照解決に共有する。
///
/// プラットフォーム間で共有カテゴリ（`typography.*` / `radius.md` /
/// `duration.normal` / `animation.*` 等）の直接値が競合する場合は後勝ちで
/// 上書きされるが、これらのキーは他のトークンからエイリアス参照されない
/// （iOS/Android の参照はいずれも自身の名前空間内のパレットキーのみを指す）
/// ため、解決結果の正しさには影響しない。文書上の表示値は各セクション自身の
/// `TokenSet` から直接レンダリングするため、この合成順序の影響を受けない。
fn resolution_universe() -> Result<TokenSet, TokenError> {
    let mut set = TokenSet::new();
    set.extend(palette_token_set()?);
    set.extend(typography_token_set()?);
    set.extend(spacing_token_set()?);
    set.extend(size_token_set()?);
    set.extend(radius_token_set()?);
    set.extend(elevation_token_set()?);
    set.extend(border_token_set()?);
    set.extend(shadow_token_set()?);
    set.extend(duration_token_set()?);
    set.extend(animation_token_set()?);
    set.extend(platform_base_token_set(ThemePlatform::Ios)?);
    set.extend(platform_base_token_set(ThemePlatform::Android)?);
    Ok(set)
}

/// スキーマ概要（カテゴリ一覧・命名規則の要約）を Markdown へレンダリングする。
///
/// カテゴリ一覧は `TokenCategory::ALL`（schema.rs、S20 が追加）から導出し、
/// enum とドキュメントの二重管理を避ける。
fn render_schema_overview() -> String {
    let mut out = String::from("\n## スキーマ概要\n\n### カテゴリ一覧\n\n");
    for category in TokenCategory::ALL {
        out.push_str("- ");
        out.push_str(category.name());
        out.push('\n');
    }
    out.push_str(
        "\n### 命名規則\n\n\
        トークンパスは `category.group.name` 形式のドット区切り階層パスです\n\
        （例: `color.primary`、`spacing.md`、`duration.normal`）。\n",
    );
    out
}

/// セクション列から Markdown リファレンス全文を生成する。
///
/// `examples/generate_token_docs.rs`（ファイル書き出し）・
/// `tests/token_docgen.rs`（鮮度検証）の双方から呼び出される、本モジュールの
/// 唯一の公開エントリーポイント。決定的な出力（同一入力から常に同一
/// Markdown 文字列）を保証するため、各セクション内はカテゴリ順
/// （`TokenCategory::ALL` の宣言順）→ パス辞書順で安定ソートしてから
/// レンダリングする（`TokenSet` が `HashMap` で列挙順が不定なため）。
///
/// # Errors
///
/// いずれかのセクション内の参照解決、または既定トークンセットの再構築が
/// 失敗した場合にそのまま伝播する（既定値のみから構築するため通常は
/// 起こらないが、フォールバックせずエラーとして検出する設計、
/// coding-rust.md 準拠）。
pub fn generate_token_reference(sections: &[TokenDocSection]) -> Result<String, TokenError> {
    let universe = resolution_universe()?;
    let resolver = TokenResolver::new(universe, TokenSet::new());

    let mut out = String::from(HEADER);
    out.push_str(&render_schema_overview());

    for section in sections {
        out.push_str("\n## ");
        out.push_str(&section.title);
        out.push_str("\n\n");
        out.push_str("| トークンパス | カテゴリ | 定義 | 解決後の値 |\n");
        out.push_str("|---|---|---|---|\n");

        let mut entries: Vec<(&TokenPath, &TokenDefinition)> = section.tokens.iter().collect();
        entries.sort_by_key(|(path, definition)| {
            (category_rank(definition.category()), path.to_string())
        });

        for (path, definition) in entries {
            out.push_str(&render_row(path, definition, &resolver)?);
            out.push('\n');
        }
    }

    Ok(out)
}

/// 既定トークンセットからセクション列を構築する（共通カテゴリ →
/// プラットフォーム別テーマの順）。
///
/// `examples/generate_token_docs.rs` と `tests/token_docgen.rs` の双方が
/// 本関数を呼び出すことで、セクション構成ロジックを二重管理しない
/// （実装計画のとおり、鮮度検証テストは本関数が返す構成を「真実源」として
/// 生成結果と比較する）。
///
/// # Errors
///
/// いずれかのカテゴリ・プラットフォームの既定トークンセット構築が失敗した
/// 場合にそのまま伝播する（定義済みリテラルのみから構築するため通常は
/// 起こらない）。
pub fn default_token_doc_sections() -> Result<Vec<TokenDocSection>, TokenError> {
    Ok(vec![
        TokenDocSection {
            title: "パレット".to_string(),
            tokens: palette_token_set()?,
        },
        TokenDocSection {
            title: "タイポグラフィ".to_string(),
            tokens: typography_token_set()?,
        },
        TokenDocSection {
            title: "スペーシング".to_string(),
            tokens: spacing_token_set()?,
        },
        TokenDocSection {
            title: "サイズ".to_string(),
            tokens: size_token_set()?,
        },
        TokenDocSection {
            title: "角丸（radius）".to_string(),
            tokens: radius_token_set()?,
        },
        TokenDocSection {
            title: "高さ（elevation）".to_string(),
            tokens: elevation_token_set()?,
        },
        TokenDocSection {
            title: "ボーダー".to_string(),
            tokens: border_token_set()?,
        },
        TokenDocSection {
            title: "シャドウ".to_string(),
            tokens: shadow_token_set()?,
        },
        TokenDocSection {
            title: "duration".to_string(),
            tokens: duration_token_set()?,
        },
        TokenDocSection {
            title: "アニメーション".to_string(),
            tokens: animation_token_set()?,
        },
        TokenDocSection {
            title: "iOS 基本テーマ（ライト、セマンティックカラーを含む）".to_string(),
            tokens: platform_base_token_set(ThemePlatform::Ios)?,
        },
        TokenDocSection {
            title: "iOS ダーク差分".to_string(),
            tokens: platform_dark_overrides(ThemePlatform::Ios)?,
        },
        TokenDocSection {
            title: "Android 基本テーマ（ライト、セマンティックカラーを含む）".to_string(),
            tokens: platform_base_token_set(ThemePlatform::Android)?,
        },
        TokenDocSection {
            title: "Android ダーク差分".to_string(),
            tokens: platform_dark_overrides(ThemePlatform::Android)?,
        },
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::token::color::palette;
    use crate::token::schema::{TokenRef, TokenValue};

    fn single_value_section(title: &str, path: &str, value: TokenValue) -> TokenDocSection {
        let mut tokens = TokenSet::new();
        tokens.insert(
            TokenPath::from_dotted(path).expect("正当なパス"),
            TokenDefinition::Value { value },
        );
        TokenDocSection {
            title: title.to_string(),
            tokens,
        }
    }

    #[test]
    fn generate_token_reference_includes_header_and_regen_command() {
        let markdown = generate_token_reference(&[]).expect("空セクションでも生成できる");
        assert!(markdown.contains("自動生成"));
        assert!(markdown.contains("cargo run -p fandhe-core --example generate_token_docs"));
    }

    #[test]
    fn generate_token_reference_lists_all_categories_in_schema_overview() {
        let markdown = generate_token_reference(&[]).expect("空セクションでも生成できる");
        for category in TokenCategory::ALL {
            assert!(
                markdown.contains(category.name()),
                "カテゴリ {} が概要に含まれていません",
                category.name()
            );
        }
    }

    #[test]
    fn generate_token_reference_is_deterministic_across_runs() {
        let sections = default_token_doc_sections().expect("既定セクションを構築できる");
        let first = generate_token_reference(&sections).expect("1 回目の生成");
        let sections_again = default_token_doc_sections().expect("既定セクションを構築できる");
        let second = generate_token_reference(&sections_again).expect("2 回目の生成");
        assert_eq!(first, second, "決定的な出力であるべき");
    }

    #[test]
    fn generate_token_reference_renders_color_value_as_hex_and_rgba() {
        let section =
            single_value_section("テスト", "color.test", TokenValue::Color(palette::BLUE_500));
        let markdown = generate_token_reference(&[section]).expect("生成できる");
        assert!(markdown.contains(&palette::BLUE_500.to_hex()));
    }

    #[test]
    fn generate_token_reference_rounds_binary_float_noise_in_typography_line_height() {
        // `36.0_f32 * 1.4` は 2 進浮動小数点演算により `50.399998...` という
        // 誤差を持つ（`typography::DEFAULT_LINE_HEIGHT_RATIO` 由来の値、
        // display テーマの `line_height` で実際に発生していた回帰）。
        // `format_f32` が丸め・トリムした `50.4px` を出し、誤差の桁を
        // そのまま露出しないことを固定化する。
        use crate::token::typography::{FontStyleValue, FontWeightValue, TypographyValue};

        let noisy_line_height = 36.0_f32 * 1.4;
        assert_ne!(
            noisy_line_height, 50.4,
            "本テストの前提（2 進浮動小数点誤差の再現）が崩れています"
        );

        let typography = TypographyValue::new(
            "sans-serif",
            36.0,
            noisy_line_height,
            FontWeightValue::new(400).expect("正当なウェイト"),
            FontStyleValue::Normal,
        )
        .expect("正当なタイポグラフィ値");
        let section = single_value_section(
            "テスト",
            "typography.test",
            TokenValue::Typography(typography),
        );

        let markdown = generate_token_reference(&[section]).expect("生成できる");
        assert!(markdown.contains("line_height=50.4px"));
        assert!(!markdown.contains("50.399998"));
    }

    #[test]
    fn generate_token_reference_escapes_pipe_and_backtick_in_text_value() {
        let section =
            single_value_section("テスト", "text.test", TokenValue::Text("a|b`c".to_string()));
        let markdown = generate_token_reference(&[section]).expect("生成できる");
        assert!(markdown.contains("a\\|b\\`c"));
        // エスケープ後の値セルは生の `|`（列区切りとの衝突）を追加で持ち込まない
        // ことを、期待される列数（4 列 = 区切り 5 本、うち 1 本はエスケープ済み
        // 値内の `\|`）で確認する。
        let row_line = markdown
            .lines()
            .find(|line| line.contains("text.test"))
            .expect("該当行が存在する");
        assert_eq!(row_line.matches('|').count(), 6);
    }

    #[test]
    fn generate_token_reference_resolves_embedded_border_color_reference() {
        // BorderValue.color は TokenRef（border.default は
        // color.palette.gray-300 を参照）を埋め込みで保持し、
        // TokenDefinition::Reference を経由しない。render_value が resolver
        // 経由で解決後の色値まで表示することを確認する。
        let section = TokenDocSection {
            title: "ボーダー".to_string(),
            tokens: border_token_set().expect("既定ボーダートークンセットを構築できる"),
        };
        let markdown = generate_token_reference(&[section]).expect("生成できる");
        assert!(markdown.contains("border.default"));
        assert!(markdown.contains("color.palette.gray-300"));
        assert!(markdown.contains(&palette::GRAY_300.to_hex()));
    }

    #[test]
    fn generate_token_reference_propagates_error_for_undefined_reference() {
        let mut tokens = TokenSet::new();
        tokens.insert(
            TokenPath::from_dotted("color.dangling").expect("正当なパス"),
            TokenDefinition::Reference {
                category: TokenCategory::Color,
                reference: TokenRef::new(
                    TokenPath::from_dotted("color.palette.does-not-exist").expect("正当なパス"),
                ),
            },
        );
        let section = TokenDocSection {
            title: "テスト".to_string(),
            tokens,
        };
        let result = generate_token_reference(&[section]);
        assert!(
            result.is_err(),
            "未定義参照はエラーとして伝播し、握り潰されないべき"
        );
    }

    #[test]
    fn default_token_doc_sections_builds_without_error_and_covers_expected_titles() {
        let sections = default_token_doc_sections().expect("既定セクションを構築できる");
        let titles: HashSet<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains("パレット"));
        assert!(titles.contains("iOS ダーク差分"));
        assert!(titles.contains("Android ダーク差分"));
        assert_eq!(sections.len(), 14);
    }
}
