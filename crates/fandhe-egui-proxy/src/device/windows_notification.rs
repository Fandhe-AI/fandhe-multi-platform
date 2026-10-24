//! Windows 向けローカル通知実装（Issue #1004、AUMID 登録は Issue #1033）。
//! `device/notification.rs` の Windows 経路から呼ばれ、
//! `fandhe_device::notification::Notifier` を WinRT `Windows.UI.Notifications`
//! （Toast）で実装する。
//!
//! # AUMID（Application User Model ID）の登録方式
//!
//! Toast の送り手（[`ToastNotificationManager::CreateToastNotifierWithId`]）には
//! 登録済みの AUMID が要る。以前の実装は本アプリ固有・未登録の AUMID で試みるだけで、
//! 未パッケージの実行形態では通常失敗していた（`Unsupported` を返して止まる）。
//! さらに以前は PowerShell の既知 AUMID を借用していたが、それでは**全通知が
//! Windows PowerShell 名義で表示され**、利用者がどのアプリからの通知か区別
//! できない（構造的な通知元詐称、codex 指摘 P1、PR #1008）。他アプリの identity
//! を借りることは `device-bridge-contracts.md`「6. 嘘の成功」の精神（失敗を
//! 成功へ畳まない）に反する誤表示でもあるため、どちらの案も採らない。
//!
//! 本実装（10-01 のユーザー判断）はユーザー単位のスタートメニュー
//! （`%APPDATA%\Microsoft\Windows\Start Menu\Programs\`）に `.lnk` ショートカットを
//! 置き、`System.AppUserModel.ID`（[`PKEY_AppUserModel_ID`]）へ [`APP_AUMID`] を
//! 設定することで、管理者権限・レジストリ書き込み・他アプリの identity 借用の
//! いずれも無しに AUMID を登録する（[`ensure_shortcut`]）。初回の通知要求で
//! ショートカットが無ければ作る。既にあれば AUMID とリンク先を確かめ、自分のものなら
//! 再利用（リンク先が古ければ作り直し）、他のものなら触らず失敗を返す
//! （[`inspect_shortcut`] 参照）。
//!
//! ## 既知の天井
//!
//! - 置き場所は `%APPDATA%` 直下決め打ち（`SHGetKnownFolderPath` は使わない）。
//!   Start Menu の Programs フォルダを既定以外へ移している環境には追従しない。
//!   ponytail: 必要になったら known folder 解決へ切り替える
//! - 登録直後は Windows 側のインデックス反映が間に合わずトーストが実際には
//!   出ないことがある。この状態も区別できないため [`Notifier::notify`] は
//!   常に `Submitted` を返す（`Shown` は返さない、下記参照）
//!
//! `unsafe` は COM・Win32 呼び出し（[`ComInit`]、[`ensure_shortcut`]、
//! [`show_toast`] 内の `SetCurrentProcessExplicitAppUserModelID`）に限り、
//! 各呼び出しの直前に `// SAFETY:` を付けて最小範囲の関数に閉じる
//! （`lib.rs` の Windows `deny(unsafe_code)` 緩和・coding-rust.md の FFI 例外規定準拠。
//! `windows_share.rs`/`windows_biometric.rs` と同型）。PROPVARIANT の解放・COM の
//! 参照カウント解放は `windows` クレートの `Drop` に任せ、手動では行わない。
//!
//! Toast が実際に画面へ出たかどうかは確かめられない（`Failed` イベントは購読
//! しない、YAGNI）ため [`fandhe_device::notification::NotifyOutcome::Shown`] は
//! 返さず、常に `Submitted` を返す。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use fandhe_device::notification::{NotificationRequest, Notifier, NotifyOutcome};
use fandhe_device::DeviceError;
use windows::core::{Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVE_FILE_FLAGS,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantChangeType, PropVariantToBSTR, PROPVARIANT, PROPVAR_CHANGE_FLAGS,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink};
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager, ToastTemplateType,
};

/// 本アプリ固有の AUMID。他アプリ（PowerShell 等）の識別子は借用しない
/// （モジュール doc 参照）。`windows_secure_credential.rs::RESOURCE` と同じ識別子を
/// 流用し、Windows 側の資源識別子を 1 か所に揃える。
pub(super) const APP_AUMID: &str = "com.fandhe.egui-proxy-desktop";

