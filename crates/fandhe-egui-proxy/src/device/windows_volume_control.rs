//! Windows 向けシステム音量実装（Issue #785）。Core Audio の MMDevice API で既定の出力
//! （`GetDefaultAudioEndpoint(eRender, eConsole)`）を毎回 OS へ問い、`IAudioEndpointVolume` の
//! `GetMasterVolumeLevelScalar` / `GetMute` で読み、`VolumeStepUp` / `VolumeStepDown` で 1 段階
//! 動かして同じ呼び出しの中で読み戻す。`device/volume_control.rs` の Windows 経路
//! （ワーカー上、5 秒の期限つき）から呼ばれ、契約は `fandhe_device::volume_control`。
//!
//! # スレッドと COM
//!
//! UI スレッドは winit が `OleInitialize`（STA）済みで、そこで MTA の `CoInitializeEx` を呼ぶと
//! `RPC_E_CHANGED_MODE` になる。このためワーカーで `ComInit`（MTA、RAII）を使う。中断できない
//! COM 呼び出しが戻らない場合は呼び出し側の期限で失敗を確定し、残った呼び出しはプロセス終了で
//! OS が片付ける。COM 参照は `windows` クレートの `Drop` が全経路で解放する。
//!
//! # 取らないもの
//!
//! エンドポイント ID・機器名は読まない・保持しない。エラーは HRESULT の値を載せず固定の
//! ラベルで返す（security.md A09）。
//!
//! # 既知の天井
//!
//! - 段の幅は OS が決める（`VolumeStepUp/Down`）。上限・下限では `S_OK` のまま値が変わらず、
//!   読み戻しの比較で `Unchanged` になる
//! - 音量は聴感に合わせた 0.0〜1.0 の正規化値。機器ごとの曲線の違いは守らない範囲
//! - 実機での動作確認は #780
//!
//! # unsafe
//!
//! COM 呼び出しに閉じ、各呼び出しの直前に `// SAFETY:` を付ける（`windows_audio_devices.rs` と
//! 同型。`lib.rs` の Windows `deny(unsafe_code)` は関数単位の `allow` で緩める）。

use std::sync::Arc;

use fandhe_device::volume_control::{
    classify_step, validate_level, StepDirection, StepOutcome, VolumeControl, VolumeControlMode,
    VolumeReading,
};
use fandhe_device::DeviceError;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use super::windows_notification::ComInit;

/// 出力機器が無いときの `GetDefaultAudioEndpoint` の戻り（`E_NOTFOUND`）。
const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;

struct WindowsVolumeControl;

/// 既定の出力の音量インターフェース。出力機器が無ければ `Ok(None)`（失敗ではない）。
#[allow(unsafe_code)]
fn default_endpoint_volume() -> Result<Option<IAudioEndpointVolume>, DeviceError> {
    // SAFETY: COM を初期化済みのスレッドで、クラス ID とインターフェースは windows クレートの定義。
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|_| DeviceError::Internal("volume-control enumerator"))?;
    // SAFETY: `enumerator` は有効な COM 参照。既定の出力機器は毎回 OS へ問い、ID を保持しない。
    let device = match unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) } {
        Ok(d) => d,
        Err(e) if e.code().0 == E_NOTFOUND => return Ok(None),
        Err(_) => return Err(DeviceError::Internal("volume-control endpoint")),
    };
    // SAFETY: `device` は有効な COM 参照。
    let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|_| DeviceError::Internal("volume-control activate"))?;
    Ok(Some(volume))
}

/// 音量インターフェースから現在の値を読む。
#[allow(unsafe_code)]
fn read_from(volume: &IAudioEndpointVolume) -> Result<VolumeReading, DeviceError> {
    // SAFETY: `volume` は有効な COM 参照。
    let raw = unsafe { volume.GetMasterVolumeLevelScalar() }
        .map_err(|_| DeviceError::Internal("volume-control level"))?;
    // SAFETY: `volume` は有効な COM 参照。
    let muted = unsafe { volume.GetMute() }
        .map_err(|_| DeviceError::Internal("volume-control mute"))?
        .as_bool();
    Ok(VolumeReading {
        // 丸め・変換の前に値域を検証する。
        level: Some(validate_level(raw)?),
        muted: Some(muted),
        mode: VolumeControlMode::Steps,
    })
}

/// 出力機器が無いときの読み取り（失敗ではなく「変えられない」）。
fn no_device() -> VolumeReading {
    VolumeReading {
        level: None,
        muted: None,
        mode: VolumeControlMode::NotChangeable,
    }
}

impl VolumeControl for WindowsVolumeControl {
    fn read(&self) -> Result<VolumeReading, DeviceError> {
        let _com = ComInit::new("volume-control read")?;
        match default_endpoint_volume()? {
            Some(v) => read_from(&v),
            None => Ok(no_device()),
        }
    }

    #[allow(unsafe_code)]
    fn step(&self, direction: StepDirection) -> Result<StepOutcome, DeviceError> {
        let _com = ComInit::new("volume-control step")?;
        let Some(volume) = default_endpoint_volume()? else {
            return Ok(StepOutcome::NotChangeable(no_device()));
        };
        let before = read_from(&volume)?;
        // SAFETY: `volume` は有効な COM 参照。イベントコンテキストは使わないので null を渡す。
        let stepped = unsafe {
            match direction {
                StepDirection::Up => volume.VolumeStepUp(std::ptr::null()),
                StepDirection::Down => volume.VolumeStepDown(std::ptr::null()),
            }
        };
        stepped.map_err(|_| DeviceError::Internal("volume-control step"))?;
        // 同じ呼び出しの中で読み戻した値で結果を決める（上限・下限では値が変わらず `Unchanged`）。
        Ok(classify_step(&before, read_from(&volume)?))
    }
}

/// Windows の `VolumeControl` 実装を返す（`device/volume_control.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn VolumeControl> {
    Arc::new(WindowsVolumeControl)
}
