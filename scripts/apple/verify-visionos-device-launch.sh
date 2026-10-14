#!/usr/bin/env bash
# REQ-3 / TASK-3.5-S2（Issue #233、親: TASK-3.5 / #231）: visionOS 実機（Apple Vision Pro）
# 上で `FandheVisionApp` が証明書・プロビジョニングを伴う署名ビルドで起動することを、
# macOS 上で 1 コマンド実行できるようにするスクリプト。
#
# 背景・経緯:
# - `scripts/apple/verify-simulator-launch.sh`（#69）はシミュレータ向け（署名不要）であり、
#   実機向けの署名ビルド・devicectl 経由のインストール・起動は対象外。本スクリプトはその
#   実機版に相当するが、#232（S1 watchOS 実機検証）と並列実行されるため、共有スクリプトは
#   作らず visionOS 専用のファイルとして独立させている（衝突回避）。
# - 呼び出し先: `scripts/apple/build-bridge-xcframework.sh`（初回 XCFramework 先行生成）・
#   `scripts/apple/generate-xcodeproj.sh`（.xcodeproj 生成）。ビルドロジックは重複させず、
#   本スクリプトは両者と `xcodebuild` / `xcrun devicectl` の呼び出し順序の統合のみを担う。
# - 呼び出し元: 人間担当者が macOS 上で直接実行する。
#
# 実行環境について: 実機向け署名ビルド・devicectl 経由のインストール・起動には Xcode
# ツールチェーンと Apple Development 証明書が必須なため macOS 限定。Linux 実行時は手順を
# 案内して明示的に失敗する（ci.md「グローバル状態を汚す処理・野良インストールを書かない」
# 準拠のため、xcodegen 等の自動インストールはしない。存在確認と導入案内のみ行う）。
#
# 前提条件:
#   - Issue #12（TASK-18.2）完了: Apple Developer Program 登録・Apple Development 証明書・
#     Development プロビジョニングプロファイル・実機 UDID のデバイス登録
#   - Apple Vision Pro 実機の調達・Developer Mode 有効化・Xcode との Wi-Fi ペアリング
#   - macOS + Xcode（visionOS SDK）・xcodegen 導入済み
#
# 使い方: scripts/apple/verify-visionos-device-launch.sh
#   環境変数 FANDHE_VISION_DEVICE でデバイス名を上書きできる（既定: `xcrun devicectl list
#   devices` から visionOS 実機の先頭デバイスを自動選択）。devicectl の出力はカラム区切り表
#   であり名前欄の抽出に実機依存の前提があるため、値は実在するデバイス名との前方一致で
#   判定する（詳細は resolve_device_identifier 内コメント参照）。内部では前方一致で
#   特定した行から実在確認済みのデバイス識別子（UUID）のみを抽出し、後続コマンドへ渡す
#   （A03 インジェクション対策。名前一致の厳密さに関わらずコマンド引数へは UUID のみを
#   展開する方針は verify-simulator-launch.sh の UDID 解決と同一）。
#   環境変数 FANDHE_DEVELOPMENT_TEAM（Apple Developer Team ID）の設定が必須。
#   実機向け署名ビルドは Automatic Signing であっても CLI（xcodebuild -allowProvisioningUpdates）
#   からは対象チームを解決できず未設定のまま実行すると署名エラーで失敗するため、
#   本スクリプトは実行前チェックとして未設定を検出し案内のうえ即座に失敗する（fail-fast）。
#   ビルド設定へは `DEVELOPMENT_TEAM=<値>` として注入する（project.yml・リポジトリへは
#   書き込まない。値そのものはログへ出力せず、xcodebuild の出力ダンプからもマスクする）。
#
# 冪等性について: 既にインストール済みのアプリに対して再実行しても失敗しない
#   （install は上書きインストールとして扱われる。simulator 版と同様の方針）。
#
# 本スクリプトの標準出力末尾に結果記録テンプレートへ転記すべき値を整形出力する。
# 進捗ログ・xcodebuild の出力は標準エラー出力へ書く（標準出力はテンプレートと
# 関数の戻り値専用に保つ。ログとの混在を避けるため）。
#
# 目視確認が必要で自動化対象外の項目（手順書の確認項目に残る）:
# - 表示文言がフォールバック文言（`"画面を表示できません"`）でなく
#   `fandhe_core::sample_screen()` 由来の文言であること
# - ボタン押下の操作が Rust コア側の状態更新に反映されること
# - 一定時間クラッシュしないこと
#
# セキュリティ上の注意（security.md 準拠）:
# - 証明書シリアル・Team ID・UDID はログ・stdout へ一切出力しない（件数・成否のみ表示）。
# - `DEVELOPMENT_TEAM` の値そのものは echo しない。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
apple_dir="${repo_root}/platforms/apple"
xcodeproj_path="${apple_dir}/Fandhe.xcodeproj"
xcframework_info_plist="${apple_dir}/FandheSwiftUIBridge/Artifacts/FandheBridge.xcframework/Info.plist"
# `/target` は .gitignore 済み（Rust ビルド成果物向けだが、実機向け DerivedData も
# 生成物であり追跡対象外であるべきなので流用する。新規 .gitignore エントリ不要）。
derived_data_dir="${repo_root}/target/apple-device-derived-data"
build_log_dir="${derived_data_dir}/build-logs"

