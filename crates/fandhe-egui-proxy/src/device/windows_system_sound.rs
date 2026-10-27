//! Windows 向けシステムの効果音実装（Issue #939）。`device/system_sound.rs` の Windows 経路から
//! 呼ばれ、`PlaySoundW` を同期で 1 回呼ぶ（スレッド制約なし、package identity 不要）。契約は
//! `fandhe_device::system_sound`。
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

use fandhe_device::system_sound::{BeepOutcome, SystemSound};
use fandhe_device::DeviceError;
use windows::core::w;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT, SND_SYSTEM};

struct WindowsSystemSound;

impl SystemSound for WindowsSystemSound {
    #[allow(unsafe_code)]
    fn beep(&self) -> Result<BeepOutcome, DeviceError> {
        // SAFETY: `w!` は静的な NUL 終端 UTF-16 で、モジュールハンドルは None。`SND_ALIAS` なので
        // 名前はレジストリの別名として解決され、呼び出し側が渡すバッファは無い。
        let ok = unsafe {
            PlaySoundW(
                w!("SystemDefault"),
                None,
                SND_ALIAS | SND_ASYNC | SND_NODEFAULT | SND_SYSTEM,
            )
        };
        Ok(super::system_sound::outcome_from_play_sound(ok.as_bool()))
    }
}

/// Windows の `SystemSound` 実装を返す（`device/system_sound.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn SystemSound> {
    Arc::new(WindowsSystemSound)
}
