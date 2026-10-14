#!/usr/bin/env bash
# TASK-4.4-S4（REQ-4 / Issue #82）: build-wasm-dom.sh が組み立てた web/dist を
# 127.0.0.1 限定のローカル HTTP サーバーで配信し、chromium headless を
# --remote-debugging-port 付きで起動して、Chrome DevTools Protocol
# （Accessibility.getFullAXTree、dump-ax-tree.mjs 経由）でブラウザ標準の
# Accessibility Tree に見出し・textbox・button・list・listitem の意味情報が
# 実際に公開されることを確認する。PoC-5（独自レンダラ、意味情報 0 件）との
# 対比が本 issue の検証の骨子（docs/architecture/dom-output-architecture.md §2.2）。
#
# セキュリティ（security.md A05 準拠）:
#   - HTTP サーバー・CDP ポートはいずれも 127.0.0.1 バインド・検証時のみ起動し、
#     trap で確実に停止する。http.server は --directory 起動でサブシェルを
#     経由しないため "$!" が実プロセスの PID と一致し kill で確実に止まる
#     （旧: cd ... && python3 ... のサブシェル経由起動だと "$!" がサブシェルの
#     PID になり、実体の http.server が孤児化する不具合があった。実プロセスで
#     再現・修正後の無孤児化を確認済み）。chromium は /snap/bin/chromium の
#     ラッパーが exec するため "$!" は実ブラウザのメインプロセス PID と一致し、
#     kill で renderer 等の子プロセスも含めて確実に終了することを実プロセスで
#     確認済み
#   - HTTP / CDP のポートは固定値ではなく OS が割り当てる空きポートを
#     実行のたびに動的取得し、並列 worktree 実行下でのポート衝突・
#     false pass を避ける
#   - chromium にサンドボックス無効化フラグ（--no-sandbox 等）は使用しない
#   - 一時プロファイルは snap chromium の書き込み許可領域
#     （~/snap/chromium/common/、AppArmor 制約）配下に作成し、既定プロファイルを
#     汚さない。ディレクトリ名はスクリプト実行のたびに mktemp で一意に生成し、
#     並列実行中の他エージェントの chromium プロセスと衝突しない
#
# 使い方: scripts/web/verify-dom-a11y.sh <出力先ディレクトリ>
#   出力先ディレクトリに ax-tree.json（getFullAXTree 抽出結果）と
#   screenshot.png（目視確認用）を生成する。
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

# ポートは固定値だと並列 worktree 実行下で衝突しうる（run B の bind 失敗 →
# readiness probe が run A のサーバーへ誤って成功し、run A のビルドに対する
# AX ツリーを記録して false pass になる）。プロセスごとに空きポートを
# 動的に払い出して衝突を避ける。
#
# find_free_port() は空きポート番号を得るために一度 bind してすぐ close する
# ため、close してから実際のサーバー（http.server / chromium）が bind する
# までの間に別プロセスがそのポートを奪う TOCTOU（time-of-check to
# time-of-use）の隙が残る（レビュー指摘）。この隙自体はポート払い出し方式を
# 変えない限り原理的に消せないため、wait_for_listener() 側で「curl が通った
# こと」だけでなく「そのポートの listen ソケットを実際に保持しているのが
# 自分が起動したプロセスの PID であること」まで ss で検証し、他プロセスが
# 同じポートを奪って先に listen してしまった場合は false pass にせず
# エラーとして検出する。
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

# 指定ポートの listen ソケットを実際に保持しているプロセスの PID を返す
# （ss -H の users:(("proc",pid=N,fd=M)) 形式から pid= の値を抜き出す）。
# 見つからなければ何も出力しない。
listener_pid_for_port() {
  local port="$1"
  ss -Htlnp "sport = :${port}" 2>/dev/null \
    | grep -oP 'pid=\K[0-9]+' \
    | head -n1
}

# curl でエンドポイントが応答するだけでなく、応答しているソケットの所有者が
# expected_pid（自分が起動した http_pid / chromium_pid）と一致することまで
# 確認してから ready 扱いにする。expected_pid が既に終了していれば
# bind 失敗（別プロセスにポートを奪われた等）とみなし即座に失敗させる。
wait_for_listener() {
  local port="$1" path="$2" expected_pid="$3" max_attempts="$4"
  local attempt owner_pid
  for ((attempt = 0; attempt < max_attempts; attempt += 1)); do
    if ! kill -0 "${expected_pid}" 2>/dev/null; then
      echo "[verify-dom-a11y] エラー: PID ${expected_pid} が起動直後に終了した（ポート ${port} の bind 失敗の可能性）。" >&2
      return 1
    fi
    if curl -sf "http://${host}:${port}${path}" >/dev/null 2>&1; then
      owner_pid="$(listener_pid_for_port "${port}")"
      if [[ "${owner_pid}" == "${expected_pid}" ]]; then
        return 0
      fi
      echo "[verify-dom-a11y] エラー: ポート ${port} の listen ソケットの所有者が想定外（期待 PID ${expected_pid}、実際 ${owner_pid:-不明}）。他プロセスにポートを奪われた可能性がある。" >&2
      return 1
    fi
    sleep 0.5
  done
  echo "[verify-dom-a11y] エラー: http://${host}:${port}${path} の起動確認がタイムアウトした。" >&2
  return 1
}