/// スタートメニューに置くショートカットのファイル名。表示名はトーストの送り元名
/// としても使われる。
const SHORTCUT_FILE: &str = "Fandhe egui proxy.lnk";

/// `ToastNotifier::Setting()` の値を契約上のエラー種別へ写す純関数。`cfg` を
/// 付けず（`windows_secure_credential.rs::is_not_found` と異なりホスト非依存の
/// 整数比較のみのため）どのホストでもテストできるようにする。
/// `NotificationSetting` の discriminant は WinRT の固定値
/// （0=Enabled, 1=DisabledForApplication, 2=DisabledForUser,
/// 3=DisabledByGroupPolicy, 4=DisabledByManifest）。
fn setting_result(code: i32) -> Result<(), DeviceError> {
    setting_result_for(code, "notification")
}

/// [`setting_result`] を機能ラベル付きに一般化したもの。同じ送り手（AUMID）の
/// トーストを使う `windows_alarm.rs`（Issue #721）が、自機能のラベルで同じ写像を
/// 共用する。未知の値の `Internal` ラベルは固定文字列にするため、`notification`
/// 経路の文言は変わらない。
pub(super) fn setting_result_for(code: i32, feature: &'static str) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::PermissionDenied(feature)),
        2 | 3 => Err(DeviceError::Disabled(feature)),
        4 => Err(DeviceError::Unsupported(feature)),
        _ => Err(DeviceError::Internal(if feature == "notification" {
            "notification setting"
        } else {
            "toast setting"
        })),
    }
}

/// `%APPDATA%`（`appdata`）配下のユーザー単位スタートメニュー Programs フォルダに
/// 置くショートカットの絶対パスを組み立てる純関数（モジュール doc の「既知の天井」
/// 参照。`SHGetKnownFolderPath` は使わない）。
fn shortcut_path(appdata: &OsStr) -> PathBuf {
    let mut path = PathBuf::from(appdata);
    path.push("Microsoft");
    path.push("Windows");
    path.push("Start Menu");
    path.push("Programs");
    path.push(SHORTCUT_FILE);
    path
}

/// COM 初期化の RAII ガード。`notify` は `device/notification.rs::bridge` が都度
/// 立てる専用スレッドで動く（他スレッドと COM 初期化状態を共有しない）ため、
/// 呼び出しのたびに初期化し、関数末尾で対にして解放する。`CoInitializeEx` が
/// 失敗した場合は `Err` を返して呼び出し元で停止する（後続の COM 操作を
/// 未初期化のまま続けない。codex 指摘 P1、PR #1035）。
///
/// Issue #1044: `windows_background_task.rs` も同じ RAII を再利用する
/// （COM 初期化ガードを機能ごとに複製しない）。`new` の `feature` は失敗時の
/// `DeviceError::Internal` のラベルに使う（呼び出し元ごとに文言を変える）。
pub(super) struct ComInit {
    should_uninit: bool,
}

impl ComInit {
    #[allow(unsafe_code)]
    pub(super) fn new(feature: &'static str) -> Result<Self, DeviceError> {
        // SAFETY: `CoInitializeEx` は呼び出しスレッドに対する初期化要求であり、
        // 失敗時（`RPC_E_CHANGED_MODE` 等）もプロセス状態を破壊しない。
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            // `should_uninit` を成功時（`S_OK`/`S_FALSE`）のみ真にすることで、
            // 対になる `CoUninitialize` を失敗時には呼ばない（二重 uninit を避ける）。
            Ok(Self {
                should_uninit: true,
            })
        } else {
            Err(DeviceError::Internal(feature))
        }
    }
}

impl Drop for ComInit {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.should_uninit {
            // SAFETY: `should_uninit` は `new` 内の `CoInitializeEx` が成功した
            // 場合のみ真になり、同一スレッド・同一 `ComInit` の生存中に 1 度だけ
            // 呼ばれる（`Drop` は 1 度しか走らない）。
            unsafe { CoUninitialize() };
        }
    }
}

