//! アニメーション・duration トークンの専用値型・スケール定数（TASK-8.4-S13、#169）。
//!
//! `docs/architecture/design-token-architecture.md`（TASK-8.4-S1、#157）§4.1 の
//! 割り当て表により `animation` / `duration` の 2 カテゴリは S13（#169）に
//! 単独で割り当てられている。同文書 §4.3 の値モデルは「時間はミリ秒相当の f32
//! （またはミリ秒整数）、イージングは閉じた列挙（曲線名）」、§5.2 の素案は
//! `DurationValue { milliseconds: f32 }` / `AnimationValue { duration: DurationValue,
//! easing: EasingValue }` / `EasingValue { Linear, EaseIn, EaseOut, EaseInOut }` である。
//! `SpacingValue`（TASK-8.4-S10、#166）が spacing / size / radius 共有の素案の型
//! エイリアスを spacing 専用の検証付き newtype へ差し替えた前例に倣い、
//! `DurationValue` も素案の生構造体をそのまま使わず検証付き newtype として実装する
//! （値モデル自体＝ミリ秒相当の f32 という契約は変えていない）。
//!
//! 値は `ui/` 側のレイアウト計算とは異なる時間軸の単位系だが、`SpacingValue` と
//! 同じ「非有限値・負値を構築時点で拒否する」防御方針を踏襲する（後段のアニメーション
//! タイミング計算へ不正値を伝播させない、security.md の DoS 耐性観点）。
//! イージングは cubic-bezier パラメータ化や spring アニメーション等を含まない閉じた
//! 4 列挙（§5.2）に留め、拡張が必要になった場合はユーザー承認のうえ別 issue で扱う
//! （out-of-scope-tracking.md 対象）。プラットフォーム準拠の具体値調整は S16（#172）、
//! ウィジェットへの配線は S14/S15、外部ファイル読み込みは S17 のスコープであり、
//! 本モジュールは値型・定数・`TokenSet` ビルダーの提供のみを担う。

use super::error::TokenError;
use super::schema::{TokenDefinition, TokenPath, TokenSet, TokenValue};

/// duration トークンの値（設計文書 §4.3・§5.2 準拠、ミリ秒相当の f32）。
///
/// `SpacingValue::new` と同じ防御方針で、有限（`is_finite()`）かつ 0.0 以上の
/// 値のみを構築時点で受理する（NaN・無限大・負値は後段のアニメーションタイミング
/// 計算へ伝播させない）。`duration.instant = 0` が意味を持つ値であるため、
/// `SpacingValue` の `spacing.none = 0` と同様に 0.0 を境界値として許容する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DurationValue(f32);

impl DurationValue {
    /// 数値から duration 値を構築する。
    ///
    /// # Errors
    ///
    /// `milliseconds` が有限な 0.0 以上の値でない場合（NaN・無限大・負値）に
    /// [`TokenError::InvalidDuration`] を返す。
    pub fn new(milliseconds: f32) -> Result<Self, TokenError> {
        if !(milliseconds.is_finite() && milliseconds >= 0.0) {
            return Err(TokenError::InvalidDuration {
                reason: format!(
                    "duration 値は有限かつ 0.0 以上である必要があります（NaN・無限大・負値は不可）: {milliseconds}"
                ),
            });
        }
        Ok(Self(milliseconds))
    }

    /// 内部で保持する数値表現（ミリ秒相当）を返す。
    pub fn milliseconds(self) -> f32 {
        self.0
    }
}

/// イージング曲線（設計文書 §4.3・§5.2 が定める閉じた列挙、4 種のみ）。
///
/// 構築は infallible（域外値という概念がないため検証不要）。cubic-bezier
/// パラメータ化・spring アニメーション等への拡張はスコープ外（モジュール doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingValue {
    /// 等速。
    Linear,
    /// 開始が緩やかに加速する。
    EaseIn,
    /// 終了が緩やかに減速する。
    EaseOut,
    /// 開始・終了の両方が緩やかになる。
    EaseInOut,
}

