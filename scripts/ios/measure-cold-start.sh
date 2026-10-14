#!/usr/bin/env bash
# TASK-5.1（REQ-5 / M4、Issue #236）: iOS 実機での fandhe / ネイティブ SDK 比較
# コールドスタート計測スクリプト。
#
# 位置づけ: scripts/ios/verify-simulator-render.sh（TASK-1.4）・
# scripts/ios/build-simulator-app.sh（TASK-11.1）が確立した「macOS 限定・冪等・
# 進捗ログは stderr / 結果は stdout 末尾に整形出力」の規約を踏襲する。ただし
# 対象はシミュレータではなく**実機**であり、`xcrun devicectl`（Xcode 15+ 同梱。
# ただし本スクリプトの計測本体（フェーズ 5）は `--console`（Xcode 16 で追加）を
# 使うため、本スクリプト自体は Xcode 16+ が必須）経由でインストール・起動・
# ログ取得を行う。Xcode 15 では `--terminate-existing` は使えても `--console` は
# 使えないため、フェーズ 1 の前提条件チェックは両者を区別して検査する
# （Review 指摘対応）。
#
# 前提条件（TASK-18.2 / Issue #12、本 issue 作成時点で OPEN）:
#   実機へのインストールには Apple Developer Program の開発署名（証明書・
#   プロビジョニングプロファイル）が必須。TASK-18.2 完了前は本スクリプトを
#   実行しても署名フェーズで fail-fast する（下記フェーズ 2 参照）。
#
# 計測方法（fandhe 側）: `crates/fandhe-egui-proxy/src/ios.rs` の `ColdStartProbe`
# が `run()` 呼び出しから初回フレーム描画完了までの経過時間を
# `COLD_START_MS=<value>` の形式で stderr（Console.app 経由で `log` コマンドから
# 参照可能）へ出力する契約（同ファイル冒頭コメント参照）。本スクリプトは
# `log stream`（または `devicectl` 経由のプロセスログ）からこの行を抽出する。
#
# 計測方法（ネイティブ比較アプリ側）: `benchmarks/coldstart/ios-native/` の
# SwiftUI アプリが `main.swift` 冒頭を `t0` として同一の `COLD_START_MS=<value>`
# 形式で出力する（同ディレクトリの `main.swift` 冒頭コメント参照）。
#
# 両者の t0 の対称性については fandhe-egui-proxy/src/ios.rs の `run()` 直前の
# コメントに明記した限界（dyld・プロセス起動時間は含まれない近似）を参照。
#
# 実行環境について: `xcodebuild` / `devicectl` は macOS + Xcode 限定のため、
# Linux 実行時は手順を案内して明示的に失敗する（ci.md「野良インストール禁止」
# 準拠のため Xcode 等の自動インストールはしない）。
#
# 環境変数インターフェース:
#   FANDHE_IOS_DEVICE_UDID   必須（macOS 実行時）。対象実機の UDID
#                            （`xcrun devicectl list devices` で確認）。
#                            値はログへ出力しない（個体識別子の記載禁止・
#                            security.md 準拠）。
#   FANDHE_IOS_TEAM_ID       必須（macOS 実行時）。Apple Developer Team ID。
#                            リポジトリ・ログには一切書き込まない
#                            （環境変数経由でのみ受け取る、security.md 準拠）。
#   FANDHE_COLD_START_TRIALS 既定: 5（各実装の試行回数。中央値を採用する）。
#
# 使い方: scripts/ios/measure-cold-start.sh
set -euo pipefail

unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# devicectl の console 出力の一時退避先（target/ 配下に置き、リポジトリ管理下を汚さない）。
launch_console_tmp_dir="${repo_root}/target/ios-cold-start"
mkdir -p "${launch_console_tmp_dir}"
trials="${FANDHE_COLD_START_TRIALS:-5}"

log() {
  echo "[measure-cold-start-ios] $*" >&2
}

fail() {
  echo "[measure-cold-start-ios] エラー: $*" >&2
  exit 1
}

case "${trials}" in
  ''|*[!0-9]*)
    fail "FANDHE_COLD_START_TRIALS は正の整数のみ対応: ${trials}"
    ;;
esac
[ "${trials}" -ge 1 ] || fail "FANDHE_COLD_START_TRIALS は 1 以上を指定してください"

