//! セマンティックカラー（役割別の色トークン）とパレット参照の橋渡し
//! （TASK-8.4-S7、#163）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）の
//! §4.4「参照・エイリアスの意味論」が定める契約（「セマンティックカラー
//! `color.primary` がパレット `color.palette.blue-500` を参照する」）を、
//! 閉じた列挙 [`SemanticColorRole`] による型安全なパス生成・エントリ構築 API
//! として実装する。同文書の分担表（§1「#157〜#180 の分担表」）は S7（#163）の
//! 責務を「§4.4 のエイリアス意味論（セマンティック → パレット参照）を実装」と
//! 定めており、既定テーマの具体色値（S16、#172）・パレット自体の型実装
//! （S6、#162）は本モジュールのスコープ外である。
//!
//! `schema::TokenValue::Color` は [`super::color::ColorValue`]（S6、#162）を保持する。
//! 本モジュールは `TokenValue` をそのまま扱うため `ColorValue` 自体には直接依存
//! しない（`resolve_semantic_color` の戻り値を `TokenValue` に留め、色の内部表現の
//! 変更が呼び出し元へ波及する範囲を局所化している）。
//!
//! 呼び出し文脈: S14（基本ウィジェットへのトークン適用配線、#170）・S19
//! （AI 型安全 DSL からのトークン参照 API、#175）が、ウィジェットの見た目を
//! 「役割」でのみ指定するための入口として本モジュールを利用する想定。

use super::error::TokenError;
use super::resolver::TokenResolver;
use super::schema::{TokenCategory, TokenDefinition, TokenPath, TokenRef, TokenSet, TokenValue};
use super::typed::ColorTokenRef;

#[cfg(test)]
use super::color::ColorValue;

/// セマンティックカラーの役割を表す閉じた列挙（設計文書 §4.2 の命名規則
/// `color.<role>` に対応するパスセグメントを、型として固定する）。
///
/// 初期セットは設計文書 §4.2 の命名規則と一般的なデザインシステム
/// （Material Design / Apple HIG）の交差を採り最小限に絞った。具体的な
/// ライト/ダーク既定値の割当は S16（#172）のスコープであり、本列挙は
/// 役割の型と参照機構のみを提供する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticColorRole {
    /// 主要な強調色（ボタンの主背景等）。
    Primary,
    /// 副次的な強調色。
    Secondary,
    /// 画面全体の背景色。
    Background,
    /// カード・パネル等、背景の上に重なる面の色。
    Surface,
    /// エラー状態を示す色。
    Error,
    /// 警告状態を示す色。
    Warning,
    /// 成功状態を示す色。
    Success,
    /// `Primary` の上に描画する内容（テキスト・アイコン）の色。
    OnPrimary,
    /// `Background` の上に描画する内容の色。
    OnBackground,
    /// `Surface` の上に描画する内容の色。
    OnSurface,
}

impl SemanticColorRole {
    /// テスト・カタログ列挙用に、全バリアントを網羅した配列を返す
    /// （設計文書 §4.2 の命名規則に沿ったパス生成をバリアント追加時に
    /// 機械的に検証できるようにするための一覧。S20〔#176〕のドキュメント
    /// 自動生成が同様の全件走査を必要になった際にも流用できる）。
    pub const ALL: [SemanticColorRole; 10] = [
        Self::Primary,
        Self::Secondary,
        Self::Background,
        Self::Surface,
        Self::Error,
        Self::Warning,
        Self::Success,
        Self::OnPrimary,
        Self::OnBackground,
        Self::OnSurface,
    ];

