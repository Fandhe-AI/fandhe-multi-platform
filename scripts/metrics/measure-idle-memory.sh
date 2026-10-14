#!/usr/bin/env bash
# アイドル時メモリ（RSS）を Xvfb + ソフトウェア GL（llvmpipe）環境で機械的・再現可能に
# 計測する読み取り専用スクリプト。
#
# 対応: REQ-6 / TASK-6.3（Issue #93、docs/spec/04-requirements.md L101-102・
# docs/spec/05-tasks.md L228-232）。PoC-4（docs/spec/03-poc/performance/README.md 2 節）の
# 計測手法（プロセスをバックグラウンド起動し n 秒後に `ps -o rss=` でサンプリング）を
# Linux ヘッドレス環境向けに踏襲する。
#
# 呼び出し文脈: メモリ比較の検証記録（非公開）の作成時に、
# fandhe-egui-proxy-desktop（対象）・scripts/metrics/baseline-blank（ベースライン）の
# 両方に対して個別に実行し、CSV 出力をレポートへ転記する。measure-code-ratio.sh・
# unsafe-audit.sh と同方針で、現時点では CI へは組み込まない（ローカル実行のみ）。
#
# 使い方:
#   bash scripts/metrics/measure-idle-memory.sh <バイナリパス> [試行回数 (既定 5)] [待機秒 (既定 3)]
#
# 出力: 標準出力に CSV（列: run,rss_kb_at_wait,rss_kb_at_10s）を書き、
# 最後に中央値・平均値のサマリを標準エラーへ出力する。
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
  echo "使い方: bash scripts/metrics/measure-idle-memory.sh <バイナリパス> [試行回数 (既定 5)] [待機秒 (既定 3)]" >&2
  exit 1
fi

TARGET_BIN="$1"
TRIALS="${2:-5}"
WAIT_SECONDS="${3:-3}"

