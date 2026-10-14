//! 位置情報（測位）の抽象（Phase 11 の拡充ループ、親 #887、Issue #888）。
//!
//! `Motion` と同じ継続 push 型（束 `continuous-push`）で、最新の測位結果だけを
//! 保持する共有状態 [`SharedLocation`] を持つ。「まだ 1 度も測位できていない」と
//! 「(0, 0) に測位できた」は型で区別する契約とし、既定値としての `(0.0, 0.0)` は
//! 一切持たない（実在座標のため、`Motion` の `(0.0, 0.0)` 初期値と違って偽装の
//! 余地を作らない）。
//!
//! 書き手（OS ごとの実測位）は本モジュール外（Windows は [`CommandLocation`]、
//! iOS/Android/Web は #887 配下の別子 issue）が [`SharedLocation::push_fix`] /
//! [`SharedLocation::push_failure`] を呼ぶ。読み手は [`Location::last_fix`] のみで、
//! OS ごとの差異を意識しない。
//!
//! Windows のみ本ファイルで実装する（本 issue のスコープ）。macOS/Linux は
//! `fandhe-egui-proxy` 側（`device/location.rs`）が起動時に
//! `push_failure(LocationFailure::Unsupported)` を 1 回入れる cfg カスケードで
//! 「使えない」表示にする（デスクトップの CoreLocation 実装は別 issue、ダミー座標へ
//! フォールバックしない）。

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::DeviceError;

/// 測位結果。緯度・経度は WGS84 の度、`accuracy_m` は水平精度（メートル）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    /// 緯度（度、`-90.0..=90.0`）。
    pub latitude_deg: f64,
    /// 経度（度、`-180.0..=180.0`）。
    pub longitude_deg: f64,
    /// 水平精度（メートル、有限かつ `0.0` 以上）。
    pub accuracy_m: f64,
}

/// 測位が失敗した理由。`DeviceError` への写像は本モジュール doc の対応表どおり
/// （[`SharedLocation::last_fix`] の実装 doc 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationFailure {
    /// ユーザー・OS が位置情報の利用を拒否した。
    PermissionDenied,
    /// OS 設定で位置情報サービス自体が無効化されている。
    ServiceDisabled,
    /// 測位がタイムアウトした。
    Timeout,
    /// 現在のプラットフォームでは未対応。
    Unsupported,
    /// 上記以外の失敗（初期化不能・子プロセス異常終了等）。
    Failed,
}

/// 位置情報の契約（`Motion` と同じくスレッド間で共有されうるため `Send + Sync`）。
pub trait Location: Send + Sync {
    /// 最新の測位結果を返す。**まだ 1 度も測位できていなければ `Ok(None)`**
    /// （`(0.0, 0.0)` を「未取得」の代用にしない、モジュール doc 参照）。
    fn last_fix(&self) -> Result<Option<Fix>, DeviceError>;
}

/// 測位状態。「未取得」「測位できた」「失敗した」を型で区別する
/// （[`SharedLocation`] の内部表現）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum LocationStatus {
    Waiting,
    /// 測位できた結果と、受け取った時刻（経過時間の表示用、[`SharedLocation::fix_age`]）。
    Fixed(Fix, Instant),
    Failed(LocationFailure),
}

/// 最新の測位結果だけを保持する共有状態（`DummyMotion` の `Arc<Mutex<..>>` と同型）。
/// 書き手（OS 別の実装）は [`push_fix`](Self::push_fix) / [`push_failure`](Self::push_failure)
/// を、読み手は [`Location`] 経由で [`last_fix`](Location::last_fix) のみを呼ぶ。
#[derive(Clone)]
pub struct SharedLocation(Arc<Mutex<LocationStatus>>);

