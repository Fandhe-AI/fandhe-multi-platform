#!/usr/bin/env bash
# TASK-1.6（REQ-1 / M4、Issue #230）: Android 実機（Pixel 10a）または
# エミュレータでの起動確認スクリプト。
#
# 位置づけ: TASK-1.5（#43〜#46）が確立した cargo-ndk クロスビルド・
# GameActivity 方式の Vulkan バックエンド（crates/fandhe-egui-proxy/src/android.rs、
# TASK-9.3-S1 / #242 で NativeActivity から確定）を、実機/エミュレータへ実際に
# インストール・起動し、REQ-1 受け入れ基準
# 「Android NDK・cargo-ndk を用いたクロスビルドが成立し、実機またはエミュレータで
# 起動する」を実証する。
#
# sub-issue 間のスコープ境界（重要）:
#   - `.aab` の実機/エミュレータへのインストール検証は TASK-11.3（#246）のスコープ。
#     本スクリプトは debug APK（gradle :app:assembleDebug）を使う。debug APK は
#     自動でデバッグ署名され `adb install` 可能だが、**配布成果物ではない**
#     （A05 対策。release 署名（TASK-11.2-S3）・ストア提出経路には触れない）。
#   - jniLibs（.so）は release ビルド（cargo ndk --release）を使用し、
#     TASK-1.5 の成果物と同一条件で確認する。
#
# 呼び出し元: ローカル開発者（人間担当: 実機の USB 接続・USB デバッグ許可・
# 画面の目視確認）が直接実行する。CI 組み込みは行わない（実機接続が前提のため、
# ci.md の CI 制約とも独立）。
#
# 前提: scripts/android/setup-android-env.sh 実行済み（JDK・SDK・NDK・Gradle・
# cargo-ndk 導入済み）。
#
# 環境変数インターフェース（build-aab.sh の FANDHE_ANDROID_* と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（`adb devices` で 1 台のみ接続されている前提）。
#                           複数台接続時にどの端末を対象にするか指定する
#                           （`adb -s <serial>`）。値はログへ出力しない
#                           （個体識別子の記載禁止・security.md 準拠）。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（実機（Pixel 10a）向けの単一 ABI）。
#                           cargo-ndk へ渡す ABI 一覧（空白区切り）。
#   FANDHE_ANDROID_KEEP_APP 既定: 空（起動確認後に adb uninstall する）。
#                           非空を指定すると検証端末に debug アプリを残置する
#                           （繰り返し検証時の再インストール省略用）。
#
# 使い方: scripts/android/verify-android-launch.sh
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと JAVA_HOME / ANDROID_HOME /
# ANDROID_NDK_HOME / GRADLE_HOME の export を行う（build-android.sh / build-aab.sh と
# 同じ前提）。
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
screenshot_dir="${repo_root}/_/verification/task-1.6"

serial="${FANDHE_ANDROID_SERIAL:-}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"

log() {
  echo "[verify-android-launch] $*"
}

fail() {
  echo "[verify-android-launch] ERROR: $*" >&2
  exit 1
}

# adb へのターゲット端末指定を一元化する。serial 未指定時は素の adb を返す
# （複数台接続時の誤操作を防ぐため、serial 指定なし・複数台接続はデバイス検出フェーズで
# エラーにする）。
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
# `adb devices` の 1 行目（"List of devices attached"）を除いた本体のみを走査する。
# adb デーモン未起動時は 1 行目の前に `* daemon not running; starting now...` 等の
# `*` 始まりの状態行が出力され、これが残ると「0 台接続」ではなく解析不能な
# 余分な行として後続の判定を誤らせるため、`*` 始まりの行も除外する。
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"

if [ -z "${device_lines}" ]; then
  fail "接続されている Android 端末がありません。Pixel 10a を USB 接続し、
  開発者向けオプション > USB デバッグ を有効化してください（人間担当手順）。
  接続後、端末側の『USB デバッグを許可しますか』ダイアログを承認してから
  再実行してください。"
fi

# unauthorized 判定は device_count（有効な device 状態の端末数）を見てから行う。
# 複数台接続時、他に 1 台でも device 状態の端末があれば検証は続行可能なため、
# unauthorized の存在だけで即 fail にはしない（有効な端末があるのに検証をブロック
# しないための fail-safe の精緻化）。
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

