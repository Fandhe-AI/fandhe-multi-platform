#!/usr/bin/env bash
# TASK-5.1（REQ-5 / M4、Issue #236）: Android 実機での fandhe / ネイティブ SDK 比較
# コールドスタート計測スクリプト。
#
# 位置づけ: TASK-1.6（#230、scripts/android/verify-android-launch.sh）が確立した
# 「クロスビルド → APK 生成 → adb install/起動 → 機械的判定」のパターンを踏襲しつつ、
# 本スクリプトは fandhe（`platforms/android/app`、GameActivity（MainActivity.java）、
# TASK-9.3-S1 / #242 で NativeActivity から移行済み）と同一機能の
# Jetpack Compose 比較アプリ（`benchmarks/coldstart/android-native/`）の双方を
# 同一条件（release ビルド・force-stop 済み cold 起動）で計測し、
# `adb shell am start -W -S` の TotalTime を中央値化・絶対値/相対値判定まで行う。
#
# sub-issue 間のスコープ境界:
#   - `.aab` のインストール検証は TASK-11.3（#246）のスコープ。本スクリプトは
#     verify-android-launch.sh と同様に debug 署名の release ビルドタイプ APK を使う
#     （release ビルドタイプは debuggable=false による起動オーバーヘッド排除が目的、
#     配布署名（TASK-11.2-S3）とは独立）。
#   - メモリ増分（TASK-5.2 / #237）・フレームレート（TASK-5.3 / #238）は対象外。
#
# 判定基準:
#   - 主判定: 絶対値 400ms 以内（fandhe 実装のみに適用。ネイティブ比較アプリは
#     参考値として同時に記録する）
#   - 参考指標: ネイティブ実装比 +30% 以内（絶対値未達時は主判定を優先する方針、
#     REQ-5 / 04-requirements.md 準拠）
#
# 環境変数インターフェース（verify-android-launch.sh と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（adb devices で 1 台のみ接続されている前提）。
#                           値はログへ出力しない（個体識別子の記載禁止・security.md 準拠）。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（実機（Pixel 10a）向け単一 ABI）。
#   FANDHE_COLD_START_TRIALS 既定: 5（各実装の試行回数。中央値を採用する）。
#
# 使い方: scripts/android/measure-cold-start.sh
set -euo pipefail

unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

fandhe_package="fandhe-egui-proxy"
fandhe_lib_name="fandhe_egui_proxy"
fandhe_application_id="com.fandhe.androidapp"
# TASK-9.3-S1（#242）以降 platforms/android は GameActivity（MainActivity.java）を
# 起動する。NativeActivity のままでは `am start -W -S` が実コンポーネントを解決できず
# TotalTime 抽出に失敗するため、verify-android-launch.sh と同一の値を使う
# （Bugbot 指摘対応）。
fandhe_activity_name="com.fandhe.androidapp.MainActivity"
fandhe_gradle_project_dir="${repo_root}/platforms/android"
fandhe_jnilibs_out_dir="${repo_root}/target/android/jniLibs"
# 署名済み（app-release.apk）を前提とする。未署名 APK（app-release-unsigned.apk）は
# `adb install` が INSTALL_PARSE_FAILED_NO_CERTIFICATES で拒否するため実機計測には
# 使えない（Review 指摘対応）。署名は docs/operations/android-aab-signing.md の
# 開発署名鍵契約（FANDHE_ANDROID_KEYSTORE 等）を再利用する（下記フェーズ 1 で必須化）。
fandhe_apk_path="${fandhe_gradle_project_dir}/app/build/outputs/apk/release/app-release.apk"

# applicationId は com.fandhe.coldstart.nativeapp（native は Java 予約キーワードのため
# AGP の namespace 検証で拒否される。benchmarks/coldstart/android-native/app/build.gradle.kts
# のコメント参照）。
native_application_id="com.fandhe.coldstart.nativeapp"
native_activity_name="com.fandhe.coldstart.nativeapp.MainActivity"
native_gradle_project_dir="${repo_root}/benchmarks/coldstart/android-native"
# 比較アプリは AGP 既定の debug signingConfig で release をデバッグ署名する
# （benchmarks/coldstart/android-native/app/build.gradle.kts 参照）。debuggable=false
# による起動オーバーヘッド排除は維持しつつ、`adb install` が受け付ける
# 署名済み APK にする（Review 指摘対応）。
native_apk_path="${native_gradle_project_dir}/app/build/outputs/apk/release/app-release.apk"


