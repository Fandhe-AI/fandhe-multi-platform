//! iOS 向けスマートホーム実装（Issue #1080、親 #147）。`ios/fandhe_ios_smart_home.m`
//! （HomeKit `HMHomeManager`。`build.rs` が iOS 分岐で feature `device` 有効時にコンパイルし HomeKit をリンクする）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しに閉じ `// SAFETY:` で契約を明記する。
//! `device/smart_home.rs` の iOS 経路からワーカースレッドで呼ばれ、契約は `fandhe_device::smart_home`。
//!
//! バッファの解釈は本ファイルの [`list_from_nul_buffer`]（ホストのテストでも使う）、戻りコードの写像は
//! `fandhe_device::smart_home::decode_code`。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 契約は `fandhe_device::smart_home` の trait doc を正とする。
//!
//! - 守る: 家と機器の名前以外を取らない。名前をログに出さない。許可と読み込みの完了が両方そろう前は
//!   読まない（生成直後の空の一覧を 0 件の成功にしない）。壊れたバッファ・未知の戻りコードは成功にせず `Failed`。
//!   期限は引数 1 つで、許可ダイアログの応答待ちを含み、期限切れでは `buf` に触れずに戻る
//! - 守らない: 読み取り後の家の構成の変化、期限切れ後に OS 側の処理が走り切ること
//!
//! # 再描画
//!
//! OS の delegate は semaphore で同じワーカーへ戻るだけで共有状態を書かない。結果を画面へ入れるのは
//! `CompletionSender::send` で、そこで `ctx.request_repaint()` が呼ばれるため
//! `crate::ios::request_repaint` は呼ばない（`ios_music_library` と同じ根拠）。
//!
//! # 未確認事項（iOS 実機確認 #1055 で確認）
//!
//! - 実機で許可ダイアログが出て、許可すると家と機器の名前が 20 件まで出ること
//! - 拒否で「許可されていません」になり 0 件にならないこと
//! - 家 0 件・機器 0 件の表示、シミュレータ（entitlement 無し）で成功にならないこと【推測】

#[cfg(target_os = "ios")]
use std::sync::Arc;
#[cfg(target_os = "ios")]
use std::time::Duration;

#[cfg(target_os = "ios")]
use fandhe_device::smart_home::{decode_code, SmartHome};
use fandhe_device::smart_home::{AccessoryList, SmartHomeFailure};

/// 受け取りバッファ。20 件 × 2 要素 × 1024 バイトが収まる。
#[cfg(target_os = "ios")]
const BUF_CAP: usize = 64 * 1024;

#[cfg(target_os = "ios")]
mod ffi {
    // SAFETY: `ios/fandhe_ios_smart_home.m`（`build.rs` が iOS 分岐で feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_smart_home_list(
            timeout_ms: u64,
            buf: *mut u8,
            cap: usize,
            out_len: *mut usize,
            out_count: *mut u32,
            out_home_count: *mut u32,
            out_overflowed: *mut i32,
        ) -> i32;
    }

    /// 戻り値の各項目。コード 0 以外のとき `len` 以降は無効。
    pub(super) struct Raw {
        pub rc: i32,
        pub len: usize,
        pub count: u32,
        pub home_count: u32,
        pub overflowed: bool,
    }

    #[allow(unsafe_code)]
    pub(super) fn list(timeout_ms: u64, buf: &mut [u8]) -> Raw {
        let (mut len, mut count, mut home_count, mut overflowed) = (0usize, 0u32, 0u32, 0i32);
        // SAFETY: `buf` は `cap` バイトの有効な排他領域、残りの 4 つは呼び出しの間有効なローカル変数への
        // 排他ポインタ。`.m` は期限切れ後に遅れて届く結果から、これらのどれにも触れず保持もしない。
        let rc = unsafe {
            fandhe_ios_smart_home_list(
                timeout_ms,
                buf.as_mut_ptr(),
                buf.len(),
                &mut len,
                &mut count,
                &mut home_count,
                &mut overflowed,
            )
        };
        Raw {
            rc,
            len,
            count,
            home_count,
            overflowed: overflowed != 0,
        }
    }
}

