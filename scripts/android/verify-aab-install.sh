#!/usr/bin/env bash
# TASK-11.3（REQ-11 / M4、Issue #246）: TASK-11.2 で完成した `.aab` 生成・開発署名
# パイプライン（`scripts/android/build-aab.sh`）の成果物を、実際に Android 端末へ
# インストール・起動確認する。
#
# REQ-11 受け入れ基準「単一コマンドから Android `.aab`（開発用署名鍵）が生成され、
# エミュレータへインストール・起動できる」のうち、「単一コマンドから `.aab` が
# 生成・署名される」までは TASK-11.2 系列（#224）で達成済み。本スクリプトは
# 残りの「インストール・起動できる」を実証する。
#
# 位置づけ（重要な前提）:
#   - `.aab` は adb で直接インストールできない（設計書
#     `docs/architecture/android-aab-pipeline.md` §6・§8 参照）。Google 公式ツール
#     bundletool（`scripts/android/versions.env` BUNDLETOOL_* が SSOT）で
#     `.aab` → `.apks`（対象端末向け split APK 集合）へ変換してからインストールする。
#   - 本ホストには KVM が無く（TASK-1.6 / #230 で確認済み）実用速度のエミュレータが
#     動かせないため、実機（Pixel 10a）を
#     主経路とする。ただしスクリプト自体は adb 接続端末に依存しないデバイス非依存
#     設計であり、エミュレータでもそのまま動作する。
#   - REQ-11 の受け入れ基準が「開発用署名鍵」を明示するため、未署名 `.aab` での
#     代替検証はしない（フェーズ 1 で fail-closed）。
#
# 呼び出し元: ローカル開発者（人間担当: 実機の USB 接続・USB デバッグ許可・keystore
# 準備）が直接実行する。CI 組み込みは行わない（CI runner にデバイスが
# 存在しないため。`.claude/rules/ci.md` の CI 制約と独立に対象外）。
#
# 前提: scripts/android/setup-android-env.sh 実行済み（JDK・SDK・NDK・Gradle・
# bundletool・cargo-ndk 導入済み）。開発署名鍵は
# docs/operations/android-aab-signing.md の手順で事前に生成しておくこと。
#
# 環境変数インターフェース（build-aab.sh / verify-android-launch.sh の
# FANDHE_ANDROID_* と同じ設計方針。build-aab.sh へそのままパススルーする）:
#   FANDHE_ANDROID_KEYSTORE          必須。開発署名鍵（.jks / PKCS12）のパス。
#                                    未設定時は fail-closed（下記フェーズ 1 参照）。
#                                    REQ-11 が「開発用署名鍵」を明示しているため、
#                                    未署名 .aab での代替検証は行わない。
#   FANDHE_ANDROID_KEYSTORE_PASSWORD 必須（FANDHE_ANDROID_KEYSTORE 設定時）。
#                                    keystore パスフレーズ。ログ・エラーメッセージへ
#                                    一切出力しない（security.md 準拠）。
#   FANDHE_ANDROID_KEY_ALIAS         既定: "fandhe-dev"。鍵エイリアス。
#   FANDHE_ANDROID_KEY_PASSWORD      既定: FANDHE_ANDROID_KEYSTORE_PASSWORD と同値
#                                    （build-aab.sh / build.gradle.kts と同じ既定）。
#   FANDHE_ANDROID_ABIS              既定: "arm64-v8a"（実機（Pixel 10a）向けの
#                                    単一 ABI。verify-android-launch.sh と同じ既定）。
#   FANDHE_ANDROID_SERIAL            既定: 空（`adb devices` で 1 台のみ接続されている
#                                    前提）。複数台接続時の対象端末指定。値はログへ
#                                    出力しない（個体識別子は記録しない方針）。
#   FANDHE_ANDROID_KEEP_APP          既定: 空（起動確認後に adb uninstall する）。
#
# 使い方:
#   FANDHE_ANDROID_KEYSTORE=/path/to/fandhe-dev.jks \
#   FANDHE_ANDROID_KEYSTORE_PASSWORD='<パスフレーズ>' \
#     scripts/android/verify-aab-install.sh
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと JAVA_HOME / ANDROID_HOME /
# ANDROID_NDK_HOME / GRADLE_HOME / BUNDLETOOL_JAR の export を行う
# （build-aab.sh / verify-android-launch.sh と同じ前提）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

