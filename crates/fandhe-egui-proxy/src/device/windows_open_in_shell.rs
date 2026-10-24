//! Windows 向け「既定アプリで開く・Explorer で表示」の実装（Issue #750、親 #146）。
//! `device/open_in_shell.rs`（effect ドライバ）の windows 経路から UI スレッドで同期に呼ばれ、
//! 契約は `fandhe_device::open_in_shell`（`OpenInShell` / `ShellPath` / `ShellOutcome`）。
//! ゴミ箱（`IFileOperation`）は破壊的なため親 #146 の判断で範囲外。
//!
//! # 方式
//!
//! - 開く: `ShellExecuteExW`（verb `open`、`lpParameters` は NULL、`SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI`）。
//!   パスは 1 つの引数として渡し、コマンドラインは組み立てない。失敗時に OS のダイアログを出さない
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
//! ドライブルート（`X:\` のみ）は要素を持たないため受け付ける（選択表示の対象になる）。途中の空要素（`\\`）と
//! 末尾の `\` は引き続き拒否する。
//!
//! # 型検査（「開く」のみ・fail-closed）
//!
//! Windows では実行ビットの検査が効かないため、OS 層で次を順に確かめる（純関数 [`open_type_check`]）。
//! 1. `GetFileAttributesW` を呼ぶ。失敗・ディレクトリ・再解析ポイントは `InvalidInput("shell type")`
//! 2. 拡張子が無ければ `InvalidInput("shell type")`（型を判定できないものは開かない）
//! 3. [`DENIED_EXTENSIONS`]（固定一覧）に当たれば `InvalidInput("shell type")`
//! 4. `AssocIsDangerous` が真なら `InvalidInput("shell type")`
//!
//! 関連付けの有無は事前に照会せず、OS（`ShellExecuteExW`）の結果で判定する。事前照会は MSIX などの
//! パッケージ版アプリの関連付けを見落とし、OS と判定がずれて TOCTOU も生むため。
//!
//! `reveal` は実行しないので型検査を要さない。
//!
//! # 結果の写像
//!
//! `ShellExecuteExW` が成功すれば `HandedOff`。失敗時は `GetLastError` が `ERROR_NO_ASSOCIATION`、または
//! `hInstApp` が `SE_ERR_NOASSOC`（31）なら `NoHandler`、それ以外は `Internal("shell os")`。
//!
//! `reveal` は PIDL を作れない・HRESULT が失敗のいずれも `Internal("shell os")`（`NoHandler` は返さない）。
//! パス・HRESULT・戻り値はログにもエラーにも出さない。

//! # 期限と守らない範囲
//!
//! 同期呼び出しで保留中の要求を持たないため期限は設けない。守らない範囲: 検査から OS 呼び出しまでの
//! ファイル差し替え（TOCTOU）、アプリ・Explorer が実際に表示したか、文書型を受けたアプリ側の挙動、
//! 複数プロセス。成功は OS が受理したことだけを表す。関連付けの無い型は
//! `ShellExecuteExW` の失敗（`ERROR_NO_ASSOCIATION` / `SE_ERR_NOASSOC`）で `NoHandler` にする。
//! `SEE_MASK_FLAG_NO_UI` 下で関連付けなしの「アプリを選ぶ」ダイアログが出ないことは実機で未確認（確認用 issue #744）。
//! 出た場合は表示が一時的に食い違うだけで、守備範囲の外として扱う。
//!
//! モジュールは純関数をホストでテストできるよう `cfg(any(windows, test))` でコンパイルする（`device.rs`）。

#[cfg(not(target_os = "windows"))]
use fandhe_device::open_in_shell::UnsupportedOpenInShell;
use fandhe_device::open_in_shell::{OpenInShell, ShellOutcome};
use fandhe_device::DeviceError;

/// `ShellExecuteExW` の `hInstApp` に入る `SE_ERR_NOASSOC`（関連付けが無い）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const SE_ERR_NOASSOC: isize = 31;

/// `ERROR_NO_ASSOCIATION`（`GetLastError`）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const ERROR_NO_ASSOCIATION: u32 = 1155;

/// `FILE_ATTRIBUTE_DIRECTORY`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const ATTR_DIRECTORY: u32 = 0x10;

