#!/usr/bin/env bash
# TASK-5.3（REQ-5 / M4、Issue #238）: iOS シミュレータ上で `frame-metrics` feature
# 有効ビルドを起動し、`FANDHE_FRAME_METRICS=<json>` 行を捕捉して参考値として記録する。
#
# 位置づけ: PoC-4（`docs/spec/03-poc/performance/README.md`）が「条件付き達成・参考値」
# に留めた macOS プロキシ実測を、固定 60Hz パネル実機（iPhone 15、TASK-5.3 の
# 「固定 60Hz 環境」要件に合致）で
# 再計測するための手順（検証記録は非公開）から
# 呼び出される。**シミュレータは可変リフレッシュレート・実機と異なる GPU パスのため
# 参考値に過ぎず、本スクリプトの出力は REQ-5 受け入れ基準の判定に使わない**
# （実機計測は同手順書 §4 の人間担当セクションで別途行う）。
#
# 呼び出し先: `scripts/ios/build-simulator-app.sh`（cargo build → .app 組み立て →
# ad-hoc 署名 → .ipa 化。フェーズ分割済み）。`FANDHE_IOS_CARGO_FEATURES=frame-metrics` を
# 与えて計測専用ビルドへ切り替える（`crates/fandhe-egui-proxy/src/frame_metrics.rs`
# モジュール doc の「feature ゲート契約」参照）。ビルド・署名・パッケージング・
# シミュレータ起動確認のロジックは重複実装せず、`verify-simulator-render.sh` と同じ
# UDID 解決・boot/install/launch パターンを踏襲する。
#
# 実行環境について: iOS シミュレータ向け cargo build（Apple SDK リンカ必須）・
# codesign・xcrun simctl は macOS 限定。Linux 実行時は手順を案内して明示的に失敗する
# （ci.md 準拠、自動インストールはしない）。
#
# 使い方: scripts/ios/measure-frame-rate.sh
#   前提: macOS 14+ / Xcode 15+（`rustup target add aarch64-apple-ios-sim` 実行済み）。
#   環境変数 FANDHE_IOS_SIM_DEVICE で起動先シミュレータのデバイス名を上書きできる
#   （`verify-simulator-render.sh` と同じ規約: 完全一致・UDID へ解決してから使用）。
#
# 出力: `FANDHE_FRAME_METRICS=<json>` を含む `xcrun simctl launch --console` の
# 標準出力・エビデンス用スクリーンショットを `_/verification/task-5.3/` へ保存し、
# 標準出力へ結果記録テンプレートを整形出力する（`verify-simulator-render.sh` と同様、
# 進捗ログは標準エラー出力へ書き、標準出力はテンプレート専用に保つ）。
set -euo pipefail

unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

app_name="FandheIosApp"
bundle_id="com.fandhe.iosapp"
out_dir="${FANDHE_IOS_OUT_DIR:-${repo_root}/target/ios-sim-frame-metrics}"
evidence_dir="${repo_root}/_/verification/task-5.3"
min_ios_major_version="17"

log() {
  echo "[measure-frame-rate:ios] $*" >&2
}

err() {
  echo "[measure-frame-rate:ios] エラー: $*" >&2
}

# --- 0. 実描画確認と両立しない委譲先環境変数の拒否 --------------------------
# `verify-simulator-render.sh` と同じ理由（未署名・スタブバイナリでは計測にならない）。
for forbidden_var in FANDHE_IOS_SKIP_SIGN FANDHE_IOS_APP_BINARY; do
  forbidden_value="${!forbidden_var:-}"
  if [[ -n "${forbidden_value}" ]]; then
    err "${forbidden_var} が設定されています。計測には実ビルド・実署名が必要なため、unset して再実行してください。"
    exit 1
  fi
done

# --- 1. 環境検査（macOS + 必須ツール） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  err "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン・xcrun simctl 必須）。"
  echo "[measure-frame-rate:ios] 手順: macOS 上で以下を実行してください。" >&2
  echo "[measure-frame-rate:ios]   1. rustup target add aarch64-apple-ios-sim" >&2
  echo "[measure-frame-rate:ios]   2. bash scripts/ios/measure-frame-rate.sh" >&2
  exit 1
fi

for tool in xcrun rustup cargo; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    err "${tool} が見つかりません（自動インストールは行いません — ci.md 準拠）。"
    exit 1
  fi
done

