//! iOS 向けスペルチェック実装（Issue #579、親 #119）。`ios/fandhe_ios_spell_check.m`
//! （UIKit `UITextChecker`。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルする）を
//! `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` は [`ffi`] に閉じ `// SAFETY:` で契約を明記する。
//! `device/spell_check.rs` の iOS 経路（ワーカースレッド）から呼ばれ、契約は
//! `fandhe_device::spell_check`。権限・Info.plist・entitlement は不要。
//!
//! 流れ: 利用可能な言語の一覧を取る → [`resolve_language`] で解決（`None` なら
//! `Internal(LANGUAGE_UNAVAILABLE)`。「誤り 0 件」にしない）→ 検査を呼ぶ → バッファを
//! `parse_apple_issue_buffer` で解釈（macOS と共用）→ [`build_report`] で組み立てる。`.m` は
//! main queue で 1 ステップずつ動き、期限（`remaining`）を超えたら Rust のバッファへ書かずに戻る。
//!
//! 再描画（契約表 4）: OS のコールバックが Rust の共有状態へ書く経路は無く、結果を置くのは
//! ワーカー（`spell_check.rs` の `slot.deliver`）だけ。待っている間は `poll_and_finish` が毎フレーム
//! `request_repaint_after` を予約するので画面は止まらない。念のため検査から戻った直後にも
//! `crate::ios::request_repaint` を呼ぶ。
//!
//! ライフサイクル: 背景遷移で開始済みのステップは main で走り切る。復帰後に期限を過ぎていれば
//! ドライバが `Timeout` で確定し、遅れて届いた結果は世代照合で捨てる。
//!
//! 戻り値コードの写像は [`map_rc`]（純関数、ホストでテストする。macOS 側と同内容の複製）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: `UITextChecker` の全経路解放（`.m`）、学習語・無視語を変えない、入力・語・候補をログに出さない、
//!   バッファ不足・範囲異常を成功にしない
//! - 守らない: 期限切れ後に OS 側の検査が走り切ること、利用者が OS に学習させた語による結果の違い
//!
//! シミュレータ・実機での正常系と失敗系（辞書の無い言語）の確認は #574 で行う。

#[cfg(target_os = "ios")]
use std::sync::Arc;
#[cfg(any(target_os = "ios", test))]
use std::time::Duration;

#[cfg(target_os = "ios")]
use fandhe_device::spell_check::LANGUAGE_UNAVAILABLE;
#[cfg(any(target_os = "ios", test))]
use fandhe_device::spell_check::TIMEOUT;
#[cfg(target_os = "ios")]
use fandhe_device::spell_check::{
    build_report, resolve_language, SpellChecker, SpellReport, MAX_ISSUES, MAX_SUGGESTIONS,
};
#[cfg(any(target_os = "ios", test))]
use fandhe_device::DeviceError;

// `.m` の FANDHE_SPELL_RC_*。
#[cfg(any(target_os = "ios", test))]
const RC_INVALID_INPUT: i32 = 2;
#[cfg(any(target_os = "ios", test))]
const RC_NO_CHECKER: i32 = 3;
#[cfg(any(target_os = "ios", test))]
const RC_TIMEOUT: i32 = 4;

/// `.m` の戻り値コードを契約の失敗へ写す（0 は成功）。チェッカー無効は `Unsupported`、期限切れは
/// `Internal(TIMEOUT)`、入力不正は `InvalidInput`、バッファ不足・範囲異常・その他は `Internal`。
#[cfg(any(target_os = "ios", test))]
fn map_rc(rc: i32) -> Result<(), DeviceError> {
    match rc {
        0 => Ok(()),
        RC_INVALID_INPUT => Err(DeviceError::InvalidInput("spell_check text")),
        RC_NO_CHECKER => Err(DeviceError::Unsupported("spell_check")),
        RC_TIMEOUT => Err(DeviceError::Internal(TIMEOUT)),
        5 => Err(DeviceError::Internal("spell_check buffer")),
        6 => Err(DeviceError::Internal("spell_check range")),
        _ => Err(DeviceError::Internal("spell_check")),
    }
}

