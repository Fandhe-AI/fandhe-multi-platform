//! macOS 向け Bluetooth Classic 実装（Issue #659、親 #130）。`macos/fandhe_macos_bluetooth_classic.m`
//! （IOBluetooth の `pairedDevices` と `IOBluetoothDeviceInquiry`。`build.rs` が macOS ターゲットの
//! feature `device` 有効時にコンパイルし IOBluetooth をリンクする）を `unsafe extern "C"` 3 関数
//! （[`paired`](MacBluetoothClassic) の一覧・[`present`]・[`stop`]）で呼ぶ。`unsafe` はこの FFI 呼び出しと
//! `CStr` の読み取りに閉じ `// SAFETY:` で契約を明記する（coding-rust.md の FFI 例外規定）。
//! `device/bluetooth_classic.rs` の macOS 経路から呼ばれ、契約は `fandhe_device::bluetooth_classic`。
//!
//! `.m` 側のコールバックから [`fandhe_rust_bt_classic_found`] / [`fandhe_rust_bt_classic_finished`]
//! （本モジュールが公開する `extern "C"` 関数）が main queue 上で呼ばれ、`device/bluetooth_classic` の
//! 観測バッファへ足す。OS から受けた名前・キー・件数・電波強度は非信頼入力として、変換の前に検証する。
//! コードの写像は `fandhe_device::bluetooth_classic::decode_code`（SSOT）に一本化する。
//! 機器名・アドレスをログへ出力しない（security.md A09）。
//!
//! 写像の純関数 [`from_raw`] はホストでもテストできるようモジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井（【推測】は Mac 目視 #650 で確かめる）
//!
//! - 【推測】`CBManager.authorization` を IOBluetooth の許可判定の代わりに使えるか
//! - 【推測】`pairedDevices` が nil になる条件。nil は 0 件の成功に畳まず `Internal` にしている
//! - 【推測】LE だけのペアリング機器が `pairedDevices` に混ざるか
//! - 連続の inquiry は OS が間引く（中断として完了した場合は `Internal`）

use fandhe_device::bluetooth_classic::{
    clamp_name, major_from_cod, normalize_devices, rssi_from_raw, ClassicDevice, DeviceKey,
    MAX_DEVICES,
};
use fandhe_device::DeviceError;
use std::ffi::CStr;

const NAME_BYTES: usize = 256;
const KEY_BYTES: usize = 32;

/// `.m` の `FandheBtClassicDeviceRaw` と同じレイアウト。
#[repr(C)]
struct RawDevice {
    name: [u8; NAME_BYTES],
    cod: u32,
    rssi: i32,
    key: [u8; KEY_BYTES],
    paired: i32,
}

/// `.m` の `FandheBtClassicListRaw` と同じレイアウト。
#[repr(C)]
struct Raw {
    count: i32,
    entries: [RawDevice; MAX_DEVICES],
}

/// 固定長バッファの NUL 終端 UTF-8 を読む。NUL が無い・UTF-8 でないときは `None`。
fn read_cstr(buf: &[u8]) -> Option<&str> {
    CStr::from_bytes_until_nul(buf).ok()?.to_str().ok()
}

/// 1 件を契約の値へ写す。キーが空・不正な機器は `None`（捨てる）。
fn device_from_raw(
    name: Option<&str>,
    key: Option<&str>,
    cod: u32,
    rssi: i32,
    paired: bool,
) -> Option<(DeviceKey, ClassicDevice)> {
    let key = DeviceKey::new(key?)?;
    Some((
        key,
        ClassicDevice {
            name: name.and_then(|n| clamp_name(n.to_owned())),
            kind: major_from_cod(cod),
            paired,
            rssi: rssi_from_raw(rssi),
        },
    ))
}