/// `path` にショートカットが無ければ、`exe` を指す `.lnk` を作って
/// `System.AppUserModel.ID` に [`APP_AUMID`] を設定する。既にあれば
/// [`inspect_shortcut`] で確かめ、自分のもので最新なら `Ok(false)`、リンク先が古ければ
/// 作り直して置き換え、他のものなら失敗を返す。書き出しは同じ
/// ディレクトリの一意な一時ファイルへ行い、完了後に `path` へ `MoveFileExW`
/// （置換フラグなし）で公開する（`rename` は Windows 上で宛先を黙って置換して
/// しまうため使わない。`hard_link`〔`CreateHardLinkW`〕も宛先が既存だと失敗し
/// 置換しない点は同じだが、ローカル NTFS でしか機能せず、`%APPDATA%` がネット
/// ワークリダイレクトされた環境等では `AlreadyExists` 以外の理由で失敗して
/// 恒久的に通知が出せなくなる〔Cursor Bugbot 指摘、PR #1035〕。`MoveFileExW` は
/// 同一ディレクトリ内の移動であれば同じ「置換しない」保証をより広い環境で
/// 満たすため、こちらを使う。作り直しのときだけ `MOVEFILE_REPLACE_EXISTING` で
/// 置き換える）。新規作成で宛先に既にファイルがある場合（`path.exists()`
/// 確認後に別プロセスが同名ファイルを作った場合を含む）は自分の一時ファイルを
/// 捨てて「既に登録済み」として `Ok(false)` を返す。それ以外のエラー（書き込み
/// 拒否等）は競合と確認できないため `Ok(false)` へ畳まず `Err` を返す
/// （codex 指摘 P1、PR #1035。畳むと未登録の AUMID のまま通知を試みてしまう）。
/// 作成そのものが成功した後の `show_toast` の失敗（反映待ち等の一時的失敗を
/// 含む）はこの関数の対象外であり、呼び出し元 `notify` は作成済みのショート
/// カットを削除しない（削除すると次回も作成→失敗→削除を繰り返し、反映待ちで
/// 一時的に失敗しているだけの場合まで恒久的な機能不全になるため）。
/// 呼び出し元 `notify` が [`ComInit`] で COM を初期化済みのスレッド上で
/// のみ呼ばれる契約。
#[allow(unsafe_code)]
fn ensure_shortcut(path: &Path, exe: &Path) -> Result<bool, DeviceError> {
    // 既にある場合は中身を確かめる（存在だけで登録済み扱いにしない、codex 指摘
    // P1・PR #1035）。自分の AUMID でリンク先も今の実行ファイルならそのまま使い、
    // 自分の AUMID でリンク先が古ければ作り直して置き換える。AUMID が違う（または
    // 読めない）ファイルは所有権を確認できないので触らず、明示的に失敗を返す。
    let replace = if path.exists() {
        match inspect_shortcut(path, exe)? {
            Existing::Current => return Ok(false),
            Existing::Stale => true,
            Existing::Foreign => {
                return Err(DeviceError::Internal("notification shortcut foreign"));
            }
        }
    } else {
        false
    };
    let parent = path
        .parent()
        .ok_or(DeviceError::Internal("notification shortcut"))?;
    std::fs::create_dir_all(parent).map_err(|_| DeviceError::Internal("notification shortcut"))?;

    // プロセス ID + スレッド ID で一意にし、同一プロセス内の並行呼び出しとも
    // 衝突しないようにする（`notify` は呼び出しごとに専用スレッドで動く）。
    let tmp_name = format!(
        "{SHORTCUT_FILE}.{}-{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    );
    let tmp_path = parent.join(tmp_name);
    if let Err(err) = write_shortcut(&tmp_path, exe) {
        // 作りかけの一時ファイルが残っていれば消す（`NotFound` は無視してよい）。
        // この一時ファイルは自分だけが使うパスなので、他プロセスのファイルを
        // 誤って消す心配はない。
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    // `std::fs::rename` は Windows 上では `MOVEFILE_REPLACE_EXISTING` 付きで
    // 実装されており、宛先が既存でも黙って置換してしまう（競合時に他プロセスの
    // ファイルを上書きしない、という上の契約に反する。codex 指摘 P1、PR #1035）。
    // `MoveFileExW` を置換フラグ無し（`MOVE_FILE_FLAGS(0)`）で直接呼べば、宛先が
    // 既存のとき `ERROR_ALREADY_EXISTS` で失敗し置換しない。`hard_link` と違い
    // 同一ボリューム内の移動であれば動作し、`hard_link`〔`CreateHardLinkW`〕が
    // 要求するファイルシステム側のハードリンク対応（ローカル NTFS 限定）を
    // 必要としないため、ネットワークリダイレクトされた `%APPDATA%` 等でも
    // 「所有権不明な既存ファイルを上書きしない」契約を満たせる
    // （Cursor Bugbot 指摘、PR #1035）。
    // SAFETY: `tmp_path`/`path` は同一ディレクトリ内の絶対パスで有効な文字列。
    // `MOVE_FILE_FLAGS(0)` は置換・コピー許可のいずれのフラグも立てないため、
    // 宛先が既存でも上書きせず失敗する（戻り値は `Result` として扱う）。
    // 自分のショートカットを作り直すときだけ置換を許す（`Existing::Stale`）。
    let flags = if replace {
        MOVEFILE_REPLACE_EXISTING
    } else {
        MOVE_FILE_FLAGS(0)
    };
    let move_result = unsafe {
        MoveFileExW(
            &HSTRING::from(tmp_path.as_path()),
            &HSTRING::from(path),
            flags,
        )
    };
    match move_result {
        Ok(()) => {
            // `path` 側に実体ができたので、一時ファイル名はもう使わない
            // （`MoveFileExW` は移動なので `tmp_path` 自体は消えている）。
            Ok(true)
        }
        Err(err) if err.code() == ERROR_ALREADY_EXISTS.to_hresult() => {
            // path.exists() から今までの間に別プロセスが同名ファイルを作った
            // 場合。自分の一時ファイルを片付け、先にできたファイルを確かめ直す
            // （確かめずに登録済み扱いにしない、codex 指摘 P1・PR #1035）。
            // 今の実行ファイルを指す自分のものなら（同時に動いた自分の別要求が
            // 作ったもの）そのまま使う。古いリンク先（`Stale`）は登録済みとみなさず
            // 失敗を返す（次の通知で `Stale` の置換経路に入る。codex 指摘 P2・
            // PR #1035）。他アプリのものは触らず失敗を返す。
            let _ = std::fs::remove_file(&tmp_path);
            match inspect_shortcut(path, exe)? {
                Existing::Current => Ok(false),
                Existing::Stale => Err(DeviceError::Internal("notification shortcut stale")),
                Existing::Foreign => Err(DeviceError::Internal("notification shortcut foreign")),
            }
        }
        Err(_) => {
            // 書き込み拒否等の実エラー。競合とは確認できないため登録済み扱いに
            // せず、呼び出し元へエラーを伝える（codex 指摘 P1、PR #1035。全ての
            // rename/move エラーを競合とみなして `Ok(false)` を返すと、
            // 未登録の AUMID のまま通知を試みてしまう）。
            let _ = std::fs::remove_file(&tmp_path);
            Err(DeviceError::Internal("notification shortcut"))
        }
    }
}

/// 既存ショートカットの状態（[`inspect_shortcut`] の結果）。
///
/// 確認（[`inspect_shortcut`]）から置換（`MOVEFILE_REPLACE_EXISTING`）までの間に
/// 別プロセスが同名ファイルを差し替える競合は防がない。置き場所は利用者本人の
/// スタートメニュー（`%APPDATA%`）で、同じ利用者として動くプロセスはこの確認と
/// 無関係にいつでもファイルを書き換えられるため、ここで排他しても守れるものが無い
/// （PR #1035 codex 指摘への判断）。確認は「他アプリが正規に置いた同名ファイルを
/// うっかり上書きしない」ためのもの。
enum Existing {
    /// 自分の AUMID で、リンク先も今の実行ファイル。
    Current,
    /// 自分の AUMID だが、リンク先が古い（実行ファイルの移動・更新後）。
    Stale,
    /// AUMID が違う、または読めない。所有権を確認できないので変更しない。
    Foreign,
}

/// 既存の `.lnk` を読み、`System.AppUserModel.ID` とリンク先を確かめる。
/// `.lnk` として読めない・AUMID が無い場合は [`Existing::Foreign`]（上書きしない）。
/// 呼び出し元 `notify` が [`ComInit`] で COM を初期化済みのスレッド上でのみ呼ばれる契約。
#[allow(unsafe_code)]
fn inspect_shortcut(path: &Path, exe: &Path) -> Result<Existing, DeviceError> {
    // SAFETY: COM 初期化済みスレッド上の呼び出し（上記の契約）。定数のみを渡す。
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    let persist: IPersistFile = link
        .cast()
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    // SAFETY: `persist` は有効な COM ポインタ、`path` は絶対パス。読み取り専用で開く。
    if unsafe { persist.Load(&HSTRING::from(path.as_os_str()), STGM_READ) }.is_err() {
        return Ok(Existing::Foreign);
    }
    let store: IPropertyStore = link
        .cast()
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    // SAFETY: `store` は有効な COM ポインタ、`PKEY_AppUserModel_ID` は定数。
    let aumid = unsafe { store.GetValue(&PKEY_AppUserModel_ID) }
        .ok()
        // SAFETY: `value` は直前に取得した有効な PROPVARIANT。
        .and_then(|value| unsafe { PropVariantToBSTR(&value) }.ok())
        .map(|bstr| bstr.to_string());
    if aumid.as_deref() != Some(APP_AUMID) {
        return Ok(Existing::Foreign);
    }
    // 長いパス（`\\?\` 付き）も入る大きさを取る。
    let mut buf = vec![0u16; 32_768];
    // SAFETY: `link` は有効な COM ポインタ、`buf` は書き込み可能な領域、
    // `pfd` は使わないので null を渡す。
    let target = unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0) }
        .ok()
        .map(|()| {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..len])
        });
    let current = exe.to_string_lossy();
    // Windows のパスは大文字小文字を区別しないため、比較もそれに合わせる。
    Ok(match target {
        Some(t) if t.eq_ignore_ascii_case(&current) => Existing::Current,
        _ => Existing::Stale,
    })
}

