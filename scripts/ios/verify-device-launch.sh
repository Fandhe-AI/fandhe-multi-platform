#!/usr/bin/env bash
# TASK-1.4 系（REQ-1、Issue #567、親: #565）: iPhone 実機（無料 Apple ID の
# Personal Team・free provisioning 前提）への fandhe アプリのビルド・実署名・
# インストール・起動確認を行う。計測（コールドスタート・フレームレート等）は
# 一切含まない（それらは scripts/ios/measure-cold-start.sh 等、有償 Apple Developer
# Program（Issue #12）完了後のスコープ）。
#
# 位置づけ・重複回避:
# - scripts/ios/measure-cold-start.sh のフェーズ 1〜3（実機接続確認・cargo build →
#   .app 手組み → .mobileprovision 埋め込み → entitlements 抽出 → codesign 実署名）が
#   前例。本スクリプトは同じ手順を「計測なしの起動確認専用」として独立実装する
#   （measure-cold-start.sh は Xcode 16 必須の --console 計測を含み目的が異なるため、
#   無理に共通化しない）。
# - scripts/ios/build-simulator-app.sh・verify-simulator-render.sh のフェーズ分割・
#   決定的パス受け渡し・進捗ログは標準エラー出力／結果は標準出力という規約を踏襲する。
# - scripts/android/verify-android-launch.sh（Issue #566）と同じ「実機起動確認」タスクの
#   姉妹実装。ただし粒度は完全には揃っていない。本スクリプトが行うのは
#   前提検査 → 接続確認 → プロファイル確保 → ビルド → 実署名 → インストール → 起動コマンド
#   の送出までで、起動後の判定は人間による目視確認に委ねる。Android 側が持つ
#   logcat によるクラッシュ検出・adb screencap によるスクリーンショット証跡取得に
#   相当する機械的判定は、iOS では devicectl に同等機能がなく別ツール
#   （libimobiledevice 等）の追加依存が必要になるため実装していない。
#   また後片付け（uninstall）は既定では行わず、目視確認のためアプリを端末に残す
#   （FANDHE_IOS_UNINSTALL_AFTER=1 指定時のみ uninstall する）。
#
# 無料 Apple ID（Personal Team）はプロビジョニングプロファイルを wildcard 発行できないため、
# platforms/apple/project.yml の iOS ホストターゲット FandheIosApp（bundle id
# com.fandhe.iosapp、CODE_SIGN_STYLE: Automatic）を Xcode の
# `-allowProvisioningUpdates` でビルドさせ、Personal Team 向けのプロファイルを
# 自動生成させたうえで、生成された .mobileprovision を実配布対象の Rust/winit 製
# FandheIosApp（fandhe-egui-proxy、こちらは Xcode ターゲット化されていない手組みバンドル）
# へ流用する。
#
# platforms/apple/project.yml には iOS ホストターゲット FandheIosApp（bundle id
# com.fandhe.iosapp）を本 PR で追加済み。project.yml が未反映のまま古い
# Fandhe.xcodeproj を使っている環境では、フェーズ 1（profile）で明確なエラーにより
# 停止する（後述）。
#
# 環境変数インターフェース:
#   FANDHE_IOS_DEVICE_UDID        既定: 空（`xcrun devicectl list devices` から接続中の
#                                  iOS 実機を自動解決。複数台接続時は候補を列挙して停止）。
#                                  値・候補一覧はログへそのまま出力しない
#                                  （末尾数桁のマスク表示のみ、security.md 準拠）。
#   FANDHE_DEVELOPMENT_TEAM       既定: 空（未指定時は `security find-identity -v -p
#                                  codesigning` の証明書から Team ID を自動解決する。
#                                  scripts/apple/verify-device-launch-watchos.sh・
#                                  verify-visionos-device-launch.sh と同じ命名。曖昧な
#                                  場合は候補件数のみ報告し停止する。値そのものは
#                                  ログへ出力しない）。
#   FANDHE_IOS_PROVISIONING_PROFILE 既定: 空（未指定時は上記の自動生成・自動解決を行う）。
#                                  指定時は、自動選定経路と同じ 4 条件（bundle id・
#                                  development team・対象実機・有効期限）を検証した
#                                  うえで採用する（Issue #567 5 巡目レビュー指摘）。
#                                  この検証を迂回する手段は用意しない（6 巡目レビュー
#                                  指摘: fail-open な迂回経路は署名前の必須検証を無効化
#                                  するため fail-closed のみを許容する）。検証不能・
#                                  不一致の場合は原因別のエラーメッセージ（no-bundle-match /
#                                  team-mismatch / device-not-provisioned / expired）で
#                                  対処方法を案内したうえで停止する。
#   FANDHE_IOS_CODESIGN_IDENTITY  既定: 空（未指定時はプロファイルの証明書と
#                                  `security find-identity -v -p codesigning` を SHA1
#                                  ハッシュで突き合わせ自動解決。曖昧な場合は候補数のみ
#                                  報告し、手動確認コマンドを案内して停止する）。
#   FANDHE_IOS_UNINSTALL_AFTER    既定: 空（起動確認後もアプリを端末に残し、目視確認できる
#                                  ようにする）。値 "1" を指定した場合のみ起動確認後に
#                                  uninstall する。
#   FANDHE_IOS_DESTINATION_TIMEOUT xcodebuild -destination-timeout に渡す秒数（既定: 120）。
#                                  無線（localNetwork）接続端末では DDI（Developer Disk
#                                  Image）マウントに時間がかかるため、実測で 15 秒では
#                                  不足することを確認済み（Issue #567 実機検証）。
#   FANDHE_ITEMS_URL              既定: 空。指定時は `http_fetch` 型サンプルの取得先を
#                                  ビルド時に焼き込み（`fandhe-egui-proxy` の
#                                  `option_env!("FANDHE_ITEMS_URL")`）、`.app` の
#                                  Info.plist に LAN 限定の ATS 例外
#                                  （NSAllowsLocalNetworking）と
#                                  NSLocalNetworkUsageDescription を差し込む
#                                  （Issue #660）。ストア提出パイプライン
#                                  （scripts/build/）はこの経路を通らない。
#                                  ビルド後は unset すること。
#

# 使い方:
#   scripts/ios/verify-device-launch.sh                # 全フェーズを順に実行
#   scripts/ios/verify-device-launch.sh --phase <name>  # フェーズ単体実行
#     <name>: profile | build | bundle | sign | install | launch
#   scripts/ios/verify-device-launch.sh --help
#
# フェーズ間の状態受け渡し: profile フェーズが選定した .mobileprovision のパスを
# target/ios-device-launch/ 配下の状態ファイルへ書き出し、bundle フェーズがそれを読んで
# アプリバンドルへ embedded.mobileprovision として同梱する。sign フェーズは
# embedded.mobileprovision（bundle 済みの .app 内）から entitlements を抽出するため、
# profile フェーズの選定結果を再解決する必要がない（build-simulator-app.sh と同じ
# 「決定的パス経由でフェーズを疎結合にする」設計）。
set -euo pipefail

unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

log() {
  echo "[verify-device-launch] $*" >&2
}

fail() {
  echo "[verify-device-launch] エラー: $*" >&2
  exit 1
}

# UDID・SHA1 等の識別子は末尾数桁のみを表示する（security.md 準拠）。
mask_tail() {
  local value="$1"
  local len="${#value}"
  if [[ "${len}" -le 4 ]]; then
    printf '****\n'
  else
    printf '...%s\n' "${value: -4}"
  fi
}

# --- フェーズ 0: 環境検査（macOS 限定） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン・xcrun devicectl 必須）。
  手順:
  1. macOS + Xcode 環境で本スクリプトを実行する
  2. 無料 Apple ID で Xcode にサインインしておく（Xcode > Settings > Accounts）
  3. iPhone を USB/Wi-Fi で接続し、端末を信頼済みにする
  4. bash scripts/ios/verify-device-launch.sh を再実行する"
fi

for tool in xcrun cargo rustup security xcodebuild python3; do
  command -v "${tool}" >/dev/null 2>&1 || fail "${tool} が見つかりません（Xcode Command Line Tools・Rust ツールチェーン導入を確認してください）。"
done
[[ -x /usr/libexec/PlistBuddy ]] || fail "/usr/libexec/PlistBuddy が見つかりません（macOS 標準搭載のはずです）。"

devicectl_help="$(xcrun devicectl device install app --help 2>&1 || true)"
printf '%s\n' "${devicectl_help}" | grep -q -- "--device" \
  || fail "xcrun devicectl が利用できません（Xcode 15 以上が必要です）。Xcode のバージョンを確認してください。"

out_dir="${repo_root}/target/ios-device-launch"
mkdir -p "${out_dir}"

app_name="FandheIosApp"
bundle_id="com.fandhe.iosapp"
fandhe_target="aarch64-apple-ios"
fandhe_bin_name="fandhe-egui-proxy-desktop"
info_plist_src="${repo_root}/platforms/apple/FandheIosApp/Resources/Info.plist"
[[ -f "${info_plist_src}" ]] || fail "Info.plist が見つかりません: ${info_plist_src}"

