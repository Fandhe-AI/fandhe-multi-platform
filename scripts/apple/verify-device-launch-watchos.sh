#!/usr/bin/env bash
# REQ-3 / TASK-3.5-S1（Issue #232、親: #231「TASK-3.5」）: watchOS 実機（証明書・
# プロビジョニングプロファイル適用済み）へ `FandheWatchApp` をビルド・インストール・
# 起動し、クラッシュせず立ち上がることを検証するスクリプト。
#
# 位置づけ・呼び出し文脈:
# - `scripts/apple/verify-simulator-launch.sh`（TASK-3.3-S5 / #69）の設計
#   （macOS 限定・冪等・ログは stderr / 記録テンプレート値は stdout・UDID 解決による
#   A03 対策）を実機版として踏襲する。シミュレータと異なり実機は署名（証明書・
#   プロビジョニングプロファイル）が必須になる点、デバイス識別子の形式・取得手段
#   （`simctl` ではなく `devicectl`）が異なる点が主な相違。
# - 呼び出し先: `scripts/apple/build-bridge-xcframework.sh`（初回 XCFramework 先行生成。
#   既定ターゲットに実機向け `aarch64-apple-watchos` を含むため追加の引数指定は不要）・
#   `scripts/apple/generate-xcodeproj.sh`（.xcodeproj 生成）。
# - 呼び出し元: 人間担当者が macOS 上で直接実行する。
#
# 実行環境について: 実機ビルド・インストール・起動には Xcode ツールチェーン・
# 署名済みプロビジョニングプロファイル・ペアリング済み実機が必須なため macOS 限定。
# Linux 実行時は手順を案内して明示的に失敗する（ci.md「グローバル状態を汚す処理・
# 野良インストールを書かない」準拠のため、xcodegen 等の自動インストールはしない）。
# 本スクリプト自体も CI には組み込まない（実機・署名必須のためローカル実行のみ）。
#
# 前提（本スクリプトが検査するのではなく、実行前に満たされている必要がある事項）:
# - Issue #12（TASK-18.2）: Apple Developer Program 登録・証明書・プロビジョニング
#   プロファイル取得が完了していること（本スクリプト作成時点で #12 は OPEN）。
# - Apple Watch 実機（Series 9 / Ultra 2 以降。XCFramework の実機スライスは
#   `aarch64-apple-watchos`（arm64）のみのため、arm64_32 世代（Series 8 以前）は対象外）。
# - 実機が Mac とペアリング済みの iPhone 経由で devicectl から見えていること。
#
# 使い方: scripts/apple/verify-device-launch-watchos.sh
#   環境変数 FANDHE_WATCH_DEVICE_NAME でインストール先実機の機種名（例: "Apple Watch
#   Series 10 (46mm)"。ユーザーが付けた個体名ではなく `hardwareProperties.marketingName`）
#   を指定できる（既定: `devicectl list devices` の JSON 出力から watchOS かつ
#   ペアリング済み（`pairingState == paired`）の実機を自動選択。`tunnelState`
#   （`connected` / `available` / `disconnected` 等）はインストール・起動前の時点では
#   未確立のことが多く必須条件にしない（ペアリング済みならインストール契機に
#   トンネルが確立されうるため）。ただし `connected` な実機があれば優先し、
#   同順位内では先頭を選択する。未ペアリングのデバイスのみ候補から除外する）。
#   値は実在する機種名と完全一致するもののみを許容し、
#   それ以外は検証エラーで停止する。内部的には一致したデバイスの identifier
#   （`^[0-9A-Fa-f-]{8,64}$` で検証済みのトークン）へ解決してから install/launch に渡す
#   （A03 インジェクション対策。デバイス名は空白を含みうるため名前文字列を直接
#   コマンド引数に展開せず、実在確認済みの identifier のみを展開する）。
#   環境変数 FANDHE_DEVELOPMENT_TEAM で署名チームを注入できる（未指定の場合、
#   Xcode 側に既定チームが設定済みならそれに従う。project.yml には一切書かない方針
#   — project.yml 末尾コメント参照）。
#   環境変数 FANDHE_ALLOW_PROVISIONING_UPDATES=1 を指定すると
#   `xcodebuild -allowProvisioningUpdates` を有効にし、不足しているプロビジョニング
#   プロファイルの自動作成・更新を許可する（既定は無効。`~/Library` 配下へ書き込む
#   副作用があり、ci.md の「グローバル状態を汚す処理を書かない」方針に鑑み既定オフとし、
#   明示的なオプトインを要求する）。
#
# 冪等性について: 既にインストール済みのアプリに対して再実行しても失敗しない
#   （`devicectl device install app` は上書きインストールとして働く）。
#
# 本スクリプトの標準出力末尾に §5-3 結果記録テンプレートへ転記すべき値を整形出力する。
# 進捗ログ・xcodebuild の出力は標準エラー出力へ書く（標準出力は §5-3 テンプレートと
# 関数の戻り値専用に保つ。ログとの混在を避けるため）。
#
# 秘密情報の取り扱い（security.md / apple-developer-setup.md §5 準拠）:
# - 証明書シリアル番号・署名アイデンティティ名・Team ID・デバイス個体識別子
#   （UDID・シリアル番号）は標準出力・標準エラー出力のいずれにも出力しない。
#   署名アイデンティティは `security find-identity` の「件数」のみをログへ出す。
# - `xcodebuild` の全出力（Team ID を含みうる）はログファイルへのみ書き出し、
#   標準エラーへは要点（末尾数十行）のみを表示する。ログファイルの内容は
#   Issue・PR にそのまま貼らないこと（本スクリプトの出力ではなく手順書 §5-4 に明記）。
#
# 目視確認が必要で自動化対象外の項目（手順書 §5-2-1 に残る）:
# - 表示文言がフォールバック（「画面を表示できません」）でなく
#   `fandhe_core::sample_screen()` 由来の文言であること
# - ボタン押下が Rust コア側の状態更新に反映されること
# - `WatchScreenModel.lastError == nil`（ログ確認）・クラッシュなしで起動完了すること
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
apple_dir="${repo_root}/platforms/apple"
xcodeproj_path="${apple_dir}/Fandhe.xcodeproj"
xcframework_info_plist="${apple_dir}/FandheSwiftUIBridge/Artifacts/FandheBridge.xcframework/Info.plist"
# `/target` は .gitignore 済み（Rust ビルド成果物向けだが、DerivedData も同様に
# 生成物であり追跡対象外であるべきなので流用する。新規 .gitignore エントリ不要）。
derived_data_dir="${repo_root}/target/apple-device-derived-data"
build_log_dir="${derived_data_dir}/build-logs"