scheme_name="FandheVisionApp"
app_name="FandheVisionApp"
bundle_id="com.fandhe.visionapp"

# 進捗ログ・エラーは標準エラー出力へ書く（標準出力は関数の戻り値・最終テンプレート専用）。
log() {
  echo "[verify-visionos-device-launch] $*" >&2
}

err() {
  echo "[verify-visionos-device-launch] エラー: $*" >&2
}

# --- 1. 環境検査（macOS + 必須ツール） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  err "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン必須）。"
  echo "[verify-visionos-device-launch] 手順: macOS 上で以下を実行してください。" >&2
  echo "[verify-visionos-device-launch]   1. brew install xcodegen" >&2
  echo "[verify-visionos-device-launch]   2. bash scripts/apple/verify-visionos-device-launch.sh" >&2
  exit 1
fi

for tool in xcodebuild xcrun xcodegen; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    err "${tool} が見つかりません。"
    echo "[verify-visionos-device-launch] 導入手順（自動インストールは行いません — ci.md 準拠）: Xcode / Xcode Command Line Tools を導入し、xcodegen は 'brew install xcodegen' で導入してください。" >&2
    exit 1
  fi
done

if ! xcrun devicectl --version >/dev/null 2>&1; then
  err "xcrun devicectl が利用できません（Xcode 15 以降が必要です）。"
  echo "[verify-visionos-device-launch] Xcode を最新版へ更新してください。" >&2
  exit 1
fi

mkdir -p "${build_log_dir}"

# --- 2. 署名前提の検査（Apple Development 証明書） --------------------------
# 証明書シリアル・Team ID はログへ出力しない（件数のみ表示。security.md 準拠）。
codesigning_identity_count="$(security find-identity -v -p codesigning 2>/dev/null | grep -cE '"Apple Development' || true)"
if [[ "${codesigning_identity_count}" -lt 1 ]]; then
  err "Apple Development 証明書がキーチェーンに見つかりません（0 件）。"
  echo "[verify-visionos-device-launch] docs/environment/apple-developer-setup.md の手順に従い、Apple Developer Program 登録・証明書取得（Issue #12）を先に完了してください。" >&2
  exit 1
fi
log "Apple Development 証明書を ${codesigning_identity_count} 件検出しました（詳細はログへ出力しません）。"

# FANDHE_DEVELOPMENT_TEAM の未設定を実行前に検出する（fail-fast）。
# Automatic Signing であっても CLI（xcodebuild -allowProvisioningUpdates）は Xcode の
# ログイン済みアカウントに紐づく既定チームを解決できないため、未設定のまま実行すると
# 署名前提（証明書・プロビジョニング）を満たしていても xcodebuild build の段階で
# 署名エラーとして失敗する。値自体はログへ出力しないため件数・成否のみ表示する。
if [[ -z "${FANDHE_DEVELOPMENT_TEAM:-}" ]]; then
  err "環境変数 FANDHE_DEVELOPMENT_TEAM が未設定です。"
  echo "[verify-visionos-device-launch] 実機向け署名ビルドには Apple Developer Team ID が必要です。" >&2
  echo "[verify-visionos-device-launch] 手順: Apple Developer サイト（Membership 詳細）または" >&2
  echo "[verify-visionos-device-launch]   Xcode > Settings > Accounts > (Apple ID を選択) > Team 一覧で確認し、" >&2
  echo "[verify-visionos-device-launch]   'export FANDHE_DEVELOPMENT_TEAM=<Team ID>' を実行してから再実行してください" >&2
  echo "[verify-visionos-device-launch]   （Team ID 自体をコミット・ログ・Issue へ貼り付けないこと。security.md 準拠）。" >&2
  exit 1
