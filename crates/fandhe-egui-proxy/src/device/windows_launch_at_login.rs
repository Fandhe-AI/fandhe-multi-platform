//! Windows 向けログイン時の自動起動（Issue #697。macOS は #696 の `SMAppService`）。
//! HKCU の Run キーに値 `fandhe-egui-proxy` を書き、Explorer のログイン時起動に載せる。
//! `device/launch_at_login.rs`（effect ドライバ）の windows 経路から、操作ごとの
//! ワーカースレッドで同期に呼ばれる。契約（結果の種類・守備範囲）は
//! `fandhe_device::launch_at_login` が定義し、本ファイルはその Windows 実装に徹する。
//!
//! # 方針
//!
//! - 値データは引用符付きの実行体パス + `--fandhe-launched-at-login`（260 文字以内）。
//!   `"` を含むパスは引数注入になりうるため拒否する。`REG_SZ` で書き、環境変数展開をさせない
//! - StartupApproved は読むだけ。値が無ければ有効、先頭バイト 0x02 は `Enabled`、
//!   0x03 は `DisabledByUser`、それ以外は `Unknown`。書き込まない
//! - 同じ値名に別の場所の実行体（型違い・引数違い・長大な値を含む）があれば
//!   `RegisteredElsewhere`。有効化でも解除でも上書き・削除しない（別インストールの登録の保護）
//! - 自分の登録が既にあれば有効化は書かずに読み戻す（べき等）。未登録の解除は OS を呼ばず
//!   `Ok(NotRegistered)`
//! - 設定画面は `ms-settings:startupapps`
//! - 実行体のパスをログ・エラー文言に出さない（`DeviceError` は `&'static str` のみ）
//!
//! # 既知の天井（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! StartupApproved は非公開形式で、値が無いときの起動有無は Windows 実機で確かめる（#694）。
//! 実行体の移動への自動追従、判定から書き込みまでの間の他プロセスによる変更は守らない。
//!
//! モジュールはホストテストのため test でもコンパイルする（`device.rs`）。

use fandhe_device::launch_at_login::{LaunchAtLogin, LoginItemStatus};
use fandhe_device::DeviceError;

/// Run キー（HKCU 配下）。
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// StartupApproved（HKCU 配下、読むだけ）。
const APPROVED_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
/// 値名。`windows_background_task` のタスク名・mutex 名と同じ接頭辞。
const VALUE_NAME: &str = "fandhe-egui-proxy";
/// 起動の目印となる引数。
const LAUNCH_FLAG: &str = "--fandhe-launched-at-login";
/// 値データの上限（UTF-16 単位）。
const MAX_COMMAND_CHARS: usize = 260;
/// 設定画面の URI（固定。任意の URI を開く経路は作らない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const SETTINGS_URI: &str = "ms-settings:startupapps";
const FEATURE: &str = "launch_at_login";

/// 実行体パスから Run キーの値データを組み立てる。`"` を含む・上限超過は `InvalidInput`。
fn run_command(exe: &str) -> Result<String, DeviceError> {
    if exe.contains('"') {
        return Err(DeviceError::InvalidInput(FEATURE));
    }
    let command = format!("\"{exe}\" {LAUNCH_FLAG}");
    if command.encode_utf16().count() > MAX_COMMAND_CHARS {
        return Err(DeviceError::InvalidInput(FEATURE));
    }
    Ok(command)
}

/// Run キーから読んだ値（型が `REG_SZ` のときだけ文字列を持つ）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunValue {
    Sz(String),
    /// `REG_SZ` 以外、または読めない長さ。
    Other,
}

/// 値名の持ち主。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Absent,
    Ours,
    Elsewhere,
}

fn ownership(value: Option<RunValue>, expected: &str) -> Ownership {
    match value {
        None => Ownership::Absent,
        Some(RunValue::Sz(s)) if s.eq_ignore_ascii_case(expected) => Ownership::Ours,
        Some(_) => Ownership::Elsewhere,
    }
}

/// StartupApproved の値（無ければ `None`）から状態を決める。値が無い Run 項目は有効扱い。
fn approved_status(bytes: Option<&[u8]>) -> LoginItemStatus {
    match bytes {
        None => LoginItemStatus::Enabled,
        Some(b) => match b.first() {
            Some(0x02) => LoginItemStatus::Enabled,
            Some(0x03) => LoginItemStatus::DisabledByUser,
            _ => LoginItemStatus::Unknown,
        },
    }
}