# FANDHE_ANDROID_SERIAL 指定時は「device 状態の端末が 1 台以上ある」だけでは
# 不十分（指定シリアルとは別の端末が device 状態であるだけで検出をすり抜け、
# 後段の adb_cmd 実行時に初めて失敗しうるため）。`adb devices` の出力を
# シリアル単位で照合し、指定シリアルそのものが device 状態であることを
# 明示的に確認する（値はログへ出力しない。security.md 準拠）。
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
# （build-android.sh / build-aab.sh と同じ理由）。
cd "${repo_root}"

# jniLibs 出力先を都度クリアする（stale ABI 対策。build-aab.sh と同じ理由）。
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
# debug ビルドタイプは自動デバッグ署名されるため adb install 可能。
# .aab のインストール検証は TASK-11.3（#246）のスコープであり、ここでは
# 「起動確認」という目的に対して最短経路の debug APK を用いる（A05 注意点は
# 上記スクリプト冒頭コメント参照）。
abi_csv="$(IFS=,; echo "${abi_list[*]}")"
log "gradle -p ${gradle_project_dir} :app:assembleDebug -PfandheAndroidAbis=${abi_csv}"
gradle -p "${gradle_project_dir}" :app:assembleDebug --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"

[ -f "${apk_path}" ] || fail "APK の生成に失敗しました: ${apk_path}"
log "APK 生成 OK: ${apk_path}"

# APK 内に要求 ABI 分の .so が収載されていることを確認する（build-aab.sh の
# 検証パターンを APK 向けに踏襲。stale ABI 混入・ビルド未反映の検知）。
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
# 前回検証の残置（同一 applicationId の debug アプリ）があると `adb install -r` の
# 署名不一致（別鍵で再ビルドした場合）でエラーになりうるため、事前に uninstall を
# 試行する（未インストール時は no-op、エラーは無視してよい）。
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true

# logcat をここでクリアし、以降の起動判定（フェーズ 6(b)）を「今回の起動」に
# スコープする（クリアせず末尾 N 行だけを見る方式だと、無関係な既存アプリの
# クラッシュ痕跡が残っていた場合に誤検知しうるため）。
adb_cmd logcat -c

log "adb install -r ${apk_path}"
adb_cmd install -r "${apk_path}"

log "adb shell am start -n ${application_id}/${activity_name}"
adb_cmd shell am start -n "${application_id}/${activity_name}"

# GameActivity の Vulkan 初期化・ウィンドウ表示には多少の時間を要するため、
# 判定前に待機する（固定 5 秒。実機性能差を吸収する簡易的なマージン）。
sleep 5

# --- フェーズ 6: 起動判定（機械的検証） ---------------------------------------
# (a) プロセス生存確認
pid="$(adb_cmd shell pidof "${application_id}" | tr -d '\r\n[:space:]' || true)"
if [ -z "${pid}" ]; then
  fail "プロセスが確認できません（起動直後にクラッシュした可能性）。
  logcat を確認してください: adb logcat -d | grep -i ${application_id}"
fi
log "  - プロセス生存確認 OK（pid=${pid}）"

# (b) logcat 内のクラッシュ痕跡確認
# `logcat -c`（上記フェーズ 5）で当該起動以降のログのみに絞ってあるため、
# 無関係な既存プロセスの過去のクラッシュ痕跡を拾わない。`--pid` は minSdk 24 で
# 対応済みの adb logcat オプションのため、当該 pid のログのみへさらに絞る。
# Vulkan/NDK バックエンド（fandhe-egui-proxy/src/android.rs）で起こりやすいネイティブ
# クラッシュ（SIGSEGV 等）は "abort" という語を含まない tombstone 行になりうるため、
# debuggerd が出力する "Fatal signal" 行・代表的なシグナル名も検出対象に含める
# （直前の (a) プロセス生存確認が既にプロセス終了ケースの大半をカバーしているため、
# 本チェックはあくまで defense-in-depth であり、これで全網羅を保証するものではない）。
crash_log="$(adb_cmd logcat -d --pid="${pid}" 2>/dev/null | grep -iE "FATAL EXCEPTION|tombstone|libc.*abort|Fatal signal|SIGSEGV|SIGABRT|SIGBUS|SIGILL" || true)"
if [ -n "${crash_log}" ]; then
  fail "logcat にクラッシュ痕跡が見つかりました:
${crash_log}"
fi
log "  - logcat クラッシュ痕跡なしを確認 OK"