/// `.m` の生値を契約の値へ写す。件数が 0..=[`MAX_DEVICES`] の外なら `Internal`（0 件の成功に畳まない）。
/// 名前は NUL 終端と UTF-8 を検証し、満たさなければ `None`（件数には数える）。キーが空の要素は捨てる。
fn from_raw(raw: &Raw) -> Result<Vec<ClassicDevice>, DeviceError> {
    let n = usize::try_from(raw.count)
        .ok()
        .filter(|&n| n <= MAX_DEVICES)
        .ok_or(DeviceError::Internal("bluetooth_classic list"))?;
    Ok(normalize_devices(raw.entries[..n].iter().filter_map(|e| {
        device_from_raw(
            read_cstr(&e.name),
            read_cstr(&e.key),
            e.cod,
            e.rssi,
            e.paired != 0,
        )
    })))
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::Raw;

    // SAFETY: `macos/fandhe_macos_bluetooth_classic.m`（`build.rs` が macOS ターゲットの feature
    // `device` 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_bt_classic_paired(out: *mut Raw) -> i32;
        fn fandhe_macos_bt_classic_start(request_id: u64, duration_ms: u64) -> i32;
        fn fandhe_macos_bt_classic_stop();
    }

    /// 完了コードを返す。0 = 成功。
    #[allow(unsafe_code)]
    pub(super) fn paired(out: &mut Raw) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持しない。
        unsafe { fandhe_macos_bt_classic_paired(out) }
    }

    #[allow(unsafe_code)]
    pub(super) fn start(request_id: u64, duration_ms: u64) -> i32 {
        // SAFETY: 引数はプリミティブのみでポインタを経由しない。戻り値 0 は依頼を受理したことだけを
        // 意味し、結果は main queue から非同期に `fandhe_rust_bt_classic_*` 経由で届く。
        unsafe { fandhe_macos_bt_classic_start(request_id, duration_ms) }
    }

    #[allow(unsafe_code)]
    pub(super) fn stop() {
        // SAFETY: 引数なし。`.m` 側は main thread 上なら直接、それ以外なら main queue へ同期
        // ディスパッチして検索を解放する。呼び出し元は Rust 側のロックを握らない契約。
        unsafe { fandhe_macos_bt_classic_stop() }
    }
}

fn blank() -> Box<Raw> {
    Box::new(Raw {
        count: 0,
        entries: std::array::from_fn(|_| RawDevice {
            name: [0; NAME_BYTES],
            cod: 0,
            rssi: 127,
            key: [0; KEY_BYTES],
            paired: 0,
        }),
    })
}

#[cfg(target_os = "macos")]
struct MacBluetoothClassic;

#[cfg(target_os = "macos")]
impl fandhe_device::bluetooth_classic::BluetoothClassic for MacBluetoothClassic {
    fn paired(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
        // 約 15KB。ヒープに置く。
        let mut raw = blank();
        fandhe_device::bluetooth_classic::decode_code(ffi::paired(&mut raw))?;
        from_raw(&raw)
    }

    fn discover(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
        // 検索は pull 型の `present` / `OBSERVED` 経路でのみ駆動する。トレイト経由の同期呼び出しは
        // 窓の間ブロックしてしまうため提供しない（成功値へは倒さない）。
        Err(DeviceError::Unsupported("bluetooth_classic"))
    }
}

/// macOS の `BluetoothClassic` 実装を返す（`device/bluetooth_classic.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> Box<dyn fandhe_device::bluetooth_classic::BluetoothClassic> {
    Box::new(MacBluetoothClassic)
}

/// 検索の開始を依頼する。窓は呼び出し元が渡す `duration`（`DISCOVERY_WINDOW_MS`）で、`.m` 側は
/// この値で閉じる。`.m` は常に 0（受理）を返す契約で、0 以外は依頼の失敗として返す。
#[cfg(target_os = "macos")]
pub(crate) fn present(request_id: u64, duration: std::time::Duration) -> Result<(), DeviceError> {
    let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    if ffi::start(request_id, ms) == 0 {
        Ok(())
    } else {
        Err(DeviceError::Internal("bluetooth_classic present"))
    }
}

/// 進行中の検索を止める（終了時専用。完了コールバックは呼ばれない）。
#[cfg(target_os = "macos")]
pub(crate) fn stop() {
    ffi::stop();
}

