#!/usr/bin/env bash
# TASK-1.4（REQ-1 / M4、Issue #229、Conditional Go 条件 3）: iOS シミュレータへの
# 実インストール・起動・スクリーンショット取得までを macOS 上で 1 コマンド実行できる
# ようにするスクリプト。
#
# 背景・経緯:
# - TASK-1.3（#39、クローズ済み）で iOS 向け wgpu（Metal）切替（`ios.rs`）と
#   `aarch64-apple-ios(-sim)` 向けビルド・.ipa 生成（TASK-11.1-S2/S3、
#   `scripts/ios/build-simulator-app.sh`）までは完了したが、実描画（シミュレータへの
#   実インストール・起動・目視確認）は未検証のまま
#   （`ios.rs` 冒頭コメント
#   参照。いずれも本 issue へ明示的に申し送り済み）。
# - 呼び出し先: `scripts/ios/build-simulator-app.sh`（cargo build → .app 組み立て →
#   ad-hoc 署名 → .ipa 化。フェーズ分割済み、TASK-11.1-S1/S2/S3 / Issue #220〜222・#486）。
#   ビルド・署名・パッケージングのロジックは重複させず、本スクリプトは
#   同スクリプトの全フェーズ呼び出しと `xcrun simctl` の起動確認手順との統合のみを担う。
# - 呼び出し元: 人間担当者が macOS 上で直接実行する。
# - `scripts/apple/verify-simulator-launch.sh`（TASK-3.3-S5 / #69、watchOS・visionOS 版）の
#   先例に規約を揃える: simctl デバイス名 → UDID 解決（A03 対策）・冪等性（既に
#   Booted なら成功扱い）・Linux 実行時は手順を案内して明示的に失敗・進捗ログは
#   標準エラー出力へ書き標準出力は結果記録テンプレート専用に保つ、の 4 点を踏襲する。
#
# 実行環境について: iOS シミュレータ向け cargo build（Apple SDK リンカ必須）・
# codesign・xcrun simctl は macOS 限定。Linux 実行時は手順を案内して明示的に失敗する
# （ci.md「グローバル状態を汚す処理・野良インストールを書かない」準拠のため、
# Xcode 等の自動インストールはしない。存在確認と導入案内のみ行う）。
#
# 使い方: scripts/ios/verify-simulator-render.sh
#   前提: macOS 14+ / Xcode 15+（`rustup target add aarch64-apple-ios-sim` 実行済み）。
#   環境変数 FANDHE_IOS_SIM_DEVICE で起動先シミュレータのデバイス名を上書きできる
#   （既定: `xcrun simctl list devices available` から iPhone/iPad の先頭デバイスを
#   自動選択）。値は `xcrun simctl list devices available` に実在するデバイス名
#   （UDID 手前の名前欄）と完全一致するもののみを許容し、それ以外は検証エラーで
#   停止する。内部的には一致したデバイスの UDID（simctl 出力から抽出した
#   `[0-9A-Fa-f-]{36}` 形式のトークン）へ解決してから boot/install/launch/screenshot に
#   渡す（A03 インジェクション対策。デバイス名は空白や記号を含みうるため名前文字列を
#   直接コマンド引数に展開せず、実在確認済みの UDID のみを展開する）。
#
#   `scripts/ios/build-simulator-app.sh` 側の環境変数インターフェース
#   （`FANDHE_IOS_SIM_TARGET` / `FANDHE_IOS_PROFILE` / `FANDHE_IOS_OUT_DIR` 等）は
#   そのまま透過する（本スクリプトは export 済みの環境変数をサブプロセスへ引き継ぐのみで、
#   独自に解釈・上書きはしない）。
#
# 冪等性について: 既に boot 済みのデバイスに対して再実行しても失敗しない
#   （`scripts/apple/verify-simulator-launch.sh` の冪等方針を踏襲）。
#
# 本スクリプトの標準出力末尾に結果記録テンプレートへ転記すべき値を整形出力する。
# 進捗ログ・cargo/xcodebuild 相当の出力は標準エラー出力へ書く（標準出力は
# テンプレートと関数の戻り値専用に保つ。ログとの混在を避けるため）。
#
# 目視確認が必要で自動化対象外の項目（手順書 §4 のチェック表に残る）:
# - 日本語ラベルがフォールバック文言でなく `fandhe_core::sample_screen()` 由来の
#   文言（「代表 1 画面サンプル」等）で、tofu 表示でないこと
# - ボタン押下・テキスト入力の操作が Rust コア側の状態更新に反映されること
#
# シミュレータ向けビルドのため ad-hoc 署名（証明書・プロビジョニングプロファイルは
# 一切扱わない、security.md 準拠）。取得するスクリーンショットは
# `_/verification/task-1.4/` 配下に保存するが、個体識別子（UDID・シリアル等）は
# 一切含めない。
set -euo pipefail