# --- フェーズ 1: 環境検査（macOS 限定） -------------------------------------
if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン・
  xcrun devicectl 必須）。手順:
  1. macOS + Xcode 16+ 環境で本スクリプトを実行する（フェーズ 5 の計測本体が
     devicectl device process launch --console（Xcode 16 以降で追加）を使うため）
  2. TASK-18.2（Apple Developer 証明書、Issue #12）が完了していることを確認する
  3. FANDHE_IOS_DEVICE_UDID / FANDHE_IOS_TEAM_ID を設定して再実行する"
fi

for tool in xcrun xcodebuild rustup cargo xcodegen; do
  command -v "${tool}" >/dev/null 2>&1 || fail "${tool} が見つかりません（Xcode / xcodegen 導入を確認してください）"
done
# `script`（BSD 版、疑似 TTY 割り当て用。フェーズ 5 の devicectl console 出力取得で
# 使用）は macOS 標準搭載のため、上記ツール群とは別に単独で検査する（Bugbot 指摘対応）。
command -v script >/dev/null 2>&1 || fail "script コマンドが見つかりません（macOS 標準搭載のはずです。PATH を確認してください）"

# コールドスタート判定の前提となる `devicectl device process launch
# --terminate-existing`（フェーズ 5 参照）が使えることを事前に検査する。
# 非対応の devicectl（Xcode 15 未満相当）のまま進行すると、フェーズ 5 の
# COLD_START_MS 抽出失敗時に「アプリの出力を確認してください」という
# 見当違いのメッセージで fail し、真因（devicectl バージョン）へ気付きにくい
# ため、ここで先回りして案内する（Review 指摘対応）。
#
# `--terminate-existing` は Xcode 15 で追加されたが、フェーズ 5 の計測本体が
# 実際に使う `--console`（起動したプロセスの標準出力/標準エラー出力を中継し、
# COLD_START_MS= 行の抽出元になる）は Xcode 16 で追加されたオプションであり、
# Xcode 15 環境では `--terminate-existing` のみ通過してこのゲートを素通りし、
# フェーズ 5 で `--console` 自体が無視される（または devicectl がエラーになる）
# ことで誤解を招く抽出失敗メッセージに落ちる。そのため両オプションを個別に
# 検査し、Xcode 16+ を要求する（Review 指摘対応）。

# `set -o pipefail` 下で `devicectl ... --help | grep -q ...` を直接パイプすると、
# grep が早期にマッチしてパイプを閉じた際に devicectl 側が SIGPIPE で
# 非ゼロ終了し、フラグが実際に存在していても fail 分岐へ入る恐れがある
# （false-fail）。先に出力をキャプチャしてから grep することで devicectl の
# 終了コードにパイプラインの結果が影響されないようにする（Review 指摘対応）。
devicectl_help="$(xcrun devicectl device process launch --help 2>&1 || true)"
printf '%s\n' "${devicectl_help}" | grep -q -- "--terminate-existing" \
  || fail "お使いの devicectl は --terminate-existing に対応していません
  （Xcode 16 以上が必要です）。Xcode のバージョンを確認してください。"
printf '%s\n' "${devicectl_help}" | grep -q -- "--console" \
  || fail "お使いの devicectl は --console に対応していません
  （本スクリプトの計測本体（フェーズ 5）はこのオプションで COLD_START_MS=
  出力を取得するため、Xcode 16 以上が必要です）。Xcode のバージョンを
  確認してください。"

device_udid="${FANDHE_IOS_DEVICE_UDID:-}"
team_id="${FANDHE_IOS_TEAM_ID:-}"
[ -n "${device_udid}" ] || fail "FANDHE_IOS_DEVICE_UDID が未設定です（対象実機の UDID を指定してください。値はログへ出力されません）。"
[ -n "${team_id}" ] || fail "FANDHE_IOS_TEAM_ID が未設定です（Apple Developer Team ID。TASK-18.2 完了後に人間担当が設定します）。"

# --- フェーズ 2: 実機の接続確認（devicectl） --------------------------------
# UDID をそのまま `grep` へ渡すため、英数字・ハイフンのみで構成されることを
# 検証してから使う（A03 対策: 外部入力を無検証でコマンドへ展開しない）。
if [[ ! "${device_udid}" =~ ^[0-9A-Fa-f-]+$ ]]; then
  fail "FANDHE_IOS_DEVICE_UDID の形式が不正です（英数字・ハイフンのみ許容）。"
fi