/// `FILE_ATTRIBUTE_REPARSE_POINT`（シンボリックリンク・ジャンクション等）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const ATTR_REPARSE_POINT: u32 = 0x400;

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
    "dll",
    "sys",
    "ocx",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
    "sct",
    "shs",
    "scf",
    "rdp",
    "diagcab",
    "ps1xml",
    "psc1",
    "xll",
    "vb",
    "ws",
    "mde",
    "ade",
    "mdb",
    "library-ms",
    "searchconnector-ms",
    "website",
    "theme",
    "themepack",
    "iso",
    "vhd",
    "vhdx",
    "img",
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
    // ドライブルート（`X:\` のみ）は要素が無いので検査対象外（選択表示で受け付けるため）。
    let rest = &p[3..];
    for comp in rest.split('\\').filter(|_| !rest.is_empty()) {
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

/// 固定の拒否一覧に当たるか。拡張子が無ければ `false`（拡張子なしの拒否は [`open_type_check`] が行う）。
#[cfg(test)]
fn is_denied_extension(path: &str) -> bool {
    extension_of(path).is_some_and(|e| DENIED_EXTENSIONS.contains(&e.as_str()))
}

/// 「開く」の型検査の結果。
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum TypeCheck {
    /// OS へ渡してよい。
    Pass,
    /// 型を拒否する（`InvalidInput("shell type")`）。
    Reject,
}

/// 「開く」の型検査（SSOT、fail-closed）。`attrs` は `GetFileAttributesW` の結果（失敗は `None`）、
/// `ext` は小文字の拡張子、`dangerous` は `AssocIsDangerous`。
/// 通常ファイルでない・型を判定できないものは `Reject`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn open_type_check(attrs: Option<u32>, ext: Option<&str>, dangerous: bool) -> TypeCheck {
    match attrs {
        Some(a) if a & (ATTR_DIRECTORY | ATTR_REPARSE_POINT) == 0 => {}
        _ => return TypeCheck::Reject,
    }
    let Some(ext) = ext else {
        return TypeCheck::Reject;
    };
    if DENIED_EXTENSIONS.contains(&ext) || dangerous {
        return TypeCheck::Reject;
    }
    TypeCheck::Pass
}

/// `ShellExecuteExW` 失敗時の写像。`last_error` は `GetLastError`、`inst_app` は `hInstApp` の数値。
/// 関連付け無しだけ `NoHandler`、未知の失敗は `Internal`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn exec_failure_outcome(last_error: u32, inst_app: isize) -> Result<ShellOutcome, DeviceError> {
    if last_error == ERROR_NO_ASSOCIATION || inst_app == SE_ERR_NOASSOC {
        Ok(ShellOutcome::NoHandler)
    } else {
        Err(DeviceError::Internal("shell os"))
    }
}

/// 選択表示の段階ごとの結果。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
enum RevealStep {
    /// PIDL を作れなかった。
    NoPidl,
    /// `SHOpenFolderAndSelectItems` が失敗した。
    Failed,
    /// 受理された。
    Done,
}