fi
log "FANDHE_DEVELOPMENT_TEAM の設定を確認しました（値はログへ出力しません）。"

# --- 3. 初回制約への対応（XCFramework 先行生成） ----------------------------
# generate-xcodeproj.sh 冒頭コメントに記載のとおり、Package.swift の binaryTarget は
# SwiftPM マニフェスト評価（依存解決）時点で XCFramework の存在を判定するため、
# 初回のみ「xcodegen generate / Xcode を開く」より前に生成しておく必要がある。
if [[ ! -f "${xcframework_info_plist}" ]]; then
  log "Artifacts/FandheBridge.xcframework が未生成のため先行生成します。"
  bash "${script_dir}/build-bridge-xcframework.sh" 1>&2
else
  log "Artifacts/FandheBridge.xcframework は生成済みです（先行生成をスキップ）。"
fi

# --- 4. .xcodeproj 生成 ------------------------------------------------------
log "generate-xcodeproj.sh を実行します。"
bash "${script_dir}/generate-xcodeproj.sh" 1>&2

# --- 5. 実機デバイス解決 ------------------------------------------------------
#
# `xcrun devicectl list devices` の出力から visionOS 実機を抽出し、
# "<表示名>|<識別子>" の形式で 1 件返す。FANDHE_VISION_DEVICE が指定された場合は
# デバイス名の前方一致を許容する（simulator 版の resolve_sim_device と異なり、
# devicectl はカラム区切り表であるため名前欄の抽出方法が異なる。下記コメント参照）。
# 内部では実在確認済みの識別子（UUID）のみを install/launch へ渡す
# （A03 インジェクション対策。デバイス名は自由テキストのため直接コマンド引数へ展開しない。
# この安全性は名前一致の厳密さ（前方一致か完全一致か）に依存しない）。
#
# 【devicectl の出力形式に関する前提（要 macOS 実機確認）】
# `xcrun devicectl list devices` はヘッダー行＋
# 「Name  Hostname  Identifier  State  Model」の 2 個以上スペース区切り表を出力する
# 想定（Xcode 15 系の一般的な形式）。simctl（`Name (UDID) (State)`）とは異なり、
# 名前欄と UUID の間に Hostname 列が挟まるため、UUID 手前までを名前として
# 単純に trim すると Hostname を巻き込む。そのため本スクリプトでは
# 「2 個以上の空白」をカラム区切りとみなして名前欄のみを抽出する
# （デバイス名自体は単一スペースを含みうるが、カラム区切りは通常それより広い
# ことを前提とする）。この前提が実機の実際の出力と異なる場合、Mac 側の運用者は
# 下記 `extract_device_name` の sed パターンを実測値に合わせて調整すること。
extract_device_name() {
  sed -E 's/[[:space:]]{2,}.*$//' | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//'
}

# devicectl の生出力（Identifier 列に UUID を含む）をログ・stderr へ出す前に必ず通す。
# UDID をログに残さない方針（本ファイル冒頭のコメント・security.md・
# Mac 側フォローアップ手順の「スクリプト出力を貼る前に識別子をスクラブする」運用と整合させる）。
mask_device_identifiers() {
  sed -E 's/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/<REDACTED-UDID>/g'
}

# xcodebuild の出力（失敗時ログダンプ等）に UDID に加え DEVELOPMENT_TEAM（Team ID）が
# 含まれうる箇所を通す共通マスク。xcodebuild はビルド設定を `DEVELOPMENT_TEAM = XXXXXXXXXX`
# の形でログへそのまま書き出すため、UUID マスクだけでは Team ID が漏洩する（security.md・
# 本ファイル冒頭「DEVELOPMENT_TEAM の値そのものは echo しない」に抵触するため必須）。
# 1) FANDHE_DEVELOPMENT_TEAM はスクリプト冒頭の fail-fast チェックで必須化済みのため、
#    呼び出し時点で必ず値が入っている。リテラル一致で置換する（主経路）。
# 2) 保険として `DEVELOPMENT_TEAM = <値>` 形式のビルド設定行そのものも捕捉する
#    （代入の右辺のみを対象にした狭い正規表現。一般的な英数字 10 文字全体を対象にすると
#    xcodebuild ログ中の無関係なトークン（ターゲット名・ハッシュ断片等）まで誤って
#    マスクし、失敗診断に必要な情報を潰してしまうため、パターンを限定する）。
mask_secrets() {
  local input
  input="$(cat)"
  if [[ -n "${FANDHE_DEVELOPMENT_TEAM:-}" ]]; then
    input="${input//${FANDHE_DEVELOPMENT_TEAM}/<REDACTED-TEAM>}"
  fi
  printf '%s\n' "${input}" \
    | mask_device_identifiers \
    | sed -E 's/(DEVELOPMENT_TEAM[[:space:]]*[:=][[:space:]]*)[A-Za-z0-9]+/\1<REDACTED-TEAM>/g'
}

