//! シリアルポート（ポートの一覧と、選んだポートでの 1 行の往復）の抽象（Phase 11 の拡充ループ、
//! 対応表 `docs/device/capabilities/serial-port.json`、親 #131、Issue #670）。
//!
//! 扱う範囲は「ポートを一覧し、選んだポートを固定設定（[`BAUD_RATE`] bps・8N1・フロー制御なし）で
//! 開き、固定の 1 行 [`SEND_LINE`] を送って LF までの 1 行を受け、毎回閉じる」ことだけ。
//! 設定の変更・連続受信・バイナリの往復・任意文字列の送信は扱わない。対象は macOS（#670）・
//! Windows（#671）・Web（#672）。iOS・Android は対応表で `no`。
//! `fandhe-egui-proxy` の `device/serial_port.rs` が専用ワーカー 1 本から [`SerialPorts::list`] と
//! [`round_trip`] を呼ぶ（ブロッキングの契約）。Web は本モジュールの trait を使わず
//! `fandhe-web-dom` が `navigator.serial` で実装し、定数と失敗の意味だけを共有する。
//!
//! # 契約
//!
//! - 失敗種別は機能固有の [`SerialPortFailure`]。`DeviceError` に variant を足すと既存の網羅 match が
//!   壊れるため（`music_library::MusicLibraryFailure` と同じ判断）。[`From`] で `DeviceError` へも写せる
//! - **三値**: 成功（0 件の一覧を含む / 1 行）、キャンセル（Web のダイアログを閉じたときだけ。
//!   [`SerialPortFailure::Cancelled`]）、失敗（それ以外）。ネイティブはキャンセルを作らない
//! - 0 件の一覧は成功。非対応・権限拒否・列挙の失敗を 0 件へ畳まない（契約表 6）
//! - 期限は [`ROUND_TRIP_TIMEOUT`] が唯一の値で、起点は要求の受付時刻。呼び出し側が
//!   単調時計のクロージャ（`Instant::elapsed` 等）を [`round_trip`] へ渡す。壁時計は使わない
//! - 期限後に完成した行は捨てて [`SerialPortFailure::Timeout`] にする。全バイトを送り切れないまま
//!   期限が来たときも `Timeout`
//! - [`SerialPorts::open`] は固定設定の適用と**受信バッファの破棄**まで済ませてから返す
//! - 全経路で link を閉じてから結果を返す（契約表 9）
//! - ポート名・送受信の中身・OS のエラー値の生値をログ・`Debug`・画面の失敗文言に出さない
//!
//! # 並行性
//!
//! 一覧も往復も 1 本の専用ワーカー（`mpsc` + 専用スレッド）へ順に流す（契約表 11）。経路ごとに
//! 別のロックを足さない。画面側は世代で古い完了を捨てる。
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、失敗を成功に見せないこと、期限後の行を成功にしないこと、
//! 全経路での close（macOS は変更前の termios を戻す）、ポート名・送受信の中身をログへ出さないこと。
//!
//! 守らない:
//! - 抜き差しの反映（再取得まで）
//! - 改行で返さない機器（期限切れになる）
//! - 他プロセスとの同時オープンの解決（使用中として失敗にするだけ）
//! - macOS の擬似端末と `/dev/tty.*`
//! - 受信した行の文字コード（不正な UTF-8 は置換文字）
//! - アプリ終了処理と進行中の OS 呼び出しの重なり（プロセス終了で OS が fd を閉じる）
//! - 複数プロセス間の一致

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::DeviceError;
use crate::device_info::clean_text;

/// 固定のボーレート（bps）。回線設定は 8N1・フロー制御なしで固定。
pub const BAUD_RATE: u32 = 9600;
/// 送る固定の 1 行。入力欄は持たない。
pub const SEND_LINE: &[u8] = b"fandhe\n";
/// 往復全体（開く〜受信）の期限。起点は要求の受付時刻。
pub const ROUND_TRIP_TIMEOUT: Duration = Duration::from_secs(3);
/// 1 回の read / write が待つ長さの上限。停止を見る間隔でもある。
pub const POLL_STEP: Duration = Duration::from_millis(100);
/// 受信する 1 行の上限（LF を除く）。超えたら [`SerialPortFailure::LineTooLong`]。
pub const MAX_LINE_BYTES: usize = 256;
/// 一覧に出すポート数の上限。
pub const MAX_PORTS: usize = 32;
/// ポート名の最大バイト数。
pub const MAX_PORT_NAME_BYTES: usize = 128;