app_dir="${out_dir}/${app_name}.app"
profile_state_file="${out_dir}/resolved-provisioning-profile-path"

# --- デバイス UDID 解決 ------------------------------------------------------
# `xcrun devicectl list devices` のテキスト表は "unavailable" が正規表現
# `connected|available` に部分一致してしまい、利用不能な端末を利用可能と誤判定する
# バグが実機で再現した（Issue #567 7 巡目レビュー指摘 M1。例:
# "unavailable" 状態の端末が `grep -Ei 'connected|available'` に一致する）。
# `--json-output` + python3 で構造的に判定する（他の JSON 解決関数と同方針）。
# 実機の `connectionProperties`（実データで確認済み）:
#   - pairingState: "paired" であること（未ペアリングは対象外）
#   - tunnelState: "unavailable" の場合は到達不能（除外）。"disconnected"
#     （通常の待機状態。devicectl が install/launch 時に必要に応じてトンネルを
#     再確立する）・"connected"（現在接続中）はいずれも利用可能として候補に含める。
#     "connected" は優先的にソートする（同じペアリング済み候補内でのタイブレークのみ。
#     除外条件にはしない。scripts/apple/verify-device-launch-watchos.sh と同方針）。
#   - hardwareProperties.platform == "iOS" で iPhone 以外（ペアリング済み Watch 等）を除外
# A03 対策の核。以降 install/launch には UDID のみを渡す。値・候補名はログへ
# 出力しない（候補列挙時もモデル名のみ表示し、個体名は出力しない。S6 参照）。
resolve_device_udid() {
  local override="${FANDHE_IOS_DEVICE_UDID:-}"
  if [[ -n "${override}" ]]; then
    if [[ ! "${override}" =~ ^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$ ]]; then
      fail "FANDHE_IOS_DEVICE_UDID の形式が不正です（UUID 形式 8-4-4-4-12 のみ許容）。"
    fi
    printf '%s\n' "${override}"
    return 0
  fi

  local json_file
  json_file="$(mktemp)"
  if ! xcrun devicectl list devices --json-output "${json_file}" >/dev/null 2>&1; then
    rm -f "${json_file}"
    fail "xcrun devicectl list devices に失敗しました。実機接続・端末の信頼済み状態を確認してください（人間担当手順）。"
  fi

  # 出力（1 行 1 候補）: "<connected 優先度: 0 か 1>\t<UDID>\t<モデル名 (productType)>"
  local candidates
  candidates="$(python3 - "${json_file}" <<'PYEOF' 2>/dev/null
import json
import sys

with open(sys.argv[1]) as f:
    data = json.load(f)

for device in data.get("result", {}).get("devices", []):
    hw = device.get("hardwareProperties", {}) or {}
    if (hw.get("platform") or "") != "iOS":
        continue
    conn = device.get("connectionProperties", {}) or {}
    if (conn.get("pairingState") or "") != "paired":
        continue
    tunnel_state = conn.get("tunnelState") or "unavailable"
    if tunnel_state == "unavailable":
        continue
    identifier = device.get("identifier") or ""
    if not identifier:
        continue
    model = hw.get("marketingName") or "unknown"
    product_type = hw.get("productType") or "unknown"
    priority = 0 if tunnel_state == "connected" else 1
    print(f"{priority}\t{identifier}\t{model} ({product_type})")
PYEOF
  )"
  rm -f "${json_file}"

  if [[ -z "${candidates}" ]]; then
    fail "接続中の iOS 実機が見つかりません。USB/Wi-Fi 接続・端末の信頼済み状態を確認してください（人間担当手順）。"
  fi

  local sorted
  sorted="$(printf '%s\n' "${candidates}" | sort -t $'\t' -k1,1)"

  local count
  count="$(printf '%s\n' "${sorted}" | grep -c . || true)"
  if [[ "${count}" -gt 1 ]]; then
    log "複数の利用可能な iOS 実機が見つかりました。FANDHE_IOS_DEVICE_UDID で対象を指定してください。候補（機種名のみ表示。個体名は出力しません）:"
    while IFS=$'\t' read -r _priority udid model; do
      log "  - ${model}（UDID 末尾: $(mask_tail "${udid}")）"
    done <<<"${sorted}"
    exit 1
  fi

  printf '%s\n' "${sorted}" | cut -f2
}

# codesign 用の署名 ID（SHA1）集合に対応する証明書の development team（Team ID）を
# 集める（resolve_development_team 専用ヘルパー）。`security find-identity -v -p
# codesigning` の CN 末尾の括弧書き（例: "(<証明書 ID>)"）は証明書 ID であって
# Team ID ではないため使わない（Issue #567 実機検証で判明。CN 末尾を Team ID として
# 誤採用すると、後続のプロファイル検証が team-mismatch で必ず失敗する）。Team ID は
# 証明書 subject の OU フィールドに入っているため、`security find-certificate -a -Z -p`
# で全証明書の SHA-1 hash + PEM を取得し、対象 SHA1 に一致する PEM だけを
# `openssl x509 -noout -subject` に通して OU を抜き出す。証明書名での部分一致検索
# （find-certificate -c "<name>"）は同名証明書で誤って別証明書を拾うため使わない。
# 値（Team ID・証明書内容）はログへ出力しない。
resolve_team_ids_for_sha1s() {
  local target_sha1s_file="$1"
  local certs_file
  certs_file="$(mktemp)"
  security find-certificate -a -Z -p >"${certs_file}" 2>/dev/null || true

  python3 - "${certs_file}" "${target_sha1s_file}" <<'PYEOF' 2>/dev/null || true
import re
import subprocess
import sys

certs_path, targets_path = sys.argv[1], sys.argv[2]

with open(targets_path) as f:
    targets = {line.strip().upper() for line in f if line.strip()}

with open(certs_path) as f:
    text = f.read()

team_ids = set()
for block in re.split(r"(?=SHA-1 hash: )", text):
    m = re.match(r"SHA-1 hash: ([0-9A-Fa-f]{40})", block)
    if not m or m.group(1).upper() not in targets:
        continue
    cert_m = re.search(
        r"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----", block, re.S
    )
    if not cert_m:
        continue
    result = subprocess.run(
        ["openssl", "x509", "-noout", "-subject", "-nameopt", "sep_multiline"],
        input=cert_m.group(0),
        capture_output=True,
        text=True,
    )
    for line in result.stdout.splitlines():
        line = line.strip()
        if line.startswith("OU="):
            team_ids.add(line[len("OU=") :])

for team_id in sorted(team_ids):
    print(team_id)
PYEOF

  rm -f "${certs_file}"
}