# CDPATH が設定された環境では相対パスへの `cd` が意図しないディレクトリへ解決され
# うるため、起点で無効化する（`scripts/ios/build-simulator-app.sh` と同じ対策、
# Issue #488 Bugbot 指摘 / A05 対策の踏襲）。
unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

app_name="FandheIosApp"
bundle_id="com.fandhe.iosapp"
out_dir="${FANDHE_IOS_OUT_DIR:-${repo_root}/target/ios-sim}"
evidence_dir="${repo_root}/_/verification/task-1.4"

# アプリの `MinimumOSVersion`（`platforms/apple/FandheIosApp/Resources/Info.plist`）と
# 揃える。既定デバイス選択（`FANDHE_IOS_SIM_DEVICE` 未指定時）がこれ未満のランタイムに
# 解決されると `simctl install` が非互換エラーで必ず失敗するため、既定選択はこの
# メジャーバージョン以降のランタイムに限定する（Review 指摘 Medium 対応）。
min_ios_major_version="17"

# 進捗ログ・エラーは標準エラー出力へ書く（標準出力は関数の戻り値・最終テンプレート専用）。
log() {
  echo "[verify-simulator-render] $*" >&2
}

err() {
  echo "[verify-simulator-render] エラー: $*" >&2
}

# --- 0. 実描画確認と両立しない委譲先環境変数の拒否 --------------------------
# `build-simulator-app.sh` の `FANDHE_IOS_SKIP_SIGN` / `FANDHE_IOS_APP_BINARY` は
# Linux 上での構造検証専用の注入点（同スクリプト冒頭コメント参照）であり、
# 本スクリプトが実施する「シミュレータへの実インストール・実描画確認」とは
# 目的が相反する。未署名の `.app` は `simctl install` が受け付けず、スタブ
# バイナリは起動しても実際の画面が描画されないため、これらが設定されたまま
# 実行すると「起動成功」の体裁で中身の伴わない検証結果を生成しうる
# （A05 対策: 検証の意図と実体の食い違いを起点で拒否する。本開発環境の
# smoke テストで実際にこの 2 変数を export したまま実行してしまう事故が
# あったため、環境変数の引き継ぎ経路を素通しにせず明示的に検査する）。
for forbidden_var in FANDHE_IOS_SKIP_SIGN FANDHE_IOS_APP_BINARY; do
  forbidden_value="${!forbidden_var:-}"
  if [[ -n "${forbidden_value}" ]]; then
    err "${forbidden_var} が設定されています（値: ${forbidden_value}）。Linux 構造検証専用の注入点のため、実描画確認では使用できません。unset して再実行してください。"
    exit 1
  fi
done

# --- 1. 環境検査（macOS + 必須ツール） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  err "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン・xcrun simctl 必須）。"
  echo "[verify-simulator-render] 手順: macOS 上で以下を実行してください。" >&2
  echo "[verify-simulator-render]   1. rustup target add aarch64-apple-ios-sim" >&2
  echo "[verify-simulator-render]   2. bash scripts/ios/verify-simulator-render.sh" >&2
  exit 1
fi

for tool in xcrun rustup cargo; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    err "${tool} が見つかりません。"
    echo "[verify-simulator-render] 導入手順（自動インストールは行いません — ci.md 準拠）: Xcode / Xcode Command Line Tools・Rust ツールチェーンを導入してください。" >&2
    exit 1
  fi
done

if ! xcrun simctl help >/dev/null 2>&1; then
  err "xcrun simctl が利用できません（Xcode Command Line Tools の導入・Xcode パス設定を確認してください）。"
  exit 1
fi

mkdir -p "${evidence_dir}"

# --- 2. ビルド委譲: build-simulator-app.sh へ全フェーズ委譲 -------------------
# build/bundle/sign/ipa/verify の実体は build-simulator-app.sh に閉じており、
# 重複実装しない（FANDHE_IOS_SIM_TARGET 等の環境変数インターフェースはそのまま
# サブプロセスへ引き継がれる。呼び出し側で改めて解釈・上書きはしない）。
log "scripts/ios/build-simulator-app.sh（全フェーズ）を実行します。"
bash "${script_dir}/build-simulator-app.sh" 1>&2

# build-simulator-app.sh の out_dir 解決規約（絶対パスへの正規化）と同じ結果に
# 揃えるため、存在確認後に正規化する。
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

# --- 3. シミュレータデバイス解決 --------------------------------------------
# `xcrun simctl list devices available` の 1 行から UDID
# （`[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}`）を
# 抽出する。デバイス名は `(17.0)` のような括弧を含みうるため UDID を安定した
# 識別子として扱う（A03 対策の核。以降 boot/install/launch/screenshot には UDID のみを渡す）。
extract_udid() {
  grep -oE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}'
}

