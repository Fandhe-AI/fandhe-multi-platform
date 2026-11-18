//! Windows 向けファイルの関連付け受信（Issue #416 で土台、#420 で中身）。
//! `driver::platform_file_association` の Windows 経路と `fandhe-gallery` の `main.rs` から使われる。検査・保持は
//! 共通側の `crate::contract::FileInbox` に任せ、本モジュールは Windows 固有の
//! 「登録」と「届き方」だけを持つ。
//!
//! # 届き方
//!
//! 1. **登録**（[`win::register`] / [`win::unregister`]）: `HKCU\Software\Classes\Fandhe.EguiProxy.Txt.1`
//!    の ProgID（`shell\open\command` = `"<exe>" --fandhe-open-file "%1"`）を作り、
//!    `.txt\OpenWithProgids` に値を足すだけ。`.txt` の既定値と UserChoice には触れないので既定アプリは
//!    奪わず、エクスプローラーの「プログラムから開く」に出る。起動のたびに黙って書かず、画面のボタン操作で
//!    登録・解除する。HKCU のみのため管理者権限は要らない。完了後に `SHChangeNotify(SHCNE_ASSOCCHANGED)`
//! 2. **受信**: パスは起動引数 `--fandhe-open-file <path>` で届く。目印の引数で通常起動・URL 起動と
//!    取り違えない。[`checked_path`] が `validate_windows_path`・`"` の拒否・拡張子を確かめ、通ったものだけ
//!    `FileInbox::receive_path` へ渡す
//! 3. **起動済みの場合の転送**: `fandhe-cap-url-scheme` の named pipe に種別 1 バイト（`F`）を付けて多重化し、
//!    既存インスタンスの [`receive`] へ渡す。単一インスタンスの調停は url-scheme が持ち、本 crate は
//!    [`register_route`] で「ファイル起動の経路」（目印の引数・解釈・受け口）だけを渡す（url-scheme は本 crate へ依存しない）
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の受信順・不正なパス（UNC・`\\?\`・相対・`"`・拡張子違い）の非表示・
//! コールドスタート分の保持・転送の失敗を成功にしないこと。
//! 守らない: UserChoice が勝つこと、他アプリによる上書き、exe を動かした後に登録し直さないこと、
//! 同じセッションの別プロセスによる pipe の妨害、ポーリング間隔の間に届いた複数件は最後の 1 件だけ出ること、
//! 遅いボリュームでの同期読み取り、解除後に残る空の `OpenWithProgids` キー。
//!
//! パスは画面にもログにも出さない。`unsafe` は [`win`] に閉じ、各呼び出しに `// SAFETY:` を付ける。
//! 純関数は全 OS でコンパイルしホストの `cargo test` で検査する。

// ホストの `cargo test` でも純関数を検査するため全 OS でコンパイルする。Windows 以外では OS 実装の入口が未使用になる。
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::ffi::OsString;
use std::path::Path;

use crate::contract::{
    EXTENSION, FileAssociation, FileInbox, FileSnapshot, accepts, validate_windows_path,
};
use crate::error::FileAssociationError;

/// 登録・解除の差し込み口（effect ドライバのテストで偽物に替える）。
pub struct Registrar {
    /// `.txt` を「プログラムから開く」に出す。
    pub register: fn() -> Result<(), FileAssociationError>,
    /// 登録を解除する（もともと未登録でも `Ok`）。
    pub unregister: fn() -> Result<(), FileAssociationError>,
}

/// url-scheme の単一インスタンス転送へ、ファイル起動の経路を登録する。入口（`fandhe` の `entry/lifecycle.rs`）が
/// url-scheme の `startup` より前に 1 度だけ呼ぶ（登録が無いとファイル起動は URL 起動として扱われず捨てられる）。
#[cfg(target_os = "windows")]
pub fn register_route() -> bool {
    fandhe_cap_url_scheme::windows::register_file_route(fandhe_cap_url_scheme::windows::FileRoute {
        open_flag: OPEN_FLAG,
        parse_args: parse_file_args,
        receive,
        max_body: crate::contract::MAX_PATH_BYTES,
    })
}