# --- development team（Team ID）解決 -----------------------------------------
# FANDHE_DEVELOPMENT_TEAM が既に注入されていればそれを使う（scripts/apple/
# verify-device-launch-watchos.sh・verify-visionos-device-launch.sh と同じ命名。
# project.yml には書かず環境変数からのみ注入する方針）。未指定時は
# `security find-identity -v -p codesigning` の有効な署名 ID（SHA1）を集め、
# resolve_team_ids_for_sha1s で証明書 subject の OU から Team ID を求め、
# 一意に決まる場合のみ採用する。値そのものはログへ出力しない。
resolve_development_team() {
  local override="${FANDHE_DEVELOPMENT_TEAM:-}"
  if [[ -n "${override}" ]]; then
    printf '%s\n' "${override}"
    return 0
  fi

  local find_identity_output
  find_identity_output="$(security find-identity -v -p codesigning 2>/dev/null || true)"
  local identity_sha1s
  identity_sha1s="$(printf '%s\n' "${find_identity_output}" | grep -oE '[0-9A-Fa-f]{40}' | tr '[:lower:]' '[:upper:]' | sort -u || true)"

  if [[ -z "${identity_sha1s}" ]]; then
    fail "codesign 用の証明書が見つからず、development team を自動解決できませんでした。
  Xcode に無料 Apple ID がサインインされているか確認するか、
  FANDHE_DEVELOPMENT_TEAM を明示指定してください。"
  fi

  local sha1s_file
  sha1s_file="$(mktemp)"
  printf '%s\n' "${identity_sha1s}" >"${sha1s_file}"
  local team_ids
  team_ids="$(resolve_team_ids_for_sha1s "${sha1s_file}")"
  rm -f "${sha1s_file}"

  local count
  count="$(printf '%s\n' "${team_ids}" | grep -c . || true)"
  if [[ "${count}" -eq 0 ]]; then
    fail "codesign 用の証明書から development team（証明書 subject の OU）を取得できませんでした。
  FANDHE_DEVELOPMENT_TEAM を明示指定してください。"
  fi
  if [[ "${count}" -gt 1 ]]; then
    fail "development team の候補が ${count} 件あるため一意に決まりません。
  FANDHE_DEVELOPMENT_TEAM を明示指定してください。候補の確認は
  \`security find-identity -v -p codesigning\` と \`security find-certificate -a -Z -p\`
  の証明書 subject（OU）で行えます。"
  fi

  printf '%s\n' "${team_ids}"
}

# --- 端末 OS バージョン解決（DDI 不整合の事前情報用） -------------------------
# 受け取る udid（CoreDevice 識別子）で `xcrun devicectl device info details
# --device <udid>` を直接呼ぶため、他関数のような「一覧から identifier で
# 引き当てる」処理自体が無く、hardwareProperties.udid との取り違えは発生しない
# （Issue #567 3 巡目レビューで確認済み。実機でも 26.6.1 と正しく表示された）。
# JSON 出力（--json-output のみが公式にサポートされたスクリプト用インターフェース）
# から osVersionNumber を再帰的に探して返す。取得に失敗しても致命的にせず空文字を
# 返す（DDI マウント失敗時のガイダンス表示を補強する目的の best-effort 情報のため）。
resolve_device_os_version() {
  local udid="$1"
  local json_file
  json_file="$(mktemp)"
  if ! xcrun devicectl device info details --device "${udid}" --json-output "${json_file}" >/dev/null 2>&1; then
    rm -f "${json_file}"
    return 0
  fi
  python3 - "${json_file}" <<'PYEOF' 2>/dev/null
import json
import sys


def find(obj, key):
    if isinstance(obj, dict):
        if key in obj:
            return obj[key]
        for value in obj.values():
            found = find(value, key)
            if found is not None:
                return found
    elif isinstance(obj, list):
        for item in obj:
            found = find(item, key)
            if found is not None:
                return found
    return None


with open(sys.argv[1]) as f:
    data = json.load(f)

version = find(data, "osVersionNumber")
if version is not None:
    print(version)
PYEOF
  rm -f "${json_file}"
}

# --- 端末の接続形態解決（無線 vs USB。DDI マウント失敗時のガイダンス用） ---------
# 受け取る udid は resolve_device_udid が返す CoreDevice 識別子（トップレベル
# "identifier"）。`xcrun devicectl list devices --json-output` の各要素も
# トップレベル "identifier" で対象端末を選び、connectionProperties.transportType
# を返す（"localNetwork" | "wired" 等）。以前は hardwareProperties.udid と比較して
# おり、resolve_device_udid が返す identifier とは別物のため常に不一致となる
# バグがあった（実機で「接続形態: 不明」と誤表示されることを確認、Issue #567
# 3 巡目レビュー）。取得失敗時は空文字を返し best-effort に留める
# （resolve_device_os_version と同方針）。
resolve_device_transport_type() {
  local udid="$1"
  local json_file
  json_file="$(mktemp)"
  if ! xcrun devicectl list devices --json-output "${json_file}" >/dev/null 2>&1; then
    rm -f "${json_file}"
    return 0
  fi
  python3 - "${json_file}" "${udid}" <<'PYEOF' 2>/dev/null
import json
import sys

with open(sys.argv[1]) as f:
    data = json.load(f)
target_udid = sys.argv[2]

for device in data.get("result", {}).get("devices", []):
    if device.get("identifier") == target_udid:
        transport = device.get("connectionProperties", {}).get("transportType")
        if transport:
            print(transport)
        break
PYEOF
  rm -f "${json_file}"
}

# --- 実機のハードウェア UDID 解決（ProvisionedDevices 照合用） -----------------
# 受け取る coredevice_udid は resolve_device_udid と同じ CoreDevice 識別子
# （トップレベル "identifier"）。xcrun devicectl list devices --json-output の
# 各要素をトップレベル "identifier" で引き当て、.mobileprovision の
# ProvisionedDevices に入る「ハードウェア UDID」（hardwareProperties.udid、
# xcodebuild が使う値）を返す（この関数はもとから identifier で正しく比較できて
# いた。resolve_device_transport_type の取り違えとは別物、Issue #567 3 巡目
# レビューで確認済み）。取得失敗時は呼び出し元が fail する（プロファイル選定の
# 必須条件のため best-effort にはしない）。
resolve_device_hardware_udid() {
  local coredevice_udid="$1"
  local json_file
  json_file="$(mktemp)"
  if ! xcrun devicectl list devices --json-output "${json_file}" >/dev/null 2>&1; then
    rm -f "${json_file}"
    return 1
  fi
  local hw_udid
  hw_udid="$(python3 - "${json_file}" "${coredevice_udid}" <<'PYEOF' 2>/dev/null
import json
import sys

with open(sys.argv[1]) as f:
    data = json.load(f)
target = sys.argv[2]

for device in data.get("result", {}).get("devices", []):
    if device.get("identifier") == target:
        udid = device.get("hardwareProperties", {}).get("udid")
        if udid:
            print(udid)
        break
PYEOF
  )"
  rm -f "${json_file}"
  [[ -n "${hw_udid}" ]] || return 1
  printf '%s\n' "${hw_udid}"
}

# xcodebuild のビルドログ（target/ios-device-launch/xcodebuild-profile.log へ保存する）
# は保存する時点でマスクを通す（scripts/apple/verify-visionos-device-launch.sh の
# mask_secrets と同方針だが、あちらはログファイル自体は未加工のまま保存し stderr 表示
# 時のみマスクする設計であるのに対し、本スクリプトはファイル本体を確実にマスク済みに
# する。ログファイルを人間が Issue・PR に共有する運用のため security.md「ログ・エラー
# メッセージにシークレットを出力しない」を保存物にも適用する）。
# 渡された各値（Team ID・CoreDevice UDID・ハードウェア UDID・署名 ID・プロファイル
# パス等）をリテラル置換した上で、UUID 形式トークン・`DEVELOPMENT_TEAM = <値>`
# 形式のビルド設定行・`Signing Identity` 行（証明書の氏名・メールアドレスを含み得る）・
# 40 桁の 16 進（証明書 SHA1）・$HOME（ユーザー固有パス）も汎用パターンで捕捉する。
#
# 【注意】このマスクは既知パターンの伏せ字化に留まり、完全性を保証しない
# （プロビジョニングプロファイル名等、想定外の形式で氏名・識別情報が漏れる余地は
# 残る）。ログを Issue・PR へ共有する前に必ず内容を目視確認すること
# （Issue #567 3 巡目レビュー・security.md 準拠）。
mask_secrets() {
  local input
  input="$(cat)"
  local value
  for value in "$@" "${HOME}"; do
    if [[ -n "${value}" ]]; then
      # value 未クォートだとパターン置換（${var//pattern/repl}）の pattern 側が
      # グロブとして解釈される（例: value に "*" を含む場合に意図せず広く一致する）。
      # クォートしてリテラル置換に固定する（Issue #567 7 巡目レビュー指摘 N3）。
      input="${input//"${value}"/<REDACTED>}"
    fi
  done
  printf '%s\n' "${input}" \
    | sed -E 's/[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}/<REDACTED-UDID>/g' \
    | sed -E 's/(DEVELOPMENT_TEAM[[:space:]]*[:=][[:space:]]*)[A-Za-z0-9]+/\1<REDACTED-TEAM>/g' \
    | sed -E 's/(Signing Identity[[:space:]]*[:=]?[[:space:]]*).*/\1<REDACTED-IDENTITY>/g' \
    | sed -E 's/[0-9A-Fa-f]{40}/<REDACTED-SHA1>/g'
}

