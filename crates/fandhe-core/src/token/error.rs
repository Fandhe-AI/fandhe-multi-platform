//! トークン解決処理のエラー型。
//!
//! `docs/architecture/token-resolution-architecture.md` §4.2 が定める 4 分類
//! （`UndefinedReference` / `TypeMismatch` / `CircularReference` /
//! `MaxDepthExceeded`）を実装契約として引き継ぐ。同設計文書は `thiserror` の
//! 使用を示唆するが、`crates/fandhe-core/src/ui/layout/error.rs` の前例
//! （「`fandhe-core` のランタイム依存を taffy のみに限定する」方針）に倣い、
//! 本エラー型も `std::error::Error` を手動実装する（型付きエラー 4 分類という
//! 契約自体は完全に維持する）。
//!
//! `InvalidPath` は §3.2 が S17（外部定義ファイル読み込み）のパース契約として
//! 定める許可文字集合・`..` セグメント拒否を、`TokenPath` の構築時点（型の
//! 不変条件）で最小限保証するために追加した分類。設計文書 §4.2 の 4 分類には
//! 含まれないが、S17 が本格的な構文パースを実装するまでの間、型システム経由の
//! 防御的検証として存在する（S17 実装時に重複・統合を検討する対象、§7 残課題）。
//!
//! `InvalidTypography` は `super::typography::FontWeightValue` / `TypographyValue` の
//! 構築時検証（ウェイト域外・行間の非有限値等）を、他のトークン値検証と同様に
//! panic せず `Result` で報告する分類（TASK-8.4-S9、#165）。
//!
//! `InvalidSpacing` は `super::spacing::SpacingValue::new` の構築時検証（非有限値・
//! 負値）を独立分類として報告する分類（TASK-8.4-S10、#166）。スペーシング値域検証と
//! タイポグラフィ値域検証は異なる契約であり、S17/S18 のエラー分類を混同させない
//! ために分ける。
//!
//! `InvalidDimension` は `super::dimension::DimensionValue`（size / radius
//! カテゴリ共用）の構築時検証（非有限値・負値）を報告する分類（TASK-8.4-S11、
//! #167）。`ElevationValue` は `u8` の階層値であり型として非負が保証されるため
//! 本バリアントを用いた検証エラーを持たない（infallible なコンストラクタ）。
//!
//! `InvalidBorder` / `InvalidShadow` は `super::border::BorderValue` /
//! `super::shadow::ShadowValue` の構築時検証（幅・ぼかし半径の非有限値・負値等）を
//! panic せず `Result` で報告する分類（TASK-8.4-S12、#168）。
//!
//! `InvalidDuration` は `super::animation::DurationValue::new` の構築時検証
//! （非有限値・負値）を独立分類として報告する分類（TASK-8.4-S13、#169）。
//! duration 値域検証とスペーシング値域検証は異なる契約であり、S17/S18 の
//! エラー分類を混同させないために分ける。
//!
//! `InvalidReferenceSyntax` / `ParseError` / `FileRead` は外部定義ファイル読み込みの
//! 境界（`token::file`、feature `token-file`）が使う分類（TASK-8.4-S17、#173）。
//! `InvalidPath` は `TokenPath` 型の不変条件（文字集合・`..` 拒否）を表す既存分類
//! であり、`token::file` はこれを内部的に呼び出すのみで独自の重複検証は行わない。
//! `InvalidReferenceSyntax` は `TokenPath` の検証範囲外にある参照文字列そのものの
//! 構文（`{...}` 波括弧・長さ上限）を扱う、`token::file` 固有のパース境界エラー
//! として分離する。`ParseError` は JSON 形状・`$type`/`$value` の値形状・上限
//! （トークン件数・ネスト深さ）等、構文レベルの検証全般を集約する（意味レベルの
//! 検証・複数エラーの集約報告は S18、#174 のスコープであり、本分類は fail-fast の
//! 単一エラーのみを返す、§5.3 のデータフロー契約）。`FileRead` は `std::io::Error`
//! を要因として保持したいが、`TokenError` は `Clone`/`PartialEq` を導出するため
//! （`io::Error` はいずれも実装しない）、理由文字列へ変換して保持する。
//!
//! `ThemeOverrideCategoryMismatch` は `docs/architecture/token-resolution-architecture.md`
//! §4.2・`docs/architecture/design-token-architecture.md` §6 の「型不一致の検出と
//! エラー報告形式」契約が使う分類（TASK-8.4-S18、#174）。既存の `TypeMismatch` は
//! エイリアスチェーンを辿った際の参照先カテゴリ不一致を検出するが、テーマ差分が
//! ベースと同一キーへ異なるカテゴリの定義を直接宣言するケース（チェーンを
//! 辿らない、キー単位の宣言不整合）は検出できない。`super::validation` の
//! `validate_token_sets` がこの分類を専有し、`TypeMismatch` の意味論
//! （チェーン走査中の検出）を変更しない。