/// 単一インスタンスの調停（url-scheme の `startup`）を、url-scheme を選ばないアプリからも呼べるようにする口。
/// ファイル起動の受け口への配送と起動済みインスタンスへの転送は `startup` が担うため、入口（`fandhe`）は
/// file-association だけを選んだとき [`register_route`] の後にこれを呼ぶ。戻り値は `startup` と同じ
/// （転送して終えるべきなら終了コード、自分が受ける側なら `None`）。
#[cfg(target_os = "windows")]
pub fn startup(args: &[std::ffi::OsString]) -> Option<i32> {
    fandhe_cap_url_scheme::windows::win::startup(args)
}

#[cfg(target_os = "windows")]
impl From<fandhe_cap_url_scheme::windows::RegistryError> for FileAssociationError {
    fn from(e: fandhe_cap_url_scheme::windows::RegistryError) -> Self {
        if e.is_access_denied() {
            FileAssociationError::PermissionDenied
        } else {
            FileAssociationError::Internal("file_association register")
        }
    }
}

/// ファイル起動を示す目印の引数（通常起動・URL 起動と取り違えない）。
pub const OPEN_FLAG: &str = "--fandhe-open-file";
/// 登録する ProgID（`Vendor.Component.Version` 形）。
pub const PROG_ID: &str = "Fandhe.EguiProxy.Txt.1";
/// ProgID のキー（`HKCU` 直下）。
pub const PROG_ID_KEY: &str = "Software\\Classes\\Fandhe.EguiProxy.Txt.1";

/// 受信したファイルの受け口（プロセス全体で 1 つ）。コールドスタート分もここに溜まる。
static INBOX: FileInbox = FileInbox::new();

/// `.txt\OpenWithProgids` のキー。拡張子は共通側の [`EXTENSION`] から組み立てる。
pub fn open_with_progids_key() -> String {
    format!("Software\\Classes\\.{EXTENSION}\\OpenWithProgids")
}

/// 起動引数を解釈する。`None` はファイル起動ではない、`Some(None)` はファイル起動だが不正
/// （引数の数が違う・非 Unicode。拒否として数える）、`Some(Some(p))` は検査候補。
pub fn parse_file_args(args: &[OsString]) -> Option<Option<String>> {
    if args.first().is_none_or(|a| a != OPEN_FLAG) {
        return None;
    }
    match args {
        [_, path] => Some(path.to_str().map(str::to_owned)),
        _ => Some(None),
    }
}

/// 起動引数がファイルを開く起動か。
pub fn is_file_launch(args: &[OsString]) -> bool {
    parse_file_args(args).is_some()
}

/// 絶対パス・`"` なし・拡張子一致のものだけを返す。Windows のファイル名に `"` は入らないので、
/// 解析後に `"` が残るパスはここで弾く。`"` が消費されて変形した起動は、受信前に生コマンドライン照合（`launch_unaltered_after`）で拒否する。
pub fn checked_path(p: &str) -> Option<&Path> {
    // ドライブ指定（先頭 2 文字）より後ろの `:` は NTFS の代替データストリーム（`a.md:x.txt`）で、
    // 拡張子検査を迂回するため弾く。
    if validate_windows_path(p).is_err()
        || p.contains('"')
        || p.get(2..).is_none_or(|r| r.contains(':'))
    {
        return None;
    }
    // ホストの `Path` は `\` を区切りとみなさないため、名前は自前で切り出す（全 OS で同じ判定）。
    let name = p.rsplit('\\').next();
    accepts(name, None).then_some(Path::new(p))
}