device_list="$(xcrun devicectl list devices 2>/dev/null || true)"
if ! printf '%s\n' "${device_list}" | grep -qF "${device_udid}"; then
  fail "指定された UDID の実機が devicectl から見つかりません。USB/Wi-Fi 接続・
  デバイスの信頼済み状態を確認してください（人間担当手順）。"
fi
log "実機接続確認 OK"

# --- フェーズ 3: fandhe 側ビルド・バンドル・実機署名 ------------------------
# 実機向け FandheIosApp は本 issue の時点で xcodeproj ターゲット化されておらず
# （platforms/apple/project.yml は FandheWatchApp/FandheVisionApp のみを定義、
# TASK-3.4 系列参照）、scripts/ios/build-simulator-app.sh（aarch64-apple-ios-sim
# 固定・ad-hoc 署名専用）と同じ「cargo build → .app 手組み → codesign」の手順を、
# 実機ターゲット（aarch64-apple-ios）・実署名（Team ID・プロビジョニング
# プロファイル）向けに本スクリプト内で行う。ロジックの重複を避けるため
# build-simulator-app.sh 本体は改修せず、本スクリプトに実機専用の手順として
# 実装する（シミュレータ検証（TASK-1.4）・実機計測（本 issue）は目的が異なり、
# 署名要件（ad-hoc / 実署名）も非対称なため、無理に共通化しない判断）。
fandhe_target="aarch64-apple-ios"
fandhe_bin_name="fandhe-egui-proxy-desktop"
fandhe_app_name="FandheIosApp"
fandhe_info_plist_src="${repo_root}/platforms/apple/FandheIosApp/Resources/Info.plist"
[ -f "${fandhe_info_plist_src}" ] || fail "Info.plist が見つかりません: ${fandhe_info_plist_src}"

fandhe_out_dir="${repo_root}/target/ios-device"
mkdir -p "${fandhe_out_dir}"
fandhe_app_dir="${fandhe_out_dir}/${fandhe_app_name}.app"

provisioning_profile="${FANDHE_IOS_PROVISIONING_PROFILE:-}"
codesign_identity="${FANDHE_IOS_CODESIGN_IDENTITY:-}"
[ -n "${provisioning_profile}" ] || fail "FANDHE_IOS_PROVISIONING_PROFILE が未設定です
（TASK-18.2 完了後に人間担当が .mobileprovision のパスを指定してください。値・
ファイル内容はリポジトリ・ログに一切含めません）。"
[ -n "${codesign_identity}" ] || fail "FANDHE_IOS_CODESIGN_IDENTITY が未設定です
（例: \"Apple Development: <氏名> (<TEAMID>)\"。security find-identity -p codesigning
-v で確認できます）。"
[ -f "${provisioning_profile}" ] || fail "FANDHE_IOS_PROVISIONING_PROFILE で指定されたファイルが見つかりません（値はログへ出力しません）。"

log "rustup target add ${fandhe_target}"
rustup target add "${fandhe_target}" >/dev/null

# `cold-start-probe` feature（既定 off、`crates/fandhe-egui-proxy/Cargo.toml` 参照）を
# 本スクリプトから明示的に有効化する。無効のままだと `ios.rs` の `run()` は
# `ColdStartProbe` を経由せず `COLD_START_MS=` を出力しないため（Review 指摘対応）、
# 計測を行う本スクリプトの実行時のみ opt-in する。
log "cargo build -p fandhe-egui-proxy --bin ${fandhe_bin_name} --target ${fandhe_target} --profile release --features fandhe-egui-proxy/cold-start-probe"
cargo build --manifest-path "${repo_root}/Cargo.toml" -p fandhe-egui-proxy --bin "${fandhe_bin_name}" \
  --target "${fandhe_target}" --profile release --features fandhe-egui-proxy/cold-start-probe

fandhe_built_binary="${repo_root}/target/${fandhe_target}/release/${fandhe_bin_name}"
[ -f "${fandhe_built_binary}" ] || fail "ビルド成果物が見つかりません: ${fandhe_built_binary}"

log "アプリバンドル組み立て: ${fandhe_app_dir}"
rm -rf "${fandhe_app_dir}"
mkdir -p "${fandhe_app_dir}"
cp "${fandhe_built_binary}" "${fandhe_app_dir}/${fandhe_app_name}"
cp "${fandhe_info_plist_src}" "${fandhe_app_dir}/Info.plist"
cp "${fandhe_info_plist_src%/*}/PrivacyInfo.xcprivacy" "${fandhe_app_dir}/PrivacyInfo.xcprivacy"
# 実機インストールにはプロビジョニングプロファイルの同梱が必須（TeamID・
# entitlements の解決に使われる。中身（証明書チェーン）はリポジトリに書き出さず、
# バンドル内へコピーするのみ）。
cp "${provisioning_profile}" "${fandhe_app_dir}/embedded.mobileprovision"

