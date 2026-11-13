//! Windows 向けシステムの効果音実装（Issue #939）。`driver.rs` の Windows 経路から
//! 呼ばれ、`PlaySoundW` を同期で 1 回呼ぶ（スレッド制約なし、package identity 不要）。契約は
//! `crate::contract`。
//!
//! - 名前は `SystemDefault`（`SND_ALIAS_SYSTEMDEFAULT` 定数は windows 0.61 に無いため文字列）
//! - `SND_ASYNC` は即座に戻る。`SND_NODEFAULT` は音が無いとき既定音へ倒さない。
//!   `SND_SYSTEM` はシステム通知音の音量に従う。`SND_FILENAME` は使わない（外部入力なし）
//! - `MessageBeep` は採らない（鳴らせないと既定音・`Beep` へ倒れ、成功に紛れるため）
//! - 戻り値 `TRUE` は `Requested`、`FALSE` は `NotPlayed`。`GetLastError` は見ず OS の答えをそのまま返す
//!
//! 素の exe から実際に鳴るかは公式文書で確かめていない（Windows 実機確認は #935）。
//! ローカルに MSVC が無いため型検査は `x86_64-pc-windows-gnu` で行う。

use std::sync::Arc;

use crate::contract::{BeepOutcome, SystemSound};
use crate::error::SystemSoundError;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT, SND_SYSTEM};
use windows::core::w;

struct WindowsSystemSound;

impl SystemSound for WindowsSystemSound {
    #[allow(unsafe_code)]
    fn beep(&self) -> Result<BeepOutcome, SystemSoundError> {
        // SAFETY: `w!` は静的な NUL 終端 UTF-16 で、モジュールハンドルは None。`SND_ALIAS` なので
        // 名前はレジストリの別名として解決され、呼び出し側が渡すバッファは無い。
        let ok = unsafe {
            PlaySoundW(
                w!("SystemDefault"),
                None,
                SND_ALIAS | SND_ASYNC | SND_NODEFAULT | SND_SYSTEM,
            )
        };
        Ok(crate::driver::outcome_from_play_sound(ok.as_bool()))
    }
}

/// Windows の `SystemSound` 実装を返す（`driver.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(WindowsSystemSound)
}
