//! Windows 向けアプリショートカット実装（Issue #428、親 #101）。タスクバーのジャンプリストに
//! 「デモを開く」タスクを 1 件登録する。`device/app_shortcuts.rs::platform` のワーカースレッド
//! （`ON_WORKER`）から呼ばれ、契約は `fandhe_device::app_shortcuts`。
//!
//! タスクは自分の exe を引数 `"fandhe://shortcut/open-demo"`（引用符付き）で起動する。受信は新設せず、
//! `windows_url_scheme` の転送経路（起動済みなら pipe 転送、未起動ならコールドスタート）に乗る。
//! 引用符は `windows_url_scheme::launch_unaltered` の期待形 `<exe> "<arg>"` に合わせる
//! （無いと拒否として数えられる）。
//!
//! 一次情報: <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-icustomdestinationlist>
//! （`AddUserTasks`）、AUMID は <https://learn.microsoft.com/en-us/windows/win32/shell/appids>。
//! AUMID は明示しない（`SetAppID` を呼ばない）。
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス 1 回の登録（`InstallCell`）、COM の失敗を成功にしないこと、途中失敗時の
//! `AbortList` による後始末、HRESULT・パス・URL をエラー文言へ出さないこと。
//! 守らない: 画面を一度開くまで項目が出ない、終了後もタスクが残る、exe を動かすと古いパスのまま残り
//! 次に画面を開くと置き換わる、他アプリから同じ URL が来た場合と区別しない、`windows_notification` が
//! 途中で明示の AUMID を付けたプロセスではジャンプリストが出なくなることがある（#413 で確かめる）、
//! ポーリング間隔ぶんの反映遅れ。
//!
//! 【推測】MTA（`ComInit`）で `DestinationList` が動くこと。文書に制約の記載はなく実機で確かめる。
//! 失敗しても `Failed` になるだけで、嘘の成功にはならない。
//!
//! `unsafe` は `cfg(windows)` の COM 呼び出しに限り、各所に `// SAFETY:` を付ける。COM と PROPVARIANT の
//! 解放は `windows` クレートの `Drop` に任せる。

/// タスクの起動引数。`windows_url_scheme::launch_unaltered` の期待形 `"<url>"` に合わせて引用符で囲む。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn task_arguments(url: &str) -> String {
    format!("\"{url}\"")
}

#[cfg(target_os = "windows")]
pub(crate) use win::implementation;

#[cfg(target_os = "windows")]
mod win {
    use fandhe_core::screens::url_scheme_show::{SHORTCUT_TITLE, SHORTCUT_URL};
    use fandhe_device::app_shortcuts::{AppShortcuts, Registration};
    use fandhe_device::DeviceError;
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::Storage::EnhancedStorage::PKEY_Title;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{
        DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink,
    };

    use super::super::windows_notification::{lpwstr_variant, ComInit};

    const FAILED: DeviceError = DeviceError::Internal("app_shortcuts jump list");

    struct WindowsAppShortcuts;

    /// Windows の実装を返す。`device/app_shortcuts.rs` の windows 経路から呼ばれる。
    pub(crate) fn implementation() -> &'static dyn AppShortcuts {
        &WindowsAppShortcuts
    }

    impl AppShortcuts for WindowsAppShortcuts {
        #[allow(unsafe_code)]
        fn install(&self) -> Result<Registration, DeviceError> {
            // COM 初期化はこのガードが生存する間だけ有効（COM オブジェクトより先に宣言し最後に解放）。
            let _com = ComInit::new("app_shortcuts com init")?;
            let exe =
                std::env::current_exe().map_err(|_| DeviceError::Internal("app_shortcuts"))?;
            // SAFETY: `DestinationList`/`CLSCTX_INPROC_SERVER` は定数。COM 初期化済みスレッド上で呼ぶ。
            let list: ICustomDestinationList =
                unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|_| FAILED)?;
            let mut slots = 0u32;
            // SAFETY: `list` は有効な COM ポインタ、`slots` は書き込み可能な u32。SetAppID は呼ばない。
            let _removed: IObjectArray =
                unsafe { list.BeginList(&mut slots) }.map_err(|_| FAILED)?;

            let build = || -> windows::core::Result<()> {
                // SAFETY: 以下の COM 呼び出しはすべて、直前に取得した有効なポインタと
                // `current_exe()` 由来の絶対パス・定数文字列に対するもの。
                unsafe {
                    let collection: IObjectCollection =
                        CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
                    let link: IShellLinkW =
                        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    let exe_h = HSTRING::from(exe.as_os_str());
                    link.SetPath(&exe_h)?;
                    link.SetArguments(&HSTRING::from(super::task_arguments(SHORTCUT_URL)))?;
                    link.SetIconLocation(&exe_h, 0)?;
                    let store: IPropertyStore = link.cast()?;
                    let title = lpwstr_variant(SHORTCUT_TITLE, "app_shortcuts jump list")
                        .map_err(|_| windows::core::Error::from(E_FAIL))?;
                    store.SetValue(&PKEY_Title, &title)?;
                    store.Commit()?;
                    collection.AddObject(&link)?;
                    list.AddUserTasks(&collection.cast::<IObjectArray>()?)?;
                    list.CommitList()
                }
            };
            match build() {
                Ok(()) => Ok(Registration::Installed),
                Err(_) => {
                    // SAFETY: `list` は BeginList 済みの有効な COM ポインタ。失敗は無視する（後始末）。
                    let _ = unsafe { list.AbortList() };
                    Err(FAILED)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::windows_url_scheme::{launch_unaltered, parse_url_args};
    use fandhe_core::screens::url_scheme_show::SHORTCUT_URL;

    #[test]
    fn task_command_line_is_accepted_by_url_scheme() {
        let raw = format!("\"C:\\app\\x.exe\" {}", task_arguments(SHORTCUT_URL));
        assert!(launch_unaltered(&raw, SHORTCUT_URL));
        // 引用符を外す退行は拒否される。
        let bare = format!("C:\\app\\x.exe {SHORTCUT_URL}");
        assert!(!launch_unaltered(&bare, SHORTCUT_URL));
    }

    #[test]
    fn task_argument_is_a_url_launch() {
        assert_eq!(
            parse_url_args(&[SHORTCUT_URL.into()]),
            Some(Some(SHORTCUT_URL.to_owned()))
        );
    }
}