fn status_of(owner: Ownership, approved: Option<LoginItemStatus>) -> LoginItemStatus {
    match owner {
        Ownership::Absent => LoginItemStatus::NotRegistered,
        Ownership::Elsewhere => LoginItemStatus::RegisteredElsewhere,
        Ownership::Ours => approved.unwrap_or(LoginItemStatus::Unknown),
    }
}

/// 操作の計画。OS を呼ぶかどうかの判断を純関数に切り出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Write,
    Delete,
    ReadBack,
    Return(LoginItemStatus),
}

fn enable_plan(owner: Ownership) -> Plan {
    match owner {
        Ownership::Absent => Plan::Write,
        Ownership::Ours => Plan::ReadBack,
        Ownership::Elsewhere => Plan::Return(LoginItemStatus::RegisteredElsewhere),
    }
}

fn disable_plan(owner: Ownership) -> Plan {
    match owner {
        Ownership::Absent => Plan::Return(LoginItemStatus::NotRegistered),
        Ownership::Ours => Plan::Delete,
        Ownership::Elsewhere => Plan::Return(LoginItemStatus::RegisteredElsewhere),
    }
}

/// Win32 エラーコードを契約上のエラー種別へ写す（全呼び出し共通。パスは含めない）。
fn map_registry_error(code: u32) -> DeviceError {
    const ERROR_ACCESS_DENIED: u32 = 5;
    if code == ERROR_ACCESS_DENIED {
        DeviceError::PermissionDenied(FEATURE)
    } else {
        DeviceError::Internal(FEATURE)
    }
}

/// `ShellExecuteW` の戻り値（32 より大きければ成功）を結果へ写す。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn settings_result(code: isize) -> Result<(), DeviceError> {
    if code > 32 {
        Ok(())
    } else {
        Err(DeviceError::Internal("launch_at_login settings"))
    }
}

