# TASK-5.1（REQ-5 / M4、Issue #236 / #594）: Windows デスクトップ版のコールドスタート計測。
#
# 位置づけ: `cold-start-probe` feature（`crates/fandhe-egui-proxy/src/cold_start.rs`）が
# 出力する `COLD_START_MS=<value>;` を N 回分回収し、中央値を採る。計測定義（t0 =
# `fn main` 冒頭、t1 = 初回 `eframe::App::update` 完了）は iOS/Android/macOS と共通で、
# 本スクリプトは Windows 側の実行・集計だけを担う。
#
# PowerShell である理由: 既存の計測スクリプトは bash（`scripts/ios/measure-cold-start.sh`
# 等）だが、Windows には `ps`/`/proc` が無く、後続のメモリ計測（measure-idle-memory.ps1）
# が `Get-Process` を必要とする。`scripts/apple/measure-idle-memory-macos.sh` が
# Linux 版の macOS 対応物として別実装を持つのと同じ位置づけで、Windows 対応物を置く。
#
# 呼び出し元: 人間担当（Windows 実機）。CI 組み込みは行わない
# （既存 measure-*.sh と同じ運用方針。ci.md の CI 制約とは独立）。
#
# 使い方:
#   powershell -ExecutionPolicy Bypass -File scripts/windows/measure-cold-start.ps1 `
#     -TargetBin target/release/fandhe-egui-proxy-desktop.exe [-Trials 5]
#
#   事前に `cold-start-probe` feature を有効にしてビルドしておくこと:
#     cargo build -p fandhe-egui-proxy --bin fandhe-egui-proxy-desktop `
#       --release --features cold-start-probe
#
# 出力: 標準出力へ CSV（列: run,cold_start_ms）、標準エラーへ中央値・平均値。
#       既存 measure-idle-memory-macos.sh と同じ形式に揃える。
#
# 記載禁止事項: 出力にシリアル番号等の個体識別子は含めない
# （個体識別子は記録しない方針）。本スクリプトは経過時間しか扱わない。

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TargetBin,

    # A03（インジェクション）対策: 回数は数値のみ許容する
    # （measure-idle-memory-macos.sh の引数検証と同じ方針）。
    # PowerShell の型制約で非数値は束縛時に弾かれるため、ここでは範囲のみ検証する。
    [ValidateRange(1, 100)]
    [int]$Trials = 5,

    # 起動が完了せずハングした場合に試行を打ち切る秒数。ゾンビ化した試行を
    # 有効なサンプルとして記録しないための安全弁（macOS 版のゾンビ除外に相当）。
    [ValidateRange(5, 600)]
    [int]$TimeoutSeconds = 60,

    # 試行間の安定待ち秒数。`scripts/android/measure-cold-start.sh`（sleep 2）・
    # `scripts/ios/measure-cold-start.sh`（sleep 2）と同じく、直前の試行の後始末
    # （プロセス終了・GPU/ファイルキャッシュの落ち着き）を待ってから次を起動する。
    [ValidateRange(0, 60)]
    [int]$SettleSeconds = 2,

    # 有効サンプルが -Trials に満たなくても中央値を出す。既定では失敗させる。
    # 理由: 本スクリプトの出力は検証レポートへ転記され、他プラットフォームの
    # 「5 試行の中央値」と並べて比較される。破棄が出たまま黙って少ない標本の中央値を
    # 返すと、計測条件が揃っていない値が同じ土俵に載る（AGENTS.md の「性能ゲートの無視
    # ＝計測条件の変更による比較可能性の破壊」に該当する）。環境要因で規定回数を
    # 揃えられず、実測できた分だけを記録すると判断した場合にのみ明示的に指定する。
    [switch]$AllowPartial
)

$ErrorActionPreference = 'Stop'

