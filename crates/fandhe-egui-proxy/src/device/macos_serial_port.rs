//! macOS 向けシリアルポート実装（Issue #670、親 #131）。`macos/fandhe_macos_serial_port.m`
//! （IOKit の列挙と POSIX 端末入出力。`build.rs` が macOS ターゲットの feature `device` 有効時に
//! コンパイルし IOKit をリンクする）を `unsafe extern "C"` 5 関数で呼ぶ。`unsafe` はこの FFI 呼び出しと
//! ハンドルの所有に閉じ `// SAFETY:` で契約を明記する（coding-rust.md の FFI 例外規定）。
//! `device/serial_port.rs` の macOS 経路から [`implementation`] が呼ばれ、契約は
//! `fandhe_device::serial_port`（往復ループ `round_trip` は共用で、ここには持たない）。
//!
//! OS から受けた件数・パス・errno は非信頼入力として、使う前に検証する。件数が範囲外なら 0 件の成功に
//! 畳まず `Internal`。開く名前は [`is_macos_callout_path`] を通ったものだけ（任意のデバイスパスを
//! 開かない、security.md A03）。errno の写像は `failure_from_errno` の 1 か所に一本化する。
//! ポート名・errno の値をログへ出力しない（security.md A09）。
//!
//! 写像の純関数 [`from_raw_list`] はホストでもテストできるようモジュールは `cfg(any(macos, test))`。
//!
//! # 既知の天井（【推測】は Mac 目視 #667 で確かめる）
//!
//! - 【推測】TCC・entitlement が不要か（サンドボックスなしの前提）
//! - 【推測】USB シリアルの抜去で select が準備完了になり、read が 0 バイトを返すか（切断として `Gone`）
//! - 待ちは `select`（`poll` は端末デバイスで不安定なため）。`FD_SETSIZE` 以上の fd は `Internal`

use std::ffi::CStr;
use std::time::Duration;

use fandhe_device::serial_port::{
    failure_from_errno, is_macos_callout_path, normalize_ports, SerialPortEntry, SerialPortFailure,
    MAX_PORTS, MAX_PORT_NAME_BYTES,
};
// FFI の実体を使う型は macOS ビルドだけで要る（ホストテストは写像の純関数だけを使う）。
#[cfg(target_os = "macos")]
use fandhe_device::serial_port::{SerialLink, SerialPorts};
#[cfg(target_os = "macos")]
use std::ffi::CString;

/// `.m` の `FandheSerialListRaw` と同じレイアウト。
#[repr(C)]
struct RawList {
    count: i32,
    names: [[u8; MAX_PORT_NAME_BYTES]; MAX_PORTS],
}

/// 固定長バッファの NUL 終端 UTF-8 を読む。NUL が無い・UTF-8 でないときは `None`。
fn read_cstr(buf: &[u8]) -> Option<&str> {
    CStr::from_bytes_until_nul(buf).ok()?.to_str().ok()
}

/// `.m` の生値を契約の値へ写す。件数が `0..=MAX_PORTS` の外なら `Internal`（0 件の成功に畳まない）。
/// NUL 終端・UTF-8・callout パス検証のどれかを満たさない項目は捨てる。
fn from_raw_list(raw: &RawList) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
    let n = usize::try_from(raw.count)
        .ok()
        .filter(|&n| n <= MAX_PORTS)
        .ok_or(SerialPortFailure::Internal)?;
    Ok(normalize_ports(
        raw.names[..n]
            .iter()
            .filter_map(|b| read_cstr(b).map(str::to_owned)),
        is_macos_callout_path,
    ))
}

/// `.m` の戻りコード（0 = 成功 / 1 = errno 有効 / 2 = 内部失敗）を失敗へ写す。
fn failure_from_code(code: i32, errno: i32) -> SerialPortFailure {
    match code {
        1 => failure_from_errno(errno),
        _ => SerialPortFailure::Internal,
    }
}

/// 待ち時間を `.m` へ渡すミリ秒へ丸める（`u32` に収まらなければ上限）。
fn wait_ms(wait: Duration) -> u32 {
    u32::try_from(wait.as_millis()).unwrap_or(u32::MAX)
}

/// `.m` の `FandheSerialHandle`（不透明）。
#[repr(C)]
struct Handle {
    _opaque: [u8; 0],
}

#[cfg(target_os = "macos")]
mod ffi {
    use super::{Handle, RawList};