/// 失敗種別。`Cancelled` は Web 専用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialPortFailure {
    /// 期限内に 1 行が届かない（部分送信・期限後の行を含む）。
    Timeout,
    /// 他のプロセスが使用中。
    Busy,
    /// ポートが無い・抜去された。
    Gone,
    /// OS または利用者が使用を拒否した。
    PermissionDenied,
    /// この OS・ブラウザでは使えない。
    Unsupported,
    /// 受信した行が [`MAX_LINE_BYTES`] を超えた。
    LineTooLong,
    /// 利用者がダイアログを閉じた（Web のみ）。
    Cancelled,
    /// 上記以外（OS の値は含めない）。
    Internal,
}

impl From<SerialPortFailure> for DeviceError {
    /// 成功へは倒さない。種別を持つものだけ同名の variant へ写し、残りは `Internal`。
    fn from(f: SerialPortFailure) -> Self {
        match f {
            SerialPortFailure::Unsupported => DeviceError::Unsupported("serial_port"),
            SerialPortFailure::PermissionDenied => DeviceError::PermissionDenied("serial_port"),
            SerialPortFailure::Cancelled => DeviceError::Cancelled("serial_port"),
            SerialPortFailure::Timeout => DeviceError::Internal("serial_port timeout"),
            SerialPortFailure::Busy => DeviceError::Internal("serial_port busy"),
            SerialPortFailure::Gone => DeviceError::Internal("serial_port gone"),
            SerialPortFailure::LineTooLong => DeviceError::Internal("serial_port line too long"),
            SerialPortFailure::Internal => DeviceError::Internal("serial_port"),
        }
    }
}

/// 一覧の 1 件。名前は画面にだけ出し、ログに出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct SerialPortEntry {
    /// macOS は `/dev/cu.*`、Windows は `COM<n>`。
    pub name: String,
}

// ポート名を出さない。
impl std::fmt::Debug for SerialPortEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SerialPortEntry")
    }
}

/// 受信した 1 行。`text` が `None` なら空行（成功）。
#[derive(Clone, PartialEq, Eq)]
pub struct ReceivedLine {
    /// [`clean_text`] 済みの本文。
    pub text: Option<String>,
}

// 受信内容を出さない。
impl std::fmt::Debug for ReceivedLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReceivedLine")
    }
}

/// 開いたポート 1 本。`Drop` でも閉じる実装にする（[`round_trip`] は明示的に `close` する）。
pub trait SerialLink: Send {
    /// 最大 `wait` 待って書ける分だけ書き、書いたバイト数を返す。0 は「まだ書けない」。
    fn write_some(&mut self, buf: &[u8], wait: Duration) -> Result<usize, SerialPortFailure>;
    /// 最大 `wait` 待って読める分だけ読み、読んだバイト数を返す。0 は「まだ届かない」。
    /// 切断（HUP 等）は `Gone`。
    fn read_some(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, SerialPortFailure>;
    /// 設定を戻して閉じる。
    fn close(self: Box<Self>);
}

/// シリアルポートの契約（モジュール doc の「契約」節）。ワーカースレッドから呼ぶブロッキング呼び出し。
pub trait SerialPorts: Send + Sync {
    /// ポートを列挙する。0 件は成功。
    fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure>;
    /// 名前を検証して開き、固定設定の適用と受信バッファの破棄まで済ませて返す。
    fn open(&self, name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure>;
}

/// 対象外 OS・スタブ・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedSerialPorts;

impl SerialPorts for UnsupportedSerialPorts {
    fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
        Err(SerialPortFailure::Unsupported)
    }

