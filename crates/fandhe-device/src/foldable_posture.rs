//! 折りたたみ端末の姿勢と折れ目の契約（Issue #723、親 #141）。
//! 端末が「平ら」か「折れている」か、折れ目がどこを通るかを取得するための橋渡しで、
//! `fandhe-egui-proxy` の `device/foldable_posture.rs` が間隔つきで
//! [`FoldablePostureSource::read`] を呼ぶ（pull 型。OS の変更通知は購読しない）。
//! Web は `fandhe-web-dom` が Device Posture API と Viewport Segments API を読み、
//! 生の値を本モジュールの [`reading_from_raw`] に通して契約の値へ写す。
//!
//! # 契約
//!
//! - 姿勢は [`Posture::Flat`]（API の `continuous`）と [`Posture::Folded`]（API の `folded`）の
//!   2 値。閉じた状態とヒンジの角度は API に値が無いため扱わない
//! - 折れ目は [`FoldInfo::Known`] か [`FoldInfo::Unknown`]。取れないときに「折れ目なし」と
//!   言わない。区画 API が無い・null・1 区画・3 区画以上は `Unknown`
//! - 姿勢 API が無ければ `Unsupported`。壊れた値（未知の文字列・非有限値・値域外・区画の並びの
//!   矛盾）は `Internal`。「平ら」に倒さず、クランプもしない
//! - **三値**（契約表 10）: 「読めた」「非対応（`Unsupported`）」「失敗（`Internal`）」。
//!   利用者の操作・権限ダイアログ・保留中の要求・世代を持たないので、キャンセル・権限拒否・
//!   古い世代の完了・期限は構造上起きない（契約表 1・2・3 は該当なし）
//! - 失敗の後に前回の値を返し続けない。値は機種の推定に使えるため、ログへ出さない
//! - `change` イベントは購読せず [`READ_INTERVAL`] ごとに読み直す（非表示の文書には届かず、
//!   bfcache から戻ったときに古い値が残るため）
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 取れないときに「平ら」や折れ目なしへ倒さないこと、1 ページ内の読み取り、
//! Web 側のタイマーとリスナーの解放。
//! 守らない: 反映の遅れ（[`READ_INTERVAL`] + 1 フレーム。背景タブではさらに遅れる）、
//! 折りたためない端末と広げた端末の区別、複数タブ間の食い違い、ページの zoom が 1 でないとき
//! の位置のずれ、Safari の設定に依存する差。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 区画の座標・大きさとして受け入れる上限（CSS px）。壊れた値を成功にしないための検査用。
pub const MAX_EXTENT: f64 = 20_000.0;

/// 区画の上下・左右の揃いと、帯の負の幅を許す誤差（CSS px）。
pub const ALIGN_TOLERANCE: f64 = 1.0;

/// 端末の姿勢。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// 平ら（API の `continuous`）。
    Flat,
    /// 折れている（API の `folded`）。
    Folded,
}

/// 折れ目の向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldOrientation {
    /// 折れ目の線が縦（画面が左右に分かれる）。
    Vertical,
    /// 折れ目の線が横（画面が上下に分かれる）。
    Horizontal,
}

/// 折れ目の帯。位置と太さは軸方向の全長 [`Fold::extent`] に対する CSS px。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fold {
    /// 向き。
    pub orientation: FoldOrientation,
    /// 軸方向の始点（左端または上端）から帯の始まりまで。
    pub start: f32,
    /// 帯の太さ。0 もあり得る。
    pub thickness: f32,
    /// 軸方向の全長。
    pub extent: f32,
}

/// 折れ目の情報。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoldInfo {
    /// 折れ目が分かる。
    Known(Fold),
    /// 取れない（「折れ目なし」ではない）。
    Unknown,
}

/// 1 回の読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PostureReading {
    /// 姿勢。
    pub posture: Posture,
    /// 折れ目。
    pub fold: FoldInfo,
}

/// Web の区画（viewport segment）1 つの矩形（CSS px）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentRect {
    /// 左端。
    pub x: f64,
    /// 上端。
    pub y: f64,
    /// 幅。
    pub width: f64,
    /// 高さ。
    pub height: f64,
}

/// 姿勢と折れ目を 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait FoldablePostureSource: Send + Sync {
    /// 現在の姿勢と折れ目を返す。失敗後に直前の値を返し続けてはならない。
    fn read(&self) -> Result<PostureReading, DeviceError>;
}