    // SAFETY: `macos/fandhe_macos_serial_port.m`（`build.rs` が macOS ターゲットの feature `device`
    // 有効時にコンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_macos_serial_list(out: *mut RawList) -> i32;
        fn fandhe_macos_serial_open(
            path: *const std::ffi::c_char,
            out_errno: *mut i32,
        ) -> *mut Handle;
        fn fandhe_macos_serial_write(
            h: *mut Handle,
            buf: *const u8,
            len: usize,
            wait_ms: u32,
            written: *mut usize,
            out_errno: *mut i32,
        ) -> i32;
        fn fandhe_macos_serial_read(
            h: *mut Handle,
            buf: *mut u8,
            cap: usize,
            wait_ms: u32,
            nread: *mut usize,
            out_errno: *mut i32,
        ) -> i32;
        fn fandhe_macos_serial_close(h: *mut Handle);
    }

    #[allow(unsafe_code)]
    pub(super) fn list(out: &mut RawList) -> i32 {
        // SAFETY: `out` は呼び出しの間有効な排他参照で、`.m` はポインタを保持せず、
        // `names` の各行は NAME_BYTES 以内に NUL 終端で書く。
        unsafe { fandhe_macos_serial_list(out) }
    }

    #[allow(unsafe_code)]
    pub(super) fn open(path: &std::ffi::CStr, errno: &mut i32) -> *mut Handle {
        // SAFETY: `path` は NUL 終端で呼び出しの間有効、`errno` は有効な排他参照。`.m` はどちらも保持しない。
        unsafe { fandhe_macos_serial_open(path.as_ptr(), errno) }
    }

    /// # Safety
    ///
    /// `h` は [`open`] が返した未 close のハンドルで、他スレッドから同時に使われないこと。
    #[allow(unsafe_code)]
    pub(super) unsafe fn write(
        h: *mut Handle,
        buf: &[u8],
        wait_ms: u32,
        written: &mut usize,
        errno: &mut i32,
    ) -> i32 {
        // SAFETY: 呼び出し元が `h` の有効性を保証する。`buf` は `len` バイト読める。
        unsafe { fandhe_macos_serial_write(h, buf.as_ptr(), buf.len(), wait_ms, written, errno) }
    }

    /// # Safety
    ///
    /// [`write`] と同じ。
    #[allow(unsafe_code)]
    pub(super) unsafe fn read(
        h: *mut Handle,
        buf: &mut [u8],
        wait_ms: u32,
        nread: &mut usize,
        errno: &mut i32,
    ) -> i32 {
        // SAFETY: 呼び出し元が `h` の有効性を保証する。`.m` は `cap` を超えて書かない。
        unsafe { fandhe_macos_serial_read(h, buf.as_mut_ptr(), buf.len(), wait_ms, nread, errno) }
    }

    /// # Safety
    ///
    /// `h` は [`open`] が返した未 close のハンドル（または NULL）で、以後使わないこと。
    #[allow(unsafe_code)]
    pub(super) unsafe fn close(h: *mut Handle) {
        // SAFETY: 呼び出し元が `h` の有効性と以後の不使用を保証する。NULL は許される。
        unsafe { fandhe_macos_serial_close(h) }
    }
}

/// 開いたポート 1 本。`Drop` で termios を戻して閉じる（全終了経路で資源を残さない）。
#[cfg(target_os = "macos")]
struct MacSerialLink {
    handle: *mut Handle,
}

// SAFETY: ハンドルは `MacSerialLink` が唯一の所有者で、`&mut self` の間だけ使い、`.m` 側は
// ハンドル以外のグローバル状態を持たない。ワーカー間で所有権を移すだけで、同時には使わない。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe impl Send for MacSerialLink {}

#[cfg(target_os = "macos")]
impl SerialLink for MacSerialLink {
    fn write_some(&mut self, buf: &[u8], wait: Duration) -> Result<usize, SerialPortFailure> {
        let (mut written, mut errno) = (0usize, 0i32);
        // SAFETY: `handle` は open が返した未 close のもので、`&mut self` により排他。
        #[allow(unsafe_code)]
        let code = unsafe { ffi::write(self.handle, buf, wait_ms(wait), &mut written, &mut errno) };
        if code != 0 {
            return Err(failure_from_code(code, errno));
        }
        // `.m` が要求より多く返すことはない。食い違いは嘘の成功にせず失敗にする。
        if written > buf.len() {
            return Err(SerialPortFailure::Internal);
        }
        Ok(written)
    }

