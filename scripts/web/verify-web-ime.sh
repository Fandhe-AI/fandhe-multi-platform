#!/usr/bin/env bash
# TASK-8.5-S22（REQ-8 / Issue #203、親 #181）: build-wasm-dom.sh が組み立てた
# web/dist を 127.0.0.1 限定のローカル HTTP サーバーで配信し、chromium headless を
# --remote-debugging-port 付きで起動して、Chrome DevTools Protocol（Runtime.evaluate
# 経由で合成 CompositionEvent を dispatch）で `crates/fandhe-web-dom/src/mount.rs`
# の `attach_input` composition 配線（compositionstart/update/end →
# `ime_event_from_composition` → 二重表示回避の no-op / commit 経路）が実 DOM 上で
# end-to-end に動作することを確認する。
#
# scripts/web/verify-dom-a11y.sh（TASK-4.4-S4）の安全設計をそのまま踏襲する:
#   - HTTP / CDP ポートは 127.0.0.1 バインド・検証時のみ起動・trap で確実に停止
#   - ポートは固定値ではなく実行のたびに動的取得し、並列 worktree 実行下の
#     ポート衝突・false pass を避ける（ss による listener PID 検証で TOCTOU を検出）
#   - chromium にサンドボックス無効化フラグ（--no-sandbox 等）は使用しない
#   - 一時プロファイルは snap chromium の書き込み許可領域（~/snap/chromium/common/）
#     配下に mktemp で一意生成する
#
# CDP の IME シミュレーション（`Input.imeSetComposition`）は headless 環境・
# 単純な `<input>` 要素に対して確実に compositionevent 列を発火させる保証がない
# ため、本スクリプトは `Runtime.evaluate` によるページ内スクリプト実行で合成
# `CompositionEvent`/`InputEvent` を `dispatchEvent` する方式（フォールバック方式。
# 限界は検証記録に明記しているが非公開）を採用する。
# `Runtime.evaluate` はページの内容を読み取らず、こちらが用意した固定スクリプトの
# 実行のみに使う（検証対象ページの内容を評価・実行しない、`dump-ax-tree.mjs` と
# 同じ A03 対策の考え方）。
#
# 使い方: scripts/web/verify-web-ime.sh <出力先ディレクトリ>
#   出力先ディレクトリに result.json（各シナリオの期待値照合結果）を生成する。
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "使い方: $0 <出力先ディレクトリ>" >&2
  exit 1
fi
out_dir="$1"
mkdir -p "${out_dir}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
dist_dir="${repo_root}/crates/fandhe-web-dom/web/dist"
host="127.0.0.1"

find_free_port() {
  python3 - <<'PY'
import socket

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}
http_port="$(find_free_port)"
cdp_port="$(find_free_port)"
while [[ "${cdp_port}" == "${http_port}" ]]; do
  cdp_port="$(find_free_port)"
done

listener_pid_for_port() {
  local port="$1"
  ss -Htlnp "sport = :${port}" 2>/dev/null \
    | grep -oP 'pid=\K[0-9]+' \
    | head -n1
}

wait_for_listener() {
  local port="$1" path="$2" expected_pid="$3" max_attempts="$4"
  local attempt owner_pid
  for ((attempt = 0; attempt < max_attempts; attempt += 1)); do
    if ! kill -0 "${expected_pid}" 2>/dev/null; then
      echo "[verify-web-ime] エラー: PID ${expected_pid} が起動直後に終了した（ポート ${port} の bind 失敗の可能性）。" >&2
      return 1
    fi
    if curl -sf "http://${host}:${port}${path}" >/dev/null 2>&1; then
      owner_pid="$(listener_pid_for_port "${port}")"
      if [[ "${owner_pid}" == "${expected_pid}" ]]; then
        return 0
      fi
      echo "[verify-web-ime] エラー: ポート ${port} の listen ソケットの所有者が想定外（期待 PID ${expected_pid}、実際 ${owner_pid:-不明}）。他プロセスにポートを奪われた可能性がある。" >&2
      return 1
    fi
    sleep 0.5
  done
  echo "[verify-web-ime] エラー: http://${host}:${port}${path} の起動確認がタイムアウトした。" >&2
  return 1
}

if [[ ! -d "${dist_dir}" ]]; then
  echo "[verify-web-ime] エラー: ${dist_dir} が無い。先に build-wasm-dom.sh を実行すること。" >&2
  exit 1
fi

if ! command -v ss >/dev/null 2>&1; then
  echo "[verify-web-ime] エラー: ss (iproute2) が無い。listen ソケットの所有プロセス検証に必要。" >&2
  exit 1
fi

chromium_common_dir="${HOME}/snap/chromium/common"
if [[ ! -d "${chromium_common_dir}" ]]; then
  echo "[verify-web-ime] エラー: ${chromium_common_dir} が無い（snap 版 chromium 前提のパス）。" >&2
  exit 1
fi
profile_dir="$(mktemp -d "${chromium_common_dir}/fandhe-ime-XXXXXX")"

http_log="$(mktemp "${TMPDIR:-/tmp}/fandhe-web-ime-http-XXXXXX.log")"
chromium_log="$(mktemp "${TMPDIR:-/tmp}/fandhe-web-ime-chromium-XXXXXX.log")"

http_pid=""
chromium_pid=""
cleanup() {
  if [[ -n "${chromium_pid}" ]]; then
    kill "${chromium_pid}" 2>/dev/null || true
    wait "${chromium_pid}" 2>/dev/null || true
  fi
  if [[ -n "${http_pid}" ]]; then
    kill "${http_pid}" 2>/dev/null || true
    wait "${http_pid}" 2>/dev/null || true
  fi
  rm -rf "${profile_dir}"
}
trap cleanup EXIT

echo "[verify-web-ime] python3 -m http.server ${http_port} --bind ${host} (${dist_dir})"
python3 -m http.server "${http_port}" --bind "${host}" --directory "${dist_dir}" >"${http_log}" 2>&1 &
http_pid=$!

if ! wait_for_listener "${http_port}" "/index.html" "${http_pid}" 20; then
  echo "[verify-web-ime] ${http_log} を確認すること。" >&2
  exit 1
fi

echo "[verify-web-ime] chromium headless（CDP port ${cdp_port}）を起動"
chromium \
  --headless=new \
  --remote-debugging-port="${cdp_port}" \
  --remote-debugging-address="${host}" \
  --user-data-dir="${profile_dir}" \
  --window-size=480,640 \
  "http://${host}:${http_port}/index.html" >"${chromium_log}" 2>&1 &
chromium_pid=$!

if ! wait_for_listener "${cdp_port}" "/json/version" "${chromium_pid}" 20; then
  echo "[verify-web-ime] ${chromium_log} を確認すること。" >&2
  exit 1
fi

result_json="${out_dir}/result.json"
echo "[verify-web-ime] composition イベントシナリオを実行: ${result_json}"
if ! node "${script_dir}/verify-web-ime.mjs" "${host}:${cdp_port}" "http://${host}:${http_port}/index.html" "${result_json}"; then
  echo "[verify-web-ime] エラー: シナリオの期待値照合に失敗した（${result_json} を確認すること）。" >&2
  exit 1
fi

echo "[verify-web-ime] 完了: ${result_json}"
