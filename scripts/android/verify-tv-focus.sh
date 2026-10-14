#!/usr/bin/env bash
# TASK-17.2（REQ-17 / M6、Issue #280）: Android TV エミュレータでの TV フォーカス
# ナビゲーション最小動作確認スクリプト。
#
# 位置づけ: `docs/architecture/tv-focus-navigation-architecture.md` 8 節が定める
# 後続タスク（TASK-17.2）の確認事項 1〜9 のうち、単一画面（`tv_sample.rs` の
# `tv_sample_screen()`）で機械的に再現できるものを、D-pad キーイベント送出 →
# スクリーンショット/logcat 回収まで自動化する。判定そのもの（合否）は証跡を見た
# 人間・レポート作成側が行う（機械的な pass/fail 判定は行わない。egui は独自
# レンダラのため OS 標準ウィジェットツリーに現れない）。
#
# **`uiautomator dump` は意図的に使わない**: Android の Accessibility フレームワークへ
# a11y ツリーを照会する操作（`uiautomator dump` を含む）は、`accesskit_android`
# （TASK-9.3-S2 で有効化した Android 向け accessibility 統合）の遅延初期化アダプタを
# 活性化させる。活性化後は画面内容が変化する操作のたびに accesskit_android が
# 既知のクラッシュ（`accesskit_android-0.4.2/src/event.rs:64` の `.unwrap()` が
# JNI 例外を Rust パニックへ変換、`SIGABRT`）を起こす。upstream 最新
# （accesskit_android 0.7.5）でも同一の `.unwrap()` が残存し未修正であることを確認済み
# （検証記録は非公開）。
# `uiautomator dump` を使わなければこの経路は発火しない。
#
# `scripts/android/verify-android-launch.sh`（TASK-1.6）・`verify-talkback-a11y.sh`
# （TASK-9.3-S3）との関係: フェーズ構成（前提検査 → デバイス検出/起動 → クロスビルド →
# APK 生成 → インストール・起動 → 証跡取得 → 後片付け）と fail-fast の判定ロジックは
# 両スクリプトの構成を踏襲する。共通化（関数抽出）は行わず本スクリプトへ複製している
# （理由は verify-talkback-a11y.sh 冒頭コメントと同じ: 各スクリプトが持つ固有フェーズの
# 独立した進化を妨げないため）。本スクリプトに固有な差分は次の 2 点:
#   (a) 実機ではなくエミュレータ（AVD `fandhe-tv-test`）を起動対象にする
#       （`FANDHE_ANDROID_SERIAL` 指定時は実機/既存エミュレータへの接続も可能）。
#   (b) `FANDHE_PROXY_APP=tv-focus` を `adb shell am start` の環境変数注入では実現できない
#       （Android に env var 注入の標準手段がない）ため、`android.rs` が読む
#       `debug.fandhe.proxy_app` システムプロパティを `adb shell setprop` で設定してから
#       起動する。このプロパティ読み取りは `debug-proxy-app` feature でしかコンパイル
#       されない（Issue #718、ストア提出物に切替経路を残さないため）ので、フェーズ 3 の
#       クロスビルドで同 feature を明示的に有効化する。
#
# 呼び出し元: ローカル開発者が直接実行する。CI 組み込みは行わない（エミュレータの
# ヘッドレス起動自体は CI 化可能だが、D-pad 操作結果の目視確認が前提のため、
# ci.md の CI 制約とは別に人間担当のスコープとする）。
#
# 前提: scripts/android/setup-android-env.sh 実行済み（JDK・SDK・NDK・Gradle・
# cargo-ndk 導入済み）、および `fandhe-tv-test`（tv_1080p・android-tv・arm64-v8a・API 34）
# AVD 作成済み（`avdmanager` で作成。本スクリプトは AVD の新規作成は行わない）。
#
# 環境変数インターフェース（既存スクリプトの FANDHE_ANDROID_* と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（後述のエミュレータ自動起動・自動検出を使う）。
#                           実機や既に起動済みの別エミュレータを対象にする場合に指定する。
#                           値はログへ出力しない（security.md 準拠）。
#   FANDHE_TV_AVD_NAME      既定: "fandhe-tv-test"。起動する AVD 名。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（TV AVD の ABI と一致させる）。
#   FANDHE_ANDROID_KEEP_APP 既定: 空（検証後に adb uninstall する）。非空指定で残置。
#   FANDHE_TV_KEEP_EMULATOR 既定: 空（本スクリプトが起動したエミュレータのみ検証後に
#                           終了する。既存起動済みエミュレータへ接続した場合は終了しない）。
#                           非空指定でエミュレータを起動したままにする（繰り返し検証用）。
#   FANDHE_TV_EMULATOR_PORT 既定: 5586（新規起動時に固定する emulator のポート。偶数のみ
#                           指定可、emulator の serial 命番規則 emulator-<port> に従う）。
#                           既定ポートが使用中の場合は自動で +2 ずつ空きポートを探索する。
#
# 使い方: scripts/android/verify-tv-focus.sh
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと JAVA_HOME / ANDROID_HOME /
# ANDROID_NDK_HOME / GRADLE_HOME の export を行う（既存スクリプトと同じ前提）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

