//! トークンセット全体の事前検証・エラー集約報告（TASK-8.4-S18、#174）。
//!
//! `docs/architecture/token-resolution-architecture.md` §4.2・§5.3、
//! `docs/architecture/design-token-architecture.md` §6 が定める「S17（#173、
//! 外部定義ファイル読み込み）→ S18（本モジュール、バリデーション）→ 解決エンジン
//! （`super::resolver::TokenResolver`）」というデータフロー契約の中間層を担う。
//! `super::resolver` が個々の参照を解決する際に遅延検出する 4 分類
//! （`TokenError::UndefinedReference` / `TypeMismatch` / `CircularReference` /
//! `MaxDepthExceeded`）を、トークンセット全体に対して fail-fast せず一括検証し、
//! 起点トークンパスと関連付けた決定的順序のレポートへ集約する。
//!
//! **責務分担**: 各値型（`ColorValue` 等）のフォーマット妥当性（16 進表記・
//! 非有限値等）は構築時検証（S6〜S13）が型の不変条件として既に保証済みのため、
//! 本モジュールは再検証しない。本モジュールが検証するのは「参照・エイリアスの
//! グラフとしての整合性」（未定義参照・型不一致・循環・深さ上限・テーマ差分の
//! カテゴリ宣言不整合）のみである。
//!
//! **セキュリティ（OWASP A03 / A05）**: 検証はメモリ上のマップ走査のみで完結し、
//! トークンパス・エラー文言をコマンド・ファイルパス・クエリへ展開しない
//! （`design-token-architecture.md` §5.2・§7 が定める「文字列由来キーの実行時
//! 解決は検証付き経路に限定する」契約の検証側実体）。チェーン走査は解決エンジンと
//! 同じ `MAX_ALIAS_DEPTH` 上限・明示的ループを共有するため、検証コストは
//! O(キー数 × 深さ上限) で有界であり、DoS 耐性も解決エンジンと同一である。

use std::collections::HashSet;
use std::fmt;

use super::error::TokenError;
use super::resolver::resolve_chain_in;
use super::schema::{TokenPath, TokenRef, TokenSet};

/// 検証で検出された個別の問題（起点トークンパスとエラーの関連付け）。
///
/// `origin` は「どのトークンキーの検証中に検出されたか」を表す。エイリアス
/// チェーンの途中で検出された場合も、チェーンの起点（`validate_token_sets` が
/// 走査を開始したキー）を保持する（`TokenError` 自体が持つチェーン内部の
/// パス情報と、レポート上での関連付けを分離するため）。
#[derive(Debug, Clone, PartialEq)]
pub struct TokenValidationIssue {
    origin: TokenPath,
    error: TokenError,
}

impl TokenValidationIssue {
    /// 問題の起点となったトークンパスを返す。
    pub fn origin(&self) -> &TokenPath {
        &self.origin
    }

    /// 検出されたエラーを返す。
    pub fn error(&self) -> &TokenError {
        &self.error
    }
}

impl fmt::Display for TokenValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.origin, self.error)
    }
}

/// トークンセット全体の検証結果を集約するレポート（設計文書 §4.2「集約・報告する
/// 上位層」の実体）。
///
/// fail-fast せず全キーの検証結果を保持する。「開発者・AI 双方に解釈しやすい形」
/// （REQ-13 の思想）で報告するため、`Display` は決定的順序（起点パスの文字列表現
/// でソート済み）の複数行レポートを出力する。
#[derive(Debug, Clone, PartialEq)]
pub struct TokenValidationReport {
    issues: Vec<TokenValidationIssue>,
}

impl TokenValidationReport {
    /// 検出された問題一覧を参照で返す（決定的順序、`validate_token_sets` 参照）。
    pub fn issues(&self) -> &[TokenValidationIssue] {
        &self.issues
    }

    /// 検出された問題の件数を返す。
    pub fn len(&self) -> usize {
        self.issues.len()
    }