# (c) Activity の RESUMED 状態確認
# `dumpsys activity activities` は ActivityRecord ヘッダー行と state= 行が
# 数十行離れているため行数固定の grep -A では捕捉できない。代わりに
# フォアグラウンド Activity 名を直接含む要約行を対象にする。
# 要約行のラベルは OS バージョンで異なる（Android 15 以前: `mResumedActivity` /
# `topResumedActivity`、Android 16: `ResumedActivity:` / `Resumed:`）ため、
# 両表記を許容する（Issue #566 の Pixel 10a / Android 16 実機確認で判明）。
resumed_line="$(adb_cmd shell dumpsys activity activities 2>/dev/null \
  | grep -E "ResumedActivity|^[[:space:]]*Resumed:" || true)"
if ! printf '%s\n' "${resumed_line}" | grep -q "${application_id}"; then
  fail "Activity が RESUMED 状態ではありません（起動失敗・フォアグラウンド遷移失敗の疑い）。
  dumpsys 抜粋（Resumed 系要約行）:
${resumed_line}"
fi
log "  - Activity RESUMED 状態を確認 OK"

# --- フェーズ 7: 証跡取得（人間の目視確認材料） -------------------------------
mkdir -p "${screenshot_dir}"
screenshot_path="${screenshot_dir}/launch-screenshot.png"
# 画面が消灯したままだと screencap が全面黒の画像を返し、証跡として使えない。
# 撮影前に画面を点灯させる。
adb_cmd shell input keyevent KEYCODE_WAKEUP >/dev/null 2>&1 || true
sleep 1
# ロック画面表示中は screencap が黒画像を返すため、非空チェックだけでは
# 無効な証跡を PASS させてしまう（fail-open）。ここは fail-closed とし、
# 人間がロックを解除してからの再実行を求める（ロック解除は自動化できない）。
#
# `dumpsys window` の KeyguardStateMonitor が出力する `mIsShowing=<true|false>`
# を明示的に解析し、`false`（ロック解除済み）を確認できた場合だけ撮影へ進む。
# `mIsShowing` は OS バージョンによって出力されない（Pixel 10a / Android 16 系で
# 未出力を確認、Issue #801）。その場合は同じ出力中の `mDreamingLockscreen=<true|false>`
# をフォールバックとして使う（Pixel 10a で 2026-09-07 に動作確認済み。ロック中 true /
# 解除後 false）。dumpsys の失敗・両フィールドとも不在で値を取得できない場合は
# 「ロック状態が不明」として fail にする（判定不能を PASS にしない）。
keyguard_dump="$(adb_cmd shell dumpsys window 2>/dev/null || true)"
keyguard_field="mIsShowing"
keyguard_showing="$(printf '%s\n' "${keyguard_dump}" \
  | sed -n 's/.*mIsShowing=\([A-Za-z]*\).*/\1/p' | head -n 1)"
if [ -z "${keyguard_showing}" ]; then
  keyguard_field="mDreamingLockscreen"
  keyguard_showing="$(printf '%s\n' "${keyguard_dump}" \
    | sed -n 's/.*mDreamingLockscreen=\([A-Za-z]*\).*/\1/p' | head -n 1)"
fi
log "  - ロック画面判定に ${keyguard_field} を使用（取得値: '${keyguard_showing}'）"
case "${keyguard_showing}" in
  false)
    ;;
  true)
    fail "ロック画面が表示されているため有効なスクリーンショットを取得できません（黒画像になります）。
  端末のロックを解除し、画面を点灯させた状態で本スクリプトを再実行してください。"
    ;;
  *)
    fail "ロック画面の状態を判定できませんでした（\`dumpsys window\` から mIsShowing / mDreamingLockscreen の
  いずれも取得できません）。
  ロック中に撮影すると黒画像を有効な証跡として扱ってしまうため、判定不能の時点で中断します。
  端末の接続状態を確認するか、OS バージョンによる dumpsys の出力形式変更を確認してください。"
    ;;
esac
log "adb exec-out screencap -p > ${screenshot_path}"
adb_cmd exec-out screencap -p > "${screenshot_path}"
[ -s "${screenshot_path}" ] || fail "スクリーンショットの取得に失敗しました: ${screenshot_path}"
log "  - スクリーンショット取得 OK: ${screenshot_path}"

# --- フェーズ 8: 後片付け ------------------------------------------------------
if [ -z "${keep_app}" ]; then
  log "adb uninstall ${application_id}"
  adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
else
  log "FANDHE_ANDROID_KEEP_APP 指定のため端末にアプリを残置します"
fi

log "起動確認 OK: ${application_id}/${activity_name}"