# android-env.sh は PATH へ platform-tools を追加するが emulator バイナリ
# （$ANDROID_HOME/emulator）は含めない（AVD 起動は通常 Android Studio が担うため）。
# 本スクリプトはヘッドレスエミュレータを自前で起動するため追加する。
export PATH="${ANDROID_HOME}/emulator:${PATH}"

package="fandhe-egui-proxy"
lib_name="fandhe_egui_proxy"
application_id="com.fandhe.androidapp"
activity_name="com.fandhe.androidapp.MainActivity"
gradle_project_dir="${repo_root}/platforms/android"
jnilibs_out_dir="${repo_root}/target/android/jniLibs"
apk_path="${gradle_project_dir}/app/build/outputs/apk/debug/app-debug.apk"
evidence_dir="${repo_root}/_/tv-focus-verify"
proxy_app_property="debug.fandhe.proxy_app"
# フェーズ 3 のクロスビルドで ${jnilibs_out_dir} へ書き込んだかを示すフラグ（cleanup が参照する）。
jnilibs_written=""

serial="${FANDHE_ANDROID_SERIAL:-}"
avd_name="${FANDHE_TV_AVD_NAME:-fandhe-tv-test}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
keep_app="${FANDHE_ANDROID_KEEP_APP:-}"
keep_emulator="${FANDHE_TV_KEEP_EMULATOR:-}"
emulator_port_base="${FANDHE_TV_EMULATOR_PORT:-5586}"

log() {
  echo "[verify-tv-focus] $*"
}

fail() {
  echo "[verify-tv-focus] ERROR: $*" >&2
  exit 1
}

adb_cmd() {
  if [ -n "${serial}" ]; then
    adb -s "${serial}" "$@"
  else
    adb "$@"
  fi
}

emulator_pid=""
started_emulator=""
proxy_app_property_was_set=""