# Windows 専用ガード: `Get-Process`/`WorkingSet64` 等の Windows 前提が成立しない環境で
# 黙って別 OS の値を「Windows 計測結果」の形式で出さないための fail-fast
# （`scripts/apple/measure-idle-memory-macos.sh` の macOS 専用ガードと同じ方針）。
# Windows PowerShell 5.1 には `$IsWindows` が存在せず $null になるため使わない。
if ($env:OS -ne 'Windows_NT') {
    Write-Error @'
本スクリプトは Windows 専用です。
  Linux（Xvfb）: scripts/metrics/measure-idle-memory.sh
  macOS        : scripts/apple/measure-idle-memory-macos.sh
  Android      : scripts/android/measure-cold-start.sh
  iOS          : scripts/ios/measure-cold-start.sh
'@
    exit 1
}

if (-not (Test-Path -LiteralPath $TargetBin -PathType Leaf)) {
    Write-Error "実行体が見つからない: $TargetBin"
    exit 1
}
# 以降は絶対パスで扱う（作業ディレクトリに依存しないため）。
$TargetBin = (Resolve-Path -LiteralPath $TargetBin).Path

$samples = New-Object System.Collections.Generic.List[int]
$failures = 0

Write-Output 'run,cold_start_ms'

for ($i = 1; $i -le $Trials; $i++) {
    if ($i -gt 1 -and $SettleSeconds -gt 0) { Start-Sleep -Seconds $SettleSeconds }

    # `Start-Process -PassThru` ではなく ProcessStartInfo から起動する。
    # 前者は本環境（Windows PowerShell 5.1）では `WaitForExit()` を呼んだ後でも
    # `ExitCode` が $null のままで、異常終了した試行を判別できない（実測で確認済み。
    # 正常終了する実行体・非 0 で終了する実行体の双方で $null だった）。
    # `[System.Diagnostics.Process]::Start` は自プロセスがハンドルを保持するため
    # `ExitCode` が確実に取得でき、stderr もストリームで直接読める（一時ファイル不要）。
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $TargetBin
    $psi.UseShellExecute = $false
    $psi.RedirectStandardError = $true
    # stdout も必ずリダイレクトして読み捨てる。対象は console subsystem のバイナリ
    # （main.rs に windows_subsystem 属性なし）であり、放置すると子の stdout が親へ
    # 流れ込んで本スクリプトの CSV へ混入する（既存 bash 版が子出力を遮断しているのと
    # 同じ意図）。読み捨てないとパイプのバッファが埋まって停止しうるため必ず drain する。
    $psi.RedirectStandardOutput = $true
    $proc = $null
    try {
        $proc = [System.Diagnostics.Process]::Start($psi)
        # ストリームは先に非同期で読み始める。`WaitForExit` より後に ReadToEnd すると
        # パイプのバッファが埋まった場合に相互待ちになりうるため。
        $errTask = $proc.StandardError.ReadToEndAsync()
        $outTask = $proc.StandardOutput.ReadToEndAsync()

        if (-not $proc.WaitForExit($TimeoutSeconds * 1000)) {
            # 期限内に終了しなかった試行は破棄する。`cold-start-probe` 有効ビルドは
            # ログ出力後に ViewportCommand::Close で自動終了する契約のため、
            # ここに来る場合は計測として成立していない。
            try { $proc.Kill() } catch {}
            $proc.WaitForExit()
            Write-Warning "run $i : ${TimeoutSeconds}s 以内に終了しなかったため破棄"
            $failures++
            continue
        }
        # 時間指定の WaitForExit(int) の後に引数なしの WaitForExit() を呼び、
        # 非同期の出力読み取りが完了した状態を保証してから読み出す（.NET の推奨手順）。
        $proc.WaitForExit()

        $stderrText = $errTask.Result
        [void]$outTask.Result
        if ($null -eq $stderrText) { $stderrText = '' }

        # 異常終了した試行は破棄する。プローブ出力後のクラッシュ・終了処理の失敗を
        # 正常な試行として中央値へ含めると、計測条件の揃わない値がレポートへ載る。
        if ($proc.ExitCode -ne 0) {
            Write-Warning "run $i : 異常終了（exit=$($proc.ExitCode)）のため破棄"
            $failures++
            continue
        }

        # 末尾の `;` を必須にする。cold_start.rs のコメントどおり、擬似 TTY 経由の記録では
        # 折り返しで数値の途中に改行が入りうるため、`;` が無い場合は抽出失敗（fail-fast）
        # に倒して過小・過大な値を黙って採らない。
        $m = [regex]::Match($stderrText, 'COLD_START_MS=(\d+);')
        if (-not $m.Success) {
            Write-Warning "run $i : COLD_START_MS を抽出できなかったため破棄（exit=$($proc.ExitCode)）"
            $failures++
            continue
        }

        $ms = [int]$m.Groups[1].Value
        $samples.Add($ms)
        Write-Output "$i,$ms"
    }
    finally {
        # タイムアウト以外の異常経路（ストリーム読み取りの例外等）でも計測対象を
        # 残さない。idle-memory / frame-rate の finally と同じ後始末。
        if ($null -ne $proc) {
            if (-not $proc.HasExited) {
                try { $proc.Kill() } catch {}
                try { $proc.WaitForExit(5000) | Out-Null } catch {}
            }
            $proc.Dispose()
        }
    }
}

