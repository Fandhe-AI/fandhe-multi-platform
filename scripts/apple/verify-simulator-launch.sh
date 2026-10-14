#!/usr/bin/env bash
# REQ-3 / TASK-3.3-S5（Issue #69）: watchOS・visionOS 両シミュレータへの起動確認を
# macOS 上で 1 コマンド実行できるようにするスクリプト。
#
# 背景・経緯:
# - Issue #69 は PR #334 の
#   マージ後、受け入れ条件（両 OS シミュレータでの起動確認）自体が本開発環境
#   （Linux・Swift/Xcode 不在）では実施不能なため reopen され、人間担当の追跡 issue と
#   なっている。手順書作成時点のブロッカー（#59・TASK-3.4 系）はその後すべて解消済み
#   だが、手順書 §5-2 は汎用記述のまま実体（TASK-3.4 成果物）と対応しておらず、
#   人間担当者が初回 XCFramework 未生成の罠（generate-xcodeproj.sh 冒頭コメント参照）を
#   踏みかねない。本スクリプトはその手順を自動化し、実施コストと事故の芽を減らす。
# - 呼び出し先: `scripts/apple/build-bridge-xcframework.sh`（初回 XCFramework 先行生成）・
#   `scripts/apple/generate-xcodeproj.sh`（.xcodeproj 生成）。ビルドロジックは重複させず、
#   本スクリプトは両者と `xcodebuild` / `xcrun simctl` の呼び出し順序の統合のみを担う。
# - 呼び出し元: 人間担当者が macOS 上で直接実行する。
#
# 実行環境について: watchOS・visionOS シミュレータビルド・起動には Xcode ツールチェーンが
# 必須なため macOS 限定。Linux 実行時は手順を案内して明示的に失敗する
# （ci.md「グローバル状態を汚す処理・野良インストールを書かない」準拠のため、
# xcodegen 等の自動インストールはしない。存在確認と導入案内のみ行う）。
#
# 使い方: scripts/apple/verify-simulator-launch.sh
#   前提: macOS 14+ / Xcode 15+（watchOS・visionOS SDK・シミュレータランタイム導入済み）、
#         `brew install xcodegen` 等で xcodegen 導入済みであること。
#   環境変数 FANDHE_WATCH_SIM_DEVICE / FANDHE_VISION_SIM_DEVICE で起動先シミュレータの
#   デバイス名を上書きできる（既定: `xcrun simctl list devices available` から
#   watchOS/visionOS の先頭デバイスを自動選択）。値は `xcrun simctl list devices available`
#   に実在するデバイス名（UDID 手前の名前欄）と完全一致するもののみを許容し、それ以外は
#   検証エラーで停止する。内部的には一致したデバイスの UDID（simctl 出力から抽出した
#   `[0-9A-Fa-f-]{36}` 形式のトークン）へ解決してから boot/install/launch に渡す
#   （A03 インジェクション対策。デバイス名は空白や記号を含みうるため名前文字列を
#   直接コマンド引数に展開せず、実在確認済みの UDID のみを展開する）。
#
# 冪等性について: 既に boot 済みのデバイスに対して再実行しても失敗しない
#   （`scripts/apple/build-bridge-xcframework.sh` 等、既存スクリプト群の冪等方針を踏襲）。
#
# 本スクリプトの標準出力末尾に §5-3 結果記録テンプレートへ転記すべき値を整形出力する。
# 進捗ログ・xcodebuild の出力は標準エラー出力へ書く（標準出力は §5-3 テンプレートと
# 関数の戻り値専用に保つ。ログとの混在を避けるため）。
#
# 目視確認が必要で自動化対象外の項目（手順書 §5-2 の確認項目に残る）:
# - 表示文言がフォールバック文言（`"画面を表示できません"`）でなく
#   `fandhe_core::sample_screen()` 由来の文言であること
# - ボタン押下・テキスト入力の操作が Rust コア側の状態更新に反映されること
# - watchOS 側の `WatchScreenModel.lastError == nil`（デバッガ・ログ出力で確認）
#
# シミュレータ向けビルドのため署名不要。証明書・プロビジョニングプロファイルは
# 一切扱わない（security.md 準拠）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
apple_dir="${repo_root}/platforms/apple"
xcodeproj_path="${apple_dir}/Fandhe.xcodeproj"
xcframework_info_plist="${apple_dir}/FandheSwiftUIBridge/Artifacts/FandheBridge.xcframework/Info.plist"
# `/target` は .gitignore 済み（Rust ビルド成果物向けだが、DerivedData も同様に
# 生成物であり追跡対象外であるべきなので流用する。新規 .gitignore エントリ不要）。
derived_data_dir="${repo_root}/target/apple-simulator-derived-data"
build_log_dir="${derived_data_dir}/build-logs"

