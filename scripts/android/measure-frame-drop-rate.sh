#!/usr/bin/env bash
# TASK-5.3（#647 S3 / Issue #658）: presentation（実表示）間隔からフレーム落ち率を集計する。
#
# 位置づけ: `measure-frame-rate.sh`（TASK-5.3 / #238）が計測しているのは egui の更新間隔で
# あり、実際に画面へ提示された presentation 間隔ではない
# （判定保留と記録済み。検証記録は非公開）。
# 本スクリプトはその欠落を埋め、`dumpsys SurfaceFlinger --latency <layer>` が返す
# 実 presentation タイムスタンプから「2 vsync 周期超え」（spec#1 案 A）のフレーム落ち率を
# 集計する。
#
# framestats（`dumpsys gfxinfo`）ではなく SurfaceFlinger --latency を採用した理由:
# framestats は HWUI（View 階層描画）のフレームしか記録しない。fandhe（GameActivity +
# wgpu の native surface）・Flutter（FlutterSurfaceView）は HWUI を経由しないため、
# fandhe / Compose / RN / Flutter の 4 者（#647）を同一方法で比較できない。
# --latency はレイヤ単位で desiredPresentTime/actualPresentTime/frameReadyTime（ns）を
# 返し、SurfaceView 系・HWUI 系のいずれのレイヤにも適用できる。
#
# 実機（Pixel 10a、Android 16）での出力形式差は本 Issue の範囲では確認できていない。
# 差が出た場合の差し替えポイントは aggregate_latency() 1 関数に閉じ込めてある。
# 出力形式が --latency と非互換なほど崩れていた場合の代替候補（未実装）:
# `dumpsys gfxinfo <package> framestats`（HWUI 系アプリに限定される）。
#
# 使い方:
#   scripts/android/measure-frame-drop-rate.sh <package> [<package> ...]   # 実機モード
#   scripts/android/measure-frame-drop-rate.sh --parse <latency-file>      # オフライン集計
#
# 環境変数（`measure-frame-rate.sh` / `verify-android-launch.sh` と同じ設計方針。
# FANDHE_ANDROID_SERIAL の値はログへ出力しない）:
#   FANDHE_ANDROID_SERIAL              既定: 空
#   FANDHE_ANDROID_LAUNCH_WAIT_SECONDS 既定: 5（起動・初期化の猶予秒数）
#   FANDHE_ANDROID_SAMPLE_SECONDS      既定: 10（--latency はリングバッファで直近 128
#                                       フレーム分のみ保持するため、60Hz 環境では
#                                       約 2 秒分が上限。窓を長く取っても 128 行が上限）
#   FANDHE_FRAME_DROP_PERIODS          既定: 2（フレーム落ちと数える vsync 周期数）
#   FANDHE_ANDROID_KEEP_APP            非空なら計測後に force-stop しない
#
# 実機実行・4 者比較判定は本 Issue の範囲外（S4 #659 / W2 #660 へ申し送り）。
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
# shellcheck source=lib/sf-layer.sh
# shellcheck disable=SC1091
. "${script_dir}/lib/sf-layer.sh"

log() {
  echo "[measure-frame-drop-rate] $*"
}

fail() {
  echo "[measure-frame-drop-rate] ERROR: $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
使い方:
  measure-frame-drop-rate.sh <package> [<package> ...]
  measure-frame-drop-rate.sh --parse <latency-file> [--package <pkg>] [--layer <layer>]
EOF
}

drop_periods="${FANDHE_FRAME_DROP_PERIODS:-2}"
[[ "${drop_periods}" =~ ^[1-9][0-9]*$ ]] || fail "FANDHE_FRAME_DROP_PERIODS が正の整数ではありません: ${drop_periods}"