if ($samples.Count -eq 0) {
    Write-Error "有効なサンプルが 0 件（失敗 $failures 件）。計測は成立していない。"
    exit 1
}
if ($samples.Count -lt $Trials) {
    $msg = "有効サンプルが $($samples.Count) 件で要求試行数 $Trials に満たない（破棄 $failures 件）。"
    if (-not $AllowPartial) {
        Write-Error "$msg 計測条件を揃えるため再実行すること。実測できた分だけを記録する場合は -AllowPartial を指定する。"
        exit 1
    }
    Write-Warning "$msg -AllowPartial 指定のため続行する。レポートには有効件数を必ず併記すること。"
}

$sorted = @($samples | Sort-Object)
# 中央値の定義は同一メトリクスの既存実装（`scripts/android/measure-cold-start.sh` /
# `scripts/ios/measure-cold-start.sh` の `median()`: `mid_index=$(((count + 1) / 2))`）に
# 合わせて**下側中央値**とする。既定の 5 試行では奇数のため差は出ないが、
# -AllowPartial で偶数件になった場合に定義がずれないようにする
# （メモリ系スクリプトは中央 2 値の平均を採る別定義であり、そちらとは揃えない）。
# `[int]` は PS 5.1 で銀行丸め（.5 は偶数側へ）になり、bash の整数除算とは一致しない。
# 例: 件数 2 なら bash は 1（下側）だが [int](3/2) は 2（上側）を選ぶ。件数 6・10 でも同様。
# 切り捨てを明示して android/ios 版の `$(((count + 1) / 2))` と同じ挙動にする。
$median = $sorted[[int][math]::Floor(($sorted.Count + 1) / 2) - 1]
$mean = ($samples | Measure-Object -Average).Average

# Windows PowerShell 5.1 は文字列内 `$()` の入れ子クォートを解釈できないため、
# 整形は必ず事前に変数へ落としてから埋め込む。小数はロケール依存の区切り文字を
# 避けるため InvariantCulture で整形する。
$sampleList = $samples -join ', '
$sampleCount = $samples.Count
$meanRounded = [math]::Round($mean, 1).ToString([System.Globalization.CultureInfo]::InvariantCulture)

[Console]::Error.WriteLine("samples : $sampleList ms")
[Console]::Error.WriteLine("count   : $sampleCount 件（破棄 $failures 件）")
[Console]::Error.WriteLine("median  : $median ms")
[Console]::Error.WriteLine("mean    : $meanRounded ms")