# Xcode 経由の署名と異なり、手組みバンドルの codesign は provisioning profile から
# entitlements（application-identifier・Team ID 等）を自動継承しない。付けないまま
# 実機（devicectl）へインストール・起動すると entitlement 不一致で失敗し得るため、
# provisioning profile から entitlements plist を抽出して明示的に渡す
# （Review 指摘対応）。抽出結果は gitignore 済みの target/ 配下（本スクリプト冒頭で
# 定義済みの fandhe_out_dir）にのみ書き出し、リポジトリへは一切含めない。
# codesign 完了後は不要になるため削除する（値自体は個体識別子・秘密情報ではないが、
# 一時ファイルを不要に残さない方針）。
entitlements_plist="${fandhe_out_dir}/entitlements.plist"
security cms -D -i "${provisioning_profile}" > "${fandhe_out_dir}/embedded-profile.plist"
/usr/libexec/PlistBuddy -x -c "Print :Entitlements" "${fandhe_out_dir}/embedded-profile.plist" > "${entitlements_plist}"
rm -f "${fandhe_out_dir}/embedded-profile.plist"

log "実署名: codesign --sign \"<identity>\" --entitlements <profile 抽出分> --force ${fandhe_app_dir}"
codesign --sign "${codesign_identity}" --entitlements "${entitlements_plist}" --force "${fandhe_app_dir}"
codesign --verify --verbose=2 "${fandhe_app_dir}"
rm -f "${entitlements_plist}"
log "fandhe 側ビルド・署名 OK: ${fandhe_app_dir}"

# --- フェーズ 4: ネイティブ比較アプリ（SwiftUI）のビルド・実機署名 ----------
native_apple_dir="${repo_root}/benchmarks/coldstart/ios-native"
native_project_yml="${native_apple_dir}/project.yml"
native_xcodeproj="${native_apple_dir}/FandheColdStartNative.xcodeproj"
[ -f "${native_project_yml}" ] || fail "比較アプリの project.yml が見つかりません: ${native_project_yml}"

log "xcodegen generate --spec ${native_project_yml}"
xcodegen generate --spec "${native_project_yml}" --project "${native_apple_dir}"
[ -d "${native_xcodeproj}" ] || fail "比較アプリの xcodeproj 生成に失敗しました: ${native_xcodeproj}"

native_build_dir="${repo_root}/target/ios-device-native"
mkdir -p "${native_build_dir}"
# DerivedData の既定パス（~/Library/Developer/Xcode/DerivedData/...）は非決定的な
# ハッシュ付きサブディレクトリを含み後段の成果物探索が脆くなるため、
# `CONFIGURATION_BUILD_DIR` でリポジトリ内の決定的パスへ出力先を固定する。
log "xcodebuild build -project ${native_xcodeproj} -scheme FandheColdStartNative -configuration Release -destination id=${device_udid}"
# scheme ベースビルドは `-configuration` を省略すると既定で Debug になる。fandhe 側は
# `--profile release`（cargo）でビルドしているため、ここで明示的に Release を指定して
# 両実装のビルド構成を揃えないと、ネイティブ側は最適化なしバイナリとの比較になり
# コールドスタート計測が非対称になる（Bugbot 指摘対応、review id 4834058437）。
xcodebuild build \
  -project "${native_xcodeproj}" \
  -scheme "FandheColdStartNative" \
  -configuration Release \
  -destination "id=${device_udid}" \
  -allowProvisioningUpdates \
  -derivedDataPath "${native_build_dir}/DerivedData" \
  CONFIGURATION_BUILD_DIR="${native_build_dir}/build" \
  DEVELOPMENT_TEAM="${team_id}" \
  CODE_SIGN_IDENTITY="${codesign_identity}"