/// UTF-16 のバイト列（REG_SZ）を文字列へ。最初の NUL 以降を除く。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_sz(bytes: &[u8]) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod win {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY, REG_SZ,
        REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_ANY,
    };

    /// 読み取りの上限（バイト）。自分の登録は 260 文字 + NUL に収まるので、超える値は別物。
    const MAX_READ_BYTES: u32 = ((MAX_COMMAND_CHARS + 1) * 2) as u32;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 値の読み取り結果。
    enum Read {
        Missing,
        Found(REG_VALUE_TYPE, Vec<u8>),
        TooLong,
    }

    fn read_value(subkey: &str, name: &str) -> Result<Read, DeviceError> {
        let sub = wide(subkey);
        let nm = wide(name);
        let mut ty = REG_VALUE_TYPE(0);
        let mut buf = vec![0u8; MAX_READ_BYTES as usize];
        let mut cap = MAX_READ_BYTES;
        // SAFETY: `sub`/`nm` は NUL 終端で呼び出し中有効。`buf` は `cap` バイトの確保済み領域で、
        // `ty`/`cap` は有効な書き込み先。関数は引数を保持しない。
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                PCWSTR(nm.as_ptr()),
                RRF_RT_ANY | RRF_NOEXPAND,
                Some(&mut ty),
                Some(buf.as_mut_ptr().cast()),
                Some(&mut cap),
            )
        };
        if err == ERROR_SUCCESS {
            buf.truncate(cap as usize);
            Ok(Read::Found(ty, buf))
        } else if err == ERROR_FILE_NOT_FOUND {
            Ok(Read::Missing)
        } else if err == ERROR_MORE_DATA {
            Ok(Read::TooLong)
        } else {
            Err(map_registry_error(err.0))
        }
    }

    fn run_value() -> Result<Option<RunValue>, DeviceError> {
        Ok(match read_value(RUN_KEY, VALUE_NAME)? {
            Read::Missing => None,
            Read::TooLong => Some(RunValue::Other),
            Read::Found(ty, bytes) if ty == REG_SZ => Some(RunValue::Sz(decode_sz(&bytes))),
            Read::Found(..) => Some(RunValue::Other),
        })
    }

    fn approved() -> Result<LoginItemStatus, DeviceError> {
        Ok(match read_value(APPROVED_KEY, VALUE_NAME)? {
            Read::Missing => approved_status(None),
            Read::Found(ty, bytes) if ty == REG_BINARY => approved_status(Some(&bytes)),
            // 型違い・長大な値は判別不能（有効と見せない）。
            Read::Found(..) | Read::TooLong => LoginItemStatus::Unknown,
        })
    }

    pub(super) fn current_owner(expected: &str) -> Result<Ownership, DeviceError> {
        Ok(ownership(run_value()?, expected))
    }

    pub(super) fn read_back(expected: &str) -> Result<LoginItemStatus, DeviceError> {
        let owner = current_owner(expected)?;
        let approved = if owner == Ownership::Ours {
            Some(approved()?)
        } else {
            None
        };
        Ok(status_of(owner, approved))
    }

    pub(super) fn write_run(command: &str) -> Result<(), DeviceError> {
        let sub = wide(RUN_KEY);
        let nm = wide(VALUE_NAME);
        let data: Vec<u8> = wide(command).iter().flat_map(|u| u.to_le_bytes()).collect();
        // SAFETY: `sub`/`nm` は NUL 終端、`data` は `data.len()` バイトの有効な領域で、
        // いずれも呼び出し中生きる。関数は保持しない。キーは関数内で開閉される。
        let err = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(sub.as_ptr()),
                PCWSTR(nm.as_ptr()),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                data.len() as u32,
            )
        };
        if err == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(map_registry_error(err.0))
        }
    }

    pub(super) fn delete_run() -> Result<(), DeviceError> {
        let sub = wide(RUN_KEY);
        let nm = wide(VALUE_NAME);
        // SAFETY: `sub`/`nm` は NUL 終端で呼び出し中有効。
        let err = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), PCWSTR(nm.as_ptr()))
        };
        if err == ERROR_SUCCESS || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(map_registry_error(err.0))
        }
    }

    /// `ShellExecuteW` 用の COM 初期化ガード。成功（`S_OK`/`S_FALSE`）時だけ Drop で解放する。
    struct ComGuard(bool);
    impl ComGuard {
        fn new() -> Self {
            // SAFETY: 呼び出しスレッドへの初期化要求。失敗（`RPC_E_CHANGED_MODE` 等）でも
            // プロセス状態を壊さず、その場合は解放しない。
            let hr =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            Self(hr.is_ok())
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: `new` で成功した初期化と 1 対 1 に対応する。
                unsafe { CoUninitialize() };
            }
        }
    }

    pub(super) fn open_settings() -> Result<(), DeviceError> {
        let _com = ComGuard::new();
        settings_result(crate::device::windows_compose_mail_sms::open_url(
            SETTINGS_URI,
        ))
    }
}

/// 実行体から期待するコマンドを作る。
#[cfg(target_os = "windows")]
fn expected_command() -> Result<String, DeviceError> {
    let exe = std::env::current_exe().map_err(|_| DeviceError::Internal(FEATURE))?;
    let exe = exe.to_str().ok_or(DeviceError::InvalidInput(FEATURE))?;
    run_command(exe)
}

#[cfg(target_os = "windows")]
struct WindowsLaunchAtLogin;

#[cfg(target_os = "windows")]
impl LaunchAtLogin for WindowsLaunchAtLogin {
    fn status(&self) -> Result<LoginItemStatus, DeviceError> {
        win::read_back(&expected_command()?)
    }

    fn enable(&self) -> Result<LoginItemStatus, DeviceError> {
        let command = expected_command()?;
        match enable_plan(win::current_owner(&command)?) {
            Plan::Write => win::write_run(&command)?,
            Plan::Return(s) => return Ok(s),
            Plan::ReadBack | Plan::Delete => {}
        }
        win::read_back(&command)
    }

    fn disable(&self) -> Result<LoginItemStatus, DeviceError> {
        let command = expected_command()?;
        match disable_plan(win::current_owner(&command)?) {
            Plan::Delete => win::delete_run()?,
            Plan::Return(s) => return Ok(s),
            Plan::ReadBack | Plan::Write => {}
        }
        win::read_back(&command)
    }

    fn open_settings(&self) -> Result<(), DeviceError> {
        win::open_settings()
    }
}

