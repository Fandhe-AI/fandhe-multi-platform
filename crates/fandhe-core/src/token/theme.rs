//! テーマ切替機構のデータ型（TASK-8.4-S5、#161）。
//!
//! `docs/architecture/theme-switching-architecture.md`（TASK-8.4-S4、#160 の
//! 成果物）§2〜§4 を実装契約としてそのまま Rust コード化する（同文書 §9 が
//! 「本文書 §2〜§4 の型・API・伝搬フローをそのまま実装する」ことを S5 のスコープ
//! と明記している）。二層アーキテクチャ（REQ-2）上は共通コアに閉じ、OS の
//! ライト/ダーク検出等の具体実装はプラットフォーム統合レイヤの責務とする
//! （§2.4・§3.3）。
//!
//! ## モジュール配置（設計文書からの乖離）
//!
//! 設計文書 §2.2 は `crates/fandhe-core/src/tokens/theme.rs`（複数形の
//! `tokens/`）を配置案としつつ、「実ファイル分割は S5 の実装判断に委ねる」と
//! 留保している（同文書 §7「モジュール配置の表記乖離」も参照）。実リポジトリの
//! 実際のディレクトリ構成は S3（#159）以降 `token/`（単数形）で確定済みのため、
//! `token::theme`（単数形）を採用する。
//!
//! ## 型 3 種
//!
//! - [`ThemeVariant`]: ライト/ダークの閉じた列挙（設計文書 §2.1）。
//! - [`Theme`]: ベース `TokenSet` に対する 1 テーマ分の差分オーバーレイ
//!   （設計文書 §2.2）。
//! - [`ThemePreference`]: ユーザー/OS 設定の意図を表す 3 値（設計文書 §2.4）。
//!
//! ## 呼び出し文脈
//!
//! [`super::resolver::TokenResolver::with_themes`] がライト/ダーク両方の
//! [`Theme`] を受け取り、[`super::resolver::TokenResolver::set_active_theme`]
//! で [`ThemeVariant`] を渡して切替を行う（`resolver.rs` モジュール doc 参照）。
//! [`super::platform_theme::platform_theme`]（S16、#172）が
//! プラットフォーム別既定テーマを [`Theme`] として構築するブリッジを提供する。

use super::schema::TokenSet;

/// アクティブテーマの区分（ライト/ダークの閉じた列挙、設計文書 §2.1）。
///
/// S2（`token-resolution-architecture.md` §4）が仮称としていた `ThemeId` を
/// 正式に置換する型であり、以降 `ThemeId` という名称は使わない（設計文書 §2.2
/// の用語 SSOT）。
///
/// `#[non_exhaustive]` は付与しない。カスタムテーマ（3 値目以降のバリアント）
/// 拡張は設計文書 §7 が明示的にスコープ外としており、内部の網羅的 match
/// （`TokenResolver::set_active_theme` の `themes` 配列インデックス変換等）を
/// 安全に保てるメリットの方が、将来の拡張性より優先される（判断根拠を実装計画
/// §3.1 に明記済み）。将来カスタムテーマを追加する場合は、この属性の要否を
/// 含めて改めて設計判断する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    /// ライトテーマ。
    Light,
    /// ダークテーマ。
    Dark,
}

impl ThemeVariant {
    /// `TokenResolver` 内部が `[TokenSet; 2]` でライト/ダーク差分を保持する際の
    /// 配列インデックス（`resolver.rs` の非公開実装詳細と対応）。
    pub(super) fn as_index(self) -> usize {
        match self {
            ThemeVariant::Light => 0,
            ThemeVariant::Dark => 1,
        }
    }
}

/// ベース `TokenSet` に対する 1 テーマ分のトークン差分（設計文書 §2.2）。
///
/// テーマそのものが独立した全トークン集合を持つわけではなく、「同一キーへの
/// 上書き値・上書き参照」の集合として表現する。差分にキーが存在しない場合は
/// ベース値を使う（§2.3 のフォールバック意味論。resolver 側の既存 `lookup`
/// 実装がベース → テーマ差分の優先順でこの意味論をそのまま満たす）。
/// ダークテーマがベースの全キーを再定義することを強制しない設計とする。
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    variant: ThemeVariant,
    overrides: TokenSet,
}

