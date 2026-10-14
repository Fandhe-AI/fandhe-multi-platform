# TASK-5.2（REQ-5 / REQ-6、Issue #237 / #594）: Windows 上でのアイドル時メモリ計測。
#
# 位置づけ: `scripts/metrics/measure-idle-memory.sh`（Linux・Xvfb 版）および
# `scripts/apple/measure-idle-memory-macos.sh`（macOS 版）の Windows 対応物。
# Windows は実ディスプレイが常時利用可能なため Xvfb 相当の処理は不要（そのまま起動する）。
#
# 採取指標: Windows には `ps -o rss=` が無いため、`Get-Process` の
# `WorkingSet64`（物理メモリ常駐量。Linux/macOS の RSS に最も近い）を主指標とする。
# 併せて `PrivateMemorySize64`（他プロセスと共有しないコミット量）も記録する。
# 共有ライブラリの扱いが OS 間で異なるため、他プラットフォームの RSS との単純比較は
# できない。この点はレポート側に明記すること。
#
# ベースライン差し引き: `scripts/metrics/baseline-blank`（winit のみで空ウィンドウを
# 生成する最小アプリ）を同じ手順で計測し、差分を「増分」として算出する。
# ただし baseline-blank は GL コンテキストを一切初期化しないため、増分には
# 「egui/eframe 固有のメモリ」に加えて「GL コンテキスト自体のメモリ」が混在する
# （baseline-blank/src/main.rs の計測上の注意を参照）。**増分は参考指標とし、
# 生の WorkingSet64 を主指標として扱う。**
#
# 呼び出し元: 人間担当（Windows 実機）。CI 組み込みは行わない。
#
# 使い方:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/measure-idle-memory.ps1 `
#     -TargetBin target/release/fandhe-egui-proxy-desktop.exe `
#     [-BaselineBin <path>] [-Trials 5] [-WaitSeconds 3]
#
#   対象は `cold-start-probe` feature を**無効**にしてビルドすること（同 feature の
#   有効ビルドは初回フレーム後に自動終了するため、アイドル計測が成立しない）:
#     cargo build -p fandhe-egui-proxy --bin fandhe-egui-proxy-desktop --release
#
# 出力: 標準出力へ CSV（列: target,run,ws_kb_at_<待機秒>s,ws_kb_at_10s,
#       priv_kb_at_<待機秒>s,priv_kb_at_10s）、標準エラーへ待機時点・10 秒時点それぞれの
#       件数・平均値・中央値と、10 秒時点の増分。既存 bash 版の summarize と同じ形式。
#
# 記載禁止事項: 出力にシリアル番号等の個体識別子は含めない
# （個体識別子は記録しない方針）。本スクリプトはメモリ量しか扱わない。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TargetBin,

    # 未指定時はスクリプト自身の位置から解決する（既存 bash 版が script_dir から
    # repo_root を導出するのと同じ方針）。cwd 相対にすると、リポジトリルート以外から
    # 実行したときに「ベースラインが見つからない」という誤誘導的な失敗になる。
    # 既定値の式ではなく束縛後に解決する: Windows PowerShell 5.1 では param() の
    # 既定値評価時点で $PSScriptRoot がまだ空文字列であり、Join-Path が失敗する。
    [string]$BaselineBin = '',

    # A03（インジェクション）対策: 回数・待機秒は数値のみ許容する
    # （measure-idle-memory-macos.sh の引数検証と同じ方針）。
    [ValidateRange(1, 100)]
    [int]$Trials = 5,

    # 上限を 10 に制限する。本スクリプトは「待機時点」と「10 秒時点」の 2 点を採取する
    # 契約（measure-idle-memory.sh / measure-idle-memory-macos.sh と共通）であり、
    # 11 以上を許すと 2 点目の待機が `Max(0, 10 - WaitSeconds)` = 0 秒になって
    # 1 点目と同じ値が 10 秒時点として記録され、実際には 10 秒時点ではない値を
    # 10 秒時点の中央値として扱ってしまう（他プラットフォームとの比較可能性が壊れる）。
    [ValidateRange(1, 10)]
    [int]$WaitSeconds = 3,

    # 有効サンプルが -Trials に満たなくても増分を算出する。既定では失敗させる。
    # 理由: 本スクリプトは対象とベースラインの中央値の差を「増分」として出す。両者の
    # 標本数が揃っていないと（例: fandhe 5 件・baseline 1 件）、条件の異なる 2 値の差を
    # 正式な増分として記録することになり比較が成立しない（AGENTS.md の性能計測の
    # 比較可能性）。環境要因で規定回数を揃えられない場合にのみ明示的に指定する。
    [switch]$AllowPartial
)

$ErrorActionPreference = 'Stop'

