//! Windows 向け音声デバイス一覧実装（Issue #1330）。Core Audio の MMDevice API
//! （`IMMDeviceEnumerator::EnumAudioEndpoints`、`DEVICE_STATE_ACTIVE`）で接続中の入力（capture）
//! と出力（render）のエンドポイントを列挙し、`PKEY_Device_FriendlyName` を名前にする。
//! `device/audio_devices.rs` の Windows 経路（`drive_worker`）からワーカースレッド上で呼ばれ、
//! 契約は `fandhe_device::audio_devices`。
//!
//! # スレッドと COM
//!
//! UI スレッドは winit が `OleInitialize`（STA）済みで、そこで MTA の `CoInitializeEx` を呼ぶと
//! `RPC_E_CHANGED_MODE` になる。このためワーカーで `ComInit`（MTA、RAII）を使う。中断できない
//! COM 呼び出しが戻らない場合は呼び出し側の 5 秒の期限で失敗を確定し、残った呼び出しは
//! プロセス終了で OS が片付ける。
//!
//! # 取らないもの
//!
//! エンドポイント ID（`IMMDevice::GetId`）は呼ばない。名前以外のプロパティも読まない。
//!
//! # unsafe
//!
//! COM 呼び出しに閉じ、各呼び出しの直前に `// SAFETY:` を付ける（`windows_notification.rs` と
//! 同型。`lib.rs` の Windows `deny(unsafe_code)` は関数単位の `allow` で緩める）。`PROPVARIANT` は
//! `Drop` を持たないため `PropVariantClear` を全経路で手動で呼ぶ。
//!
//! # 既知の天井
//!
//! - 名前は `FriendlyName`（`マイク (USB Audio Device)` のような装置名付きの表記）
//! - マイクのプライバシー設定との関係は実機で未確認（#1329）

use std::sync::Arc;

use fandhe_device::audio_devices::{
    listed, require_listed, AudioDeviceList, AudioDevices, AudioDirection, MAX_DEVICES,
};
use fandhe_device::DeviceError;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eRender, EDataFlow, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Variant::VT_LPWSTR;

use super::windows_notification::ComInit;

struct WindowsAudioDevices;

/// エンドポイントの `FriendlyName`。`VT_LPWSTR` 以外・失敗は `None`。
#[allow(unsafe_code)]
fn friendly_name(device: &IMMDevice) -> Option<String> {
    // SAFETY: `device` は有効な COM 参照。
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    // SAFETY: `store` は有効な COM 参照で、キーは静的な定数。
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    // SAFETY: `vt` が `VT_LPWSTR` のときに限り `pwszVal` は NUL 終端の UTF-16 を指し、
    // `value` が生きている間（下の `PropVariantClear` まで）有効。
    let name = unsafe {
        let inner = &value.Anonymous.Anonymous;
        (inner.vt == VT_LPWSTR)
            .then(|| inner.Anonymous.pwszVal.to_string().ok())
            .flatten()
    };
    // SAFETY: `value` は `GetValue` が初期化した有効な `PROPVARIANT`。ここで 1 度だけ解放する。
    let _ = unsafe { PropVariantClear(&mut value) };
    name
}

/// 1 方向を列挙する。列挙自体の失敗は `Unavailable`（0 件と区別する）。
#[allow(unsafe_code)]
fn direction(enumerator: &IMMDeviceEnumerator, flow: EDataFlow) -> AudioDirection {
    // SAFETY: `enumerator` は有効な COM 参照。
    let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) })
    else {
        return AudioDirection::Unavailable;
    };
    // SAFETY: `collection` は有効な COM 参照。
    let Ok(count) = (unsafe { collection.GetCount() }) else {
        return AudioDirection::Unavailable;
    };
    let n = (count as usize).min(MAX_DEVICES);
    listed((0..n).map(|i| {
        // SAFETY: `i < count` の範囲で `collection` は有効な COM 参照。
        let device = unsafe { collection.Item(i as u32) }.ok()?;
        friendly_name(&device)
    }))
}

impl AudioDevices for WindowsAudioDevices {
    #[allow(unsafe_code)]
    fn list(&self) -> Result<AudioDeviceList, DeviceError> {
        let _com = ComInit::new("audio_devices list")?;
        // SAFETY: COM を初期化済みのスレッドで、クラス ID とインターフェースは windows クレートの定義。
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|_| DeviceError::Internal("audio_devices list"))?;
        require_listed(AudioDeviceList {
            inputs: direction(&enumerator, eCapture),
            outputs: direction(&enumerator, eRender),
        })
    }
}

/// Windows の `AudioDevices` 実装を返す（`device/audio_devices.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn AudioDevices> {
    Arc::new(WindowsAudioDevices)
}