    fn read_some(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, SerialPortFailure> {
        let (mut nread, mut errno) = (0usize, 0i32);
        // SAFETY: `write_some` と同じ。
        #[allow(unsafe_code)]
        let code = unsafe { ffi::read(self.handle, buf, wait_ms(wait), &mut nread, &mut errno) };
        if code != 0 {
            return Err(failure_from_code(code, errno));
        }
        if nread > buf.len() {
            return Err(SerialPortFailure::Internal);
        }
        Ok(nread)
    }

    fn close(self: Box<Self>) {
        // `Drop` が termios を戻して閉じる。
        drop(self);
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacSerialLink {
    fn drop(&mut self) {
        // SAFETY: open が返した未 close のハンドルで、Drop は 1 度だけ走り以後使わない。
        #[allow(unsafe_code)]
        unsafe {
            ffi::close(self.handle);
        }
    }
}

/// macOS の `SerialPorts` 実装。状態を持たない。
#[cfg(target_os = "macos")]
struct MacSerialPorts;

#[cfg(target_os = "macos")]
impl SerialPorts for MacSerialPorts {
    fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
        let mut raw = Box::new(RawList {
            count: 0,
            names: [[0; MAX_PORT_NAME_BYTES]; MAX_PORTS],
        });
        match ffi::list(&mut raw) {
            0 => from_raw_list(&raw),
            code => Err(failure_from_code(code, 0)),
        }
    }

    fn open(&self, name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
        // 任意のデバイスパスを開かない（security.md A03）。
        if !is_macos_callout_path(name) {
            return Err(SerialPortFailure::Internal);
        }
        let path = CString::new(name).map_err(|_| SerialPortFailure::Internal)?;
        let mut errno = 0i32;
        let handle = ffi::open(&path, &mut errno);
        if handle.is_null() {
            return Err(failure_from_errno(errno));
        }
        Ok(Box::new(MacSerialLink { handle }))
    }
}

/// macOS の `SerialPorts` 実装を返す（`device/serial_port.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "macos")]
pub(crate) fn implementation() -> std::sync::Arc<dyn SerialPorts> {
    std::sync::Arc::new(MacSerialPorts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(names: &[&[u8]], count: i32) -> RawList {
        let mut r = RawList {
            count,
            names: [[0; MAX_PORT_NAME_BYTES]; MAX_PORTS],
        };
        for (i, n) in names.iter().enumerate() {
            r.names[i][..n.len()].copy_from_slice(n);
        }
        r
    }

    #[test]
    fn out_of_range_count_is_internal_not_empty_success() {
        for count in [-1, MAX_PORTS as i32 + 1, i32::MAX] {
            assert_eq!(
                from_raw_list(&raw(&[], count)).unwrap_err(),
                SerialPortFailure::Internal
            );
        }
    }

    #[test]
    fn zero_count_is_empty_success() {
        assert!(from_raw_list(&raw(&[], 0)).unwrap().is_empty());
    }

    #[test]
    fn invalid_items_are_dropped() {
        let r = raw(
            &[
                b"/dev/cu.ok",
                b"/dev/tty.dialin",
                b"\xff\xfe",
                b"/dev/cu.",
                b"/dev/cu.ok",
            ],
            5,
        );
        let v = from_raw_list(&r).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "/dev/cu.ok");
    }

    #[test]
    fn unterminated_name_is_dropped() {
        let mut r = raw(&[], 1);
        r.names[0] = [b'a'; MAX_PORT_NAME_BYTES];
        assert!(from_raw_list(&r).unwrap().is_empty());
    }

    #[test]
    fn code_mapping_uses_errno_only_for_code_one() {
        assert_eq!(failure_from_code(1, 16), SerialPortFailure::Busy);
        assert_eq!(
            failure_from_code(1, 13),
            SerialPortFailure::PermissionDenied
        );
        assert_eq!(failure_from_code(2, 16), SerialPortFailure::Internal);
        assert_eq!(failure_from_code(99, 16), SerialPortFailure::Internal);
    }

    #[test]
    fn wait_is_clamped() {
        assert_eq!(wait_ms(Duration::from_millis(100)), 100);
        assert_eq!(wait_ms(Duration::from_secs(u64::MAX / 2)), u32::MAX);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_rejects_non_callout_names_without_touching_os() {
        let p = implementation();
        for bad in [
            "/dev/tty.x",
            "/etc/passwd",
            "/dev/cu.",
            "/dev/cu.a/../b",
            "",
        ] {
            assert!(
                matches!(p.open(bad), Err(SerialPortFailure::Internal)),
                "{bad}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_missing_port_is_gone() {
        let p = implementation();
        assert!(matches!(
            p.open("/dev/cu.fandhe-test-nonexistent"),
            Err(SerialPortFailure::Gone)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn list_returns_only_valid_callout_paths() {
        let entries = implementation().list().expect("IOKit の列挙は成功する");
        assert!(entries.len() <= MAX_PORTS);
        for e in entries {
            assert!(is_macos_callout_path(&e.name));
        }
    }
}
