//! AR 表示の契約（Issue #307、親 #90）。ARKit / WebXR の追跡状態・開始地点からの位置・水平面の数を
//! 画面に出すための橋渡しで、`fandhe-egui-proxy` の `device/ar.rs` が開始・停止と間隔つきの
//! [`ArTracking::read`] を行う（pull 型。OS 側が最新値を持ち、画面側が読む）。カメラ映像は `ar-camera-view` の範囲で、同じ画面・
//! 同じセッションに出す（下の「カメラ映像」）。
//!
//! # 契約
//!
//! - 結果は [`ArSample`] か [`ArFailure`]。`DeviceError` へ variant を足すと既存の網羅 match が
//!   壊れるため機能固有の enum にする（`compass::CompassFailure` と同じ判断）
//! - [`ArSample::Waiting`] は「まだ値が無い / 未開始 / 停止後 / 無効なサンプルを捨てた後」。
//!   原点（0,0,0）や直前の値で代用しない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - [`ArSample::AwaitingPermission`] は OS の権限ダイアログ待ち。driver は期限を進めない
//! - 座標系は開始地点を原点とする m。y 上向き・右手系・-z が前方（ARKit / WebXR 共通）
//! - 水平面の数は数えられない環境（Web）では `None`。0 で代用しない
//! - 中断（iOS の背景遷移・`sessionWasInterrupted`）の後は原点を取り直す
//! - **三値**: 利用者の「停止」と Web の `end`（戻る操作）は失敗でなく待機へ戻す。権限拒否・非対応・
//!   期限切れ（`compass::NO_READING_TIMEOUT_MS` の間値が届かない）・その他の失敗はそれぞれ別表示
//! - 位置・NSError・OS 固有値をログへ出さない（security.md A09）
//!
//! # カメラ映像（`ar-camera-view`、iOS）
//!
//! - 映像は [`ArTracking::read`] が値ありを返した同じ読み取りでだけ取る。新しい経路・タイマー・
//!   OS コールバックのリスナーは足さない
//! - 取れない形式・未着（二面の YCbCr でない・フレームが無い・寸法や stride の検査不合格）は
//!   「映像なし」として扱い、失敗にも成功にもしない。形式違いや欠けを画像に見せかけない
//! - 停止・失敗・背景・中断では映像を捨て、直前の映像を残さない。古い世代の映像も捨てる
//! - Web は `immersive-ar` のブラウザが映像を合成するため、この契約の映像は出さない
//! - 守らない範囲: 映像の向き（端末の向きに合わせていない元画像のまま）、表示のなめらかさと遅れ、
//!   映像と数値が同じフレーム由来であること、色の厳密さ・解像度の指定、3D 物体・水平面の描画
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の値（`NotAvailable` の位置・Web の 0 枚）を出さない、二重に開始しない、停止・失敗の
//! 全経路でセッションを止める、古い世代の完了を捨てる。
//! 守らない: 姿勢（回転）・画面回転の補正・hit-test・アンカー、OS 間の値の一致、複数タブ・複数
//! プロセス、OS が非同期に確定する中断と表示の時間差（最大 [`READ_INTERVAL`] + フレーム周期）、
//! Web の `NotSupportedError` が利用者拒否か非対応かの区別の限界。
//!
//! 契約表 10（三値）・11（並行経路）への対応: 経路は画面の driver 1 本に集約され、開始・停止は
//! `wanted` の立ち上がり・立ち下がりでだけ行う。
//!
//! 期限の追跡は `compass::WaitTracker` をそのまま使う。
//! ponytail: 3 機能目で共通モジュールへ移す（現状 2 機能目なので複製せず参照のみ）。

use std::time::Duration;

use crate::camera_preview_stream::ScaledFrame;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_millis(100);

/// 位置の各成分の絶対値の上限（m）。これを超える値は無効サンプルとして捨てる。
pub const MAX_ABS_POSITION_M: f64 = 1000.0;

/// 追跡が制限付きになった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitedReason {
    /// 初期化中。
    Initializing,
    /// 動きが速すぎる。
    ExcessiveMotion,
    /// 特徴点が足りない。
    InsufficientFeatures,
    /// 位置を合わせ直している。
    Relocalizing,
    /// 上記以外・不明。
    Other,
}

/// 追跡状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    /// 追跡できない。
    NotAvailable,
    /// 制限付き。
    Limited(LimitedReason),
    /// 良好。
    Normal,
}