application_id="com.fandhe.androidapp"
# GameActivity 方式（TASK-9.3-S1 / #242 で NativeActivity から確定。
# platforms/android/app/src/main/AndroidManifest.xml SSOT、verify-android-launch.sh と
# 同じ値）。
activity_name="com.fandhe.androidapp.MainActivity"
aab_path="${repo_root}/platforms/android/app/build/outputs/bundle/release/app-release.aab"
apks_out_dir="${repo_root}/target/android/verify-aab-install"
apks_path="${apks_out_dir}/app.apks"
screenshot_dir="${repo_root}/_/verification/task-11.3"

keystore_path="${FANDHE_ANDROID_KEYSTORE:-}"
keystore_password="${FANDHE_ANDROID_KEYSTORE_PASSWORD:-}"
key_alias="${FANDHE_ANDROID_KEY_ALIAS:-fandhe-dev}"
key_password="${FANDHE_ANDROID_KEY_PASSWORD:-${keystore_password}}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
serial="${FANDHE_ANDROID_SERIAL:-}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"

log() {
  echo "[verify-aab-install] $*"
}

fail() {
  echo "[verify-aab-install] ERROR: $*" >&2
  exit 1
}

# adb へのターゲット端末指定を一元化する（verify-android-launch.sh と同じ理由）。
adb_cmd() {
  if [ -n "${serial}" ]; then
    adb -s "${serial}" "$@"
  else
    adb "$@"
  fi
}

# --- フェーズ 1: 前提検査（fail-fast） ---------------------------------------
command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません（rustup 導入後に再実行してください）"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] \
  || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
[ -d "${GRADLE_HOME}" ] \
  || fail "GRADLE_HOME（${GRADLE_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
[ -d "${JAVA_HOME}" ] \
  || fail "JAVA_HOME（${JAVA_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"
command -v gradle >/dev/null 2>&1 || fail "gradle が見つかりません（PATH に GRADLE_HOME/bin が含まれているか確認してください）"
command -v adb >/dev/null 2>&1 || fail "adb が見つかりません（PATH に ANDROID_HOME/platform-tools が含まれているか確認してください）"
[ -f "${BUNDLETOOL_JAR}" ] \
  || fail "bundletool（${BUNDLETOOL_JAR}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"

# REQ-11 受け入れ基準が「開発用署名鍵」を明示するため、未署名 .aab での代替検証は
# 行わない（fail-closed。docs/operations/android-aab-signing.md の keystore 生成手順を案内）。
if [ -z "${keystore_path}" ]; then
  fail "FANDHE_ANDROID_KEYSTORE が未設定です。REQ-11 受け入れ基準（開発用署名鍵での
  インストール確認）を満たすため、開発署名鍵での実行が必須です。
  docs/operations/android-aab-signing.md の keystore 生成手順を参照してください。"
fi
if [ -z "${keystore_password}" ]; then
  fail "FANDHE_ANDROID_KEYSTORE_PASSWORD が未設定です（FANDHE_ANDROID_KEYSTORE 設定時は必須）"
fi
[ -f "${keystore_path}" ] || fail "FANDHE_ANDROID_KEYSTORE で指定された keystore が見つかりません: ${keystore_path}"
# build-aab.sh へ絶対パスとして引き継ぐ（build-aab.sh 内の file() 解決基準と同じ理由）。
keystore_path="$(cd -- "$(dirname -- "${keystore_path}")" >/dev/null 2>&1 && pwd -P)/$(basename -- "${keystore_path}")"

# --- フェーズ 2: デバイス検出（人間担当手順の明示） --------------------------
# verify-android-launch.sh と同じロジック（adb デーモン起動時の状態行除外・
# unauthorized/複数台の扱い）を踏襲する。
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"

if [ -z "${device_lines}" ]; then
  fail "接続されている Android 端末がありません。Pixel 10a を USB 接続し、
  開発者向けオプション > USB デバッグ を有効化してください（人間担当手順）。
  接続後、端末側の『USB デバッグを許可しますか』ダイアログを承認してから
  再実行してください。"
fi

device_count="$(printf '%s\n' "${device_lines}" | grep -c "device$" || true)"