/// 選択表示の結果。PIDL を作れない場合も HRESULT 失敗も `Internal`。`NoHandler` は返さない。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn reveal_outcome(step: RevealStep) -> Result<ShellOutcome, DeviceError> {
    match step {
        RevealStep::Done => Ok(ShellOutcome::HandedOff),
        RevealStep::NoPidl | RevealStep::Failed => Err(DeviceError::Internal("shell os")),
    }
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        AssocIsDangerous, ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteExW,
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
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

    /// `GetFileAttributesW`。失敗（存在しない・権限なし等）は `None`。
    #[allow(unsafe_code)]
    fn file_attributes(path: &HSTRING) -> Option<u32> {
        // SAFETY: `path` は NUL 終端の UTF-16 で呼び出しの間だけ生きる。OS は引数を保持しない。
        let a = unsafe { GetFileAttributesW(path) };
        (a != INVALID_FILE_ATTRIBUTES).then_some(a)
    }

    #[allow(unsafe_code)]
    pub(super) fn open(canonical: &str) -> Result<ShellOutcome, DeviceError> {
        let path = to_win32_path(canonical)?;
        let file = HSTRING::from(path.as_str());
        let attrs = file_attributes(&file);
        let ext = extension_of(&path);
        let dangerous = match ext.as_deref() {
            Some(e) => {
                let dotted = HSTRING::from(format!(".{e}"));
                // SAFETY: `dotted` は NUL 終端の UTF-16 で呼び出しの間だけ生きる。OS は引数を保持しない。
                unsafe { AssocIsDangerous(&dotted) }.as_bool()
            }
            None => false,
        };
        match open_type_check(attrs, ext.as_deref(), dangerous) {
            TypeCheck::Pass => {}
            TypeCheck::Reject => return Err(DeviceError::InvalidInput("shell type")),
        }
        let _com = ComGuard::new();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR::null(),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: `info` は `cbSize` を設定した初期化済み構造体。`lpFile` は `file`（NUL 終端の UTF-16）、
        // `lpVerb` は静的な NUL 終端文字列で、いずれも呼び出しの間だけ生きる。UI スレッドから同期に呼ぶ。
        match unsafe { ShellExecuteExW(&mut info) } {
            Ok(()) => Ok(ShellOutcome::HandedOff),
            Err(e) => {
                // `windows::core::Error` は Win32 エラーを `0x8007xxxx` の HRESULT で持つ。
                let hr = e.code().0 as u32;
                let last = if hr >> 16 == 0x8007 { hr & 0xFFFF } else { 0 };
                // `hInstApp` は失敗時に `SE_ERR_*` の数値コードが入る（ポインタとしては読まない）。
                exec_failure_outcome(last, info.hInstApp.0 as isize)
            }
        }
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
            return reveal_outcome(RevealStep::NoPidl);
        }
        // SAFETY: `pidl.0` は有効な完全 PIDL。cidl = 0 のとき親フォルダーを開いてその項目を選択する。
        // UI スレッドから同期に呼ぶ。
        let r = unsafe { SHOpenFolderAndSelectItems(pidl.0, None, 0) };
        reveal_outcome(if r.is_ok() {
            RevealStep::Done
        } else {
            RevealStep::Failed
        })
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
        // ドライブルートは受け付ける（途中の空要素の拒否と区別する）。
        assert_eq!(to_win32_path(r"\\?\C:\").unwrap(), r"C:\");
        assert_eq!(to_win32_path(r"C:\").unwrap(), r"C:\");
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
            r"C:\a\",
            r"C:\\",
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
    fn exec_failure_mapping() {
        assert!(matches!(
            exec_failure_outcome(ERROR_NO_ASSOCIATION, 0),
            Ok(ShellOutcome::NoHandler)
        ));
        assert!(matches!(
            exec_failure_outcome(0, 31),
            Ok(ShellOutcome::NoHandler)
        ));
        for (e, i) in [(0, 0), (2, 2), (5, 5), (31, 0), (0, 32), (0, -1)] {
            assert!(matches!(
                exec_failure_outcome(e, i),
                Err(DeviceError::Internal("shell os"))
            ));
        }
    }

    #[test]
    fn reveal_mapping() {
        assert!(matches!(
            reveal_outcome(RevealStep::Done),
            Ok(ShellOutcome::HandedOff)
        ));
        for s in [RevealStep::NoPidl, RevealStep::Failed] {
            assert!(matches!(
                reveal_outcome(s),
                Err(DeviceError::Internal("shell os"))
            ));
        }
    }

    const FILE: Option<u32> = Some(0x20);

    #[test]
    fn type_check_rejects_non_regular_and_unknown() {
        // ディレクトリ・再解析ポイント・属性を読めない・拡張子なし・固定一覧・危険。
        assert_eq!(
            open_type_check(Some(ATTR_DIRECTORY), Some("txt"), false),
            TypeCheck::Reject
        );
        assert_eq!(
            open_type_check(Some(0x20 | ATTR_REPARSE_POINT), Some("txt"), false),
            TypeCheck::Reject
        );
        assert_eq!(open_type_check(None, Some("txt"), false), TypeCheck::Reject);
        assert_eq!(open_type_check(FILE, None, false), TypeCheck::Reject);
        assert_eq!(open_type_check(FILE, Some("iso"), false), TypeCheck::Reject);
        assert_eq!(open_type_check(FILE, Some("txt"), true), TypeCheck::Reject);
    }

    #[test]
    fn type_check_pass() {
        assert_eq!(open_type_check(FILE, Some("txt"), false), TypeCheck::Pass);
    }
}