impl SharedLocation {
    /// 初期状態は「未取得」（`Waiting`）。
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(LocationStatus::Waiting)))
    }

    /// 測位結果を書き込む。範囲外・非有限値は保存せず [`LocationFailure::Failed`] へ
    /// 倒す（`DummyMotion::set` の `clamp_finite` と同じ fail-safe。OS・橋渡し側が
    /// 異常値を push しても下流へそのまま伝播させない、security.md A08）。
    pub fn push_fix(&self, fix: Fix) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = if is_valid_fix(fix) {
            LocationStatus::Fixed(fix, Instant::now())
        } else {
            LocationStatus::Failed(LocationFailure::Failed)
        };
    }

    /// 失敗を書き込む。保持していた `Fix` は破棄する（権限拒否後に古い座標を
    /// 返さないため、モジュール doc・§3.1 参照）。
    pub fn push_failure(&self, failure: LocationFailure) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = LocationStatus::Failed(failure);
    }

    /// 「未取得」へ戻す。明示的な再試行でドライバを再起動する直前に呼ぶ契約
    /// （`fandhe-egui-proxy` の `device/location.rs::ensure_windows_driver` 参照。
    /// 古い失敗・座標を保持したまま再起動すると、新ドライバが初回結果を出す前に
    /// 画面が旧結果へ一瞬戻ってしまうため、先に「取得中」へ倒す）。
    pub fn reset_to_waiting(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = LocationStatus::Waiting;
    }

    /// 現在の `Fix` を受け取ってからの経過時間。`Fix` を持たない（未取得・失敗）ときは `None`。
    /// 画面が「位置の受信: N 秒前」を出すための値（鮮度の判定には使わない）。
    pub fn fix_age(&self) -> Option<Duration> {
        let state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            LocationStatus::Fixed(_, at) => Some(at.elapsed()),
            _ => None,
        }
    }
}

/// 緯度・経度・精度が実在しうる範囲内かつ有限であるかを検証する純関数。
///
/// Web 経路（`fandhe-web-dom::location`、Issue #891）も `navigator.geolocation` から
/// 得た座標を同じ基準で検証するため `pub` にする（検証ロジックの SSOT を本関数に一本化する）。
pub fn is_valid_fix(fix: Fix) -> bool {
    fix.latitude_deg.is_finite()
        && (-90.0..=90.0).contains(&fix.latitude_deg)
        && fix.longitude_deg.is_finite()
        && (-180.0..=180.0).contains(&fix.longitude_deg)
        && fix.accuracy_m.is_finite()
        && fix.accuracy_m >= 0.0
}

impl Default for SharedLocation {
    fn default() -> Self {
        Self::new()
    }
}

/// [`LocationFailure`] を [`DeviceError`] へ写す（モジュール doc の対応表）。
fn failure_to_device_error(failure: LocationFailure) -> DeviceError {
    match failure {
        LocationFailure::PermissionDenied => DeviceError::PermissionDenied("location"),
        LocationFailure::ServiceDisabled => DeviceError::Disabled("location"),
        LocationFailure::Timeout => DeviceError::Internal("location timeout"),
        LocationFailure::Unsupported => DeviceError::Unsupported("location"),
        LocationFailure::Failed => DeviceError::Internal("location"),
    }
}

impl Location for SharedLocation {
    fn last_fix(&self) -> Result<Option<Fix>, DeviceError> {
        // 他所の panic に由来する poison を呼び出し側へ伝播させない
        // （coding-rust.md: unwrap/expect を本実装コードで使わない方針）。
        let state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            LocationStatus::Waiting => Ok(None),
            LocationStatus::Fixed(fix, _) => Ok(Some(fix)),
            LocationStatus::Failed(failure) => Err(failure_to_device_error(failure)),
        }
    }
}

// ---------------------------------------------------------------------------
// Windows 実装（`CommandLocation`）。
// ---------------------------------------------------------------------------