# 進捗ログ・エラーは標準エラー出力へ書く（標準出力は関数の戻り値・最終テンプレート専用）。
log() {
  echo "[verify-simulator-launch] $*" >&2
}

err() {
  echo "[verify-simulator-launch] エラー: $*" >&2
}

# --- 1. 環境検査（macOS + 必須ツール） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  err "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン必須）。"
  echo "[verify-simulator-launch] 手順: macOS 上で以下を実行してください。" >&2
  echo "[verify-simulator-launch]   1. brew install xcodegen" >&2
  echo "[verify-simulator-launch]   2. bash scripts/apple/verify-simulator-launch.sh" >&2
  exit 1
fi

for tool in xcodebuild xcrun xcodegen; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    err "${tool} が見つかりません。"
    echo "[verify-simulator-launch] 導入手順（自動インストールは行いません — ci.md 準拠）: Xcode / Xcode Command Line Tools を導入し、xcodegen は 'brew install xcodegen' で導入してください。" >&2
    exit 1
  fi
done

mkdir -p "${build_log_dir}"

# --- 2. 初回制約への対応（XCFramework 先行生成） ----------------------------
# generate-xcodeproj.sh 冒頭コメントに記載のとおり、Package.swift の binaryTarget は
# SwiftPM マニフェスト評価（依存解決）時点で XCFramework の存在を判定するため、
# 初回のみ「xcodegen generate / Xcode を開く」より前に生成しておく必要がある。
if [[ ! -f "${xcframework_info_plist}" ]]; then
  log "Artifacts/FandheBridge.xcframework が未生成のため先行生成します。"
  bash "${script_dir}/build-bridge-xcframework.sh" 1>&2
else
  log "Artifacts/FandheBridge.xcframework は生成済みです（先行生成をスキップ）。"
fi

# --- 3. .xcodeproj 生成 ------------------------------------------------------
log "generate-xcodeproj.sh を実行します。"
bash "${script_dir}/generate-xcodeproj.sh" 1>&2

# --- 4. ビルド・シミュレータ起動のための共通ヘルパー -------------------------

# `xcrun simctl list devices available` の 1 行から UDID
# （`[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}`）を
# 抽出する。デバイス名は `(45mm)` のような括弧を含みうるため UDID を安定した
# 識別子として扱う（A03 対策の核。以降 boot/install/launch には UDID のみを渡す）。
extract_udid() {
  grep -oE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}'
}

# `simctl list devices available` の 1 行から表示名を取り出す。デバイス名自体が
# `(45mm)` のようなケースサイズを括弧付きで含む（例: `Apple Watch Series 9 (45mm)`）
# ため、素朴に最初の `(` で切ると名前を壊す。UDID を含む括弧の直前までを名前とする
# ことで、名前中の括弧を保持したまま UDID・状態表記のみを取り除く。
extract_device_name() {
  sed -E 's/\([0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\).*$//' \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'
}

# 対象 OS（watchOS/visionOS）に一致する利用可能なシミュレータデバイスを 1 件選び、
# "<表示名>|<UDID>" の形式で返す。FANDHE_*_SIM_DEVICE が指定された場合はデバイス名の
# 完全一致（UDID 手前の名前欄の trim 済み文字列と一致）のみを許容する。
resolve_sim_device() {
  local os_grep_pattern="$1"
  local override_var_name="$2"
  local override_value="${!override_var_name:-}"
  local available_devices
  available_devices="$(xcrun simctl list devices available | grep -E "${os_grep_pattern}" || true)"

  local matched_line=""
  if [[ -n "${override_value}" ]]; then
    # 名前欄（先頭の `(` より前）を trim して完全一致するデバイスのみ許容する
    # （部分一致だと `FANDHE_WATCH_SIM_DEVICE=Watch` のような曖昧な値が誤って
    # 通過し、後段の simctl 呼び出しでデバイス未特定のまま失敗しうるため）。
    matched_line="$(echo "${available_devices}" | while IFS= read -r line; do
      name="$(extract_device_name <<<"${line}")"
      if [[ "${name}" == "${override_value}" ]]; then
        echo "${line}"
        break
      fi
    done)"
    if [[ -z "${matched_line}" ]]; then
      err "${override_var_name}=${override_value} は利用可能なシミュレータデバイスに見つかりません（完全一致のみ許容）。"
      echo "[verify-simulator-launch] 利用可能なデバイス一覧:" >&2
      echo "${available_devices}" >&2
      exit 1
    fi
  else
    matched_line="$(echo "${available_devices}" | head -n 1)"
    if [[ -z "${matched_line}" ]]; then
      err "利用可能なシミュレータデバイスが見つかりません（パターン: ${os_grep_pattern}）。"
      echo "[verify-simulator-launch] Xcode の Settings > Platforms からランタイムを導入してください。" >&2
      exit 1
    fi
  fi

  local device_name
  device_name="$(extract_device_name <<<"${matched_line}")"
  local device_udid
  device_udid="$(extract_udid <<<"${matched_line}")"
  if [[ -z "${device_udid}" ]]; then
    err "デバイス行から UDID を抽出できませんでした: ${matched_line}"
    exit 1
  fi
  echo "${device_name}|${device_udid}"
}

