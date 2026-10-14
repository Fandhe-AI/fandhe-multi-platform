# TASK-5.3（REQ-5 / M4、Issue #238 / #594）: Windows デスクトップ版のフレームレート計測。
#
# 位置づけ: `frame-metrics` feature（`crates/fandhe-egui-proxy/src/frame_metrics.rs`）が
# 出力する `FANDHE_FRAME_METRICS=<json>` を回収する。計測プロトコル（300 項目
# VirtualList・warmup 120 フレーム破棄・600 フレーム採取・p95・16.6ms 超過率）は
# PoC-4 準拠で iOS/Android/Linux と共通であり、本スクリプトは Windows 側の
# 実行・回収だけを担う。
#
# デスクトップ版は計測完了後も自動終了せず結果をオーバーレイ表示し続ける契約のため
# （Android のみ結果ファイルへ書き出す）、本スクリプトが stderr を監視して
# `FANDHE_FRAME_METRICS=` の 1 行を検出した時点でプロセスを終了させる。
#
# 呼び出し元: 人間担当（Windows 実機）。CI 組み込みは行わない。
#
# 使い方:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/measure-frame-rate.ps1 `
#     -TargetBin target/release/fandhe-egui-proxy-desktop.exe [-TimeoutSeconds 120]
#
#   事前に `frame-metrics` feature を有効にしてビルドしておくこと:
#     cargo build -p fandhe-egui-proxy --bin fandhe-egui-proxy-desktop `
#       --release --features frame-metrics
#
# 出力: 標準出力へ結果 JSON 1 行、標準エラーへ要約と計測環境（ディスプレイの
#       リフレッシュレート）。リフレッシュレートは NFR-3 の判定に直結するため必ず記録する
#       （vsync 周期がフレームタイムの下限を決めるため。60Hz でも周期 16.667ms は
#       基準値 16.6ms を上回る）。
#
# 記載禁止事項: 出力にシリアル番号等の個体識別子は含めない
# （個体識別子は記録しない方針）。GPU 名・解像度は個体識別子ではないが、
# レポートへの転記時は必要最小限に留めること。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TargetBin,

    # A03（インジェクション）対策: 秒数は数値のみ許容する。
    # 計測は warmup 120 + 採取 600 = 720 フレーム（60Hz で約 12 秒）のため、
    # 既定 120 秒は十分な余裕を持つ。
    [ValidateRange(20, 600)]
    [int]$TimeoutSeconds = 120,

    # 表示先モニタのリフレッシュレートを自動判定できない環境で、明示的に指定するための
    # 退避口（0 = 自動判定）。自動判定できないまま計測を続けると、vsync 周期の分からない
    # フレームタイムを正式値として記録してしまうため、既定は fail-closed とし本パラメータで
    # 明示させる。
    [ValidateRange(0, 1000)]
    [int]$RefreshRateHz = 0,

    # 60Hz 近傍以外のリフレッシュレートでも計測を続行する。既定では失敗させる。
    # 理由: `frame_metrics.rs` の計測プロトコルは固定 60Hz 環境を前提としており
    # （warmup 120 + 600 フレーム、budget 16.6ms）、Android 版スクリプトは
    # `peak_refresh_rate` を 60 へ強制固定してから計測する。144Hz パネルで実行すると
    # p50 が半分以下の「見かけ上優秀な」値になり、同じ形式で出力されるため他
    # プラットフォームと並べたときに比較が成立しない（AGENTS.md の「性能ゲートの無視
    # ＝計測条件の変更による比較可能性の破壊」に該当する）。
    [switch]$AllowNonStandardRefreshRate,

    # 計測ウィンドウを特定するためのタイトル。既定値は `frame-metrics` 有効ビルドが
    # `eframe::run_native` へ渡す名前（`crates/fandhe-egui-proxy/src/frame_metrics.rs`）と
    # 一致させている。通常ビルドの `fandhe-egui-proxy`（`src/main.rs`）とは別名である点に注意。
    # Process.MainWindowHandle は winit の内部ウィンドウ（クラス
    # "Winit Thread Event Target"、タイトル空、可視扱い）を返すため使えない
    # （下記 FindWindows のコメント参照）。
    [ValidateNotNullOrEmpty()]
    [string]$WindowTitle = 'fandhe-egui-proxy-frame-metrics'
)

