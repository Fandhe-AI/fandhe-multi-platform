//! Windows 向け「既定アプリで開く・Explorer で表示」の実装（Issue #750、親 #146）。
//! `device/open_in_shell.rs`（effect ドライバ）の windows 経路から UI スレッドで同期に呼ばれ、
//! 契約は `fandhe_device::open_in_shell`（`OpenInShell` / `ShellPath` / `ShellOutcome`）。
//! ゴミ箱（`IFileOperation`）は破壊的なため親 #146 の判断で範囲外。
//!
//! # 方式
//!
//! - 開く: `ShellExecuteW`（verb `open`、`lpParameters` は NULL）。パスは 1 つの引数として渡し、
//!   コマンドラインは組み立てない
//! - 選択表示: `ILCreateFromPathW` + `SHOpenFolderAndSelectItems`（cidl = 0）。PIDL は RAII で
//!   全経路で `ILFree` する
//! - COM は STA で初期化する（`windows_launch_at_login.rs` の `ComGuard` と同型。可視性を変えずここへ複製）
//!
//! # パスの正規化（[`to_win32_path`]）
//!
//! `ShellPath` は `canonicalize` 済みで Windows では `\\?\C:\...` 形式になる。`\\?\` を外して
//! `X:\...` で OS に渡す。次は `InvalidInput("shell path")` で拒否する: UNC（`\\?\UNC\` 含む）、
//! デバイスパス（`\\.\`）、`\\?\Volume{` 等その他の `\\?\`、ドライブ文字なし、`/` 混在、
//! 代替データストリーム（ドライブの `:` 以外の `:`）、要素末尾の `.` と空白、UTF-16 で 260 文字以上。
//!
//! # 型検査（「開く」のみ）
//!
//! Windows では実行ビットの検査が効かないため 2 段で fail-closed にする。どちらも
//! `InvalidInput("shell type")`。
//! 1. [`is_denied_extension`]: 実行可能・スクリプト・ショートカット・インストーラー等の固定一覧
//! 2. `AssocIsDangerous`: OS が危険と判定する型
//!
//! `reveal` は実行しないので型検査を要さない。
//!
//! # 結果の写像
//!
//! | `ShellExecuteW` の戻り値 | 結果 |
//! |---|---|
//! | `> 32` | `HandedOff` |
//! | `31`（`SE_ERR_NOASSOC`） | `NoHandler` |
//! | その他 | `Internal("shell os")` |
//!
//! `reveal` は PIDL を作れない・HRESULT が失敗のいずれも `Internal("shell os")`（`NoHandler` は返さない）。
//! パス・HRESULT・戻り値はログにもエラーにも出さない。
//!
//! # 期限と守らない範囲
//!
//! 同期呼び出しで保留中の要求を持たないため期限は設けない。守らない範囲: 検査から OS 呼び出しまでの
//! ファイル差し替え（TOCTOU）、アプリ・Explorer が実際に表示したか、文書型を受けたアプリ側の挙動、
//! 複数プロセス。成功は OS が受理したことだけを表す。関連付けの無い型で「アプリを選ぶ」画面が出て
//! `> 32` が返るか `NoHandler` になるかは実機で確かめる（#744）。
//!
//! モジュールは純関数をホストでテストできるよう `cfg(any(windows, test))` でコンパイルする（`device.rs`）。

#[cfg(not(target_os = "windows"))]
use fandhe_device::open_in_shell::UnsupportedOpenInShell;
use fandhe_device::open_in_shell::{OpenInShell, ShellOutcome};
use fandhe_device::DeviceError;

/// `ShellExecuteW` の `SE_ERR_NOASSOC`（関連付けが無い）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const SE_ERR_NOASSOC: isize = 31;

/// `MAX_PATH`（終端込み）。パスはこれ未満の UTF-16 長だけ通す。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const MAX_PATH: usize = 260;

/// 「開く」を無条件で拒否する拡張子（小文字）。`AssocIsDangerous` が判定しない型も止める。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const DENIED_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "msi",
    "msp",
    "scr",
    "pif",
    "cpl",
    "lnk",
    "url",
    "jar",
    "reg",
    "msc",
    "appref-ms",
    "application",
    "gadget",
    "inf",
    "chm",
    "settingcontent-ms",
];