    fn open(&self, _name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
        Err(SerialPortFailure::Unsupported)
    }
}

/// 受信バイト列を LF までの 1 行へ組む。LF 以降のバイトは捨てる（1 往復で 1 行しか使わない）。
#[derive(Default)]
pub struct LineAssembler {
    buf: Vec<u8>,
}

impl LineAssembler {
    /// 空の組み立て器。
    pub fn new() -> Self {
        Self::default()
    }

    /// バイトを足す。LF が来たら 1 行を返す（末尾 CR は除く）。CR と LF を除いた本文が
    /// [`MAX_LINE_BYTES`] を超えたら、LF を待たず `LineTooLong`。
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<ReceivedLine>, SerialPortFailure> {
        for &b in bytes {
            if b == b'\n' {
                if self.buf.last() == Some(&b'\r') {
                    self.buf.pop();
                }
                let text = clean_text(&String::from_utf8_lossy(&self.buf));
                self.buf.clear();
                return Ok(Some(ReceivedLine { text }));
            }
            self.buf.push(b);
            // 末尾 CR は LF の直前なら本文に数えない。
            let body = self.buf.len() - usize::from(self.buf.last() == Some(&b'\r'));
            if body > MAX_LINE_BYTES {
                return Err(SerialPortFailure::LineTooLong);
            }
        }
        Ok(None)
    }
}

/// Darwin の errno を失敗種別へ写す（macOS の `.m` が返す値。写像はここ 1 か所）。
pub fn failure_from_errno(errno: i32) -> SerialPortFailure {
    match errno {
        // EBUSY
        16 => SerialPortFailure::Busy,
        // EPERM / EACCES
        1 | 13 => SerialPortFailure::PermissionDenied,
        // ENOENT / EIO / ENXIO / ENODEV
        2 | 5 | 6 | 19 => SerialPortFailure::Gone,
        _ => SerialPortFailure::Internal,
    }
}

/// Win32 エラーコードを失敗種別へ写す（#671 が呼ぶ）。
///
/// 対応は文書からの推測で、Windows 実機確認で確かめる（COM ポートが使用中のとき
/// `ERROR_ACCESS_DENIED` が返る前提で `Busy` に寄せている）。
pub fn failure_from_win32(code: u32) -> SerialPortFailure {
    match code {
        // ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION
        5 | 32 => SerialPortFailure::Busy,
        // FILE_NOT_FOUND / PATH_NOT_FOUND / BAD_COMMAND / GEN_FAILURE /
        // DEVICE_NOT_CONNECTED / DEVICE_REMOVED
        2 | 3 | 22 | 31 | 1167 | 1617 => SerialPortFailure::Gone,
        _ => SerialPortFailure::Internal,
    }
}

/// macOS の callout デバイスのパスか。`/dev/cu.` で始まり残りが空でなく、`/` と NUL を含まず、
/// ASCII 可視文字だけで、[`MAX_PORT_NAME_BYTES`] 以下。任意のデバイスパスを開かないための検証。
pub fn is_macos_callout_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/dev/cu.") else {
        return false;
    };
    path.len() <= MAX_PORT_NAME_BYTES
        && !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_graphic() && b != b'/')
}

/// Windows の COM ポート名（`COM1`〜`COM255`）か。
pub fn is_windows_com_name(name: &str) -> bool {
    let Some(digits) = name.strip_prefix("COM") else {
        return false;
    };
    // 先頭 0 と符号を拒否する（`COM01` や `COM+1` を別名で通さない）。
    !digits.starts_with('0')
        && digits.len() <= 3
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits.parse::<u16>().is_ok_and(|n| (1..=255).contains(&n))
}

/// 一覧を整える。`is_valid` を通らない項目を捨て、重複を除き、[`MAX_PORTS`] 件で切る。
pub fn normalize_ports(
    names: impl IntoIterator<Item = String>,
    is_valid: impl Fn(&str) -> bool,
) -> Vec<SerialPortEntry> {
    let mut out: Vec<SerialPortEntry> = Vec::new();
    for name in names {
        if out.len() == MAX_PORTS {
            break;
        }
        if is_valid(&name) && !out.iter().any(|e| e.name == name) {
            out.push(SerialPortEntry { name });
        }
    }
    out
}