/// `&str` を VT_LPWSTR の PROPVARIANT にする。`PROPVARIANT::from(&str)` は VT_BSTR になり、
/// プロパティストアの正規型（`System.AppUserModel.ID`・`System.Title` は VT_LPWSTR）と合わないため変換する。
/// `write_shortcut` と `windows_app_shortcuts`（ジャンプリストの表示名）が使う。失敗は `feature` ラベルの
/// `DeviceError::Internal` に写す。
#[allow(unsafe_code)]
pub(super) fn lpwstr_variant(
    value: &str,
    feature: &'static str,
) -> Result<PROPVARIANT, DeviceError> {
    let bstr_variant = PROPVARIANT::from(value);
    let mut out = PROPVARIANT::default();
    // SAFETY: `bstr_variant`/`out` はどちらも有効な PROPVARIANT
    // （前者は `From<&str>`、後者は `Default` によるゼロ初期化）。
    unsafe { PropVariantChangeType(&mut out, &bstr_variant, PROPVAR_CHANGE_FLAGS(0), VT_LPWSTR) }
        .map_err(|_| DeviceError::Internal(feature))?;
    Ok(out)
}

/// [`ensure_shortcut`] の本体。COM 経由で `.lnk` を 1 本書き出す。
#[allow(unsafe_code)]
fn write_shortcut(path: &Path, exe: &Path) -> Result<(), DeviceError> {
    // SAFETY: 呼び出し元 `notify` が `ComInit` で COM を初期化済みのスレッド上での
    // み呼ぶ契約。`ShellLink`/`CLSCTX_INPROC_SERVER` はどちらも定数で、戻り値は
    // 失敗時 `Err` として扱う。
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    // SAFETY: `link` は上で取得した有効な COM ポインタ。`exe` は呼び出し元
    // （`notify`）が `current_exe()` から得た絶対パス。
    unsafe { link.SetPath(&HSTRING::from(exe.as_os_str())) }
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;

    // `System.AppUserModel.ID`（PKEY_AppUserModel_ID）の正規型は VT_LPWSTR。
    let store: IPropertyStore = link
        .cast()
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    let lpwstr_variant = lpwstr_variant(APP_AUMID, "notification shortcut")?;
    // SAFETY: `store` は上で取得した有効な COM ポインタ、`PKEY_AppUserModel_ID` は
    // 定数、`lpwstr_variant` は直前に変換した有効な PROPVARIANT。
    unsafe { store.SetValue(&PKEY_AppUserModel_ID, &lpwstr_variant) }
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    // SAFETY: `store` は上と同じ有効な COM ポインタ。
    unsafe { store.Commit() }.map_err(|_| DeviceError::Internal("notification shortcut"))?;

    let persist: IPersistFile = link
        .cast()
        .map_err(|_| DeviceError::Internal("notification shortcut"))?;
    // SAFETY: `persist` は上で取得した有効な COM ポインタ、`path` は呼び出し元が
    // 組み立てた絶対パス。
    unsafe { persist.Save(&HSTRING::from(path.as_os_str()), true) }
        .map_err(|_| DeviceError::Internal("notification shortcut"))
}

