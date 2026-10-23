//! ウィンドウ管理（位置・サイズ・最前面）（Phase 11 の拡充ループ、親 #127、Issue #636、対応表
//! `docs/device/capabilities/window-management.json`）。
//!
//! 扱う範囲は「今のウィンドウ 1 枚」の位置・大きさの読み戻しと、左上への移動・既定サイズへの復帰・
//! 最前面の ON / OFF だけ。対象は macOS（#636）、Windows（#637）、Web の PWA 窓（#638）。
//! 画面の列挙（multi-display）、全画面（fullscreen-immersive）、複数ウィンドウ、位置・大きさの保存と復元は扱わない。
//! `fandhe-egui-proxy` の橋渡し層が UI スレッドから [`WindowManager::request`] で操作を依頼し、
//! [`WindowManager::snapshot`] を毎フレーム読み戻して画面
//! （`fandhe_core::screens::window_management_show`）へ返す。
//!
//! # 契約
//!
//! - **依頼と完了を分ける**: `request` は OS へ依頼するだけで完了を待たない。成功は `snapshot` が
//!   目標に [`TOLERANCE`] 内で一致したとき（[`reached`]）だけ。期限は依頼から 3 秒
//!   （共通コアの `CONFIRM_TIMEOUT_SECS`）で、起点は単調時計（`Instant`、Web は `performance.now()`）。
//!   期限後の一致は失敗で確定し、表示は実際の状態に追従する
//! - **全画面中は断る**: 3 操作とも OS へ依頼を出さず失敗にする（全画面の状態が読めないときも同様）
//! - **読めない値は `None`**: 最小化中・ウィンドウ未準備などで `0` や `false` の代用をしない
//! - **エラー種別**: 非対応 → `Err(Unsupported)`、OS 呼び出しの失敗・入力不正 → `Err(Internal)`。
//!   権限は不要で利用者がキャンセルする経路も無いため `PermissionDenied` / `Cancelled` は返さない。
//!   契約表 10 の三値のうちキャンセルは該当なしで、成功 / 失敗の二値
//! - **書き込みは eframe の `ViewportCommand`**（`OuterPosition`・`InnerSize`・`WindowLevel`）。
//!   FFI は読み取り（最前面の読み戻しと作業領域）だけで、`SetWindowPos` 等を直接呼ばない
//!   （winit が持つフラグと食い違うため）
//! - **ログ**: 座標・画面の大きさ・OS のエラーコードをログへ出さない（利用環境の推定に使えるため）
//! - **座標系**: 左上原点。デスクトップは egui point（`zoom_factor` 込み）、Web は CSS px。
//!   macOS は Cocoa の左下原点を主画面（`NSScreen.screens[0]`）の高さで反転する
//!   （[`frame_from_cocoa`]、winit の `flip_window_screen_coordinates` と同じ）。
//!   Windows は `rcWork`（物理 px）を `pixels_per_point` で割る（#637）
//! - **既定サイズの SSOT** は `fandhe_core::screens::window_management_show::DEFAULT_INNER_SIZE`。
//!   `fandhe-core` は本クレートに依存できず、`main.rs` は feature `device` 無効時も使うため、
//!   ここへは複製せず [`WindowOp::ResizeInner`] が目標値を持つ
//! - **スレッド**: UI スレッドから同期で呼ぶ。ワーカーは作らない（契約表 11）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス 1 ウィンドウでの要求と完了の順序、古い世代の完了の破棄、表示が実際の位置・
//! 大きさ・最前面に追従すること、失敗・非対応・読めない状態を成功と見せないこと、全画面中に操作を
//! 出さないこと、Web のタイマーとリスナーの解放。
//!
//! 守らない（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）: 複数ウィンドウ、位置・大きさの
//! 保存と復元、倍率の違う画面をまたぐ移動での期限切れ、ドラッグ中の一時的なずれ、他アプリの最前面窓との前後、
//! 外部からの最前面変更の反映遅れ（最大 1 秒）、最小化中（読めないと出す）、プロセス終了時の後片付け
//! （OS がウィンドウごと片付ける）、Web のタブ・Chromium 以外。いずれも表示が一時的に食い違うか
//! 操作できないだけで、データ消失・資源漏れ・嘘の成功には至らない。
//!
//! # 後続 OS の目安
//!
//! - Windows（#637）: `GetWindowLongW(GWL_EXSTYLE)` の `WS_EX_TOPMOST` と、`MonitorFromWindow` +
//!   `GetMonitorInfoW` の `rcWork` を読む。外枠の見えない縁のため見た目は数 px 内側になる
//! - Web（#638）: インストールした PWA の窓で `moveTo` / `resizeTo`、`screenX` / `innerWidth` 等で読み戻す。
//!   最前面は不可（`always_on_top` を `false` にする）