serial="${FANDHE_ANDROID_SERIAL:-}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
trials="${FANDHE_COLD_START_TRIALS:-5}"

log() {
  # 標準エラー出力へ書く（標準出力は関数の戻り値専用に保つ）。`run_trials` は
  # 中央値を `$( ... )`（コマンド置換）経由で受け取るため、標準出力へ書くと
  # ログ行まで戻り値に混入して数値判定が壊れる（Review 指摘対応）。
  echo "[measure-cold-start] $*" >&2
}

fail() {
  echo "[measure-cold-start] ERROR: $*" >&2
  exit 1
}

adb_cmd() {
  if [ -n "${serial}" ]; then
    adb -s "${serial}" "$@"
  else
    adb "$@"
  fi
}

# 失敗時の診断メッセージへ埋め込む adb 呼び出し。FANDHE_ANDROID_SERIAL 指定時に
# 素の `adb shell` を案内すると複数台接続で `more than one device` になり、
# スクリプトが見たのと同じ出力を再現できないため、-s を含めた形で提示する。
# serial の実値はログへ出力しない契約のため、値ではなく変数参照の形で提示する。
adb_hint="adb"
if [ -n "${serial}" ]; then
  adb_hint='adb -s "$FANDHE_ANDROID_SERIAL"'
fi

# --- フェーズ 1: 前提検査（fail-fast） -------------------------------------
command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません"
[ -d "${GRADLE_HOME}" ] || fail "GRADLE_HOME（${GRADLE_HOME}）が見つかりません"
[ -d "${JAVA_HOME}" ] || fail "JAVA_HOME（${JAVA_HOME}）が見つかりません"
command -v gradle >/dev/null 2>&1 || fail "gradle が見つかりません"
command -v adb >/dev/null 2>&1 || fail "adb が見つかりません"

# fandhe 側 APK は署名済みでないと `adb install` が
# INSTALL_PARSE_FAILED_NO_CERTIFICATES で拒否するため、開発署名鍵
# （docs/operations/android-aab-signing.md の契約、TASK-11.2-S3）を必須化する。
# `platforms/android/app/build.gradle.kts` は FANDHE_ANDROID_KEYSTORE が非空の
# 場合のみ signingConfigs を定義するため、ここで未設定を検知して案内する
# （values そのものはログへ出力しない。security.md 準拠）。
[ -n "${FANDHE_ANDROID_KEYSTORE:-}" ] || fail "FANDHE_ANDROID_KEYSTORE が未設定です。
docs/operations/android-aab-signing.md の手順で開発署名鍵を生成し、
FANDHE_ANDROID_KEYSTORE（・必要に応じ FANDHE_ANDROID_KEYSTORE_PASSWORD /
FANDHE_ANDROID_KEY_ALIAS / FANDHE_ANDROID_KEY_PASSWORD）を設定してください。
未署名 APK は adb install が拒否するため実機計測には使えません。"
[ -n "${FANDHE_ANDROID_KEYSTORE_PASSWORD:-}" ] || fail "FANDHE_ANDROID_KEYSTORE_PASSWORD が未設定です
（FANDHE_ANDROID_KEYSTORE 非空時は必須。docs/operations/android-aab-signing.md 参照）。"

case "${trials}" in
  ''|*[!0-9]*)
    fail "FANDHE_COLD_START_TRIALS は正の整数のみ対応: ${trials}"
    ;;
esac
[ "${trials}" -ge 1 ] || fail "FANDHE_COLD_START_TRIALS は 1 以上を指定してください"

# --- フェーズ 2: デバイス検出（verify-android-launch.sh と同じロジック） ---
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"
if [ -z "${device_lines}" ]; then
  fail "接続されている Android 端末がありません。Pixel 10a を USB 接続し、
  開発者向けオプション > USB デバッグ を有効化してください（人間担当手順）。"