# 60Hz 近傍と見なす範囲。59.94Hz のディスプレイは dmDisplayFrequency が 59 を返すため
# `-eq 60` の厳格判定では正当な 60Hz 系環境まで弾かれる（本検証機がこの例で、
# 検証記録（非公開）に記録している）。
$script:RefreshRateMin = 59
$script:RefreshRateMax = 61

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $TargetBin -PathType Leaf)) {
    Write-Error "実行体が見つからない: $TargetBin"
    exit 1
}
$TargetBin = (Resolve-Path -LiteralPath $TargetBin).Path

# 計測環境の記録: vsync 周期がフレームタイムの下限を決めるため、リフレッシュレートは
# 結果の解釈に必須（p95 が基準値を超えたのが描画性能の問題なのか、単に vsync 周期が
# 基準値を上回っているだけなのかは、レートが分からないと判別できない）。
#
# `Win32_VideoController` の先頭要素は使わない。複数 GPU・外付け GPU・仮想ディスプレイを
# 含む環境では、計測ウィンドウを表示しているモニタを駆動するアダプタとは限らないため。
# 代わりに「実際に計測ウィンドウが載っているモニタ」の現在の表示モードから取得する
# （`Screen.FromHandle` でウィンドウのモニタを特定 → `EnumDisplaySettings` の
# ENUM_CURRENT_SETTINGS でそのモニタの dmDisplayFrequency を読む）。
Add-Type -AssemblyName System.Windows.Forms
if (-not ('FandheDisplayMode' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class FandheDisplayMode {
    // DEVMODEW（wingdi.h）。dmFields の直後にある 16 バイトは union であり、
    // プリンタ用（short × 8）と表示用（POINTL + DWORD × 2）が同じ領域を共有する。
    // 表示モード取得にはこのうち**表示用の側だけ**を展開すればよく、プリンタ用の
    // dmOrientation〜dmPrintQuality は直列に並べない（並べると 16 バイト分ずれる）。
    // なお dmColor〜dmCollate は union の外側にある通常のフィールドである。
    //
    // 正しさは Assert-DevmodeLayout（本ファイル下部）が実行時に検証する:
    //   sizeof(DEVMODEW)               = 220
    //   offsetof(dmColor)              = 92   （= 76 + union 16 バイト）
    //   offsetof(dmDisplayFrequency)   = 184
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct DEVMODEW {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public ushort dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
        public uint dmFields;
        // ここから 16 バイトが union の表示用の側。
        public int dmPositionX, dmPositionY;
        public uint dmDisplayOrientation, dmDisplayFixedOutput;
        // ここから union の外側。
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public ushort dmLogPixels;
        public uint dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public uint dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
        public uint dmPanningWidth, dmPanningHeight;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool EnumDisplaySettingsW(string deviceName, int modeNum, ref DEVMODEW devMode);
    // modeNum = -1 は ENUM_CURRENT_SETTINGS（現在適用中のモード）。
    public static uint CurrentFrequency(string deviceName) {
        DEVMODEW dm = new DEVMODEW();
        dm.dmSize = (ushort)Marshal.SizeOf(typeof(DEVMODEW));
        if (!EnumDisplaySettingsW(deviceName, -1, ref dm)) { return 0; }
        return dm.dmDisplayFrequency;
    }

    // Process.MainWindowHandle は使えない。winit は可視ウィンドウとは別に
    // クラス名 "Winit Thread Event Target" の内部用ウィンドウを持ち、そちらが
    // MainWindowHandle として返ってくる（実測で確認）。この内部ウィンドウは画面上の
    // 位置に意味が無いため、Screen.FromHandle が計測ウィンドウとは別のモニタを
    // 返しうる。対象プロセスの可視トップレベルウィンドウのうち、指定タイトルに
    // 一致するものだけを列挙する。
    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")]
    private static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int maxCount);

    public static IntPtr[] FindWindows(int processId, string expectedTitle) {
        List<IntPtr> found = new List<IntPtr>();
        EnumWindows(delegate(IntPtr hWnd, IntPtr lParam) {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);
            if (pid != (uint)processId) { return true; }
            if (!IsWindowVisible(hWnd)) { return true; }
            StringBuilder sb = new StringBuilder(512);
            GetWindowTextW(hWnd, sb, sb.Capacity);
            if (sb.ToString() == expectedTitle) { found.Add(hWnd); }
            return true;
        }, IntPtr.Zero);
        return found.ToArray();
    }
}
'@
}