scheme="FandheWatchApp"
bundle_id="com.fandhe.watchapp"

# 進捗ログ・エラーは標準エラー出力へ書く（標準出力は関数の戻り値・最終テンプレート専用）。
log() {
  echo "[verify-device-launch-watchos] $*" >&2
}

err() {
  echo "[verify-device-launch-watchos] エラー: $*" >&2
}

# --- 1. 環境検査（macOS + 必須ツール） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  err "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン・実機接続必須）。"
  echo "[verify-device-launch-watchos] 手順: macOS 上で以下を実行してください。" >&2
  echo "[verify-device-launch-watchos]   1. brew install xcodegen jq" >&2
  echo "[verify-device-launch-watchos]   2. bash scripts/apple/verify-device-launch-watchos.sh" >&2
  exit 1
fi

for tool in xcodebuild xcrun xcodegen jq security; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    err "${tool} が見つかりません。"
    echo "[verify-device-launch-watchos] 導入手順（自動インストールは行いません — ci.md 準拠）: Xcode / Xcode Command Line Tools を導入し、xcodegen・jq は 'brew install xcodegen jq' で導入してください。" >&2
    exit 1
  fi
done

mkdir -p "${build_log_dir}"

# --- 2. 署名アイデンティティの存在確認（件数のみログへ出す。A05/秘密情報対策） ----
identity_count="$(security find-identity -v -p codesigning 2>/dev/null | grep -cE '^[[:space:]]*[0-9]+\)' || true)"
if [[ "${identity_count}" -eq 0 ]]; then
  err "コード署名アイデンティティがキーチェーンに見つかりません（件数: 0）。"
  echo "[verify-device-launch-watchos] docs/environment/apple-developer-setup.md に従い Apple Development 証明書を取得・インポートしてください。" >&2
  exit 1