fi
device_count="$(printf '%s\n' "${device_lines}" | grep -c "device$" || true)"
if [ "${device_count}" -lt 1 ]; then
  fail "device 状態の端末が見つかりません（unauthorized/offline のみ検出の可能性）。
  端末側のダイアログ承認・USB 接続状態を確認してください。"
fi
if [ -z "${serial}" ] && [ "${device_count}" -gt 1 ]; then
  fail "複数台の端末が接続されています。FANDHE_ANDROID_SERIAL で対象端末を指定してください
  （値はログへ出力されません）。"
fi
if [ -n "${serial}" ]; then
  serial_state="$(printf '%s\n' "${device_lines}" | awk -v s="${serial}" '$1 == s { print $2 }')"
  [ "${serial_state}" = "device" ] || fail "FANDHE_ANDROID_SERIAL で指定された端末が device 状態ではありません（値はログへ出力されません）。"
fi
log "デバイス検出 OK（${device_count} 台、device 状態）"

# --- フェーズ 3: fandhe 側ビルド（release、jniLibs + APK） ------------------
cd "${repo_root}"
log "jniLibs 出力先をクリア: ${fandhe_jnilibs_out_dir}"
rm -rf "${fandhe_jnilibs_out_dir}"

declare -a cargo_ndk_args=()
# shellcheck disable=SC2206
abi_list=(${abis})
[ "${#abi_list[@]}" -gt 0 ] || fail "FANDHE_ANDROID_ABIS が空です"
for abi in "${abi_list[@]}"; do
  cargo_ndk_args+=(-t "${abi}")
done
cargo_ndk_args+=(--platform "${ANDROID_MIN_API}" -o "${fandhe_jnilibs_out_dir}" build -p "${fandhe_package}" --release)
log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"
for abi in "${abi_list[@]}"; do
  so_path="${fandhe_jnilibs_out_dir}/${abi}/lib${fandhe_lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
done

abi_csv="$(IFS=,; echo "${abi_list[*]}")"
# release ビルドタイプで assemble する。debuggable=false による起動オーバーヘッド
# 排除が目的（手順書「計測方法」節参照）。署名済み（app-release.apk）が前提であり、
# 上記フェーズ 1 の FANDHE_ANDROID_KEYSTORE 必須化ガードで担保する
# （未署名 APK は adb install が INSTALL_PARSE_FAILED_NO_CERTIFICATES で拒否するため
# 実機計測には使えない。Review 指摘対応：旧コメントとの矛盾を解消）。
log "gradle -p ${fandhe_gradle_project_dir} :app:assembleRelease -PfandheAndroidAbis=${abi_csv}"
gradle -p "${fandhe_gradle_project_dir}" :app:assembleRelease --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"
[ -f "${fandhe_apk_path}" ] || fail "fandhe APK の生成に失敗しました: ${fandhe_apk_path}"

# --- フェーズ 4: ネイティブ比較アプリのビルド --------------------------------
if [ ! -d "${native_gradle_project_dir}" ]; then
  fail "比較アプリのプロジェクトが見つかりません: ${native_gradle_project_dir}"
fi
log "gradle -p ${native_gradle_project_dir} :app:assembleRelease"
gradle -p "${native_gradle_project_dir}" :app:assembleRelease --console=plain
[ -f "${native_apk_path}" ] || fail "比較アプリ APK の生成に失敗しました: ${native_apk_path}"

# --- フェーズ 5: インストール ------------------------------------------------
# 前回検証の残置があると別鍵での再ビルド時に署名不一致で失敗しうるため、
# 事前に uninstall を試行する（未インストール時は no-op、エラーは無視してよい。
# verify-android-launch.sh と同じ理由）。
adb_cmd uninstall "${fandhe_application_id}" >/dev/null 2>&1 || true
adb_cmd uninstall "${native_application_id}" >/dev/null 2>&1 || true