/// 残り時間を `.m` へ渡すミリ秒にする（0 以下にならないよう下限 1、上限は `i32::MAX`）。
#[cfg(any(target_os = "ios", test))]
fn timeout_ms(remaining: Duration) -> i32 {
    remaining.as_millis().clamp(1, i32::MAX as u128) as i32
}

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `macos/fandhe_ios_spell_check.m`（`build.rs` が macOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_spell_languages(
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            timeout_ms: i32,
        ) -> i32;
        fn fandhe_ios_spell_check(
            text: *const u8,
            text_len: usize,
            lang: *const u8,
            lang_len: usize,
            max_issues: u32,
            max_suggestions: u32,
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            out_truncated: *mut i32,
            timeout_ms: i32,
        ) -> i32;
    }

    /// 言語一覧。`(戻り値コード, 書いたバイト数, 件数)`。`len <= buf.len()` を検査して返す。
    #[allow(unsafe_code)]
    pub(super) fn languages(buf: &mut [u8], timeout_ms: i32) -> (i32, usize, u32) {
        let (mut len, mut count) = (0usize, 0u32);
        // SAFETY: `buf` は `cap` バイトの有効な排他領域、残りは呼び出しの間有効なローカル変数への
        // 排他ポインタで、`.m` はどれも保持しない。
        let rc = unsafe {
            fandhe_ios_spell_languages(
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                timeout_ms,
            )
        };
        if rc == 0 && len > buf.len() {
            return (7, 0, 0);
        }
        (rc, len, count)
    }

    /// 検査。`(戻り値コード, 書いたバイト数, 件数, OS 側で打ち切ったか)`。
    #[allow(unsafe_code)]
    pub(super) fn check(
        text: &str,
        lang: &str,
        max_issues: u32,
        max_suggestions: u32,
        buf: &mut [u8],
        timeout_ms: i32,
    ) -> (i32, usize, u32, bool) {
        let (mut len, mut count, mut truncated) = (0usize, 0u32, 0i32);
        // SAFETY: `text`・`lang` は長さ付きの有効な読み取り領域（NUL 終端は要しない）、`buf` は `cap`
        // バイトの有効な排他領域、残りは呼び出しの間有効なローカル変数への排他ポインタ。`.m` は
        // どれも保持せず、期限切れの後に `buf` へ書かない（結果は `.m` 側のヒープに集めてから写す）。
        let rc = unsafe {
            fandhe_ios_spell_check(
                text.as_ptr(),
                text.len(),
                lang.as_ptr(),
                lang.len(),
                max_issues,
                max_suggestions,
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                &mut truncated,
                timeout_ms,
            )
        };
        if rc == 0 && len > buf.len() {
            return (7, 0, 0, false);
        }
        (rc, len, count, truncated != 0)
    }
}

/// 言語一覧の受け取りバッファのバイト数。
#[cfg(target_os = "ios")]
const LANGUAGES_BUF_BYTES: usize = 16 * 1024;
/// 検査結果の受け取りバッファのバイト数（誤り 50 件 × 候補 5 件 × 候補 256 バイトが収まる大きさ）。
#[cfg(target_os = "ios")]
const ISSUES_BUF_BYTES: usize = 128 * 1024;

#[cfg(target_os = "ios")]
struct IosSpellChecker;

#[cfg(target_os = "ios")]
impl SpellChecker for IosSpellChecker {
    fn check(
        &self,
        text: &str,
        language: &str,
        remaining: Duration,
    ) -> Result<SpellReport, DeviceError> {
        let started = std::time::Instant::now();

        let mut lbuf = vec![0u8; LANGUAGES_BUF_BYTES];
        let (rc, len, count) = ffi::languages(&mut lbuf, timeout_ms(remaining));
        map_rc(rc)?;
        let available = super::spell_check::languages_from_nul_buffer(&lbuf[..len], count as usize)
            .ok_or(DeviceError::Internal("spell_check languages"))?;
        // 辞書の無い言語を「誤り 0 件」にしない。
        let resolved = resolve_language(language, &available)
            .ok_or(DeviceError::Internal(LANGUAGE_UNAVAILABLE))?;

        let left = remaining.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(DeviceError::Internal(TIMEOUT));
        }
        let mut ibuf = vec![0u8; ISSUES_BUF_BYTES];
        let (rc, len, count, os_truncated) = ffi::check(
            text,
            &resolved,
            // 上限超過を検出できるよう 1 件多く求める。
            (MAX_ISSUES + 1) as u32,
            MAX_SUGGESTIONS as u32,
            &mut ibuf,
            timeout_ms(left),
        );
        // 待っている間は `poll_and_finish` が次フレームを予約するが、検査から戻った直後にも起こす。
        crate::ios::request_repaint();
        map_rc(rc)?;
        let raw = super::spell_check::parse_apple_issue_buffer(&ibuf[..len], count as usize)
            .ok_or(DeviceError::Internal("spell_check buffer"))?;
        build_report(text, &resolved, raw, os_truncated)
    }
}

/// iOS の `SpellChecker` 実装を返す（`device/spell_check.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(IosSpellChecker)
}

/// iOS 以外（ホストの `cargo test` が本モジュールをコンパイルするとき）は呼ばれない。
/// 万一呼ばれても成功を装わず `Unsupported` を返す。
#[cfg(not(target_os = "ios"))]
#[allow(dead_code)]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::spell_check::SpellChecker> {
    std::sync::Arc::new(fandhe_device::spell_check::UnsupportedSpellChecker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_mapping() {
        assert!(map_rc(0).is_ok());
        assert!(matches!(
            map_rc(RC_INVALID_INPUT),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            map_rc(RC_NO_CHECKER),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            map_rc(RC_TIMEOUT),
            Err(DeviceError::Internal(TIMEOUT))
        ));
        for rc in [1, 5, 6, 7, 99, -1] {
            assert!(matches!(map_rc(rc), Err(DeviceError::Internal(_))), "{rc}");
        }
    }

    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout_ms(Duration::ZERO), 1);
        assert_eq!(timeout_ms(Duration::from_secs(10)), 10_000);
        assert_eq!(timeout_ms(Duration::from_secs(u64::MAX / 2000)), i32::MAX);
    }
}
