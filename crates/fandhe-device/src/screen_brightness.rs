//! 画面輝度の取得・設定（Phase 11 の拡充ループ、親 #126、Issue #633、対応表
//! `docs/device/capabilities/screen-brightness.json`）。
//!
//! 扱う範囲は「アプリが前面にいる間だけ輝度を一時的に上書きする」ことだけ。現在値の読み取り、
//! 0.0〜1.0 での上書き、解除（利用者の値へ戻す）を持つ。システム設定の輝度は変えない。
//! 対象は iOS（#634）と Android（#635）。macOS・Web は `no`、Windows は `deferred` のため
//! [`UnsupportedScreenBrightness`] を返す。`fandhe-egui-proxy` の橋渡し層が UI スレッドから
//! 呼び、[`ScreenBrightness::read`] の読み戻しを画面
//! （`fandhe_core::screens::screen_brightness_show`）へ返す。
//!
//! # 契約
//!
//! - **表示は読み戻しで決める**: 画面は [`BrightnessReading::overriding`] が真のときだけ
//!   「上書き中」と出す（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **エラー種別**: 非対応 → `Unsupported`、範囲外・NaN → `InvalidInput`（OS へ渡さない）、
//!   OS 呼び出しの失敗・期限切れ → `Internal`。権限もキャンセル経路も無いので
//!   `PermissionDenied` / `Cancelled` は返さない。成功 / 失敗の二値になる（同「10」）
//! - **失敗時は直前の状態のまま**: set に失敗したら OS の値も `overriding` も変えない。
//!   clear に失敗したら `overriding` は真のまま。未上書きでの clear は冪等に `Ok`
//! - **`Ok` なのに読み戻しが要求と食い違う場合**は、driver が失敗として扱う（嘘の成功の禁止）
//! - **検証は丸めの前**: [`validate_level`] を driver と各 OS 実装の両方が OS 呼び出しの前に通す
//! - **`level` が `None`** になるのは、現在値を出さない OS の場合（Android で上書きしていない間）
//! - **ライフサイクル**: 背景に回ったら利用者の値へ戻し、前面に戻ったら要求が残っていれば掛け直す。
//!   iOS はアプリ側（`.m`）が `didEnterBackground` / `willEnterForeground` で行い、Android は
//!   OS が行う。iOS で上書き中に利用者が輝度を変えたら（[`NEAR_EQUAL_TOLERANCE`] で判定）
//!   上書きをやめて利用者の値を残し、以後の `read().overriding` は偽になる。Android の Activity
//!   再生成では要求値を持ち越して掛け直す
//! - **スレッド**: UI スレッド（egui の update ループ）から同期で呼ぶ。iOS の状態と observer は
//!   main スレッドのみで扱う。Android は UI スレッドへの post を期限付き（最長 1 秒程度）で待ち、
//!   適用開始と放棄を CAS で排他にして、期限後に適用しない（同「3」）
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは（1 プロセス内で）、解除・背景遷移で利用者の値へ戻ること、上書きしていない状態を
//! 「上書き中」と見せないこと、範囲外の値を OS へ渡さないこと。
//! 守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）は、プロセス強制終了時の
//! 復元（Android はウィンドウ破棄で OS が解き、iOS は画面ロックまで残る。OS が片付けるため
//! 同期させない）、複数ウィンドウ・分割画面・外部ディスプレイ、自動輝度による OS 側の変化と
//! 表示までの遅れ（最長 [`READ_INTERVAL`]）、端末ごとの輝度曲線の差。いずれも表示が一時的に
//! 食い違うだけで、データ消失・資源漏れ・嘘の成功には至らない。

use std::time::Duration;

use crate::DeviceError;

/// 輝度の下限（0.0 = 最暗）。
pub const LEVEL_MIN: f32 = 0.0;
/// 輝度の上限（1.0 = 最明）。
pub const LEVEL_MAX: f32 = 1.0;

/// 画面が OS を読み直す間隔（pull 型）。仮の値で、実機確認（#628）の結果で見直す。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// iOS で「利用者が輝度を変えた」と判定する差の閾値。仮の値で、実機の読み戻し誤差で確定する
/// （#634 が使う）。
pub const NEAR_EQUAL_TOLERANCE: f32 = 0.02;

/// 輝度の読み戻し。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrightnessReading {
    /// 現在の輝度（0.0〜1.0）。現在値を出さない OS では `None`。
    pub level: Option<f32>,
    /// OS 側で実際に上書きが効いているか。
    pub overriding: bool,
}

/// 範囲外・非有限値を弾く。丸めの前に呼ぶ（1.004 は丸めれば 1.0 だが `InvalidInput`）。
pub fn validate_level(level: f32) -> Result<f32, DeviceError> {
    if level.is_finite() && (LEVEL_MIN..=LEVEL_MAX).contains(&level) {
        Ok(level)
    } else {
        Err(DeviceError::InvalidInput("screen-brightness level"))
    }
}

/// 画面輝度の契約（モジュール doc の「契約」節）。
pub trait ScreenBrightness: Send + Sync {
    /// アプリ前面の間だけ輝度を `level`（0.0〜1.0）で上書きする。
    fn set_override(&self, level: f32) -> Result<(), DeviceError>;

    /// 上書きを解いて利用者の値へ戻す。未上書きなら冪等に `Ok`。
    fn clear_override(&self) -> Result<(), DeviceError>;

    /// 現在の読み戻し。
    fn read(&self) -> Result<BrightnessReading, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedScreenBrightness;

impl ScreenBrightness for UnsupportedScreenBrightness {
    fn set_override(&self, _level: f32) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen-brightness"))
    }

    fn clear_override(&self) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen-brightness"))
    }

    fn read(&self) -> Result<BrightnessReading, DeviceError> {
        Err(DeviceError::Unsupported("screen-brightness"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let u = UnsupportedScreenBrightness;
        assert!(matches!(
            u.set_override(0.5),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            u.clear_override(),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(u.read(), Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn validate_level_rejects_before_rounding() {
        for bad in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.01,
            1.01,
            1.004,
        ] {
            assert!(
                matches!(validate_level(bad), Err(DeviceError::InvalidInput(_))),
                "{bad}"
            );
        }
        for ok in [0.0, 0.5, 1.0] {
            assert_eq!(validate_level(ok).unwrap(), ok);
        }
    }
}