# インストール以降はどこで終了しても計測用アプリを端末へ残さない。統制条件の検証
# （フェーズ 6）は画面消灯・非充電などの通常起こりうる入力で fail し、2 本目の
# インストール自体が失敗することもあるため、1 本目の install より前に登録する
# （codex review 指摘対応、Issue #568）。未インストールへの uninstall は no-op。
cleanup_installed_apps() {
  adb_cmd uninstall "${fandhe_application_id}" >/dev/null 2>&1 || true
  adb_cmd uninstall "${native_application_id}" >/dev/null 2>&1 || true
}
trap cleanup_installed_apps EXIT

log "adb install -r -t ${fandhe_apk_path}"
adb_cmd install -r -t "${fandhe_apk_path}"
log "adb install -r -t ${native_apk_path}"
adb_cmd install -r -t "${native_apk_path}"

# --- フェーズ 6: 統制条件の確認・記録 ----------------------------------------
# 手順書 §6 が定める統制条件のうち、adb から機械的に確認できるものを計測直前に取得し、
# ログへ残す。人手の記憶に頼ると「どの条件下の値か」が後から追えず、性能ゲートの判定を
# 他プラットフォームと比較できなくなるため（codex review 指摘対応、Issue #568）。
# 取得した条件は記録するだけでなく、下部で fail-closed に検証する（充電中・節電モード無効・
# 機内モードオフ・画面点灯・ロック解除のいずれかを満たさない、または取得できない場合は中止）。
# 条件外の値がそのまま性能ゲートの結果として転記されるのを防ぐため。
log "=== 計測時の統制条件（手順書 §6） ==="
battery_dump="$(adb_cmd shell dumpsys battery 2>/dev/null | tr -d '\r' || true)"
# AC/USB/Wireless の 3 行が一致しうるため、そのままだと log が複数行に割れて
# `[measure-cold-start]` 接頭辞の付かない行が証跡へ残る。1 行へ畳んで渡す。
ac_powered="$(printf '%s\n' "${battery_dump}" | grep -E '^ *(AC|USB|Wireless) powered:' | tr -d ' ' | paste -sd' ' - || true)"
battery_level="$(printf '%s\n' "${battery_dump}" | grep -E '^ *level:' | awk '{print $2}' || true)"
low_power="$(adb_cmd shell settings get global low_power 2>/dev/null | tr -d '\r' || true)"
airplane_mode="$(adb_cmd shell settings get global airplane_mode_on 2>/dev/null | tr -d '\r' || true)"
wakefulness="$(adb_cmd shell dumpsys power 2>/dev/null | grep -oE 'mWakefulness=[A-Za-z]+' | head -n 1 | tr -d '\r' || true)"
# keyguard（ロック画面）の表示状態。フィールド名は Android バージョン・機種で異なり、
# `mDreamingLockscreen` は dream 状態のロック画面を指すもので通常の keyguard を一意には
# 表さない（codex review 指摘対応、Issue #568）。そのため候補を優先順に探索し、最初に
# 見つかったものを採る。いずれも見つからなければ判定不能として中止する（fail-closed）。
window_policy_dump="$(adb_cmd shell dumpsys window policy 2>/dev/null | tr -d '\r' || true)"

# 探索範囲は KeyguardServiceDelegate セクション内に限定する。`dumpsys window policy` 全体から
# 汎用的な `showing=` を拾うと、別 UI の状態を keyguard の状態と誤認して「ロック解除済み」と
# 判定しかねないため（codex review 指摘対応、Issue #568）。セクションは、見出し行より
# 深くインデントされた行の連なりとして切り出す。
keyguard_section="$(printf '%s\n' "${window_policy_dump}" | awk '
  /KeyguardServiceDelegate/ { header_indent = match($0, /[^ ]/); in_section = 1; print; next }
  in_section {
    indent = match($0, /[^ ]/)
    if (indent == 0 || indent <= header_indent) { in_section = 0; next }
    print
  }
')"