# --- プロビジョニングプロファイルの検証・選定（共通処理） --------------------
# 「bundle id 一致 / development team 一致 / 対象実機が ProvisionedDevices に
# 含まれる / 有効期限内」の 4 条件を、自動選定経路（mode=select、
# 存在する複数の profiles_dir を横断して候補を 1 件選ぶ）と明示指定経路
# （mode=check、FANDHE_IOS_PROVISIONING_PROFILE で与えられた 1 件を検証する）の
# 両方から同じ判定ロジックで検証する（Issue #567 5 巡目レビュー指摘: 明示指定経路が
# 検証を迂回していた問題への対応。二重実装を避けるため evaluate() を共有する）。
# select: 渡された全ディレクトリ（新パス・旧パスのうち存在する分。呼び出し元
#   phase_profile が解決する）を横断して候補を集め、条件を満たす中で有効期限が
#   最も新しいものを選ぶ（Issue #567 10 巡目レビュー指摘 P1: 新パスが存在する場合に
#   旧パス側の有効なプロファイルを一切探索しないバグへの対応）。1 件も無ければ、
#   全候補が共通してブロックされた最初の段階（bundle → team → device → 期限）を
#   理由として報告する（元の逐次フィルタ実装と同じ優先順位）。
# check: 指定 1 件が 4 条件を満たすか検証し、満たさなければ最初に外れた段階を
#   理由として報告する。
select_or_check_profile() {
  python3 - "$@" <<'PYEOF'
import datetime
import plistlib
import subprocess
import sys
from pathlib import Path

STAGE_RANK = {
    "no-bundle-match": 0,
    "team-mismatch": 1,
    "device-not-provisioned": 2,
    "expired": 3,
    "ok": 4,
}


def evaluate(path, bundle_id, team_id, hardware_udid, now):
    """1 件のプロファイルを検証し、(reason, expiry_or_none) を返す。
    reason は STAGE_RANK のキーのいずれか（"ok" は成功）。
    読み取り不能な場合は "no-bundle-match" 扱いとする（存在しない候補として除外する）。
    """
    try:
        decoded = subprocess.run(
            ["security", "cms", "-D", "-i", str(path)],
            check=True, capture_output=True,
        ).stdout
        profile = plistlib.loads(decoded)
    except Exception:
        return "no-bundle-match", None

    # Team prefix と App ID を "." で分離し、App ID 部分の完全一致で判定する
    # （末尾一致だと "<Team>.other.com.fandhe.iosapp" のような別アプリ用
    # プロファイルも通過してしまう誤りが実証された。Issue #567 7 巡目レビュー
    # 指摘 S1）。Personal Team は wildcard（"TEAM.*"）プロファイルを発行できない
    # 前提のため、完全一致に限定しても運用上の問題はない（スクリプト冒頭コメント
    # 参照）。
    app_id = profile.get("Entitlements", {}).get("application-identifier", "")
    if "." not in app_id:
        return "no-bundle-match", None
    app_id_prefix, app_id_suffix = app_id.split(".", 1)
    if app_id_suffix != bundle_id:
        return "no-bundle-match", None

    # development team の検証は「どちらか一方が一致すれば可」にしない（fail-closed）。
    # com.apple.developer.team-identifier が存在する場合は指定 Team との完全一致を必須と
    # し、ApplicationIdentifierPrefix は application-identifier の prefix と整合するかを
    # 独立に検証する。or 条件にすると、Entitlements が別 Team を指す不整合なプロファイル
    # でも prefix 側の一致だけで通過してしまう。
    entitlement_team = profile.get("Entitlements", {}).get("com.apple.developer.team-identifier", "")
    prefixes = [str(x) for x in (profile.get("ApplicationIdentifierPrefix", []) or [])]
    if entitlement_team:
        if entitlement_team != team_id:
            return "team-mismatch", None
    elif team_id not in prefixes:
        # team-identifier を持たない古い形式のプロファイルのみ prefix へフォールバックする。
        return "team-mismatch", None
    # application-identifier の prefix 部（app_id_prefix）は Team ID と一致するはず。
    # ApplicationIdentifierPrefix が申告されている場合はそこにも含まれることを確認する。
    if app_id_prefix != team_id:
        return "team-mismatch", None
    if prefixes and app_id_prefix not in prefixes:
        return "team-mismatch", None

    provisioned = {str(u).upper() for u in profile.get("ProvisionedDevices", []) or []}
    if hardware_udid.upper() not in provisioned:
        return "device-not-provisioned", None

    expiry = profile.get("ExpirationDate")
    if not isinstance(expiry, datetime.datetime) or expiry <= now:
        return "expired", None

    return "ok", expiry


mode = sys.argv[1]
now = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)

if mode == "select":
    # 新パス（Xcode 16 以降）・旧パスのうち存在する分だけが呼び出し元から渡される
    # （1 件以上。0 件になる場合は呼び出し元が事前に fail する）。「最も新しい有効期限」の
    # 選定基準は渡された全ディレクトリを横断して適用する（Issue #567 10 巡目レビュー
    # 指摘 P1: 新パス存在時に旧パス側の有効なプロファイルを見落とすバグへの対応）。
    bundle_id, team_id, hardware_udid = sys.argv[2], sys.argv[3], sys.argv[4]
    profiles_dirs = sys.argv[5:]
    best_path = None
    best_expiry = None
    reasons = set()

    for profiles_dir in profiles_dirs:
        for path in Path(profiles_dir).glob("*.mobileprovision"):
            reason, expiry = evaluate(path, bundle_id, team_id, hardware_udid, now)
            if reason != "ok":
                reasons.add(reason)
                continue
            if best_expiry is None or expiry > best_expiry:
                best_expiry = expiry
                best_path = str(path)

    if best_path is None:
        passed_bundle = any(STAGE_RANK[r] >= 1 for r in reasons)
        passed_team = any(STAGE_RANK[r] >= 2 for r in reasons)
        passed_device = any(STAGE_RANK[r] >= 3 for r in reasons)
        if not passed_bundle:
            report = "no-bundle-match"
        elif not passed_team:
            report = "team-mismatch"
        elif not passed_device:
            report = "device-not-provisioned"
        else:
            report = "expired"
        print(f"REASON:{report}")
        sys.exit(1)
    print(best_path)

elif mode == "check":
    path, bundle_id, team_id, hardware_udid = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
    reason, _expiry = evaluate(Path(path), bundle_id, team_id, hardware_udid, now)
    if reason != "ok":
        print(f"REASON:{reason}")
        sys.exit(1)
    print(path)

else:
    print(f"REASON:unknown-mode:{mode}", file=sys.stderr)
    sys.exit(2)
PYEOF
}