    /// パスセグメント名（設計文書 §4.2 のハイフン区切り小文字表記）を返す。
    pub fn name(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Background => "background",
            Self::Surface => "surface",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Success => "success",
            Self::OnPrimary => "on-primary",
            Self::OnBackground => "on-background",
            Self::OnSurface => "on-surface",
        }
    }

    /// この役割に対応する `color.<name>` の `TokenPath` を生成する。
    ///
    /// `name()` が返すセグメントは本モジュール内で完結する `'static` な
    /// 許可文字集合（英数字・`-`）のみの検証済みリテラルであり、
    /// `TokenPath::new` の検証（`schema.rs` 参照）を必ず通過する。外部入力
    /// 文字列を経由しないため、`unwrap()` 等でエラーを握り潰すのではなく、
    /// この構造的な保証を呼び出し元へそのまま伝播する `Result` を返す
    /// （OWASP A03 対策: 閉じた列挙からのみパスを生成し、外部入力文字列を
    /// パス構築へ渡さない、`.claude/rules/security.md` 参照）。
    pub fn token_path(self) -> Result<TokenPath, TokenError> {
        TokenPath::new(vec!["color".to_string(), self.name().to_string()])
    }

    /// 解決エンジンへ渡す `TokenRef` を生成する。
    pub fn token_ref(self) -> Result<TokenRef, TokenError> {
        self.token_path().map(TokenRef::new)
    }

    /// [`Self::token_ref`] の型付き版（TASK-8.4-S19、#175）。
    ///
    /// セマンティックカラーは常に color カテゴリを指すため、`typed::ColorKind` で
    /// 固定した [`ColorTokenRef`] を返す。DSL 生成コードの主経路（「ロール列挙 →
    /// 型付き参照」）を型安全に揃えるための入口であり、`token_ref()` を薄く包む
    /// だけで新たな検証・パース経路は追加しない。
    pub fn typed_ref(self) -> Result<ColorTokenRef, TokenError> {
        self.token_ref().map(ColorTokenRef::from_ref)
    }
}

/// セマンティックカラー 1 件分のトークンエントリ（パス・定義のペア）を構築する。
///
/// 設計文書 §4.4「色を要求するフィールドは color カテゴリのトークンのみを
/// 参照できる」という制約を、`TokenDefinition::Reference { category:
/// TokenCategory::Color, .. }` の構築時点で型として固定する（呼び出し元が
/// 誤って他カテゴリを指定できない）。
///
/// # Errors
///
/// `role.token_path()` が失敗した場合（本モジュール内の実装ミスでない限り
/// 通常は起こらない）にそのままエラーを伝播する。
pub fn semantic_color_entry(
    role: SemanticColorRole,
    palette_ref: TokenRef,
) -> Result<(TokenPath, TokenDefinition), TokenError> {
    let path = role.token_path()?;
    let definition = TokenDefinition::Reference {
        category: TokenCategory::Color,
        reference: palette_ref,
    };
    Ok((path, definition))
}

/// 複数のセマンティックカラー役割をまとめて構築するマッピング。
///
/// ベーストークンセット・テーマ差分（`TokenResolver::set_theme_overrides`）の
/// いずれに対しても同一構造で使える（設計文書 §3.3「ベース・テーマ差分は
/// 同一構造」との整合。S5〔#161〕がテーマ切替時の差分構築にそのまま流用する
/// 想定）。
#[derive(Debug, Clone, Default)]
pub struct SemanticColorMapping {
    entries: Vec<(SemanticColorRole, TokenRef)>,
}

impl SemanticColorMapping {
    /// 空のマッピングを構築する。
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 役割とパレット参照の対応を 1 件追加する（ビルダースタイル）。
    ///
    /// 同一役割を複数回登録した場合は最後の登録が有効になる
    /// （`into_token_set` / `merge_into` が `HashMap` へ挿入する際に
    /// 後勝ちで上書きされる、`TokenSet` の通常の挿入意味論と同一）。
    pub fn with(mut self, role: SemanticColorRole, palette_ref: TokenRef) -> Self {
        self.entries.push((role, palette_ref));
        self
    }

    /// 保持する対応を独立した `TokenSet` へ変換する。
    ///
    /// # Errors
    ///
    /// いずれかの役割のパス生成が失敗した場合、最初に検出したエラーを返す。
    pub fn into_token_set(self) -> Result<TokenSet, TokenError> {
        let mut set = TokenSet::new();
        self.merge_into(&mut set)?;
        Ok(set)
    }

    /// 保持する対応を既存の `TokenSet` へマージする（ベース・テーマ差分の
    /// 構築いずれにも使える窓口。既存キーは上書きする）。
    ///
    /// # Errors
    ///
    /// いずれかの役割のパス生成が失敗した場合、最初に検出したエラーを返す
    /// （途中まで挿入済みの `set` は部分的に更新された状態のまま残る）。
    pub fn merge_into(&self, set: &mut TokenSet) -> Result<(), TokenError> {
        for (role, palette_ref) in &self.entries {
            let (path, definition) = semantic_color_entry(*role, palette_ref.clone())?;
            set.insert(path, definition);
        }
        Ok(())
    }
}