screen_locked=""
screen_locked_field=""
if [ -n "${keyguard_section}" ]; then
  # KeyguardServiceDelegate 配下でも Android バージョンによりフィールド名が異なるため、
  # 候補を優先順に探索する。いずれも無ければ判定不能として下の case で中止する。
  for field in 'mKeyguardShowing=(true|false)' 'mShowingLockscreen=(true|false)' 'mIsShowing=(true|false)' '(^|[^A-Za-z])showing=(true|false)'; do
    match="$(printf '%s\n' "${keyguard_section}" | grep -oE "${field}" | head -n 1 || true)"
    if [ -n "${match}" ]; then
      screen_locked_field="$(printf '%s\n' "${match}" | sed 's/^[^A-Za-z]*//')"
      screen_locked="$(printf '%s\n' "${match}" | grep -oE '(true|false)$' || true)"
      break
    fi
  done
fi
log "  電源接続      : ${ac_powered:-取得できず}（バッテリー残量 ${battery_level:-不明}%）"
log "  節電モード    : low_power=${low_power:-取得できず}（0=無効）"
log "  機内モード    : airplane_mode_on=${airplane_mode:-取得できず}（0=オフ）"
log "  画面状態      : ${wakefulness:-取得できず}"
log "  keyguard 表示 : ${screen_locked_field:-取得できず}（false=ロック解除済み）"
log "  ※ バックグラウンド通知・同期の抑制状況は adb から機械的に判定できないため、"
log "     実行者が手順書 §6 に従って確認すること"

# 画面点灯とロック解除は別条件（点灯したままロック中は mWakefulness=Awake になり得る）。
# どちらも起動が抑制されて値が歪むため、両方を fail-closed で検証する。
case "${wakefulness}" in
  mWakefulness=Awake) ;;
  "") fail "端末の画面状態（mWakefulness）を取得できませんでした。画面消灯・ロック中の
  計測は起動が抑制されて値が歪むため、状態を確認できないまま計測は行いません。
  次で確認できます: ${adb_hint} shell dumpsys power | grep mWakefulness" ;;
  *) fail "端末の画面が点灯していません（${wakefulness}）。画面を点灯・ロック解除してから
  再実行してください。消灯中は起動が抑制され、比較可能な値になりません。" ;;
esac

case "${screen_locked}" in
  false) ;;
  "") fail "端末の keyguard 表示状態を取得できませんでした（KeyguardServiceDelegate セクションが
  無いか、既知のフィールド名が含まれていません）。ロック中の計測は起動が抑制されて値が歪む
  ため、解除済みを明示的に確認できないまま計測は行いません。機種・OS バージョン依存で
  セクション名・フィールド名が異なる場合は、次の出力を確認のうえ本スクリプトの
  keyguard_section 抽出条件または探索候補へ追加してください:
    ${adb_hint} shell dumpsys window policy | grep -iE 'keyguard|lockscreen'" ;;
  *) fail "端末がロックされています（${screen_locked_field}）。ロックを解除してから再実行して
  ください。ロック中は起動が抑制され、比較可能な値になりません。" ;;
esac

# 電源接続・節電モード・機内モードも手順書 §6 の必須条件。値を記録するだけでは
# 条件外の測定値がそのまま性能ゲートの結果として転記され得るため、画面・ロックと同様に
# fail-closed で検証する（取得できない場合も中止する。codex review 指摘対応、Issue #568）。
case "${ac_powered}" in
  *ACpowered:true* | *USBpowered:true* | *Wirelesspowered:true*) ;;
  "") fail "端末の電源接続状態（dumpsys battery）を取得できませんでした。充電中であることを
  確認できないまま計測は行いません。次で確認できます:
    ${adb_hint} shell dumpsys battery" ;;
  *) fail "端末が充電されていません（${ac_powered}）。手順書 §6 は充電中での計測を求めています。
  充電ケーブルを接続してから再実行してください。非充電時は省電力制御で値が歪みます。" ;;
esac

case "${low_power}" in
  0) ;;
  "" | null) fail "節電モードの状態（settings get global low_power）を取得できませんでした。
  無効であることを確認できないまま計測は行いません。次で確認できます:
    ${adb_hint} shell settings get global low_power" ;;
  *) fail "バッテリーセーバーが有効です（low_power=${low_power}）。無効にしてから再実行して
  ください。有効時は CPU/GPU が制限され、比較可能な値になりません。" ;;
esac

