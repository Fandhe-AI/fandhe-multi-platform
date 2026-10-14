//! 複数ディスプレイ・外部ディスプレイの一覧の契約（Issue #1341、親 #1134）。接続中の画面を
//! 画面ごとの物理解像度・倍率・主画面か・名前で返す橋渡しで、`fandhe-egui-proxy` の
//! `device/multi_display.rs` が間隔つきで [`MultiDisplay::read`] を呼ぶ（pull 型。OS の変化
//! 通知は購読しない）。Web は `fandhe-web-dom` 側が同じ [`validate`] を通す。
//!
//! # 契約
//!
//! - **三値**: 読めた（`Ok`）/ 非対応（`DeviceError::Unsupported`）/ 失敗（`DeviceError::Internal`）。
//!   Web の「許可なし」は画面側（core）の型で表し、ここでは新しい失敗 enum を作らない
//! - サイズは物理ピクセル。Web だけは CSS px × 画面ごとの devicePixelRatio を丸めた「約」の値
//! - `scale` は論理 1 に対する物理の比（取れない・不正は `None`）。`is_primary` は macOS が
//!   `NSScreen.screens` の先頭、Windows が `MONITORINFOF_PRIMARY`。主画面を決められなければ全て false
//! - `name` は OS が返す表示名をそのまま（空文字だけ `None`、trim しない）。取れない OS は `None`
//! - [`validate`] が契約を 1 か所で強制する。空の一覧・サイズ 0 の画面を成功にしない
//! - 個体識別子（EDID シリアル・`CGDisplaySerialNumber`・`Display.getUniqueId()`・
//!   `DisplayAddress`・`DisplayConfig` のパス）を読まない。名前・値をログへ出さない
//! - ミラーリングは OS の返すとおりにする。返るのは「OS が有効と認める論理ディスプレイ」で、
//!   物理ディスプレイの数と一致するとは限らない
//! - 呼び出し: UI スレッドから同期で呼ばれるため短時間で戻ること。最後の一覧を、切断後も
//!   返し続けてはならない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の値を出さないこと、主画面を決められなければ全て false、個体識別子を取らないこと、
//! 1 プロセス内の取得。
//! 守らない: OS 間の粒度の一致（Android の仮想ディスプレイ、Windows の名前なし、Web の「約」）、
//! 反映遅れ（最大 [`READ_INTERVAL`]。Web は背景タブの間引きでさらに遅れる）、ミラーリングの
//! OS 差、複数タブ・複数プロセス、Windows の擬似モニター、画面の位置・並び。

use std::time::Duration;

use crate::DeviceError;

/// 画面側が OS を読み直す間隔。`fandhe-egui-proxy` と `fandhe-web-dom` が共有する唯一の値。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 画面 1 枚の情報。
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenInfo {
    /// 幅（物理ピクセル。Web は約）。
    pub width_px: u32,
    /// 高さ（物理ピクセル。Web は約）。
    pub height_px: u32,
    /// 倍率（`None` は不明）。
    pub scale: Option<f32>,
    /// 主画面か。
    pub is_primary: bool,
    /// 表示名（取れない OS は `None`）。
    pub name: Option<String>,
}

/// 接続中の画面を 1 回読む。
pub trait MultiDisplay: Send + Sync {
    /// 現在の画面一覧を返す。呼び出し側が [`validate`] を通す。
    fn read(&self) -> Result<Vec<ScreenInfo>, DeviceError>;
}

/// 常に非対応を返す実装（対応 OS 以外・未実装 OS の dispatch 先）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedMultiDisplay;

impl MultiDisplay for UnsupportedMultiDisplay {
    fn read(&self) -> Result<Vec<ScreenInfo>, DeviceError> {
        Err(DeviceError::Unsupported("multi_display"))
    }
}

/// 契約を強制する。空の一覧・サイズ 0 の画面が 1 つでもあれば `Internal`（一部だけ捨てて
/// 個数を偽らない）。不正な `scale` は `None`、主画面がちょうど 1 つでなければ全て false、
/// 空の名前は `None`。並び順は変えない。
pub fn validate(r: Result<Vec<ScreenInfo>, DeviceError>) -> Result<Vec<ScreenInfo>, DeviceError> {
    let mut screens = r?;
    if screens.is_empty() || screens.iter().any(|s| s.width_px == 0 || s.height_px == 0) {
        return Err(DeviceError::Internal("multi_display read"));
    }
    if screens.iter().filter(|s| s.is_primary).count() != 1 {
        screens.iter_mut().for_each(|s| s.is_primary = false);
    }
    for s in &mut screens {
        s.scale = s.scale.filter(|v| v.is_finite() && *v > 0.0);
        if s.name.as_deref() == Some("") {
            s.name = None;
        }
    }
    Ok(screens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scr(primary: bool) -> ScreenInfo {
        ScreenInfo {
            width_px: 100,
            height_px: 50,
            scale: Some(2.0),
            is_primary: primary,
            name: Some("A".into()),
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let r = UnsupportedMultiDisplay.read();
        assert!(matches!(r, Err(DeviceError::Unsupported("multi_display"))));
    }

    #[test]
    fn empty_and_zero_size_are_errors() {
        assert!(matches!(
            validate(Ok(vec![])),
            Err(DeviceError::Internal(_))
        ));
        for (w, h) in [(0, 5), (5, 0)] {
            let mut bad = scr(false);
            bad.width_px = w;
            bad.height_px = h;
            assert!(validate(Ok(vec![scr(true), bad])).is_err());
        }
    }

    #[test]
    fn errors_pass_through() {
        assert!(matches!(
            validate(Err(DeviceError::Unsupported("multi_display"))),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn primary_must_be_exactly_one() {
        for v in [vec![scr(false), scr(false)], vec![scr(true), scr(true)]] {
            let out = validate(Ok(v)).unwrap();
            assert!(out.iter().all(|s| !s.is_primary));
        }
        let out = validate(Ok(vec![scr(false), scr(true)])).unwrap();
        assert_eq!(
            out.iter().map(|s| s.is_primary).collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn bad_scale_becomes_none() {
        for v in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            let mut s = scr(true);
            s.scale = Some(v);
            assert_eq!(validate(Ok(vec![s])).unwrap()[0].scale, None);
        }
    }

    #[test]
    fn name_only_empty_is_none_and_not_trimmed() {
        let mut a = scr(true);
        a.name = Some(String::new());
        let mut b = scr(false);
        b.name = Some(" X ".into());
        let out = validate(Ok(vec![a, b])).unwrap();
        assert_eq!(out[0].name, None);
        assert_eq!(out[1].name.as_deref(), Some(" X "));
    }
}