# --- 集計本体（awk 1 本。python3/jq 非依存） -----
# 入力: `dumpsys SurfaceFlinger --latency <layer>` の生出力（1 行目がリフレッシュ周期 ns、
# 2 行目以降が「desiredPresentTime actualPresentTime frameReadyTime」の TAB/空白区切り）。
# 出力: 1 アプリ分の JSON オブジェクト断片（末尾改行なし）。
#
# 集計定義（#647 S3 の計画・spec#1 案 A）:
#   - 有効行: actualPresentTime が 0 でも INT64_MAX（未確定値、2^63-1）でもない行
#   - 連続する有効行の actualPresentTime 差分を presentation 間隔とする
#   - periods = round(interval_ns / period_ns) をヒストグラムに積む
#   - dropped: interval_ns > threshold_periods * period_ns * 1.05（5% は vsync ジッタ吸収）
#
# 入力検証（P1 レビュー対応、Issue #658）: 1 行目が正整数、データ行が 3 列の整数で
# なければ不正な dumpsys 出力（エラー文・形式変更・切り詰め）とみなし非 0 で終了する。
# 集計可能な interval が 1 件も無い場合も同様に非 0 で終了する（成立していない計測結果を
# 正常終了として保存させない）。
#
# package/layer は `awk -v` ではなく環境変数経由（ENVIRON）で渡す。`awk -v var=value` は
# POSIX で value 中のバックスラッシュエスケープ列（`\t` `\n` 等）を解釈してから代入するため、
# 未信頼な --package/--layer に生のバックスラッシュを含めると json_escape() へ渡る前に
# 値が変質する（P1 レビュー指摘）。ENVIRON はエスケープ処理を行わないため安全に生値が渡る。
aggregate_latency() {
  local file="$1" package="$2" layer="$3" threshold="$4"
  FANDHE_FD_PKG="${package}" FANDHE_FD_LAYER="${layer}" \
    awk -v threshold="${threshold}" '
    BEGIN {
      pkg = ENVIRON["FANDHE_FD_PKG"]
      layer = ENVIRON["FANDHE_FD_LAYER"]
      # 制御文字（U+0000-001F）を \uXXXX へ変換するための char -> code 変換表。
      for (i = 0; i < 32; i++) ctrl_ord[sprintf("%c", i)] = i
    }
    # --package/--layer は任意入力を受ける汎用 CLI のため、`"` `\` に加えて
    # 改行・タブ等の制御文字も JSON として妥当な形へエスケープする。
    function json_escape(s,    out, i, n, c, code) {
      out = ""
      n = length(s)
      for (i = 1; i <= n; i++) {
        c = substr(s, i, 1)
        if (c == "\\") out = out "\\\\"
        else if (c == "\"") out = out "\\\""
        else if (c == "\n") out = out "\\n"
        else if (c == "\r") out = out "\\r"
        else if (c == "\t") out = out "\\t"
        else if ((c in ctrl_ord)) out = out sprintf("\\u%04x", ctrl_ord[c])
        else out = out c
      }
      return out
    }
    NR == 1 {
      if (NF != 1 || $1 !~ /^[0-9]+$/ || ($1 + 0) <= 0) {
        printf "不正な入力です（1 行目はリフレッシュ周期の正整数である必要があります）: %s\n", $0 > "/dev/stderr"
        invalid = 1
        exit 1
      }
      period_ns = $1 + 0
      next
    }
    {
      if (NF != 3 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $3 !~ /^[0-9]+$/) {
        printf "不正な入力です（%d 行目は 3 列の整数である必要があります）: %s\n", NR, $0 > "/dev/stderr"
        invalid = 1
        exit 1
      }
      actual = $2 + 0
      # INT64_MAX（未確定値）と 0（未提示）を無効行として除外する。
      if (actual == 0 || actual >= 9223372036854775807) next
      frames++
      if (have_prev) {
        interval = actual - prev
        if (interval > 0 && period_ns > 0) {
          intervals++
          periods = interval / period_ns
          bucket = int(periods + 0.5)
          if (bucket < 1) bucket = 1
          hist[bucket]++
          if (interval > threshold * period_ns * 1.05) dropped++
        }
      }
      prev = actual
      have_prev = 1
    }
    END {
      if (invalid) exit 1
      if (intervals == 0) {
        print "集計可能な presentation 間隔がありません（有効なデータ行が不足しています）" > "/dev/stderr"
        exit 1
      }
      drop_rate = sprintf("%.4f", dropped / intervals)
      hist_json = ""
      for (b in hist) {
        if (hist_json != "") hist_json = hist_json ","
        hist_json = hist_json "\"" b "\":" hist[b]
      }
      printf "{\"package\":\"%s\",\"layer\":\"%s\",\"refresh_period_ns\":%d,\"frames\":%d,\"intervals\":%d,\"dropped\":%d,\"drop_rate\":%s,\"histogram_periods\":{%s}}", \
        json_escape(pkg), json_escape(layer), period_ns, frames, intervals+0, dropped+0, drop_rate, hist_json
    }
  ' "${file}"
}