/// セマンティックカラー役割を解決エンジン経由で具体値まで解決する。
///
/// `resolver.resolve` の結果が `TokenValue::Color` であることを検証し、
/// 不一致は `TokenError::TypeMismatch` として報告する（セマンティックカラーの
/// 参照先が誤って color 以外のカテゴリを指す定義になっていた場合の防御。
/// 通常は `semantic_color_entry` が構築時点で `category: TokenCategory::Color`
/// を固定するため到達しない想定だが、`TokenSet` を直接構築する呼び出し元
/// （テスト等）が契約を破った場合の安全網として残す）。
///
/// 未登録の役割は `resolver.resolve` が返す `TokenError::UndefinedReference`
/// をそのまま伝播する（フォールバック値を自動補完しない、設計文書 §6 /
/// resolver.rs の契約）。戻り値は `ColorValue` の中身を直接晒さず `TokenValue`
/// のまま返し、色の内部表現（`ColorValue` の構造）が変わった場合の影響を
/// 呼び出し元から局所化する。
pub fn resolve_semantic_color(
    resolver: &TokenResolver,
    role: SemanticColorRole,
) -> Result<TokenValue, TokenError> {
    let token_ref = role.token_ref()?;
    let resolved = resolver.resolve(&token_ref)?;
    let value = resolved.value().clone();
    match value.category() {
        TokenCategory::Color => Ok(value),
        actual => Err(TokenError::TypeMismatch {
            origin: token_ref.to_string(),
            reference: token_ref.to_string(),
            expected: TokenCategory::Color.name(),
            actual: actual.name(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    #[test]
    fn all_roles_produce_valid_color_dot_prefixed_paths() {
        for role in SemanticColorRole::ALL {
            let token_path = role.token_path().expect("全役割はパス生成に成功する");
            let segments = token_path.segments();
            assert_eq!(segments[0], "color");
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[1], role.name());
        }
    }

    #[test]
    fn on_primary_role_uses_hyphenated_segment() {
        let token_path = SemanticColorRole::OnPrimary
            .token_path()
            .expect("ハイフンを含む役割名もパス生成に成功する");
        assert_eq!(token_path.to_string(), "color.on-primary");
    }

    #[test]
    fn typed_ref_resolves_via_typed_token_ref_api() {
        // TASK-8.4-S19（#175）: typed_ref() が返す ColorTokenRef が
        // TypedTokenRef::resolve 経由でも正しく解決できることを確認する。
        let mut base = TokenSet::new();
        base.insert(
            path("color.palette.blue-500"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0x33, 0x66, 0xff)),
            },
        );
        let mapping = SemanticColorMapping::new().with(
            SemanticColorRole::Primary,
            TokenRef::new(path("color.palette.blue-500")),
        );
        mapping
            .merge_into(&mut base)
            .expect("マッピングはベースへマージできる");

        let resolver = TokenResolver::new(base, TokenSet::new());
        let typed_ref = SemanticColorRole::Primary
            .typed_ref()
            .expect("型付き参照の生成に成功する");
        let value = typed_ref
            .resolve(&resolver)
            .expect("型付き参照経由でも色トークンを解決できる");
        assert_eq!(value, ColorValue::rgb(0x33, 0x66, 0xff));
    }

    #[test]
    fn resolves_semantic_color_to_palette_value() {
        let mut base = TokenSet::new();
        base.insert(
            path("color.palette.blue-500"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0x33, 0x66, 0xff)),
            },
        );
        let mapping = SemanticColorMapping::new().with(
            SemanticColorRole::Primary,
            TokenRef::new(path("color.palette.blue-500")),
        );
        mapping
            .merge_into(&mut base)
            .expect("マッピングはベースへマージできる");

        let resolver = TokenResolver::new(base, TokenSet::new());
        let value = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect("セマンティック → パレット参照は解決できる");
        assert_eq!(value, TokenValue::Color(ColorValue::rgb(0x33, 0x66, 0xff)));
    }

    #[test]
    fn theme_override_swaps_semantic_color_target() {
        // S5（#161）との契約検証: テーマ差分でセマンティックエントリの参照先を
        // 別パレット色に差し替えると解決結果が変わること。
        let mut base = TokenSet::new();
        base.insert(
            path("color.palette.blue-500"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0xee, 0xee, 0xee)),
            },
        );
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Primary,
                TokenRef::new(path("color.palette.blue-500")),
            )
            .merge_into(&mut base)
            .expect("ベースへマージできる");

        let mut dark_overrides = TokenSet::new();
        dark_overrides.insert(
            path("color.palette.blue-900"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0x11, 0x11, 0x11)),
            },
        );
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Primary,
                TokenRef::new(path("color.palette.blue-900")),
            )
            .merge_into(&mut dark_overrides)
            .expect("テーマ差分へマージできる");

        let resolver = TokenResolver::new(base, dark_overrides);
        let value = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect("テーマ差分側の参照先が解決される");
        assert_eq!(value, TokenValue::Color(ColorValue::rgb(0x11, 0x11, 0x11)));
    }

    #[test]
    fn type_mismatch_when_semantic_color_references_dimension() {
        let mut base = TokenSet::new();
        base.insert(
            path("spacing.md"),
            TokenDefinition::Value {
                value: TokenValue::Dimension(8.0),
            },
        );
        // semantic_color_entry は常に category: Color を固定するため、resolver 側の
        // 型不一致検出（参照先が実際には Dimension）がそのまま伝播することを確認する。
        let (semantic_path, semantic_def) = semantic_color_entry(
            SemanticColorRole::Primary,
            TokenRef::new(path("spacing.md")),
        )
        .expect("エントリ構築自体は成功する");
        base.insert(semantic_path, semantic_def);

        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect_err("色以外のカテゴリを参照した場合は型不一致になる");
        assert!(matches!(err, TokenError::TypeMismatch { .. }));
    }

    #[test]
    fn undefined_palette_reference_is_reported() {
        let resolver = TokenResolver::new(TokenSet::new(), TokenSet::new());
        let err = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect_err("未登録の役割は未定義参照になる");
        assert!(matches!(err, TokenError::UndefinedReference { .. }));
    }

    #[test]
    fn multi_step_semantic_alias_resolves() {
        // セマンティック → セマンティック → パレットの 2 段参照。
        let mut base = TokenSet::new();
        base.insert(
            path("color.palette.green-500"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0x00, 0xff, 0x00)),
            },
        );
        SemanticColorMapping::new()
            .with(
                SemanticColorRole::Success,
                TokenRef::new(path("color.palette.green-500")),
            )
            .merge_into(&mut base)
            .expect("成功色エントリをマージできる");
        // Secondary が Success の色を再利用するセマンティック間エイリアス。
        let success_path = SemanticColorRole::Success
            .token_path()
            .expect("パス生成に成功する");
        let (secondary_path, secondary_def) =
            semantic_color_entry(SemanticColorRole::Secondary, TokenRef::new(success_path))
                .expect("セマンティック間参照のエントリを構築できる");
        base.insert(secondary_path, secondary_def);

        let resolver = TokenResolver::new(base, TokenSet::new());
        let value = resolve_semantic_color(&resolver, SemanticColorRole::Secondary)
            .expect("2 段のセマンティックエイリアスは解決できる");
        assert_eq!(value, TokenValue::Color(ColorValue::rgb(0x00, 0xff, 0x00)));
    }

    #[test]
    fn circular_semantic_reference_is_detected() {
        // セマンティック同士の相互参照（Primary <-> Secondary）。
        let mut base = TokenSet::new();
        let primary_path = SemanticColorRole::Primary
            .token_path()
            .expect("パス生成に成功する");
        let secondary_path = SemanticColorRole::Secondary
            .token_path()
            .expect("パス生成に成功する");
        base.insert(
            primary_path,
            TokenDefinition::Reference {
                category: TokenCategory::Color,
                reference: TokenRef::new(secondary_path.clone()),
            },
        );
        base.insert(
            secondary_path,
            TokenDefinition::Reference {
                category: TokenCategory::Color,
                reference: TokenRef::new(
                    SemanticColorRole::Primary
                        .token_path()
                        .expect("パス生成に成功する"),
                ),
            },
        );

        let resolver = TokenResolver::new(base, TokenSet::new());
        let err = resolve_semantic_color(&resolver, SemanticColorRole::Primary)
            .expect_err("セマンティック同士の相互参照は循環として検出される");
        assert!(matches!(err, TokenError::CircularReference { .. }));
    }
}