/// 常に非対応を返す実装（ネイティブ全 OS の dispatch 先）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedFoldablePosture;

impl FoldablePostureSource for UnsupportedFoldablePosture {
    fn read(&self) -> Result<PostureReading, DeviceError> {
        Err(DeviceError::Unsupported("foldable_posture"))
    }
}

fn bad() -> DeviceError {
    DeviceError::Internal("foldable_posture read")
}

/// Device Posture API の `type` 文字列を姿勢へ写す。完全一致の 2 値以外
/// （空・大文字違い・前後の空白つきを含む）は `Internal` とし、「平ら」へ倒さない。
pub fn parse_posture(type_str: &str) -> Result<Posture, DeviceError> {
    match type_str {
        "continuous" => Ok(Posture::Flat),
        "folded" => Ok(Posture::Folded),
        _ => Err(bad()),
    }
}

fn valid_rect(r: &SegmentRect) -> bool {
    let pos_ok = |v: f64| v.is_finite() && (0.0..=MAX_EXTENT).contains(&v);
    let size_ok = |v: f64| v.is_finite() && v > 0.0 && v <= MAX_EXTENT;
    pos_ok(r.x) && pos_ok(r.y) && size_ok(r.width) && size_ok(r.height)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= ALIGN_TOLERANCE
}

/// 帯の太さを検査する。誤差内の負値は 0、それを超える負値（重なり）は `Internal`。
fn thickness_of(gap: f64) -> Result<f64, DeviceError> {
    if gap < -ALIGN_TOLERANCE {
        Err(bad())
    } else {
        Ok(gap.max(0.0))
    }
}

/// Viewport Segments API の区画列から折れ目を求める。
///
/// - `None`・1 区画・3 区画以上は `Ok(Unknown)`（取れないときに折れ目なしと言わない）。0 区画は `Internal`
/// - 2 区画は値域を検査（f32 へ変換する前。契約表 5）したうえで、上端と高さが揃えば左右並び、
///   左端と幅が揃えば上下並びとして求める。どちらでもなければ `Internal`
pub fn fold_from_segments(segments: Option<&[SegmentRect]>) -> Result<FoldInfo, DeviceError> {
    let Some(segs) = segments else {
        return Ok(FoldInfo::Unknown);
    };
    match segs.len() {
        0 => Err(bad()),
        2 => {
            let (p, q) = (&segs[0], &segs[1]);
            if !valid_rect(p) || !valid_rect(q) {
                return Err(bad());
            }
            let fold = if close(p.y, q.y) && close(p.height, q.height) {
                let (a, b) = if p.x <= q.x { (p, q) } else { (q, p) };
                Fold {
                    orientation: FoldOrientation::Vertical,
                    start: a.width as f32,
                    thickness: thickness_of(b.x - (a.x + a.width))? as f32,
                    extent: (b.x + b.width - a.x) as f32,
                }
            } else if close(p.x, q.x) && close(p.width, q.width) {
                let (a, b) = if p.y <= q.y { (p, q) } else { (q, p) };
                Fold {
                    orientation: FoldOrientation::Horizontal,
                    start: a.height as f32,
                    thickness: thickness_of(b.y - (a.y + a.height))? as f32,
                    extent: (b.y + b.height - a.y) as f32,
                }
            } else {
                return Err(bad());
            };
            Ok(FoldInfo::Known(fold))
        }
        _ => Ok(FoldInfo::Unknown),
    }
}