# 本スクリプトが起動したエミュレータのみ後片付けで終了する（人間が別途起動していた
# エミュレータ・実機を巻き込んで終了させない、verify-android-launch.sh の
# 「対象端末を明示的に絞る」設計方針と同じ理由）。
cleanup() {
  # 検証用に設定した debug.fandhe.proxy_app を必ずクリアする。system property は
  # アプリの uninstall 後も端末に残留するため、放置すると以後の通常起動まで恒常的に
  # TvFocusApp が選択され続ける（端末のグローバル状態を汚染しない）。空文字への
  # setprop は「未設定」と同じ扱いになる（android.rs 側は空文字で既定経路へ落ちる）。
  if [ -n "${proxy_app_property_was_set}" ]; then
    adb_cmd shell setprop "${proxy_app_property}" "" >/dev/null 2>&1 || true
  fi
  # 検証用に `debug-proxy-app` feature 付きでビルドした .so を共有ディレクトリに残さない
  # （Issue #718 の PR レビュー P1）。${jnilibs_out_dir} は Gradle プロジェクトが
  # jniLibs.srcDirs として常時参照する（platforms/android/app/build.gradle.kts）ため、
  # 残置すると Rust を再ビルドせずに `:app:bundleRelease` した場合に切替経路つきの .so が
  # ストア提出物へ収録され得る。フェーズ 3 に到達した実行でのみ削除する（前提検査で
  # 落ちた場合に既存の成果物を巻き添えにしない）。
  if [ -n "${jnilibs_written}" ]; then
    rm -rf "${jnilibs_out_dir}"
  fi
  if [ -n "${started_emulator}" ] && [ -z "${keep_emulator}" ] && [ -n "${emulator_pid}" ]; then
    log "起動したエミュレータを終了します（pid=${emulator_pid}）"
    kill "${emulator_pid}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

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
command -v emulator >/dev/null 2>&1 || fail "emulator が見つかりません（PATH に ANDROID_HOME/emulator が含まれているか確認してください）"

# emulator-* serial の AVD 名を照会する（`adb emu avd name` の応答は
# "<AVD 名>\nOK\n" 形式。末尾の OK 行と空行を除いた最後の行が AVD 名になる）。
get_avd_name_for_serial() {
  local candidate="$1"
  adb -s "${candidate}" emu avd name 2>/dev/null \
    | tr -d '\r' | sed '/^OK$/d; /^[[:space:]]*$/d' | tail -n1
}

# --- フェーズ 2: エミュレータ起動（既起動なら再利用） -------------------------
if [ -n "${serial}" ]; then
  log "FANDHE_ANDROID_SERIAL 指定のため既存端末/エミュレータへ接続します（自動起動しません）"
else
  # 既起動中の emulator-* を無条件に再利用しない（fail-closed）。並行して別 AVD の
  # エミュレータが起動していると誤って別端末へ uninstall/install・setprop・キー入力を
  # 行いかねないため、各候補の AVD 名を照合して対象 AVD（avd_name）と一致するものだけ
  # 採用する。一致 0 件なら新規起動、2 件以上なら曖昧としてエラー終了する。
  candidate_serials="$(adb devices | tail -n +2 | grep -v '^\*' | awk '$2 == "device" { print $1 }' | grep '^emulator-' || true)"
  matched_serial=""
  matched_count=0
  if [ -n "${candidate_serials}" ]; then
    while IFS= read -r candidate; do
      [ -n "${candidate}" ] || continue
      candidate_avd="$(get_avd_name_for_serial "${candidate}")"
      if [ "${candidate_avd}" = "${avd_name}" ]; then
        matched_serial="${candidate}"
        matched_count=$((matched_count + 1))
      fi
    done <<< "${candidate_serials}"
  fi

  if [ "${matched_count}" -gt 1 ]; then
    fail "AVD '${avd_name}' に一致する起動中エミュレータが複数見つかりました。対象を FANDHE_ANDROID_SERIAL で明示的に指定してください。"
  fi

  if [ "${matched_count}" -eq 1 ]; then
    log "対象 AVD（${avd_name}）に一致する起動中エミュレータを再利用します"
    serial="${matched_serial}"
  else
    command -v avdmanager >/dev/null 2>&1 || fail "avdmanager が見つかりません（PATH に ANDROID_HOME/cmdline-tools/latest/bin が含まれているか確認してください）"
    avdmanager list avd 2>/dev/null | grep -q "Name: ${avd_name}\$" \
      || fail "AVD '${avd_name}' が見つかりません。事前に avdmanager で作成してください（人間担当手順）。"

    [ $((emulator_port_base % 2)) -eq 0 ] \
      || fail "FANDHE_TV_EMULATOR_PORT（${emulator_port_base}）は偶数を指定してください（emulator の serial 命番規則）。"

    # 起動前に serial（emulator-<port>）を固定するため、空きポートを探索する。
    # ponytail: `adb -s emulator-<port> get-state` が失敗する（デバイス未応答）ことを
    # 「空きポート」の代理指標にする簡易ヒューリスティック。OS レベルの真のポート占有
    # チェック（lsof 等）ではないが、既存エミュレータとの serial 衝突回避には十分。
    # 真のポート空き確認が必要になったら lsof -i へ切り替える。
    emulator_port=""
    for offset in $(seq 0 2 40); do
      candidate_port=$((emulator_port_base + offset))
      candidate_serial="emulator-${candidate_port}"
      if ! adb -s "${candidate_serial}" get-state >/dev/null 2>&1; then
        emulator_port="${candidate_port}"
        break
      fi
    done
    [ -n "${emulator_port}" ] \
      || fail "空きエミュレータポートが見つかりませんでした（${emulator_port_base} から +40 までの偶数 21 ポートを確認）。FANDHE_TV_EMULATOR_PORT で別ポートを指定してください。"

    log "エミュレータを起動します（AVD: ${avd_name}、ヘッドレス、port=${emulator_port}）"
    emulator -avd "${avd_name}" -port "${emulator_port}" -no-window -no-audio -no-boot-anim >/dev/null 2>&1 &
    emulator_pid=$!
    started_emulator="1"
    serial="emulator-${emulator_port}"

    # R1（identifier leak）: 接続先の識別子はログへ出さない（直前行で port は出力済み）。
    log "デバイス検出を待機します"
    device_detected=""
    for _ in $(seq 1 60); do
      if adb -s "${serial}" get-state 2>/dev/null | grep -q '^device$'; then
        device_detected="1"
        break
      fi
      sleep 2
    done
    [ -n "${device_detected}" ] || fail "エミュレータの起動を検出できませんでした（120 秒待機、serial=${serial}）。手動で 'emulator -avd ${avd_name} -port ${emulator_port}' を起動して調査してください。"

    log "boot_completed を待機します"
    for _ in $(seq 1 60); do
      boot_completed="$(adb -s "${serial}" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r\n' || true)"
      [ "${boot_completed}" = "1" ] && break
      sleep 2
    done
    [ "${boot_completed}" = "1" ] || fail "エミュレータの boot_completed を確認できませんでした（120 秒待機）。"
  fi
fi
log "対象デバイス検出 OK（起動確認済み）"

# --- フェーズ 3: クロスビルド（release、jniLibs レイアウト出力） -------------
cd "${repo_root}"

log "jniLibs 出力先をクリア: ${jnilibs_out_dir}"
rm -rf "${jnilibs_out_dir}"

declare -a cargo_ndk_args=()
# 外部入力 FANDHE_ANDROID_ABIS を空白区切りで配列化する。未クォート展開
# `(${abis})` は word splitting に加えて pathname expansion（glob）まで発生し、
# `*` 等が作業ツリー内のファイル名へ展開されて cargo ndk / Gradle に渡る
# 恐れがあるため、glob を発生させない read -a を使う。
read -r -a abi_list <<< "${abis}"
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
  # Issue #718: `debug.fandhe.proxy_app` フォールバックは opt-in feature。
  # 検証専用ビルドのためここでのみ有効化する（配布ビルドでは無効のまま）。
  --features debug-proxy-app
)