/// アニメーション値（duration + easing の組み合わせ、設計文書 §5.2 準拠）。
///
/// 構成要素（`DurationValue` / `EasingValue`）が構築時点で検証済みのため、
/// `new` は infallible。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationValue {
    duration: DurationValue,
    easing: EasingValue,
}

impl AnimationValue {
    /// duration と easing から `AnimationValue` を構築する。
    pub const fn new(duration: DurationValue, easing: EasingValue) -> Self {
        Self { duration, easing }
    }

    /// アニメーションの duration を返す。
    pub fn duration(self) -> DurationValue {
        self.duration
    }

    /// アニメーションの easing を返す。
    pub fn easing(self) -> EasingValue {
        self.easing
    }
}

/// duration スケール定数（フレームワーク中立の既定値、設計文書 §4.2 の命名規則
/// 準拠のキーは [`duration_token_set`] 側で付与する）。
///
/// `spacing::scale` の前例に倣い、`DurationValue::new` が非 `const fn`（`Result`
/// を返す）ため定数コンテキストで呼べない代わりに、プライベートフィールドへ
/// 直接アクセスして構築する。ここで列挙する値はすべてリテラルの有限非負値であり、
/// [`DurationValue::new`] が課す不変条件（有限かつ 0.0 以上）を満たすことをコード上で
/// 確認済み（下記 `scale_constants_are_ascending_and_pairwise_distinct` テストが
/// 昇順性を担保する）。プラットフォーム準拠の具体値調整は S16（#172）のスコープ。
pub mod scale {
    use super::DurationValue;

    /// `duration.instant`（0ms、即時反映を明示するための値）。
    pub const INSTANT: DurationValue = DurationValue(0.0);
    /// `duration.fast`（100ms）。
    pub const FAST: DurationValue = DurationValue(100.0);
    /// `duration.normal`（250ms）。
    pub const NORMAL: DurationValue = DurationValue(250.0);
    /// `duration.slow`（400ms）。
    pub const SLOW: DurationValue = DurationValue(400.0);
    /// `duration.slower`（700ms）。
    pub const SLOWER: DurationValue = DurationValue(700.0);
}

/// アニメーションプリセット定数（duration + easing の既定組み合わせ）。
///
/// `AnimationValue::new` が `const fn` であるため、[`scale`] の定数を直接
/// 組み合わせて定数コンテキストで構築できる。
pub mod presets {
    use super::{AnimationValue, EasingValue, scale};

    /// `animation.standard`（NORMAL + EaseInOut、汎用トランジション）。
    pub const STANDARD: AnimationValue = AnimationValue::new(scale::NORMAL, EasingValue::EaseInOut);
    /// `animation.enter`（NORMAL + EaseOut、要素の出現）。
    pub const ENTER: AnimationValue = AnimationValue::new(scale::NORMAL, EasingValue::EaseOut);
    /// `animation.exit`（FAST + EaseIn、要素の消失）。
    pub const EXIT: AnimationValue = AnimationValue::new(scale::FAST, EasingValue::EaseIn);
    /// `animation.emphasized`（SLOW + EaseInOut、強調トランジション）。
    pub const EMPHASIZED: AnimationValue = AnimationValue::new(scale::SLOW, EasingValue::EaseInOut);
}

/// duration スケール全体を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`duration.<段階>`）に従い、検証付き
/// コンストラクタ `TokenPath::from_dotted` 経由でのみ生成する（生文字列を
/// 解決エンジンの境界へ渡さない契約、`spacing_token_set` の前例踏襲）。
/// 定義済みリテラルのみから生成するため実行時に検証が失敗することはないが、
/// `TokenPath::new` が `Result` を返す契約を守り `unwrap`/`expect` を使わずに
/// `?` で伝播する（coding-rust.md の panic 禁止方針）。
pub fn duration_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, DurationValue)] = &[
        ("duration.instant", scale::INSTANT),
        ("duration.fast", scale::FAST),
        ("duration.normal", scale::NORMAL),
        ("duration.slow", scale::SLOW),
        ("duration.slower", scale::SLOWER),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Duration(*value),
            },
        );
    }
    Ok(set)
}