/// Windows の `LaunchAtLogin` 実装を返す（非 Windows の test ビルドでは `Unsupported`）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Box<dyn LaunchAtLogin> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsLaunchAtLogin)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Box::new(fandhe_device::launch_at_login::UnsupportedLaunchAtLogin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LoginItemStatus::*;

    const EXP: &str = "\"C:\\a\\app.exe\" --fandhe-launched-at-login";

    #[test]
    fn run_command_builds_and_limits() {
        assert_eq!(run_command("C:\\a\\app.exe").unwrap(), EXP);
        assert!(matches!(
            run_command("C:\\a\"b.exe"),
            Err(DeviceError::InvalidInput(_))
        ));
        let fixed = format!("\"\" {LAUNCH_FLAG}").encode_utf16().count();
        assert!(run_command(&"x".repeat(MAX_COMMAND_CHARS - fixed)).is_ok());
        assert!(run_command(&"x".repeat(MAX_COMMAND_CHARS - fixed + 1)).is_err());
    }

    #[test]
    fn ownership_cases() {
        assert_eq!(ownership(None, EXP), Ownership::Absent);
        assert_eq!(
            ownership(Some(RunValue::Sz(EXP.into())), EXP),
            Ownership::Ours
        );
        assert_eq!(
            ownership(Some(RunValue::Sz(EXP.to_uppercase())), EXP),
            Ownership::Ours
        );
        assert_eq!(
            ownership(
                Some(RunValue::Sz(
                    "\"D:\\b.exe\" --fandhe-launched-at-login".into()
                )),
                EXP
            ),
            Ownership::Elsewhere
        );
        assert_eq!(ownership(Some(RunValue::Other), EXP), Ownership::Elsewhere);
        assert_eq!(
            ownership(Some(RunValue::Sz("\"C:\\a\\app.exe\"".into())), EXP),
            Ownership::Elsewhere
        );
    }

    #[test]
    fn approved_bytes() {
        assert_eq!(approved_status(None), Enabled);
        assert_eq!(approved_status(Some(&[0x02, 0, 0])), Enabled);
        assert_eq!(approved_status(Some(&[0x03, 0, 0])), DisabledByUser);
        assert_eq!(approved_status(Some(&[0x06])), Unknown);
        assert_eq!(approved_status(Some(&[])), Unknown);
        assert_eq!(approved_status(Some(&[0x00])), Unknown);
    }

    #[test]
    fn status_mapping_never_fakes_success() {
        assert_eq!(status_of(Ownership::Absent, Some(Enabled)), NotRegistered);
        assert_eq!(
            status_of(Ownership::Elsewhere, Some(Enabled)),
            RegisteredElsewhere
        );
        assert_eq!(status_of(Ownership::Ours, None), Unknown);
        assert!(!status_of(Ownership::Ours, Some(DisabledByUser)).is_enabled());
        assert!(!RegisteredElsewhere.is_enabled());
        assert!(!Unknown.is_enabled());
        assert!(status_of(Ownership::Ours, Some(Enabled)).is_enabled());
    }

    #[test]
    fn plans() {
        assert_eq!(enable_plan(Ownership::Absent), Plan::Write);
        assert_eq!(enable_plan(Ownership::Ours), Plan::ReadBack);
        assert_eq!(
            enable_plan(Ownership::Elsewhere),
            Plan::Return(RegisteredElsewhere)
        );
        assert_eq!(disable_plan(Ownership::Absent), Plan::Return(NotRegistered));
        assert_eq!(
            disable_plan(Ownership::Elsewhere),
            Plan::Return(RegisteredElsewhere)
        );
        assert_eq!(disable_plan(Ownership::Ours), Plan::Delete);
    }

    #[test]
    fn error_mapping_hides_paths() {
        assert!(matches!(
            map_registry_error(5),
            DeviceError::PermissionDenied(_)
        ));
        assert!(matches!(map_registry_error(1234), DeviceError::Internal(_)));
        let text = format!("{}", map_registry_error(5));
        assert!(!text.contains('\\') && !text.contains(".exe"));
    }

    #[test]
    fn settings_result_threshold() {
        assert!(settings_result(33).is_ok());
        for c in [0, 2, 31, 32] {
            assert!(settings_result(c).is_err());
        }
    }

    #[test]
    fn decode_sz_strips_nul() {
        let b: Vec<u8> = "ab\0"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_sz(&b), "ab");
    }
}