#[cfg(target_os = "ios")]
struct IosSmartHome;

#[cfg(target_os = "ios")]
impl SmartHome for IosSmartHome {
    fn list_accessories(&self, timeout: Duration) -> Result<AccessoryList, SmartHomeFailure> {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut buf = vec![0u8; BUF_CAP];
        let raw = ffi::list(timeout_ms, &mut buf);
        if let Some(failure) = decode_code(raw.rc) {
            return Err(failure);
        }
        // 範囲の検証は解釈の前に行う。
        if raw.len > buf.len() {
            return Err(SmartHomeFailure::Failed);
        }
        list_from_nul_buffer(&buf[..raw.len], raw.count, raw.home_count, raw.overflowed)
    }
}

/// 「home\0name\0」の組の連続を `AccessoryList` へ解釈する。末尾が NUL でない・組の数が合わない・
/// UTF-8 でないバッファは成功にせず `Failed`。空文字列は `None`。
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
fn list_from_nul_buffer(
    buf: &[u8],
    count: u32,
    home_count: u32,
    overflowed: bool,
) -> Result<AccessoryList, SmartHomeFailure> {
    let fail = SmartHomeFailure::Failed;
    let parts: Vec<&[u8]> = match buf {
        [] => Vec::new(),
        // 末尾は必ず NUL 終端。
        [rest @ .., 0] => rest.split(|b| *b == 0).collect(),
        _ => return Err(fail),
    };
    if !parts.len().is_multiple_of(2) || parts.len() / 2 != count as usize {
        return Err(fail);
    }
    let to_opt = |b: &[u8]| -> Result<Option<String>, SmartHomeFailure> {
        let s = std::str::from_utf8(b).map_err(|_| fail)?;
        Ok((!s.is_empty()).then(|| s.to_string()))
    };
    let mut rows = Vec::with_capacity(count as usize);
    for pair in parts.chunks(2) {
        rows.push((to_opt(pair[0])?, to_opt(pair[1])?));
    }
    let home_count = usize::try_from(home_count).map_err(|_| fail)?;
    let mut list = fandhe_device::smart_home::accessories_from(home_count, rows);
    list.truncated |= overflowed;
    Ok(list)
}

/// iOS の `SmartHome` 実装を返す（`device/smart_home.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn SmartHome> {
    Arc::new(IosSmartHome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_distinguishes_home_count() {
        let none = list_from_nul_buffer(b"", 0, 0, false).unwrap();
        let some = list_from_nul_buffer(b"", 0, 2, false).unwrap();
        assert_eq!((none.home_count, none.accessories.len()), (0, 0));
        assert_eq!((some.home_count, some.accessories.len()), (2, 0));
    }

    #[test]
    fn one_pair() {
        let l = list_from_nul_buffer(b"home\0lamp\0", 1, 1, false).unwrap();
        assert_eq!(l.accessories.len(), 1);
        assert_eq!(l.accessories[0].home.as_deref(), Some("home"));
        assert_eq!(l.accessories[0].name.as_deref(), Some("lamp"));
        assert!(!l.truncated);
    }

    #[test]
    fn overflow_flag_sets_truncated() {
        assert!(list_from_nul_buffer(b"", 0, 1, true).unwrap().truncated);
    }

    #[test]
    fn broken_buffers_fail() {
        let f = Err(SmartHomeFailure::Failed);
        // 末尾に NUL が無い
        assert_eq!(list_from_nul_buffer(b"a\0b", 1, 1, false).map(|_| ()), f);
        // 組の数の不一致
        assert_eq!(list_from_nul_buffer(b"a\0b\0", 2, 1, false).map(|_| ()), f);
        // 不正な UTF-8
        assert_eq!(
            list_from_nul_buffer(b"\xff\0b\0", 1, 1, false).map(|_| ()),
            f
        );
    }

    #[test]
    fn unnamed_row_is_kept_as_none() {
        let l = list_from_nul_buffer(b"\0\0", 1, 1, false).unwrap();
        assert_eq!(l.accessories.len(), 1);
        assert_eq!(l.accessories[0].home, None);
        assert_eq!(l.accessories[0].name, None);
    }
}