# --- フェーズ 1: profile ------------------------------------------------------
# プロビジョニングプロファイルを確保する。FANDHE_IOS_PROVISIONING_PROFILE 指定時は
# それを使う（select_or_check_profile の check モードで下記 4 条件を検証する）。
# 未指定時は project.yml の FandheIosApp ホストターゲットを
# `-allowProvisioningUpdates` でビルドさせ、Personal Team 向けに自動生成させたうえで
# 保存先ディレクトリから bundle id・development team・対象実機・有効期限の 4 条件
# （select_or_check_profile の select モード）を満たす候補中、有効期限が最も新しい
# ものを選ぶ。条件を満たす候補が複数あっても曖昧エラーにはせず最新期限のものへ
# 決定的に絞り込むため、追加の曖昧性検査は行わない。
phase_profile() {
  local explicit="${FANDHE_IOS_PROVISIONING_PROFILE:-}"
  if [[ -n "${explicit}" ]]; then
    [[ -f "${explicit}" ]] || fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたファイルが見つかりません（値はログへ出力しません）。"

    # 検証を迂回する手段は用意しない（fail-closed。6 巡目レビュー指摘）。
    # 自動選定経路と同じ 4 条件（bundle id・team・対象実機・有効期限）を検証する
    # ため、明示指定経路でも Team ID・対象実機のハードウェア UDID を先に解決する。
    local device_udid team_id hardware_udid
    device_udid="$(resolve_device_udid)"
    team_id="$(resolve_development_team)"
    hardware_udid="$(resolve_device_hardware_udid "${device_udid}")" \
      || fail "対象実機のハードウェア UDID を解決できませんでした（xcrun devicectl list
  devices --json-output の hardwareProperties.udid）。実機接続・信頼済み状態を
  確認してください。"

    local check_output
    if ! check_output="$(select_or_check_profile check "${explicit}" "${bundle_id}" "${team_id}" "${hardware_udid}")"; then
      case "${check_output}" in
        REASON:no-bundle-match)
          fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたプロファイルは bundle id
  ${bundle_id} に一致しません（読み取り不能な場合も含みます。パスの値はログへ
  出力しません）。誤ったファイルを指定していないか確認するか、正しい bundle id
  向けのプロファイルを Xcode で生成し直してください。"
          ;;
        REASON:team-mismatch)
          fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたプロファイルは
  development team（値はログへ出力しません）と一致しません。対象 Team の
  プロファイルを Xcode で生成し直すか、FANDHE_DEVELOPMENT_TEAM が正しいか
  確認してください。"
          ;;
        REASON:device-not-provisioned)
          fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたプロファイルの
  ProvisionedDevices に対象実機（UDID 末尾: $(mask_tail "${hardware_udid}")）が
  含まれていません。Xcode の自動プロビジョニングで端末を登録してから
  プロファイルを生成し直してください。"
          ;;
        REASON:expired)
          fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたプロファイルは有効期限
  切れです。Xcode で再生成してから再実行してください。"
          ;;
        *)
          fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたプロファイルの検証に
  失敗しました（想定外の出力: ${check_output}）。"
          ;;
      esac
    fi

    printf '%s' "${explicit}" > "${profile_state_file}"
    log "FANDHE_IOS_PROVISIONING_PROFILE 指定によりプロファイルを確定しました（bundle id・
  development team・対象実機・有効期限を検証済み）。"
    return 0
  fi

  local project_yml="${repo_root}/platforms/apple/project.yml"
  [[ -f "${project_yml}" ]] || fail "platforms/apple/project.yml が見つかりません。"
  grep -qE '^[[:space:]]*FandheIosApp:[[:space:]]*$' "${project_yml}" \
    || fail "platforms/apple/project.yml に iOS ホストターゲット FandheIosApp が
  定義されていません（bundle id ${bundle_id}、CODE_SIGN_STYLE: Automatic）。
  通常は本リポジトリに追加済みのはずです。取得したチェックアウトが古い・
  ローカルで誤って削除した等の場合は、最新の main を取得し直してください。"

  command -v xcodegen >/dev/null 2>&1 || fail "xcodegen が見つかりません。導入手順（自動インストールは行いません — ci.md 準拠）: brew install xcodegen"

  local apple_dir="${repo_root}/platforms/apple"
  local xcodeproj_path="${apple_dir}/Fandhe.xcodeproj"
  # project.yml が SSOT のため、.xcodeproj が未生成・project.yml より古い・
  # FandheIosApp scheme が含まれていない（この PR より前に生成された環境で
  # ターゲット追加が反映されていない）のいずれかであれば再生成する。
  # xcodegen generate は冪等（generate-xcodeproj.sh 冒頭コメント参照）なので
  # 都度実行のコストは小さく、無条件の毎回再生成より軽い判定に留める。
  local needs_regen=0
  if [[ ! -d "${xcodeproj_path}" ]]; then
    needs_regen=1
  elif [[ "${project_yml}" -nt "${xcodeproj_path}" ]]; then
    log "project.yml が Fandhe.xcodeproj より新しいため再生成します。"
    needs_regen=1
  else
    # `xcodebuild -list ... | grep -q` はパイプ直結にすると、grep が一致時点で読み取りを
    # 打ち切った際に producer 側の xcodebuild が SIGPIPE で非 0 終了し、set -o pipefail の
    # 下で「scheme が存在するのに未検出」と誤判定し得る。出力をいったん変数へ受けてから
    # here-string で grep することで producer を SIGPIPE から切り離す。
    local scheme_list
    scheme_list="$(xcodebuild -list -project "${xcodeproj_path}" 2>/dev/null || true)"
    if ! grep -qx '[[:space:]]*FandheIosApp' <<<"${scheme_list}"; then
      log "既存の Fandhe.xcodeproj に FandheIosApp scheme が見つからないため再生成します。"
      needs_regen=1
    fi
  fi
  if [[ "${needs_regen}" -eq 1 ]]; then
    log "scripts/apple/generate-xcodeproj.sh を実行して Fandhe.xcodeproj を（再）生成します。"
    bash "${script_dir}/../apple/generate-xcodeproj.sh" 1>&2
  fi
  [[ -d "${xcodeproj_path}" ]] || fail "Fandhe.xcodeproj の生成に失敗しました: ${xcodeproj_path}"

  local device_udid
  device_udid="$(resolve_device_udid)"
  log "実機（UDID 末尾: $(mask_tail "${device_udid}")）向けに FandheIosApp をビルドし、Personal Team のプロビジョニングプロファイルを自動生成させます。"

  # DEVELOPMENT_TEAM は project.yml へ書かず環境変数からのみ注入する（既存の
  # scripts/apple/verify-device-launch-watchos.sh・verify-visionos-device-launch.sh と
  # 同方針）。CODE_SIGN_STYLE: Automatic でも DEVELOPMENT_TEAM 自体は必須のため、
  # 未指定時は codesign 用証明書から自動解決する（resolve_development_team 参照）。
  local team_id
  team_id="$(resolve_development_team)"
  log "development team を解決しました（値はログへ出力しません）。"

  # .mobileprovision の ProvisionedDevices と照合するためのハードウェア UDID
  # （resolve_device_udid が返す CoreDevice 識別子とは別物。上記
  # resolve_device_hardware_udid 参照）。
  local hardware_udid
  hardware_udid="$(resolve_device_hardware_udid "${device_udid}")" \
    || fail "対象実機のハードウェア UDID を解決できませんでした（xcrun devicectl list
  devices --json-output の hardwareProperties.udid）。実機接続・信頼済み状態を
  確認してください。"
  log "実機のハードウェア UDID を解決しました（値はログへ出力しません）。"

  # xcodebuild は既定 60 秒 destination 待ちのうえ、DDI（Developer Disk Image）が
  # マウントできない場合でも「タイムアウトした」としか表示せず原因が読み取りにくい。
  # -destination-timeout で早めに失敗させ、下記のログ検査で原因を補足する。
  # 既定 120 秒: 無線（localNetwork）接続端末では DDI マウントに時間がかかり、実測で
  # 15 秒では不足・120 秒で成功することを確認済み（Issue #567 実機検証）。
  local destination_timeout="${FANDHE_IOS_DESTINATION_TIMEOUT:-120}"
  local -a xcodebuild_args=(
    -allowProvisioningUpdates
    -project "${xcodeproj_path}"
    -scheme "FandheIosApp"
    -destination "platform=iOS,id=${device_udid}"
    -destination-timeout "${destination_timeout}"
    "DEVELOPMENT_TEAM=${team_id}"
    build
  )

  # DDI 不整合（Xcode 同梱 DDI が端末 iOS より古い）の事前情報として、Xcode ビルド番号・
  # 端末 OS バージョン・接続形態（無線 vs USB）を表示する。バージョン比較による事前
  # ブロックは行わない（新しい Xcode が新 OS に対応済みのケースを誤って弾かないため。
  # あくまで失敗時のガイダンス強化が目的）。無線（localNetwork）接続の場合は DDI
  # マウントが遅延・失敗しやすいため、ビルド前に警告を出す（実測の主因、Issue #567）。
  local xcode_build_version device_os_version device_transport_type
  xcode_build_version="$(xcodebuild -version 2>/dev/null | awk '/Build version/{print $NF}')"
  device_os_version="$(resolve_device_os_version "${device_udid}")"
  device_transport_type="$(resolve_device_transport_type "${device_udid}")"
  log "Xcode ビルド番号: ${xcode_build_version:-不明} / 端末 OS バージョン: ${device_os_version:-不明} / 接続形態: ${device_transport_type:-不明}"
  if [[ "${device_transport_type}" == "localNetwork" ]]; then
    log "端末が無線（localNetwork）接続です。DDI（Developer Disk Image）のマウントに
  時間がかかる・失敗しやすいため、可能であれば USB 直結で再試行してください。"
  fi

  log "xcodebuild -allowProvisioningUpdates -scheme FandheIosApp -destination 'platform=iOS,id=<UDID>' -destination-timeout ${destination_timeout} build"
  local build_log="${out_dir}/xcodebuild-profile.log"
  # スクリプト冒頭の `set -euo pipefail` により、パイプの終了ステータスは
  # 右端から見て最初の非 0（＝ xcodebuild の失敗）になる（mask_secrets は常に 0 で
  # 終了するため PIPESTATUS の個別参照は不要）。ログファイルはマスク済みの内容のみを
  # 書き込む（未加工の出力を一切ディスクへ残さない）。
  if ! xcodebuild "${xcodebuild_args[@]}" 2>&1 \
    | mask_secrets "${team_id}" "${device_udid}" "${hardware_udid}" "${FANDHE_IOS_CODESIGN_IDENTITY:-}" >"${build_log}"; then
    tail -n 60 "${build_log}" >&2
    if grep -qi "developer disk image could not be mounted\|Timed out waiting for all destinations" "${build_log}"; then
      log "DDI（Developer Disk Image）のマウントに失敗した可能性があります。主な原因:
  1. 端末が無線（localNetwork）接続の場合、DDI マウントに時間がかかり失敗しやすい
     ため、USB 直結を試してください（接続形態: ${device_transport_type:-不明}）。
  2. -destination-timeout（現在 ${destination_timeout} 秒）が不足している可能性があり
     ます。FANDHE_IOS_DESTINATION_TIMEOUT でさらに延ばして再試行してください。
  3. （副次的な可能性）Xcode ビルド番号（${xcode_build_version:-不明}）が端末の OS
     バージョン（${device_os_version:-不明}）に対応していない場合があります。"
    fi
    fail "xcodebuild によるプロファイル自動生成に失敗しました（詳細ログ: ${build_log}）。Xcode に
  無料 Apple ID がサインインされているか（Xcode > Settings > Accounts）、端末が信頼済みかを
  確認してください。ログファイルは DEVELOPMENT_TEAM・UDID・Signing Identity・証明書 SHA1 等
  主要な署名情報はマスク済みですが、マスクの完全性は保証していません（想定外の形式で
  証明書の氏名・プロファイル名等が残る可能性があります）。Issue・PR へ共有する前に
  必ず内容を目視確認してください。"
  fi

  # Xcode 16 以降、プロビジョニングプロファイルの保存先が
  # ~/Library/Developer/Xcode/UserData/Provisioning Profiles/ へ変更された（実測: 旧パス
  # は未生成で新パスに 4 件存在。Issue #567）。ただし Xcode の更新・切り戻しや複数
  # バージョン併用後は旧パス側にのみ有効なプロファイルが残る環境もあり得るため、
  # 存在する方だけに絞らず両方を探索対象にする（新パスが存在すれば旧パスを
  # 探索しない、という誤り方は過去の検証記録（非公開）の
  # 「両方の位置をチェックする」契約に反する。Issue #567 10 巡目レビュー指摘 P1）。
  local profiles_dir_new="${HOME}/Library/Developer/Xcode/UserData/Provisioning Profiles"
  local profiles_dir_legacy="${HOME}/Library/MobileDevice/Provisioning Profiles"
  local -a profiles_dirs=()
  [[ -d "${profiles_dir_new}" ]] && profiles_dirs+=("${profiles_dir_new}")
  [[ -d "${profiles_dir_legacy}" ]] && profiles_dirs+=("${profiles_dir_legacy}")
  if [[ "${#profiles_dirs[@]}" -eq 0 ]]; then
    fail "プロビジョニングプロファイルディレクトリが見つかりません:
  ${profiles_dir_new}
  ${profiles_dir_legacy}"
  fi
  log "プロビジョニングプロファイルの探索先（${#profiles_dirs[@]} 件）:"
  local dir
  for dir in "${profiles_dirs[@]}"; do
    log "  - ${dir}"
  done

  # bundle id・development team・対象実機（ProvisionedDevices）が一致し、かつ
  # 有効期限内のプロファイルを選ぶ。判定条件は select_or_check_profile()（上部
  # 共通処理）に集約しており、明示指定経路（mode=check）とロジックを共有する
  # （Issue #567 5 巡目レビュー指摘）。`security cms -D` による復号と有効期限比較は
  # plistlib（<date> を素の datetime として解釈できる）に任せることで、PlistBuddy
  # の日時書式パースの脆さを避ける（build-simulator-app.sh が Info.plist 検査に
  # python3 を使っている先例と同方針）。同一 bundle id のプロファイルが複数 Team・
  # 複数端末向けに同居し得るため（実機検証で確認済み）、Team・端末不一致のものは
  # 候補から除外する。ExpirationDate は plist 上 UTC を表す naive datetime として
  # 復元されるため、比較対象の「今」も UTC 基準（datetime.now(timezone.utc) を
  # naive 化）に揃える（ローカル時刻のまま比較すると UTC より進んだタイムゾーンで
  # 早期に期限切れ誤判定する）。
  local selection_output
  if ! selection_output="$(select_or_check_profile select "${bundle_id}" "${team_id}" "${hardware_udid}" "${profiles_dirs[@]}")"; then
    case "${selection_output}" in
      REASON:no-bundle-match)
        fail "bundle id ${bundle_id} に一致するプロビジョニングプロファイルが
  探索先（$(printf '%s ' "${profiles_dirs[@]}")）に見つかりません（自動生成に失敗した可能性があります）。"
        ;;
      REASON:team-mismatch)
        fail "bundle id ${bundle_id} に一致するプロファイルはありますが、development
  team（値はログへ出力しません）と一致するものがありません。同一マシンに複数 Team
  のプロファイルが同居している可能性があります。FANDHE_IOS_PROVISIONING_PROFILE で
  対象プロファイルを明示指定するか、Xcode で当該 Team のプロファイルを再生成して
  ください。"
        ;;
      REASON:device-not-provisioned)
        fail "bundle id・development team に一致するプロファイルはありますが、対象
  実機（UDID 末尾: $(mask_tail "${hardware_udid}")）が ProvisionedDevices に
  含まれていません。Xcode の自動プロビジョニングで端末を登録してから再実行するか、
  FANDHE_IOS_PROVISIONING_PROFILE で対象プロファイルを明示指定してください。"
        ;;
      REASON:expired)
        fail "bundle id・development team・対象実機に一致するプロファイルはあります
  が、有効期限内のものがありません。Xcode で再生成してから再実行してください。"
        ;;
      *)
        fail "bundle id ${bundle_id} に一致する有効なプロビジョニングプロファイルが
  探索先（$(printf '%s ' "${profiles_dirs[@]}")）に見つかりません（自動生成に失敗した可能性があります）。"
        ;;
    esac
  fi

  local selected="${selection_output}"
  printf '%s' "${selected}" > "${profile_state_file}"
  log "プロビジョニングプロファイルを自動選定しました（パスはログへ出力しません）。"
}