# Windows 専用ガード（measure-cold-start.ps1 と同じ理由）。
# Windows PowerShell 5.1 には `$IsWindows` が存在せず $null になるため使わない。
if ($env:OS -ne 'Windows_NT') {
    Write-Error @'
本スクリプトは Windows 専用です。
  Linux（Xvfb）: scripts/metrics/measure-idle-memory.sh
  macOS        : scripts/apple/measure-idle-memory-macos.sh
'@
    exit 1
}

function Resolve-Bin {
    param([string]$Path, [string]$Label)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Write-Error "${Label}の実行体が見つからない: $Path"
        exit 1
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

if ([string]::IsNullOrWhiteSpace($BaselineBin)) {
    $BaselineBin = Join-Path $PSScriptRoot '..\metrics\baseline-blank\target\release\baseline-blank.exe'
}

$TargetBin = Resolve-Bin -Path $TargetBin -Label '計測対象'
$BaselineBin = Resolve-Bin -Path $BaselineBin -Label 'ベースライン'

# 1 プロセス分の計測。待機秒後と 10 秒後の 2 点でサンプリングする
# （measure-idle-memory-macos.sh と同じ 2 点採取）。
function Measure-Process {
    param([string]$Bin, [string]$Label, [int]$Run)

    # stdout/stderr は一時ファイルへ捨てる。対象は console subsystem のバイナリのため、
    # 放置すると子の出力が本スクリプトの CSV へ混入する。-NoNewWindow を併用して
    # 試行ごとにコンソールウィンドウが開く副作用も抑止する。
    $outFile = [System.IO.Path]::GetTempFileName()
    $errFile = [System.IO.Path]::GetTempFileName()
    $proc = $null
    try {
        $proc = Start-Process -FilePath $Bin -PassThru -NoNewWindow `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile

        # 1 点目・2 点目とも「起動継続中であること」を確認してから読む。
        # HasExited の確認と読み取りの間にプロセスが終了すると WorkingSet64 の再取得が
        # 例外になりうるため try/catch で包み、その試行だけを破棄する（1 試行の異常で
        # 計測全体が中断しないようにする。ラウンド 1 の「空 stderr で全体中断」と同型）。
        Start-Sleep -Seconds $WaitSeconds
        $atWait = Read-MemorySample -Proc $proc -Label $Label -Run $Run -When "$WaitSeconds 秒時点"
        if ($null -eq $atWait) { return $null }

        Start-Sleep -Seconds ([math]::Max(0, 10 - $WaitSeconds))
        $at10 = Read-MemorySample -Proc $proc -Label $Label -Run $Run -When '10 秒時点'
        if ($null -eq $at10) { return $null }

        return [pscustomobject]@{
            WsWait   = $atWait.Ws
            Ws10     = $at10.Ws
            PrivWait = $atWait.Priv
            Priv10   = $at10.Priv
        }
    }
    finally {
        if ($null -ne $proc) {
            if (-not $proc.HasExited) {
                try { $proc.CloseMainWindow() | Out-Null } catch {}
                Start-Sleep -Milliseconds 500
                if (-not $proc.HasExited) { try { $proc.Kill() } catch {} }
            }
            try { $proc.WaitForExit(5000) | Out-Null } catch {}
        }
        Remove-Item -LiteralPath $outFile, $errFile -Force -ErrorAction SilentlyContinue
    }
}

# 生存確認 → メモリ読み取りを 1 セットで行う。取得できなければ $null（＝その試行は破棄）。
function Read-MemorySample {
    param([System.Diagnostics.Process]$Proc, [string]$Label, [int]$Run, [string]$When)

    try {
        if ($Proc.HasExited) {
            Write-Warning "$Label run $Run : $When より前に終了したため破棄"
            return $null
        }
        $Proc.Refresh()
        $ws = [int]($Proc.WorkingSet64 / 1KB)
        $priv = [int]($Proc.PrivateMemorySize64 / 1KB)
    }
    catch {
        # HasExited 確認後の終了（レース）等で情報取得に失敗した場合。
        Write-Warning "$Label run $Run : $When のメモリ取得に失敗したため破棄（$($_.Exception.GetType().Name)）"
        return $null
    }
    # 終了直後のプロセスは 0 を返しうる。0 を有効値として中央値へ混ぜない。
    if ($ws -le 0 -or $priv -le 0) {
        Write-Warning "$Label run $Run : $When のメモリが 0 以下（ws=$ws priv=$priv）のため破棄"
        return $null
    }
    return [pscustomobject]@{ Ws = $ws; Priv = $priv }
}

# 中央 2 値の平均を採る。メモリ系の既存実装（measure-idle-memory.sh /
# measure-idle-memory-macos.sh の summarize）と同じ定義。
function Get-Median {
    param([int[]]$Values)
    $sorted = @($Values | Sort-Object)
    if ($sorted.Count % 2 -eq 1) {
        return $sorted[[int](($sorted.Count - 1) / 2)]
    }
    return ($sorted[$sorted.Count / 2 - 1] + $sorted[$sorted.Count / 2]) / 2
}

# 待機秒を CSV ヘッダーへ埋め込む。既存 bash 版が `rss_kb_at_${WAIT_SECONDS}s` と
# するのと同じ。1 点目が何秒時点の値かがデータ自体に残らないと、転記後に計測条件を
# 検証できなくなる。
Write-Output "target,run,ws_kb_at_${WaitSeconds}s,ws_kb_at_10s,priv_kb_at_${WaitSeconds}s,priv_kb_at_10s"

$results = @{}
foreach ($entry in @(
        @{ Label = 'fandhe'; Bin = $TargetBin },
        @{ Label = 'baseline'; Bin = $BaselineBin }
    )) {
    $label = $entry.Label
    $wsWaitList = New-Object System.Collections.Generic.List[int]
    $ws10List = New-Object System.Collections.Generic.List[int]
    for ($i = 1; $i -le $Trials; $i++) {
        $r = Measure-Process -Bin $entry.Bin -Label $label -Run $i
        if ($null -eq $r) { continue }
        Write-Output "$label,$i,$($r.WsWait),$($r.Ws10),$($r.PrivWait),$($r.Priv10)"
        $wsWaitList.Add($r.WsWait)
        $ws10List.Add($r.Ws10)
    }
    if ($ws10List.Count -eq 0) {
        Write-Error "${label}: 有効なサンプルが 0 件。計測は成立していない。"
        exit 1
    }
    if ($ws10List.Count -lt $Trials) {
        $msg = "${label}: 有効サンプルが $($ws10List.Count) 件で要求試行数 $Trials に満たない。"
        if (-not $AllowPartial) {
            Write-Error "$msg 対象とベースラインの標本数を揃えるため再実行すること。実測できた分だけを記録する場合は -AllowPartial を指定する。"
            exit 1
        }
        Write-Warning "$msg -AllowPartial 指定のため続行する。"
    }
    $results[$label] = [pscustomobject]@{ AtWait = $wsWaitList; At10 = $ws10List }
}

# -AllowPartial で続行した場合でも、両者の標本数が異なる差分は比較として成立しないため
# 必ず明示する（macOS 版 summarize が n= を必ず出すのと同じ方針）。
$fandheN = $results['fandhe'].At10.Count
$baselineN = $results['baseline'].At10.Count
if ($fandheN -ne $baselineN) {
    Write-Warning "対象($fandheN 件)とベースライン($baselineN 件)で有効サンプル数が異なる。増分は条件の揃わない 2 値の差であり、レポートへ転記する際は必ずその旨を併記すること。"
}

# 待機時点・10 秒時点の両方について n・平均・中央値を出す（macOS 版 summarize と同じ）。
# Windows PowerShell 5.1 は文字列内 `$()` の入れ子クォートを解釈できないため、
# 整形は必ず事前に変数へ落としてから埋め込む。
$inv = [System.Globalization.CultureInfo]::InvariantCulture
foreach ($label in @('fandhe', 'baseline')) {
    foreach ($point in @(
            @{ Name = "$WaitSeconds 秒時点"; Values = $results[$label].AtWait },
            @{ Name = '10 秒時点'; Values = $results[$label].At10 }
        )) {
        $vals = $point.Values
        $n = $vals.Count
        $avg = [math]::Round(($vals | Measure-Object -Average).Average, 1).ToString($inv)
        $med = (Get-Median -Values $vals.ToArray()).ToString($inv)
        $name = $point.Name
        [Console]::Error.WriteLine("サマリ（$label / $name）: n=$n 平均=$avg KB 中央値=$med KB")
    }
}

$fandheMedian = Get-Median -Values $results['fandhe'].At10.ToArray()
$baselineMedian = Get-Median -Values $results['baseline'].At10.ToArray()
$deltaKb = $fandheMedian - $baselineMedian

$fandheMb = [math]::Round($fandheMedian / 1024, 1).ToString($inv)
$baselineMb = [math]::Round($baselineMedian / 1024, 1).ToString($inv)
$deltaMb = [math]::Round($deltaKb / 1024, 1).ToString($inv)

[Console]::Error.WriteLine('')
[Console]::Error.WriteLine("fandhe   n=$fandheN WorkingSet64 中央値（10s 時点）: $fandheMedian KB（$fandheMb MB）")
[Console]::Error.WriteLine("baseline n=$baselineN WorkingSet64 中央値（10s 時点）: $baselineMedian KB（$baselineMb MB）")
[Console]::Error.WriteLine("増分（参考指標）                        : $deltaKb KB（$deltaMb MB）")
[Console]::Error.WriteLine('注: 増分には GL コンテキスト自体のメモリが混在する。主指標は生の WorkingSet64。')