if ! xcrun simctl help >/dev/null 2>&1; then
  err "xcrun simctl が利用できません（Xcode Command Line Tools の導入・Xcode パス設定を確認してください）。"
  exit 1
fi

mkdir -p "${evidence_dir}"

# --- 2. ビルド委譲: build-simulator-app.sh へ計測専用 feature 付きで全フェーズ委譲 ---
log "FANDHE_IOS_CARGO_FEATURES=frame-metrics で scripts/ios/build-simulator-app.sh（全フェーズ）を実行します。"
FANDHE_IOS_OUT_DIR="${out_dir}" FANDHE_IOS_CARGO_FEATURES="frame-metrics" \
  bash "${script_dir}/build-simulator-app.sh" 1>&2

if [[ ! -d "${out_dir}" ]]; then
  err "ビルド出力先ディレクトリが見つかりません: ${out_dir}"
  exit 1
fi
out_dir="$(cd -- "${out_dir}" >/dev/null 2>&1 && pwd -P)"
app_dir="${out_dir}/${app_name}.app"

if [[ ! -d "${app_dir}" ]]; then
  err "アプリバンドルが見つかりません: ${app_dir}（build-simulator-app.sh の失敗が疑われます）"
  exit 1
fi

# --- 3. シミュレータデバイス解決（verify-simulator-render.sh と同じ規約） --------
extract_udid() {
  grep -oE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}'
}

extract_device_name() {
  sed -E 's/\([0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\).*$//' \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'
}

filter_devices_by_min_ios_major_version() {
  local min_major="$1"
  local current_major=0
  local line
  while IFS= read -r line; do
    if [[ "${line}" =~ ^--\ iOS\ ([0-9]+) ]]; then
      current_major="${BASH_REMATCH[1]}"
      continue
    fi
    if [[ "${line}" == "--"*"--"* ]]; then
      current_major=0
      continue
    fi
    if (( current_major >= min_major )); then
      echo "${line}"
    fi
  done
}

resolve_sim_device() {
  local override_value="${FANDHE_IOS_SIM_DEVICE:-}"
  local raw_devices
  raw_devices="$(xcrun simctl list devices available)"
  local all_available_devices
  all_available_devices="$(echo "${raw_devices}" | grep -E 'iPhone|iPad' || true)"

  local matched_line=""
  if [[ -n "${override_value}" ]]; then
    matched_line="$(echo "${all_available_devices}" | while IFS= read -r line; do
      name="$(extract_device_name <<<"${line}")"
      if [[ "${name}" == "${override_value}" ]]; then
        echo "${line}"
        break
      fi
    done)"
    if [[ -z "${matched_line}" ]]; then
      err "FANDHE_IOS_SIM_DEVICE=${override_value} は利用可能なシミュレータデバイスに見つかりません（完全一致のみ許容）。"
      exit 1
    fi
  else
    local default_candidates
    default_candidates="$(echo "${raw_devices}" | filter_devices_by_min_ios_major_version "${min_ios_major_version}" | grep -E 'iPhone|iPad' || true)"
    matched_line="$(echo "${default_candidates}" | head -n 1)"
    if [[ -z "${matched_line}" ]]; then
      err "iOS ${min_ios_major_version} 以降の利用可能なシミュレータデバイスが見つかりません（iPhone/iPad）。"
      exit 1
    fi
  fi

  local device_udid
  device_udid="$(extract_udid <<<"${matched_line}")"
  if [[ -z "${device_udid}" ]]; then
    err "デバイス行から UDID を抽出できませんでした: ${matched_line}"
    exit 1
  fi
  local device_name
  device_name="$(extract_device_name <<<"${matched_line}")"
  echo "${device_name}|${device_udid}"
}

is_device_booted() {
  local device_udid="$1"
  xcrun simctl list devices available \
    | grep -F "(${device_udid})" \
    | grep -qE '\(Booted\)[[:space:]]*$'
}

resolved="$(resolve_sim_device)"
device_name="${resolved%%|*}"
device_udid="${resolved##*|}"
log "iOS シミュレータデバイス: ${device_name} (${device_udid})"

if is_device_booted "${device_udid}"; then
  log "xcrun simctl boot ${device_udid} をスキップ（既に Booted）"
else
  log "xcrun simctl boot ${device_udid}"
  xcrun simctl boot "${device_udid}"
  xcrun simctl bootstatus "${device_udid}" 1>&2
fi