/// PowerShell へ渡す固定スクリプト。外部入力を一切連結しない
/// （security.md A03、tts/file_picker の Windows 経路と同じ方針）。位置情報は
/// 入力テキストを持たないため、そもそも注入面が無い。
///
/// `Geolocator.RequestAccessAsync` の結果で分岐し、許可されればループで測位して
/// 1 行ずつ標準出力へ書く。数値は `InvariantCulture` で整形する（ja-JP/de-DE 等
/// 小数点カンマのロケールで Rust 側 `f64` パースが壊れるのを防ぐ）。
///
/// WinRT の `IAsyncOperation.AsTask()` は C# の拡張メソッドで、Windows PowerShell の
/// ドット呼び出しはインスタンスへ自動バインドできない（`$op.AsTask()` は
/// `MethodNotFound` になる。cursor[bot] 指摘 L168-179）。`Add-Type` で
/// アセンブリを読み込むだけでは解決しないため、`Wait-WinRtOperation` でリフレクション
/// 経由に明示的にバインドして呼ぶ。
///
/// `GetGeopositionAsync` は引数無しだと GPS ロック不能時に無言で待ち続ける
/// （cursor[bot] 指摘 L177-179）。`(maximumAge, timeout)` オーバーロードで
/// 明示的にタイムアウトを持たせ、`Stopwatch` で実測した経過時間がタイムアウト
/// 以上なら `ERR timeout` を返す（WinRT 例外の型が .NET 側でどう写像されるかに
/// 依存しないため）。
///
/// ループ先頭・catch 節の両方で `Geolocator.LocationStatus` を見て `Disabled`
/// （OS 設定で位置情報サービス自体が無効）を区別する（cursor[bot] 指摘 L170-187）。
/// 行プロトコルは `parse_line` の対応表どおり: `OK <lat> <lon> <acc>` /
/// `ERR denied|disabled|timeout|failed`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_LOCATION_SCRIPT: &str = r#"
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    Add-Type -AssemblyName System.Runtime.WindowsRuntime

    # `Add-Type -AssemblyName System.Runtime.WindowsRuntime` は AsTask 拡張メソッドの
    # ためのアセンブリ読み込みであり、Windows.Devices.Geolocation 配下の WinRT 型
    # メタデータは登録されない。`New-Object` は WinRT 型を解決できないため、各型を
    # 先に `[Type, Assembly, ContentType=WindowsRuntime]` 構文で評価してロードしておく
    # （codex-review 指摘 crates/fandhe-device/src/location.rs:209。
    # https://devblogs.microsoft.com/oldnewthing/20180110-00/?p=97755）。
    [Windows.Devices.Geolocation.Geolocator, Windows.Devices.Geolocation, ContentType=WindowsRuntime] | Out-Null
    [Windows.Devices.Geolocation.PositionStatus, Windows.Devices.Geolocation, ContentType=WindowsRuntime] | Out-Null
    [Windows.Devices.Geolocation.GeolocationAccessStatus, Windows.Devices.Geolocation, ContentType=WindowsRuntime] | Out-Null
    [Windows.Devices.Geolocation.Geoposition, Windows.Devices.Geolocation, ContentType=WindowsRuntime] | Out-Null

    $asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
        $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
        $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
    })[0]
    function Wait-WinRtOperation($operation, [type]$resultType) {
        $task = $asTaskGeneric.MakeGenericMethod($resultType).Invoke($null, @($operation))
        $task.Wait()
        return $task.Result
    }

    $geolocator = New-Object Windows.Devices.Geolocation.Geolocator
    while ($geolocator.LocationStatus -eq [Windows.Devices.Geolocation.PositionStatus]::Disabled) {
        # 起動時点で無効でもプロセスを終了しない（Bugbot 指摘 L196-200）。有効化を
        # 待ちながらループし、有効化されたら下の権限確認へ抜ける。
        Write-Output "ERR disabled"
        Start-Sleep -Seconds 5
    }

    $accessOp = [Windows.Devices.Geolocation.Geolocator]::RequestAccessAsync()
    $access = Wait-WinRtOperation $accessOp ([Windows.Devices.Geolocation.GeolocationAccessStatus])
    if ($access -ne [Windows.Devices.Geolocation.GeolocationAccessStatus]::Allowed) {
        Write-Output "ERR denied"
        exit 0
    }

    $culture = [System.Globalization.CultureInfo]::InvariantCulture
    $maxAge = [TimeSpan]::FromSeconds(0)
    $timeout = [TimeSpan]::FromSeconds(30)
    while ($true) {
        if ($geolocator.LocationStatus -eq [Windows.Devices.Geolocation.PositionStatus]::Disabled) {
            Write-Output "ERR disabled"
            Start-Sleep -Seconds 5
            continue
        }
        $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        try {
            $positionOp = $geolocator.GetGeopositionAsync($maxAge, $timeout)
            $position = Wait-WinRtOperation $positionOp ([Windows.Devices.Geolocation.Geoposition])
            $coord = $position.Coordinate
            $lat = $coord.Point.Position.Latitude.ToString($culture)
            $lon = $coord.Point.Position.Longitude.ToString($culture)
            $acc = $coord.Accuracy.ToString($culture)
            Write-Output "OK $lat $lon $acc"
        } catch {
            if ($geolocator.LocationStatus -eq [Windows.Devices.Geolocation.PositionStatus]::Disabled) {
                Write-Output "ERR disabled"
            } elseif ($stopwatch.Elapsed -ge $timeout) {
                Write-Output "ERR timeout"
            } else {
                Write-Output "ERR failed"
            }
        }
        Start-Sleep -Seconds 5
    }
} catch {
    Write-Output "ERR failed"
    exit 1
}
"#;