/// 検査を通れば受け口へ渡して `true`、通らなければ拒否として数えて `false`。
pub fn receive_into(inbox: &FileInbox, payload: Option<&str>, now_ms: u64) -> bool {
    match payload.and_then(checked_path) {
        Some(p) => {
            inbox.receive_path(Some(p), now_ms);
            true
        }
        None => {
            inbox.reject();
            false
        }
    }
}

/// 自プロセスの受け口へ入れる（起動引数・pipe 転送の双方から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn receive(payload: Option<&str>) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    receive_into(&INBOX, payload, now)
}

/// レジストリへ書く `shell\open\command` の値。`"` を含む exe パスは引数注入になりうるため拒否する。
pub fn register_command(exe: &Path) -> Result<String, FileAssociationError> {
    let exe = exe
        .to_str()
        .filter(|s| !s.contains('"'))
        .ok_or(FileAssociationError::InvalidInput)?;
    Ok(format!("\"{exe}\" {OPEN_FLAG} \"%1\""))
}

/// Windows 向け `FileAssociation` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
struct WindowsFileAssociation;

impl FileAssociation for WindowsFileAssociation {
    fn snapshot(&self) -> Result<FileSnapshot, FileAssociationError> {
        Ok(INBOX.snapshot())
    }
}

/// Windows の実装を返す。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &WindowsFileAssociation
}

