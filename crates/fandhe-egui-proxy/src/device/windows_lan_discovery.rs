//! Windows 向け LAN のサービス探索（mDNS / DNS-SD）実装（Issue #487。親 #109 の windows の子）。
//!
//! 呼び出し元: `device/lan_discovery.rs` の Windows 経路が [`present`] を呼び、終了時に
//! [`stop_shared`] を呼ぶ。結果は同ファイルの `OBSERVED`（request_id 照合つき）へ足し、完了は
//! `finish` へ渡す。契約の SSOT は `fandhe_device::lan_discovery`。
//!
//! # 流れ（ワーカースレッド上で同期的に）
//!
//! 1. 未接続（`WindowsNetworkStatus` が `Offline`）なら `Disabled`。読めなかった場合は未接続に
//!    畳まず探索へ進む
//! 2. Win32 `DnsServiceBrowse`（`_http._tcp.local`、全インタフェース）を開始する。結果は OS の
//!    スレッドプールから [`on_browse`] へ届き、同じ応答に含まれる PTR / SRV / A / AAAA を
//!    `fandhe_device::lan_discovery::link_dns_records` で結んで `OBSERVED` へ足す
//! 3. 呼び出しの開始を起点に単調時計で `duration` 待つ（停止合図なら中断として `Internal`）
//! 4. **全経路で** `DnsServiceBrowseCancel` を呼ぶ。`clear_if_current` してから `finish` を呼ぶ
//!
//! コールバックの context は request_id（整数）、QueryName は `'static` で、キャンセル後に
//! 遅れて届くコールバックでも解放済みメモリを触らない。レコードリストはコールバックで必ず
//! `DnsFree` する。OS 側のエラー通知は `finish(Err)` にし、部分結果は捨てられる。
//!
//! # 守らない範囲と既知の天井
//!
//! 結合は 1 回のコールバック内のレコードだけで行う（未解決は名前だけの行になる）。
//! 窓をまたぐ `DnsServiceResolve` は未実装。ファイアウォールが UDP 5353 を塞ぐ環境では 0 件。
//! パッケージ無しの exe での実機動作は未確認（#481）。ローカルに MSVC が無く、型検査は
//! `x86_64-pc-windows-gnu` のみ。名前・アドレス・ステータスの数値はログへ出さない。

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_device::lan_discovery::{
    error_from_dns_status, link_dns_records, DnsRecord, SERVICE_TYPE,
};
use fandhe_device::network_status::{NetworkSnapshot, NetworkStatus};
use fandhe_device::DeviceError;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{DNS_REQUEST_PENDING, ERROR_CANCELLED};
use windows::Win32::NetworkManagement::Dns::{
    DnsFree, DnsFreeRecordList, DnsServiceBrowse, DnsServiceBrowseCancel,
    DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST,
    DNS_SERVICE_BROWSE_REQUEST_0, DNS_SERVICE_CANCEL,
};

use super::lan_discovery::{finish, OBSERVED};
use super::windows_network_status::WindowsNetworkStatus;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;

struct Session {
    request_id: u64,
    stop_tx: mpsc::Sender<()>,
}

/// 現在進行中のセッション（プロセス static、1 度に 1 つ）。
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// `_http._tcp.local` の NUL 終端 UTF-16。OS へ渡す QueryName は `'static` を保つ。
static QUERY: OnceLock<Vec<u16>> = OnceLock::new();

fn lock_session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn clear_if_current(request_id: u64) {
    let mut session = lock_session();
    if session.as_ref().is_some_and(|s| s.request_id == request_id) {
        *session = None;
    }
}

/// `device/lan_discovery.rs::drive_callback` から呼ばれる。ワーカーを起動して即座に返る。
/// 進行中のセッションがあれば停止を送って置き換える（古い結果は request_id 照合で捨てる）。
pub(crate) fn present(request_id: u64, duration: Duration) -> Result<(), DeviceError> {
    // 期限の起点は呼び出しの開始（単調時計）。
    let deadline = Instant::now() + duration;
    let (stop_tx, stop_rx) = mpsc::channel();
    {
        let mut session = lock_session();
        if let Some(old) = session.replace(Session {
            request_id,
            stop_tx,
        }) {
            let _ = old.stop_tx.send(());
        }
    }
    let spawned = std::thread::Builder::new().spawn(move || {
        let result = run_browse(request_id, deadline, &stop_rx);
        // 届ける前に状態を確定する（自分の request_id のときだけ解除）。
        clear_if_current(request_id);
        finish(request_id, result);
    });
    if spawned.is_err() {
        clear_if_current(request_id);
        return Err(DeviceError::Internal("lan_discovery present"));
    }
    Ok(())
}

/// アプリ終了時に進行中のセッションへ停止を送る（ロックを外してから送る）。
pub(crate) fn stop_shared() {
    let session = lock_session().take();
    if let Some(s) = session {
        let _ = s.stop_tx.send(());
    }
}

