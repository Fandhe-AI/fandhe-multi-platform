#!/usr/bin/env bash
# TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）: macOS 上でのアイドル時メモリ（RSS）
# 再計測スクリプト。
#
# 位置づけ: TASK-6.3（#93）が
# 申し送った「macOS 実機（またはプロキシ環境）で fandhe-egui-proxy-desktop の feature
# 絞り込み前後のアイドル時メモリを PoC-4 と同一手法（`ps -o rss=`、空 NSWindow
# ベースライン差し引き）で再計測する」を実施するためのスクリプト。
# `scripts/metrics/measure-idle-memory.sh`（Linux・Xvfb 版）の macOS 対応物であり、
# macOS は実ディスプレイが常時利用可能なため Xvfb 相当の処理は不要（そのまま起動する）。
#
# 呼び出し元: 人間担当（Mac + Xcode 環境）。CI 組み込みは行わない
# （measure-idle-memory.sh と同じ運用方針。ci.md の CI 制約とも独立）。
#
# 使い方:
#   bash scripts/apple/measure-idle-memory-macos.sh <実行体パス> [試行回数 (既定 5)] [待機秒 (既定 3)]
#
# 出力: 標準出力に CSV（列: run,rss_kb_at_wait,rss_kb_at_10s）を書き、
# 最後に中央値・平均値を標準エラーへ出力する（measure-idle-memory.sh と同じ形式）。
set -euo pipefail

# macOS 専用ガード: Linux では `ps -o rss=` の意味・NSWindow ベースラインの前提
# （AppKit）自体が成立しないため、案内を出して明示的に失敗させる（fail-fast。
# 黙って Linux 上で無意味な値を出さないための安全策）。
if [ "$(uname -s)" != "Darwin" ]; then
  echo "エラー: 本スクリプトは macOS 専用です（uname -s = $(uname -s)）。" >&2
  echo "Linux 環境でのアイドル時メモリ計測は scripts/metrics/measure-idle-memory.sh を使ってください。" >&2
  exit 1
fi

if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
  echo "使い方: bash scripts/apple/measure-idle-memory-macos.sh <実行体パス> [試行回数 (既定 5)] [待機秒 (既定 3)]" >&2
  exit 1
fi

TARGET_BIN="$1"
TRIALS="${2:-5}"
WAIT_SECONDS="${3:-3}"

# A03（インジェクション）対策: 回数・待機秒は数値のみ許容する（measure-idle-memory.sh と同じ検証）。
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
TARGET_BIN="$(cd -- "$(dirname -- "${TARGET_BIN}")" >/dev/null 2>&1 && pwd -P)/$(basename -- "${TARGET_BIN}")"

command -v ps >/dev/null 2>&1 || { echo "エラー: ps が見つかりません" >&2; exit 1; }

RSS_AT_WAIT=()
RSS_AT_10S=()

# measure-idle-memory.sh の sample_rss と同じ方針: ゾンビプロセスの RSS を
# 有効なサンプルとして誤記録しない（Linux 版で確認済みの Bugbot 指摘への対応を踏襲）。
sample_rss() {
  local pid="$1"
  local state
  local rss=""
  if [[ "${pid}" =~ ^[0-9]+$ ]] && kill -0 "${pid}" 2>/dev/null; then
    state="$(ps -o state= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
    if [[ "${state}" != Z* ]]; then
      rss="$(ps -o rss= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
    fi
  fi
  echo "${rss}"
}

app_pid=""
cleanup() {
  if [ -n "${app_pid}" ]; then
    kill "${app_pid}" 2>/dev/null || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "run,rss_kb_at_${WAIT_SECONDS}s,rss_kb_at_10s"

trial=1
while [ "${trial}" -le "${TRIALS}" ]; do
  "${TARGET_BIN}" >/dev/null 2>&1 &
  app_pid=$!

  sleep "${WAIT_SECONDS}"

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
  app_pid=""

  echo "${trial},${rss_at_wait},${rss_at_10s}"
  if [ "${rss_at_wait}" != "NA" ]; then RSS_AT_WAIT+=("${rss_at_wait}"); fi
  if [ "${rss_at_10s}" != "NA" ]; then RSS_AT_10S+=("${rss_at_10s}"); fi

  trial=$((trial + 1))
done

summarize() {
  # bash 3.2（macOS 標準）には nameref（local -n）が無いため、配列は
  # スペース区切り文字列として値渡しし、関数内で再展開する。
  local values=($1)
  local label="$2"
  local n=${#values[@]}
  if [ "${n}" -eq 0 ]; then
    echo "サマリ（${label}）: 有効なサンプルがありません" >&2
    return
  fi
  local sorted
  # shellcheck disable=SC2207
  sorted=($(printf '%s\n' "${values[@]}" | sort -n))
  local sum=0
  for v in "${sorted[@]}"; do sum=$((sum + v)); done
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

# bash 3.2 では nounset（set -u）下で空配列の `${arr[*]}` が unbound variable となり、
# 全試行が NA だった場合に summarize 内の「有効なサンプルがありません」分岐へ到達できない。
# 既定値付き展開（`${arr[*]-}`）で空文字を渡し、その分岐を機能させる。
summarize "${RSS_AT_WAIT[*]-}" "${WAIT_SECONDS}秒時点"
summarize "${RSS_AT_10S[*]-}" "10秒時点"