/// 期限までの残り。尽きていれば `None`。
fn remaining(elapsed: &dyn Fn() -> Duration) -> Option<Duration> {
    ROUND_TRIP_TIMEOUT
        .checked_sub(elapsed())
        .filter(|d| !d.is_zero())
}

fn run(
    link: &mut dyn SerialLink,
    elapsed: &dyn Fn() -> Duration,
    stop: &AtomicBool,
) -> Result<ReceivedLine, SerialPortFailure> {
    let mut written = 0;
    while written < SEND_LINE.len() {
        if stop.load(Ordering::SeqCst) {
            return Err(SerialPortFailure::Internal);
        }
        let left = remaining(elapsed).ok_or(SerialPortFailure::Timeout)?;
        written += link
            .write_some(&SEND_LINE[written..], left.min(POLL_STEP))?
            .min(SEND_LINE.len() - written);
    }
    let mut assembler = LineAssembler::new();
    let mut chunk = [0u8; 64];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(SerialPortFailure::Internal);
        }
        let left = remaining(elapsed).ok_or(SerialPortFailure::Timeout)?;
        let n = link.read_some(&mut chunk, left.min(POLL_STEP))?;
        if n == 0 {
            continue;
        }
        if let Some(line) = assembler.push(&chunk[..n.min(chunk.len())])? {
            // 期限後に完成した行は成功にしない。
            return if remaining(elapsed).is_some() {
                Ok(line)
            } else {
                Err(SerialPortFailure::Timeout)
            };
        }
    }
}