# xcodebuild build を実行し .app パスを解決する。DerivedData 配下の Build/Products/
# 直下に生成される規約（-derivedDataPath 指定時）に依存する。xcodebuild の出力全体は
# ログファイルへ保存し、失敗時にはそのパスを案内する（標準エラー出力への垂れ流しは
# 大量になるため要点のみ表示する）。
build_scheme_app() {
  local scheme="$1"
  local destination="$2"
  local app_name="$3"
  local log_file="${build_log_dir}/${scheme}.log"

  log "xcodebuild build -scheme ${scheme} -destination '${destination}'（ログ: ${log_file}）"
  if ! xcodebuild build \
    -project "${xcodeproj_path}" \
    -scheme "${scheme}" \
    -destination "${destination}" \
    -derivedDataPath "${derived_data_dir}" \
    >"${log_file}" 2>&1; then
    err "xcodebuild build に失敗しました（${scheme}）。詳細ログ: ${log_file}"
    tail -n 40 "${log_file}" >&2
    exit 1
  fi

  local app_path
  app_path="$(find "${derived_data_dir}/Build/Products" -maxdepth 2 -type d -name "${app_name}.app" | head -n 1)"
  if [[ -z "${app_path}" ]]; then
    err "${app_name}.app のビルド成果物が見つかりません（${derived_data_dir}/Build/Products 配下）。詳細ログ: ${log_file}"
    exit 1
  fi
  echo "${app_path}"
}

# 対象デバイスが既に Booted かどうかを simctl の状態表記から判定する
# （`... (<UDID>) (Booted)` の末尾括弧を読む。冪等性確保のため boot 前に確認する）。
is_device_booted() {
  local device_udid="$1"
  xcrun simctl list devices available \
    | grep -F "(${device_udid})" \
    | grep -qE '\(Booted\)[[:space:]]*$'
}