/// `ShellPath::as_str()`（`\\?\X:\...` または `X:\...`）を OS に渡す `X:\...` へ正規化する。
/// 規則は本モジュール doc の「パスの正規化」。不適合は `InvalidInput("shell path")`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn to_win32_path(canonical: &str) -> Result<String, DeviceError> {
    const ERR: DeviceError = DeviceError::InvalidInput("shell path");
    let p = canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
    let b = p.as_bytes();
    // `X:\` 形式のみ許可（UNC・デバイス・Volume{} はここで弾かれる）。
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return Err(ERR);
    }
    if p.contains('/') || p.contains('\0') {
        return Err(ERR);
    }
    // ADS: ドライブの `:` より後ろの `:`。
    if p[2..].contains(':') {
        return Err(ERR);
    }
    // 要素末尾の `.` と空白（Win32 が落として別名に解決するのを避ける）。空要素（`\\`）も拒否。
    for comp in p[3..].split('\\') {
        if comp.is_empty() || comp.ends_with('.') || comp.ends_with(' ') {
            return Err(ERR);
        }
    }
    if p.encode_utf16().count() >= MAX_PATH {
        return Err(ERR);
    }
    Ok(p.to_string())
}

/// 最後の要素の拡張子（`.` なし・小文字）。無ければ `None`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extension_of(path: &str) -> Option<String> {
    let name = path.rsplit('\\').next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// 固定の拒否一覧に当たるか。拡張子が無ければ `false`（関連付けが無く `NoHandler` に倒れる）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_denied_extension(path: &str) -> bool {
    extension_of(path).is_some_and(|e| DENIED_EXTENSIONS.contains(&e.as_str()))
}

/// `ShellExecuteW` の戻り値 → 結果の写像（SSOT）。未知の値は失敗に倒し、成功を作らない。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn shell_execute_outcome(code: isize) -> Result<ShellOutcome, DeviceError> {
    match code {
        c if c > 32 => Ok(ShellOutcome::HandedOff),
        SE_ERR_NOASSOC => Ok(ShellOutcome::NoHandler),
        _ => Err(DeviceError::Internal("shell os")),
    }
}

/// 選択表示の結果。PIDL を作れない場合も HRESULT 失敗も `Internal`。`NoHandler` は返さない。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn reveal_outcome(pidl_null: bool, hr_ok: bool) -> Result<ShellOutcome, DeviceError> {
    if pidl_null || !hr_ok {
        Err(DeviceError::Internal("shell os"))
    } else {
        Ok(ShellOutcome::HandedOff)
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        AssocIsDangerous, ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// COM（STA）初期化ガード。成功（`S_OK`/`S_FALSE`）時だけ Drop で解放する。
    /// `windows_launch_at_login.rs` の `ComGuard` と同型。
    struct ComGuard(bool);
    impl ComGuard {
        #[allow(unsafe_code)]
        fn new() -> Self {
            // SAFETY: 呼び出しスレッドへの初期化要求。失敗（`RPC_E_CHANGED_MODE` 等）でも
            // プロセス状態を壊さず、その場合は解放しない。
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            Self(hr.is_ok())
        }
    }
    impl Drop for ComGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: `new` で成功した初期化と 1 対 1 に対応する。
                unsafe { CoUninitialize() };
            }
        }
    }

    /// `ILCreateFromPathW` が返した PIDL の所有者。Drop で一度だけ `ILFree` する（null は解放しない）。
    struct PidlGuard(*mut ITEMIDLIST);
    impl Drop for PidlGuard {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `ILCreateFromPathW` が確保した PIDL で、このガードだけが所有し一度だけ解放する。
                unsafe { ILFree(Some(self.0)) };
            }
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn open(canonical: &str) -> Result<ShellOutcome, DeviceError> {
        let path = to_win32_path(canonical)?;
        if is_denied_extension(&path) {
            return Err(DeviceError::InvalidInput("shell type"));
        }
        if let Some(ext) = extension_of(&path) {
            let dotted = HSTRING::from(format!(".{ext}"));
            // SAFETY: `dotted` は NUL 終端の UTF-16 で呼び出しの間だけ生きる。OS は引数を保持しない。
            if unsafe { AssocIsDangerous(&dotted) }.as_bool() {
                return Err(DeviceError::InvalidInput("shell type"));
            }
        }
        let _com = ComGuard::new();
        let file = HSTRING::from(path);
        // SAFETY: `file` は NUL 終端の UTF-16 で呼び出しの間だけ生きる。`w!("open")` は静的な
        // NUL 終端文字列。ShellExecuteW は引数を保持せず、UI スレッドから同期に呼ぶ。
        let h = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &file,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        shell_execute_outcome(h.0 as isize)
    }

    #[allow(unsafe_code)]
    pub(super) fn reveal(canonical: &str) -> Result<ShellOutcome, DeviceError> {
        let path = to_win32_path(canonical)?;
        let _com = ComGuard::new();
        let wide = HSTRING::from(path);
        // SAFETY: `wide` は NUL 終端の UTF-16 で呼び出しの間だけ生きる。返った PIDL は Guard が
        // 全経路で一度だけ解放する。
        let pidl = PidlGuard(unsafe { ILCreateFromPathW(&wide) });
        if pidl.0.is_null() {
            return reveal_outcome(true, false);
        }
        // SAFETY: `pidl.0` は有効な完全 PIDL。cidl = 0 のとき親フォルダーを開いてその項目を選択する。
        // UI スレッドから同期に呼ぶ。
        let r = unsafe { SHOpenFolderAndSelectItems(pidl.0, None, 0) };
        reveal_outcome(false, r.is_ok())
    }
}

