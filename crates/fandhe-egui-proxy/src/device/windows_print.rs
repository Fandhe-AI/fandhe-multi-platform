//! Windows 向け印刷の実装（Issue #326）。`PrintDlgExW`（`PD_RETURNDC`）で印刷ダイアログを出し、
//! 返ってきたプリンタ DC へ GDI（`StartDocW` / `DrawTextW` / `EndDoc`）で固定の短い文章を 1 ページ描く。
//! `device/print.rs` の Windows 経路から呼ばれ、契約は `fandhe_device::print`。見出し・本文は
//! `PrintRequest` で検査済みのものだけを OS へ渡す。UI スレッドで同期に呼ぶ（macOS と同じ同期モーダル）。
//!
//! 結果の写像（純関数 [`after_dialog`] / [`start_doc_failure`]）はホストでもテストできるよう、
//! モジュールは `cfg(any(windows, test))`。`windows` crate を使う ffi・`WindowsPrint` は
//! `cfg(target_os = "windows")` に閉じる。
//!
//! # 写像
//!
//! - `PD_RESULT_PRINT` でジョブを投入 → `Submitted`（PDF 出力を含む。紙に出たことは意味しない）
//! - `PD_RESULT_CANCEL` / `PD_RESULT_APPLY` → `Cancelled`
//! - `StartDocW` の `ERROR_CANCELLED` → `Cancelled`。「Microsoft Print to PDF」は `StartDocW` の
//!   中で保存先ダイアログを出すため、そこでの取り消しを失敗に見せない（契約表 10）。trait doc の
//!   表は CANCEL / APPLY だけを挙げるが、「取り消しを失敗に畳まない」原則の当てはめとして扱う。
//!   実際に 1223 が返るかは実機確認（#324）
//! - ダイアログを出す前の失敗 → `Internal("print present")`、描画・投入の失敗 →
//!   `Internal("print failed")`、未知の結果コード → `Internal("print outcome")`
//! - `Unconfirmed` には到達しない
//!
//! # 既知の天井
//!
//! - ponytail: 1 ページのみ。折り返し後の高さが用紙を超えた分は切れる（現在の固定文は収まる）。
//!   上げ方は `DT_CALCRECT` で行を送って `EndPage` / `StartPage` で改ページする
//! - 書体 `Yu Gothic UI` が無い環境では GDI が代替書体を選ぶ。書体・余白の差は守らない範囲
//! - `PrintDlgExW` は自前のメッセージループを回すため、表示中は egui の描画が止まる
//! - 紙に出たか、投入後のプリンタ側の失敗は追わない
//! - 見出し・本文・HRESULT・`GetLastError` の値はログに出さない

use fandhe_device::print::PrintOutcome;
use fandhe_device::DeviceError;

/// `PRINTDLGEXW.dwResultAction`: 印刷が選ばれた。
const PD_RESULT_PRINT: u32 = 1;
/// 取り消し。
const PD_RESULT_CANCEL: u32 = 0;
/// 適用のみ（印刷せずに閉じた）。
const PD_RESULT_APPLY: u32 = 2;
/// `GetLastError` の `ERROR_CANCELLED`（PDF 保存先ダイアログの取り消し）。
const ERROR_CANCELLED: u32 = 1223;

/// ダイアログの結果 → 次の動作。`Ok(None)` は印刷へ進む、`Ok(Some(_))` はそこで確定。
/// 未知の値は失敗に倒し成功を作らない。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn after_dialog(action: u32) -> Result<Option<PrintOutcome>, DeviceError> {
    match action {
        PD_RESULT_PRINT => Ok(None),
        PD_RESULT_CANCEL | PD_RESULT_APPLY => Ok(Some(PrintOutcome::Cancelled)),
        _ => Err(DeviceError::Internal("print outcome")),
    }
}

/// `StartDocW` 失敗時の `GetLastError` → 結果。取り消しだけ `Cancelled`、他は失敗。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn start_doc_failure(last_error: u32) -> Result<PrintOutcome, DeviceError> {
    if last_error == ERROR_CANCELLED {
        Ok(PrintOutcome::Cancelled)
    } else {
        Err(DeviceError::Internal("print failed"))
    }
}