use crate::DeviceError;

/// 位置・大きさ・最前面の一致とみなす許容誤差（egui point / CSS px）。
pub const TOLERANCE: f64 = 1.0;

/// 左上原点の矩形（位置と大きさ）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowFrame {
    /// 左端。
    pub x: f64,
    /// 上端。
    pub y: f64,
    /// 幅。
    pub width: f64,
    /// 高さ。
    pub height: f64,
}

/// 実装が提供できる操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSupport {
    /// 位置・大きさを変えられる。
    pub move_resize: bool,
    /// 最前面を切り替えられる。
    pub always_on_top: bool,
}

/// 依頼する操作。目標値は呼び出し側が持つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowOp {
    /// 外枠の左上を `(x, y)` へ動かす。
    MoveTo {
        /// 左端。
        x: f64,
        /// 上端。
        y: f64,
    },
    /// 内側の大きさを変える。
    ResizeInner {
        /// 幅。
        width: f64,
        /// 高さ。
        height: f64,
    },
    /// 最前面の ON / OFF。
    SetAlwaysOnTop(bool),
}

/// OS から読み戻した現在の状態。読めない項目は `None`。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowSnapshot {
    /// 外枠（位置の判定に使う）。最小化中・読めないとき `None`。
    pub outer: Option<WindowFrame>,
    /// 内側（大きさの判定に使う）。
    pub inner: Option<WindowFrame>,
    /// 最前面か。
    pub always_on_top: Option<bool>,
    /// 全画面か。
    pub fullscreen: Option<bool>,
}

/// ウィンドウ管理の契約（モジュール doc の「契約」節）。
pub trait WindowManager: Send + Sync {
    /// 提供できる操作。
    fn support(&self) -> WindowSupport;

    /// 現在の状態。読めない項目は `None`。
    fn snapshot(&self) -> WindowSnapshot;

    /// 今いる画面の作業領域（メニューバー・Dock・タスクバーを除く）。`snapshot` と同じ座標系。
    fn target(&self) -> Result<WindowFrame, DeviceError>;

    /// 操作を依頼する。完了は待たない。
    fn request(&self, op: WindowOp) -> Result<(), DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedWindowManager;

impl WindowManager for UnsupportedWindowManager {
    fn support(&self) -> WindowSupport {
        WindowSupport {
            move_resize: false,
            always_on_top: false,
        }
    }

    fn snapshot(&self) -> WindowSnapshot {
        WindowSnapshot::default()
    }

    fn target(&self) -> Result<WindowFrame, DeviceError> {
        Err(DeviceError::Unsupported("window-management"))
    }

    fn request(&self, _op: WindowOp) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("window-management"))
    }
}

fn near(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= TOLERANCE
}

/// `snap` が `op` の目標に達しているか。該当項目が `None`・非有限なら `false`（嘘の成功の禁止）。
pub fn reached(op: &WindowOp, snap: &WindowSnapshot) -> bool {
    match *op {
        WindowOp::MoveTo { x, y } => snap.outer.is_some_and(|f| near(f.x, x) && near(f.y, y)),
        WindowOp::ResizeInner { width, height } => snap
            .inner
            .is_some_and(|f| near(f.width, width) && near(f.height, height)),
        WindowOp::SetAlwaysOnTop(on) => snap.always_on_top == Some(on),
    }
}