/// スタートメニューのショートカットで [`APP_AUMID`] を登録済みにする（無ければ作る）。
/// `notify` と `windows_alarm.rs`（Issue #721）が同じ送り手を使うための共通入口。
/// `feature` は APPDATA・実行ファイルが取れないときの `Unsupported` のラベル。
/// 呼び出し元が [`ComInit`] で COM を初期化済みのスレッド上でのみ呼ぶ契約。
pub(super) fn ensure_app_aumid(feature: &'static str) -> Result<(), DeviceError> {
    let appdata = std::env::var_os("APPDATA").ok_or(DeviceError::Unsupported(feature))?;
    let exe = std::env::current_exe().map_err(|_| DeviceError::Unsupported(feature))?;
    let path = shortcut_path(&appdata);
    ensure_shortcut(&path, &exe)?;
    Ok(())
}

/// WinRT Toast（`Windows.UI.Notifications`）による `Notifier` 実装。
pub(crate) struct WindowsToastNotifier;

impl Notifier for WindowsToastNotifier {
    fn notify(&self, request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError> {
        // COM 初期化はこのガードが生存する間だけ有効（宣言順が Drop 順を決めるため、
        // COM オブジェクトより先に宣言し、関数末尾で最後に解放する）。初期化自体が
        // 失敗したら後続の COM 操作へ進まず、ここで止める。
        let _com = ComInit::new("notification com init")?;

        // `ensure_shortcut` の失敗（作成そのものの失敗）はここで作りかけのファイルを
        // 自前で消して `Err` を返す（内部で完結、下記参照）。一方 `show_toast` の
        // 失敗はショートカット作成とは独立した事象であり、特に AUMID 登録直後は
        // Windows 側のインデックス反映待ちで一時的に失敗し得る（モジュール doc の
        // 「既知の天井」参照）。ここで `show_toast` の失敗を理由にショートカットを
        // 削除すると、次回呼び出しでも「作成→反映待ちで失敗→削除」を繰り返し、
        // 反映さえ済めば成功するはずの一時的失敗が恒久的な機能不全になる。
        // したがって `show_toast` の成否に関わらずショートカットは残す。
        ensure_app_aumid("notification")?;
        show_toast(request)
    }
}

/// 登録済みの [`APP_AUMID`] でトーストを送る。[`ensure_shortcut`] の後にのみ
/// 呼ばれる契約（AUMID が未登録のまま呼ぶと通常失敗する）。
#[allow(unsafe_code)]
fn show_toast(request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError> {
    // プロセスの AUMID をここで明示する。タスクバーのまとめは UI 表示前に設定した
    // 場合のみ効く（Microsoft のドキュメント上の推奨）が、`notify` はウィンドウ
    // 生成後のワーカースレッドから呼ばれるため、この呼び出しの時点ではタスクバー
    // のまとめには既に間に合っていない。`CreateToastNotifierWithId` が明示的に
    // AUMID を渡すためトースト送信自体にも必須ではないので、失敗しても
    // 通知自体は止めない（cursor Bugbot 指摘、PR #1035）。冪等なため毎回呼ぶ。
    // SAFETY: `HSTRING::from(APP_AUMID)` は呼び出しの間だけ有効な一時文字列で、
    // 呼び出し先はその場で読むだけ（保持しない）。
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_AUMID)) };

    // 登録済みの AUMID で試みる。登録後も失敗し得る（インデックス未反映等）ため
    // `Unsupported` ではなく `Internal` へ写す（未対応ではなく実行時エラーの扱い）。
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_AUMID))
        .map_err(|_| DeviceError::Internal("notification send"))?;

    let setting: NotificationSetting = notifier
        .Setting()
        .map_err(|_| DeviceError::Internal("notification setting"))?;
    setting_result(setting.0)?;

    let xml: XmlDocument =
        ToastNotificationManager::GetTemplateContent(ToastTemplateType::ToastText02)
            .map_err(|_| DeviceError::Internal("notification send"))?;
    let texts = xml
        .GetElementsByTagName(&HSTRING::from("text"))
        .map_err(|_| DeviceError::Internal("notification send"))?;
    // XML 文字列を連結せず `CreateTextNode`/`AppendChild` で値を差し込む
    // （XML 注入を防ぐ、security.md A03）。
    set_text_at(&xml, &texts, 0, request.title())?;
    set_text_at(&xml, &texts, 1, request.body())?;

    let toast = ToastNotification::CreateToastNotification(&xml)
        .map_err(|_| DeviceError::Internal("notification send"))?;
    notifier
        .Show(&toast)
        .map_err(|_| DeviceError::Internal("notification send"))?;

    // Toast が実際に表示されたかは確かめられない（モジュール doc 参照）ため
    // `Submitted` を返す（`Shown` は返さない）。
    Ok(NotifyOutcome::Submitted)
}