# DEVMODEW のマーシャリングが Windows SDK のレイアウトと一致することを実行時に検証する。
# 構造体定義を誤ると EnumDisplaySettings は「成功したうえで別の領域を読んだ値」を返しうる
# （＝誤ったリフレッシュレートで計測を受理してしまい、比較可能性が壊れる）。
# 期待値は wingdi.h の DEVMODEW から算出したもので、一致しなければ fail-closed にする。
function Assert-DevmodeLayout {
    # PS 5.1 では Type を変数経由で渡すと SizeOf(object) の多重定義へ解決されてしまうため、
    # 型リテラルを直接書いて SizeOf(Type) へ束縛させる。
    $size = [System.Runtime.InteropServices.Marshal]::SizeOf([type]'FandheDisplayMode+DEVMODEW')
    $offColor = [System.Runtime.InteropServices.Marshal]::OffsetOf([type]'FandheDisplayMode+DEVMODEW', 'dmColor').ToInt32()
    $offFreq = [System.Runtime.InteropServices.Marshal]::OffsetOf([type]'FandheDisplayMode+DEVMODEW', 'dmDisplayFrequency').ToInt32()

    $expected = @(
        @{ Name = 'sizeof(DEVMODEW)'; Actual = $size; Expected = 220 },
        @{ Name = 'offsetof(dmColor)'; Actual = $offColor; Expected = 92 },
        @{ Name = 'offsetof(dmDisplayFrequency)'; Actual = $offFreq; Expected = 184 }
    )
    $bad = @($expected | Where-Object { $_.Actual -ne $_.Expected })
    if ($bad.Count -gt 0) {
        $detail = ($bad | ForEach-Object { "$($_.Name): 実測 $($_.Actual) / 期待 $($_.Expected)" }) -join ' / '
        Write-Error "DEVMODEW のレイアウトが Windows SDK と一致しない（$detail）。リフレッシュレートを誤読する恐れがあるため計測を中止する。"
        exit 1
    }
}
Assert-DevmodeLayout

# 計測ウィンドウ（可視・指定タイトル）のハンドルを 1 つ特定する。
# 見つからない場合・複数ある場合は $null を返し、呼び出し側で fail-closed にする。
function Get-MeasurementWindowHandle {
    param([int]$ProcessId, [string]$ExpectedTitle)

    $handles = [FandheDisplayMode]::FindWindows($ProcessId, $ExpectedTitle)
    if ($handles.Count -eq 1) { return $handles[0] }
    if ($handles.Count -eq 0) { return $null }
    # 同一タイトルの可視ウィンドウが複数ある場合、どれが計測対象か判定できない。
    Write-Warning "タイトル '$ExpectedTitle' の可視ウィンドウが $($handles.Count) 個ある。表示先モニタを一意に特定できない。"
    return $null
}

# ウィンドウハンドルから表示先モニタのリフレッシュレートを得る。
# 取得できない場合は 0 を返し、呼び出し側で fail-closed にする。
function Get-WindowRefreshRate {
    param([IntPtr]$WindowHandle)

    if ($WindowHandle -eq [IntPtr]::Zero) { return $null }
    $screen = [System.Windows.Forms.Screen]::FromHandle($WindowHandle)
    if ($null -eq $screen) { return $null }
    $hz = [FandheDisplayMode]::CurrentFrequency($screen.DeviceName)
    # Win32 の仕様上 0 と 1 は「ハードウェア既定」を意味する番兵値であり、
    # 実際のレートではない。周期を算出できないため無効として扱う。
    if ($hz -le 1) { return [pscustomobject]@{ Device = $screen.DeviceName; Hz = $null } }
    return [pscustomobject]@{ Device = $screen.DeviceName; Hz = [int]$hz }
}