# boot（起動済みなら no-op）→ install → launch を行い、launch の PID 出力有無で成否判定する。
# UDID のみを引数として simctl へ渡す（A03 対策。resolve_sim_device が実在確認済み）。
boot_install_launch() {
  local device_udid="$1"
  local app_path="$2"
  local bundle_id="$3"

  local just_booted=0
  if is_device_booted "${device_udid}"; then
    log "xcrun simctl boot ${device_udid} をスキップ（既に Booted）"
  else
    log "xcrun simctl boot ${device_udid}"
    local boot_output
    # 状態確認と boot 実行の間に別プロセスが boot させる競合稀に備え、
    # "already booted" 系のメッセージも許容する（冪等性の二重の担保）。
    if ! boot_output="$(xcrun simctl boot "${device_udid}" 2>&1)"; then
      if ! grep -qiE "already booted|current state: booted" <<<"${boot_output}"; then
        err "simctl boot に失敗しました: ${boot_output}"
        exit 1
      fi
      # 「既に Booted」競合で boot が失敗したケースは、実質的に is_device_booted で
      # 検出できるはずだった既存ブートと同じ状態（Bugbot 指摘: Boot race mishandles
      # readiness failure）。このスクリプトが新規に boot を開始したわけではないため
      # just_booted は 0 のまま維持し、後続の bootstatus 失敗を非致命的に扱う
      # 「既に Booted」スキップパスとロジックを揃える。
      log "simctl boot が競合（既に Booted）で失敗したため、bootstatus は非致命的に扱います（device=${device_udid}）。"
    else
      just_booted=1
    fi
  fi

  # `simctl boot` はランタイムの準備完了を待たずに返るため、特に watchOS・visionOS の
  # コールドブート（初回起動）では Booting 状態のまま install/launch を実行すると
  # 失敗しうる（Bugbot 指摘: Missing simulator boot readiness wait）。
  # `simctl bootstatus` はブート完了までブロックするコマンドで待つ。冪等性のため
  # boot をスキップした分岐（既に Booted）でも無条件に呼び出すが、その分岐では
  # bootstatus が非ゼロ終了しても致命扱いにしない（simctl の未検証の挙動差異で
  # 既存の「既に Booted なら成功」パスを壊さないため）。本スクリプトが実際に
  # boot を実行した（just_booted=1）場合のみ、起動完了を確認できないことを致命
  # エラーとする。進捗（コールドブート時は数分かかりうる）は標準エラー出力へ
  # そのまま流し、標準出力は関数の戻り値専用に保つ契約を維持する。
  log "xcrun simctl bootstatus ${device_udid}（起動完了待ち。コールドブート時は数分かかる場合があります）"
  if ! xcrun simctl bootstatus "${device_udid}" 1>&2; then
    if [[ "${just_booted}" == "1" ]]; then
      err "simctl bootstatus でシミュレータの起動完了を確認できませんでした（device=${device_udid}）。"
      exit 1
    fi
    log "警告: simctl bootstatus が非ゼロ終了しましたが、既に Booted のため続行します（device=${device_udid}）。"
  fi

  log "xcrun simctl install ${device_udid} ${app_path}"
  # 標準出力は boot_install_launch の戻り値（launch 結果）としてテンプレートへ流用する契約のため、
  # install の標準出力（通常無出力だが将来的なメッセージ追加に備え）は破棄する。
  if ! xcrun simctl install "${device_udid}" "${app_path}" >/dev/null; then
    err "simctl install に失敗しました（device=${device_udid}, app=${app_path}）。"
    exit 1
  fi

  log "xcrun simctl launch ${device_udid} ${bundle_id}"
  local launch_output
  if ! launch_output="$(xcrun simctl launch "${device_udid}" "${bundle_id}" 2>&1)"; then
    err "simctl launch に失敗しました: ${launch_output}"
    exit 1
  fi
  if ! grep -qE "^${bundle_id}: [0-9]+$" <<<"${launch_output}"; then
    err "simctl launch の出力が想定形式（<bundle_id>: <pid>）と一致しません: ${launch_output}"
    exit 1
  fi
  echo "${launch_output}"
}

# --- 5. watchOS ---------------------------------------------------------------
watch_resolved="$(resolve_sim_device "Watch" FANDHE_WATCH_SIM_DEVICE)"
watch_device_name="${watch_resolved%%|*}"
watch_device_udid="${watch_resolved##*|}"
log "watchOS シミュレータデバイス: ${watch_device_name} (${watch_device_udid})"
watch_app_path="$(build_scheme_app "FandheWatchApp" "generic/platform=watchOS Simulator" "FandheWatchApp")"
watch_launch_result="$(boot_install_launch "${watch_device_udid}" "${watch_app_path}" "com.fandhe.watchapp")"

# --- 6. visionOS --------------------------------------------------------------
vision_resolved="$(resolve_sim_device "Vision" FANDHE_VISION_SIM_DEVICE)"
vision_device_name="${vision_resolved%%|*}"
vision_device_udid="${vision_resolved##*|}"
log "visionOS シミュレータデバイス: ${vision_device_name} (${vision_device_udid})"
vision_app_path="$(build_scheme_app "FandheVisionApp" "generic/platform=visionOS Simulator" "FandheVisionApp")"
vision_launch_result="$(boot_install_launch "${vision_device_udid}" "${vision_app_path}" "com.fandhe.visionapp")"

# --- 7. §5-3 結果記録テンプレート向けの整形出力（標準出力） -------------------
log "完了。以下の結果記録テンプレートを控えてください。"
cat <<EOF

--- watchOS ---
OS / シミュレータ機種: ${watch_device_name}
起動結果: 成功（${watch_launch_result}）

--- visionOS ---
OS / シミュレータ機種: ${vision_device_name}
起動結果: 成功（${vision_launch_result}）

--- 目視確認（自動化対象外・手動で確認のうえ記入） ---
表示文言（フォールバックでないか）:
lastError（watchOS）:
スクリーンショット:
合否:
EOF