/// 登録・解除の差し込み口。Windows では実レジストリ、それ以外では `None`（ボタンを出さない）。
pub fn registrar() -> Option<&'static Registrar> {
    #[cfg(target_os = "windows")]
    {
        Some(&SYSTEM_REGISTRAR)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 実機のレジストリを操作する [`Registrar`]。
#[cfg(target_os = "windows")]
static SYSTEM_REGISTRAR: Registrar = Registrar {
    register: win::register,
    unregister: win::unregister,
};

/// Win32 呼び出しを閉じる（`unsafe` はこのサブモジュールに限る）。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub mod win {
    use crate::error::FileAssociationError;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_NONE, REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW,
    };
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    use windows::core::PCWSTR;

    use super::{PROG_ID, PROG_ID_KEY, open_with_progids_key, register_command};
    use fandhe_cap_url_scheme::windows::RegistryError;
    use fandhe_cap_url_scheme::windows::win::{open_key, set_value, utf16_bytes, wide};

    /// 関連付けの変更を Shell へ知らせる（呼ばないと再起動まで反映されないことがある）。
    fn notify() {
        // SAFETY: イベント種別とフラグだけを渡し、ポインタは渡さない（None）。
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }

    fn register_inner() -> Result<(), FileAssociationError> {
        let exe = std::env::current_exe()
            .map_err(|_| FileAssociationError::Internal("file_association register"))?;
        let command = register_command(&exe)?;
        let root = open_key(PROG_ID_KEY)?;
        set_value(
            &root,
            None,
            REG_SZ,
            &utf16_bytes("Fandhe EguiProxy テキスト"),
        )?;
        let cmd = open_key(&format!("{PROG_ID_KEY}\\shell\\open\\command"))?;
        set_value(&cmd, None, REG_SZ, &utf16_bytes(&command))?;
        // 存在しない ProgID を指さないよう、最後に足す。`.txt` の既定値と UserChoice は触らない。
        let owp = open_key(&open_with_progids_key())?;
        Ok(set_value(&owp, Some(PROG_ID), REG_NONE, &[])?)
    }

    /// `.txt` を「プログラムから開く」に出す。途中で失敗したら `Err`（巻き戻しはしない）。
    // ponytail: 失敗しても巻き戻さない。exe を動かしたら登録し直しが要る。
    pub fn register() -> Result<(), FileAssociationError> {
        let r = register_inner();
        notify();
        r
    }

    fn ok_or_missing(err: WIN32_ERROR) -> Result<(), FileAssociationError> {
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(RegistryError(err.0).into())
        }
    }

    fn unregister_inner() -> Result<(), FileAssociationError> {
        let owp = wide(&open_with_progids_key());
        let name = wide(PROG_ID);
        // SAFETY: どちらも NUL 終端で呼び出し中有効。自分が足した値だけを消す。
        ok_or_missing(unsafe {
            RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(owp.as_ptr()),
                PCWSTR(name.as_ptr()),
            )
        })?;
        let sub = wide(PROG_ID_KEY);
        // SAFETY: `sub` は NUL 終端で呼び出し中有効。自分の ProgID 配下だけを消す。
        ok_or_missing(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr())) })
    }

    /// 自分が足した値と ProgID だけ消す。未登録なら `Ok`。空の `.txt\OpenWithProgids` は残る。
    pub fn unregister() -> Result<(), FileAssociationError> {
        let r = unregister_inner();
        notify();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_file_args_classifies_launches() {
        assert_eq!(parse_file_args(&args(&[])), None);
        assert_eq!(parse_file_args(&args(&["--foo"])), None);
        assert_eq!(parse_file_args(&args(&["fandhe://a"])), None);
        assert_eq!(parse_file_args(&args(&[OPEN_FLAG])), Some(None));
        assert_eq!(
            parse_file_args(&args(&[OPEN_FLAG, "C:\\a.txt"])),
            Some(Some("C:\\a.txt".into()))
        );
        assert_eq!(parse_file_args(&args(&[OPEN_FLAG, "p", "x"])), Some(None));
        assert_eq!(parse_file_args(&args(&["x", OPEN_FLAG, "p"])), None);
        assert!(is_file_launch(&args(&[OPEN_FLAG, "p"])));
        assert!(!is_file_launch(&args(&[])));
    }

    #[cfg(windows)]
    #[test]
    fn parse_file_args_rejects_non_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let bad = OsString::from_wide(&[0x43, 0x3A, 0x5C, 0xD800]);
        assert_eq!(
            parse_file_args(&[OsString::from(OPEN_FLAG), bad]),
            Some(None)
        );
    }

    #[test]
    fn checked_path_accepts_only_valid_txt() {
        assert!(checked_path("C:\\a.txt").is_some());
        assert!(checked_path("C:\\A.TXT").is_some());
        for bad in [
            "C:\\a.md",
            "\\\\server\\s\\a.txt",
            "\\\\?\\C:\\a.txt",
            "a.txt",
            "C:\\a\"b.txt",
            "C:\\a\u{7}.txt",
            "C:\\a.md:x.txt",
            "C:/a.md:x.txt",
            "C:\\a.txt:s",
        ] {
            assert!(checked_path(bad).is_none(), "{bad}");
        }
        assert!(checked_path(&format!("C:\\{}.txt", "a".repeat(3000))).is_none());
    }

    #[test]
    fn receive_into_counts_rejects_and_surfaces_read_failure() {
        let inbox = FileInbox::new();
        assert!(!receive_into(&inbox, Some("C:\\a.md"), 1));
        assert!(!receive_into(&inbox, None, 2));
        let s = inbox.snapshot();
        assert_eq!(s.rejected, 2);
        assert!(s.latest.is_none());
        // 検査は通るが存在しない: 転送は成功、画面には失敗として出る（`Path` が Windows 流の
        // ホストでだけ確かめられる）。
        #[cfg(windows)]
        assert!(receive_into(&inbox, Some("C:\\none-420\\x.txt"), 3));
        #[cfg(windows)]
        {
            let s = inbox.snapshot();
            assert_eq!(s.rejected, 2);
            assert!(matches!(s.latest, Some(Err(_))));
        }
    }

    #[test]
    fn register_command_and_key_shapes() {
        let c = register_command(Path::new("C:\\a b\\app.exe")).unwrap();
        assert_eq!(c, "\"C:\\a b\\app.exe\" --fandhe-open-file \"%1\"");
        assert!(register_command(Path::new("C:\\a\"b.exe")).is_err());
        assert_eq!(
            open_with_progids_key(),
            format!("Software\\Classes\\.{EXTENSION}\\OpenWithProgids")
        );
    }
}
