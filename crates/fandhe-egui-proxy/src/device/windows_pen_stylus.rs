//! Windows 向けペン・筆圧入力実装（Issue #359、親 #95、契約は `fandhe_device::pen_stylus`）。
//! `device/pen_stylus.rs` の dispatch から呼ばれ、effect ドライバが UI スレッドから `latest()` を pull する。
//!
//! winit 0.30 は `WM_POINTER*` を `WindowEvent::Touch` に変換するとき、種類（ペン・指）と傾きを捨て、
//! メッセージ自体も消費する。そこでメインウィンドウ（`windows_share::window()`）へ `SetWindowSubclass` で
//! 観測専用のサブクラスを 1 回だけ付け、主ポインタ 1 本の種類・筆圧・傾きを `SharedPen` へ置く。
//! 位置は従来どおり egui のポインタから取る。サブクラスは常に `DefSubclassProc` へ流し、メッセージを消費しない。
//!
//! - 筆圧・傾きは `Pen`（ペン・消しゴム）で、かつ OS が値の有効を示すマスクを立てたときだけ出す
//! - 傾きは X・Y の両方が有効なときだけ出す（片方欠けた 0 を垂直と取り違えない）
//! - マウスも扱う: winit は `EnableMouseInPointer` を呼ばず、マウスは `WM_LBUTTON*` で届く。
//!   拾わないと押下中に `Ok(None)` となりストロークが終わり、マウスで描けなくなる
//! - 離す・捕捉喪失・ホバー・非アクティブ化・ウィンドウ破棄で `clear` し、最後の値を出し続けない
//! - 登録はサブクラスをウィンドウと同じスレッドで付ける必要があるため `latest()` の初回に遅延する。
//!   HWND 未取得・スレッド違い・登録失敗は `Internal` で返し、キャッシュしない（次回の呼び出しで回復する）
//! - 再描画は要求しない。同じポインタは winit 経由で egui にも届き、押下中はドライバが次の描画を予約している
//! - 筆圧・座標・描画内容はログへ出さない
//!
//! 守る範囲: 1 プロセス内の順序・嘘の値を出さないこと・離したら消すこと。
//! 守らない範囲: 主ポインタ 1 本以外、ホバー・消しゴムの区別、履歴サンプル、静止したままの筆圧変化、複数ウィンドウ。
// ponytail: 主ポインタ 1 本・履歴サンプル（`GetPointerPenInfoHistory`）を読まない。速い筆致で点が粗くなるなら履歴を足す。

use std::sync::Arc;

#[cfg(not(target_os = "windows"))]
use fandhe_device::pen_stylus::UnsupportedPenStylus;
use fandhe_device::pen_stylus::{altitude_from_tilt_deg, PenSample, PenStylus, PointerKind};

// ホストのテストでも使えるよう、Win32 の定数はここで持つ（Windows ビルドでは imp の assert で windows クレートと突き合わせる）。
const WM_KILLFOCUS: u32 = 0x0008;
const WM_ACTIVATEAPP: u32 = 0x001C;
const WM_NCDESTROY: u32 = 0x0082;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_POINTERUPDATE: u32 = 0x0245;
const WM_POINTERDOWN: u32 = 0x0246;
const WM_POINTERUP: u32 = 0x0247;
const WM_POINTERLEAVE: u32 = 0x024A;
const WM_POINTERCAPTURECHANGED: u32 = 0x024C;
const MK_LBUTTON: usize = 0x0001;
const POINTER_FLAG_INCONTACT: u32 = 0x0004;
const POINTER_FLAG_PRIMARY: u32 = 0x0010;
const PT_TOUCH: i32 = 2;
const PT_PEN: i32 = 3;
const PT_MOUSE: i32 = 4;
const PT_TOUCHPAD: i32 = 5;
const PEN_MASK_PRESSURE: u32 = 0x1;
const PEN_MASK_TILT_X: u32 = 0x4;
const PEN_MASK_TILT_Y: u32 = 0x8;
/// ペン・タッチから昇格したマウスメッセージの `GetMessageExtraInfo` の署名（上位 24 bit）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MOUSE_FROM_POINTER_SIGNATURE: isize = 0xFF51_5700;