#[cfg(target_os = "windows")]
struct WindowsOpenInShell;

#[cfg(target_os = "windows")]
impl OpenInShell for WindowsOpenInShell {
    fn open(
        &self,
        path: &fandhe_device::open_in_shell::ShellPath,
    ) -> Result<ShellOutcome, DeviceError> {
        win::open(path.as_str())
    }

    fn reveal(
        &self,
        path: &fandhe_device::open_in_shell::ShellPath,
    ) -> Result<ShellOutcome, DeviceError> {
        win::reveal(path.as_str())
    }
}

/// Windows の `OpenInShell` 実装を返す。非 Windows（ホストテスト時）は `Unsupported` 固定。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Box<dyn OpenInShell> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsOpenInShell)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Box::new(UnsupportedOpenInShell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_path_err(r: Result<String, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::InvalidInput("shell path")))
    }

    #[test]
    fn path_accepts_normal_forms() {
        assert_eq!(to_win32_path(r"\\?\C:\a\b.txt").unwrap(), r"C:\a\b.txt");
        assert_eq!(to_win32_path(r"C:\a\b.txt").unwrap(), r"C:\a\b.txt");
        assert_eq!(
            to_win32_path(r"\\?\D:\ユーザー 名\メモ 1.txt").unwrap(),
            r"D:\ユーザー 名\メモ 1.txt"
        );
    }

    #[test]
    fn path_rejects_unsafe_forms() {
        for p in [
            r"\\?\UNC\srv\s\a.txt",
            r"\\server\share\a.txt",
            r"\\.\COM1",
            r"\\?\Volume{1234}\a",
            r"C:\a\b.txt:ads",
            r"C:\a\b.exe.",
            r"C:\a\b.txt ",
            r"C:\a. \b.txt",
            "C:/a/b.txt",
            r"C:\a\\b.txt",
            "relative",
            "",
            r"C:",
            r"1:\a",
        ] {
            assert!(is_path_err(to_win32_path(p)), "{p}");
        }
    }

    #[test]
    fn path_length_boundary() {
        let ok = format!(r"C:\{}", "a".repeat(MAX_PATH - 1 - 3));
        assert_eq!(ok.encode_utf16().count(), MAX_PATH - 1);
        assert!(to_win32_path(&ok).is_ok());
        let ng = format!("{ok}a");
        assert!(is_path_err(to_win32_path(&ng)));
    }

    #[test]
    fn denied_extensions() {
        for e in DENIED_EXTENSIONS {
            assert!(is_denied_extension(&format!(r"C:\a\f.{e}")), "{e}");
            assert!(is_denied_extension(&format!(
                r"C:\a\f.{}",
                e.to_uppercase()
            )));
        }
        assert!(!is_denied_extension(r"C:\a\f.txt"));
        assert!(!is_denied_extension(r"C:\a\noext"));
        assert!(!is_denied_extension(r"C:\a.exe\noext"));
    }

    #[test]
    fn shell_execute_mapping() {
        assert!(matches!(
            shell_execute_outcome(33),
            Ok(ShellOutcome::HandedOff)
        ));
        assert!(matches!(
            shell_execute_outcome(42),
            Ok(ShellOutcome::HandedOff)
        ));
        assert!(matches!(
            shell_execute_outcome(31),
            Ok(ShellOutcome::NoHandler)
        ));
        for c in [0, 2, 3, 5, 32, -1, isize::MIN] {
            assert!(
                matches!(
                    shell_execute_outcome(c),
                    Err(DeviceError::Internal("shell os"))
                ),
                "{c}"
            );
        }
        assert!(shell_execute_outcome(isize::MAX).is_ok());
    }

    #[test]
    fn reveal_mapping() {
        assert!(matches!(
            reveal_outcome(false, true),
            Ok(ShellOutcome::HandedOff)
        ));
        for (n, ok) in [(true, true), (true, false), (false, false)] {
            assert!(matches!(
                reveal_outcome(n, ok),
                Err(DeviceError::Internal("shell os"))
            ));
        }
    }
}