use core::fmt;

/// トークン解決時に発生し得るエラー（TASK-8.4-S3、#159）。
///
/// coding-rust.md の「panic を API 契約にしない」方針に従い、`TokenResolver`
/// は本エラーを `Result` で返す。呼び出し元（S19 の DSL 参照 API 等）は
/// フォールバック値を自動補完しない（設計文書 §4.1）。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenError {
    /// 参照先のトークンキーがトークンセット（ベース・テーマ差分いずれにも）
    /// 存在しない。
    UndefinedReference {
        /// 参照が解決できなかったトークンパスの文字列表現。
        reference: String,
    },
    /// 参照元と参照先でトークンのカテゴリ（色・寸法等）が一致しない。
    TypeMismatch {
        /// エイリアスチェーンの起点となったトークンパスの文字列表現。
        /// `expected` はこのパスが宣言するカテゴリ。
        origin: String,
        /// 型不一致が検出されたターゲット（チェーン上の参照先）のトークンパスの
        /// 文字列表現。`actual` はこのパスが実際に持つカテゴリ。
        reference: String,
        /// チェーンの起点（`origin`）が期待していたカテゴリ名。
        expected: &'static str,
        /// `reference` が実際に持っていたカテゴリ名。
        actual: &'static str,
    },
    /// エイリアスチェーンが循環している（自己参照・間接循環のいずれも含む）。
    CircularReference {
        /// 循環を検出するまでに辿ったトークンパスの連鎖（" -> " 区切り）。
        chain: String,
    },
    /// エイリアスチェーンの深さが `resolver::MAX_ALIAS_DEPTH` を超えた
    /// （DoS 対策、設計文書 §5.2）。
    MaxDepthExceeded {
        /// 上限超過の起点となったトークンパスの文字列表現。
        reference: String,
        /// 適用された最大深さの上限値。
        limit: u32,
    },
    /// `TokenPath` の構築時検証（許可文字集合・`..` セグメント拒否）に失敗した。
    InvalidPath {
        /// 検証に失敗した理由（不正な文字・空セグメント等）。
        reason: String,
    },
    /// `ColorValue::from_hex`（TASK-8.4-S6、#162）による 16 進色文字列の解釈に
    /// 失敗した。`InvalidPath` とは別分類として扱う（トークンパスの構文検証と
    /// 色値のフォーマット検証は異なる契約であり、S17/S18 のエラー分類を
    /// 混同させないため）。
    InvalidColor {
        /// 検証に失敗した理由（先頭 `#` 欠落・長さ不正・非 16 進文字等）。
        reason: String,
    },
    /// `TypographyValue::new`（TASK-8.4-S8、#164）/
    /// `typography::FontWeightValue::new`（TASK-8.4-S9、#165）によるタイポグラフィ値の
    /// 構築時検証に失敗した。`InvalidColor` とは別分類として扱う（フォント
    /// ファミリ・サイズ・行間・ウェイトの不変条件は色値の 16 進フォーマット検証と
    /// 異なる契約であり、S8/S9 で同じ分類を再利用する）。
    InvalidTypography {
        /// 検証に失敗した理由（空 family・制御文字混入・過大長・非有限 size・
        /// 0 以下の size・域外のウェイト・非有限の行間等）。
        reason: String,
    },
    /// `spacing::SpacingValue::new`（TASK-8.4-S10、#166）による検証に失敗した。
    /// 非有限値（NaN・無限大）・負値を拒否する。`InvalidTypography` とは別分類
    /// として扱う（スペーシング値域検証とタイポグラフィ値域検証は異なる契約で
    /// あり、S17/S18 のエラー分類を混同させないため）。
    InvalidSpacing {
        /// 検証に失敗した理由（非有限値・負値等）。
        reason: String,
    },
    /// `super::dimension::DimensionValue::new`（TASK-8.4-S11、#167）による
    /// size / radius トークン値の構築時検証に失敗した。
    InvalidDimension {
        /// 検証に失敗した理由（非有限値・負値等）。
        reason: String,
    },
    /// `BorderValue::new`（TASK-8.4-S12、#168）によるボーダー値の構築時検証に
    /// 失敗した。
    InvalidBorder {
        /// 検証に失敗した理由（非有限の幅・負の幅等）。
        reason: String,
    },
    /// `ShadowValue::new`（TASK-8.4-S12、#168）によるシャドウ値の構築時検証に
    /// 失敗した。
    InvalidShadow {
        /// 検証に失敗した理由（非有限のオフセット・非有限または負のぼかし半径等）。
        reason: String,
    },
    /// `animation::DurationValue::new`（TASK-8.4-S13、#169）による検証に失敗した。
    /// 非有限値（NaN・無限大）・負値を拒否する。`InvalidSpacing` とは別分類として
    /// 扱う（duration 値域検証とスペーシング値域検証は異なる契約であり、
    /// S17/S18 のエラー分類を混同させないため）。
    InvalidDuration {
        /// 検証に失敗した理由（非有限値・負値等）。
        reason: String,
    },
    /// 外部定義ファイルの参照文字列（`"{group.token}"` 形式）の構文検証に失敗した
    /// （TASK-8.4-S17、#173、feature `token-file`）。
    InvalidReferenceSyntax {
        /// 検証に失敗した理由（波括弧欠落・空パス・長さ超過等）。
        reason: String,
    },
    /// 外部定義ファイルの JSON 形状・値の解釈に失敗した（TASK-8.4-S17、#173、
    /// feature `token-file`）。意味レベルの検証（未定義参照・循環等）は含まず、
    /// 構文レベルの検証のみを表す（§5.3 のデータフロー契約: S17 → S18 → 解決エンジン）。
    ParseError {
        /// 問題が検出されたトークンパスの文字列表現（グループ直下等パス確定前は空文字列）。
        path: String,
        /// 検証に失敗した理由。
        reason: String,
    },
    /// 外部定義ファイルの読み込み（存在しないパス・サイズ上限超過・I/O エラー）に
    /// 失敗した（TASK-8.4-S17、#173、feature `token-file`）。
    FileRead {
        /// 失敗理由（`std::io::Error` の `Display` 出力、またはサイズ上限超過等の
        /// 独自メッセージ）。
        reason: String,
    },
    /// テーマ差分がベースと同一キーに対して異なるカテゴリの定義を宣言している
    /// （TASK-8.4-S18、#174）。`TypeMismatch` とは別分類として扱う（チェーンを
    /// 辿らない、キー単位の宣言不整合を検出するため）。
    ThemeOverrideCategoryMismatch {
        /// カテゴリが食い違っているトークンパスの文字列表現。
        path: String,
        /// ベース側が宣言するカテゴリ名。
        base: &'static str,
        /// テーマ差分側が宣言するカテゴリ名。
        overriding: &'static str,
    },
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedReference { reference } => {
                write!(f, "未定義のトークン参照です: {reference}")
            }
            Self::TypeMismatch {
                origin,
                reference,
                expected,
                actual,
            } => write!(
                f,
                "トークンの型が一致しません: チェーンの起点 {origin} は {expected} を期待していましたが、参照先 {reference} は {actual} でした"
            ),
            Self::CircularReference { chain } => {
                write!(f, "循環参照を検出しました: {chain}")
            }
            Self::MaxDepthExceeded { reference, limit } => write!(
                f,
                "エイリアスチェーンの深さが上限（{limit}）を超えました: {reference}"
            ),
            Self::InvalidPath { reason } => {
                write!(f, "トークンパスが不正です: {reason}")
            }
            Self::InvalidColor { reason } => {
                write!(f, "色トークンの値が不正です: {reason}")
            }
            Self::InvalidTypography { reason } => {
                write!(f, "タイポグラフィトークンの値が不正です: {reason}")
            }
            Self::InvalidSpacing { reason } => {
                write!(f, "スペーシングトークンの値が不正です: {reason}")
            }
            Self::InvalidDimension { reason } => {
                write!(f, "寸法トークンの値が不正です: {reason}")
            }
            Self::InvalidBorder { reason } => {
                write!(f, "ボーダートークンの値が不正です: {reason}")
            }
            Self::InvalidShadow { reason } => {
                write!(f, "シャドウトークンの値が不正です: {reason}")
            }
            Self::InvalidDuration { reason } => {
                write!(f, "duration トークンの値が不正です: {reason}")
            }
            Self::InvalidReferenceSyntax { reason } => {
                write!(f, "トークン参照の構文が不正です: {reason}")
            }
            Self::ParseError { path, reason } => {
                if path.is_empty() {
                    write!(f, "トークン定義ファイルの解析に失敗しました: {reason}")
                } else {
                    write!(
                        f,
                        "トークン定義ファイルの解析に失敗しました（{path}）: {reason}"
                    )
                }
            }
            Self::FileRead { reason } => {
                write!(f, "トークン定義ファイルの読み込みに失敗しました: {reason}")
            }
            Self::ThemeOverrideCategoryMismatch {
                path,
                base,
                overriding,
            } => write!(
                f,
                "テーマ差分のカテゴリがベースと一致しません: {path} はベースで {base}、テーマ差分で {overriding} と宣言されています"
            ),
        }
    }
}

