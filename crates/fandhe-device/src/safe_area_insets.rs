//! セーフエリア（ディスプレイ切り欠き・システムバー）の契約（Issue #444、親 #105）。
//! 画面の 4 辺について「OS が内容を置くなと言っている余白」を論理単位で取得するための
//! 橋渡しで、`fandhe-egui-proxy` の `device/safe_area_insets.rs` が間隔つきで
//! [`SafeAreaSource::read`] を呼ぶ（pull 型。OS の変更通知は購読しない）。
//! Web は `fandhe-web-dom` が viewport の `env(safe-area-inset-*)` を読む。
//!
//! # 契約
//!
//! - 結果は `Ok(Some(Insets))`（読めた）・`Ok(None)`（OS からまだ値が届いていない。
//!   Android の受信前。画面は「読み取り中」に戻す）・`DeviceError`（`Unsupported` / `Internal`
//!   のみ）。取れないときの 0 を成功として返さない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **三値**（契約表 10）: 「読めた」「非対応（`Unsupported`）」「失敗（`Internal`）」に、
//!   未取得（`Ok(None)`）を加える。利用者のキャンセル・権限ダイアログ・入力は無いので、
//!   キャンセルと権限拒否は該当なし
//! - 購読・保留中の要求・世代を持たないので、古い世代の完了・期限・キャンセルの問題は
//!   構造上起きない（契約表 1・2・3 は該当なし）。失敗の後に前回の値を返し続けない
//! - 値は機種の推定に使えるため、ログへ出さない
//! - 「回転への追従」は最大 [`READ_INTERVAL`] + 1 フレームで反映される、と読み替える
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 嘘の値（取れないときの 0）を出さないこと、1 プロセス内の取得。
//! 守らない: 反映の遅れ（最大 1 秒 + 1 フレーム。Web の背景タブはさらに遅れる）、OS ごとの
//! 意味の違い（iOS は key window、Android は `systemBars` と `displayCutout` の合計、macOS は
//! 窓の `contentView` で OS が全画面の中身をセーフエリアの内側に置くため普段は 0、Web は
//! viewport の `env()`）、egui の zoom が 1 でないときの point とのずれ、Android の Activity
//! 再生成の直後に古い値が残り 4 辺の組が 1 回混ざり得ること、複数窓では key window の値だけを
//! 見ること。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 1 辺の余白として受け入れる上限（論理単位）。実在するセーフエリアより十分大きく、
/// 壊れた値（巨大・負・非有限）を成功にしないための検査用。
pub const MAX_INSET: f64 = 2000.0;

/// 4 辺の余白（論理単位: pt / dp / CSS px）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Insets {
    /// 上。
    pub top: f32,
    /// 右。
    pub right: f32,
    /// 下。
    pub bottom: f32,
    /// 左。
    pub left: f32,
}

/// セーフエリアを 1 回読む。UI スレッドで同期に呼ばれるため短時間で戻ること。
pub trait SafeAreaSource: Send + Sync {
    /// 現在の余白を返す。`Ok(None)` は OS からまだ値が届いていないことを表す。
    /// 失敗後に直前の値を返し続けてはならない。
    fn read(&self) -> Result<Option<Insets>, DeviceError>;
}

/// 常に非対応を返す実装（未実装 OS の dispatch 先・スタブ）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedSafeArea;

impl SafeAreaSource for UnsupportedSafeArea {
    fn read(&self) -> Result<Option<Insets>, DeviceError> {
        Err(DeviceError::Unsupported("safe_area_insets"))
    }
}

/// ネイティブ層（macOS・iOS の C ABI）が返す状態コードと 4 値を契約の値へ写す。SSOT。
/// `raw` の並びは top / right / bottom / left。
///
/// - `0`: 4 値すべてが有限で `0.0..=MAX_INSET` なら成功。外れたらクランプせず `Internal`
/// - `-2`: この OS 版では取れない（`Unsupported`）
/// - `-1` とそれ以外: 読み取り失敗・未定義コード（`Internal`。成功にしない）
///
/// 値域の検査は f32 へ変換する前に行う。
pub fn insets_from_raw(status: i32, raw: [f64; 4]) -> Result<Insets, DeviceError> {
    match status {
        0 => {
            if raw
                .iter()
                .all(|v| v.is_finite() && (0.0..=MAX_INSET).contains(v))
            {
                Ok(Insets {
                    top: raw[0] as f32,
                    right: raw[1] as f32,
                    bottom: raw[2] as f32,
                    left: raw[3] as f32,
                })
            } else {
                Err(DeviceError::Internal("safe_area_insets read"))
            }
        }
        -2 => Err(DeviceError::Unsupported("safe_area_insets")),
        _ => Err(DeviceError::Internal("safe_area_insets read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedSafeArea.read(),
            Err(DeviceError::Unsupported("safe_area_insets"))
        ));
    }

    #[test]
    fn status_zero_maps_values() {
        let i = insets_from_raw(0, [1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(
            i,
            Insets {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 4.0
            }
        );
        assert!(insets_from_raw(0, [0.0; 4]).is_ok());
        assert!(insets_from_raw(0, [MAX_INSET; 4]).is_ok());
    }

    #[test]
    fn out_of_range_or_non_finite_never_succeed() {
        for bad in [
            -0.5,
            MAX_INSET + 1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            for side in 0..4 {
                let mut raw = [1.0; 4];
                raw[side] = bad;
                assert!(
                    matches!(insets_from_raw(0, raw), Err(DeviceError::Internal(_))),
                    "{bad} side {side}"
                );
            }
        }
    }

    #[test]
    fn sentinel_and_unknown_status_never_succeed() {
        assert!(matches!(
            insets_from_raw(-1, [0.0; 4]),
            Err(DeviceError::Internal("safe_area_insets read"))
        ));
        assert!(matches!(
            insets_from_raw(-2, [0.0; 4]),
            Err(DeviceError::Unsupported("safe_area_insets"))
        ));
        for c in [1, -3, i32::MIN, i32::MAX] {
            assert!(
                matches!(insets_from_raw(c, [0.0; 4]), Err(DeviceError::Internal(_))),
                "{c}"
            );
        }
    }
}