# `simctl list devices available` の 1 行から表示名を取り出す。UDID を含む括弧の
# 直前までを名前とすることで、名前中の括弧（機種バリエーション等）を保持したまま
# UDID・状態表記のみを取り除く（verify-simulator-launch.sh と同方式）。
extract_device_name() {
  sed -E 's/\([0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\).*$//' \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'
}

# `xcrun simctl list devices available` の出力を 1 行ずつ走査し、
# `-- iOS <version> --` 見出しでランタイムのメジャーバージョンを追跡しながら、
# 指定メジャーバージョン以上のセクションに属するデバイス行のみを通す
# （見出し以外の `--` 区切り、例: `-- tvOS ... --` / `-- watchOS ... --` /
# `-- Unavailable ... --` に入ったら追跡中バージョンを 0 にリセットする）。
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

# iOS（iPhone/iPad）に一致する利用可能なシミュレータデバイスを 1 件選び、
# "<表示名>|<UDID>" の形式で返す。FANDHE_IOS_SIM_DEVICE が指定された場合は
# デバイス名の完全一致（UDID 手前の名前欄の trim 済み文字列と一致）のみを許容する
# （明示指定時はユーザーの判断を優先し、バージョン絞り込みは行わない）。
# 既定選択（未指定時）は `min_ios_major_version` 未満のランタイムを候補から除外する
# （Review 指摘 Medium 対応: 除外しないと旧ランタイムの先頭デバイスが選ばれ
# `simctl install` が非互換エラーで必ず失敗しうるため）。
resolve_sim_device() {
  local override_value="${FANDHE_IOS_SIM_DEVICE:-}"
  local raw_devices
  raw_devices="$(xcrun simctl list devices available)"
  local all_available_devices
  all_available_devices="$(echo "${raw_devices}" | grep -E 'iPhone|iPad' || true)"

  local matched_line=""
  if [[ -n "${override_value}" ]]; then
    # 名前欄（UDID 手前）を trim して完全一致するデバイスのみ許容する
    # （部分一致だと `FANDHE_IOS_SIM_DEVICE=iPhone` のような曖昧な値が誤って
    # 通過し、後段の simctl 呼び出しでデバイス未特定のまま失敗しうるため）。
    matched_line="$(echo "${all_available_devices}" | while IFS= read -r line; do
      name="$(extract_device_name <<<"${line}")"
      if [[ "${name}" == "${override_value}" ]]; then
        echo "${line}"
        break
      fi
    done)"
    if [[ -z "${matched_line}" ]]; then
      err "FANDHE_IOS_SIM_DEVICE=${override_value} は利用可能なシミュレータデバイスに見つかりません（完全一致のみ許容）。"
      echo "[verify-simulator-render] 利用可能なデバイス一覧:" >&2
      echo "${all_available_devices}" >&2
      exit 1
    fi
  else
    local default_candidates
    default_candidates="$(echo "${raw_devices}" | filter_devices_by_min_ios_major_version "${min_ios_major_version}" | grep -E 'iPhone|iPad' || true)"
    matched_line="$(echo "${default_candidates}" | head -n 1)"
    if [[ -z "${matched_line}" ]]; then
      err "iOS ${min_ios_major_version} 以降の利用可能なシミュレータデバイスが見つかりません（iPhone/iPad）。アプリの MinimumOSVersion=17.0（platforms/apple/FandheIosApp/Resources/Info.plist）未満のランタイムしか導入されていない可能性があります。"
      echo "[verify-simulator-render] Xcode の Settings > Platforms から iOS ${min_ios_major_version} 以降のランタイムを導入するか、FANDHE_IOS_SIM_DEVICE で対象デバイスを明示指定してください。" >&2
      echo "[verify-simulator-render] 利用可能なデバイス一覧（全ランタイム）:" >&2
      echo "${all_available_devices}" >&2
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

# 対象デバイスが既に Booted かどうかを simctl の状態表記から判定する
# （`... (<UDID>) (Booted)` の末尾括弧を読む。冪等性確保のため boot 前に確認する）。
is_device_booted() {
  local device_udid="$1"
  xcrun simctl list devices available \
    | grep -F "(${device_udid})" \
    | grep -qE '\(Booted\)[[:space:]]*$'
}