/// サブクラスプロシージャがメッセージごとに取る動作。
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
enum Action {
    /// ポインタ ID を読んで値を置く。
    ReadPointer(u32),
    /// マウスの左ボタン押下を置く。
    Mouse,
    /// 接触なしに戻す。
    Clear,
    /// ウィンドウ破棄。接触なしに戻してサブクラスを外す。
    Destroy,
    /// 何もしない。
    Ignore,
}

/// メッセージを動作に分類する純関数。`from_pointer` はマウスメッセージがペン・タッチ由来か。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn classify(msg: u32, wparam: usize, from_pointer: bool) -> Action {
    match msg {
        WM_POINTERDOWN | WM_POINTERUPDATE => {
            let flags = ((wparam >> 16) & 0xFFFF) as u32;
            if flags & POINTER_FLAG_PRIMARY == 0 {
                Action::Ignore
            } else if flags & POINTER_FLAG_INCONTACT == 0 {
                Action::Clear
            } else {
                Action::ReadPointer((wparam & 0xFFFF) as u32)
            }
        }
        WM_POINTERUP | WM_POINTERCAPTURECHANGED | WM_POINTERLEAVE => Action::Clear,
        WM_LBUTTONDOWN if !from_pointer => Action::Mouse,
        WM_MOUSEMOVE if !from_pointer && wparam & MK_LBUTTON != 0 => Action::Mouse,
        WM_LBUTTONUP | WM_CAPTURECHANGED | WM_KILLFOCUS => Action::Clear,
        WM_ACTIVATEAPP if wparam == 0 => Action::Clear,
        WM_NCDESTROY => Action::Destroy,
        _ => Action::Ignore,
    }
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn kind_from_pointer_type(t: i32) -> PointerKind {
    match t {
        PT_PEN => PointerKind::Pen,
        PT_TOUCH => PointerKind::Touch,
        PT_MOUSE | PT_TOUCHPAD => PointerKind::Mouse,
        _ => PointerKind::Unknown,
    }
}