# --- フェーズ 5: インストール・計測本体（devicectl） ------------------------
# `devicectl device process launch --console` は起動したプロセスの標準出力/
# 標準エラー出力を呼び出し元の端末へ中継する（Xcode 16+。冒頭コメント・
# フェーズ 1 の前提条件チェック参照）。fandhe/ネイティブとも
# `COLD_START_MS=<value>` 行を出力する契約（本ファイル冒頭コメント参照）のため、
# この出力から抽出する。
median() {
  local sorted
  sorted="$(printf '%s\n' "$@" | sort -n)"
  local count="$#"
  local mid_index=$(( (count + 1) / 2 ))
  printf '%s\n' "${sorted}" | sed -n "${mid_index}p"
}

extract_cold_start_ms() {
  # `--console` の出力から `COLD_START_MS=<数値>;` の数値部分のみを抽出する。
  # `script`（疑似 TTY 経由の記録、下記 run_trials 参照）は CR を挟むことがあるため
  # 先に取り除く。末尾の `;`（fandhe 側 `ios.rs`・ネイティブ側 `ContentView.swift`
  # 双方が出力する行終端マーカー）を正規表現へ必須で含めることで、pty の折り返し幅で
  # 数値が途中改行された場合に断片（例: `COLD_START_MS=12`）を誤って正しい値として
  # 抽出することを防ぎ、抽出失敗として fail-fast させる（Bugbot 指摘対応、
  # review id 4834058437）。
  tr -d '\r' | grep -oE "COLD_START_MS=[0-9]+;" | tail -n1 | sed -E 's/^COLD_START_MS=([0-9]+);$/\1/'
}

run_trials() {
  local label="$1"
  local bundle_id="$2"
  local app_path="$3"
  local -a values=()
  local i
  log "adb 相当（devicectl）install: ${app_path}"
  # `run_trials` は戻り値（中央値）を `$( ... )`（コマンド置換）経由で受け取るため、
  # `devicectl` の進捗出力が標準出力に漏れると戻り値へ混入して数値判定が壊れる
  # （scripts/android/measure-cold-start.sh の同種の Review 指摘と同じ理由）。
  # 進捗はここでは案内しないため /dev/null へ捨てる（失敗時は非 0 終了で
  # set -e により停止するので情報は失われない）。
  xcrun devicectl device install app --device "${device_udid}" "${app_path}" >/dev/null
  for ((i = 1; i <= trials; i++)); do
    log "  [${label}] 試行 ${i}/${trials}"
    # `devicectl device process terminate` は `--bundle-id` を受け付けず `--pid`
    # のみを受理する。かつては（誤って）`--bundle-id` を渡して `|| true` で
    # エラーを握り潰していたため、無音の no-op になり実機上のプロセスが
    # 生き残ったまま次の試行へ進んでいた（Bugbot 指摘対応）。ウォッチドッグは
    # ローカルの `--console` ホストプロセスのみを kill し、実機上のアプリ本体は
    # 終了させない。`--pid` 単発 terminate 自体は存在するが、使うには
    # `devicectl device info processes` から対象 bundle-id の PID を毎回検索する
    # 手順が必要で、その出力形式に依存する分脆い。そのため事前 terminate には
    # 頼らず、下記の起動コマンド側の `--terminate-existing` で「起動直前に同一
    # bundle-id の実行中インスタンスを終了してから起動する」ことを devicectl 自身に
    # 保証させる方式に切り替える（2 回目以降の試行を確実にコールドスタートにする）。
    sleep 1
    local value
    # `--console` はプロセスが終了するまで標準出力/標準エラー出力を中継し続ける。
    # fandhe / ネイティブ比較アプリのいずれも `COLD_START_MS=` 出力後もプロセスは
    # 起動状態を維持するため（ユーザー操作を待つ通常の GUI アプリ）、放置すると
    # `devicectl` コマンド自体が終了せず本スクリプトがハングする。バックグラウンド
    # 実行 + ウォッチドッグで一定時間後に強制終了することで、`COLD_START_MS=` 行を
    # 受け取った後にハングしないようにする（macOS には GNU coreutils の `timeout`
    # が既定で入っていないため自前で実装する）。
    local launch_pid
    # `devicectl device process launch --console` はアプリの標準出力/標準エラー出力を
    # 中継する際に、呼び出し元の標準出力が TTY であるかどうかで挙動を変える実装が
    # 確認されている。バックグラウンド実行 + パイプ/リダイレクトのみ（TTY 無し）の
    # 状態では `print`/`eprintln` 出力（`COLD_START_MS=` 行を含む）がストリームされず、
    # 抽出が毎回失敗し得る（Bugbot 指摘対応、review id 4834058437）。`script -q` で
    # 疑似 TTY を割り当てた上で devicectl を起動し、その TTY 越しの出力をファイルへ
    # 記録することで、TTY 有無に依存する分岐を回避する。
    script -q "${launch_console_tmp_dir}/.launch-console.tmp" \
      xcrun devicectl device process launch --console --terminate-existing --device "${device_udid}" "${bundle_id}" \
      >/dev/null 2>&1 &
    launch_pid=$!
    # `script` は子プロセス（xcrun 経由の devicectl）へ SIGTERM を転送しないため、
    # `launch_pid`（script 自身）だけを kill してもプロセスツリー
    # （script → xcrun → devicectl）の下位が孤児として残り続け、devicectl が
    # pty のスレーブ側を握ったまま次回試行の `--console` アタッチと競合しうる。
    # `pgrep -P` でプロセスツリーを辿り、末端（devicectl）から順に TERM を送る
    # （Bugbot 指摘対応、review id 4834058437）。
    ( sleep 15
      local descendant child
      for descendant in $(pgrep -P "${launch_pid}" 2>/dev/null || true); do
        for child in $(pgrep -P "${descendant}" 2>/dev/null || true); do
          kill -TERM "${child}" >/dev/null 2>&1 || true
        done
        kill -TERM "${descendant}" >/dev/null 2>&1 || true
      done
      kill -TERM "${launch_pid}" >/dev/null 2>&1 || true
    ) &
    local watchdog_pid=$!
    wait "${launch_pid}" 2>/dev/null || true
    kill "${watchdog_pid}" >/dev/null 2>&1 || true
    value="$(extract_cold_start_ms < "${launch_console_tmp_dir}/.launch-console.tmp" || true)"
    rm -f "${launch_console_tmp_dir}/.launch-console.tmp"
    [ -n "${value}" ] || fail "${label} の COLD_START_MS 抽出に失敗しました（試行 ${i}）。
    アプリが COLD_START_MS= 行を出力しているか確認してください。"
    log "    COLD_START_MS=${value}"
    values+=("${value}")
    sleep 2
  done
  median "${values[@]}"
}