# --- --parse モード（オフライン集計。実機非依存のテスト・再利用用） -------------
if [ "${1:-}" = "--parse" ]; then
  shift
  latency_file="${1:-}"
  [ -n "${latency_file}" ] || { usage; fail "--parse には集計対象ファイルが必要です"; }
  shift || true
  parse_package=""
  parse_layer=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --package) parse_package="${2:-}"; shift 2 ;;
      --layer) parse_layer="${2:-}"; shift 2 ;;
      *) fail "不明な引数: $1" ;;
    esac
  done
  [ -f "${latency_file}" ] || fail "ファイルが見つかりません: ${latency_file}"
  fragment="$(aggregate_latency "${latency_file}" "${parse_package}" "${parse_layer}" "${drop_periods}")"
  printf '{"generated_at":"%s","threshold_periods":%d,"apps":[%s]}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${drop_periods}" "${fragment}"
  exit 0
fi

# --- 実機モード -----------------------------------------------------------------
[ $# -ge 1 ] || { usage; fail "パッケージ名を 1 つ以上指定してください"; }

# A03（インジェクション）対策: パッケージ名は adb コマンドへ渡す前に形式検証する。
package_pattern='^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$'
for pkg in "$@"; do
  [[ "${pkg}" =~ ${package_pattern} ]] || fail "不正なパッケージ名です: ${pkg}"
done

serial="${FANDHE_ANDROID_SERIAL:-}"
launch_wait_seconds="${FANDHE_ANDROID_LAUNCH_WAIT_SECONDS:-5}"
sample_seconds="${FANDHE_ANDROID_SAMPLE_SECONDS:-10}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"

adb_cmd() {
  if [ -n "${serial}" ]; then
    adb -s "${serial}" "$@"
  else
    adb "$@"
  fi
}

command -v adb >/dev/null 2>&1 || fail "adb が見つかりません"

device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"
[ -n "${device_lines}" ] || fail "接続されている Android 端末がありません"
device_count="$(printf '%s\n' "${device_lines}" | grep -c "device$" || true)"
if [ -z "${serial}" ] && [ "${device_count}" -gt 1 ]; then
  fail "複数台の端末が接続されています。FANDHE_ANDROID_SERIAL で対象端末を指定してください"
fi
[ "${device_count}" -ge 1 ] || fail "device 状態の端末が見つかりません（unauthorized/offline のみ検出）"

# 起動したアプリを異常終了時も確実に force-stop する（A05: 検証専用の状態を端末に残置しない）。
launched_packages=()
cleanup_on_exit() {
  [ -n "${keep_app}" ] && return 0
  for p in "${launched_packages[@]:-}"; do
    [ -n "${p}" ] && adb_cmd shell am force-stop "${p}" >/dev/null 2>&1 || true
  done
}
trap cleanup_on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# レイヤ選択は lib/sf-layer.sh の select_surfaceflinger_layer に委譲する（P1 レビュー対応:
# measure-frame-rate.sh フェーズ 7 と選択規則がずれないようにするため、Issue #658）。
find_layer() {
  local pkg="$1" list layer status
  list="$(adb_cmd shell dumpsys SurfaceFlinger --list 2>/dev/null | tr -d '\r')"
  # `if layer=$(...); then ... fi` の直後で `$?` を参照すると、条件が偽の場合
  # （then 節が 1 度も実行されない場合）POSIX 上 if 文自体の終了ステータスは 0 に
  # 確定してしまい、select_surfaceflinger_layer の実際の失敗理由（戻り値 2 =
  # 不正文字）を握りつぶして常に「レイヤ名を特定できません」になる（Bugbot 指摘、
  # Issue #658）。else 節の中で直後に参照することで、代入式自身の終了ステータスを
  # 取りこぼさず保持する。
  if layer="$(select_surfaceflinger_layer "${list}" "${pkg}")"; then
    printf '%s' "${layer}"
    return 0
  else
    status=$?
  fi
  if [ "${status}" -eq 2 ]; then
    fail "レイヤ名に想定外の文字が含まれます（パッケージ: ${pkg}）"
  fi
  if [ "${status}" -eq 3 ]; then
    fail "パッケージ: ${pkg} に一致するレイヤが複数あり一意に選べません。他アプリを終了するかフォアグラウンドを 1 つに絞ってください"
  fi
  fail "レイヤ名を特定できません（パッケージ: ${pkg}）。アプリが前面表示されているか確認してください"
}

tmp_dir="$(mktemp -d)"
# EXIT トラップは 1 つしか有効にならないため、force-stop と一時ディレクトリ削除を
# 1 つのハンドラへ束ねる（先に登録した cleanup_on_exit 単体のトラップを上書きする）。
cleanup_on_exit_all() {
  cleanup_on_exit
  rm -rf "${tmp_dir}"
}
trap cleanup_on_exit_all EXIT

fragments=()
for pkg in "$@"; do
  adb_cmd shell pm path "${pkg}" >/dev/null 2>&1 \
    || fail "未インストールです（先に対象アプリをインストールしてください）: ${pkg}"

  log "起動: monkey -p ${pkg} -c android.intent.category.LAUNCHER 1"
  adb_cmd shell monkey -p "${pkg}" -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1 \
    || fail "アプリの起動に失敗しました（未インストールの可能性）: ${pkg}"
  launched_packages+=("${pkg}")
  sleep "${launch_wait_seconds}"

  layer="$(find_layer "${pkg}")"
  log "レイヤ検出: ${layer}"

  # クリア失敗を無視すると、直前のリングバッファの古いサンプルが今回の計測に
  # 混入し得るため、失敗時は中止する（P1 レビュー対応、Issue #658）。
  adb_cmd shell dumpsys SurfaceFlinger --latency-clear "${layer}" >/dev/null 2>&1 \
    || fail "latency バッファのクリアに失敗しました（レイヤ: ${layer}）。未対応端末・レイヤ指定不整合の可能性があります"
  sleep "${sample_seconds}"

  latency_file="${tmp_dir}/${pkg}.latency"
  adb_cmd shell dumpsys SurfaceFlinger --latency "${layer}" 2>/dev/null | tr -d '\r' > "${latency_file}"
  [ -s "${latency_file}" ] || fail "--latency の取得結果が空です（レイヤ: ${layer}）"

  fragments+=("$(aggregate_latency "${latency_file}" "${pkg}" "${layer}" "${drop_periods}")")

  if [ -z "${keep_app}" ]; then
    adb_cmd shell am force-stop "${pkg}" >/dev/null 2>&1 || true
  fi
done

apps_json="$(IFS=,; echo "${fragments[*]}")"
printf '{"generated_at":"%s","threshold_periods":%d,"apps":[%s]}\n' \
  "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${drop_periods}" "${apps_json}"