/// 子プロセス起動情報（tts の `SpeechCommand`/file_picker の `PickCommand` と同型）。
/// テストから `/bin/sh` 等を注入できるよう、Windows 向け組み立てと実行を分離する。
struct LocationCommand {
    program: PathBuf,
    args: Vec<String>,
}

/// Windows 向け測位コマンドを組み立てる。`powershell.exe` は絶対パスで起動する
/// （PATH 乗っ取り対策、tts/file_picker と同じ理由）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_location_command() -> LocationCommand {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let program = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    LocationCommand {
        program,
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            WINDOWS_LOCATION_SCRIPT.to_string(),
        ],
    }
}

/// 1 行の出力を [`Fix`] または [`LocationFailure`] へ変換する純関数（`cfg` 無しで
/// 全ターゲットからテストできる）。行プロトコル（モジュール doc 参照）:
/// 成功 `OK <lat> <lon> <acc>`、失敗 `ERR denied|disabled|timeout|failed`。
/// 認識できない行・数値としてパースできない行は「壊れた行」として `None` を返し、
/// 呼び出し元は無視する（「取得中」のまま固まらせない・偽の成功も作らない）。
fn parse_line(line: &str) -> Option<Result<Fix, LocationFailure>> {
    let mut parts = line.split_whitespace();
    match parts.next()? {
        "OK" => {
            let lat: f64 = parts.next()?.parse().ok()?;
            let lon: f64 = parts.next()?.parse().ok()?;
            let acc: f64 = parts.next()?.parse().ok()?;
            if parts.next().is_some() {
                return None;
            }
            Some(Ok(Fix {
                latitude_deg: lat,
                longitude_deg: lon,
                accuracy_m: acc,
            }))
        }
        "ERR" => {
            let reason = parts.next()?;
            if parts.next().is_some() {
                return None;
            }
            let failure = match reason {
                "denied" => LocationFailure::PermissionDenied,
                "disabled" => LocationFailure::ServiceDisabled,
                "timeout" => LocationFailure::Timeout,
                "failed" => LocationFailure::Failed,
                _ => return None,
            };
            Some(Err(failure))
        }
        _ => None,
    }
}

