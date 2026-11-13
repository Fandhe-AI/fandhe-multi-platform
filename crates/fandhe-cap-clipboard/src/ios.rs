//! iOS 向けクリップボード実装（`UIPasteboard`、Issue #859）。`driver` の `platform_clipboard` が
//! iOS 経路で [`IosClipboard`] を返し、`ios/fandhe_ios_clipboard.m`（UIKit の `UIPasteboard`）を `unsafe extern "C"`
//! 3 関数で呼ぶ。ボタン押下時のみ呼ばれる（毎フレームではない）。
//!
//! `UIPasteboard` は UIKit の main スレッド前提 API のため、FFI 呼び出し前に `fandhe_platform_apple::is_main_thread` で
//! 検査し、main スレッド以外では `Internal` を返して unsafe な前提を呼び出し側へ漏らさない。
//! 内容・エラーはログに出さない（security.md A09）。`unsafe` はこの FFI 宣言・呼び出しに閉じ、各所に `// SAFETY:` を書く。

use crate::contract::Clipboard;
use crate::error::ClipboardError;

// main thread 判定は Apple 共通層（`fandhe-platform-apple`）が持つ（codex-review 指摘 P1、PR #844）。
#[cfg(target_os = "ios")]
use fandhe_platform_apple::is_main_thread;
/// ホストのテストでは main スレッド判定を使わない（FFI は iOS ターゲットでしか載らない）。
#[cfg(not(target_os = "ios"))]
#[allow(dead_code)]
fn is_main_thread() -> bool {
    false
}

// SAFETY: `ios/fandhe_ios_clipboard.m` が定義する C ABI（`build.rs` が iOS ターゲットのみコンパイル・リンクする）。
// 契約は下記 `IosClipboard` の各メソッドのコメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_clipboard_read(out_text: *mut *mut core::ffi::c_char, out_len: *mut usize)
    -> i32;
    fn fandhe_ios_clipboard_free(ptr: *mut core::ffi::c_char);
    fn fandhe_ios_clipboard_write(text: *const core::ffi::c_char) -> i32;
}

/// `fandhe_ios_clipboard_read` が確保したバッファを解放する（`NULL` 許容）。
/// `unsafe` を 1 箇所へ集約するための小関数。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
fn free_clipboard_buffer(ptr: *mut core::ffi::c_char) {
    // SAFETY: `fandhe_ios_clipboard_free` は NULL でも安全（`.m` 側は `free(NULL)` に
    // 委譲するのみ）。非 NULL の場合は `fandhe_ios_clipboard_read` が返した malloc
    // 領域を指す前提で、呼び出し元はこの呼び出し後に `ptr` を再利用しない契約にする。
    unsafe { fandhe_ios_clipboard_free(ptr) };
}

/// iOS 実クリップボード実装（`UIPasteboard`、Issue #859）。
pub struct IosClipboard;

#[cfg(target_os = "ios")]
impl Clipboard for IosClipboard {
    #[allow(unsafe_code)]
    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        if !is_main_thread() {
            return Err(ClipboardError::Internal(
                "clipboard: called off main thread",
            ));
        }

        let mut out_text: *mut core::ffi::c_char = std::ptr::null_mut();
        let mut out_len: usize = 0;

        // SAFETY: `fandhe_ios_clipboard_read` の契約（`.m` ファイル冒頭コメント）:
        // 戻り値 0 のときのみ `out_text` が `out_len` バイトの UTF-8 バイト列を指す
        // malloc 領域へ書き換えられ（内容に NUL を含み得るが末尾にも NUL が置かれる）、
        // 所有権は呼び出し側（本関数）へ移る。0 以外では両 out 引数とも書き換えられない
        // （このスコープ内で初期値のまま扱う）。引数は上記でスタック上に確保済みの
        // 有効なポインタで、呼び出し中に他スレッドが書き換えることはない。
        let code = unsafe { fandhe_ios_clipboard_read(&mut out_text, &mut out_len) };

        match code {
            0 => {}
            // 1 = テキスト不在（空・許可拒否。`.m` の契約）。異常ではないため `None`。
            1 => return Ok(None),
            // 2 = 内部エラー（UTF-8 変換・malloc 失敗）。未知のコードも安全側で同様に扱う。
            _ => return Err(ClipboardError::Internal("clipboard read")),
        }

        if out_text.is_null() {
            // 契約違反（0 を返したのにポインタが NULL）を検出した経路。
            return Err(ClipboardError::Internal("clipboard read"));
        }

        // SAFETY: 戻り値 0 の契約により `out_text` は `out_len` バイトの有効な読み取り
        // 可能領域を指す。`from_raw_parts` はコピーを取らないため、`to_vec` で所有
        // バイト列へ写した後に [`free_clipboard_buffer`] で ObjC 側の所有権を解放する
        // （以降 `out_text` は再利用しない）。`strlen` ではなく `out_len` を使うため、
        // 埋め込み NUL があっても途中で切り詰めない（codex-review 指摘 P1、PR #860）。
        let bytes = unsafe { std::slice::from_raw_parts(out_text.cast::<u8>(), out_len) }.to_vec();
        free_clipboard_buffer(out_text);

        // 内部 NUL を含む文字列は受け付けない。書き込み側が `CString::new` で同じ理由
        // （C 文字列にできない）から弾いており、読み書きで判定を揃える
        // （codex-review 指摘 P1、PR #860。黙って切り詰めるより明示的に失敗させる）。
        if bytes.contains(&0) {
            return Err(ClipboardError::Internal("clipboard read: embedded nul"));
        }

        match String::from_utf8(bytes) {
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            // `.m` 側が UTF-8 を保証する契約だが、破損していた場合は内容を
            // 推測せずエラーにする（security.md A03: 検証できない入力を通さない）。
            Err(_) => Err(ClipboardError::Internal("clipboard read")),
        }
    }

    #[allow(unsafe_code)]
    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        if !is_main_thread() {
            return Err(ClipboardError::Internal(
                "clipboard: called off main thread",
            ));
        }

        // 内部 NUL を含む文字列は C 文字列へできない（`fandhe-core` 側の入力欄からは
        // 到達しないが、契約として弾く）。
        let Ok(c_text) = std::ffi::CString::new(text) else {
            return Err(ClipboardError::Internal("clipboard write"));
        };

        // SAFETY: `fandhe_ios_clipboard_write` の契約（`.m` ファイル冒頭コメント）:
        // 引数は NUL 終端の UTF-8 文字列へのポインタで、ObjC 側は内部でコピーを
        // 取るため本関数の戻り後に `c_text` を解放してよい（所有権は移らない）。
        // `c_text` はこの呼び出しの間スコープ内で生存する。
        let code = unsafe { fandhe_ios_clipboard_write(c_text.as_ptr()) };

        match code {
            0 => Ok(()),
            _ => Err(ClipboardError::Internal("clipboard write")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ホスト（iOS 以外）では FFI を持たないため、型だけが `Send + Sync` の契約を満たすことを確かめる。
    #[test]
    fn ios_clipboard_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IosClipboard>();
        let _ = ClipboardError::Unsupported;
        let _: Option<&dyn Clipboard> = None;
    }
}