/// `<text>` 要素の `index` 番目へテキストノードを追加する。`GetElementsByTagName`
/// の `ToastText02` テンプレートは `<text>` を 2 つ持つため、`index` は 0（タイトル）
/// または 1（本文）を渡す契約。
fn set_text_at(
    xml: &XmlDocument,
    texts: &windows::Data::Xml::Dom::XmlNodeList,
    index: u32,
    value: &str,
) -> Result<(), DeviceError> {
    let node = texts
        .Item(index)
        .map_err(|_| DeviceError::Internal("notification send"))?;
    let text_node = xml
        .CreateTextNode(&HSTRING::from(value))
        .map_err(|_| DeviceError::Internal("notification send"))?;
    node.AppendChild(&text_node)
        .map_err(|_| DeviceError::Internal("notification send"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_toast_notifier_implements_notifier() {
        fn assert_impl<T: Notifier>() {}
        assert_impl::<WindowsToastNotifier>();
    }

    #[test]
    fn shortcut_path_joins_start_menu_programs() {
        let path = shortcut_path(OsStr::new(r"C:\Users\test\AppData\Roaming"));
        assert_eq!(
            path,
            PathBuf::from(
                r"C:\Users\test\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Fandhe egui proxy.lnk"
            )
        );
    }

    #[test]
    fn setting_result_maps_enabled_to_ok() {
        assert!(setting_result(0).is_ok());
    }

    #[test]
    fn setting_result_maps_disabled_for_application_to_permission_denied() {
        assert!(matches!(
            setting_result(1),
            Err(DeviceError::PermissionDenied("notification"))
        ));
    }

    #[test]
    fn setting_result_maps_disabled_for_user_to_disabled() {
        assert!(matches!(
            setting_result(2),
            Err(DeviceError::Disabled("notification"))
        ));
    }

    #[test]
    fn setting_result_maps_disabled_by_group_policy_to_disabled() {
        assert!(matches!(
            setting_result(3),
            Err(DeviceError::Disabled("notification"))
        ));
    }

    #[test]
    fn setting_result_maps_disabled_by_manifest_to_unsupported() {
        assert!(matches!(
            setting_result(4),
            Err(DeviceError::Unsupported("notification"))
        ));
    }

    #[test]
    fn setting_result_maps_unknown_code_to_internal() {
        assert!(matches!(
            setting_result(99),
            Err(DeviceError::Internal("notification setting"))
        ));
    }
}
