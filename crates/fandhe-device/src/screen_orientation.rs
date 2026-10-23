//! 画面向きの取得・固定（Phase 11 の拡充ループ、親 #128、Issue #655、対応表
//! `docs/device/capabilities/screen-orientation.json`）。
//!
//! 扱う範囲は 3 点。現在の向き（縦・横）と角度の読み取り、向きの変化への追従、縦固定・横固定・
//! 解除。上下・左右の向きの選択は OS に任せる。対象は iOS（#656）・Android（#657）・Web（#658）。
//! macOS は `n-a`、Windows は `deferred` のため [`UnsupportedScreenOrientation`] を返す。
//! `fandhe-egui-proxy` の橋渡し層が UI スレッドから呼び、読み戻しを画面
//! （`fandhe_core::screens::screen_orientation_show`）へ返す。
//!
//! # 契約
//!
//! - **依頼と完了を分ける**: [`ScreenOrientation::request_lock`] は依頼を出すだけで完了を待たない。
//!   表示は [`ScreenOrientation::read`] の読み戻しで決める（`device-bridge-contracts.md`「6」）
//! - **エラー種別は 2 種だけ**: 非対応 → `Unsupported`（macOS・Windows・iPad の固定・Safari の
//!   lock）、OS 呼び出しの失敗・期限切れ → `Internal`。権限もキャンセル経路も無いので
//!   `PermissionDenied` / `Cancelled` / `InvalidInput` は返さない。結果は成功 / 失敗の二値（同「10」）
//! - **成功の確定条件**: 固定は「実際の向きが固定した種類に入り、OS 側の希望が固定した値になった」
//!   とき。解除は「OS 側の希望が `None` に戻った」とき
//! - **失敗時の扱い**: `request_lock` が `Err` を返したときは、各 OS 実装の責務として OS 側の希望を
//!   変えない。期限（[`CONFIRM_TIMEOUT`]）切れのときは driver が `request_lock(直前の値)` で戻す
//! - **同じ固定の再依頼**は driver が OS を呼ばずに成功にする
//! - **検証**: FFI・JS から来る整数コードは [`parse_reading`] を通す。未知の値は `Internal` で、
//!   成功値にしない
//! - **スレッド**: UI スレッドから同期で呼び、ワーカーは作らない。Android は UI スレッドへの post を
//!   期限付きで待ち、適用の開始と放棄を CAS で排他にして期限後に適用しない（同「3」）。iOS は main
//!   スレッドで扱い、OS コールバック後に `crate::ios::request_repaint` を呼ぶ（同「4」）
//!
//! # OS ごとの目安
//!
//! - iOS: VC の差し替え、`setNeedsUpdateOfSupportedInterfaceOrientations`、`requestGeometryUpdate`、
//!   `effectiveGeometry.interfaceOrientation`。iPad の固定は `Unsupported`（取得のみ）
//! - Android: `setRequestedOrientation`（`SENSOR_PORTRAIT`・`SENSOR_LANDSCAPE`・`UNSPECIFIED`）、
//!   `Display.getRotation` と `Configuration.orientation`（API 30 で分岐）。manifest は変えない
//! - Web: `screen.orientation`。lock の前に全画面へ入り、解除ではこの画面が入れた全画面だけを抜ける。
//!   Safari・デスクトップ Chrome は実行時に `Unsupported`（取得は動く）
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは（1 プロセス内で）、表示が実際の向きと固定に追従すること、固定できていない状態を
//! 固定中と見せないこと、失敗後に OS 側の希望を残さないこと。
//! 守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）は、回転アニメーション中の
//! 表示の遅れ、次の読み取りまでの表示の遅れ（最大 [`READ_INTERVAL`] + 1 フレーム）、Activity
//! 再生成での固定の引き継ぎ、背景遷移中の状態、分割画面・マルチウィンドウ・PiP・外部ディスプレイで
//! OS が固定を無視すること、端末の画面回転ロックとの関係、プロセス終了時の後片付け。いずれも表示が
//! 一時的に食い違うだけで、データ消失・資源漏れ・嘘の成功には至らない。

use std::time::Duration;

use crate::DeviceError;

/// 画面が OS を読み直す間隔（pull 型）。仮の値で、実機確認（#649）の結果で見直す。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 固定・解除の確認を待つ期限。起点は依頼の開始で、単調時計（`Instant`）で測る。
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// 画面の向きの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// 縦。
    Portrait,
    /// 横。
    Landscape,
}