fi
log "コード署名アイデンティティ: ${identity_count} 件確認（詳細はログへ出力しません）。"

# --- 3. 実機の解決（devicectl の JSON 出力から identifier を取得） --------------
# `xcrun devicectl` のテキスト表は将来のフォーマット変更に弱く、また実機
# identifier は simctl の UDID とは形式が異なる（8-4-4-4-12 の保証がない）ため、
# 名前欄の素朴な文字列切り出しに頼らず `--json-output` を jq で解析する。
devicectl_json="${derived_data_dir}/devicectl-list-devices.json"
log "xcrun devicectl list devices --json-output ${devicectl_json}"
# `devicectl list devices` は `--json-output` 指定時でも人間可読の表を標準出力へ
# 出し続ける（構造化データは JSON ファイル側に書かれるため、この表は不要）。
# この表にはデバイス名・identifier（UDID 相当）が含まれるため、標準エラーへ
# リダイレクトすると秘密情報の混入禁止方針（security.md）に反する。標準出力は
# 破棄し、進捗メッセージのみ本来の標準エラーへ流す。
if ! xcrun devicectl list devices --json-output "${devicectl_json}" >/dev/null; then
  err "devicectl list devices に失敗しました。実機が Mac（または経由の iPhone）とペアリング済みか確認してください。"
  exit 1
fi