if [ "${device_count}" -lt 1 ] && printf '%s\n' "${device_lines}" | grep -q "unauthorized"; then
  fail "接続されている端末が unauthorized 状態です。端末側の
  『USB デバッグを許可しますか』ダイアログを承認してください（人間担当手順）。"
fi

if [ -z "${serial}" ] && [ "${device_count}" -gt 1 ]; then
  fail "複数台の端末が接続されています。FANDHE_ANDROID_SERIAL で対象端末を指定してください
  （値はログへ出力されません）。"
fi
if [ "${device_count}" -lt 1 ]; then
  fail "device 状態の端末が見つかりません（unauthorized/offline のみ検出）。
  端末側のダイアログ承認・USB 接続状態を確認してください。"
fi

if [ -n "${serial}" ]; then
  serial_state="$(printf '%s\n' "${device_lines}" | awk -v s="${serial}" '$1 == s { print $2 }')"
  if [ -z "${serial_state}" ]; then
    fail "FANDHE_ANDROID_SERIAL で指定された端末が adb devices の出力に見つかりません。
    接続状態・シリアル指定を確認してください（値はログへ出力されません）。"
  fi
  if [ "${serial_state}" != "device" ]; then
    fail "FANDHE_ANDROID_SERIAL で指定された端末が device 状態ではありません
    （状態: ${serial_state}）。unauthorized の場合は端末側の
    『USB デバッグを許可しますか』ダイアログを承認してください（人間担当手順）。"
  fi
fi

log "デバイス検出 OK（${device_count} 台、device 状態）"

# --- フェーズ 3: 署名済み .aab 生成（build-aab.sh へ委譲） --------------------
# cargo-ndk クロスビルド・Gradle bundleRelease・署名は build-aab.sh の責務。
# ここで二重実装しない（docs/operations/android-build-pipeline.md 4.3 節と
# 同じ「生成ロジックは 1 箇所」の方針）。
log "build-aab.sh へ委譲: 署名済み .aab を生成します"
FANDHE_ANDROID_ABIS="${abis}" \
FANDHE_ANDROID_KEYSTORE="${keystore_path}" \
FANDHE_ANDROID_KEYSTORE_PASSWORD="${keystore_password}" \
FANDHE_ANDROID_KEY_ALIAS="${key_alias}" \
FANDHE_ANDROID_KEY_PASSWORD="${key_password}" \
  "${script_dir}/build-aab.sh"

[ -f "${aab_path}" ] || fail ".aab の生成に失敗しました: ${aab_path}"
log ".aab 生成 OK: ${aab_path}"

# --- フェーズ 4: .apks 生成（bundletool build-apks） --------------------------
# bundletool は「.aab の署名」ではなく「.apks 生成時に指定した鍵」で split APK 群に
# 署名する（.aab 自体の署名ブロックはデバイスのインストール検証には使われない）。
# REQ-11 受け入れ基準（開発用署名鍵で .aab が生成される）はフェーズ 3 の .aab 自体の
# 署名で満たし、デバイスへのインストールを通すためにフェーズ 4 でも同じ開発鍵を
# 明示的に指定する（2 箇所での指定が必要な理由）。
mkdir -p "${apks_out_dir}"
rm -f "${apks_path}"

# keystore/key パスワードは `--ks-pass=file:` / `--key-pass=file:` 形式でのみ渡す
# （`pass:` 直渡しはプロセス一覧・シェル履歴への露出リスクがあるため禁止。
# security.md 準拠）。umask 077 の一時ファイルへ書き、trap で確実に削除する。
ks_pass_file=""
key_pass_file=""
cleanup_pass_files() {
  [ -n "${ks_pass_file}" ] && rm -f "${ks_pass_file}"
  [ -n "${key_pass_file}" ] && rm -f "${key_pass_file}"
}
trap cleanup_pass_files EXIT INT TERM

ks_pass_file="$(umask 077 && mktemp)"
key_pass_file="$(umask 077 && mktemp)"
printf '%s' "${keystore_password}" >"${ks_pass_file}"
printf '%s' "${key_password}" >"${key_pass_file}"

log "bundletool build-apks（--connected-device、開発署名鍵で署名）"
declare -a bundletool_device_args=()
if [ -n "${serial}" ]; then
  bundletool_device_args+=(--device-id "${serial}")