/// Web で読んだ生の値を契約の値へ写す。SSOT。姿勢 API が無いこと（`Unsupported`）の判定は
/// JS のオブジェクトを見る `fandhe-web-dom` の責務で、ここでは扱わない。
pub fn reading_from_raw(
    type_str: &str,
    segments: Option<&[SegmentRect]>,
) -> Result<PostureReading, DeviceError> {
    Ok(PostureReading {
        posture: parse_posture(type_str)?,
        fold: fold_from_segments(segments)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, width: f64, height: f64) -> SegmentRect {
        SegmentRect {
            x,
            y,
            width,
            height,
        }
    }

    fn is_internal<T: std::fmt::Debug>(v: Result<T, DeviceError>) -> bool {
        matches!(v, Err(DeviceError::Internal(_)))
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedFoldablePosture.read(),
            Err(DeviceError::Unsupported("foldable_posture"))
        ));
    }

    #[test]
    fn parse_posture_maps_two_values() {
        assert_eq!(parse_posture("continuous").unwrap(), Posture::Flat);
        assert_eq!(parse_posture("folded").unwrap(), Posture::Folded);
    }

    #[test]
    fn parse_posture_rejects_unknown() {
        for s in ["FOLDED", " folded", "", "closed", "half-opened"] {
            assert!(is_internal(parse_posture(s)), "{s:?}");
        }
    }

    #[test]
    fn missing_or_ambiguous_segments_are_unknown() {
        assert_eq!(fold_from_segments(None).unwrap(), FoldInfo::Unknown);
        let one = [r(0.0, 0.0, 100.0, 100.0)];
        assert_eq!(fold_from_segments(Some(&one)).unwrap(), FoldInfo::Unknown);
        let three = [r(0.0, 0.0, 10.0, 10.0); 3];
        assert_eq!(fold_from_segments(Some(&three)).unwrap(), FoldInfo::Unknown);
        assert!(is_internal(fold_from_segments(Some(&[]))));
    }

    #[test]
    fn side_by_side_is_vertical() {
        let s = [r(0.0, 0.0, 400.0, 800.0), r(420.0, 0.0, 380.0, 800.0)];
        let FoldInfo::Known(f) = fold_from_segments(Some(&s)).unwrap() else {
            panic!("Known を期待");
        };
        assert_eq!(f.orientation, FoldOrientation::Vertical);
        assert_eq!((f.start, f.thickness, f.extent), (400.0, 20.0, 800.0));
    }

    #[test]
    fn stacked_is_horizontal_and_order_independent() {
        let a = [r(0.0, 0.0, 600.0, 300.0), r(0.0, 310.0, 600.0, 290.0)];
        let b = [a[1], a[0]];
        let fa = fold_from_segments(Some(&a)).unwrap();
        assert_eq!(fa, fold_from_segments(Some(&b)).unwrap());
        let FoldInfo::Known(f) = fa else {
            panic!("Known を期待");
        };
        assert_eq!(f.orientation, FoldOrientation::Horizontal);
        assert_eq!((f.start, f.thickness, f.extent), (300.0, 10.0, 600.0));
    }

    #[test]
    fn zero_thickness_is_accepted() {
        let s = [r(0.0, 0.0, 400.0, 800.0), r(400.0, 0.0, 400.0, 800.0)];
        let FoldInfo::Known(f) = fold_from_segments(Some(&s)).unwrap() else {
            panic!("Known を期待");
        };
        assert_eq!(f.thickness, 0.0);
    }

    #[test]
    fn broken_segments_are_internal() {
        let ok = r(420.0, 0.0, 380.0, 800.0);
        let bads = [
            r(f64::NAN, 0.0, 400.0, 800.0),
            r(0.0, f64::INFINITY, 400.0, 800.0),
            r(0.0, 0.0, f64::NEG_INFINITY, 800.0),
            r(-1.0, 0.0, 400.0, 800.0),
            r(0.0, 0.0, -400.0, 800.0),
            r(0.0, 0.0, 0.0, 800.0),
            r(0.0, 0.0, 400.0, 0.0),
            r(0.0, 0.0, MAX_EXTENT + 1.0, 800.0),
            r(MAX_EXTENT + 1.0, 0.0, 400.0, 800.0),
        ];
        for bad in bads {
            assert!(is_internal(fold_from_segments(Some(&[bad, ok]))), "{bad:?}");
        }
    }

    #[test]
    fn overlap_and_diagonal_are_internal() {
        let overlap = [r(0.0, 0.0, 400.0, 800.0), r(300.0, 0.0, 400.0, 800.0)];
        assert!(is_internal(fold_from_segments(Some(&overlap))));
        let diagonal = [r(0.0, 0.0, 400.0, 400.0), r(420.0, 420.0, 300.0, 300.0)];
        assert!(is_internal(fold_from_segments(Some(&diagonal))));
    }

    #[test]
    fn reading_from_raw_combines_and_propagates_failure() {
        let s = [r(0.0, 0.0, 400.0, 800.0), r(420.0, 0.0, 380.0, 800.0)];
        let ok = reading_from_raw("folded", Some(&s)).unwrap();
        assert_eq!(ok.posture, Posture::Folded);
        assert!(matches!(ok.fold, FoldInfo::Known(_)));
        let flat = reading_from_raw("continuous", None).unwrap();
        assert_eq!(flat.fold, FoldInfo::Unknown);
        assert!(is_internal(reading_from_raw("bogus", None)));
        assert!(is_internal(reading_from_raw("folded", Some(&[]))));
    }
}