impl std::error::Error for TokenError {}

#[cfg(test)]
mod tests {
    //! TASK-8.4-S21（#177）: `Display` 実装が各バリアントの診断情報（参照パス・
    //! limit 等）を文言に含むことのスモークテスト。文言全文の固定はせず
    //! （日本語メッセージの言い回し変更は本テストの対象外）、呼び出し元
    //! （`resolver.rs` 等）がエラーメッセージから診断に必要な情報を読み取れる
    //! ことのみを保証する。

    use super::TokenError;

    #[test]
    fn display_includes_diagnostic_fields() {
        let err = TokenError::UndefinedReference {
            reference: "color.missing".to_string(),
        };
        assert!(err.to_string().contains("color.missing"));

        let err = TokenError::TypeMismatch {
            origin: "color.primary".to_string(),
            reference: "spacing.md".to_string(),
            expected: "Color",
            actual: "Dimension",
        };
        let message = err.to_string();
        assert!(message.contains("color.primary"));
        assert!(message.contains("spacing.md"));
        assert!(message.contains("Color"));
        assert!(message.contains("Dimension"));

        let err = TokenError::CircularReference {
            chain: "a -> b -> a".to_string(),
        };
        assert!(err.to_string().contains("a -> b -> a"));

        let err = TokenError::MaxDepthExceeded {
            reference: "t0".to_string(),
            limit: 32,
        };
        let message = err.to_string();
        assert!(message.contains("t0"));
        assert!(message.contains("32"));

        let err = TokenError::InvalidPath {
            reason: "不正な理由".to_string(),
        };
        assert!(err.to_string().contains("不正な理由"));
    }
}