/// Cocoa（左下原点）の `visibleFrame` を左上原点の [`WindowFrame`] へ変換する。
///
/// `visible` は `[x, y, 幅, 高さ]`、`primary_height` は主画面（`NSScreen.screens[0]`）の高さ、
/// `zoom` は egui の `zoom_factor`。非有限・幅高さが 0 以下・`primary_height` / `zoom` が 0 以下は
/// `None`（検査は変換の前に行う）。
pub fn frame_from_cocoa(visible: [f64; 4], primary_height: f64, zoom: f64) -> Option<WindowFrame> {
    let [x, y, w, h] = visible;
    let finite = [x, y, w, h, primary_height, zoom]
        .iter()
        .all(|v| v.is_finite());
    if !finite || w <= 0.0 || h <= 0.0 || primary_height <= 0.0 || zoom <= 0.0 {
        return None;
    }
    Some(WindowFrame {
        x: x / zoom,
        y: (primary_height - (y + h)) / zoom,
        width: w / zoom,
        height: h / zoom,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, w: f64, h: f64) -> WindowFrame {
        WindowFrame {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn unsupported_never_succeeds_nor_reads() {
        let m = UnsupportedWindowManager;
        assert_eq!(
            m.support(),
            WindowSupport {
                move_resize: false,
                always_on_top: false
            }
        );
        assert_eq!(m.snapshot(), WindowSnapshot::default());
        assert!(matches!(
            m.target(),
            Err(DeviceError::Unsupported("window-management"))
        ));
        for op in [
            WindowOp::MoveTo { x: 0.0, y: 0.0 },
            WindowOp::ResizeInner {
                width: 1.0,
                height: 1.0,
            },
            WindowOp::SetAlwaysOnTop(true),
        ] {
            assert!(matches!(
                m.request(op),
                Err(DeviceError::Unsupported("window-management"))
            ));
        }
    }

    #[test]
    fn reached_respects_tolerance() {
        let op = WindowOp::MoveTo { x: 0.0, y: 25.0 };
        let snap = |x, y| WindowSnapshot {
            outer: Some(frame(x, y, 100.0, 100.0)),
            ..Default::default()
        };
        assert!(reached(&op, &snap(0.5, 25.9)));
        assert!(!reached(&op, &snap(2.0, 25.0)));
        assert!(!reached(&op, &snap(0.0, 27.5)));
    }

    #[test]
    fn reached_is_false_when_unreadable_or_not_finite() {
        let move_op = WindowOp::MoveTo { x: 0.0, y: 0.0 };
        let size_op = WindowOp::ResizeInner {
            width: 480.0,
            height: 360.0,
        };
        let top_op = WindowOp::SetAlwaysOnTop(false);
        let empty = WindowSnapshot::default();
        assert!(!reached(&move_op, &empty));
        assert!(!reached(&size_op, &empty));
        assert!(!reached(&top_op, &empty));
        let nan = WindowSnapshot {
            outer: Some(frame(f64::NAN, 0.0, 1.0, 1.0)),
            inner: Some(frame(0.0, 0.0, f64::INFINITY, 360.0)),
            ..Default::default()
        };
        assert!(!reached(&move_op, &nan));
        assert!(!reached(&size_op, &nan));
    }

    #[test]
    fn reached_size_uses_inner_and_top_is_exact() {
        let size_op = WindowOp::ResizeInner {
            width: 480.0,
            height: 360.0,
        };
        let snap = WindowSnapshot {
            inner: Some(frame(10.0, 10.0, 480.4, 359.6)),
            always_on_top: Some(true),
            ..Default::default()
        };
        assert!(reached(&size_op, &snap));
        assert!(reached(&WindowOp::SetAlwaysOnTop(true), &snap));
        assert!(!reached(&WindowOp::SetAlwaysOnTop(false), &snap));
    }

    #[test]
    fn frame_from_cocoa_flips_y_with_primary_height() {
        // 主画面の高さ 900、作業領域は下端 0 から高さ 875（メニューバー 25 を除く）。
        let f = frame_from_cocoa([0.0, 0.0, 1440.0, 875.0], 900.0, 1.0).unwrap();
        assert_eq!(f, frame(0.0, 25.0, 1440.0, 875.0));
    }

    #[test]
    fn frame_from_cocoa_applies_zoom() {
        let f = frame_from_cocoa([0.0, 0.0, 1440.0, 875.0], 900.0, 2.0).unwrap();
        assert_eq!(f, frame(0.0, 12.5, 720.0, 437.5));
    }

    #[test]
    fn frame_from_cocoa_rejects_invalid_input() {
        let ok = [0.0, 0.0, 100.0, 100.0];
        assert!(frame_from_cocoa(ok, 900.0, 1.0).is_some());
        assert!(frame_from_cocoa([f64::NAN, 0.0, 100.0, 100.0], 900.0, 1.0).is_none());
        assert!(frame_from_cocoa([0.0, 0.0, f64::INFINITY, 100.0], 900.0, 1.0).is_none());
        assert!(frame_from_cocoa([0.0, 0.0, 0.0, 100.0], 900.0, 1.0).is_none());
        assert!(frame_from_cocoa([0.0, 0.0, 100.0, -1.0], 900.0, 1.0).is_none());
        assert!(frame_from_cocoa(ok, 0.0, 1.0).is_none());
        assert!(frame_from_cocoa(ok, 900.0, 0.0).is_none());
        assert!(frame_from_cocoa(ok, f64::NAN, 1.0).is_none());
    }
}
