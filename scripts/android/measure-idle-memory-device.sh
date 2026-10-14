#!/usr/bin/env bash
# TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）: Android 実機（Pixel 10a）での
# アイドル時メモリ増分計測スクリプト。
#
# 位置づけ: TASK-6.3（#93）が Linux プロキシ（fandhe-egui-proxy-desktop）で計測した
# アイドル時メモリを、実機・実 UI フレームワーク比較へ発展させる。本スクリプトは
# 配布用アプリ（platforms/android/app、GameActivity + fandhe-egui-proxy。
# TASK-9.3-S1 / #242 で NativeActivity から確定）と、
# 検証専用の 2 アプリ（scripts/metrics/android-memory/baseline・native-sample、
# 配布ビルドから分離。README 参照）の計 3 アプリを対象に、
# `adb shell dumpsys meminfo <pkg>` の TotalPSS を主指標として計測する
# （verify-android-launch.sh のデバイス検出・前提検査パターンを踏襲）。
#
# 判定はここでは行わない（値の記録のみ）。REQ-5 受け入れ基準（fandhe 増分が
# ネイティブ SDK 実装比 +50% 以内・絶対値 20MB 以内）への当てはめは
# 人間担当が結果を見て行う。
#
# 呼び出し元: 人間担当（Pixel 10a を本 Linux マシンへ USB 接続して実行）。
# CI 組み込みは行わない（実機接続前提のため、ci.md の CI 制約とも独立。
# verify-android-launch.sh と同じ運用方針）。
#
# 前提: scripts/android/setup-android-env.sh 実行済み。3 アプリの debug APK
# （platforms/android/app、scripts/metrics/android-memory/{baseline,native-sample}）が
# それぞれ既にビルド済み、またはビルド可能であること（本スクリプトが gradle
# assembleDebug を都度実行する）。
#
# fandhe（platforms/android/app）は GameActivity（MainActivity.java）+
# fandhe-egui-proxy の cdylib（libfandhe_egui_proxy.so）を起動する薄いラッパーであり
# （TASK-9.3-S1 / #242、README 参照）、gradle :app:assembleDebug だけでは jniLibs（.so）は
# 生成されず、verify-android-launch.sh と同様に本スクリプトが cargo ndk で先に
# ビルドして target/android/jniLibs へ配置する（Bugbot 指摘対応、Issue #237）。
# baseline・native-sample の 2 アプリは Rust コアに依存しないため対象外。
#
# 環境変数インターフェース（verify-android-launch.sh と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（`adb devices` で 1 台のみ接続されている前提）。
#                           値はログ・CSV へ出力しない（security.md 準拠）。
#   FANDHE_ANDROID_TRIALS   既定: 5。各アプリの試行回数。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（実機（Pixel 10a）向けの単一 ABI）。
#                           cargo-ndk へ渡す ABI 一覧（空白区切り、verify-android-launch.sh と同じ）。
#   FANDHE_ANDROID_KEEP_APP 既定: 空（計測後に adb uninstall する）。非空で残置する。
#
# 使い方: bash scripts/android/measure-idle-memory-device.sh
#
# 出力: 標準出力に CSV（列: app,run,total_pss_kb_at_3s,total_pss_kb_at_10s）を書き、
# 各アプリごとに中央値・平均を標準エラーへ出力する。
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

serial="${FANDHE_ANDROID_SERIAL:-}"
trials="${FANDHE_ANDROID_TRIALS:-5}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"

# fandhe（platforms/android/app）のネイティブライブラリビルドに用いる定数
# （verify-android-launch.sh と同じ package/lib_name/jniLibs 出力先）。
fandhe_cargo_package="fandhe-egui-proxy"
fandhe_lib_name="fandhe_egui_proxy"
jnilibs_out_dir="${repo_root}/target/android/jniLibs"

# A03（インジェクション）対策: 試行回数は数値のみ許容する。
if ! [[ "${trials}" =~ ^[1-9][0-9]*$ ]]; then
  echo "エラー: FANDHE_ANDROID_TRIALS は正の整数で指定してください: ${trials}" >&2
  exit 1