/// 往復ループの唯一の実装（macOS・Windows が共用）。open → [`SEND_LINE`] を全バイト送る →
/// [`POLL_STEP`] 刻みで受信して 1 行に組む。`elapsed` は要求の受付時刻からの単調時計の経過
/// （`Instant` を直接使わないので wasm32 でもコンパイルでき、テストで時間を差し込める）。
/// `stop` が立ったら `Internal`。どの経路でも link を閉じてから返す。
pub fn round_trip(
    ports: &dyn SerialPorts,
    name: &str,
    elapsed: &dyn Fn() -> Duration,
    stop: &AtomicBool,
) -> Result<ReceivedLine, SerialPortFailure> {
    if stop.load(Ordering::SeqCst) {
        return Err(SerialPortFailure::Internal);
    }
    if remaining(elapsed).is_none() {
        return Err(SerialPortFailure::Timeout);
    }
    let mut link = ports.open(name)?;
    let result = run(link.as_mut(), elapsed, stop);
    link.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    #[test]
    fn unsupported_never_succeeds() {
        let p = UnsupportedSerialPorts;
        assert_eq!(p.list().unwrap_err(), SerialPortFailure::Unsupported);
        assert!(matches!(
            p.open("/dev/cu.x"),
            Err(SerialPortFailure::Unsupported)
        ));
    }

    #[test]
    fn assembler_handles_lf_crlf_and_chunks() {
        let mut a = LineAssembler::new();
        assert_eq!(a.push(b"he").unwrap(), None);
        let l = a.push(b"llo\r\nrest").unwrap().unwrap();
        assert_eq!(l.text.as_deref(), Some("hello"));
        // LF 以降は捨てる。
        let mut a = LineAssembler::new();
        let l = a.push(b"a\nb\n").unwrap().unwrap();
        assert_eq!(l.text.as_deref(), Some("a"));
    }

    #[test]
    fn assembler_empty_line_is_none_text() {
        let mut a = LineAssembler::new();
        assert_eq!(a.push(b"\n").unwrap().unwrap().text, None);
        let mut a = LineAssembler::new();
        assert_eq!(a.push(b"\r\n").unwrap().unwrap().text, None);
    }

    #[test]
    fn assembler_rejects_overlong_line() {
        let mut a = LineAssembler::new();
        assert_eq!(
            a.push(&vec![b'x'; MAX_LINE_BYTES + 1]).unwrap_err(),
            SerialPortFailure::LineTooLong
        );
        // ちょうど上限は通る（CRLF 終端でも）。
        let mut a = LineAssembler::new();
        let mut v = vec![b'x'; MAX_LINE_BYTES];
        v.extend_from_slice(b"\r\n");
        assert!(a.push(&v).unwrap().is_some());
        // LF が来なくても、別チャンクで上限超過を検知する。
        let mut a = LineAssembler::new();
        assert_eq!(a.push(&vec![b'x'; MAX_LINE_BYTES]).unwrap(), None);
        assert_eq!(a.push(b"y").unwrap_err(), SerialPortFailure::LineTooLong);
    }

    #[test]
    fn assembler_lossy_and_strips_controls() {
        let mut a = LineAssembler::new();
        let l = a.push(b"a\xFFb\x07c\n").unwrap().unwrap();
        assert_eq!(l.text.as_deref(), Some("a\u{FFFD}bc"));
    }

    #[test]
    fn errno_mapping() {
        assert_eq!(failure_from_errno(16), SerialPortFailure::Busy);
        assert_eq!(failure_from_errno(13), SerialPortFailure::PermissionDenied);
        assert_eq!(failure_from_errno(1), SerialPortFailure::PermissionDenied);
        for e in [2, 5, 6, 19] {
            assert_eq!(failure_from_errno(e), SerialPortFailure::Gone);
        }
        assert_eq!(failure_from_errno(9999), SerialPortFailure::Internal);
        assert_eq!(failure_from_errno(0), SerialPortFailure::Internal);
    }

    #[test]
    fn win32_mapping() {
        for c in [5, 32] {
            assert_eq!(failure_from_win32(c), SerialPortFailure::Busy);
        }
        for c in [2, 3, 22, 31, 1167, 1617] {
            assert_eq!(failure_from_win32(c), SerialPortFailure::Gone);
        }
        assert_eq!(failure_from_win32(12345), SerialPortFailure::Internal);
    }

    #[test]
    fn macos_path_validation() {
        assert!(is_macos_callout_path("/dev/cu.usbserial-1410"));
        assert!(!is_macos_callout_path("/dev/tty.usbserial"));
        assert!(!is_macos_callout_path("/dev/cu."));
        assert!(!is_macos_callout_path("/dev/cu.a/b"));
        assert!(!is_macos_callout_path("/dev/cu.a\0b"));
        assert!(!is_macos_callout_path("/dev/cu.a b"));
        assert!(!is_macos_callout_path("/dev/cu.日本"));
        assert!(!is_macos_callout_path("../dev/cu.a"));
        let long = format!("/dev/cu.{}", "a".repeat(MAX_PORT_NAME_BYTES));
        assert!(!is_macos_callout_path(&long));
    }

    #[test]
    fn windows_name_validation() {
        assert!(is_windows_com_name("COM1"));
        assert!(is_windows_com_name("COM255"));
        for bad in [
            "COM0", "COM256", "com1", "COM1x", "COM", "COM01", "COM+1", "LPT1", "",
        ] {
            assert!(!is_windows_com_name(bad), "{bad}");
        }
    }

    #[test]
    fn normalize_drops_invalid_dedups_and_caps() {
        let names = (0..40)
            .map(|i| format!("/dev/cu.p{i}"))
            .chain(["/dev/tty.x".to_string()]);
        let v = normalize_ports(names, is_macos_callout_path);
        assert_eq!(v.len(), MAX_PORTS);
        let v = normalize_ports(
            ["/dev/tty.x", "/dev/cu.a", "/dev/cu.a"].map(String::from),
            is_macos_callout_path,
        );
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn debug_hides_contents() {
        let e = SerialPortEntry {
            name: "/dev/cu.secret".into(),
        };
        let r = ReceivedLine {
            text: Some("secret".into()),
        };
        assert!(!format!("{e:?}{r:?}").contains("secret"));
    }

    #[test]
    fn failure_maps_to_device_error_without_success() {
        assert!(matches!(
            DeviceError::from(SerialPortFailure::Unsupported),
            DeviceError::Unsupported(_)
        ));
        assert!(matches!(
            DeviceError::from(SerialPortFailure::PermissionDenied),
            DeviceError::PermissionDenied(_)
        ));
        assert!(matches!(
            DeviceError::from(SerialPortFailure::Cancelled),
            DeviceError::Cancelled(_)
        ));
        assert!(matches!(
            DeviceError::from(SerialPortFailure::Timeout),
            DeviceError::Internal(_)
        ));
    }

    // ---- round_trip 用の偽物（時計はミリ秒の共有カウンタ） ----

    type Reads = Vec<Result<Vec<u8>, SerialPortFailure>>;

    struct FakeLink {
        reads: Reads,
        /// write が一度に進むバイト数（0 なら進まない）。
        write_step: usize,
        write_err: Option<SerialPortFailure>,
        clock: Arc<AtomicU64>,
        write_tick_ms: u64,
        read_tick_ms: u64,
        closes: Arc<AtomicU64>,
        sent: Arc<Mutex<Vec<u8>>>,
    }

    impl SerialLink for FakeLink {
        fn write_some(&mut self, buf: &[u8], _w: Duration) -> Result<usize, SerialPortFailure> {
            self.clock.fetch_add(self.write_tick_ms, Ordering::SeqCst);
            if let Some(e) = self.write_err {
                return Err(e);
            }
            let n = self.write_step.min(buf.len());
            self.sent.lock().unwrap().extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn read_some(&mut self, buf: &mut [u8], _w: Duration) -> Result<usize, SerialPortFailure> {
            self.clock.fetch_add(self.read_tick_ms, Ordering::SeqCst);
            if self.reads.is_empty() {
                return Ok(0);
            }
            let data = self.reads.remove(0)?;
            buf[..data.len()].copy_from_slice(&data);
            Ok(data.len())
        }
        fn close(self: Box<Self>) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakePorts {
        link: Mutex<Option<FakeLink>>,
        open_err: Option<SerialPortFailure>,
    }

    impl SerialPorts for FakePorts {
        fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
            Ok(vec![])
        }
        fn open(&self, _name: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
            if let Some(e) = self.open_err {
                return Err(e);
            }
            Ok(Box::new(self.link.lock().unwrap().take().unwrap()))
        }
    }

    struct Rig {
        ports: FakePorts,
        clock: Arc<AtomicU64>,
        closes: Arc<AtomicU64>,
        sent: Arc<Mutex<Vec<u8>>>,
    }

    /// `write_step` は一度に進める送信バイト数、`ticks` は (write, read) 1 回ごとに進む時間（ms）。
    fn rig(reads: Reads, write_step: usize, ticks: (u64, u64)) -> Rig {
        let clock = Arc::new(AtomicU64::new(0));
        let closes = Arc::new(AtomicU64::new(0));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let link = FakeLink {
            reads,
            write_step,
            write_err: None,
            clock: clock.clone(),
            write_tick_ms: ticks.0,
            read_tick_ms: ticks.1,
            closes: closes.clone(),
            sent: sent.clone(),
        };
        Rig {
            ports: FakePorts {
                link: Mutex::new(Some(link)),
                open_err: None,
            },
            clock,
            closes,
            sent,
        }
    }

    fn go(r: &Rig, stop: &AtomicBool) -> Result<ReceivedLine, SerialPortFailure> {
        let clock = r.clock.clone();
        round_trip(
            &r.ports,
            "/dev/cu.x",
            &move || Duration::from_millis(clock.load(Ordering::SeqCst)),
            stop,
        )
    }

    fn closes(r: &Rig) -> u64 {
        r.closes.load(Ordering::SeqCst)
    }

    #[test]
    fn round_trip_success_sends_full_line_and_closes_once() {
        let r = rig(vec![Ok(b"pong\r\n".to_vec())], 100, (10, 10));
        let line = go(&r, &AtomicBool::new(false)).unwrap();
        assert_eq!(line.text.as_deref(), Some("pong"));
        assert_eq!(r.sent.lock().unwrap().as_slice(), SEND_LINE);
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_partial_writes_are_completed() {
        let r = rig(vec![Ok(b"x\n".to_vec())], 2, (1, 1));
        go(&r, &AtomicBool::new(false)).unwrap();
        assert_eq!(r.sent.lock().unwrap().as_slice(), SEND_LINE);
    }

    #[test]
    fn round_trip_no_reply_times_out() {
        let r = rig(vec![], 100, (1, 100));
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Timeout
        );
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_late_line_is_discarded() {
        // 読み取りの 1 回で時計が期限を越え、その read で行が完成する。
        let r = rig(vec![Ok(b"late\n".to_vec())], 100, (0, 4_000));
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Timeout
        );
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_stalled_write_times_out() {
        let r = rig(vec![], 0, (500, 1));
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Timeout
        );
        assert!(r.sent.lock().unwrap().is_empty());
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_maps_write_and_read_errors() {
        let r = rig(vec![], 100, (1, 1));
        r.ports.link.lock().unwrap().as_mut().unwrap().write_err = Some(SerialPortFailure::Gone);
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Gone
        );
        assert_eq!(closes(&r), 1);

        let r = rig(vec![Err(SerialPortFailure::Gone)], 100, (1, 1));
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Gone
        );
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_overlong_line_fails() {
        let r = rig(vec![Ok(vec![b'x'; 64]); 6], 100, (1, 1));
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::LineTooLong
        );
        assert_eq!(closes(&r), 1);
    }

    #[test]
    fn round_trip_stop_flag_is_internal() {
        // open 前に立っていれば open もしない。
        let r = rig(vec![], 100, (1, 1));
        assert_eq!(
            go(&r, &AtomicBool::new(true)).unwrap_err(),
            SerialPortFailure::Internal
        );
        assert_eq!(closes(&r), 0);
    }

    #[test]
    fn round_trip_stop_during_run_closes() {
        // 送信後、受信待ちの間に stop が立つ（read が 0 を返し続け、1 回目の read で立てる）。
        struct StopLink(Arc<AtomicBool>, Arc<AtomicU64>);
        impl SerialLink for StopLink {
            fn write_some(&mut self, b: &[u8], _: Duration) -> Result<usize, SerialPortFailure> {
                Ok(b.len())
            }
            fn read_some(&mut self, _: &mut [u8], _: Duration) -> Result<usize, SerialPortFailure> {
                self.0.store(true, Ordering::SeqCst);
                Ok(0)
            }
            fn close(self: Box<Self>) {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct One(Mutex<Option<StopLink>>);
        impl SerialPorts for One {
            fn list(&self) -> Result<Vec<SerialPortEntry>, SerialPortFailure> {
                Ok(vec![])
            }
            fn open(&self, _: &str) -> Result<Box<dyn SerialLink>, SerialPortFailure> {
                Ok(Box::new(self.0.lock().unwrap().take().unwrap()))
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let closes = Arc::new(AtomicU64::new(0));
        let ports = One(Mutex::new(Some(StopLink(stop.clone(), closes.clone()))));
        let res = round_trip(&ports, "/dev/cu.x", &|| Duration::ZERO, &stop);
        assert_eq!(res.unwrap_err(), SerialPortFailure::Internal);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn round_trip_open_failure_does_not_close() {
        let mut r = rig(vec![], 100, (1, 1));
        r.ports.open_err = Some(SerialPortFailure::Busy);
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Busy
        );
        assert_eq!(closes(&r), 0);
    }

    #[test]
    fn round_trip_already_expired_does_not_open() {
        let r = rig(vec![], 100, (1, 1));
        r.clock
            .store(ROUND_TRIP_TIMEOUT.as_millis() as u64, Ordering::SeqCst);
        assert_eq!(
            go(&r, &AtomicBool::new(false)).unwrap_err(),
            SerialPortFailure::Timeout
        );
        assert_eq!(closes(&r), 0);
    }
}
