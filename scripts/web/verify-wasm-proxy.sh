#!/usr/bin/env bash
# TASK-1.2（REQ-1 / M2）: build-wasm-proxy.sh が組み立てた web/dist を 127.0.0.1 限定の
# ローカル HTTP サーバーで配信し、chromium headless（swiftshader）でスクリーンショットを
# 取得する（PoC-1: docs/spec/03-poc/ui-rendering/ の build-web.sh の手法を踏襲）。
#
# セキュリティ（security.md A05 準拠）:
#   - HTTP サーバーは 127.0.0.1 バインド・検証時のみ起動し、常駐させない（trap で必ず停止）
#   - chromium にサンドボックス無効化フラグ（--no-sandbox 等）は使用しない
#
# 使い方: scripts/web/verify-wasm-proxy.sh <出力PNGパス>
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "使い方: $0 <出力PNGパス>" >&2
  exit 1
fi
out_png="$1"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
dist_dir="${repo_root}/crates/fandhe-egui-proxy/web/dist"
port=8781
host="127.0.0.1"

if [[ ! -d "${dist_dir}" ]]; then
  echo "[verify-wasm-proxy] エラー: ${dist_dir} が無い。先に build-wasm-proxy.sh を実行すること。" >&2
  exit 1
fi

server_pid=""
cleanup() {
  if [[ -n "${server_pid}" ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "[verify-wasm-proxy] python3 -m http.server ${port} --bind ${host} (${dist_dir})"
(cd "${dist_dir}" && python3 -m http.server "${port}" --bind "${host}" >/tmp/fandhe-proxy-http.log 2>&1) &
server_pid=$!

# サーバー起動待ち（固定 sleep ではなくポート応答をポーリング）。
# タイムアウトしてもそのまま chromium を起動すると、接続失敗のまま
# headless chromium が exit 0 を返すことがあり「見かけ上の成功」が生じる
# （Bugbot 指摘: サーバー起動未確認のまま検証完了扱いになる不整合の是正）。
server_ready=0
for _ in $(seq 1 20); do
  if curl -sf "http://${host}:${port}/index.html" >/dev/null 2>&1; then
    server_ready=1
    break
  fi
  sleep 0.5
done

if [[ "${server_ready}" -ne 1 ]]; then
  echo "[verify-wasm-proxy] エラー: http://${host}:${port}/index.html の起動確認がタイムアウトした。/tmp/fandhe-proxy-http.log を確認すること。" >&2
  exit 1
fi

echo "[verify-wasm-proxy] chromium headless でスクリーンショット取得: ${out_png}"
chromium \
  --headless=new \
  --use-gl=angle \
  --use-angle=swiftshader \
  --window-size=480,360 \
  --virtual-time-budget=5000 \
  --screenshot="${out_png}" \
  "http://${host}:${port}/index.html"

# chromium は screenshot 書き込みに失敗しても（例: 出力先ディレクトリへの権限問題）
# 終了コード 0 を返すことがあり、その場合でも `set -euo pipefail` では検出できず
# 「見かけ上の成功」のまま完了扱いになってしまう（後段の canvas サイズ検査は
# PNG の存在を前提にしていないため防げない）。Issue #76 レビュー対応で発見。
if [[ ! -f "${out_png}" ]]; then
  echo "[verify-wasm-proxy] エラー: スクリーンショットが生成されなかった: ${out_png}" >&2
  exit 1
fi

echo "[verify-wasm-proxy] 完了: ${out_png}"

# TASK-4.1（REQ-4 / M3、条件 4）回帰ガード:
# canvas に CSS サイズ明示（web/index.html）+ ensure_canvas_css_size ガード
# （src/web.rs）が効いていれば、描画後も canvas.width/height 属性は
# devicePixelRatio 倍程度に収まる。Safari のリサイズ無限ループ（PoC-5 発見事項 1、
# emilk/egui #4699 と同種）が再発すると canvas.width/height が 2^26 まで異常膨張
# するため、閾値（devicePixelRatio を大きく見積もっても届かない余裕を持たせた
# 8192px）を超えていれば「見かけ上の成功（スクリーンショット取得は成功）」の
# まま症状を見逃さないよう非 0 終了する。
canvas_size_threshold=8192
dom_dump="$(chromium \
  --headless=new \
  --use-gl=angle \
  --use-angle=swiftshader \
  --window-size=480,360 \
  --virtual-time-budget=5000 \
  --dump-dom \
  "http://${host}:${port}/index.html")"

canvas_attrs="$(printf '%s\n' "${dom_dump}" | grep -o '<canvas[^>]*>' | head -n1 || true)"
if [[ -z "${canvas_attrs}" ]]; then
  echo "[verify-wasm-proxy] エラー: --dump-dom の出力から <canvas> 要素を検出できなかった。" >&2
  exit 1
fi

canvas_width="$(printf '%s' "${canvas_attrs}" | grep -o 'width="[0-9]*"' | grep -o '[0-9]*' || true)"
canvas_height="$(printf '%s' "${canvas_attrs}" | grep -o 'height="[0-9]*"' | grep -o '[0-9]*' || true)"

if [[ -z "${canvas_width}" || -z "${canvas_height}" ]]; then
  echo "[verify-wasm-proxy] エラー: canvas の width/height 属性を取得できなかった: ${canvas_attrs}" >&2
  exit 1
fi

echo "[verify-wasm-proxy] canvas 属性確認: width=${canvas_width} height=${canvas_height}（閾値 ${canvas_size_threshold}px）"

if (( canvas_width > canvas_size_threshold || canvas_height > canvas_size_threshold )); then
  echo "[verify-wasm-proxy] エラー: canvas サイズが閾値を超過（Safari 型リサイズ無限ループの症状の疑い、TASK-4.1 / docs/architecture/web-canvas-css-sizing.md 参照）: width=${canvas_width} height=${canvas_height}" >&2
  exit 1
fi

echo "[verify-wasm-proxy] canvas サイズ回帰チェック: 合格"