/// 自然な向きからの回転角。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// 0 度。
    Deg0,
    /// 90 度。
    Deg90,
    /// 180 度。
    Deg180,
    /// 270 度。
    Deg270,
}

impl Rotation {
    /// 度数から変換する。0・90・180・270 以外は `None`。
    pub fn from_degrees(d: i32) -> Option<Self> {
        match d {
            0 => Some(Self::Deg0),
            90 => Some(Self::Deg90),
            180 => Some(Self::Deg180),
            270 => Some(Self::Deg270),
            _ => None,
        }
    }

    /// 度数。
    pub fn degrees(self) -> u16 {
        match self {
            Self::Deg0 => 0,
            Self::Deg90 => 90,
            Self::Deg180 => 180,
            Self::Deg270 => 270,
        }
    }
}

/// 向きの読み戻し。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrientationReading {
    /// 実際の向き。
    pub orientation: Orientation,
    /// 自然な向きからの角度。
    pub rotation: Rotation,
    /// OS 側の固定の希望。`None` は解除。
    pub lock: Option<Orientation>,
}

/// FFI・JS から来る整数コードを検証して読み戻しにする。未知の値は `Internal` で、成功値にしない。
///
/// コード表: `orientation` は 1 = 縦・2 = 横、`degrees` は 0・90・180・270、`lock` は 0 = なし・
/// 1 = 縦・2 = 横。iOS・Android・Web（`fandhe-web-dom` も本クレートに依存）が共通に使う。
pub fn parse_reading(
    orientation: i32,
    degrees: i32,
    lock: i32,
) -> Result<OrientationReading, DeviceError> {
    let orientation = match orientation {
        1 => Orientation::Portrait,
        2 => Orientation::Landscape,
        _ => return Err(DeviceError::Internal("screen-orientation orientation")),
    };
    let rotation = Rotation::from_degrees(degrees)
        .ok_or(DeviceError::Internal("screen-orientation rotation"))?;
    let lock = match lock {
        0 => None,
        1 => Some(Orientation::Portrait),
        2 => Some(Orientation::Landscape),
        _ => return Err(DeviceError::Internal("screen-orientation lock")),
    };
    Ok(OrientationReading {
        orientation,
        rotation,
        lock,
    })
}

/// 画面向きの契約（モジュール doc の「契約」節）。
pub trait ScreenOrientation: Send + Sync {
    /// 現在の読み戻し。
    fn read(&self) -> Result<OrientationReading, DeviceError>;

    /// 固定（`Some`）・解除（`None`）を依頼するだけで完了は待たない。`Err` なら OS 側の希望は変えない。
    fn request_lock(&self, target: Option<Orientation>) -> Result<(), DeviceError>;

    /// 固定できる端末か（iPad・Safari・非対応 OS は偽）。取得の可否とは別。
    fn can_lock(&self) -> bool;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedScreenOrientation;

impl ScreenOrientation for UnsupportedScreenOrientation {
    fn read(&self) -> Result<OrientationReading, DeviceError> {
        Err(DeviceError::Unsupported("screen-orientation"))
    }

    fn request_lock(&self, _target: Option<Orientation>) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("screen-orientation"))
    }

    fn can_lock(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        let u = UnsupportedScreenOrientation;
        assert!(matches!(u.read(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            u.request_lock(Some(Orientation::Portrait)),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            u.request_lock(None),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(!u.can_lock());
    }

    #[test]
    fn parse_reading_rejects_unknown_codes() {
        for (o, d, l) in [
            (1, 45, 0),
            (1, -90, 0),
            (1, 360, 0),
            (0, 0, 0),
            (3, 0, 0),
            (1, 0, 3),
            (1, 0, -1),
        ] {
            assert!(
                matches!(parse_reading(o, d, l), Err(DeviceError::Internal(_))),
                "{o} {d} {l}"
            );
        }
    }

    #[test]
    fn parse_reading_accepts_valid() {
        for o in [1, 2] {
            for d in [0, 90, 180, 270] {
                for l in [0, 1, 2] {
                    let r = parse_reading(o, d, l).unwrap();
                    assert_eq!(r.rotation.degrees() as i32, d);
                    assert_eq!(r.orientation == Orientation::Portrait, o == 1);
                    assert_eq!(r.lock.is_some(), l != 0);
                }
            }
        }
    }

    #[test]
    fn rotation_round_trip() {
        for d in [0, 90, 180, 270] {
            assert_eq!(Rotation::from_degrees(d).unwrap().degrees() as i32, d);
        }
        assert!(Rotation::from_degrees(1).is_none());
    }
}