#[cfg(target_os = "windows")]
mod ffi {
    use super::{after_dialog, start_doc_failure};
    use fandhe_device::print::PrintOutcome;
    use fandhe_device::DeviceError;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, GlobalFree, RECT};
    use windows::Win32::Graphics::Gdi::{
        CreateFontW, DeleteDC, DeleteObject, DrawTextW, GetDeviceCaps, SelectObject,
        CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DEFAULT_QUALITY, DT_EDITCONTROL,
        DT_NOPREFIX, DT_WORDBREAK, FF_DONTCARE, FW_BOLD, FW_NORMAL, HDC, HFONT, HGDIOBJ, HORZRES,
        LOGPIXELSX, LOGPIXELSY, OUT_DEFAULT_PRECIS,
    };
    use windows::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};
    use windows::Win32::UI::Controls::Dialogs::{
        PrintDlgExW, PD_NOCURRENTPAGE, PD_NOPAGENUMS, PD_NOSELECTION, PD_RETURNDC,
        PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGEXW, PRINTPAGERANGE, START_PAGE_GENERAL,
    };

    const PRESENT: DeviceError = DeviceError::Internal("print present");
    const FAILED: DeviceError = DeviceError::Internal("print failed");

    /// `PrintDlgExW` が返した DC・`hDevMode`・`hDevNames` を、早期 return を含む全経路で解放する。
    struct DialogResult(PRINTDLGEXW);

    impl Drop for DialogResult {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: 各ハンドルは `PrintDlgExW` が確保して返したもので、この Drop で一度だけ
            // 解放する。NULL は解放しない。
            unsafe {
                if !self.0.hDevMode.0.is_null() {
                    let _ = GlobalFree(Some(self.0.hDevMode));
                }
                if !self.0.hDevNames.0.is_null() {
                    let _ = GlobalFree(Some(self.0.hDevNames));
                }
                if !self.0.hDC.0.is_null() {
                    let _ = DeleteDC(self.0.hDC);
                }
            }
        }
    }

    /// 作成したフォントを元のフォントへ戻してから削除する。
    struct Font {
        dc: HDC,
        font: HFONT,
        old: HGDIOBJ,
    }

    impl Drop for Font {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: `dc` は呼び出し中有効（`DialogResult` より先に drop される）。`font` は
            // 自分が作成したもので、選択を戻してから一度だけ削除する。
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.font.into());
            }
        }
    }

    /// 指定ポイントの `Yu Gothic UI` を作って DC に選択する。
    ///
    /// # Safety
    /// `dc` が有効なプリンタ DC であること。
    #[allow(unsafe_code)]
    unsafe fn select_font(dc: HDC, pt: i32, weight: i32) -> Option<Font> {
        // SAFETY: 前提により `dc` は有効なプリンタ DC。GetDeviceCaps は値を返すだけでポインタを受け取らない。
        let dpi = unsafe { GetDeviceCaps(Some(dc), LOGPIXELSY) };
        // SAFETY: 書体名は `w!` の NUL 終端静的文字列で呼び出し中有効。他は値渡しで、戻り値は直後に null 検査する。
        let font = unsafe {
            CreateFontW(
                -(pt * dpi / 72),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Yu Gothic UI"),
            )
        };
        if font.0.is_null() {
            return None;
        }
        // SAFETY: `dc` は前提により有効、`font` は直前に作成し null 検査済み。旧オブジェクトは `Font` の Drop で戻す。
        let old = unsafe { SelectObject(dc, font.into()) };
        Some(Font { dc, font, old })
    }

    /// 1 段落を幅 `width` で折り返して `y` から描き、描いた高さを返す。0 以下は失敗。
    ///
    /// # Safety
    /// `dc` が有効なプリンタ DC で、ページが開始済みであること。
    #[allow(unsafe_code)]
    unsafe fn draw(dc: HDC, text: &str, x: i32, y: i32, width: i32) -> i32 {
        let mut buf: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: i32::MAX / 2,
        };
        // SAFETY: `buf`・`rect` はこの関数のローカルで呼び出し中有効かつ排他。`dc` は前提により有効でページ開始済み。
        unsafe {
            DrawTextW(
                dc,
                &mut buf,
                &mut rect,
                DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
            )
        }
    }

    /// 開始済みのジョブに 1 ページ描いて終える。失敗時は呼び出し側が `AbortDoc` する。
    ///
    /// # Safety
    /// `dc` が有効なプリンタ DC で、`StartDocW` 済みであること。
    #[allow(unsafe_code)]
    unsafe fn render(dc: HDC, title: &str, body: &str) -> Result<(), DeviceError> {
        // SAFETY: 前提により `dc` は有効で StartDocW 済み。ページ開始の成否は戻り値で検査する。
        if unsafe { StartPage(dc) } <= 0 {
            return Err(FAILED);
        }
        // SAFETY: `dc` は前提により有効。以下の GetDeviceCaps 3 件も値を返すだけ。
        let mx = unsafe { GetDeviceCaps(Some(dc), LOGPIXELSX) } / 2;
        // SAFETY: 同上。
        let my = unsafe { GetDeviceCaps(Some(dc), LOGPIXELSY) } / 2;
        // SAFETY: 同上。
        let width = unsafe { GetDeviceCaps(Some(dc), HORZRES) } - 2 * mx;
        let mut y = my;
        for (pt, weight, text) in [
            (18, FW_BOLD.0 as i32, title),
            (12, FW_NORMAL.0 as i32, body),
        ] {
            // SAFETY: `dc` は有効で、ページは StartPage 済み。`_font` は `dc` が有効なうちにこのスコープで drop される。
            let _font = unsafe { select_font(dc, pt, weight) }.ok_or(FAILED)?;
            // SAFETY: `dc` は有効でページ開始済み（上の StartPage 成功による）。文字列は呼び出し中借用される。
            let h = unsafe { draw(dc, text, mx, y, width) };
            if h <= 0 {
                return Err(FAILED);
            }
            // 見出しと本文の間に半行ぶん空ける。
            y += h + h / 2;
        }
        // SAFETY: `dc` は有効で、ページは StartPage 済み・未終了。
        if unsafe { EndPage(dc) } <= 0 {
            return Err(FAILED);
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    pub(super) fn print_text(title: &str, body: &str) -> Result<PrintOutcome, DeviceError> {
        let hwnd = super::super::windows_share::window().ok_or(PRESENT)?;
        // 要素 1 個のバッファ。NULL を渡したときの扱いに依存しない。
        let mut ranges = [PRINTPAGERANGE::default()];
        let mut pd = PRINTDLGEXW {
            lStructSize: size_of::<PRINTDLGEXW>() as u32,
            hwndOwner: hwnd,
            Flags: PD_RETURNDC
                | PD_NOPAGENUMS
                | PD_NOSELECTION
                | PD_NOCURRENTPAGE
                | PD_USEDEVMODECOPIESANDCOLLATE,
            nPageRanges: 0,
            nMaxPageRanges: 1,
            lpPageRanges: ranges.as_mut_ptr(),
            nMinPage: 1,
            nMaxPage: 1,
            nStartPage: START_PAGE_GENERAL,
            ..Default::default()
        };
        // SAFETY: `pd` / `ranges` はこの呼び出しの間だけ生きるローカル値。`hwnd` は
        // `windows_share::window()` が返したメインウィンドウ（`ProxyApp` が生きている間は有効）。
        let res = unsafe { PrintDlgExW(&mut pd) };
        // 成否にかかわらず、返ったハンドルはガードで解放する。
        let pd = DialogResult(pd);
        res.map_err(|_| PRESENT)?;
        if let Some(done) = after_dialog(pd.0.dwResultAction)? {
            return Ok(done);
        }
        let dc = pd.0.hDC;
        if dc.0.is_null() {
            return Err(PRESENT);
        }

        let mut name: Vec<u16> = title.encode_utf16().collect();
        name.push(0);
        let doc = DOCINFOW {
            cbSize: size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: `dc` は `pd` ガードが生きている間有効。`doc` / `name` はこの呼び出しの間
        // 生きる（`name` は NUL 終端）。
        if unsafe { StartDocW(dc, &doc) } <= 0 {
            // SAFETY: 直前の呼び出しの失敗理由を同一スレッドで読む。
            let last = unsafe { GetLastError() }.0;
            return start_doc_failure(last);
        }
        // SAFETY: `dc` は有効で `StartDocW` 済み。
        if let Err(e) = unsafe { render(dc, title, body) } {
            // SAFETY: ジョブ開始後の失敗なので中断して破棄する。
            unsafe { AbortDoc(dc) };
            return Err(e);
        }
        // SAFETY: `dc` は有効で `StartDocW` 済み。
        if unsafe { EndDoc(dc) } <= 0 {
            return Err(FAILED);
        }
        Ok(PrintOutcome::Submitted)
    }
}

#[cfg(target_os = "windows")]
struct WindowsPrint;

#[cfg(target_os = "windows")]
impl fandhe_device::print::Print for WindowsPrint {
    fn print(&self, req: &fandhe_device::print::PrintRequest) -> Result<PrintOutcome, DeviceError> {
        ffi::print_text(req.title(), req.body())
    }
}

/// Windows の `Print` 実装を返す（`device/print.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::print::Print> {
    Box::new(WindowsPrint)
}

/// 非 Windows のテストビルドで dispatch が参照できるよう、非対応の実装を返す。
#[cfg(not(target_os = "windows"))]
pub(crate) fn implementation() -> Box<dyn fandhe_device::print::Print> {
    Box::new(fandhe_device::print::UnsupportedPrint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_table() {
        assert_eq!(after_dialog(1).unwrap(), None);
        assert_eq!(after_dialog(0).unwrap(), Some(PrintOutcome::Cancelled));
        assert_eq!(after_dialog(2).unwrap(), Some(PrintOutcome::Cancelled));
        for a in [3, u32::MAX] {
            assert!(after_dialog(a).is_err());
        }
    }

    #[test]
    fn start_doc_table() {
        assert_eq!(start_doc_failure(1223).unwrap(), PrintOutcome::Cancelled);
        for e in [0, 5, 1222] {
            assert!(start_doc_failure(e).is_err());
        }
    }
}
