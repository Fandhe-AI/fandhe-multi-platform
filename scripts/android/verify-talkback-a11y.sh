#!/usr/bin/env bash
# TASK-9.3-S3（REQ-9 / M4、Issue #244）: TalkBack 読み上げ実機検証の支援スクリプト。
#
# 位置づけ: TASK-9.3-S2（#243、PR #505）で accesskit_android（GameActivity 限定）との
# 依存チェーン・feature 整合を機械的に確認済み。本スクリプトはその成果物を実機へ
# インストール・起動し、`adb shell uiautomator dump` による a11y ツリーの機械的検証
# （accesskit_android の InjectingAdapter が Android の Accessibility フレームワークへ
# 実際にノードを公開していることの証跡）までを担う。TalkBack の音声読み上げそのものの
# 確認は Linux 環境では実施不可能なため人間担当とし、本スクリプトはそこへ至るまでの
# ビルド・インストール・起動・機械的証跡取得を自動化する
# （AccessKit 統合の手順書を実装化。検証記録は非公開）。
#
# `scripts/android/verify-android-launch.sh`（TASK-1.6 / #230）との関係:
#   フェーズ構成（前提検査 → デバイス検出 → クロスビルド → APK 生成 → インストール・起動
#   → 起動判定 → 証跡取得 → 後片付け）と fail-fast の判定ロジックは同スクリプトの構成を
#   踏襲する。共通化（関数抽出）は行わず本スクリプトへ複製している。理由:
#   verify-android-launch.sh は「起動確認」のみがスコープで a11y ツリー検証を持たず、
#   本スクリプトは a11y 固有のフェーズ（フェーズ 6b）を追加で持つため、共通部を
#   切り出すと両スクリプトの独立した進化（起動確認側の変更が a11y 側に無関係な影響を
#   与える等）を妨げるおそれがあるとの判断による。
#
# 呼び出し元: ローカル開発者（人間担当: 実機の USB 接続・USB デバッグ許可・TalkBack の
# 有効化・音声読み上げの目視/聴取確認）が直接実行する。CI 組み込みは行わない
# （実機接続が前提のため、ci.md の CI 制約とも独立）。
#
# 前提: scripts/android/setup-android-env.sh 実行済み（JDK・SDK・NDK・Gradle・
# cargo-ndk 導入済み）。
#
# 環境変数インターフェース（verify-android-launch.sh の FANDHE_ANDROID_* と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（`adb devices` で 1 台のみ接続されている前提）。
#                           値はログへ出力しない（個体識別子の記載禁止・security.md 準拠）。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（実機（Pixel 10a）向けの単一 ABI）。
#   FANDHE_ANDROID_KEEP_APP 既定: 空（検証後に adb uninstall する）。非空指定で残置。
#
# 使い方: scripts/android/verify-talkback-a11y.sh
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと JAVA_HOME / ANDROID_HOME /
# ANDROID_NDK_HOME / GRADLE_HOME の export を行う（build-android.sh / build-aab.sh /
# verify-android-launch.sh と同じ前提）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

package="fandhe-egui-proxy"
lib_name="fandhe_egui_proxy"
application_id="com.fandhe.androidapp"
activity_name="com.fandhe.androidapp.MainActivity"
gradle_project_dir="${repo_root}/platforms/android"
jnilibs_out_dir="${repo_root}/target/android/jniLibs"
apk_path="${gradle_project_dir}/app/build/outputs/apk/debug/app-debug.apk"
evidence_dir="${repo_root}/_/verification/task-9.3-s3"
uiautomator_device_path="/sdcard/fandhe-talkback-a11y-dump.xml"

serial="${FANDHE_ANDROID_SERIAL:-}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"

log() {
  echo "[verify-talkback-a11y] $*"
}

fail() {
  echo "[verify-talkback-a11y] ERROR: $*" >&2
  exit 1
}

# adb へのターゲット端末指定を一元化する（verify-android-launch.sh と同じ理由。
# serial 未指定時は素の adb を返し、複数台接続時の誤操作をデバイス検出フェーズで防ぐ）。
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

# --- フェーズ 2: デバイス検出（人間担当手順の明示） --------------------------
# 判定ロジックは verify-android-launch.sh フェーズ 2 と同一（daemon 起動時の `*` 行除外・
# unauthorized/複数台判定・serial 指定時の状態確認）。TalkBack 検証では実機の USB 接続に
# 加えて TalkBack 有効化も人間担当となるため、エラーメッセージにその旨を含める。
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"

if [ -z "${device_lines}" ]; then
  fail "接続されている Android 端末がありません。Pixel 10a を USB 接続し、
  開発者向けオプション > USB デバッグ を有効化してください（人間担当手順）。
  接続後、端末側の『USB デバッグを許可しますか』ダイアログを承認してから
  再実行してください。TalkBack（設定 > ユーザー補助 > TalkBack）の有効化・
  音声読み上げの確認も人間担当です。"
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