resolve_device_identifier() {
  local override_value="${FANDHE_VISION_DEVICE:-}"
  local list_output
  if ! list_output="$(xcrun devicectl list devices 2>&1)"; then
    err "xcrun devicectl list devices に失敗しました: $(echo "${list_output}" | mask_device_identifiers)"
    exit 1
  fi

  # visionOS 実機は Model 列等に "Vision" を含む行として現れる想定。
  # UUID は `[0-9A-Fa-f-]{36}` 形式で抽出する（simulator 版の extract_udid と同一パターン）。
  local vision_lines
  vision_lines="$(echo "${list_output}" | grep -iE 'vision' || true)"

  # available（接続・ペアリング済み）でない行を選ぶと install が失敗するため、
  # 該当時は警告ログへ状態を残す（simulator 版の `list devices available` 相当の
  # フィルタが devicectl には存在しないため、状態列を目視できるようログに残す方針）。
  local matched_line=""
  if [[ -n "${override_value}" ]]; then
    matched_line="$(echo "${vision_lines}" | while IFS= read -r line; do
      name="$(extract_device_name <<<"${line}")"
      # 完全一致だと Hostname 巻き込みの実測差異で常に不一致になりうるため、
      # 前方一致（名前欄が override_value で始まる）を許容する。
      if [[ "${name}" == "${override_value}"* ]]; then
        echo "${line}"
        break
      fi
    done)"
    if [[ -z "${matched_line}" ]]; then
      err "FANDHE_VISION_DEVICE=$(echo "${override_value}" | mask_device_identifiers) は接続済みの visionOS 実機に見つかりません（前方一致で判定）。"
      echo "[verify-visionos-device-launch] 接続済みデバイス一覧（該当行。識別子（UUID）はマスク済み）:" >&2
      echo "${vision_lines}" | mask_device_identifiers >&2
      exit 1
    fi
  else
    matched_line="$(echo "${vision_lines}" | head -n 1)"
    if [[ -z "${matched_line}" ]]; then
      err "接続済みの visionOS 実機が見つかりません。"
      echo "[verify-visionos-device-launch] Apple Vision Pro の Developer Mode を有効化し、Xcode と同一 Wi-Fi でペアリングしてください。" >&2
      exit 1
    fi
  fi

  if ! grep -qi 'available' <<<"${matched_line}"; then
    log "警告: 選択したデバイス行に 'available' 相当の状態表記が見当たりません。接続・ペアリング状態を確認してください（該当行の状態表記はログ上部の devicectl 生出力を参照）。"
  fi

  local device_name
  device_name="$(extract_device_name <<<"${matched_line}")"
  local device_identifier
  device_identifier="$(echo "${matched_line}" | grep -oE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}' | head -n 1 || true)"
  if [[ -z "${device_identifier}" ]]; then
    err "デバイス行から識別子（UUID）を抽出できませんでした（該当行の形式が想定と異なります）。devicectl の出力形式が本スクリプトの前提と異なる可能性があります。"
    exit 1
  fi
  echo "${device_name}|${device_identifier}"
}

device_resolved="$(resolve_device_identifier)"
device_name="${device_resolved%%|*}"
device_identifier="${device_resolved##*|}"
# UDID・個体識別子はログへ出力しない（機種名のみ表示。security.md 準拠）。
log "visionOS 実機デバイス: ${device_name}"