/// 起動した子プロセスの 1 行ずつを読み、[`SharedLocation`] へ書き込み続ける
/// （読み取りスレッド本体）。`reader` の EOF（子プロセスの終了）・読み取りエラーで
/// 自然に抜ける。
///
/// ループを抜けた時点で `stopping`（[`CommandLocation::stop`] が立てる）が偽、かつ
/// 直前に処理した行が `denied`（`PermissionDenied`、スクリプトが意図して `exit 0`
/// する唯一の到達可能な終了経路）でなければ、予期しない終了とみなし
/// [`LocationFailure::Failed`] を共有状態へ反映する（codex-review 指摘: EOF・読み取り
/// エラーで共有状態を放置すると `Waiting` の張り付き・古い座標の表示継続が起きる）。
fn read_lines_into(
    reader: BufReader<ChildStdout>,
    shared: SharedLocation,
    stopping: Arc<AtomicBool>,
) {
    let mut last_was_denied = false;
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        match parse_line(&line) {
            Some(Ok(fix)) => {
                shared.push_fix(fix);
                last_was_denied = false;
            }
            Some(Err(failure)) => {
                shared.push_failure(failure);
                last_was_denied = failure == LocationFailure::PermissionDenied;
            }
            // 壊れた行は無視する（偽の成功・偽の「取得中」を作らない）。
            None => {}
        }
    }

    if !stopping.load(Ordering::SeqCst) && !last_was_denied {
        shared.push_failure(LocationFailure::Failed);
    }
}

/// Windows 向け実装。長命の PowerShell 子プロセスを 1 個起動し、更新を 1 行 1 件で
/// stdout から読み続ける（モジュール doc・[`WINDOWS_LOCATION_SCRIPT`] 参照）。
pub struct CommandLocation {
    child: Mutex<Option<Child>>,
    reader_thread: Mutex<Option<JoinHandle<()>>>,
    shared: SharedLocation,
    /// `stop()` が立てる「意図した停止」フラグ。`read_lines_into` が EOF/エラーで
    /// 抜けたときに、明示停止と予期しない終了を区別するために参照する。
    stopping: Arc<AtomicBool>,
}

impl CommandLocation {
    /// 既定のプログラム（PowerShell）で起動する。`powershell.exe` が存在しなければ
    /// `Err(DeviceError::Unsupported("location"))`（ダミーへ絶対にフォールバックしない）。
    pub fn start(shared: SharedLocation) -> Result<Self, DeviceError> {
        let command = windows_location_command();
        if !command.program.is_file() {
            return Err(DeviceError::Unsupported("location"));
        }
        Self::start_with(command, shared)
    }

    /// 任意のコマンドで起動する（テストから `/bin/sh` 等を注入する入口）。
    fn start_with(command: LocationCommand, shared: SharedLocation) -> Result<Self, DeviceError> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args);
        cmd.stdout(Stdio::piped());
        #[cfg(windows)]
        {
            // GUI ホストで測位のたびにコンソールウィンドウが表示されるのを防ぐ
            // （tts/file_picker と同じ対策）。CREATE_NO_WINDOW = 0x08000000。
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut spawned = cmd
            .spawn()
            .map_err(|_| DeviceError::Internal("location spawn"))?;
        let Some(stdout) = spawned.stdout.take() else {
            let _ = spawned.kill();
            let _ = spawned.wait();
            return Err(DeviceError::Internal("location spawn"));
        };

        let stopping = Arc::new(AtomicBool::new(false));
        let reader_shared = shared.clone();
        let reader_stopping = Arc::clone(&stopping);
        let reader_thread = std::thread::spawn(move || {
            read_lines_into(BufReader::new(stdout), reader_shared, reader_stopping);
        });

        Ok(Self {
            child: Mutex::new(Some(spawned)),
            reader_thread: Mutex::new(Some(reader_thread)),
            shared,
            stopping,
        })
    }

    /// 測位を止める。子プロセスを kill し、読み取りスレッドの終了を待つ。
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self
            .reader_thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = handle.join();
        }
    }
}

