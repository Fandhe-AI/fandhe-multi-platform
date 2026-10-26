//! Windows 向けファイル・動画のサムネイル（Issue #918。プレビュー表示は対象外）。
//!
//! 呼び出し元: `device/file_preview.rs` の Windows 経路が [`thumbnailer`]・[`present_preview`] を呼ぶ。
//! effect ドライバの専用ワーカースレッド（UI スレッドではない）から同期に呼ばれ、期限・世代・遅着結果の
//! 破棄はドライバ側が持つ。
//!
//! # 流れ
//!
//! 固定ファイルを `temp_dir()/fandhe-file-preview/` へ書き（`fandhe_device::file_preview::write_fixed`）、
//! COM を STA で初期化し、`SHCreateItemFromParsingName` → `IShellItemImageFactory::GetImage`
//! （`SIIGBF_THUMBNAILONLY`。アイコンへ倒れない）で `HBITMAP` を得て、`GetDIBits` で 32bpp BGRA にし
//! `thumbnail_from_windows_bgra` で RGBA 化・再検査する。外から任意のパスは受け取らない。
//!
//! # 契約
//!
//! - HRESULT は `GetImage` / `SHCreateItemFromParsingName` の両方で
//!   `fandhe_device::file_preview::thumbnail_from_windows_hresult` に一本化する。COM 初期化・GDI の失敗は
//!   `Internal(OUTCOME)`（COM 未初期化のまま進めない）
//! - 資源（一時ファイル・COM・`HBITMAP`・HDC）は RAII ガードで全経路で解放する
//! - `GetImage` は取り消せないため `timeout` は使わない。期限超過後の結果はドライバが捨てる
//! - サムネイルが無いときの HRESULT は `WTS_E_FAILEDEXTRACTION` と想定するが実機未確認（#890）。
//!   別の値が返っても失敗表示になるだけで成功には化けない
//! - パス・画素・HRESULT・OS のエラー文はログへ出さない
//!
//! `unsafe` は Win32 FFI 呼び出しに閉じ、各ブロックに `// SAFETY:` を書く。

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fandhe_device::file_preview::{
    thumbnail_from_windows_bgra, thumbnail_from_windows_hresult, write_fixed, FileKind,
    FileThumbnailer, PreviewResult, ThumbnailResult, DIR_NAME, OUTCOME, SIZE, THUMBNAIL_MAX_SIDE,
};
use fandhe_device::DeviceError;
use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE as WinSize;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY,
};

/// `IShellItemImageFactory` による [`FileThumbnailer`]。
pub struct WindowsFileThumbnailer;

/// Windows のサムネイル実装。
pub(crate) fn thumbnailer() -> Arc<dyn FileThumbnailer> {
    Arc::new(WindowsFileThumbnailer)
}

/// Windows のプレビュー提示。対象外なので OS を呼ばず `TargetUnavailable` を返す。
pub(crate) fn present_preview(
    _request_id: u64,
    _kind: FileKind,
    _bytes: Vec<u8>,
) -> Result<Option<PreviewResult>, DeviceError> {
    Ok(Some(PreviewResult::TargetUnavailable))
}

/// 一時ファイルの所有者。Drop で必ず消す。
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// COM（STA）初期化ガード。成功（`S_OK`/`S_FALSE`）時だけ作られ、Drop で解放する。
struct ComGuard;
impl ComGuard {
    #[allow(unsafe_code)]
    fn new() -> Result<Self, DeviceError> {
        // SAFETY: 呼び出しスレッドへの初期化要求。失敗時は解放しない（ガードを作らない）。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr.is_ok() {
            Ok(Self)
        } else {
            Err(DeviceError::Internal(OUTCOME))
        }
    }
}
impl Drop for ComGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `new` で成功した初期化と 1 対 1 に対応する。
        unsafe { CoUninitialize() };
    }
}