/// animation スケール全体を束ねた `TokenSet` を構築する。
///
/// キーは設計文書 §4.2 の命名規則（`animation.<段階>`）に従う。
/// `duration_token_set` と同じ理由で `unwrap`/`expect` を使わずに `?` で伝播する。
pub fn animation_token_set() -> Result<TokenSet, TokenError> {
    let entries: &[(&str, AnimationValue)] = &[
        ("animation.standard", presets::STANDARD),
        ("animation.enter", presets::ENTER),
        ("animation.exit", presets::EXIT),
        ("animation.emphasized", presets::EMPHASIZED),
    ];

    let mut set = TokenSet::new();
    for (key, value) in entries {
        let path = TokenPath::from_dotted(key)?;
        set.insert(
            path,
            TokenDefinition::Value {
                value: TokenValue::Animation(*value),
            },
        );
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_new_accepts_zero_boundary_value() {
        let value = DurationValue::new(0.0).expect("0.0 は境界値として許容される");
        assert_eq!(value.milliseconds(), 0.0);
    }

    #[test]
    fn duration_new_accepts_positive_value() {
        let value = DurationValue::new(250.0).expect("正値は許容される");
        assert_eq!(value.milliseconds(), 250.0);
    }

    #[test]
    fn duration_new_rejects_negative_value() {
        let err = DurationValue::new(-1.0).expect_err("負値は拒否される");
        assert!(matches!(err, TokenError::InvalidDuration { .. }));
    }

    #[test]
    fn duration_new_rejects_nan() {
        let err = DurationValue::new(f32::NAN).expect_err("NaN は拒否される");
        assert!(matches!(err, TokenError::InvalidDuration { .. }));
    }

    #[test]
    fn duration_new_rejects_infinite_value() {
        let err = DurationValue::new(f32::INFINITY).expect_err("無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidDuration { .. }));

        let err = DurationValue::new(f32::NEG_INFINITY).expect_err("負の無限大は拒否される");
        assert!(matches!(err, TokenError::InvalidDuration { .. }));
    }

    #[test]
    fn duration_scale_constants_are_ascending_and_pairwise_distinct() {
        let scale = [
            scale::INSTANT,
            scale::FAST,
            scale::NORMAL,
            scale::SLOW,
            scale::SLOWER,
        ];
        for window in scale.windows(2) {
            assert!(
                window[0].milliseconds() < window[1].milliseconds(),
                "スケール定数は昇順である必要があります: {:?} -> {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn animation_presets_are_pairwise_distinct() {
        let presets = [
            presets::STANDARD,
            presets::ENTER,
            presets::EXIT,
            presets::EMPHASIZED,
        ];
        for i in 0..presets.len() {
            for j in (i + 1)..presets.len() {
                assert_ne!(
                    presets[i], presets[j],
                    "プリセットは相互に異なる値である必要があります: {:?} と {:?}",
                    presets[i], presets[j]
                );
            }
        }
    }

    #[test]
    fn duration_token_set_builds_without_error_and_has_expected_count() {
        let set = duration_token_set().expect("定義済みリテラルのみからの構築は必ず成功する");
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn duration_token_set_has_no_duplicate_keys() {
        let set = duration_token_set().expect("duration トークン構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn duration_token_set_all_values_are_duration_category() {
        let set = duration_token_set().expect("duration トークン構築は成功する");
        for definition in set.values() {
            assert_eq!(
                definition.category(),
                super::super::schema::TokenCategory::Duration
            );
        }
    }

    #[test]
    fn animation_token_set_builds_without_error_and_has_expected_count() {
        let set = animation_token_set().expect("定義済みリテラルのみからの構築は必ず成功する");
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn animation_token_set_has_no_duplicate_keys() {
        let set = animation_token_set().expect("animation トークン構築は成功する");
        let mut seen = std::collections::HashSet::new();
        for key in set.keys() {
            assert!(seen.insert(key.to_string()), "キーが重複しています: {key}");
        }
    }

    #[test]
    fn animation_token_set_all_values_are_animation_category() {
        let set = animation_token_set().expect("animation トークン構築は成功する");
        for definition in set.values() {
            assert_eq!(
                definition.category(),
                super::super::schema::TokenCategory::Animation
            );
        }
    }
}