log "xcrun simctl install ${device_udid} ${app_dir}"
xcrun simctl install "${device_udid}" "${app_dir}" >/dev/null

# --- 4. 計測本体の実行 -------------------------------------------------------
# `--console` はアプリの標準出力・標準エラー出力をフォアグラウンドで転送し続ける
# ため、`FrameMetricsApp`（600 フレーム収集完了、約 10 秒 @ 60Hz）が
# `FANDHE_FRAME_METRICS=<json>` を出力してからアプリを終了させる必要がある。
# 本スクリプトはタイムアウト付きで出力を捕捉し、行を検出し次第 `simctl terminate`
# する（アプリ自体は計測完了後もオーバーレイ表示を継続する設計、
# `frame_metrics.rs` の `FrameMetricsApp::update` 参照）。
timeout_seconds=60
console_log="${out_dir}/console.log"
log "xcrun simctl launch --console ${device_udid} ${bundle_id}（最大 ${timeout_seconds} 秒待機）"

metrics_line=""
# `timeout` コマンド（GNU coreutils）の有無に関わらず、常にバックグラウンド実行 +
# `FANDHE_FRAME_METRICS=` 行検出によるポーリング早期終了を用いる。`timeout` 実行時に
# 待機を固定 ${timeout_seconds} 秒に固定してしまうと、計測完了（約 10 秒）後も
# 残り時間分無駄に待機してしまうため（macOS 標準 coreutils 非対応分岐との非対称を解消）。
xcrun simctl launch --console "${device_udid}" "${bundle_id}" > "${console_log}" 2>&1 &
console_pid=$!
waited=0
while kill -0 "${console_pid}" 2>/dev/null && [[ "${waited}" -lt "${timeout_seconds}" ]]; do
  sleep 1
  waited=$((waited + 1))
  if grep -q "FANDHE_FRAME_METRICS=" "${console_log}" 2>/dev/null; then
    break
  fi
done

# ここではまだアプリを終了させない。`FrameMetricsApp::update`（frame_metrics.rs）は
# 計測完了後もオーバーレイウィンドウの表示を継続する設計のため、検出直後の
# アプリ生存中にスクリーンショットを撮ることで、これから記録する metrics_line と
# 完全に同じ計測ランのオーバーレイをエビデンスにできる（アプリを一旦終了して
# 再起動すると、そのランはゼロから計測をやり直すため、証跡画像と記録済みの
# メトリクス値が食い違ってしまう）。
metrics_line="$(grep -o 'FANDHE_FRAME_METRICS=.*' "${console_log}" | tail -n 1 || true)"
if [[ -z "${metrics_line}" ]]; then
  kill "${console_pid}" 2>/dev/null || true
  wait "${console_pid}" 2>/dev/null || true
  xcrun simctl terminate "${device_udid}" "${bundle_id}" >/dev/null 2>&1 || true
  err "FANDHE_FRAME_METRICS 行を ${timeout_seconds} 秒以内に捕捉できませんでした。コンソールログ: ${console_log}"
  exit 1
fi
log "計測結果を捕捉しました: ${metrics_line}"

# `eprintln!("FANDHE_FRAME_METRICS=...")` は同一 `update()` 呼び出し内で
# `egui::Window::show` より前に実行されるため、ログ検出の瞬間に必ずしも
# オーバーレイの描画がコンポジタへ反映済みとは限らない。1 フレーム分の余裕を
# 見てからスクリーンショットを取得する。
sleep 1

evidence_timestamp="$(date +%Y%m%d-%H%M%S)"
screenshot_path="${evidence_dir}/simulator-${evidence_timestamp}.png"
log "xcrun simctl io ${device_udid} screenshot ${screenshot_path}（オーバーレイ表示のエビデンス、記録した metrics_line と同一ラン）"
xcrun simctl io "${device_udid}" screenshot "${screenshot_path}" >&2 || true

kill "${console_pid}" 2>/dev/null || true
wait "${console_pid}" 2>/dev/null || true
xcrun simctl terminate "${device_udid}" "${bundle_id}" >/dev/null 2>&1 || true

# --- 5. 結果記録テンプレート向けの整形出力（標準出力） ------------------------
log "完了。以下の結果を控えてください（参考値扱い）。"
cat <<EOF

--- iOS シミュレータ（参考値。実機計測は別途 §4 の人間担当手順で行う） ---
デバイス機種: ${device_name}
${metrics_line}
スクリーンショット: ${screenshot_path}
取得日時: $(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF
