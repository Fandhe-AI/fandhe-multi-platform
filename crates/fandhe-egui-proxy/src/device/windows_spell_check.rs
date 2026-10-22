//! Windows 向けスペルチェック実装（Issue #581、親 #119）。OS 標準の Spell Checking API
//! （`ISpellCheckerFactory` / `ISpellChecker` / `IEnumSpellingError`、spellcheck.h、Windows 8+、
//! package identity 不要）を呼ぶ。`device/spell_check.rs` の Windows 経路（ドライバのワーカー
//! スレッド）から呼ばれ、契約は `fandhe_device::spell_check`。dispatch・画面・registry は #578 で
//! 最終形になっており、本ファイルでは触らない。
//!
//! 流れ: MTA で COM 初期化 → Factory 生成 → `SupportedLanguages` で辞書の有無を確認し
//! `resolve_language` で解決（`None` なら `Internal(LANGUAGE_UNAVAILABLE)`。「誤り 0 件」にしない）
//! → `Check` で誤りを列挙 → 誤りごとに [`classify`] で扱いを決めて候補を集める →
//! `build_report`（UTF-16 単位の範囲を `char` 単位へ直す）。OS が返す `PWSTR` はどの経路でも
//! `CoTaskStr` が `CoTaskMemFree` で 1 回だけ解放する。COM 参照は windows-rs の `Drop` が解放する。
//!
//! `unsafe` は COM 呼び出しの関数に閉じ、各ブロックに `// SAFETY:` を付ける。
//!
//! # 守る範囲 / 守らない範囲
//!
//! - 守る: 辞書の無い言語・OS の失敗を成功や誤り 0 件にしない、COM 参照と CoTaskMem 文字列の全経路での
//!   解放、`ComInit` の対称な uninit、端末のユーザー辞書（`Add` / `Ignore` / `AutoCorrect`）を変えない、
//!   入力・語・候補をログに出さない、経過時間を単調時計で測り誤りごとに期限を確認する
//! - 守らない: 重複語（`CORRECTIVE_ACTION_DELETE`）の検出、期限切れ後も走り始めた `Check` / `Suggest`
//!   （out-of-proc の RPC）が走り切ること（中断できない。結果はドライバが捨てる）、利用者が OS に学習
//!   させた語による結果の違い

#[cfg(target_os = "windows")]
use std::sync::Arc;
#[cfg(any(target_os = "windows", test))]
use std::time::Instant;

#[cfg(any(target_os = "windows", test))]
use fandhe_device::spell_check::TIMEOUT;
#[cfg(target_os = "windows")]
use fandhe_device::spell_check::{build_report, SpellChecker, SpellReport};
#[cfg(any(target_os = "windows", test))]
use fandhe_device::DeviceError;

/// `CORRECTIVE_ACTION` の値（spellcheck.h）。
#[cfg(any(target_os = "windows", test))]
const ACTION_GET_SUGGESTIONS: i32 = 1;
#[cfg(any(target_os = "windows", test))]
const ACTION_REPLACE: i32 = 2;

/// `REGDB_E_CLASSNOTREG`（クラス未登録。Spell Checking API が無い環境）。
#[cfg(any(target_os = "windows", test))]
const HR_CLASS_NOT_REGISTERED: i32 = 0x8004_0154_u32 as i32;

/// 言語一覧の読み取り上限（無限列挙の防止）。
#[cfg(any(target_os = "windows", test))]
const MAX_LANGUAGES: usize = 256;

/// 誤り 1 件の扱い。
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    /// `Suggest` で候補を取る。
    Suggest,
    /// `Replacement()` を候補 1 件にする。
    Replace,
    /// 飛ばす（重複語の削除・何もしない・未知の値）。
    Skip,
}

/// `CorrectiveAction` の値から扱いを決める。DELETE（重複語）は範囲外のため `Skip`。
#[cfg(any(target_os = "windows", test))]
fn classify(action: i32) -> Kind {
    match action {
        ACTION_GET_SUGGESTIONS => Kind::Suggest,
        ACTION_REPLACE => Kind::Replace,
        _ => Kind::Skip,
    }
}

/// Factory 生成の HRESULT を契約の失敗へ写す。クラス未登録は `Unsupported`、それ以外は `Internal`。
#[cfg(any(target_os = "windows", test))]
fn map_factory_error(hr: i32) -> DeviceError {
    if hr == HR_CLASS_NOT_REGISTERED {
        DeviceError::Unsupported("spell_check")
    } else {
        DeviceError::Internal("spell_check")
    }
}

/// UTF-16 列から `[start, start + len)` を切り出す。範囲外・長さ 0 は `None`。
#[cfg(any(target_os = "windows", test))]
fn utf16_slice(wide: &[u16], start: usize, len: usize) -> Option<&[u16]> {
    if len == 0 {
        return None;
    }
    let end = start.checked_add(len)?;
    wide.get(start..end)
}

/// 期限（単調時計）を過ぎたか。
#[cfg(any(target_os = "windows", test))]
fn past(deadline_at: Instant, now: Instant) -> bool {
    now >= deadline_at
}

/// 誤りの収集を止める件数に達したか（`build_report` が切り詰めて `truncated` を立てるよう 1 件多く集める）。
#[cfg(any(target_os = "windows", test))]
fn issues_full(collected: usize, max_issues: usize) -> bool {
    collected > max_issues
}

/// 期限切れを `Internal(TIMEOUT)` にする。
#[cfg(any(target_os = "windows", test))]
fn check_deadline(deadline_at: Instant) -> Result<(), DeviceError> {
    if past(deadline_at, Instant::now()) {
        Err(DeviceError::Internal(TIMEOUT))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod os {
    use std::ffi::c_void;
    use std::time::Instant;

    use fandhe_device::spell_check::{
        resolve_language, RawIssue, LANGUAGE_UNAVAILABLE, MAX_ISSUES, MAX_SUGGESTIONS,
    };
    use fandhe_device::DeviceError;
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::S_OK;
    use windows::Win32::Globalization::{ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoTaskMemFree, IEnumString, CLSCTX_INPROC_SERVER,
    };

    use super::{
        check_deadline, classify, issues_full, map_factory_error, utf16_slice, Kind, MAX_LANGUAGES,
    };

    const E_GENERIC: DeviceError = DeviceError::Internal("spell_check");

    /// OS が `CoTaskMemAlloc` で確保した `PWSTR` の RAII ガード。`Drop` で必ず 1 回解放する。
    struct CoTaskStr(PWSTR);

    impl CoTaskStr {
        /// UTF-16 から `String` へ。変換に失敗してもガードの `Drop` で解放される。
        #[allow(unsafe_code)]
        fn to_string(&self) -> Option<String> {
            if self.0.is_null() {
                return None;
            }
            // SAFETY: `self.0` は OS が返した NUL 終端の有効な文字列で、ガードの生存中は解放されない。
            unsafe { self.0.to_string().ok() }
        }
    }

    impl Drop for CoTaskStr {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` は CoTaskMem で確保された文字列で、このガードだけが所有し、
                // `Drop` は 1 度しか走らないため二重解放にならない。
                unsafe { CoTaskMemFree(Some(self.0 .0 as *const c_void)) };
            }
        }
    }

    /// `IEnumString` を最大 `limit` 件読む。失敗は `Internal`。文字列は件ごとに解放する。
    #[allow(unsafe_code)]
    fn read_enum_string(en: &IEnumString, limit: usize) -> Result<Vec<String>, DeviceError> {
        let mut out = Vec::new();
        while out.len() < limit {
            let mut slot = [PWSTR::null()];
            let mut fetched = 0u32;
            // SAFETY: `slot`・`fetched` は呼び出しの間有効なローカル領域で、`en` は有効な COM 参照。
            let hr = unsafe { en.Next(&mut slot, Some(&mut fetched)) };
            if hr.is_err() {
                return Err(E_GENERIC);
            }
            if hr != S_OK || fetched != 1 {
                break;
            }
            // 受け取った時点でガードに包む（変換失敗でも解放が走る）。
            let guard = CoTaskStr(slot[0]);
            if let Some(s) = guard.to_string() {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// NUL 終端の UTF-16 にする。
    fn nul_terminated(s: &[u16]) -> Vec<u16> {
        let mut v = Vec::with_capacity(s.len() + 1);
        v.extend_from_slice(s);
        v.push(0);
        v
    }

    /// 検査の本体。`(解決した言語, 生の誤り)` を返す。`text` は検証済み（NUL なし）。
    #[allow(unsafe_code)]
    pub(super) fn run(
        text: &str,
        language: &str,
        deadline_at: Instant,
    ) -> Result<(String, Vec<RawIssue>), DeviceError> {
        // SAFETY: CLSID は windows crate の定義で、戻りは `ISpellCheckerFactory` として取得する。
        let factory: ISpellCheckerFactory =
            unsafe { CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| map_factory_error(e.code().0))?;

        // SAFETY: `factory` は有効な COM 参照。
        let langs_enum = unsafe { factory.SupportedLanguages() }.map_err(|_| E_GENERIC)?;
        let langs = read_enum_string(&langs_enum, MAX_LANGUAGES)?;
        // 辞書の無い言語を「誤り 0 件」にしない。
        let resolved = resolve_language(language, &langs)
            .ok_or(DeviceError::Internal(LANGUAGE_UNAVAILABLE))?;
        check_deadline(deadline_at)?;

        let tag = HSTRING::from(resolved.as_str());
        // SAFETY: `tag` は呼び出しの間生きている NUL 終端の HSTRING。
        let checker: ISpellChecker =
            unsafe { factory.CreateSpellChecker(&tag) }.map_err(|_| E_GENERIC)?;

        let wide: Vec<u16> = text.encode_utf16().collect();
        let wide_nul = nul_terminated(&wide);
        // SAFETY: `wide_nul` は呼び出しの間生きている NUL 終端のバッファ。
        let errors = unsafe { checker.Check(PCWSTR(wide_nul.as_ptr())) }.map_err(|_| E_GENERIC)?;

        let mut raw = Vec::new();
        loop {
            check_deadline(deadline_at)?;
            let mut item = None;
            // SAFETY: `item` は呼び出しの間有効な出力先。
            let hr = unsafe { errors.Next(&mut item) };
            if hr.is_err() {
                return Err(E_GENERIC);
            }
            let Some(err) = item else { break };
            if hr != S_OK {
                break;
            }
            // SAFETY: `err` は有効な COM 参照。
            let (start, len, action) = unsafe {
                (
                    err.StartIndex().map_err(|_| E_GENERIC)?,
                    err.Length().map_err(|_| E_GENERIC)?,
                    err.CorrectiveAction().map_err(|_| E_GENERIC)?,
                )
            };
            let suggestions = match classify(action.0) {
                Kind::Skip => continue,
                Kind::Replace => {
                    // SAFETY: `err` は有効な COM 参照。返る文字列はガードが解放する。
                    let p = unsafe { err.Replacement() }.map_err(|_| E_GENERIC)?;
                    let guard = CoTaskStr(p);
                    guard.to_string().into_iter().collect()
                }
                Kind::Suggest => {
                    let word = utf16_slice(&wide, start as usize, len as usize)
                        .ok_or(DeviceError::Internal("spell_check range"))?;
                    let word_nul = nul_terminated(word);
                    // SAFETY: `word_nul` は呼び出しの間生きている NUL 終端のバッファ。
                    let en = unsafe { checker.Suggest(PCWSTR(word_nul.as_ptr())) }
                        .map_err(|_| E_GENERIC)?;
                    read_enum_string(&en, MAX_SUGGESTIONS * 2)?
                }
            };
            raw.push(RawIssue {
                utf16_start: start as usize,
                utf16_len: len as usize,
                suggestions,
            });
            if issues_full(raw.len(), MAX_ISSUES) {
                break;
            }
        }
        Ok((resolved, raw))
    }
}

#[cfg(target_os = "windows")]
struct WindowsSpellChecker;

#[cfg(target_os = "windows")]
impl SpellChecker for WindowsSpellChecker {
    fn check(
        &self,
        text: &str,
        language: &str,
        remaining: std::time::Duration,
    ) -> Result<SpellReport, DeviceError> {
        let deadline_at = Instant::now() + remaining;
        // ワーカースレッドで MTA を初期化する（Drop で対称に uninit）。
        let _com = super::windows_notification::ComInit::new("spell_check")?;
        let (resolved, raw) = os::run(text, language, deadline_at)?;
        build_report(text, &resolved, raw, false)
    }
}

/// Windows の `SpellChecker` 実装を返す（`device/spell_check.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn SpellChecker> {
    Arc::new(WindowsSpellChecker)
}

/// 非 Windows ビルド用の入口（`device.rs` の `pub mod` が OS を問わず置かれているため残す。呼ばれない）。
#[cfg(not(target_os = "windows"))]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::spell_check::SpellChecker> {
    std::sync::Arc::new(fandhe_device::spell_check::UnsupportedSpellChecker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn classify_actions() {
        assert_eq!(classify(1), Kind::Suggest);
        assert_eq!(classify(2), Kind::Replace);
        assert_eq!(classify(3), Kind::Skip);
        assert_eq!(classify(0), Kind::Skip);
        assert_eq!(classify(99), Kind::Skip);
        assert_eq!(classify(-1), Kind::Skip);
    }

    #[test]
    fn utf16_slice_bounds() {
        let w: Vec<u16> = "hello".encode_utf16().collect();
        assert_eq!(utf16_slice(&w, 0, 5).map(|s| s.len()), Some(5));
        assert_eq!(utf16_slice(&w, 1, 2).map(|s| s.len()), Some(2));
        assert!(utf16_slice(&w, 0, 0).is_none());
        assert!(utf16_slice(&w, 5, 1).is_none());
        assert!(utf16_slice(&w, 3, 3).is_none());
        assert!(utf16_slice(&w, usize::MAX, 2).is_none());
    }

    #[test]
    fn factory_error_mapping() {
        assert!(matches!(
            map_factory_error(HR_CLASS_NOT_REGISTERED),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            map_factory_error(0x8000_4005_u32 as i32),
            DeviceError::Internal(_)
        ));
    }

    #[test]
    fn deadline_and_cap() {
        let now = Instant::now();
        assert!(past(now, now));
        assert!(!past(now + Duration::from_secs(5), now));
        assert!(matches!(
            check_deadline(now),
            Err(DeviceError::Internal(TIMEOUT))
        ));
        assert!(check_deadline(now + Duration::from_secs(60)).is_ok());
        assert!(!issues_full(50, 50));
        assert!(issues_full(51, 50));
        assert_eq!(MAX_LANGUAGES, 256);
    }
}
