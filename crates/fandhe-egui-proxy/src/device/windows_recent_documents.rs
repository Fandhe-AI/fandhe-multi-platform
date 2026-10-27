//! Windows 向け「最近使った項目」の実装（Issue #989、親 #193、骨格は #988）。
//! `device/recent_documents.rs`（effect ドライバ）の windows 経路から UI スレッドで同期に呼ばれ、
//! 契約は `fandhe_device::recent_documents`。
//!
//! # 方式
//!
//! - 登録: `SHAddToRecentDocs(SHARD_PATHW, パス)`。`pv` は必ず非 NULL（NULL は全アプリの履歴を消す）。
//!   パスは `ShellPath` を `windows_open_in_shell::to_win32_path` で正規化したものだけ
//! - 読み戻し: `IApplicationDocumentLists::GetList(ADLT_RECENT, 0)`（0 は全件）。返る集合は
//!   `IShellItem` と `IShellLink` が混在しうるため両方を扱う。`SetAppID` は呼ばない
//!   （本リポジトリは明示 AUMID を持たず、呼ばなければ OS が推定するため）
//! - 登録直後に同じ読み戻しを行い、載っていれば `Listed`。登録 API は戻り値を持たず、成功に畳まない
//!
//! # 結果の写像
//!
//! 全要素のパスを読めて一致があれば `Listed`、無ければ（空を含む）`NotListed`。要素が 1 件でも読めない、
//! または HRESULT が失敗なら `Internal("recent documents os")`（残りの要素だけで結果を決めない）。
//! 比較は `\\?\` を外し大文字小文字を無視する。
//!
//! # 解放と期限
//!
//! COM 参照は windows-rs の Drop が全経路で解放し、`GetDisplayName` の PWSTR は 1 回だけ
//! `CoTaskMemFree` する。STA 初期化は `ComGuard` が対で解放する。同期呼び出しのため期限は設けない。
//!
//! # 守らない範囲
//!
//! 登録後の非同期反映の時間差（再試行・待ちのタイマーは置かず、利用者が「確かめる」で OS を読み直す）、
//! Jump List の表示そのもの、複数プロセス。既知の天井: 関連付けの無い `.txt` が一覧に載るか
//! （常に `NotListed` なら #968 の結果で判断する）。
//! パス・HRESULT・一覧の他の項目（他アプリの履歴を含みうる）はログにもエラー文言にも出さない。

#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
use fandhe_device::recent_documents::RecentOutcome;
#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
use fandhe_device::DeviceError;

/// 比較用の正規化: `\\?\` を外し、大文字小文字を無視する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn normalize(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_lowercase()
}

/// 読み戻した一覧（`None` は要素のパスが取れなかったもの）から結果を決める。
/// 1 件でも `None` があれば、一致の有無や順序にかかわらず `Internal`（`NotListed` に畳まない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn outcome_from_entries(
    target: &str,
    entries: impl IntoIterator<Item = Option<String>>,
) -> Result<RecentOutcome, DeviceError> {
    let want = normalize(target);
    let mut found = false;
    for e in entries {
        let Some(p) = e else {
            return Err(DeviceError::Internal("recent documents os"));
        };
        found |= normalize(&p) == want;
    }
    Ok(if found {
        RecentOutcome::Listed
    } else {
        RecentOutcome::NotListed
    })
}