# watchOS デバイスのみを対象に、"<表示名>\t<identifier>" 形式で 1 行ずつ抽出する。
# 表示名はユーザーが付けた個体名（例:「〇〇の Apple Watch」）ではなく機種名
# （`hardwareProperties.marketingName`、例: "Apple Watch Series 10 (46mm)"）を
# 優先して使う。結果記録テンプレート（§5-3）へそのまま転記される値であり、
# 個体名が記録に残らないようにするため（個体識別子の記載禁止方針
# 方針と同じ配慮）。marketingName が取得できない場合のみ deviceProperties.name に
# フォールバックする。
# identifier は devicectl の `identifier` フィールド（UUID 形式が多いが、simctl の
# UDID と混同しないよう固定長を仮定せず `^[0-9A-Fa-f-]{8,64}$` で緩く検証する）。
# ペアリング済みの実機のみを候補とする（`connectionProperties.pairingState` が
# "paired" のもの）。`tunnelState`（"connected" / "available" / "disconnected" 等）は
# フィルタ条件にしない — ペアリング済みでも install/launch 前は "available" や
# "disconnected" のままであることが多く、これを必須にすると「ペアリング済みの
# 使用可能な実機が接続されているにもかかわらず見つからない」という誤検出で
# インストール前に中断してしまう（Issue #232 PR #493 レビュー指摘）。
# ただし tunnelState == "connected" な実機があれば install/launch の成功率が
# 高いため、同じペアリング済み実機の中では先に並べる（sort_by によるタイブレーク。
# 除外はしない）。devicectl の一覧にはオフライン・未ペアリングの watch も
# 含まれうるため、未ペアリングのみを候補から除外する。
watch_devices="$(jq -r '
  [
    .result.devices[]?
    | select(.deviceProperties.platform // .hardwareProperties.platform // "" | test("watch"; "i"))
    | select(((.connectionProperties.pairingState // "") | ascii_downcase) == "paired")
  ]
  | sort_by(if ((.connectionProperties.tunnelState // "") | ascii_downcase) == "connected" then 0 else 1 end)
  | .[]
  | [(.hardwareProperties.marketingName // .deviceProperties.name), .identifier] | @tsv
' "${devicectl_json}" 2>/dev/null || true)"

if [[ -z "${watch_devices}" ]]; then
  err "ペアリング済みの watchOS 実機が見つかりません（devicectl list devices の結果に該当なし。未ペアリングのデバイスは候補から除外しています）。"
  echo "[verify-device-launch-watchos] 実機を Mac にペアリング済みの iPhone 経由で接続し、信頼設定を済ませてください。Apple Watch アプリで「このデバイスとの接続」が確立されているかも確認してください。" >&2
  exit 1
fi

override_value="${FANDHE_WATCH_DEVICE_NAME:-}"
matched_line=""
if [[ -n "${override_value}" ]]; then
  matched_line="$(echo "${watch_devices}" | while IFS=$'\t' read -r name identifier; do
    if [[ "${name}" == "${override_value}" ]]; then
      printf '%s\t%s\n' "${name}" "${identifier}"
      break
    fi
  done)"
  if [[ -z "${matched_line}" ]]; then
    err "FANDHE_WATCH_DEVICE_NAME=${override_value} は検出済みの watchOS 実機に見つかりません（完全一致のみ許容）。"
    echo "[verify-device-launch-watchos] 検出済みデバイス一覧:" >&2
    echo "${watch_devices}" | cut -f1 >&2
    exit 1
  fi
else
  matched_line="$(echo "${watch_devices}" | head -n 1)"
fi

device_name="$(echo "${matched_line}" | cut -f1)"
device_identifier="$(echo "${matched_line}" | cut -f2)"

# A03 対策: identifier を保守的な文字集合で検証してからのみ後続コマンドへ渡す。
if [[ ! "${device_identifier}" =~ ^[0-9A-Fa-f-]{8,64}$ ]]; then
  err "デバイス identifier の形式が想定外です（devicectl の出力形式が変わった可能性があります。identifier 自体は個体識別子のためログへ出しません）。"
  exit 1
fi
log "watchOS 実機: ${device_name}（identifier は個体識別子のためログへ出しません）"

# install/launch 失敗時のログ抜粋（tail）には devicectl が identifier（UDID 相当）を
# 含めて出力することがあるため、表示前に必ず本関数を通して redact する
# （security.md: 標準出力・標準エラーいずれにも UDID 相当の識別子を出力しない）。
redact_device_identifiers() {
  sed -E "s/${device_identifier}/<device-identifier-redacted>/g; s/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/<uuid-redacted>/g"
}

# --- 4. 初回制約への対応（XCFramework 先行生成） ----------------------------
# `build-bridge-xcframework.sh` の既定ターゲットには実機向け `aarch64-apple-watchos`
# （slice: watchos-arm64）が含まれるため、追加の環境変数指定は不要。
if [[ ! -f "${xcframework_info_plist}" ]]; then
  log "Artifacts/FandheBridge.xcframework が未生成のため先行生成します（実機スライス aarch64-apple-watchos を含む）。"
  bash "${script_dir}/build-bridge-xcframework.sh" 1>&2
else
  log "Artifacts/FandheBridge.xcframework は生成済みです（先行生成をスキップ）。実機スライスが含まれているか不明な場合は削除して再生成してください。"
fi

# --- 5. .xcodeproj 生成 ------------------------------------------------------
log "generate-xcodeproj.sh を実行します。"
bash "${script_dir}/generate-xcodeproj.sh" 1>&2

# --- 6. 実機向け署名付きビルド ------------------------------------------------
# DEVELOPMENT_TEAM は project.yml に一切書かず、環境変数からのみ注入する
# （project.yml 末尾コメント・security.md 準拠）。xcodebuild の全出力（Team ID を
# 含みうる）はログファイルへのみ書き、標準エラーへは要点のみを表示する。
log "xcodebuild build -scheme ${scheme} -destination 'generic/platform=watchOS'"
build_log_file="${build_log_dir}/${scheme}-device.log"

xcodebuild_args=(
  build
  -project "${xcodeproj_path}"
  -scheme "${scheme}"
  -destination "generic/platform=watchOS"
  -derivedDataPath "${derived_data_dir}"
)
if [[ -n "${FANDHE_DEVELOPMENT_TEAM:-}" ]]; then
  xcodebuild_args+=("DEVELOPMENT_TEAM=${FANDHE_DEVELOPMENT_TEAM}")
  log "DEVELOPMENT_TEAM を環境変数から注入します（値はログへ出しません）。"
else
  log "FANDHE_DEVELOPMENT_TEAM 未指定。Xcode 側の既定チーム設定に従います（未設定の場合は署名エラーになります）。"
fi
if [[ "${FANDHE_ALLOW_PROVISIONING_UPDATES:-0}" == "1" ]]; then
  xcodebuild_args+=(-allowProvisioningUpdates)
  log "FANDHE_ALLOW_PROVISIONING_UPDATES=1 のため -allowProvisioningUpdates を有効化します（~/Library への書き込みを伴います）。"
fi

if ! xcodebuild "${xcodebuild_args[@]}" >"${build_log_file}" 2>&1; then
  err "xcodebuild build に失敗しました（${scheme}）。詳細ログ: ${build_log_file}"
  echo "[verify-device-launch-watchos] 注意: 上記ログファイルには DEVELOPMENT_TEAM 等の署名情報が含まれる場合があります。Issue・PR には貼らず、要点のみ共有してください。" >&2
  tail -n 40 "${build_log_file}" >&2
  exit 1
fi

app_path="$(find "${derived_data_dir}/Build/Products" -maxdepth 2 -type d -name "${scheme}.app" | head -n 1)"
if [[ -z "${app_path}" ]]; then
  err "${scheme}.app のビルド成果物が見つかりません（${derived_data_dir}/Build/Products 配下）。詳細ログ: ${build_log_file}"
  exit 1
fi
log "ビルド成功: ${app_path}"

# --- 7. 実機へのインストール・起動 --------------------------------------------
# identifier のみを devicectl へ渡す（A03 対策。§3 で実在確認済みの identifier）。
log "xcrun devicectl device install app --device <identifier> ${app_path}"
install_log_file="${build_log_dir}/${scheme}-install.log"
if ! xcrun devicectl device install app --device "${device_identifier}" "${app_path}" >"${install_log_file}" 2>&1; then
  err "devicectl device install app に失敗しました。詳細ログ: ${install_log_file}"
  tail -n 40 "${install_log_file}" | redact_device_identifiers >&2
  exit 1
fi
log "インストール成功。"

log "xcrun devicectl device process launch --device <identifier> ${bundle_id}"
launch_log_file="${build_log_dir}/${scheme}-launch.log"
if ! xcrun devicectl device process launch --terminate-existing --device "${device_identifier}" "${bundle_id}" >"${launch_log_file}" 2>&1; then
  err "devicectl device process launch に失敗しました。詳細ログ: ${launch_log_file}"
  echo "[verify-device-launch-watchos] devicectl 経路が使えない場合は手順書の Xcode GUI（Run ボタン）フォールバック手順を使用してください。" >&2
  tail -n 40 "${launch_log_file}" | redact_device_identifiers >&2
  exit 1
fi
log "起動コマンド発行成功（クラッシュせず起動しているかは実機の目視確認が必要です）。"

# --- 8. §5-3 結果記録テンプレート向けの整形出力（標準出力） -------------------
log "完了。以下の結果記録テンプレートを控えてください。"
cat <<EOF

--- watchOS 実機起動 ---
デバイス機種: ${device_name}
ビルド構成: Debug（xcodebuild 既定）
インストール結果: 成功
起動コマンド発行結果: 成功（devicectl の終了コードで判定。クラッシュ有無は目視確認が必要）

--- 目視確認（自動化対象外・手動で確認のうえ記入） ---
表示文言（フォールバックでないか）:
lastError（watchOS）:
クラッシュの有無:
スクリーンショット:
合否:
EOF