fandhe_bundle_id="com.fandhe.iosapp"
native_bundle_id="com.fandhe.coldstart.native"
native_app_build_dir="${native_build_dir}/build/FandheColdStartNative.app"

log "fandhe 実装の計測を開始します（${trials} 回）"
fandhe_median="$(run_trials "fandhe" "${fandhe_bundle_id}" "${fandhe_app_dir}")"

log "ネイティブ比較実装の計測を開始します（${trials} 回）"
[ -d "${native_app_build_dir}" ] || fail "ネイティブ比較アプリの .app が見つかりません: ${native_app_build_dir}"
native_median="$(run_trials "native" "${native_bundle_id}" "${native_app_build_dir}")"

# --- フェーズ 6: 後片付け ------------------------------------------------------
xcrun devicectl device uninstall app --device "${device_udid}" "${fandhe_bundle_id}" >/dev/null 2>&1 || true
xcrun devicectl device uninstall app --device "${device_udid}" "${native_bundle_id}" >/dev/null 2>&1 || true

# --- フェーズ 7: 判定・整形出力 -----------------------------------------------

absolute_gate_ms=400
absolute_verdict="NG"
if [ "${fandhe_median}" -le "${absolute_gate_ms}" ]; then
  absolute_verdict="OK"
fi

relative_pct="N/A"
relative_verdict="参考（絶対値主判定）"
if [ "${native_median}" -gt 0 ]; then
  relative_pct=$(( (fandhe_median - native_median) * 100 / native_median ))
  if [ "${relative_pct}" -le 30 ]; then
    relative_verdict="OK（参考）"
  else
    relative_verdict="NG（参考、絶対値主判定を優先）"
  fi
fi

log "=== 計測結果（中央値、${trials} 回試行） ==="
log "fandhe 実装      : ${fandhe_median} ms"
log "ネイティブ比較実装: ${native_median} ms"
log "絶対値判定（400ms 以内）: ${absolute_verdict}"
log "相対値（ネイティブ比、参考指標）: ${relative_pct}% -> ${relative_verdict}"
log ""
log "結果記録テンプレートへの転記値:"
log "  | iOS | ${fandhe_median} | ${native_median} | ${relative_pct} | ${absolute_verdict} |"

log "完了。上記の結果を控えてください。"