impl Location for CommandLocation {
    fn last_fix(&self) -> Result<Option<Fix>, DeviceError> {
        self.shared.last_fix()
    }
}

impl Drop for CommandLocation {
    /// ハンドル破棄時に子プロセス・読み取りスレッドを回収する（tts の `CommandTts`
    /// と同じ理由。`Drop` が無いとアプリ終了時に PowerShell が残る）。
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_fix() -> Fix {
        Fix {
            latitude_deg: 35.0,
            longitude_deg: 139.0,
            accuracy_m: 10.0,
        }
    }

    #[test]
    fn initial_last_fix_is_ok_none() {
        let shared = SharedLocation::new();
        assert_eq!(shared.last_fix().unwrap(), None);
    }

    #[test]
    fn push_fix_then_last_fix_returns_fix() {
        let shared = SharedLocation::new();
        shared.push_fix(valid_fix());
        assert_eq!(shared.last_fix().unwrap(), Some(valid_fix()));
    }

    #[test]
    fn push_failure_permission_denied_maps_and_discards_fix() {
        let shared = SharedLocation::new();
        shared.push_fix(valid_fix());
        shared.push_failure(LocationFailure::PermissionDenied);
        assert!(matches!(
            shared.last_fix(),
            Err(DeviceError::PermissionDenied("location"))
        ));
    }

    #[test]
    fn push_failure_service_disabled_maps_to_disabled() {
        let shared = SharedLocation::new();
        shared.push_failure(LocationFailure::ServiceDisabled);
        assert!(matches!(
            shared.last_fix(),
            Err(DeviceError::Disabled("location"))
        ));
    }

    #[test]
    fn push_failure_timeout_maps_to_internal_timeout() {
        let shared = SharedLocation::new();
        shared.push_failure(LocationFailure::Timeout);
        match shared.last_fix() {
            Err(DeviceError::Internal(msg)) => assert!(msg.contains("timeout")),
            other => panic!("got: {other:?}"),
        }
    }

    #[test]
    fn push_failure_unsupported_does_not_become_success_or_waiting() {
        let shared = SharedLocation::new();
        shared.push_failure(LocationFailure::Unsupported);
        assert!(matches!(
            shared.last_fix(),
            Err(DeviceError::Unsupported("location"))
        ));
    }