/// `POINTER_PEN_INFO` の生値を [`PenSample`] にする。マスクが立っていない値は `None`。
/// 検証・丸めは変換の前に済ませる（`altitude_from_tilt_deg` / `PenSample::new`）。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn sample_from_pen(mask: u32, pressure: u32, tilt_x: i32, tilt_y: i32) -> PenSample {
    let pressure = (mask & PEN_MASK_PRESSURE != 0).then(|| pressure as f32 / 1024.0);
    let altitude = (mask & PEN_MASK_TILT_X != 0 && mask & PEN_MASK_TILT_Y != 0)
        .then(|| altitude_from_tilt_deg(tilt_x as f32, tilt_y as f32))
        .flatten();
    PenSample::new(PointerKind::Pen, pressure, altitude)
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use fandhe_device::pen_stylus::SharedPen;
    use fandhe_device::DeviceError;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::Pointer::{GetPointerPenInfo, GetPointerType, POINTER_PEN_INFO};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging as wm;

    // 手書きの定数が windows クレートの値と一致することをビルド時に確かめる。
    const _: () = {
        assert!(WM_KILLFOCUS == wm::WM_KILLFOCUS);
        assert!(WM_ACTIVATEAPP == wm::WM_ACTIVATEAPP);
        assert!(WM_NCDESTROY == wm::WM_NCDESTROY);
        assert!(WM_MOUSEMOVE == wm::WM_MOUSEMOVE);
        assert!(WM_LBUTTONDOWN == wm::WM_LBUTTONDOWN);
        assert!(WM_LBUTTONUP == wm::WM_LBUTTONUP);
        assert!(WM_CAPTURECHANGED == wm::WM_CAPTURECHANGED);
        assert!(WM_POINTERUPDATE == wm::WM_POINTERUPDATE);
        assert!(WM_POINTERDOWN == wm::WM_POINTERDOWN);
        assert!(WM_POINTERUP == wm::WM_POINTERUP);
        assert!(WM_POINTERLEAVE == wm::WM_POINTERLEAVE);
        assert!(WM_POINTERCAPTURECHANGED == wm::WM_POINTERCAPTURECHANGED);
        assert!(PT_PEN == wm::PT_PEN.0);
        assert!(PT_TOUCH == wm::PT_TOUCH.0);
        assert!(PT_MOUSE == wm::PT_MOUSE.0);
        assert!(PT_TOUCHPAD == wm::PT_TOUCHPAD.0);
    };

    const SUBCLASS_ID: usize = 0x5045_4E31; // "PEN1"

    fn pen() -> &'static SharedPen {
        static PEN: OnceLock<SharedPen> = OnceLock::new();
        PEN.get_or_init(SharedPen::new)
    }

    static INSTALLED: AtomicBool = AtomicBool::new(false);

    /// 初回だけサブクラスを付ける。ウィンドウと別スレッドからは付けられないため失敗として返す。
    #[allow(unsafe_code)]
    fn ensure_installed() -> Result<(), DeviceError> {
        if INSTALLED.load(Ordering::Acquire) {
            return Ok(());
        }
        let err = || DeviceError::Internal("pen_stylus");
        let hwnd = super::super::windows_share::window().ok_or_else(err)?;
        // SAFETY: `hwnd` は `windows_share::window()` が返したメインウィンドウ。
        // 引数は値渡しで、ポインタを保持しない。
        let owner = unsafe { wm::GetWindowThreadProcessId(hwnd, None) };
        // SAFETY: 引数なしの読み取り専用呼び出し。
        if owner != unsafe { GetCurrentThreadId() } {
            return Err(err());
        }
        // SAFETY: 同一スレッドの有効な HWND に、状態を持たない（`dwRefData` = 0）観測専用の
        // プロシージャを付ける。外すのは `WM_NCDESTROY` で行い、ダングリングを残さない。
        let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) };
        if !ok.as_bool() {
            return Err(err());
        }
        INSTALLED.store(true, Ordering::Release);
        Ok(())
    }

    /// 主ポインタの種類・筆圧・傾きを読んで置く。読めなければ嘘の値を出さず接触なしにする。
    #[allow(unsafe_code)]
    fn read_pointer(id: u32) {
        let mut ty = wm::POINTER_INPUT_TYPE::default();
        // SAFETY: `id` は現在のメッセージのポインタ ID、出力先はローカル変数。
        if unsafe { GetPointerType(id, &mut ty) }.is_err() {
            pen().clear();
            return;
        }
        let kind = kind_from_pointer_type(ty.0);
        if kind != PointerKind::Pen {
            pen().set(PenSample::new(kind, None, None));
            return;
        }
        let mut info = POINTER_PEN_INFO::default();
        // SAFETY: 同上。出力先はローカルの `POINTER_PEN_INFO`。
        let sample = if unsafe { GetPointerPenInfo(id, &mut info) }.is_ok() {
            sample_from_pen(info.penMask, info.pressure, info.tiltX, info.tiltY)
        } else {
            PenSample::new(PointerKind::Pen, None, None)
        };
        pen().set(sample);
    }

    /// 観測専用のサブクラスプロシージャ。常に `DefSubclassProc` へ流し、メッセージを消費しない。
    #[allow(unsafe_code)]
    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _ref_data: usize,
    ) -> LRESULT {
        let from_pointer = matches!(msg, WM_LBUTTONDOWN | WM_MOUSEMOVE)
            // SAFETY: 引数なしの読み取り専用呼び出し。
            && (unsafe { wm::GetMessageExtraInfo() }.0 & 0xFFFF_FF00) == MOUSE_FROM_POINTER_SIGNATURE;
        match classify(msg, wparam.0, from_pointer) {
            Action::ReadPointer(id) => read_pointer(id),
            Action::Mouse => pen().set(PenSample::new(PointerKind::Mouse, None, None)),
            Action::Clear => pen().clear(),
            Action::Destroy => {
                pen().clear();
                INSTALLED.store(false, Ordering::Release);
                // SAFETY: 付けたときと同じ HWND・ID。`WM_NCDESTROY` の処理中に外すのは公式の手順。
                let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            }
            Action::Ignore => {}
        }
        // SAFETY: このプロシージャが受け取った引数をそのまま次へ渡す。
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub(super) struct WindowsPenStylus;

    impl PenStylus for WindowsPenStylus {
        fn latest(&self) -> Result<Option<PenSample>, DeviceError> {
            ensure_installed()?;
            pen().latest()
        }
    }
}