case "${airplane_mode}" in
  0) ;;
  "" | null) fail "機内モードの状態（settings get global airplane_mode_on）を取得できませんでした。
  オフであることを確認できないまま計測は行いません。次で確認できます:
    ${adb_hint} shell settings get global airplane_mode_on" ;;
  *) fail "機内モードが有効です（airplane_mode_on=${airplane_mode}）。手順書 §6 は通信状態を
  安定させた条件での計測を求めています。オフにしてから再実行してください。" ;;
esac

# --- フェーズ 7: 計測本体 ----------------------------------------------------
# `am start -W -S` の出力から `TotalTime: <ms>` 行を抽出する。`-S` は起動前に
# force-stop するため、cold 条件（プロセス未存在）を毎回保証する
# （手順書「統制条件」節。画面点灯・ロック解除・機内モード等の残りの統制条件は
# 人間担当の実行手順として明記する）。
measure_one() {
  local application_id="$1"
  local activity_name="$2"
  # `adb shell` はデバイス側の CRLF 改行をそのまま中継するため、抽出値の末尾に
  # `\r` が付いたまま `sort -n` / 数値比較（`-le` 等）へ渡すと壊れる
  # （verify-android-launch.sh が `pidof` 出力に `tr -d '\r\n[:space:]'` を
  # 適用しているのと同じ理由。Review 指摘対応）。
  adb_cmd shell am start -W -S -n "${application_id}/${activity_name}" \
    | grep -E "^TotalTime:" | awk '{print $2}' | tr -d '\r'
}

# 中央値算出: `sort -n`（数値ソート）でないと桁数違いの値（例: 1000 と 200）を
# 誤って比較してしまうため、明示的に -n を付ける。
median() {
  # 引数で渡された数値列を数値順に並べ、真ん中の要素を返す（trials は奇数運用を
  # 推奨するが、偶数の場合は下側中央値を返す簡易実装で十分とする）。
  local sorted
  sorted="$(printf '%s\n' "$@" | sort -n)"
  local count
  count="$#"
  local mid_index=$(( (count + 1) / 2 ))
  printf '%s\n' "${sorted}" | sed -n "${mid_index}p"
}

run_trials() {
  local label="$1"
  local application_id="$2"
  local activity_name="$3"
  local -a values=()
  local i
  for ((i = 1; i <= trials; i++)); do
    log "  [${label}] 試行 ${i}/${trials}"
    local value
    value="$(measure_one "${application_id}" "${activity_name}")"
    [ -n "${value}" ] || fail "${label} の TotalTime 抽出に失敗しました（試行 ${i}）。
    logcat・adb の出力を確認してください。"
    log "    TotalTime=${value}ms"
    values+=("${value}")
    # 次試行への影響を避けるため、起動後に安定待ちを挟む（実機ノイズ緩和）。
    sleep 2
  done
  median "${values[@]}"
}

log "fandhe 実装の計測を開始します（${trials} 回）"
fandhe_median="$(run_trials "fandhe" "${fandhe_application_id}" "${fandhe_activity_name}")"

log "ネイティブ比較実装の計測を開始します（${trials} 回）"
native_median="$(run_trials "native" "${native_application_id}" "${native_activity_name}")"

# --- フェーズ 8: 後片付け ------------------------------------------------------
# 両アプリのアンインストールはフェーズ 5 で登録した EXIT trap が担う
# （途中終了時も確実に実行するため）。

# --- フェーズ 9: 判定・整形出力 ---------------------------------------------

absolute_gate_ms=400
absolute_verdict="NG"
if [ "${fandhe_median}" -le "${absolute_gate_ms}" ]; then
  absolute_verdict="OK"
fi

# 相対判定（参考指標）: (fandhe - native) / native * 100 <= 30 を許容とする。
# シェル算術は整数演算のため、100 倍してから除算し小数点以下概算を得る
# （bc 等の外部コマンド依存を避ける。目的は「参考指標」であり高精度は要求しない）。
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
log "  | Android | ${fandhe_median} | ${native_median} | ${relative_pct} | ${absolute_verdict} |"

log "完了。上記の結果を控えてください。"