/// Windows の `RecentDocuments` 実装を返す（`device/recent_documents.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::recent_documents::RecentDocuments> {
    Box::new(win::WinRecentDocuments)
}

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::c_void;

    use fandhe_device::recent_documents::{RecentDocuments, ShellPath};
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::Common::IObjectArray;
    use windows::Win32::UI::Shell::{
        ApplicationDocumentLists, IApplicationDocumentLists, IShellItem, IShellLinkW,
        SHAddToRecentDocs, ADLT_RECENT, SHARD_PATHW, SIGDN_FILESYSPATH,
    };

    use super::super::windows_open_in_shell::to_win32_path;
    use super::*;

    const ERR: DeviceError = DeviceError::Internal("recent documents os");

    /// COM（STA）初期化ガード。成功（`S_OK`/`S_FALSE`）時だけ Drop で解放する。
    /// `windows_open_in_shell.rs` の `ComGuard` と同型。
    struct ComGuard(bool);
    impl ComGuard {
        #[allow(unsafe_code)]
        fn new() -> Self {
            // SAFETY: 呼び出しスレッドへの初期化要求。失敗時はプロセス状態を壊さず、解放もしない。
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

    /// 要素 1 件のファイルパス。`IShellItem` を先に試し、だめなら `IShellLinkW`。読めなければ `None`。
    #[allow(unsafe_code)]
    fn entry_path(list: &IObjectArray, i: u32) -> Option<String> {
        // SAFETY: `list` は有効な COM 参照、`i` は `GetCount` 未満。
        if let Ok(item) = unsafe { list.GetAt::<IShellItem>(i) } {
            // SAFETY: `item` は有効。成功時の PWSTR は呼び出し側の所有で、変換の成否にかかわらず
            // 直後に 1 回だけ `CoTaskMemFree` する。
            let s = unsafe {
                let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
                let s = p.to_string().ok();
                CoTaskMemFree(Some(p.0 as *const c_void));
                s
            };
            return s.filter(|s| !s.is_empty());
        }
        // SAFETY: 同上。
        let link = unsafe { list.GetAt::<IShellLinkW>(i) }.ok()?;
        // 切り詰めによる誤一致を避けるため 32768 要素（Win32 パス長の上限）を確保する。
        let mut buf = vec![0u16; 32768];
        // SAFETY: `buf` は書き込み可能な領域で、OS はその長さを超えて書かない。`pfd` は未使用で null。
        unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0) }.ok()?;
        let n = buf.iter().position(|&c| c == 0)?;
        String::from_utf16(&buf[..n]).ok().filter(|s| !s.is_empty())
    }

    /// OS の「最近」一覧の全要素のパス。取得そのものの失敗は `Internal`。
    #[allow(unsafe_code)]
    fn read_entries() -> Result<Vec<Option<String>>, DeviceError> {
        let _com = ComGuard::new();
        // SAFETY: COM 初期化済みスレッドでの生成。戻りの参照は Drop が解放する。
        let lists: IApplicationDocumentLists =
            unsafe { CoCreateInstance(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| ERR)?;
        // SAFETY: `lists` は有効。0 は全件。
        let arr: IObjectArray = unsafe { lists.GetList(ADLT_RECENT, 0) }.map_err(|_| ERR)?;
        // SAFETY: `arr` は有効。
        let n = unsafe { arr.GetCount() }.map_err(|_| ERR)?;
        Ok((0..n).map(|i| entry_path(&arr, i)).collect())
    }

    fn read_back(path: &ShellPath) -> Result<RecentOutcome, DeviceError> {
        let p = to_win32_path(path.as_str())?;
        outcome_from_entries(&p, read_entries()?)
    }

    pub(super) struct WinRecentDocuments;

    impl RecentDocuments for WinRecentDocuments {
        #[allow(unsafe_code)]
        fn register(&self, path: &ShellPath) -> Result<RecentOutcome, DeviceError> {
            let p = to_win32_path(path.as_str())?;
            let h = HSTRING::from(p.as_str());
            // SAFETY: `h` は呼び出しの間だけ生きる NUL 終端の UTF-16 で、`to_win32_path` が 3 文字以上を
            // 保証するため空でない。`pv` は必ず非 NULL（NULL は全アプリの履歴を消す）。OS は引数を保持しない。
            unsafe { SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(h.as_ptr() as *const c_void)) };
            read_back(path)
        }

        fn check(&self, path: &ShellPath) -> Result<RecentOutcome, DeviceError> {
            read_back(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    #[test]
    fn match_is_listed() {
        let r = outcome_from_entries(r"C:\a\b.txt", [s(r"C:\x.txt"), s(r"C:\a\b.txt")]);
        assert_eq!(r.unwrap(), RecentOutcome::Listed);
    }

    #[test]
    fn mismatch_and_empty_are_not_listed() {
        assert_eq!(
            outcome_from_entries(r"C:\a.txt", [s(r"C:\b.txt")]).unwrap(),
            RecentOutcome::NotListed
        );
        assert_eq!(
            outcome_from_entries(r"C:\a.txt", []).unwrap(),
            RecentOutcome::NotListed
        );
    }

    #[test]
    fn case_and_prefix_are_ignored() {
        let r = outcome_from_entries(r"\\?\C:\Temp\A.TXT", [s(r"c:\temp\a.txt")]);
        assert_eq!(r.unwrap(), RecentOutcome::Listed);
    }

    #[test]
    fn unreadable_entry_is_internal_regardless_of_position() {
        for entries in [
            vec![None, s(r"C:\a.txt")],
            vec![s(r"C:\a.txt"), None],
            vec![None],
        ] {
            assert!(matches!(
                outcome_from_entries(r"C:\a.txt", entries),
                Err(DeviceError::Internal("recent documents os"))
            ));
        }
    }
}