$errFile = [System.IO.Path]::GetTempFileName()
# stdout も一時ファイルへ捨てる。対象は console subsystem のバイナリのため、
# 放置すると子の stdout が親へ流れ込み、本スクリプトが stdout へ出す結果 JSON に
# 混入する（既存 bash 版が子出力を遮断しているのと同じ意図）。
$outFile = [System.IO.Path]::GetTempFileName()
$json = $null
$stderrTail = $null
# finally が $proc の未定義参照に依存しないよう明示的に初期化する。
$proc = $null
try {
    $proc = Start-Process -FilePath $TargetBin -PassThru -NoNewWindow `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile

    if ($RefreshRateHz -gt 0) {
        $refresh = $RefreshRateHz
        [Console]::Error.WriteLine("display : -RefreshRateHz による明示指定")
    } else {
        # 表示先モニタの判定はウィンドウが生成された後でなければできないため、
        # 計測ウィンドウが現れるまで待つ。計測本体（warmup 120 フレーム）より前に完了する。
        $hwnd = [IntPtr]::Zero
        $hwndDeadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $hwndDeadline) {
            if ($proc.HasExited) { break }
            $found = Get-MeasurementWindowHandle -ProcessId $proc.Id -ExpectedTitle $WindowTitle
            if ($null -ne $found) { $hwnd = $found; break }
            Start-Sleep -Milliseconds 200
        }

        $info = Get-WindowRefreshRate -WindowHandle $hwnd
        if ($null -eq $info -or $null -eq $info.Hz) {
            $reason = if ($null -eq $info) { "計測ウィンドウ（タイトル '$WindowTitle'）を特定できなかった" }
                      else { "表示先 $($info.Device) の現在のリフレッシュレートを取得できなかった（Win32 が既定値の番兵を返した）" }
            # fail-closed: vsync 周期が不明なままフレームタイムを正式値として出力すると、
            # 基準超過が描画性能の問題なのか周期の問題なのか判別できない記録が残る。
            Write-Error "$reason。-RefreshRateHz でモニタのリフレッシュレートを明示指定して再実行すること。"
            exit 1
        }
        $refresh = $info.Hz
        [Console]::Error.WriteLine("display : $($info.Device)")
    }
    $vsyncMs = [math]::Round(1000.0 / $refresh, 3).ToString([System.Globalization.CultureInfo]::InvariantCulture)
    [Console]::Error.WriteLine("refresh : $refresh Hz")
    [Console]::Error.WriteLine("vsync   : $vsyncMs ms/frame")

    # 固定 60Hz 前提のプロトコルであるため、60Hz 近傍以外は既定で計測を打ち切る。
    if ($refresh -lt $script:RefreshRateMin -or $refresh -gt $script:RefreshRateMax) {
        if (-not $AllowNonStandardRefreshRate) {
            Write-Error @"
リフレッシュレート $refresh Hz は 60Hz 近傍（$($script:RefreshRateMin)〜$($script:RefreshRateMax) Hz）ではない。
本計測のプロトコル（frame_metrics.rs）は固定 60Hz 環境を前提としており、他プラットフォーム
（Android は peak_refresh_rate を 60 へ固定して計測）と比較可能な値にならない。
ディスプレイ設定を 60Hz へ変更して再実行するか、参考値として記録する場合は
-AllowNonStandardRefreshRate を指定すること。
"@
            exit 1
        }
        Write-Warning "リフレッシュレート $refresh Hz は 60Hz 近傍ではない。-AllowNonStandardRefreshRate 指定のため続行するが、他プラットフォームとの比較には使えない。レポートには必ずレートを併記すること。"
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $exitedEarly = $false
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            $exitedEarly = $true
            break
        }
        Start-Sleep -Milliseconds 500

        $text = Get-Content -LiteralPath $errFile -Raw -ErrorAction SilentlyContinue
        if ($null -eq $text) { continue }
        $m = [regex]::Match($text, 'FANDHE_FRAME_METRICS=(\{[^\r\n]*\})')
        if ($m.Success) {
            $json = $m.Groups[1].Value
            break
        }
    }

    # ループ脱出後にもう一度だけ読む。ポーリングは「生存確認 → 500ms 待機 → 読み取り」の
    # 順であるため、メトリクス行が書かれた直後にプロセスが終了した場合や期限境界で
    # 抜けた場合に、成立している計測を読まないまま捨ててしまう（false negative）。
    if ($null -eq $json) {
        $text = Get-Content -LiteralPath $errFile -Raw -ErrorAction SilentlyContinue
        if ($null -ne $text) {
            $m = [regex]::Match($text, 'FANDHE_FRAME_METRICS=(\{[^\r\n]*\})')
            if ($m.Success) { $json = $m.Groups[1].Value }
        }
    }
    if ($exitedEarly -and $null -eq $json) {
        Write-Warning '計測完了前にプロセスが終了した'
    }
}
finally {
    # 本スクリプトはプロセスの終了コードを計測の有効性判定に使わない。
    # `frame-metrics` 有効ビルドのデスクトップ版は計測完了後も自動終了せず結果を
    # オーバーレイ表示し続ける契約であり（`src/frame_metrics.rs`。結果ファイルへの
    # 書き出しは Android のみ）、本スクリプト側が CloseMainWindow / Kill で終了させる。
    # したがって終了コードは「こちらが終了させた事実」を反映するだけで、計測が
    # 成立したかどうかは示さない。有効性の判定は `FANDHE_FRAME_METRICS=` 行を
    # 回収できたかどうかで行う（回収できなければ下で非ゼロ終了する）。
    if ($null -ne $proc -and -not $proc.HasExited) {
        try { $proc.CloseMainWindow() | Out-Null } catch {}
        Start-Sleep -Milliseconds 500
        if (-not $proc.HasExited) { try { $proc.Kill() } catch {} }
    }
    if ($null -ne $proc) { try { $proc.WaitForExit(5000) | Out-Null } catch {} }

    # 一時ファイルの削除は必ずこの finally で行う。try 節の外に置くと、Start-Process や
    # ポーリング中の Get-Content が terminating error になった場合（$ErrorActionPreference
    # = 'Stop'）にスクリプトが抜けて一時ファイルが残り、反復実行する計測スクリプトの
    # 冪等性を損なう。失敗時の根拠として使う末尾行は削除前にここで確保する。
    if ($null -eq $json) {
        $stderrTail = Get-Content -LiteralPath $errFile -Tail 20 -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $errFile, $outFile -Force -ErrorAction SilentlyContinue
}

if ($null -eq $json) {
    [Console]::Error.WriteLine('--- stderr（末尾）---')
    if ($null -ne $stderrTail) { $stderrTail | ForEach-Object { [Console]::Error.WriteLine($_) } }
    Write-Error "${TimeoutSeconds}s 以内に FANDHE_FRAME_METRICS を回収できなかった。"
    exit 1
}

Write-Output $json

# 要約の整形。Windows PowerShell 5.1 は文字列内 `$()` の入れ子クォートを
# 解釈できないため、事前に変数へ落としてから埋め込む。
$parsed = $json | ConvertFrom-Json
$p50 = $parsed.p50_ms
$p95 = $parsed.p95_ms
$maxMs = $parsed.max_ms
$overRatio = $parsed.over_budget_ratio
$overPct = [math]::Round($overRatio * 100, 2)
$count = $parsed.sample_count

[Console]::Error.WriteLine('')
[Console]::Error.WriteLine("samples : $count フレーム")
[Console]::Error.WriteLine("p50     : $p50 ms")
[Console]::Error.WriteLine("p95     : $p95 ms")
[Console]::Error.WriteLine("max     : $maxMs ms")
[Console]::Error.WriteLine("over16.6: $overPct %")