fi

log() { echo "[measure-idle-memory-device] $*" >&2; }
fail() {
  echo "[measure-idle-memory-device] ERROR: $*" >&2
  exit 1
}

adb_cmd() {
  if [ -n "${serial}" ]; then
    adb -s "${serial}" "$@"
  else
    adb "$@"
  fi
}

# --- フェーズ 1: 前提検査（fail-fast） ---------------------------------------
command -v gradle >/dev/null 2>&1 || fail "gradle が見つかりません（PATH に GRADLE_HOME/bin が含まれているか確認してください）"
command -v adb >/dev/null 2>&1 || fail "adb が見つかりません（PATH に ANDROID_HOME/platform-tools が含まれているか確認してください）"
# fandhe（platforms/android/app）の libfandhe_egui_proxy.so クロスビルドに必要
# （verify-android-launch.sh と同じ前提検査）。
command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません（rustup 導入後に再実行してください）"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] \
  || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"

# --- フェーズ 2: デバイス検出（verify-android-launch.sh と同じロジック） -----
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"
if [ -z "${device_lines}" ]; then
  fail "接続されている Android 端末がありません。Pixel 10a を USB 接続し、
  開発者向けオプション > USB デバッグ を有効化・許可してから再実行してください（人間担当手順）。"
fi
device_count="$(printf '%s\n' "${device_lines}" | grep -c "device$" || true)"
if [ "${device_count}" -lt 1 ] && printf '%s\n' "${device_lines}" | grep -q "unauthorized"; then
  fail "接続されている端末が unauthorized 状態です。端末側のダイアログを承認してください（人間担当手順）。"
fi
if [ -z "${serial}" ] && [ "${device_count}" -gt 1 ]; then
  fail "複数台の端末が接続されています。FANDHE_ANDROID_SERIAL で対象端末を指定してください。"
fi
if [ "${device_count}" -lt 1 ]; then
  fail "device 状態の端末が見つかりません（unauthorized/offline のみ検出）。"
fi
if [ -n "${serial}" ]; then
  serial_state="$(printf '%s\n' "${device_lines}" | awk -v s="${serial}" '$1 == s { print $2 }')"
  [ -n "${serial_state}" ] || fail "FANDHE_ANDROID_SERIAL で指定された端末が adb devices の出力に見つかりません。"
  [ "${serial_state}" = "device" ] || fail "FANDHE_ANDROID_SERIAL で指定された端末が device 状態ではありません（状態: ${serial_state}）。"
fi
log "デバイス検出 OK（${device_count} 台、device 状態）"

# --- フェーズ 3: 計測対象アプリの定義（外部入力に依存しない固定定数） --------
# app_id: applicationId（固定定数、adb 出力・引数からは組み立てない。A03 対策）
# gradle_dir: assembleDebug 対象の Gradle プロジェクトディレクトリ
# apk_path: 生成される debug APK の絶対パス
# activity: `adb shell am start -n <app_id>/<activity>` に渡す Activity 名
declare -a APP_IDS=(
  "com.fandhe.androidapp"
  "com.fandhe.metrics.baseline"
  "com.fandhe.metrics.nativesample"
)
declare -a APP_LABELS=(
  "fandhe（GameActivity + fandhe-egui-proxy）"
  "baseline（空 Activity）"
  "native-sample（Jetpack Compose）"
)
declare -a APP_GRADLE_DIRS=(
  "${repo_root}/platforms/android"
  "${repo_root}/scripts/metrics/android-memory"
  "${repo_root}/scripts/metrics/android-memory"
)
declare -a APP_GRADLE_TASKS=(
  ":app:assembleDebug"
  ":baseline:assembleDebug"
  ":native-sample:assembleDebug"
)
declare -a APP_APK_PATHS=(
  "${repo_root}/platforms/android/app/build/outputs/apk/debug/app-debug.apk"
  "${repo_root}/scripts/metrics/android-memory/baseline/build/outputs/apk/debug/baseline-debug.apk"
  "${repo_root}/scripts/metrics/android-memory/native-sample/build/outputs/apk/debug/native-sample-debug.apk"
)
declare -a APP_ACTIVITIES=(
  "com.fandhe.androidapp.MainActivity"
  "com.fandhe.metrics.baseline.BaselineActivity"
  "com.fandhe.metrics.nativesample.NativeSampleActivity"
)