# --- フェーズ 3: クロスビルド（release、jniLibs レイアウト出力） -------------
# rust-toolchain.toml（rustc 固定）を確実に効かせるため、リポジトリルートで実行する
# （build-android.sh / build-aab.sh / verify-android-launch.sh と同じ理由）。
cd "${repo_root}"

log "jniLibs 出力先をクリア: ${jnilibs_out_dir}"
rm -rf "${jnilibs_out_dir}"

declare -a cargo_ndk_args=()
# shellcheck disable=SC2206
abi_list=(${abis})
[ "${#abi_list[@]}" -gt 0 ] || fail "FANDHE_ANDROID_ABIS が空です。少なくとも 1 つ ABI を指定してください"
for abi in "${abi_list[@]}"; do
  cargo_ndk_args+=(-t "${abi}")
done
cargo_ndk_args+=(
  --platform "${ANDROID_MIN_API}"
  -o "${jnilibs_out_dir}"
  build
  -p "${package}"
  --release
)

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

for abi in "${abi_list[@]}"; do
  so_path="${jnilibs_out_dir}/${abi}/lib${lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
  log "  - ${so_path} 存在確認 OK"
done

# --- フェーズ 4: APK 生成（debug、起動確認専用） ------------------------------
# debug ビルドタイプは自動デバッグ署名されるため adb install 可能。配布成果物ではない
# （A05 対策。.aab のインストール検証は TASK-11.3（#246）のスコープ、
# release 署名は TASK-11.2-S3 のスコープであり本スクリプトでは触れない）。
abi_csv="$(IFS=,; echo "${abi_list[*]}")"
log "gradle -p ${gradle_project_dir} :app:assembleDebug -PfandheAndroidAbis=${abi_csv}"
gradle -p "${gradle_project_dir}" :app:assembleDebug --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"

[ -f "${apk_path}" ] || fail "APK の生成に失敗しました: ${apk_path}"
log "APK 生成 OK: ${apk_path}"

if command -v unzip >/dev/null 2>&1; then
  apk_listing="$(unzip -l "${apk_path}")"
elif command -v python3 >/dev/null 2>&1; then
  apk_listing="$(python3 -m zipfile -l "${apk_path}")"
else
  fail "unzip も python3 も見つからないため APK の構造検証ができません"
fi
for abi in "${abi_list[@]}"; do
  if ! printf '%s\n' "${apk_listing}" | grep -q "lib/${abi}/lib${lib_name}\.so"; then
    fail "APK 内に lib/${abi}/lib${lib_name}.so が見つかりません"
  fi
  log "  - lib/${abi}/lib${lib_name}.so 収載確認 OK"
done

# --- フェーズ 5: インストール・起動 -------------------------------------------
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true

adb_cmd logcat -c

log "adb install -r ${apk_path}"
adb_cmd install -r "${apk_path}"

log "adb shell am start -n ${application_id}/${activity_name}"
adb_cmd shell am start -n "${application_id}/${activity_name}"

# GameActivity の Vulkan 初期化・ウィンドウ表示には多少の時間を要するため、
# 判定前に待機する（verify-android-launch.sh と同じ固定 5 秒のマージン）。
sleep 5

# --- フェーズ 6: 起動判定（機械的検証） ---------------------------------------
pid="$(adb_cmd shell pidof "${application_id}" | tr -d '\r\n[:space:]' || true)"
if [ -z "${pid}" ]; then
  fail "プロセスが確認できません（起動直後にクラッシュした可能性）。
  logcat を確認してください: adb logcat -d | grep -i ${application_id}"
fi
log "  - プロセス生存確認 OK（pid=${pid}）"

crash_log="$(adb_cmd logcat -d --pid="${pid}" 2>/dev/null | grep -iE "FATAL EXCEPTION|tombstone|libc.*abort|Fatal signal|SIGSEGV|SIGABRT|SIGBUS|SIGILL" || true)"
if [ -n "${crash_log}" ]; then
  fail "logcat にクラッシュ痕跡が見つかりました:
${crash_log}"
fi
log "  - logcat クラッシュ痕跡なしを確認 OK"

resumed_line="$(adb_cmd shell dumpsys activity activities 2>/dev/null \
  | grep -E "mResumedActivity|topResumedActivity" || true)"
if ! printf '%s\n' "${resumed_line}" | grep -q "${application_id}"; then
  fail "Activity が RESUMED 状態ではありません（起動失敗・フォアグラウンド遷移失敗の疑い）。
  dumpsys 抜粋（mResumedActivity/topResumedActivity）:
${resumed_line}"
fi
log "  - Activity RESUMED 状態を確認 OK"