fn run_browse(
    request_id: u64,
    deadline: Instant,
    stop_rx: &mpsc::Receiver<()>,
) -> Result<(), DeviceError> {
    // 未接続だけ Disabled。読めなかったときは探索へ進む（未接続に畳まない）。
    if matches!(WindowsNetworkStatus.read(), Ok(NetworkSnapshot::Offline)) {
        return Err(DeviceError::Disabled("lan_discovery"));
    }
    let context =
        usize::try_from(request_id).map_err(|_| DeviceError::Internal("lan_discovery id"))?;
    let query = QUERY.get_or_init(|| {
        format!("{SERVICE_TYPE}.local")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    });

    // キャンセルが終わるまでこのスタックに生存させる。
    let mut cancel = DNS_SERVICE_CANCEL::default();
    let request = DNS_SERVICE_BROWSE_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1.0,
        InterfaceIndex: 0,
        QueryName: PCWSTR(query.as_ptr()),
        Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 {
            pBrowseCallback: Some(on_browse),
        },
        // context は ID を整数のまま持つ（ポインタとして参照しない）。
        pQueryContext: context as *mut c_void,
    };
    // SAFETY: `request` / `cancel` / `query` は呼び出しの間生存し、`cancel` は後続の
    // `DnsServiceBrowseCancel` まで動かさない。QueryName は NUL 終端で `'static`。
    #[allow(unsafe_code)]
    let ret = unsafe { DnsServiceBrowse(&request, &mut cancel) };
    if ret != DNS_REQUEST_PENDING {
        return Err(error_from_dns_status(ret as u32));
    }

    let result = match stop_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
        _ => Err(DeviceError::Internal("lan_discovery interrupted")),
    };
    // SAFETY: `cancel` は成功した `DnsServiceBrowse` が初期化したハンドルで、まだ生存している。
    #[allow(unsafe_code)]
    unsafe {
        DnsServiceBrowseCancel(&cancel);
    }
    result
}

/// PWSTR → String（NULL・不正は `None`）。
///
/// # Safety
/// 非 NULL なら NUL 終端の有効な UTF-16 を指すこと。
#[allow(unsafe_code)]
unsafe fn wstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    unsafe { p.to_string().ok() }
}

/// OS のスレッドプールから呼ばれる。`rec` は成否に関わらず最後に解放する。
#[allow(unsafe_code)]
unsafe extern "system" fn on_browse(status: u32, context: *const c_void, rec: *const DNS_RECORDW) {
    let request_id = context as usize as u64;
    if status == 0 {
        let mut records = Vec::new();
        let mut cur = rec;
        // SAFETY: OS が渡したレコードリストは `pNext` で NULL まで連なり、`DnsFree` まで有効。
        // 共用体は `wType` が示す要素だけを読む。
        while let Some(r) = unsafe { cur.as_ref() } {
            if let Some(rr) = unsafe { copy_record(r) } {
                records.push(rr);
            }
            cur = r.pNext;
        }
        for s in link_dns_records(&records) {
            OBSERVED.observe(request_id, s);
        }
    } else if status != ERROR_CANCELLED.0 {
        finish(request_id, Err(error_from_dns_status(status)));
    }
    if !rec.is_null() {
        // SAFETY: コールバックが受けた所有権を `DnsFreeRecordList` で 1 回だけ返す。
        unsafe { DnsFree(Some(rec.cast()), DnsFreeRecordList) };
    }
}

/// 生のレコード 1 件を Rust 型へコピーする（対象外の型・NULL を含むものは `None`）。
///
/// # Safety
/// `r` が OS の返した有効なレコードであること。
#[allow(unsafe_code)]
unsafe fn copy_record(r: &DNS_RECORDW) -> Option<DnsRecord> {
    // SAFETY: `wType` に対応する共用体の要素を読む。名前は NUL 終端の PWSTR。
    unsafe {
        let owner = wstr(r.pName)?;
        match r.wType {
            TYPE_PTR => Some(DnsRecord::Ptr {
                instance: wstr(r.Data.PTR.pNameHost)?,
            }),
            TYPE_SRV => Some(DnsRecord::Srv {
                owner,
                target: wstr(r.Data.SRV.pNameTarget)?,
                port: r.Data.SRV.wPort,
            }),
            TYPE_A => Some(DnsRecord::Addr {
                owner,
                // IpAddress はメモリ上がネットワーク順。
                ip: IpAddr::V4(Ipv4Addr::from(r.Data.A.IpAddress.to_ne_bytes())),
            }),
            TYPE_AAAA => Some(DnsRecord::Addr {
                owner,
                ip: IpAddr::V6(Ipv6Addr::from(r.Data.AAAA.Ip6Address.IP6Byte)),
            }),
            _ => None,
        }
    }
}