# --- フェーズ 3.5: fandhe（platforms/android/app）のネイティブライブラリビルド ---
# gradle :app:assembleDebug は Java/Kotlin ソースを持たない薄いラッパーのビルドのみ
# 行い、libfandhe_egui_proxy.so は生成しない。verify-android-launch.sh と同じ手順
# （cargo ndk → target/android/jniLibs 配置）を先に実行し、GameActivity が
# 起動時にロードするライブラリを用意する（Bugbot 指摘対応、Issue #237）。
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
  -p "${fandhe_cargo_package}"
  --release
)

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

for abi in "${abi_list[@]}"; do
  so_path="${jnilibs_out_dir}/${abi}/lib${fandhe_lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
  log "  - ${so_path} 存在確認 OK"
done

abi_csv="$(IFS=,; echo "${abi_list[*]}")"

out_dir="${repo_root}/_/verification/task-5.2"
mkdir -p "${out_dir}"
csv_path="${out_dir}/memory-increment-device.csv"

echo "app,run,total_pss_kb_at_3s,total_pss_kb_at_10s" | tee "${csv_path}"

# TotalPSS を安全に取り出すヘルパー。`dumpsys meminfo` の出力（端末由来の未信頼テキスト）
# から数値のみを抽出し、正整数であることを再検証してから返す（A03 対策。
# measure-idle-memory.sh の sample_rss と同じ「疑わしい値は NA」方針）。
# 見出し表記は Android バージョンにより "TOTAL PSS" / "TOTAL:" 等の揺れがあるため、
# 複数パターンを順に試す（最初にヒットしたものを採用）。
sample_total_pss_kb() {
  local app_id="$1"
  local dump
  local raw
  dump="$(adb_cmd shell dumpsys meminfo "${app_id}" 2>/dev/null || true)"
  raw="$(printf '%s\n' "${dump}" | grep -E "TOTAL PSS" | head -n1 | grep -oE "[0-9]+" | head -n1 || true)"
  if [ -z "${raw}" ]; then
    raw="$(printf '%s\n' "${dump}" | grep -E "^\s*TOTAL\s" | head -n1 | grep -oE "[0-9]+" | head -n1 || true)"
  fi
  if [[ "${raw}" =~ ^[0-9]+$ ]]; then
    echo "${raw}"
  else
    echo ""
  fi
}

# `dumpsys meminfo` の見出し表記に起因する fail-fast は、プロセスが実際に起動できて
# いる（pidof で確認できた）場合にのみ発火させる。起動自体に失敗したアプリ（PSS
# サンプルが得られないのはプロセスが存在しないため）まで「dumpsys 形式の問題」と
# 誤診断しないよう、両者を区別して扱う（Bugbot 指摘対応、Issue #237）。
# fandhe（i=0、GameActivity + Vulkan）は 3 アプリ中もっとも起動失敗しやすいため、
# fandhe 単体の起動失敗で baseline / native-sample の計測まで巻き込んで中断しない
# ように、fail-fast はアプリ番号を問わず「プロセス起動済みなのに TotalPSS が
# 一度も取れない」ケースにのみ限定する。
dumpsys_format_confirmed_ok=""
# アプリごとに 3 秒・10 秒の両時点とも全試行で TotalPSS が 1 件も取れなかった場合を
# 記録する（dumpsys 形式の問題として fail-fast しなかったケース、例えば起動失敗が
# 起きたアプリ）。3 アプリ（iOS は 4 アプリ）を空振りさせたまま exit 0 で完走させると、
# 真の原因（起動失敗等）が「計測完了」の陰に埋もれてしまうため、全アプリの計測後に
# まとめて exit 1 とする（Bugbot 指摘対応、Issue #237）。
declare -a FAILED_APPS=()