    #[test]
    fn push_fix_rejects_out_of_range_and_nan() {
        let shared = SharedLocation::new();
        shared.push_fix(Fix {
            latitude_deg: 999.0,
            longitude_deg: 0.0,
            accuracy_m: 0.0,
        });
        assert!(matches!(shared.last_fix(), Err(DeviceError::Internal(_))));

        let shared = SharedLocation::new();
        shared.push_fix(Fix {
            latitude_deg: f64::NAN,
            longitude_deg: 0.0,
            accuracy_m: 0.0,
        });
        assert!(matches!(shared.last_fix(), Err(DeviceError::Internal(_))));

        let shared = SharedLocation::new();
        shared.push_fix(Fix {
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            accuracy_m: -1.0,
        });
        assert!(matches!(shared.last_fix(), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn zero_zero_fix_is_distinguishable_from_waiting() {
        // 受入 2: 「未取得」と「(0, 0)」を型で区別する。
        let shared = SharedLocation::new();
        assert_eq!(shared.last_fix().unwrap(), None);
        shared.push_fix(Fix {
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            accuracy_m: 5.0,
        });
        assert_eq!(
            shared.last_fix().unwrap(),
            Some(Fix {
                latitude_deg: 0.0,
                longitude_deg: 0.0,
                accuracy_m: 5.0
            })
        );
    }

    #[test]
    fn parse_line_ok_variants() {
        assert_eq!(
            parse_line("OK 35.5 139.5 12.3"),
            Some(Ok(Fix {
                latitude_deg: 35.5,
                longitude_deg: 139.5,
                accuracy_m: 12.3
            }))
        );
    }

    #[test]
    fn parse_line_err_variants() {
        assert_eq!(
            parse_line("ERR denied"),
            Some(Err(LocationFailure::PermissionDenied))
        );
        assert_eq!(
            parse_line("ERR disabled"),
            Some(Err(LocationFailure::ServiceDisabled))
        );
        assert_eq!(
            parse_line("ERR timeout"),
            Some(Err(LocationFailure::Timeout))
        );
        assert_eq!(parse_line("ERR failed"), Some(Err(LocationFailure::Failed)));
    }

    #[test]
    fn parse_line_rejects_malformed_and_locale_comma() {
        assert_eq!(parse_line(""), None);
        assert_eq!(
            parse_line("OK 35,0 139,0 1,0"),
            None,
            "小数点カンマは拒否する"
        );
        assert_eq!(parse_line("OK 35.0 139.0"), None, "引数不足");
        assert_eq!(parse_line("OK 35.0 139.0 1.0 extra"), None, "引数過多");
        assert_eq!(parse_line("ERR unknown"), None);
        assert_eq!(parse_line("HELLO"), None);
    }

    /// `run_command`/`run_windows_pick` と同じ手法: OS 判定を経ない `start_with` へ
    /// `/bin/sh -c` を注入し、成功行・失敗行・壊れた行・異常終了を実プロセスで検証する
    /// （CI の Linux/macOS runner でも動く）。
    #[cfg(unix)]
    fn shell_command(script: &str) -> LocationCommand {
        LocationCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    #[test]
    #[cfg(unix)]
    fn start_with_reads_success_line_into_shared_state() {
        // 行を出力した直後にプロセスが終了すると EOF による Failed 反映（新仕様）が
        // 割り込むレースになるため、`stop()` するまでプロセスを生かしておく。
        let shared = SharedLocation::new();
        let location = CommandLocation::start_with(
            shell_command("echo 'OK 35.0 139.0 10.0'; sleep 5"),
            shared.clone(),
        )
        .expect("起動できる");

        // 読み取りスレッドが 1 行処理するのを待つ（ポーリング。テスト専用の緩い待ち）。
        wait_until(|| shared.last_fix().unwrap().is_some());
        assert_eq!(
            shared.last_fix().unwrap(),
            Some(Fix {
                latitude_deg: 35.0,
                longitude_deg: 139.0,
                accuracy_m: 10.0
            })
        );
        location.stop();
    }

    #[test]
    #[cfg(unix)]
    fn start_with_reads_failure_line_into_shared_state() {
        let shared = SharedLocation::new();
        let location =
            CommandLocation::start_with(shell_command("echo 'ERR denied'"), shared.clone())
                .expect("起動できる");

        wait_until(|| shared.last_fix().is_err());
        assert!(matches!(
            shared.last_fix(),
            Err(DeviceError::PermissionDenied("location"))
        ));
        location.stop();
    }

    #[test]
    #[cfg(unix)]
    fn start_with_ignores_garbage_lines_without_hanging() {
        // 最終行が OK のため、直後にプロセスが終了すると EOF による Failed 反映
        // （新仕様）とのレースになる。`stop()` するまでプロセスを生かしておく。
        let shared = SharedLocation::new();
        let location = CommandLocation::start_with(
            shell_command("echo 'not a real line'; echo 'OK 1.0 2.0 3.0'; sleep 5"),
            shared.clone(),
        )
        .expect("起動できる");

        wait_until(|| shared.last_fix().unwrap().is_some());
        assert_eq!(
            shared.last_fix().unwrap(),
            Some(Fix {
                latitude_deg: 1.0,
                longitude_deg: 2.0,
                accuracy_m: 3.0
            })
        );
        location.stop();
    }

    #[test]
    #[cfg(unix)]
    fn start_with_abnormal_exit_reflects_failed_state() {
        // 子プロセスが出力なしに終了しても「取得中」のまま固まらせず、予期しない
        // 終了として LocationFailure::Failed を反映する（codex-review 指摘：
        // read_lines_into が EOF/読み取りエラーで共有状態を更新しない問題の修正）。
        let shared = SharedLocation::new();
        let location = CommandLocation::start_with(shell_command("exit 7"), shared.clone())
            .expect("起動できる");
        wait_until(|| shared.last_fix().is_err());
        assert!(matches!(shared.last_fix(), Err(DeviceError::Internal(_))));
        location.stop();
    }

    #[test]
    #[cfg(unix)]
    fn stop_does_not_push_failed_for_intentional_kill() {
        // stop() による意図した終了は「予期しない終了」ではないため Failed を
        // 反映しない（denied/disabled と並ぶ区別、codex-review 指摘）。
        let shared = SharedLocation::new();
        let location = CommandLocation::start_with(shell_command("sleep 5"), shared.clone())
            .expect("起動できる");
        location.stop();
        assert_eq!(shared.last_fix().unwrap(), None);
    }

    #[test]
    fn reset_to_waiting_clears_fix_and_failure() {
        let shared = SharedLocation::new();
        shared.push_fix(valid_fix());
        shared.reset_to_waiting();
        assert_eq!(shared.last_fix().unwrap(), None);

        shared.push_failure(LocationFailure::PermissionDenied);
        shared.reset_to_waiting();
        assert_eq!(shared.last_fix().unwrap(), None);
    }

    // 呼び出し元は `#[cfg(unix)]` テストのみ（unix 専用のプロセス終了シナリオを
    // 待ち合わせるヘルパー）。wasm32 では unix cfg が成立せず呼び出し元が
    // 無いため、同じ cfg に揃えて dead_code を防ぐ。
    #[cfg(unix)]
    fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..200 {
            if condition() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("条件が満たされないままタイムアウトした");
    }

    #[test]
    fn windows_command_uses_absolute_powershell_path() {
        // Windows 以外のホストでは `PathBuf::join` がバックスラッシュ区切りの
        // パスを 1 コンポーネントとして扱うため `is_absolute()` が成立しない
        // （`file_picker`/`tts` の同型テストも同じ理由で絶対パス判定はしていない）。
        // ここでは組み立て文字列に `SystemRoot` からの絶対パス相当の断片が
        // 含まれることのみを確認する。
        let command = windows_location_command();
        let path = command.program.to_string_lossy();
        assert!(path.contains("System32"));
        assert!(path.contains("WindowsPowerShell"));
    }

    #[test]
    fn nonexistent_program_fails_to_spawn() {
        let result = CommandLocation::start_with(
            LocationCommand {
                program: PathBuf::from("/nonexistent/fandhe-location"),
                args: Vec::new(),
            },
            SharedLocation::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn start_missing_powershell_returns_unsupported() {
        // `start()`（本番経路）は `is_file()` を事前確認してから起動する。
        // `windows_location_command` を直接テストするとホスト依存になるため、
        // 存在しないパスを構成した `LocationCommand` に対する `start_with` の
        // 事前チェック相当（is_file）を通らないケースは `start` 自体の契約として
        // ここでは間接的に `nonexistent_program_fails_to_spawn` が担保する。
        // 本テストは Windows 以外のホストで `start()` 自体を呼んでも
        // `powershell.exe` が存在しないため必ず `Unsupported` になることを固定する。
        let result = CommandLocation::start(SharedLocation::new());
        #[cfg(not(target_os = "windows"))]
        assert!(matches!(result, Err(DeviceError::Unsupported("location"))));
        #[cfg(target_os = "windows")]
        drop(result);
    }
}