    /// 問題が 1 件も検出されなかった場合に `true` を返す。
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

impl fmt::Display for TokenValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "トークンセットの検証で {} 件の問題を検出しました:",
            self.issues.len()
        )?;
        for issue in &self.issues {
            writeln!(f, "- {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TokenValidationReport {}

/// `TokenSet`（ベース + テーマ差分）全体を事前検証するゲートウェイ
/// （設計文書 §5.3 の「S17 → S18 → 解決エンジン」データフロー契約の実体）。
///
/// `TokenResolver::validated` が本関数を経由して構築を許可するかを判定する。
/// ベース・テーマ差分の全キーの和集合に対し、以下を検証する:
///
/// 1. 各エントリのエイリアスチェーンを辿り、解決エンジンと同一のルックアップ
///    優先順位（テーマ差分 → ベース）・DoS 対策（`MAX_ALIAS_DEPTH`・訪問済み
///    集合）で `UndefinedReference` / `TypeMismatch` / `CircularReference` /
///    `MaxDepthExceeded` を検出する。
/// 2. テーマ差分がベースと同一キーで異なるカテゴリを宣言していないかを検査する
///    （`ThemeOverrideCategoryMismatch`。チェーンを辿らないキー単位の宣言
///    不整合であり、1. の `TypeMismatch` 検出では捕捉できない）。
/// 3. ベースで定義されている各キーについて、テーマ差分を一切適用しない
///    （空集合として扱う）単独ルックアップでも同じチェーンが解決できるかを
///    追加で検証する（TASK-8.4-S18、#174）。1. のテーマ差分
///    優先ルックアップだけでは、同一キーがテーマ差分側で有効な値に上書き
///    されている場合にベース側の破損（未定義参照・型不一致等）が検証を
///    すり抜ける。`platform_dark_overrides` 等が「ライト/ダークで値が同一の
///    キーは意図的にテーマ差分へ含めず、ベース値へのフォールバックに委ねる」
///    契約（同モジュール doc）を前提とする設計である以上、ベースはテーマ差分
///    無しでも自己完結して解決できる必要がある。この単独チェックにより、
///    後から `TokenResolver::set_theme_overrides` で当該キーの上書きが外れる
///    テーマ切替が発生しても、`validated` の「検証済みセットのみで構築する」
///    保証が壊れないことを型構築時点で担保する。同一キーについて 1. の
///    チェーン解決（`resolve_chain_in`）で既に問題を検出済みの場合は重複
///    報告しない。ただし 2. の `ThemeOverrideCategoryMismatch`（キー単位の
///    宣言不一致でチェーンを辿らない）のみが検出された場合はベース側
///    チェーンの自己完結性を未検証のままにしないため、3. の対象から除外
///    しない。
///
/// **決定的順序**: `HashMap` のイテレーション順に依存しないよう、1.・2. の
/// テーマ差分適用ビューの走査、3. のベース単独ビューの走査それぞれで検証対象
/// キーを起点パスの文字列表現でソートしてから走査し、最終的に集約した
/// `issues` 全体も起点パスの文字列表現で安定ソートする（同一起点内では
/// テーマ差分適用ビュー由来の問題がベース単独ビュー由来の問題より先に来る
/// 順序を維持する）。これにより同一入力に対し `Display` 出力が常に一致する
/// （開発者・AI が diff で比較できる形式にするため）。
///
/// # Errors
///
/// 1 件以上の問題を検出した場合、全件を集約した `TokenValidationReport` を返す。
pub fn validate_token_sets(
    base: &TokenSet,
    theme_overrides: &TokenSet,
) -> Result<(), TokenValidationReport> {
    let mut keys: Vec<TokenPath> =
        HashSet::<&TokenPath>::from_iter(base.keys().chain(theme_overrides.keys()))
            .into_iter()
            .cloned()
            .collect();
    keys.sort_by_key(TokenPath::to_string);

    let mut issues = Vec::new();
    // 3. の対象から除外する起点は「1. のチェーン解決（resolve_chain_in）で
    // 問題が検出された起点」のみに限定する。2. の
    // `ThemeOverrideCategoryMismatch` はチェーンを辿らないキー単位の宣言
    // 不一致であり、ベース側チェーンの自己完結性については何も検証していない
    // ため、これのみが検出された起点を 3. の対象から除外してしまうと、
    // カテゴリ不一致と組み合わさったベース側の破損（未定義参照等）を
    // 見逃す（TASK-8.4-S18、#174）。
    let mut flagged_origins: HashSet<TokenPath> = HashSet::new();
    for path in &keys {
        if let Some(issue) = check_category_mismatch(base, theme_overrides, path) {
            issues.push(issue);
        }

        let reference = TokenRef::new(path.clone());
        if let Err(error) = resolve_chain_in(base, theme_overrides, &reference) {
            flagged_origins.insert(path.clone());
            issues.push(TokenValidationIssue {
                origin: path.clone(),
                error,
            });
        }
    }

    // 3. の単独チェック（ベースのみのキーを対象に、テーマ差分を空集合として
    // 再走査する）。1. のチェーン解決で既に問題を報告済みのキーのみ対象外とする
    // （2. のカテゴリ不一致はチェーンの自己完結性を検証していないため対象外にしない）。
    let empty_overrides = TokenSet::new();
    let mut base_keys: Vec<TokenPath> = base.keys().cloned().collect();
    base_keys.sort_by_key(TokenPath::to_string);
    for path in &base_keys {
        if flagged_origins.contains(path) {
            continue;
        }
        let reference = TokenRef::new(path.clone());
        if let Err(error) = resolve_chain_in(base, &empty_overrides, &reference) {
            issues.push(TokenValidationIssue {
                origin: path.clone(),
                error,
            });
        }
    }

    // 1./2. のテーマ差分適用ビューと 3. のベース単独ビューを別々の走査で
    // 集めたため、最終的な報告順序を起点パスの文字列表現で安定ソートし直す
    // （安定ソートのため、同一起点内の相対順序＝テーマ差分適用ビュー由来が
    // ベース単独ビュー由来より先、は維持される）。
    issues.sort_by_key(|issue| issue.origin.to_string());

    if issues.is_empty() {
        Ok(())
    } else {
        Err(TokenValidationReport { issues })
    }
}

/// テーマ差分がベースと同一キーに異なるカテゴリの定義を直接宣言していないかを
/// 検査する（設計文書 §6「型不一致の検出とエラー報告形式」契約の一部）。
///
/// 両方のマップに同一キーが存在する場合のみ比較する（片方にしかないキーは
/// 通常のエイリアス・値定義であり、本チェックの対象外）。
fn check_category_mismatch(
    base: &TokenSet,
    theme_overrides: &TokenSet,
    path: &TokenPath,
) -> Option<TokenValidationIssue> {
    let base_definition = base.get(path)?;
    let override_definition = theme_overrides.get(path)?;

    let base_category = base_definition.category();
    let override_category = override_definition.category();
    if base_category == override_category {
        return None;
    }

    Some(TokenValidationIssue {
        origin: path.clone(),
        error: TokenError::ThemeOverrideCategoryMismatch {
            path: path.to_string(),
            base: base_category.name(),
            overriding: override_category.name(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::color::ColorValue;
    use super::super::schema::{TokenCategory, TokenDefinition, TokenValue};
    use super::*;

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    fn value_def(value: TokenValue) -> TokenDefinition {
        TokenDefinition::Value { value }
    }

    fn reference_def(category: TokenCategory, target: &str) -> TokenDefinition {
        TokenDefinition::Reference {
            category,
            reference: TokenRef::new(path(target)),
        }
    }

    fn color(byte: u8) -> TokenValue {
        TokenValue::Color(ColorValue::rgb(byte, byte, byte))
    }

    #[test]
    fn valid_set_passes_validation() {
        let mut base = TokenSet::new();
        base.insert(path("color.primary"), value_def(color(0x01)));
        base.insert(
            path("button.background"),
            reference_def(TokenCategory::Color, "color.primary"),
        );
        assert!(validate_token_sets(&base, &TokenSet::new()).is_ok());
    }

    #[test]
    fn empty_sets_pass_validation() {
        assert!(validate_token_sets(&TokenSet::new(), &TokenSet::new()).is_ok());
    }

    #[test]
    fn theme_overrides_only_set_can_pass_validation() {
        let mut overrides = TokenSet::new();
        overrides.insert(path("color.primary"), value_def(color(0x02)));
        assert!(validate_token_sets(&TokenSet::new(), &overrides).is_ok());
    }

    #[test]
    fn all_undefined_references_are_aggregated_in_one_call() {
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "missing.a"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "missing.b"));
        let report = validate_token_sets(&base, &TokenSet::new())
            .expect_err("未定義参照を含むセットは検証を通過しない");
        assert_eq!(report.len(), 2);
        assert!(
            report
                .issues()
                .iter()
                .all(|issue| matches!(issue.error(), TokenError::UndefinedReference { .. }))
        );
    }

    #[test]
    fn self_reference_cycle_is_reported_with_origin() {
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "a"));
        let report =
            validate_token_sets(&base, &TokenSet::new()).expect_err("自己参照は検証で検出される");
        assert_eq!(report.len(), 1);
        let issue = &report.issues()[0];
        assert_eq!(issue.origin(), &path("a"));
        assert!(matches!(
            issue.error(),
            TokenError::CircularReference { .. }
        ));
    }

    #[test]
    fn indirect_cycle_is_reported_for_both_origins() {
        // a -> b -> a の間接循環は a・b いずれを起点にしても検出されるため、
        // 和集合走査では両方が問題として報告される（決定的順序の確認も兼ねる）。
        let mut base = TokenSet::new();
        base.insert(path("a"), reference_def(TokenCategory::Color, "b"));
        base.insert(path("b"), reference_def(TokenCategory::Color, "a"));
        let report =
            validate_token_sets(&base, &TokenSet::new()).expect_err("間接循環は検証で検出される");
        assert_eq!(report.len(), 2);
        assert!(
            report
                .issues()
                .iter()
                .all(|issue| matches!(issue.error(), TokenError::CircularReference { .. }))
        );
    }

    #[test]
    fn type_mismatch_across_categories_is_aggregated() {
        let mut base = TokenSet::new();
        base.insert(
            path("color.primary"),
            reference_def(TokenCategory::Color, "spacing.md"),
        );
        base.insert(path("spacing.md"), value_def(TokenValue::Dimension(8.0)));
        let report = validate_token_sets(&base, &TokenSet::new())
            .expect_err("カテゴリ跨ぎ参照は検証で検出される");
        assert!(
            report
                .issues()
                .iter()
                .any(|issue| matches!(issue.error(), TokenError::TypeMismatch { .. }))
        );
    }

    #[test]
    fn max_depth_exceeded_is_reported_without_hanging() {
        use super::super::resolver::MAX_ALIAS_DEPTH;

        let mut base = TokenSet::new();
        let chain_len = MAX_ALIAS_DEPTH as usize + 8;
        for i in 0..chain_len {
            let key = format!("t{i}");
            let next = format!("t{}", i + 1);
            base.insert(path(&key), reference_def(TokenCategory::Color, &next));
        }
        base.insert(path(&format!("t{chain_len}")), value_def(color(0x03)));
        let report = validate_token_sets(&base, &TokenSet::new())
            .expect_err("上限を超えるチェーンは検証で検出される");
        assert!(
            report
                .issues()
                .iter()
                .any(|issue| matches!(issue.error(), TokenError::MaxDepthExceeded { .. }))
        );
    }

    #[test]
    fn theme_override_category_mismatch_is_detected() {
        let mut base = TokenSet::new();
        base.insert(path("shared.token"), value_def(color(0x04)));

        let mut overrides = TokenSet::new();
        overrides.insert(path("shared.token"), value_def(TokenValue::Dimension(4.0)));

        let report = validate_token_sets(&base, &overrides)
            .expect_err("テーマ差分のカテゴリ不一致は検証で検出される");
        assert_eq!(report.len(), 1);
        let issue = &report.issues()[0];
        assert_eq!(issue.origin(), &path("shared.token"));
        assert!(matches!(
            issue.error(),
            TokenError::ThemeOverrideCategoryMismatch { .. }
        ));
    }

    #[test]
    fn category_mismatch_does_not_hide_base_only_chain_error() {
        // レビュー指摘再現（#174 discussion_r3689300744）: テーマ差分が有効な
        // 値で上書きしているため 1. のテーマ差分適用チェーン解決は成功するが、
        // ベース定義自体は未定義参照を指しており、ベース単独では解決できない。
        // カテゴリ不一致の検出だけで 3. のスキップ対象にしてしまうと、この
        // ベース側の `UndefinedReference` が報告から漏れる。
        let mut base = TokenSet::new();
        base.insert(
            path("shared.token"),
            reference_def(TokenCategory::Color, "missing.base_only"),
        );

        let mut overrides = TokenSet::new();
        overrides.insert(path("shared.token"), value_def(TokenValue::Dimension(4.0)));

        let report = validate_token_sets(&base, &overrides)
            .expect_err("カテゴリ不一致とベース単独の未定義参照の両方が検証で検出される");
        assert_eq!(report.len(), 2);
        assert!(report.issues().iter().any(|issue| matches!(
            issue.error(),
            TokenError::ThemeOverrideCategoryMismatch { .. }
        )));
        assert!(
            report
                .issues()
                .iter()
                .any(|issue| matches!(issue.error(), TokenError::UndefinedReference { .. }))
        );
    }

    #[test]
    fn report_display_is_deterministic_across_repeated_calls() {
        let mut base = TokenSet::new();
        base.insert(
            path("z.token"),
            reference_def(TokenCategory::Color, "missing.z"),
        );
        base.insert(
            path("a.token"),
            reference_def(TokenCategory::Color, "missing.a"),
        );

        let first = validate_token_sets(&base, &TokenSet::new())
            .expect_err("未定義参照は検証で検出される")
            .to_string();
        let second = validate_token_sets(&base, &TokenSet::new())
            .expect_err("未定義参照は検証で検出される")
            .to_string();
        assert_eq!(first, second);
        // 起点パスの文字列表現でソートされているため a.token が先に出現する。
        assert!(first.find("a.token").unwrap() < first.find("z.token").unwrap());
    }
}