log "cargo ndk ${cargo_ndk_args[*]}"
jnilibs_written="1"
# 呼び出し元シェルの FANDHE_PROXY_APP を明示的に落としてからビルドする（Issue #718 の PR レビュー P1）。
# `android_main` は `screens::selected_app_name()` 経由でビルド時 `option_env!("FANDHE_PROXY_APP")` を
# 解決するため、環境変数が設定されたシェルから本スクリプトを実行するとその値がバイナリへ焼き込まれ、
# 後段の `adb shell setprop debug.fandhe.proxy_app tv-focus` が参照されない（本スクリプトが検証したい
# system property 経路そのものが無効化される）。ここで `tv-focus` を焼き込む選択は取らない
# （それでは setprop 経路を検証したことにならないため）。
env -u FANDHE_PROXY_APP cargo ndk "${cargo_ndk_args[@]}"

for abi in "${abi_list[@]}"; do
  so_path="${jnilibs_out_dir}/${abi}/lib${lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
  log "  - ${so_path} 存在確認 OK"
done

# --- フェーズ 4: APK 生成（debug、起動確認専用） ------------------------------
abi_csv="$(IFS=,; echo "${abi_list[*]}")"
log "gradle -p ${gradle_project_dir} :app:assembleDebug -PfandheAndroidAbis=${abi_csv}"
gradle -p "${gradle_project_dir}" :app:assembleDebug --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"

[ -f "${apk_path}" ] || fail "APK の生成に失敗しました: ${apk_path}"
log "APK 生成 OK: ${apk_path}"

# --- フェーズ 5: インストール・起動（TV フォーカスサンプルへ切替） -------------
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
adb_cmd logcat -c

log "adb shell setprop ${proxy_app_property} tv-focus"
adb_cmd shell setprop "${proxy_app_property}" tv-focus
proxy_app_property_was_set=1

log "adb install -r ${apk_path}"
adb_cmd install -r "${apk_path}"

log "adb shell am start -n ${application_id}/${activity_name}"
adb_cmd shell am start -n "${application_id}/${activity_name}"