fi
"${JAVA_HOME}/bin/java" -jar "${BUNDLETOOL_JAR}" build-apks \
  --bundle="${aab_path}" \
  --output="${apks_path}" \
  --connected-device \
  "${bundletool_device_args[@]}" \
  --ks="${keystore_path}" \
  --ks-pass="file:${ks_pass_file}" \
  --ks-key-alias="${key_alias}" \
  --key-pass="file:${key_pass_file}"

[ -f "${apks_path}" ] || fail ".apks の生成に失敗しました: ${apks_path}"
log ".apks 生成 OK: ${apks_path}"

# --- フェーズ 5: インストール（bundletool install-apks） ----------------------
# 前回検証の残置があると署名不一致（別鍵で再ビルドした場合）でエラーになりうるため、
# 事前に uninstall を試行する（verify-android-launch.sh と同じ理由。未インストール時は
# no-op、エラーは無視してよい）。
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true

log "bundletool install-apks"
declare -a bundletool_install_device_args=()
if [ -n "${serial}" ]; then
  bundletool_install_device_args+=(--device-id "${serial}")
fi
"${JAVA_HOME}/bin/java" -jar "${BUNDLETOOL_JAR}" install-apks \
  --apks="${apks_path}" \
  "${bundletool_install_device_args[@]}"

# --- フェーズ 6: 起動 ----------------------------------------------------------
# logcat をここでクリアし、以降の起動判定（フェーズ 7）を「今回の起動」にスコープする
# （verify-android-launch.sh と同じ理由）。
adb_cmd logcat -c

log "adb shell am start -n ${application_id}/${activity_name}"
adb_cmd shell am start -n "${application_id}/${activity_name}"

# GameActivity（MainActivity.java 経由）の Vulkan 初期化・ウィンドウ表示には多少の時間を要するため、
# 判定前に待機する（固定 5 秒。verify-android-launch.sh と同じ簡易マージン）。
sleep 5

# --- フェーズ 7: 起動判定（機械的検証、verify-android-launch.sh と同じロジック） ---
# (a) プロセス生存確認
pid="$(adb_cmd shell pidof "${application_id}" | tr -d '\r\n[:space:]' || true)"
if [ -z "${pid}" ]; then
  fail "プロセスが確認できません（起動直後にクラッシュした可能性）。
  logcat を確認してください: adb logcat -d | grep -i ${application_id}"
fi
log "  - プロセス生存確認 OK（pid=${pid}）"

# (b) logcat 内のクラッシュ痕跡確認
crash_log="$(adb_cmd logcat -d --pid="${pid}" 2>/dev/null | grep -iE "FATAL EXCEPTION|tombstone|libc.*abort|Fatal signal|SIGSEGV|SIGABRT|SIGBUS|SIGILL" || true)"
if [ -n "${crash_log}" ]; then
  fail "logcat にクラッシュ痕跡が見つかりました:
${crash_log}"
fi
log "  - logcat クラッシュ痕跡なしを確認 OK"

# (c) Activity の RESUMED 状態確認
resumed_line="$(adb_cmd shell dumpsys activity activities 2>/dev/null \
  | grep -E "mResumedActivity|topResumedActivity" || true)"
if ! printf '%s\n' "${resumed_line}" | grep -q "${application_id}"; then
  fail "Activity が RESUMED 状態ではありません（起動失敗・フォアグラウンド遷移失敗の疑い）。
  dumpsys 抜粋（mResumedActivity/topResumedActivity）:
${resumed_line}"
fi
log "  - Activity RESUMED 状態を確認 OK"

# --- フェーズ 8: 証跡取得・後片付け --------------------------------------------
mkdir -p "${screenshot_dir}"
screenshot_path="${screenshot_dir}/install-launch-screenshot.png"
log "adb exec-out screencap -p > ${screenshot_path}"
adb_cmd exec-out screencap -p > "${screenshot_path}"
[ -s "${screenshot_path}" ] || fail "スクリーンショットの取得に失敗しました: ${screenshot_path}"
log "  - スクリーンショット取得 OK: ${screenshot_path}"

if [ -z "${keep_app}" ]; then
  log "adb uninstall ${application_id}"
  adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
else
  log "FANDHE_ANDROID_KEEP_APP 指定のため端末にアプリを残置します"
fi

log "インストール・起動確認 OK: ${application_id}/${activity_name}（.aab → .apks 経由）"