/// `GetImage` が返した `HBITMAP` の所有者。Drop で一度だけ `DeleteObject`。
struct BitmapGuard(HBITMAP);
impl Drop for BitmapGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `GetImage` が返した呼び出し側所有のハンドルで、このガードだけが一度だけ解放する。
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}

/// `CreateCompatibleDC` が返した HDC の所有者。
struct DcGuard(HDC);
impl Drop for DcGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `CreateCompatibleDC` が作った HDC で、このガードだけが一度だけ解放する。
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

/// `HBITMAP` を 32bpp BGRA（上から下）で読む。寸法は確保の前に `1..=max_side` で検査する。
#[allow(unsafe_code)]
fn read_bgra(bmp: &BitmapGuard, max_side: u32) -> Result<(u32, u32, Vec<u8>), DeviceError> {
    let mut info = BITMAP::default();
    // SAFETY: `info` は `BITMAP` 1 個分の書き込み先で、サイズを正しく渡す。`bmp.0` は有効なハンドル。
    let got = unsafe {
        GetObjectW(
            HGDIOBJ(bmp.0 .0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut c_void),
        )
    };
    if got == 0 {
        return Err(DeviceError::Internal(OUTCOME));
    }
    let side_ok = |v: i32| v >= 1 && v as u32 <= max_side;
    if !side_ok(info.bmWidth) || !side_ok(info.bmHeight) {
        return Err(DeviceError::Internal(SIZE));
    }
    let (w, h) = (info.bmWidth as u32, info.bmHeight as u32);
    let len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(DeviceError::Internal(SIZE))?;
    let mut buf = vec![0u8; len];
    // SAFETY: 引数 `None` は画面互換の DC を作る。失敗は無効ハンドルで、下で検査する。
    let dc = DcGuard(unsafe { CreateCompatibleDC(None) });
    if dc.0.is_invalid() {
        return Err(DeviceError::Internal(OUTCOME));
    }
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `buf` は `w*h*4` バイトの確保済み領域で、ヘッダの寸法・ビット深度と一致する。`bi` は
    // 呼び出しの間だけ有効な書き込み先。`dc` / `bmp` は有効なハンドルで、`bmp` は他の DC に選択されていない。
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bmp.0,
            0,
            h,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    if lines != h as i32 {
        return Err(DeviceError::Internal(OUTCOME));
    }
    Ok((w, h, buf))
}

impl FileThumbnailer for WindowsFileThumbnailer {
    #[allow(unsafe_code)]
    fn thumbnail(
        &self,
        kind: FileKind,
        bytes: &[u8],
        max_side: u32,
        _timeout: Duration,
    ) -> Result<ThumbnailResult, DeviceError> {
        if bytes.is_empty() || max_side == 0 || max_side > THUMBNAIL_MAX_SIDE {
            return Err(DeviceError::InvalidInput("file_preview input"));
        }
        let file = TempFile(write_fixed(
            &std::env::temp_dir().join(DIR_NAME),
            kind,
            bytes,
        )?);
        let _com = ComGuard::new()?;
        let path = HSTRING::from(file.0.as_os_str());
        // SAFETY: `path` は NUL 終端の有効な UTF-16 文字列で、呼び出しの間だけ読まれる。
        let factory: IShellItemImageFactory =
            match unsafe { SHCreateItemFromParsingName(&path, None) } {
                Ok(f) => f,
                Err(e) => return thumbnail_from_windows_hresult(e.code().0),
            };
        let size = WinSize {
            cx: max_side as i32,
            cy: max_side as i32,
        };
        // SAFETY: `factory` は有効な COM 参照。返る `HBITMAP` は下のガードが解放する。
        let bmp = match unsafe { factory.GetImage(size, SIIGBF_THUMBNAILONLY) } {
            Ok(b) => BitmapGuard(b),
            Err(e) => return thumbnail_from_windows_hresult(e.code().0),
        };
        let (w, h, bgra) = read_bgra(&bmp, max_side)?;
        thumbnail_from_windows_bgra(w, h, bgra)
    }
}