# --- フェーズ 2: build ---------------------------------------------------
# 実機向け crate ビルド。既定では計測用 feature を付けない（起動確認専用）。
# `FANDHE_IOS_CARGO_FEATURES` を指定した場合のみ、その feature 付きでビルドする
# （TASK-5.3 / Issue #571: 実機フレームレート計測で `frame-metrics` を渡すため。
# 環境変数名・入力検証は scripts/ios/build-simulator-app.sh と同一の契約に揃える）。
phase_build() {
  log "rustup target add ${fandhe_target}"
  rustup target add "${fandhe_target}" >/dev/null

  # 英数・ハイフン・カンマのみを許容する（cargo の feature 名規約に合わせた
  # ホワイトリスト）。任意文字列をそのまま `--features` へ展開しない（A03 対策）。
  local cargo_features="${FANDHE_IOS_CARGO_FEATURES:-}"
  if [[ -n "${cargo_features}" ]] && [[ ! "${cargo_features}" =~ ^[A-Za-z0-9,-]+$ ]]; then
    fail "FANDHE_IOS_CARGO_FEATURES に許容されない文字が含まれています: ${cargo_features}"
  fi

  # 未指定時に空要素が `cargo build` へ渡らないよう、配列自体を分岐で組み立てる。
  local -a feature_args=()
  if [[ -n "${cargo_features}" ]]; then
    feature_args=(--features "${cargo_features}")
    log "計測用 feature を有効化します: ${cargo_features}"
  fi

  log "cargo build -p fandhe-egui-proxy --bin ${fandhe_bin_name} --target ${fandhe_target} --profile release${cargo_features:+ --features ${cargo_features}}"
  cargo build --manifest-path "${repo_root}/Cargo.toml" -p fandhe-egui-proxy --bin "${fandhe_bin_name}" \
    --target "${fandhe_target}" --profile release "${feature_args[@]+"${feature_args[@]}"}"

  local built_binary="${repo_root}/target/${fandhe_target}/release/${fandhe_bin_name}"
  [[ -f "${built_binary}" ]] || fail "ビルド成果物が見つかりません: ${built_binary}"
}

