#!/usr/bin/env bash
# TASK-4.2（REQ-4 / M3、条件 4）: build-wasm-proxy.sh が組み立てた web/dist を 127.0.0.1 限定の
# ローカル HTTP サーバーで配信し、Firefox で実描画確認を行う。
# scripts/web/verify-wasm-proxy.sh（chromium 用）の作法（set -euo pipefail・127.0.0.1 バインド・
# trap による確実なサーバー停止・ポート応答ポーリング・サンドボックス無効化フラグ不使用）を踏襲する。
#
# Firefox 固有の事情（chromium との差異）:
#   - chromium の `--virtual-time-budget` に相当する「仮想時間を進めて確実に描画完了後に撮る」
#     オプションが Firefox の `--screenshot`（純粋 headless）には無い。`--screenshot` は
#     document の load イベント時点で撮影するため、WASM 初期化（fetch + instantiate、非同期）
#     完了前の白紙画像になることを実機確認で確認した（本スクリプト作成時の検証ログ参照）。
#   - そのため xvfb（仮想 X サーバー）上で通常起動の Firefox を `--kiosk`（ブラウザ chrome 非表示・
#     指定ウィンドウサイズそのまま）で開き、WASM 初期化完了を待つ固定ディレイの後に
#     画面全体（PIL ImageGrab、Xlib 経由）をキャプチャする方式へフォールバックする。
#   - 初回起動時の "Welcome to Firefox" オンボーディング画面が描画結果を覆ってしまうため、
#     一時プロファイルの user.js でオンボーディング・データ収集通知を無効化する
#     （新規インストールのプロファイルのみに適用され、リポジトリにも既存プロファイルにも影響しない）。
#
# セキュリティ（security.md A05 準拠）:
#   - HTTP サーバーは 127.0.0.1 バインド・検証時のみ起動し、常駐させない（trap で必ず停止）
#   - firefox にサンドボックス無効化フラグは使用しない
#   - Firefox のプロファイルは一時ディレクトリに分離し、ホームディレクトリの既定プロファイルを汚さない
#
# 使い方: FANDHE_FIREFOX_BIN=/path/to/firefox scripts/web/verify-wasm-proxy-firefox.sh <出力PNGパス>
#   環境変数 FANDHE_FIREFOX_BIN で Firefox バイナリパスを指定する（既定: PATH 上の firefox）。
#   本スクリプトは Firefox のインストール自体は行わない（呼び出し側で用意すること）。
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "使い方: $0 <出力PNGパス>" >&2
  exit 1
fi
out_png="$1"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
dist_dir="${repo_root}/crates/fandhe-egui-proxy/web/dist"
port=8782
host="127.0.0.1"
firefox_bin="${FANDHE_FIREFOX_BIN:-firefox}"
window_w=480
window_h=360

if [[ ! -d "${dist_dir}" ]]; then
  echo "[verify-wasm-proxy-firefox] エラー: ${dist_dir} が無い。先に build-wasm-proxy.sh を実行すること。" >&2
  exit 1
fi

if ! command -v "${firefox_bin}" >/dev/null 2>&1 && [[ ! -x "${firefox_bin}" ]]; then
  echo "[verify-wasm-proxy-firefox] エラー: firefox バイナリが見つからない（FANDHE_FIREFOX_BIN=${firefox_bin}）。" >&2
  exit 1
fi

if ! command -v xvfb-run >/dev/null 2>&1; then
  echo "[verify-wasm-proxy-firefox] エラー: xvfb-run が見つからない。Firefox headless は load イベント直後に" >&2
  echo "  撮影してしまい WASM 初期化前の白紙画像になるため、本スクリプトは xvfb 経由の撮影を必須とする。" >&2
  exit 1
fi

profile_dir="$(mktemp -d)"
cleanup() {
  if [[ -n "${server_pid:-}" ]]; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  # Firefox は xvfb-run のサブシェル内で ff_inner_pid として起動され、同サブシェル内の
  # kill/wait で撮影直後に停止される（下段参照）ため、この外側スコープで停止対象になる
  # Firefox プロセスは存在しない（レビュー指摘・Low: 未使用の ff_pid を削除）。
  rm -rf "${profile_dir}"
}
trap cleanup EXIT

# オンボーディング画面・データ収集通知を無効化する（新規一時プロファイルのみに適用）。
cat > "${profile_dir}/user.js" <<'EOF'
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("browser.startup.firstrunSkipsHomepage", true);
user_pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
EOF

echo "[verify-wasm-proxy-firefox] python3 -m http.server ${port} --bind ${host} (${dist_dir})"
(cd "${dist_dir}" && python3 -m http.server "${port}" --bind "${host}" >/tmp/fandhe-proxy-firefox-http.log 2>&1) &
server_pid=$!

# サーバー起動待ち（固定 sleep ではなくポート応答をポーリング。verify-wasm-proxy.sh と同じ作法）。
server_ready=0
for _ in $(seq 1 20); do
  if curl -sf "http://${host}:${port}/index.html" >/dev/null 2>&1; then
    server_ready=1
    break
  fi
  sleep 0.5