if [[ ! -d "${dist_dir}" ]]; then
  echo "[verify-dom-a11y] エラー: ${dist_dir} が無い。先に build-wasm-dom.sh を実行すること。" >&2
  exit 1
fi

if ! command -v ss >/dev/null 2>&1; then
  echo "[verify-dom-a11y] エラー: ss (iproute2) が無い。listen ソケットの所有プロセス検証に必要。" >&2
  exit 1
fi

# snap chromium は AppArmor 閉じ込めにより $HOME 配下でも任意パスへの書き込みが
# 拒否される（SingletonLock 作成失敗）。書き込み許可領域（~/snap/chromium/common/）配下に
# 一意な一時プロファイルディレクトリを作る（並列実行中の他 issue の chromium と
# 衝突しないよう mktemp で一意化）。
chromium_common_dir="${HOME}/snap/chromium/common"
if [[ ! -d "${chromium_common_dir}" ]]; then
  echo "[verify-dom-a11y] エラー: ${chromium_common_dir} が無い（snap 版 chromium 前提のパス）。" >&2
  exit 1
fi
profile_dir="$(mktemp -d "${chromium_common_dir}/fandhe-ax-XXXXXX")"

http_log="$(mktemp "${TMPDIR:-/tmp}/fandhe-dom-http-XXXXXX.log")"
chromium_log="$(mktemp "${TMPDIR:-/tmp}/fandhe-dom-chromium-XXXXXX.log")"

http_pid=""
chromium_pid=""
# スクリーンショット取得（フォアグラウンド chromium）用の一時プロファイル。
# ここで空文字に初期化しておき、cleanup() が未生成時に誤って rm -rf "" を
# 実行しないようにする（mktemp -d 実行後に実パスが入る）。
screenshot_profile_dir=""
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
  # スクリーンショット用 chromium が非 0 終了・早期終了した場合でも一時
  # プロファイルが ~/snap/chromium/common/ 配下に残留しないよう、成功パス・
  # 異常終了パスの両方をこの EXIT トラップで確実にクリーンアップする。
  if [[ -n "${screenshot_profile_dir}" ]]; then
    rm -rf "${screenshot_profile_dir}"
  fi
}
trap cleanup EXIT

echo "[verify-dom-a11y] python3 -m http.server ${http_port} --bind ${host} (${dist_dir})"
# サブシェル（cd ... && python3 ...）経由で起動すると "$!" がサブシェルの
# PID になり、実体の http.server プロセスの PID を取得できない。cleanup() の
# kill がサブシェル（既に終了済み）にしか効かず、http.server が孤児化して
# ポートを掴んだまま生き残る不具合があったため、--directory で作業ディレクトリを
# 指定し、サブシェルを介さずに直接起動する（Python 3.7+ で利用可能）。
python3 -m http.server "${http_port}" --bind "${host}" --directory "${dist_dir}" >"${http_log}" 2>&1 &
http_pid=$!

if ! wait_for_listener "${http_port}" "/index.html" "${http_pid}" 20; then
  echo "[verify-dom-a11y] ${http_log} を確認すること。" >&2
  exit 1
fi

echo "[verify-dom-a11y] chromium headless（CDP port ${cdp_port}）を起動"
chromium \
  --headless=new \
  --remote-debugging-port="${cdp_port}" \
  --remote-debugging-address="${host}" \
  --user-data-dir="${profile_dir}" \
  --window-size=480,640 \
  "http://${host}:${http_port}/index.html" >"${chromium_log}" 2>&1 &
chromium_pid=$!

if ! wait_for_listener "${cdp_port}" "/json/version" "${chromium_pid}" 20; then
  echo "[verify-dom-a11y] ${chromium_log} を確認すること。" >&2
  exit 1
fi

ax_json="${out_dir}/ax-tree.json"
echo "[verify-dom-a11y] Accessibility.getFullAXTree を取得: ${ax_json}"
if ! node "${script_dir}/dump-ax-tree.mjs" "${host}:${cdp_port}" "http://${host}:${http_port}/index.html" "${ax_json}"; then
  echo "[verify-dom-a11y] エラー: 期待ロールの検査に失敗した（${ax_json} を確認すること）。" >&2
  exit 1
fi

screenshot_profile_dir="$(mktemp -d "${chromium_common_dir}/fandhe-ax-shot-XXXXXX")"
screenshot_png="${out_dir}/screenshot.png"
# 以降の chromium 起動が失敗・早期終了しても screenshot_profile_dir は
# cleanup() の EXIT トラップで削除される（ここでの明示的な rm -rf は行わない）。
echo "[verify-dom-a11y] スクリーンショット取得（別プロセス、swiftshader ソフトウェアレンダリング）: ${screenshot_png}"
chromium \
  --headless=new \
  --use-gl=angle \
  --use-angle=swiftshader \
  --user-data-dir="${screenshot_profile_dir}" \
  --window-size=480,640 \
  --virtual-time-budget=5000 \
  --screenshot="${screenshot_png}" \
  "http://${host}:${http_port}/index.html"

if [[ ! -f "${screenshot_png}" ]]; then
  echo "[verify-dom-a11y] エラー: スクリーンショットが生成されなかった。" >&2
  exit 1
fi

echo "[verify-dom-a11y] 完了: ${ax_json} / ${screenshot_png}"