# --- フェーズ 3: bundle ----------------------------------------------------
# .app を組み立てる（バイナリ + Info.plist + embedded.mobileprovision）。
# プロファイルは必ず profile フェーズが検証済みで書き出した状態ファイル
# （profile_state_file）からのみ読む。FANDHE_IOS_PROVISIONING_PROFILE を
# ここで直接参照すると、`--phase bundle` 単体実行時に select_or_check_profile
# の 4 条件検証（bundle id・development team・対象実機・有効期限）を一切
# 通らずに未検証のプロファイルを採用できてしまう（Issue #567 6 巡目レビュー
# 指摘 2）。「検証を通った状態ファイル経由でのみ次フェーズへ進む」という
# 既存の疎結合契約（本ファイル冒頭コメント参照）をそのまま検証契約としても
# 適用する設計を採る（案 a。案 b の「bundle フェーズ内で検証を再実行する」
# よりも実装が単純で、検証ロジックの置き場所を profile フェーズ 1 箇所に
# 保てる）。FANDHE_IOS_PROVISIONING_PROFILE を使いたい場合は
# `--phase profile` を先に実行して状態ファイルへ検証済みパスを確定させる。
#
# 状態ファイルは検証時点のパス 1 行のみを保持し、検証に使った対象実機・Team・
# 検証日時とは結び付いていない（UDID・Team ID を状態ファイルへ平文で永続化する
# 案は、ディスク上に増える識別子を避けるため不採用。Issue #567 7 巡目レビュー
# 指摘 S2）。そのため profile フェーズ実行後に対象実機・Team・プロファイルの
# 有効期限が変わっていても検知できないという古さの問題が残る。これを防ぐため、
# bundle フェーズの冒頭で現在の team / hardware_udid を用いて
# select_or_check_profile check を再実行し、状態ファイルの内容が今も有効か
# 確認してから採用する。
phase_bundle() {
  local built_binary="${repo_root}/target/${fandhe_target}/release/${fandhe_bin_name}"
  [[ -f "${built_binary}" ]] || fail "ビルド成果物が見つかりません: ${built_binary}（先に build フェーズを実行してください）"

  [[ -f "${profile_state_file}" ]] || fail "プロビジョニングプロファイルが未解決です（先に profile フェーズを実行してください。
  FANDHE_IOS_PROVISIONING_PROFILE を使う場合も --phase profile を先に実行し、
  4 条件検証を通した状態ファイルを確定させる必要があります）。"
  local profile_path
  profile_path="$(cat "${profile_state_file}")"
  [[ -f "${profile_path}" ]] || fail "プロビジョニングプロファイルが見つかりません（値はログへ出力しません）。"

  local device_udid team_id hardware_udid
  device_udid="$(resolve_device_udid)"
  team_id="$(resolve_development_team)"
  hardware_udid="$(resolve_device_hardware_udid "${device_udid}")" \
    || fail "対象実機のハードウェア UDID を解決できませんでした（xcrun devicectl list
  devices --json-output の hardwareProperties.udid）。実機接続・信頼済み状態を
  確認してください。"

  local recheck_output
  if ! recheck_output="$(select_or_check_profile check "${profile_path}" "${bundle_id}" "${team_id}" "${hardware_udid}")"; then
    fail "profile フェーズで検証したプロビジョニングプロファイルが、現在の対象実機・
  development team に対して無効になっています（理由: ${recheck_output#REASON:}）。
  対象実機・Team を変更していないか、プロファイルの有効期限が切れていないかを
  確認したうえで、--phase profile を再実行してください。"
  fi

  log "アプリバンドル組み立て: ${app_dir}"
  rm -rf "${app_dir}"
  mkdir -p "${app_dir}"
  cp "${built_binary}" "${app_dir}/${app_name}"
  cp "${info_plist_src}" "${app_dir}/Info.plist"
  cp "${info_plist_src%/*}/PrivacyInfo.xcprivacy" "${app_dir}/PrivacyInfo.xcprivacy"

  # FANDHE_ITEMS_URL 指定時のみ、LAN 限定の ATS 例外を .app 側の Info.plist へ
  # 追加する（リポジトリの plist ファイル自体は変更しない、Issue #660）。
  # http_fetch.rs の FANDHE_ITEMS_URL 形式検証（http://<host>...、ビルド時 assert）に
  # 加えて、ここでも許容文字のホワイトリストで弾く（A03 対策、
  # FANDHE_IOS_CARGO_FEATURES の検査に倣う）。値をログへ出す場合はホスト部を
  # マスクする（security.md ログ方針）。
  local items_url="${FANDHE_ITEMS_URL:-}"
  if [[ -n "${items_url}" ]]; then
    # 正規表現は変数に置く（`[[ =~ ]]` に直書きすると角括弧の扱いが意図とずれる）。
    # IPv6 の角括弧表記は fandhe-device 側でも許可対象外のため受け付けない。
    local items_url_re='^http://[A-Za-z0-9][A-Za-z0-9.:-]*(/[A-Za-z0-9._/-]*)?$'
    if [[ ! "${items_url}" =~ ${items_url_re} ]]; then
      fail "FANDHE_ITEMS_URL に許容されない形式が含まれています（値はログへ出力しません）。"
    fi
    log "FANDHE_ITEMS_URL 指定を検出しました。LAN 限定の ATS 例外を Info.plist へ追加します。"
    # `-insert` は既存キーがあると失敗する（リポジトリ側 plist に同キーが入ったら
    # `-replace` へ切り替える）。
    plutil -insert NSAppTransportSecurity -xml \
      '<dict><key>NSAllowsLocalNetworking</key><true/></dict>' \
      "${app_dir}/Info.plist" \
      || fail "Info.plist への NSAppTransportSecurity 差し込みに失敗しました（既にキーがある場合は -replace へ変更してください）。"
    plutil -insert NSLocalNetworkUsageDescription -string \
      '型サンプル「HTTP 取得」の実機確認で Mac 上のダミー API へ接続します' \
      "${app_dir}/Info.plist" \
      || fail "Info.plist への NSLocalNetworkUsageDescription 差し込みに失敗しました。"
  fi

  cp "${profile_path}" "${app_dir}/embedded.mobileprovision"
  log "バンドル組み立て OK: ${app_dir}"
}

# sign フェーズが作る一時 plist の後始末。trap へはこの関数名だけを登録し、
# パスをコマンド文字列として展開しない（インジェクション経路を作らないため）。
# 対象パスは phase_sign が sign_entitlements_plist / sign_embedded_profile_plist へ
# 設定する（local にすると関数を抜けた時点で参照できなくなるためグローバル）。
sign_entitlements_plist=""
sign_embedded_profile_plist=""
cleanup_sign_artifacts() {
  rm -f -- "${sign_entitlements_plist}" "${sign_embedded_profile_plist}"
}

# --- フェーズ 4: sign ------------------------------------------------------
# embedded.mobileprovision（bundle フェーズで同梱済み）から entitlements を抽出し、
# 実署名する（measure-cold-start.sh フェーズ 3 と同じ手順）。
phase_sign() {
  [[ -d "${app_dir}" ]] || fail "アプリバンドルが見つかりません: ${app_dir}（先に bundle フェーズを実行してください）"
  local embedded_profile="${app_dir}/embedded.mobileprovision"
  [[ -f "${embedded_profile}" ]] || fail "embedded.mobileprovision が見つかりません（先に bundle フェーズを実行してください）。"

  local identity="${FANDHE_IOS_CODESIGN_IDENTITY:-}"
  if [[ -z "${identity}" ]]; then
    identity="$(resolve_codesign_identity "${embedded_profile}")"
  else
    # 明示指定された署名 ID も、embedded profile の DeveloperCertificates に
    # 含まれるかを検証する（未検証のまま通すと自動解決経路と同型の迂回になる。
    # Issue #567 7 巡目レビュー指摘 S3。resolve_codesign_identity と同じ
    # SHA1 突き合わせロジックを check モードとして共有する）。
    check_codesign_identity "${identity}" "${embedded_profile}"
  fi
  [[ -n "${identity}" ]] || fail "codesign 用の署名 ID を解決できませんでした。"

  # sign フェーズが途中で失敗した場合、entitlements.plist・embedded-profile.plist
  # （Team prefix 付き App ID を含む）を target/ 配下に残置しない（Issue #567
  # 7 巡目レビュー指摘 N4）。set -e により失敗時は `return` を経由せず即座に
  # プロセスが終了するため、RETURN トラップではなく EXIT トラップを使う
  # （変数値はトラップ設定時点で展開しておき、関数を抜けた後の変数消失を回避する）。
  # 正常終了時は明示的に rm したうえでトラップを解除する。
  # 一時 plist（Team prefix 付き App ID を含む）は sign が途中で失敗しても残さない。
  # trap にパスをコマンド文字列として埋め込むと、repo_root（checkout 先の絶対パス）に
  # シングルクォートが含まれる場合に引用が閉じて任意コマンドが実行される。実際に
  # 再現を確認したため、パスはグローバル変数へ渡し、trap には固定の関数名だけを登録する。
  sign_entitlements_plist="${out_dir}/entitlements.plist"
  sign_embedded_profile_plist="${out_dir}/embedded-profile.plist"
  local entitlements_plist="${sign_entitlements_plist}"
  local embedded_profile_plist="${sign_embedded_profile_plist}"
  trap cleanup_sign_artifacts EXIT

  security cms -D -i "${embedded_profile}" > "${embedded_profile_plist}"
  /usr/libexec/PlistBuddy -x -c "Print :Entitlements" "${embedded_profile_plist}" > "${entitlements_plist}"

  log "実署名: codesign --sign <identity> --entitlements <profile 抽出分> --force ${app_dir}（identity はログへ出力しません）"
  codesign --sign "${identity}" --entitlements "${entitlements_plist}" --force "${app_dir}"
  codesign --verify --verbose=2 "${app_dir}"
  rm -f "${entitlements_plist}" "${embedded_profile_plist}"
  trap - EXIT
  log "実署名 OK"
}

# embedded.mobileprovision の DeveloperCertificates（DER 証明書群）から SHA1
# ハッシュ一覧を抽出する（resolve_codesign_identity・check_codesign_identity の
# 共通処理。旧実装ではこの結果を格納する変数名が matched_count という
# リファクタ残骸だったため改名した。Issue #567 7 巡目レビュー指摘 N2）。
extract_profile_cert_sha1s() {
  local profile_path="$1"
  python3 - "${profile_path}" <<'PYEOF' 2>/dev/null
import hashlib
import plistlib
import subprocess
import sys

decoded = subprocess.run(
    ["security", "cms", "-D", "-i", sys.argv[1]],
    check=True, capture_output=True,
).stdout
profile = plistlib.loads(decoded)
hashes = set()
for der in profile.get("DeveloperCertificates", []):
    hashes.add(hashlib.sha1(der).hexdigest().upper())
for h in sorted(hashes):
    print(h)
PYEOF
}

# embedded.mobileprovision の DeveloperCertificates（DER 証明書群）の SHA1 と
# `security find-identity -v -p codesigning` の有効な署名 ID を突き合わせ、一意に
# 決まる場合のみその SHA1 を返す（codesign --sign には SHA1 ハッシュをそのまま渡せるため、
# 証明書名・Team ID を含む識別子文字列自体はログへ一切出力しない）。
resolve_codesign_identity() {
  local profile_path="$1"
  local find_identity_output
  find_identity_output="$(security find-identity -v -p codesigning 2>/dev/null || true)"

  local profile_cert_sha1s
  profile_cert_sha1s="$(extract_profile_cert_sha1s "${profile_path}")"
  [[ -n "${profile_cert_sha1s}" ]] || fail "プロビジョニングプロファイルから証明書を抽出できませんでした。"

  local candidates=()
  local sha1
  while IFS= read -r sha1; do
    [[ -z "${sha1}" ]] && continue
    if grep -qF "${sha1}" <<<"${find_identity_output}"; then
      candidates+=("${sha1}")
    fi
  done <<<"${profile_cert_sha1s}"

  if [[ "${#candidates[@]}" -eq 0 ]]; then
    fail "プロファイルの証明書に一致する codesign 用の署名 ID がキーチェーンに見つかりません。
  \`security find-identity -v -p codesigning\` で有効な署名 ID を確認し、
  FANDHE_IOS_CODESIGN_IDENTITY で明示指定してください。"
  fi
  if [[ "${#candidates[@]}" -gt 1 ]]; then
    fail "codesign 用の署名 ID が複数一致し曖昧です（${#candidates[@]} 件）。
  \`security find-identity -v -p codesigning\` を確認し、
  FANDHE_IOS_CODESIGN_IDENTITY で明示指定してください。"
  fi

  printf '%s\n' "${candidates[0]}"
}

# FANDHE_IOS_CODESIGN_IDENTITY で明示指定された署名 ID を検証する（fail-closed。
# resolve_codesign_identity と同じ SHA1 突き合わせロジックを使う。Issue #567
# 7 巡目レビュー指摘 S3）。SHA1 ハッシュ（40 桁 16 進）をそのまま指定した場合は
# 直接照合する。それ以外（証明書名等）を指定した場合は
# `security find-identity -v -p codesigning` から一意に対応する SHA1 を
# 引き当てたうえで照合する（曖昧な場合は SHA1 での明示指定を促して停止する）。
check_codesign_identity() {
  local identity="$1"
  local profile_path="$2"

  local sha1_to_check
  if [[ "${identity}" =~ ^[0-9A-Fa-f]{40}$ ]]; then
    sha1_to_check="$(printf '%s' "${identity}" | tr '[:lower:]' '[:upper:]')"
  else
    local find_identity_output matches match_count
    find_identity_output="$(security find-identity -v -p codesigning 2>/dev/null || true)"
    matches="$(printf '%s\n' "${find_identity_output}" | grep -F -- "${identity}" | grep -oE '[0-9A-Fa-f]{40}' || true)"
    match_count="$(printf '%s\n' "${matches}" | grep -c . || true)"
    if [[ "${match_count}" -ne 1 ]]; then
      fail "FANDHE_IOS_CODESIGN_IDENTITY（値はログへ出力しません）に一致する署名 ID を
  一意に特定できませんでした。\`security find-identity -v -p codesigning\` の
  SHA1 ハッシュ（40 桁 16 進）を直接指定してください。"
    fi
    sha1_to_check="$(printf '%s\n' "${matches}" | head -n1 | tr '[:lower:]' '[:upper:]')"
  fi

  local profile_cert_sha1s
  profile_cert_sha1s="$(extract_profile_cert_sha1s "${profile_path}")"
  if ! grep -qxF "${sha1_to_check}" <<<"${profile_cert_sha1s}"; then
    fail "FANDHE_IOS_CODESIGN_IDENTITY で指定された署名 ID は embedded.mobileprovision
  の DeveloperCertificates に含まれていません（値はログへ出力しません）。
  対象プロファイルに対応する証明書を指定するか、指定を外して自動解決に
  任せてください。"
  fi
}

# --- フェーズ 5: install ---------------------------------------------------
# device UDID は profile フェーズと独立に毎回 resolve_device_udid() で解決し直す
# （状態ファイルへ UDID を平文で永続化しない方針は S2 と同じ。Issue #567
# 7 巡目レビュー指摘 S4）。実害の有無は以下の理由で限定的と判断した:
# - 端末が 1 台のみ、または FANDHE_IOS_DEVICE_UDID 明示指定時は毎回同じ値に決定的に
#   解決される（挙動が変わり得るのは複数台環境で自動選定に委ねた場合のみ）。
# - 万一 profile フェーズと異なる端末へインストールしても、当該端末に対象
#   プロファイルが登録されていなければ iOS 側の署名・プロビジョニング検証で
#   install/launch が失敗する（fail-closed。「誤った端末へ黙って配布される」
#   事態にはならない）。
# 複数台を明示的に切り替えて検証する場合は、各 --phase 実行のたびに
# FANDHE_IOS_DEVICE_UDID を明示指定して意図しない切り替わりを防ぐこと。
phase_install() {
  [[ -d "${app_dir}" ]] || fail "アプリバンドルが見つかりません: ${app_dir}（先に sign フェーズを実行してください）"
  local device_udid
  device_udid="$(resolve_device_udid)"
  log "xcrun devicectl device install app --device <UDID> ${app_name}.app（UDID はログへ出力しません）"
  xcrun devicectl device install app --device "${device_udid}" "${app_dir}" >/dev/null
  log "インストール OK"
}

# --- フェーズ 6: launch ---------------------------------------------------
# `--console`（Xcode 16+ 必須）は使わない。起動プロセスの出力中継は行わず、
# `--terminate-existing` のみで既存インスタンスを終了してから起動する。
# device UDID の独立再解決については phase_install 冒頭コメント（S4）を参照。
phase_launch() {
  local device_udid
  device_udid="$(resolve_device_udid)"
  log "xcrun devicectl device process launch --terminate-existing --device <UDID> ${bundle_id}（UDID はログへ出力しません）"
  if ! xcrun devicectl device process launch --terminate-existing --device "${device_udid}" "${bundle_id}" 1>&2; then
    fail "アプリの起動に失敗しました。実機の画面ロック解除・信頼済み状態を確認してください。"
  fi
  log "起動コマンド送出 OK"
  log ""
  log "=== 目視確認のお願い（人間担当） ==="
  log "実機の画面で fandhe_core::sample_screen() 由来の代表 1 画面サンプルが描画されていることを確認してください。"
  log "確認項目: 日本語ラベルがフォールバック文言・tofu 表示でないこと / ボタン押下やテキスト入力の状態反映。"

  if [[ "${FANDHE_IOS_UNINSTALL_AFTER:-}" == "1" ]]; then
    log "FANDHE_IOS_UNINSTALL_AFTER=1 のためアプリを uninstall します。"
    xcrun devicectl device uninstall app --device "${device_udid}" "${bundle_id}" >/dev/null 2>&1 || true
  else
    log "目視確認のためアプリは端末に残置します（uninstall する場合は FANDHE_IOS_UNINSTALL_AFTER=1 を指定して再実行してください）。"
  fi
}

# --- 引数解析（--phase <name> / --help） -------------------------------------
print_help() {
  cat <<'EOF'
使い方: scripts/ios/verify-device-launch.sh [--phase <name>]

iPhone 実機（無料 Apple ID の Personal Team）へ fandhe アプリをビルド・実署名・
インストールし、起動確認する（TASK-1.4 系 / Issue #567）。

オプション:
  --phase <name>  指定フェーズのみ実行する。
                  <name>: profile | build | bundle | sign | install | launch
                  省略時は全フェーズを順に実行する。
  --help          このヘルプを表示する。

環境変数:
  FANDHE_IOS_DEVICE_UDID           対象実機の UDID（既定: 自動解決）
  FANDHE_DEVELOPMENT_TEAM          development team（Team ID）。既定: 自動解決
                                    （codesign 用証明書から。曖昧な場合は候補件数のみ
                                    報告して停止する）
  FANDHE_IOS_PROVISIONING_PROFILE  .mobileprovision のパス（既定: 自動生成・自動解決。
                                    指定時も bundle id・development team・対象実機・
                                    有効期限を自動選定経路と同じ条件で検証する。
                                    この検証を迂回する手段はない）
  FANDHE_IOS_CODESIGN_IDENTITY     codesign 用の署名 ID（既定: 自動解決）
  FANDHE_IOS_UNINSTALL_AFTER       "1" 指定時のみ起動確認後に uninstall する
  FANDHE_IOS_DESTINATION_TIMEOUT   xcodebuild -destination-timeout の秒数（既定: 120。
                                    無線接続端末での DDI マウント遅延を考慮した値）
EOF
}

requested_phase="all"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --phase)
      [[ $# -ge 2 ]] || fail "--phase にはフェーズ名を指定してください。"
      requested_phase="$2"
      shift 2
      ;;
    --help | -h)
      print_help
      exit 0
      ;;
    *)
      fail "未知の引数です: $1（--help を参照してください）"
      ;;
  esac
done

case "${requested_phase}" in
  all | profile | build | bundle | sign | install | launch) ;;
  *)
    fail "未知のフェーズです: ${requested_phase}（対応: profile/build/bundle/sign/install/launch、省略時は全フェーズ）"
    ;;
esac

# --- ディスパッチ -----------------------------------------------------------
case "${requested_phase}" in
  all)
    phase_profile
    phase_build
    phase_bundle
    phase_sign
    phase_install
    phase_launch
    ;;
  profile) phase_profile ;;
  build) phase_build ;;
  bundle) phase_bundle ;;
  sign) phase_sign ;;
  install) phase_install ;;
  launch) phase_launch ;;
esac