/// 機器 1 件の通知（`.m` から main queue 上で呼ばれる）。`name` は NULL 可（未取得）。ポインタは
/// この呼び出しが戻るまでのみ有効なため、ここで同期的にコピーして検証し切る。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_bt_classic_found(
    request_id: u64,
    key: *const std::os::raw::c_char,
    name: *const std::os::raw::c_char,
    cod: u32,
    rssi: i32,
    paired: i32,
) {
    if key.is_null() {
        return;
    }
    // SAFETY: `key` は非 NULL で、`.m` 側が NUL 終端 UTF-8 を渡す契約。呼び出しが戻るまで有効で、
    // ここで即座にコピーする。
    let key = unsafe { CStr::from_ptr(key) }
        .to_str()
        .ok()
        .map(str::to_owned);
    let name = if name.is_null() {
        None
    } else {
        // SAFETY: 同上（NULL は上で除外済み）。
        unsafe { CStr::from_ptr(name) }
            .to_str()
            .ok()
            .map(str::to_owned)
    };
    if let Some((k, d)) = device_from_raw(name.as_deref(), key.as_deref(), cod, rssi, paired != 0) {
        super::bluetooth_classic::OBSERVED.observe(request_id, k, d);
    }
}

/// 完了通知（`.m` から main queue 上で 1 要求につき 1 回だけ呼ばれる）。
/// `code` の意味は `fandhe_device::bluetooth_classic::decode_code`。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_bt_classic_finished(request_id: u64, code: i32) {
    super::bluetooth_classic::finish(
        request_id,
        fandhe_device::bluetooth_classic::decode_code(code),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::bluetooth_classic::DeviceKind;

    fn put(slot: &mut [u8], s: &[u8]) {
        slot[..s.len()].copy_from_slice(s);
    }

    #[test]
    fn maps_normal_values() {
        let mut r = blank();
        r.count = 1;
        put(&mut r.entries[0].name, "ヘッドホン".as_bytes());
        put(&mut r.entries[0].key, b"aa-bb");
        r.entries[0].cod = 0x0000_0418;
        r.entries[0].rssi = -60;
        r.entries[0].paired = 1;
        let v = from_raw(&r).unwrap();
        assert_eq!(
            v,
            [ClassicDevice {
                name: Some("ヘッドホン".into()),
                kind: DeviceKind::AudioVideo,
                paired: true,
                rssi: Some(-60),
            }]
        );
    }

    #[test]
    fn out_of_range_count_is_internal_error_not_empty_success() {
        for c in [-1, MAX_DEVICES as i32 + 1] {
            let mut r = blank();
            r.count = c;
            assert!(matches!(
                from_raw(&r),
                Err(DeviceError::Internal("bluetooth_classic list"))
            ));
        }
    }

    #[test]
    fn bad_utf8_name_becomes_none_but_is_counted() {
        let mut r = blank();
        r.count = 1;
        put(&mut r.entries[0].key, b"k");
        put(&mut r.entries[0].name, &[0xff, 0xfe]);
        let v = from_raw(&r).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, None);
    }

    #[test]
    fn unterminated_name_becomes_none() {
        let mut r = blank();
        r.count = 1;
        put(&mut r.entries[0].key, b"k");
        r.entries[0].name = [b'a'; NAME_BYTES];
        assert_eq!(from_raw(&r).unwrap()[0].name, None);
    }

    #[test]
    fn empty_key_is_dropped() {
        let mut r = blank();
        r.count = 2;
        put(&mut r.entries[1].key, b"k");
        let v = from_raw(&r).unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn sentinel_and_out_of_range_rssi_become_none() {
        let mut r = blank();
        r.count = 2;
        put(&mut r.entries[0].key, b"a");
        put(&mut r.entries[1].key, b"b");
        r.entries[0].rssi = 127;
        r.entries[1].rssi = -200;
        let v = from_raw(&r).unwrap();
        assert!(v.iter().all(|d| d.rssi.is_none()));
    }

    #[test]
    fn same_key_is_merged() {
        let mut r = blank();
        r.count = 2;
        put(&mut r.entries[0].key, b"same");
        put(&mut r.entries[1].key, b"same");
        put(&mut r.entries[1].name, b"Late");
        assert_eq!(from_raw(&r).unwrap().len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn discover_via_trait_is_unsupported_not_empty_success() {
        assert!(matches!(
            implementation().discover(),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