# A03（インジェクション）対策: 回数・待機秒は数値のみ許容する（外部入力をそのままシェル
# 展開・コマンドへ渡さない）。
if ! [[ "${TRIALS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "エラー: 試行回数は正の整数で指定してください: ${TRIALS}" >&2
  exit 1
fi
if ! [[ "${WAIT_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
  echo "エラー: 待機秒は正の整数で指定してください: ${WAIT_SECONDS}" >&2
  exit 1
fi

if [ ! -x "${TARGET_BIN}" ]; then
  echo "エラー: 実行可能ファイルが見つかりません: ${TARGET_BIN}" >&2
  exit 1
fi
# 相対パスのまま子プロセスへ渡すと、後続処理で cd した場合に解決が変わりうるため、
# 呼び出し時点の絶対パスに固定する。
TARGET_BIN="$(cd -- "$(dirname -- "${TARGET_BIN}")" >/dev/null 2>&1 && pwd -P)/$(basename -- "${TARGET_BIN}")"

for cmd in Xvfb ps; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "エラー: 計測に必要なコマンドが見つかりません: ${cmd}" >&2
    exit 1
  fi
done

RSS_AT_WAIT=()
RSS_AT_10S=()

# RSS を安全にサンプリングするヘルパー。`kill -0` はゾンビプロセスに対しても成功し、
# Linux はゾンビの RSS を `0` と報告するため、生存確認だけでは数値チェックを通過してしまい
# 実サンプルとして誤記録される（Bugbot 指摘: Zombie RSS treated as valid）。
# プロセス状態（`ps -o state=`）が `Z`（ゾンビ）の場合は明示的に未取得（空文字）を返し、
# 呼び出し側で "NA" 扱いにする。
sample_rss() {
  local pid="$1"
  local state
  local rss=""
  if [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null; then
    state="$(ps -o state= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
    if [ "${state}" != "Z" ]; then
      rss="$(ps -o rss= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
    fi
  fi
  echo "${rss}"
}

# 割り込み・早期失敗（Xvfb 起動タイムアウト等）を含むあらゆる終了経路で Xvfb・対象アプリの
# プロセスを確実に kill するためのクリーンアップ。孤立プロセスがディスプレイ番号・メモリを
# 消費し続け、後続の idle-RSS 計測結果にバイアスを与えるのを防ぐ（Bugbot 指摘 1 対応）。
# xvfb_pid / app_pid は `set -u` 下でトラップから参照されるため、ループ開始前に空文字で
# 初期化し、各試行の正常終了時（既に reap 済みの箇所）にも空へ戻す。
xvfb_pid=""
app_pid=""
cleanup() {
  if [ -n "${app_pid}" ]; then
    kill "${app_pid}" 2>/dev/null || true
  fi
  if [ -n "${xvfb_pid}" ]; then
    kill "${xvfb_pid}" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "run,rss_kb_at_${WAIT_SECONDS}s,rss_kb_at_10s"

trial=1
while [ "${trial}" -le "${TRIALS}" ]; do
  # ディスプレイ番号は試行間の衝突を避けるため乱数から導出する（複数計測の並列実行や
  # 前回の残存プロセスとの衝突を避けるため、固定番号にしない）。
  display_num=$(( (RANDOM % 5000) + 100 ))
  display=":${display_num}"

  # -nolisten tcp: A05（セキュリティ設定ミス）対策としてネットワーク待ち受けを作らない
  # （ローカル専用の Unix ソケットのみで通信する）。
  Xvfb "${display}" -nolisten tcp -screen 0 1280x1024x24 >/dev/null 2>&1 &
  xvfb_pid=$!
  # Xvfb の起動完了を待つ（固定 sleep はソケット未生成のまま起動対象を走らせる事故を
  # 招くため、ソケットファイルの出現に加えて Xvfb プロセス自体の生存も確認する。
  # 生存確認はループの毎周先頭で必ず行う（ソケットの存在チェックを先に行うと、
  # 別プロセス由来の残存ソケットが既に存在する場合に一度もループ本体へ入らず、
  # 自分が起動した Xvfb がバインドに失敗していても検知できないため。Bugbot 指摘 2 対応）。
  xvfb_wait_deadline=$((SECONDS + 10))
  while true; do
    if ! kill -0 "${xvfb_pid}" 2>/dev/null \
        || [ "$(ps -o state= -p "${xvfb_pid}" 2>/dev/null | tr -d '[:space:]')" = "Z" ]; then
      echo "エラー: Xvfb（${display}）が起動直後に終了しました" >&2
      exit 1
    fi
    if [ -S "/tmp/.X11-unix/X${display_num}" ]; then
      break
    fi
    if [ "${SECONDS}" -ge "${xvfb_wait_deadline}" ]; then
      echo "エラー: Xvfb（${display}）の起動確認がタイムアウトしました" >&2
      exit 1
    fi
    sleep 0.2
  done

  DISPLAY="${display}" LIBGL_ALWAYS_SOFTWARE=1 "${TARGET_BIN}" >/dev/null 2>&1 &
  app_pid=$!

  sleep "${WAIT_SECONDS}"

  # PID の数値検証・ゾンビ判定は sample_rss 内で行う（A03 インジェクション対策、
  # ps へ渡す前段で既に整数の PID のみを使っているが二重に検証する）。
  rss_at_wait="$(sample_rss "${app_pid}")"
  if [ -z "${rss_at_wait}" ] || ! [[ "${rss_at_wait}" =~ ^[0-9]+$ ]]; then
    echo "警告: run ${trial}: ${WAIT_SECONDS} 秒経過時点でプロセスが生存していません（RSS 取得不可）" >&2
    rss_at_wait="NA"
  fi

  remaining=$((10 - WAIT_SECONDS))
  if [ "${remaining}" -gt 0 ]; then
    sleep "${remaining}"
  fi

  rss_at_10s="$(sample_rss "${app_pid}")"
  if [ -z "${rss_at_10s}" ] || ! [[ "${rss_at_10s}" =~ ^[0-9]+$ ]]; then
    echo "警告: run ${trial}: 10 秒経過時点でプロセスが生存していません（RSS 取得不可）" >&2
    rss_at_10s="NA"
  fi

  kill "${app_pid}" 2>/dev/null || true
  wait "${app_pid}" 2>/dev/null || true
  kill "${xvfb_pid}" 2>/dev/null || true
  wait "${xvfb_pid}" 2>/dev/null || true
  # 正常に reap 済みのため、EXIT トラップが PID 再利用後の無関係なプロセスを
  # kill してしまわないよう空へ戻す。
  app_pid=""
  xvfb_pid=""

  echo "${trial},${rss_at_wait},${rss_at_10s}"
  if [ "${rss_at_wait}" != "NA" ]; then
    RSS_AT_WAIT+=("${rss_at_wait}")
  fi
  if [ "${rss_at_10s}" != "NA" ]; then
    RSS_AT_10S+=("${rss_at_10s}")
  fi

  trial=$((trial + 1))
done

# 中央値・平均値の算出（sort -n で数値ソートしてから中央要素を採る）。
summarize() {
  local -n values_ref=$1
  local label="$2"
  local n=${#values_ref[@]}
  if [ "${n}" -eq 0 ]; then
    echo "サマリ（${label}）: 有効なサンプルがありません" >&2
    return
  fi
  local sorted
  # shellcheck disable=SC2207
  sorted=($(printf '%s\n' "${values_ref[@]}" | sort -n))
  local sum=0
  for v in "${sorted[@]}"; do
    sum=$((sum + v))
  done
  local mean=$((sum / n))
  local median
  if [ $((n % 2)) -eq 1 ]; then
    median="${sorted[$((n / 2))]}"
  else
    local lo=${sorted[$((n / 2 - 1))]}
    local hi=${sorted[$((n / 2))]}
    median=$(( (lo + hi) / 2 ))
  fi
  echo "サマリ（${label}）: n=${n} 平均=${mean}KB 中央値=${median}KB" >&2
}

summarize RSS_AT_WAIT "${WAIT_SECONDS}秒時点"
summarize RSS_AT_10S "10秒時点"