# --- 6. 実機向け署名ビルド ----------------------------------------------------
#
# `-allowProvisioningUpdates` は Apple Development 証明書・Development プロファイルの
# 自動更新を許可するが、署名種別自体は project.yml の CODE_SIGN_STYLE: Automatic の
# 範囲に閉じる（A05: 過剰権限を追加しない）。FANDHE_DEVELOPMENT_TEAM は手順 2 で
# 未設定を検出済み（fail-fast）のため、ここでは無条件にビルド設定へ注入する。
# 値自体はログへ出力せず、xcodebuild の失敗ログダンプからも mask_secrets でマスクする。
build_args=(
  build
  -project "${xcodeproj_path}"
  -scheme "${scheme_name}"
  -destination "generic/platform=visionOS"
  -derivedDataPath "${derived_data_dir}"
  -allowProvisioningUpdates
  "DEVELOPMENT_TEAM=${FANDHE_DEVELOPMENT_TEAM}"
)

log "xcodebuild build -scheme ${scheme_name} -destination 'generic/platform=visionOS'"
log_file="${build_log_dir}/${scheme_name}-device.log"
if ! xcodebuild "${build_args[@]}" >"${log_file}" 2>&1; then
  err "xcodebuild build に失敗しました（${scheme_name}）。詳細ログ: ${log_file}"
  # 詳細ログファイル自体（${log_file}）には Team ID 等がマスクされずに残るため、
  # フォローアップ（GitHub issue へ貼り付け等）ではこの stderr 出力（マスク済み）のみを
  # 使うこと。ログファイルそのものを配布・貼付しないこと（security.md 準拠）。
  tail -n 40 "${log_file}" | mask_secrets >&2
  exit 1
fi

app_path="$(find "${derived_data_dir}/Build/Products" -maxdepth 2 -type d -name "${app_name}.app" 2>/dev/null | head -n 1 || true)"
if [[ -z "${app_path}" ]]; then
  err "${app_name}.app のビルド成果物が見つかりません（${derived_data_dir}/Build/Products 配下）。詳細ログ: ${log_file}"
  exit 1
fi

# --- 7. インストール・起動 ----------------------------------------------------
#
# UDID のみを引数として devicectl へ渡す（A03 対策。resolve_device_identifier が
# 実在確認済み）。launch 成功（PID 取得）を成否判定とする。
log "xcrun devicectl device install app --device <device> ${app_path}"
install_output=""
if ! install_output="$(xcrun devicectl device install app --device "${device_identifier}" "${app_path}" 2>&1)"; then
  err "devicectl device install app に失敗しました: $(echo "${install_output}" | mask_device_identifiers)"
  exit 1
fi
log "install 完了。"

log "xcrun devicectl device process launch --device <device> ${bundle_id}"
launch_output=""
if ! launch_output="$(xcrun devicectl device process launch --device "${device_identifier}" "${bundle_id}" 2>&1)"; then
  err "devicectl device process launch に失敗しました: $(echo "${launch_output}" | mask_device_identifiers)"
  exit 1
fi

# 成否判定は devicectl launch コマンド自体の終了コードのみで行う（上の if で確認済み）。
# PID の抽出はテンプレートへの参考情報にすぎず、devicectl の実際の出力キー名は
# 本環境（Linux）では実測できないため、抽出できない場合でも致命エラーにしない
# （launch 自体が成功した最後のステップで exit させ、macOS 側の運用者を
# 手詰まりにしないため）。抽出できなかった場合は生出力をそのままテンプレートへ流す。
launch_pid="$(echo "${launch_output}" | grep -oE 'processIdentifier[[:space:]:=]+[0-9]+' | grep -oE '[0-9]+' | head -n 1 || true)"
if [[ -z "${launch_pid}" ]]; then
  launch_pid="(未取得。生出力（識別子（UUID）はマスク済み）: $(echo "${launch_output}" | mask_device_identifiers))"
  log "警告: process launch の出力から PID を自動抽出できませんでした。生出力をテンプレートへ転記します（devicectl の出力キー名が想定と異なる可能性があります）。"
else
  log "launch 成功（PID: ${launch_pid}）。"
fi

# --- 8. 結果記録テンプレート向けの整形出力（標準出力） -------------------------
log "完了。以下の結果記録テンプレートを控えてください。"
cat <<EOF

--- visionOS 実機 ---
機種: ${device_name}
起動結果: 成功（devicectl launch の終了コードで判定。PID: ${launch_pid}）

--- 目視確認（自動化対象外・手動で確認のうえ記入） ---
表示文言（フォールバックでないか）:
一定時間クラッシュしないこと:
スクリーンショット:
合否:

（注意）UDID・証明書シリアル・Team ID は記録に含めないこと（security.md 準拠）。
EOF