sleep 5

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

# --- フェーズ 6: D-pad 操作シーケンス・証跡取得 --------------------------------
# 各ステップは 8 節の確認事項（コメントで番号を付す）に対応する。判定は行わず、
# キー送出直後のスクリーンショットを集めるのみ（目視・レポート側での判定に委ねる、
# 本スクリプト冒頭コメント参照）。
#
# **`uiautomator dump` は使わない（重要・既知のクラッシュ誘発手順）**: `uiautomator dump`
# は Android の Accessibility フレームワークへ a11y ツリーを照会するため、これを 1 回
# 実行しただけで `accesskit_android`（TASK-9.3-S2 で有効化した accessibility 統合）の
# 遅延初期化アダプタが活性化する。活性化後は、画面内容が変化する操作（ラベル変更・
# `VirtualList` のスクロール等）のたびに `accesskit_android` が
# `ViewParent.requestSendAccessibilityEvent` を JNI 経由で呼ぼうとして
# `JavaException` を投げ、`accesskit_android-0.4.2/src/event.rs:64` の `.unwrap()` が
# そのまま Rust パニック（`SIGABRT`）に変換される（既知の不具合、upstream 最新
# （accesskit_android 0.7.5）でも同一の `.unwrap()` が残存し未修正）。TalkBack の有効/無効
# （`accessibility_enabled` システム設定）には依存しない——`uiautomator dump` 自体が
# a11y クエリの発生源になる。`uiautomator dump` を一切使わなければこの経路は
# 発火せず、確認事項 2・4・7・8・9 を含む全シーケンスがクラッシュせず完走することを
# コールドブートしたエミュレータで実証済み。
mkdir -p "${evidence_dir}"
rm -f "${evidence_dir}"/*.png "${evidence_dir}"/*.txt 2>/dev/null || true

step_no=0

# `adb shell input keyevent` はキー名（KEYCODE_DPAD_* 等）を直接受け付ける。
# GameActivity 経由で実際に KeyEvent が配信されるか自体が確認事項 6 の対象であり、
# ここでは配信手段（emulator の仮想リモコン相当）として最も標準的な経路を使う。
send_key() {
  local keycode="$1"
  local repeat="${2:-1}"
  local i
  for ((i = 0; i < repeat; i++)); do
    adb_cmd shell input keyevent "${keycode}"
  done
}

capture() {
  local name="$1"
  step_no=$((step_no + 1))
  local prefix
  prefix="$(printf '%02d' "${step_no}")"
  local shot_path="${evidence_dir}/${prefix}-${name}.png"
  adb_cmd exec-out screencap -p > "${shot_path}"
  [ -s "${shot_path}" ] || fail "スクリーンショット取得に失敗しました: ${shot_path}"
  log "  - 証跡取得: ${shot_path}"
}

log "確認事項 1・6: 初期フォーカス（棚1 先頭）を確認"
capture "initial-shelf1"

log "確認事項 1: Down で棚1→棚2 へ（棚内を横移動しない）"
send_key KEYCODE_DPAD_DOWN
capture "down-shelf1-to-shelf2"

log "確認事項 3: Right で棚2 内の隣接項目へ移動"
send_key KEYCODE_DPAD_RIGHT 2
capture "right-in-shelf2"

log "確認事項 3: 棚2 右端で Right → no-op（同一項目に留まる）"
send_key KEYCODE_DPAD_RIGHT
capture "right-edge-noop"

# 確認事項 2（DPAD_CENTER での決定操作）: 可視領域（棚 2、画面上部）にあるボタンで
# 1 回 probe する。ラベルの押下回数表示がスクリーンショットで直接確認できる位置を
# 選ぶ（grid/大規模リストは画面下部の可視域外に描画されるため、決定操作の証跡としては
# 使えない。トップレベル `Screen` 自体はスクロールしない設計であり、`_/tv-focus-verify/`
# の実測で 1080p 解像度だと棚 2 段の直後で画面下端に達することを確認済み）。
# クラッシュのおそれ（accesskit_android の既知の不具合）は `uiautomator dump` を
# 使わない限り発生しないことをコールドブート実験で確認済みのため、シーケンス中盤で
# 実行してよい。
log "確認事項 2・6: Enter/決定（DPAD_CENTER、可視領域の棚 2 で probe）"
send_key KEYCODE_DPAD_CENTER
capture "decide-shelf2-visible"

log "確認事項 1: Up で棚2→棚1 へ戻る"
send_key KEYCODE_DPAD_UP
capture "up-back-to-shelf1"

log "確認事項 9: ネストしたリストへ descend し、可視窓外まで連続移動（多段 reveal）"
send_key KEYCODE_DPAD_DOWN 2
capture "enter-nested-list"
send_key KEYCODE_DPAD_DOWN 40
capture "nested-list-deep-scroll"

log "確認事項 4・9: Grid 区画まで移動（内側リスト・外側グループを抜ける）"
send_key KEYCODE_DPAD_DOWN 60
capture "grid-focus"

log "確認事項 4: Grid 行内で Right（行境界を越えない）"
send_key KEYCODE_DPAD_RIGHT 2
capture "grid-right-in-row"
send_key KEYCODE_DPAD_RIGHT
capture "grid-right-row-boundary-noop"

log "確認事項 4: Grid で Down（最終不完全行への到達を含む）"
send_key KEYCODE_DPAD_DOWN 2
capture "grid-down-to-last-row"

log "確認事項 4: Grid 最上行での Up → 棚（祖先コンテナ）へエスケープ確認用に一旦上へ戻す"
send_key KEYCODE_DPAD_UP 2
capture "grid-up-escape"
send_key KEYCODE_DPAD_DOWN 2
capture "grid-back-down"

log "確認事項 7・8・9: 大規模 VirtualList（1200 件）へ進入し高速連打"
send_key KEYCODE_DPAD_DOWN 3
capture "biglist-entry"

# macOS の `date`（BSD 版）は GNU 拡張の `%N`（ナノ秒）を解釈しないため、
# ミリ秒精度は使わず秒単位で計測する（`確認事項 8` の参考値としては十分な精度）。
biglist_stress_start_s="$(date +%s)"
send_key KEYCODE_DPAD_DOWN 150
biglist_stress_end_s="$(date +%s)"
capture "biglist-after-150-downs"
log "  - 大規模リスト連打所要時間: $((biglist_stress_end_s - biglist_stress_start_s)) 秒（150 回、確認事項 8 の参考値）"

send_key KEYCODE_DPAD_UP 80
capture "biglist-scroll-up"

log "確認事項 2・6・7・8: 大規模リスト連打の深部で Enter/決定（DPAD_CENTER）probe"
send_key KEYCODE_DPAD_CENTER
sleep 1
decide_pid="$(adb_cmd shell pidof "${application_id}" | tr -d '\r\n[:space:]' || true)"
if [ "${decide_pid}" = "${pid}" ]; then
  log "  - プロセス生存確認 OK（決定操作後もクラッシュせず、pid=${decide_pid} 維持）"
  capture "decide-probe-deep-survived"
else
  log "  - 警告: 決定操作後にプロセスが消失しました（logcat で詳細確認してください）。"
  capture "decide-probe-deep-crashed"
fi

# --- フェーズ 6b: logcat 抜粋（確認事項 6・7・8 のクラッシュ・エラー有無） -----
logcat_path="${evidence_dir}/logcat-session.txt"
adb_cmd logcat -d --pid="${pid}" > "${logcat_path}" 2>/dev/null || true
log "  - logcat 全量取得: ${logcat_path}"
session_crash_log="$(grep -iE "FATAL EXCEPTION|tombstone|libc.*abort|Fatal signal|SIGSEGV|SIGABRT|SIGBUS|SIGILL|panic" "${logcat_path}" || true)"
if [ -n "${session_crash_log}" ]; then
  log "  - 警告: セッション中の logcat にクラッシュ/panic 痕跡があります（レポートへ転記してください）:
${session_crash_log}"
else
  log "  - セッション中の logcat にクラッシュ/panic 痕跡なし"
fi

# --- フェーズ 7: 後片付け ------------------------------------------------------
if [ -z "${keep_app}" ]; then
  log "adb uninstall ${application_id}"
  adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
else
  log "FANDHE_ANDROID_KEEP_APP 指定のため端末にアプリを残置します"
fi

log "検証完了: 証跡一式は ${evidence_dir} を参照してください"
log "確認事項 1〜9 の合否・証跡要点を手元の検証記録へ記録してください"