/// この OS の実装。Windows 以外（ホストのテスト）は非対応を返す。
pub(crate) fn implementation() -> Arc<dyn PenStylus> {
    #[cfg(target_os = "windows")]
    {
        Arc::new(imp::WindowsPenStylus)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Arc::new(UnsupportedPenStylus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ptr_wparam(id: u32, flags: u32) -> usize {
        ((flags as usize) << 16) | id as usize
    }

    #[test]
    fn pressure_is_scaled_and_clamped() {
        let p = |pr| sample_from_pen(PEN_MASK_PRESSURE, pr, 0, 0).pressure;
        assert_eq!(p(1024), Some(1.0));
        assert_eq!(p(512), Some(0.5));
        assert_eq!(p(5000), Some(1.0));
        assert_eq!(sample_from_pen(0, 512, 0, 0).pressure, None);
    }

    #[test]
    fn tilt_needs_both_axes() {
        let both = PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
        assert_eq!(sample_from_pen(both, 0, 0, 0).altitude_deg, Some(90.0));
        assert_eq!(sample_from_pen(PEN_MASK_TILT_X, 0, 0, 0).altitude_deg, None);
        assert_eq!(sample_from_pen(PEN_MASK_TILT_Y, 0, 0, 0).altitude_deg, None);
        assert_eq!(sample_from_pen(both, 0, 91, 0).altitude_deg, None);
    }

    #[test]
    fn pointer_kinds() {
        assert_eq!(kind_from_pointer_type(PT_PEN), PointerKind::Pen);
        assert_eq!(kind_from_pointer_type(PT_TOUCH), PointerKind::Touch);
        assert_eq!(kind_from_pointer_type(PT_MOUSE), PointerKind::Mouse);
        assert_eq!(kind_from_pointer_type(PT_TOUCHPAD), PointerKind::Mouse);
        assert_eq!(kind_from_pointer_type(1), PointerKind::Unknown);
    }

    #[test]
    fn pointer_messages_classify() {
        let contact = POINTER_FLAG_PRIMARY | POINTER_FLAG_INCONTACT;
        assert_eq!(
            classify(WM_POINTERDOWN, ptr_wparam(7, contact), false),
            Action::ReadPointer(7)
        );
        assert_eq!(
            classify(WM_POINTERUPDATE, ptr_wparam(7, POINTER_FLAG_PRIMARY), false),
            Action::Clear
        );
        assert_eq!(
            classify(
                WM_POINTERUPDATE,
                ptr_wparam(7, POINTER_FLAG_INCONTACT),
                false
            ),
            Action::Ignore
        );
        for m in [WM_POINTERUP, WM_POINTERCAPTURECHANGED, WM_POINTERLEAVE] {
            assert_eq!(classify(m, 0, false), Action::Clear);
        }
    }

    #[test]
    fn mouse_and_lifecycle_messages_classify() {
        assert_eq!(classify(WM_LBUTTONDOWN, 0, false), Action::Mouse);
        assert_eq!(classify(WM_MOUSEMOVE, MK_LBUTTON, false), Action::Mouse);
        assert_eq!(classify(WM_MOUSEMOVE, 0, false), Action::Ignore);
        assert_eq!(classify(WM_LBUTTONDOWN, 0, true), Action::Ignore);
        assert_eq!(classify(WM_MOUSEMOVE, MK_LBUTTON, true), Action::Ignore);
        for m in [WM_LBUTTONUP, WM_CAPTURECHANGED, WM_KILLFOCUS] {
            assert_eq!(classify(m, 0, false), Action::Clear);
        }
        assert_eq!(classify(WM_ACTIVATEAPP, 0, false), Action::Clear);
        assert_eq!(classify(WM_ACTIVATEAPP, 1, false), Action::Ignore);
        assert_eq!(classify(WM_NCDESTROY, 0, false), Action::Destroy);
    }
}