impl Theme {
    /// `variant` 区分・`overrides`（差分）から `Theme` を構築する。
    pub fn new(variant: ThemeVariant, overrides: TokenSet) -> Self {
        Self { variant, overrides }
    }

    /// このテーマが表す区分（ライト/ダーク）を返す。
    pub fn variant(&self) -> ThemeVariant {
        self.variant
    }

    /// 差分トークンセットを参照で返す。
    pub fn overrides(&self) -> &TokenSet {
        &self.overrides
    }

    /// 差分トークンセットを所有権ごと取り出す（`TokenResolver::with_themes` が
    /// 内部の `[TokenSet; 2]` へ格納する際に clone を避けるための消費 API）。
    pub(super) fn into_overrides(self) -> TokenSet {
        self.overrides
    }
}

/// ユーザー/OS のテーマ設定意図（設計文書 §2.4）。
///
/// `System` は「OS のライト/ダーク設定に追従する」という**意図**を表すのみで、
/// OS 設定の実際の値はコアが直接検出しない（REQ-2 の一方向依存: 最適化レイヤ
/// → コアの一方向。コアが OS API・環境変数を読むと `.claude/rules/security.md`
/// の A05「セキュリティ設定ミス・過剰な権限」に抵触する）。
///
/// コアは「`System` を保持し続けて解決を先延ばしにする」設計を取らない。
/// `System` の実際の解決（OS 判定 → `ThemeVariant` への変換）はプラットフォーム
/// 統合レイヤの責務であり、[`Self::resolved_with`] はその変換を表す純関数
/// （呼び出し元が渡した `system` 引数をそのまま返すだけ）として提供する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePreference {
    /// OS のライト/ダーク設定に追従する。
    System,
    /// 常にライトテーマを使う。
    Light,
    /// 常にダークテーマを使う。
    Dark,
}

impl ThemePreference {
    /// `system`（プラットフォームレイヤが判定した OS の現在のテーマ）を用いて
    /// `self` を具体的な [`ThemeVariant`] へ解決する。
    ///
    /// `System` の場合のみ `system` をそのまま返す純関数（OS 判定自体はコアの
    /// 責務ではないため、コアはこの変換を行うのみで OS API を呼び出さない）。
    pub fn resolved_with(self, system: ThemeVariant) -> ThemeVariant {
        match self {
            ThemePreference::System => system,
            ThemePreference::Light => ThemeVariant::Light,
            ThemePreference::Dark => ThemeVariant::Dark,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::color::ColorValue;
    use crate::token::schema::{TokenDefinition, TokenPath, TokenValue};

    fn path(s: &str) -> TokenPath {
        TokenPath::from_dotted(s).expect("テスト用の正当なパス")
    }

    #[test]
    fn theme_preference_resolved_with_covers_all_three_values() {
        assert_eq!(
            ThemePreference::System.resolved_with(ThemeVariant::Dark),
            ThemeVariant::Dark
        );
        assert_eq!(
            ThemePreference::System.resolved_with(ThemeVariant::Light),
            ThemeVariant::Light
        );
        assert_eq!(
            ThemePreference::Light.resolved_with(ThemeVariant::Dark),
            ThemeVariant::Light
        );
        assert_eq!(
            ThemePreference::Dark.resolved_with(ThemeVariant::Light),
            ThemeVariant::Dark
        );
    }

    #[test]
    fn theme_accessors_expose_variant_and_overrides() {
        let mut overrides = TokenSet::new();
        overrides.insert(
            path("color.primary"),
            TokenDefinition::Value {
                value: TokenValue::Color(ColorValue::rgb(0x01, 0x02, 0x03)),
            },
        );
        let theme = Theme::new(ThemeVariant::Dark, overrides.clone());

        assert_eq!(theme.variant(), ThemeVariant::Dark);
        assert_eq!(theme.overrides(), &overrides);
        assert_eq!(theme.into_overrides(), overrides);
    }

    #[test]
    fn theme_variant_as_index_maps_light_and_dark_distinctly() {
        assert_eq!(ThemeVariant::Light.as_index(), 0);
        assert_eq!(ThemeVariant::Dark.as_index(), 1);
    }
}