done

if [[ "${server_ready}" -ne 1 ]]; then
  echo "[verify-wasm-proxy-firefox] エラー: http://${host}:${port}/index.html の起動確認がタイムアウトした。/tmp/fandhe-proxy-firefox-http.log を確認すること。" >&2
  exit 1
fi

# out_png は引数として渡し、Python ヒアドキュメント文字列へ直接埋め込まない
# （レビュー指摘・Low: パスに `"` を含む場合に壊れるのを避けるため、下段の
# 非ブランク検査と同様 sys.argv 経由で受け取る）。
capture_script="$(mktemp --suffix=.py)"
cat > "${capture_script}" <<'PYEOF'
import sys
from PIL import ImageGrab

out_path = sys.argv[1]
img = ImageGrab.grab()
img.save(out_path)
print("[verify-wasm-proxy-firefox] capture size:", img.size)
PYEOF

# 使い回し画像による見かけ上の成功を防ぐ（レビュー指摘・High: stale screenshot false success）。
# out_png に既存ファイル（git 管理下の証跡 PNG 等）が残っていると、内側シェルで撮影が失敗しても
# 後段の `[[ -f "${out_png}" ]]` 判定が古いファイルを検出して素通りしてしまうため、
# 撮影前に必ず削除して「新規キャプチャが実際に成功したか」だけを判定基準にする。
rm -f "${out_png}"

echo "[verify-wasm-proxy-firefox] xvfb 上で firefox --kiosk 起動 → WASM 初期化待ち → スクリーンショット取得"
xvfb_log="$(mktemp)"
# レビュー指摘・Medium 対応: `set -euo pipefail` 環境下では `xvfb-run` が非ゼロ終了した瞬間に
# スクリプトが即 abort し、直後の `xvfb_status=$?` 代入と後段の診断ログ出力（xvfb_log の cat）
# が実行されないまま終了してしまう。`xvfb_status=0; ... || xvfb_status=$?` の形にして
# `set -e` の影響を受けずに終了コードを捕捉し、必ず診断ログを出力できるようにする。
xvfb_status=0
xvfb-run -a --server-args="-screen 0 ${window_w}x${window_h}x24" bash -c "
  set -u
  '${firefox_bin}' --new-instance --no-remote --profile '${profile_dir}' --kiosk \
    --width=${window_w} --height=${window_h} \
    'http://${host}:${port}/index.html' &
  ff_inner_pid=\$!
  sleep 6
  capture_status=0
  python3 '${capture_script}' '${out_png}' || capture_status=\$?
  kill \"\${ff_inner_pid}\" 2>/dev/null || true
  wait \"\${ff_inner_pid}\" 2>/dev/null || true
  exit \"\${capture_status}\"
" > "${xvfb_log}" 2>&1 || xvfb_status=$?
cat "${xvfb_log}"
rm -f "${capture_script}" "${xvfb_log}"

# レビュー指摘・High の本体対応: 内側シェルは `set -e` を持たず `wait ... || true` で
# 常に終了コード 0 を返していたため、python3 のキャプチャ失敗が握り潰されていた。
# capture_status を明示的に内側シェルの終了コードとして伝搬させ、ここで検査する
# （xvfb-run 自体の終了コードにも capture_status が反映される）。
if [[ ${xvfb_status} -ne 0 ]]; then
  echo "[verify-wasm-proxy-firefox] エラー: スクリーンショット撮影コマンドが失敗した（終了コード ${xvfb_status}）。" >&2
  exit 1
fi

if [[ ! -f "${out_png}" ]]; then
  echo "[verify-wasm-proxy-firefox] エラー: スクリーンショットが生成されなかった: ${out_png}" >&2
  exit 1
fi

echo "[verify-wasm-proxy-firefox] 完了: ${out_png}"

# 非ブランク検査（verify-wasm-proxy.sh の Bugbot 指摘と同趣旨: 「見かけ上の成功」の排除）。
# 撮影に成功していても、WASM 初期化前の白紙画像のままではピクセル値が一様になるため、
# ピクセル値の標準偏差がしきい値を上回ることを「描画済み」の判定条件とする。
python3 - "${out_png}" <<'PYEOF'
import sys
from PIL import Image, ImageStat

path = sys.argv[1]
img = Image.open(path).convert("L")
stat = ImageStat.Stat(img)
stddev = stat.stddev[0]
threshold = 3.0
print(f"[verify-wasm-proxy-firefox] 非ブランク検査: stddev={stddev:.3f}（しきい値 {threshold}）")
if stddev <= threshold:
    print("[verify-wasm-proxy-firefox] エラー: スクリーンショットがほぼ一様な色で「白紙」の疑いがある。", file=sys.stderr)
    sys.exit(1)
print("[verify-wasm-proxy-firefox] 非ブランク検査: 合格")
PYEOF