/// ある時点の AR 状態。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArReading {
    /// 追跡状態。
    pub tracking: Tracking,
    /// 開始地点を原点とする位置（m）。`NotAvailable` では常に `None`。
    pub position: Option<[f64; 3]>,
    /// 検出した水平面の数。数えられない環境では `None`（0 ではない）。
    pub horizontal_planes: Option<u32>,
}

/// 1 回の読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArSample {
    /// 値なし（未開始・停止後・無効サンプルを捨てた後・まだ届かない）。
    Waiting,
    /// OS の権限ダイアログの応答待ち。期限に数えない。
    AwaitingPermission,
    /// 値あり。
    Reading(ArReading),
}

/// AR 取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArFailure {
    /// 端末・API・OS 版が AR に対応しない。
    Unsupported,
    /// 利用者または OS 設定がカメラ（AR）の利用を拒否した。
    PermissionDenied,
    /// 上記以外の OS 呼び出し失敗。
    Failed,
}

/// AR 追跡の契約（モジュール doc の「契約」節）。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait ArTracking: Send + Sync {
    /// セッションを始める。冪等。同期で分かる失敗（非対応）はここで返す。
    fn start(&self) -> Result<(), ArFailure>;
    /// 最新の状態を返す。未開始・停止後・値なしは [`ArSample::Waiting`]。直前の値を返し続けない。
    fn read(&self) -> Result<ArSample, ArFailure>;
    /// セッションを止め、保持している最新値を捨てる。冪等で失敗しない。
    fn stop(&self);
    /// 直前の [`ArTracking::read`] が `Ok(ArSample::Reading(_))` を返した同じ読み取りでだけ呼ばれ、
    /// 同じセッションの最新のカメラ映像を縮小して返す（長辺は
    /// [`crate::camera_preview_stream::PREVIEW_MAX_SIDE`] 以下、RGBA8）。形式違い・フレームなし・
    /// 検査不合格は `None`（失敗にも成功にもしない）。OS のフレーム・バッファを呼び出しの外で保持しない。
    /// 既定は `None`（映像を出さない OS・未実装 OS）。契約の全体はモジュール doc の「カメラ映像」節。
    fn read_frame(&self) -> Option<ScaledFrame> {
        None
    }
}

/// 対象外 OS・未実装 OS の dispatch 先。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedAr;

impl ArTracking for UnsupportedAr {
    fn start(&self) -> Result<(), ArFailure> {
        Err(ArFailure::Unsupported)
    }
    fn read(&self) -> Result<ArSample, ArFailure> {
        Err(ArFailure::Unsupported)
    }
    fn stop(&self) {}
}

/// OS の生の値を検査して [`ArReading`] にする。`NotAvailable` の位置は常に捨てる。それ以外は
/// 3 成分すべてが有限かつ `abs <= MAX_ABS_POSITION_M` のときだけ受け、1 つでも外れたら全体を
/// `None`（無効サンプル = 値なし）にする。丸めの前に検証する。
pub fn checked_reading(
    tracking: Tracking,
    raw: [f64; 3],
    planes: Option<u32>,
) -> Option<ArReading> {
    let position = if tracking == Tracking::NotAvailable {
        None
    } else if raw
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_ABS_POSITION_M)
    {
        Some(raw)
    } else {
        return None;
    };
    Some(ArReading {
        tracking,
        position,
        horizontal_planes: planes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(UnsupportedAr.start(), Err(ArFailure::Unsupported));
        assert_eq!(UnsupportedAr.read(), Err(ArFailure::Unsupported));
        UnsupportedAr.stop();
        assert!(UnsupportedAr.read_frame().is_none());
    }

    #[test]
    fn checked_rejects_invalid_position() {
        for bad in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1000.01,
            -1000.01,
        ] {
            assert_eq!(
                checked_reading(Tracking::Normal, [0.0, bad, 0.0], Some(1)),
                None
            );
        }
        let ok = checked_reading(Tracking::Normal, [1000.0, -1000.0, 0.0], None).unwrap();
        assert_eq!(ok.position, Some([1000.0, -1000.0, 0.0]));
        assert_eq!(ok.horizontal_planes, None);
    }

    #[test]
    fn not_available_has_no_position() {
        let r = checked_reading(Tracking::NotAvailable, [0.0; 3], Some(2)).unwrap();
        assert_eq!(r.position, None);
        assert_eq!(r.horizontal_planes, Some(2));
        // 生の値が不正でも NotAvailable は位置を使わないので受ける。
        assert!(checked_reading(Tracking::NotAvailable, [f64::NAN; 3], None).is_some());
    }
}