# --- 4. boot（冪等）→ install → launch → screenshot ------------------------
# UDID のみを引数として simctl へ渡す（A03 対策。resolve_sim_device が実在確認済み）。
boot_install_launch_screenshot() {
  local device_udid="$1"
  local screenshot_path="$2"

  local just_booted=0
  if is_device_booted "${device_udid}"; then
    log "xcrun simctl boot ${device_udid} をスキップ（既に Booted）"
  else
    log "xcrun simctl boot ${device_udid}"
    local boot_output
    # 状態確認と boot 実行の間に別プロセスが boot させる競合稀に備え、
    # "already booted" 系のメッセージも許容する（冪等性の二重の担保。
    # verify-simulator-launch.sh の Boot race 対応と同方針）。
    if ! boot_output="$(xcrun simctl boot "${device_udid}" 2>&1)"; then
      if ! grep -qiE "already booted|current state: booted" <<<"${boot_output}"; then
        err "simctl boot に失敗しました: ${boot_output}"
        exit 1
      fi
      just_booted=0
      log "simctl boot が競合（既に Booted）で失敗したため、bootstatus は非致命的に扱います（device=${device_udid}）。"
    else
      just_booted=1
    fi
  fi

  # `simctl boot` はランタイムの準備完了を待たずに返るため、コールドブート
  # （初回起動）では Booting 状態のまま install/launch を実行すると失敗しうる。
  # `simctl bootstatus` で起動完了までブロックして待つ。本スクリプトが実際に
  # boot を実行した（just_booted=1）場合のみ、起動完了を確認できないことを
  # 致命エラーとする（verify-simulator-launch.sh と同方針）。
  log "xcrun simctl bootstatus ${device_udid}（起動完了待ち。コールドブート時は数分かかる場合があります）"
  if ! xcrun simctl bootstatus "${device_udid}" 1>&2; then
    if [[ "${just_booted}" == "1" ]]; then
      err "simctl bootstatus でシミュレータの起動完了を確認できませんでした（device=${device_udid}）。"
      exit 1
    fi
    log "警告: simctl bootstatus が非ゼロ終了しましたが、既に Booted のため続行します（device=${device_udid}）。"
  fi

  log "xcrun simctl install ${device_udid} ${app_dir}"
  if ! xcrun simctl install "${device_udid}" "${app_dir}" >/dev/null; then
    err "simctl install に失敗しました（device=${device_udid}, app=${app_dir}）。"
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

  # エビデンス取得。画面が描画されるまで一瞬の猶予を挟む（launch 直後は
  # まだ描画完了前のフレームが撮れることがあるため）。
  sleep 2
  log "xcrun simctl io ${device_udid} screenshot ${screenshot_path}"
  if ! xcrun simctl io "${device_udid}" screenshot "${screenshot_path}" >&2; then
    err "simctl io screenshot に失敗しました（device=${device_udid}）。"
    exit 1
  fi

  echo "${launch_output}"
}

resolved="$(resolve_sim_device)"
device_name="${resolved%%|*}"
device_udid="${resolved##*|}"
log "iOS シミュレータデバイス: ${device_name} (${device_udid})"

# 対象 UDID が属するランタイムの表示用バージョン文字列（例: "iOS 17.2"）を求める。
# `grep -B <固定行数>` の遡り窓だとランタイムが多い環境で見出し行が窓外に出て
# 「不明」へ落ちうるため、見出し追跡による 1 パス走査で解決する（Review 指摘 Low 対応）。
resolve_os_version_for_udid() {
  local target_udid="$1"
  local current_header=""
  local found_version=""
  local line
  while IFS= read -r line; do
    if [[ "${line}" =~ ^--\ (iOS\ [0-9.]+)\ -- ]]; then
      current_header="${BASH_REMATCH[1]}"
      continue
    fi
    if [[ "${line}" == "--"*"--"* ]]; then
      current_header=""
      continue
    fi
    if [[ "${line}" == *"(${target_udid})"* ]]; then
      found_version="${current_header}"
      break
    fi
  done < <(xcrun simctl list devices available)
  echo "${found_version}"
}

os_version="$(resolve_os_version_for_udid "${device_udid}")"

# 同日内の再実行で過去のエビデンス（スクリーンショット）を無言で上書きしないよう、
# 日付だけでなく時刻（秒単位）まで含めたタイムスタンプをファイル名へ使う
# （Review 指摘 Low 対応）。
evidence_timestamp="$(date +%Y%m%d-%H%M%S)"
screenshot_path="${evidence_dir}/simulator-${evidence_timestamp}.png"

launch_result="$(boot_install_launch_screenshot "${device_udid}" "${screenshot_path}")"

# --- 5. 結果記録テンプレート向けの整形出力（標準出力） ------------------------
log "完了。以下の結果記録テンプレートを控えてください。"
cat <<EOF

--- iOS シミュレータ ---
デバイス機種: ${device_name}
OS バージョン: ${os_version:-不明（xcrun simctl list devices available で確認）}
ビルドプロファイル: ${FANDHE_IOS_PROFILE:-release}
起動結果: 成功（${launch_result}）
スクリーンショット: ${screenshot_path}

--- 目視確認（自動化対象外・手動で確認のうえ記入） ---
表示文言（フォールバックでないか）:
日本語ラベルの描画（tofu でないか）:
ボタン押下・テキスト入力の状態反映:
要素欠落件数:
合否:
EOF