# --- フェーズ 6b: アクセシビリティツリー機械検証 ------------------------------
# `uiautomator dump` は Android の Accessibility フレームワーク（UiAutomation API）が
# 実際に走査できるノードツリーを XML 化する。accesskit_android の InjectingAdapter が
# 装着した AccessibilityNodeProvider 経由の仮想ノードがここに現れれば、TalkBack を含む
# 任意の a11y サービスへ到達可能であることの機械的証跡になる
# （TalkBack 自体の音声出力は本フェーズでは確認できない。§ 人間担当手順へ委譲）。
mkdir -p "${evidence_dir}"
uiautomator_dump_path="${evidence_dir}/uiautomator-dump.xml"
dumpsys_a11y_path="${evidence_dir}/dumpsys-accessibility.txt"

log "adb shell uiautomator dump ${uiautomator_device_path}"
uiautomator_dump_output="$(adb_cmd shell uiautomator dump "${uiautomator_device_path}" 2>&1 || true)"
log "  - uiautomator dump 出力: ${uiautomator_dump_output}"
adb_cmd pull "${uiautomator_device_path}" "${uiautomator_dump_path}" >/dev/null 2>&1 || true
# 端末側の一時ファイルを残置しない（後片付け。個体識別子は含まないファイルだが
# 検証専用の一時生成物のため端末に残さない方針で統一する）。
adb_cmd shell rm -f "${uiautomator_device_path}" >/dev/null 2>&1 || true

# dumpsys accessibility はサービス登録状況・ウィンドウの a11y 情報を補助的に示す
# （uiautomator dump が SurfaceView 止まりで期待ラベルを拾えない場合のフォールバック
# 記録用。判定には使わず、結果は事実のまま本ログ・レポートへ記録する）。
adb_cmd shell dumpsys accessibility > "${dumpsys_a11y_path}" 2>&1 || true

if [ -s "${uiautomator_dump_path}" ]; then
  log "  - uiautomator dump 取得 OK: ${uiautomator_dump_path}"
  # sample_screen() の期待ラベル（見出し・テキスト入力欄・ボタン・リスト項目、
  # crates/fandhe-core/src/ui/sample.rs 参照）が dump へ現れることを grep 検査する。
  # 見つからない場合でも fail-fast にはしない（GameActivity + SurfaceView 構成では
  # ノードが仮想ノードとして遅延生成され、アプリ起動直後の dump ではまだ枝が
  # 展開されていない可能性があるため。結果は事実のまま記録し、読み上げ成立の
  # 最終判定は人間担当の TalkBack 確認へ委ねる）。
  missing_labels=()
  for expected_label in "サンプル画面" "カウントを増やす" "項目を追加"; do
    if ! grep -q "${expected_label}" "${uiautomator_dump_path}"; then
      missing_labels+=("${expected_label}")
    fi
  done
  if [ "${#missing_labels[@]}" -eq 0 ]; then
    log "  - 期待ラベル（見出し・ボタン）の uiautomator dump 内での検出 OK"
  else
    log "  - 期待ラベルの一部が uiautomator dump 内に見つかりませんでした（未検出: ${missing_labels[*]}）。
      SurfaceView 止まりで仮想ノードが dump 対象になっていない可能性があります。
      ${dumpsys_a11y_path} のフォールバック確認結果と併せてレポートへ記録してください。
      TalkBack 有効時の実読み上げ確認（人間担当）で最終判定してください。"
  fi
else
  log "  - uiautomator dump の取得に失敗しました（空または取得不可）。
    ${dumpsys_a11y_path} のフォールバック確認結果を参照してください。
    TalkBack 有効時の実読み上げ確認（人間担当）で最終判定してください。"
fi

# --- フェーズ 7: 証跡取得（人間の目視確認材料） -------------------------------
screenshot_path="${evidence_dir}/launch-screenshot.png"
log "adb exec-out screencap -p > ${screenshot_path}"
adb_cmd exec-out screencap -p > "${screenshot_path}"
[ -s "${screenshot_path}" ] || fail "スクリーンショットの取得に失敗しました: ${screenshot_path}"
log "  - スクリーンショット取得 OK: ${screenshot_path}"

# --- フェーズ 8: 後片付け ------------------------------------------------------
# 端末のグローバル設定（TalkBack の有効/無効等のアクセシビリティ設定）は
# スクリプトから変更しない（TalkBack の有効化・無効化・音声読み上げ確認は
# 人間が端末 UI で行う。A05 対策・security.md 準拠）。
if [ -z "${keep_app}" ]; then
  log "adb uninstall ${application_id}"
  adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
else
  log "FANDHE_ANDROID_KEEP_APP 指定のため端末にアプリを残置します"
fi

log "機械的検証完了: ${application_id}/${activity_name}"
if [ -z "${keep_app}" ]; then
  log "注記: ${application_id} は上記でアンインストール済みです。TalkBack 読み上げ確認を行う場合は"
  log "FANDHE_ANDROID_KEEP_APP=1 を指定して本スクリプトを再実行してください"
else
  log "TalkBack を有効化し（設定 > ユーザー補助 > TalkBack）、音声読み上げを人間担当で確認してください"
fi