for i in "${!APP_IDS[@]}"; do
  app_id="${APP_IDS[$i]}"
  label="${APP_LABELS[$i]}"
  gradle_dir="${APP_GRADLE_DIRS[$i]}"
  gradle_task="${APP_GRADLE_TASKS[$i]}"
  apk_path="${APP_APK_PATHS[$i]}"
  activity="${APP_ACTIVITIES[$i]}"

  log "=== ${label}（${app_id}）==="

  # --- ビルド ---
  if [ "${i}" -eq 0 ]; then
    # fandhe（platforms/android/app）: jniLibs.srcDirs が参照する ABI 一覧を
    # build.gradle.kts の fandheAndroidAbis プロパティで揃える
    # （verify-android-launch.sh / build-aab.sh と同じ運用）。
    log "gradle -p ${gradle_dir} ${gradle_task} -PfandheAndroidAbis=${abi_csv}"
    gradle -p "${gradle_dir}" "${gradle_task}" --console=plain \
      "-PfandheAndroidAbis=${abi_csv}"
  else
    log "gradle -p ${gradle_dir} ${gradle_task}"
    gradle -p "${gradle_dir}" "${gradle_task}" --console=plain
  fi
  [ -f "${apk_path}" ] || fail "APK の生成に失敗しました: ${apk_path}"

  # --- インストール ---
  adb_cmd uninstall "${app_id}" >/dev/null 2>&1 || true
  log "adb install -r ${apk_path}"
  adb_cmd install -r "${apk_path}"

  declare -a PSS_AT_3S=()
  declare -a PSS_AT_10S=()

  run=1
  while [ "${run}" -le "${trials}" ]; do
    adb_cmd logcat -c
    log "adb shell am start -n ${app_id}/${activity}"
    adb_cmd shell am start -n "${app_id}/${activity}" >/dev/null

    sleep 3
    pid_3s="$(adb_cmd shell pidof "${app_id}" | tr -d '\r\n[:space:]' || true)"
    if [ -z "${pid_3s}" ]; then
      log "警告: run ${run}: 3 秒経過時点でプロセスが確認できません（${label} の起動失敗の疑い）"
      pss_3s=""
    else
      pss_3s="$(sample_total_pss_kb "${app_id}")"
      # プロセスは起動できているのに TotalPSS が取れない場合のみ、dumpsys の
      # 出力形式そのものに問題がある可能性を示す証拠として扱う（起動失敗との
      # 混同を避けるため）。
      if [ -n "${pss_3s}" ]; then
        dumpsys_format_confirmed_ok=1
      fi
    fi
    if [ -z "${pss_3s}" ]; then
      log "警告: run ${run}: 3 秒時点の TotalPSS を取得できませんでした"
      pss_3s="NA"
    fi

    sleep 7
    pid_10s="$(adb_cmd shell pidof "${app_id}" | tr -d '\r\n[:space:]' || true)"
    if [ -z "${pid_10s}" ]; then
      log "警告: run ${run}: 10 秒経過時点でプロセスが確認できません（${label} の起動失敗またはクラッシュの疑い）"
      pss_10s=""
    else
      pss_10s="$(sample_total_pss_kb "${app_id}")"
      if [ -n "${pss_10s}" ]; then
        dumpsys_format_confirmed_ok=1
      fi
    fi
    if [ -z "${pss_10s}" ]; then
      log "警告: run ${run}: 10 秒時点の TotalPSS を取得できませんでした"
      pss_10s="NA"
    fi

    echo "${app_id},${run},${pss_3s},${pss_10s}" | tee -a "${csv_path}"
    if [ "${pss_3s}" != "NA" ]; then PSS_AT_3S+=("${pss_3s}"); fi
    if [ "${pss_10s}" != "NA" ]; then PSS_AT_10S+=("${pss_10s}"); fi

    # プロセス自体は起動できたのに TotalPSS を一度も取得できていない場合のみ、
    # `dumpsys meminfo` の見出し表記が本端末・OS バージョンで想定外である可能性が
    # 高いと判断して fail-fast する。i==0（fandhe）の起動失敗（pid が確認できない
    # ケース）はここでは扱わず、単なる警告に留めて baseline / native-sample の計測へ
    # 進める（起動失敗を dumpsys 形式の問題と誤診断しないための区別、Bugbot 指摘対応、
    # Issue #237）。
    if [ "${run}" -eq 1 ] && { [ -n "${pid_3s}" ] || [ -n "${pid_10s}" ]; } \
      && [ -z "${dumpsys_format_confirmed_ok}" ]; then
      fail "${label}（${app_id}）でプロセスは起動できましたが、1 回目の計測で TotalPSS を
      一度も取得できませんでした。dumpsys meminfo の出力形式が想定と異なる可能性があります。
      手動で以下を実行し、'TOTAL' を含む行を確認してください:
        adb shell dumpsys meminfo ${app_id}
      本スクリプトの sample_total_pss_kb（本ファイル内）のパターンを出力に合わせて
      修正してから再実行してください。"
    fi

    adb_cmd shell am force-stop "${app_id}" >/dev/null 2>&1 || true
    run=$((run + 1))
  done

  summarize() {
    # bash 3.2（macOS 標準）には nameref（local -n）が無いため、配列は
    # スペース区切り文字列として値渡しし、関数内で再展開する
    # （scripts/apple/measure-idle-memory-macos.sh の summarize と同方針）。
    # shellcheck disable=SC2206
    local values_ref=($1)
    local label2="$2"
    local n=${#values_ref[@]}
    if [ "${n}" -eq 0 ]; then
      log "サマリ（${app_id} / ${label2}）: 有効なサンプルがありません"
      return
    fi
    local sorted
    # shellcheck disable=SC2207
    sorted=($(printf '%s\n' "${values_ref[@]}" | sort -n))
    local sum=0
    for v in "${sorted[@]}"; do sum=$((sum + v)); done
    local mean=$((sum / n))
    local median
    if [ $((n % 2)) -eq 1 ]; then
      median="${sorted[$((n / 2))]}"
    else
      local lo=${sorted[$((n / 2 - 1))]}
      local hi=${sorted[$((n / 2))]}
      median=$(( (lo + hi) / 2 ))
    fi
    log "サマリ（${app_id} / ${label2}）: n=${n} 平均=${mean}KB 中央値=${median}KB"
  }
  # bash 3.2 では nounset（set -u）下で空配列の `${arr[*]}` が unbound variable と
  # なるため、既定値付き展開（`${arr[*]-}`）で空文字を渡して n=0 分岐へ到達させる。
  summarize "${PSS_AT_3S[*]-}" "3秒時点"
  summarize "${PSS_AT_10S[*]-}" "10秒時点"

  if [ "${#PSS_AT_3S[@]}" -eq 0 ] && [ "${#PSS_AT_10S[@]}" -eq 0 ]; then
    FAILED_APPS+=("${label}（${app_id}）")
  fi

  if [ -z "${keep_app}" ]; then
    adb_cmd uninstall "${app_id}" >/dev/null 2>&1 || true
  else
    log "FANDHE_ANDROID_KEEP_APP 指定のため端末にアプリを残置します（${app_id}）"
  fi
done

if [ "${#FAILED_APPS[@]}" -gt 0 ]; then
  fail "以下のアプリで有効な TotalPSS サンプルが 1 件も取得できませんでした（起動失敗・
  クラッシュ等が疑われます。CSV（${csv_path}）の NA 行を確認してください）:
$(printf '    - %s\n' "${FAILED_APPS[@]}")
  計測は完走しましたが、記入欄への転記前に原因を確認してください。"
fi

log "計測完了。CSV: ${csv_path}"
log "結果は ${csv_path} に保存しました。"
