//! Windows 向けメール・SMS 作成画面の実装（Issue #1384・#31）。`ShellExecuteW` で `mailto:` /
//! `sms:` URL を開き、既定のハンドラの作成画面を出す。`device/compose_mail_sms.rs` の Windows 経路から呼ばれ、
//! 契約は `fandhe_device::compose_mail_sms`。渡すのは `ComposeRequest::to_url` が組み立てた
//! `mailto:` / `sms:` URL の 2 種だけで、scheme は固定。任意の scheme・パス・引数を渡す経路は作らない。
//! 添付付きの要求は URL で渡せないため `to_url` が `Unsupported` を返し、ここで拒否する（Issue #32）。
//! Windows 本体は `sms:` のハンドラを同梱しない。登録したアプリ（スマートフォン連携など）があれば
//! 開き、無ければ `SE_ERR_NOASSOC` → `NoHandler`。WinRT の `ChatMessageManager` は制限付き
//! capability とパッケージ ID が要るため採らない（#31）。
//!
//! 戻り値の写像は純関数 [`outcome_from_shell`] に置き、ホストでもテストできるよう
//! モジュールは `cfg(any(windows, test))`。`unsafe` は `ShellExecuteW` の 1 呼び出しに閉じる。
//!
//! # 既知の天井
//!
//! - 戻り値 `> 32` は「OS が受理した」だけで、メールアプリが開いたかは不明（`HandedOff`）
//! - `mailto:` の関連付けが無いと、Windows 10 以降は「アプリを選ぶ」を出して `> 32` を返すことが
//!   ある。`NoHandler` にならない場合があり、実機で確かめる（#1383）
//! - `sms:` も同じく、関連付けが無くても「アプリを選ぶ」等で `> 32` が返り `NoHandler` にならない
//!   ことがある。宛先が空の `sms:?body=` をハンドラが受けるかも実機で確かめる（#34）
//!   （ponytail: 事前に `AssocQueryStringW` で調べれば `NoHandler` を確実にできる。必要になったら足す）
//! - URL が 2083 文字を超える場合の挙動は OS 依存で、失敗は `Internal` に倒れる

use fandhe_device::compose_mail_sms::ComposeOutcome;
use fandhe_device::DeviceError;

/// `SE_ERR_NOASSOC`（関連付けが無い）。
pub(crate) const SE_ERR_NOASSOC: isize = 31;

/// `ShellExecuteW` の戻り値 → 結果の写像（SSOT）。公式の表のとおり `> 32` が成功、
/// 31 が関連付け無し、それ以外（0〜32 の他のエラー値）は失敗。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_shell(code: isize) -> Result<ComposeOutcome, DeviceError> {
    match code {
        c if c > 32 => Ok(ComposeOutcome::HandedOff),
        SE_ERR_NOASSOC => Ok(ComposeOutcome::NoHandler),
        _ => Err(DeviceError::Internal("compose present")),
    }
}

#[cfg(target_os = "windows")]
struct WindowsComposeMailSms;

#[cfg(target_os = "windows")]
impl fandhe_device::compose_mail_sms::ComposeMailSms for WindowsComposeMailSms {
    fn compose(
        &self,
        req: &fandhe_device::compose_mail_sms::ComposeRequest,
    ) -> Result<ComposeOutcome, DeviceError> {
        outcome_from_shell(open_url(&req.to_url()?))
    }
}

/// `ShellExecuteW` を呼んで戻り値を返す。UI（STA）スレッドから呼ぶ。
/// `windows_web_auth_session` も既定ブラウザを開くために使う。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(crate) fn open_url(url: &str) -> isize {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let url = HSTRING::from(url);
    // SAFETY: `url` は NUL 終端の UTF-16 で、この呼び出しの間だけ生きる。`w!("open")` は
    // 静的な NUL 終端文字列。`ShellExecuteW` は引数を保持せず、UI スレッドから同期に呼ぶ。
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &url,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    result.0 as isize
}

/// Windows の `ComposeMailSms` 実装を返す（`device/compose_mail_sms.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::compose_mail_sms::ComposeMailSms> {
    Box::new(WindowsComposeMailSms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_code_table() {
        assert_eq!(outcome_from_shell(33).unwrap(), ComposeOutcome::HandedOff);
        assert_eq!(outcome_from_shell(31).unwrap(), ComposeOutcome::NoHandler);
        for c in [0, 2, 5, 32, -1, isize::MIN] {
            assert!(outcome_from_shell(c).is_err(), "{c}");
        }
    }
}
