#!/usr/bin/env bash
# TASK-5.3（REQ-5 / M4、Issue #238）: Android 実機（Pixel 10a）でリフレッシュレートを
# 固定 60Hz へ強制した上で `frame-metrics` feature 有効ビルドを起動し、
# `AndroidApp::internal_data_path()` 配下に書き出された計測結果 JSON を回収する。
#
# 位置づけ: PoC-4（`docs/spec/03-poc/performance/README.md`）が「条件付き達成・参考値」
# に留めた macOS プロキシ実測を、固定 60Hz 環境（実機の可変リフレッシュレートを
# 明示的に固定した状態）で再計測するための手順
# （検証記録は非公開）から呼ばれる。
#
# `scripts/android/verify-android-launch.sh`（TASK-1.6 / #230）のフェーズ構成・
# 環境変数規約（`FANDHE_ANDROID_SERIAL` 等、値はログへ出力しない）をそのまま踏襲し、
# 重複実装を避けるためビルド・インストール・起動確認のロジックは同スクリプトの
# フェーズ 1〜6 と同型のまま `--features frame-metrics` の追加・リフレッシュレート
# 固定/復元・結果 JSON 回収のみを追加する。
#
# 環境変数インターフェース（verify-android-launch.sh と同じ設計方針）:
#   FANDHE_ANDROID_SERIAL   既定: 空（`adb devices` で 1 台のみ接続されている前提）。
#                           値はログへ出力しない（security.md 準拠）。
#   FANDHE_ANDROID_ABIS     既定: "arm64-v8a"（実機（Pixel 10a）向けの単一 ABI）。
#   FANDHE_ANDROID_WAIT_SECONDS 既定: 20（warmup 120 フレーム + サンプル 600 フレーム
#                           ＝ 60Hz 環境で約 12 秒。起動・初期化猶予込みの待機秒数）。
#
# 使い方: scripts/android/measure-frame-rate.sh
# 前提: scripts/android/setup-android-env.sh 実行済み。Pixel 10a を USB 接続し、
#   USB デバッグを有効化・許可済みであること（人間担当手順）。
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"
# shellcheck source=lib/sf-layer.sh
# shellcheck disable=SC1091
. "${script_dir}/lib/sf-layer.sh"

package="fandhe-egui-proxy"
lib_name="fandhe_egui_proxy"
application_id="com.fandhe.androidapp"
activity_name="com.fandhe.androidapp.MainActivity"
gradle_project_dir="${repo_root}/platforms/android"
jnilibs_out_dir="${repo_root}/target/android/jniLibs-frame-metrics"
apk_path="${gradle_project_dir}/app/build/outputs/apk/debug/app-debug.apk"
evidence_dir="${repo_root}/_/verification/task-5.3"
result_json_path="${evidence_dir}/android-frame-metrics-$(date +%Y%m%d-%H%M%S).json"
screenshot_path="${evidence_dir}/android-overlay-$(date +%Y%m%d-%H%M%S).png"
# 実効リフレッシュレートの検証結果（active mode ID・fps・判定）のみを証跡として残す。
# `dumpsys display` の全文は端末固有情報を含むため保存しない。
refresh_rate_evidence_path="${evidence_dir}/android-refresh-rate-$(date +%Y%m%d-%H%M%S).txt"

serial="${FANDHE_ANDROID_SERIAL:-}"
abis="${FANDHE_ANDROID_ABIS:-arm64-v8a}"
wait_seconds="${FANDHE_ANDROID_WAIT_SECONDS:-20}"

log() {
  echo "[measure-frame-rate:android] $*"
}

fail() {
  echo "[measure-frame-rate:android] ERROR: $*" >&2
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
# スクリプトが見たのと同じ出力を再現できない。ただし serial の実値はログへ出さない契約の
# ため、値ではなく変数参照の形（`adb -s "$FANDHE_ANDROID_SERIAL"`）で提示する。
adb_hint="adb"
if [ -n "${serial}" ]; then
  adb_hint='adb -s "$FANDHE_ANDROID_SERIAL"'
fi

# --- フェーズ 0: リフレッシュレート固定・復元の trap 登録 ---------------------
# 異常終了時も端末設定を元に戻す（`.claude/rules/security.md` A05 対策:
# 検証専用の設定変更を端末に残置しない）。固定前の値を保持し、未計測時は
# "null"（未設定）へ戻す。
original_min_refresh_rate=""
original_peak_refresh_rate=""
refresh_rate_overridden=0

# standard_jnilibs_dir（Gradle 参照用共有パス）はビルド・アプリ・verify-android-launch.sh
# が共有する成果物であり、フェーズ 5 で計測用 .so により恒久的に上書きしてはならない
# （上書きしたままだと、後続で build-aab.sh 等が再ビルドを忘れた場合に
# frame-metrics 用バイナリが混入しうる）。上書き前の状態を退避し、EXIT 時に
# リフレッシュレート設定と対称的に必ず復元する。
standard_jnilibs_dir="${repo_root}/target/android/jniLibs"
standard_jnilibs_backup_dir="${repo_root}/target/android/.jniLibs-frame-metrics-backup"
jnilibs_overridden=0
# 退避対象の元データが実在したかどうかを、退避処理（rm/mv）の着手前に確定させておく
# フラグ。バックアップディレクトリの実在だけで復元要否を判定すると、
# 「stale backup を rm -rf で消す」から「mv で退避する」までの間に割り込みが入った
# 場合に、バックアップが無い＝元々何も無かったと誤認して standard_jnilibs_dir
# （まだ mv されておらず元データがそのまま残っている）を rm -rf してしまう
# （共有アセットの意図しない消失）。元データの実在を退避処理と独立に記録しておくことで、
# 「バックアップ不在」を「復元不要（何もしない）」と「削除してよい（元々何も無かった）」の
# 2 通りに正しく区別する。
jnilibs_original_existed=0

restore_standard_jnilibs() {
  if [ "${jnilibs_overridden}" -eq 1 ]; then
    log "Gradle 参照用 jniLibs（${standard_jnilibs_dir}）を計測前の状態へ復元します"
    if [ "${jnilibs_original_existed}" -eq 1 ]; then
      if [ -d "${standard_jnilibs_backup_dir}" ]; then
        rm -rf "${standard_jnilibs_dir}"
        mv "${standard_jnilibs_backup_dir}" "${standard_jnilibs_dir}"
      fi
      # バックアップが無い場合は退避（mv）がまだ完了していない状態であり、
      # standard_jnilibs_dir には元データがそのまま残っているため何もしない
      # （ここで rm -rf すると元データを消してしまう）。
    else
      # 元々 standard_jnilibs_dir が存在しなかった場合のみ、計測用に配置した
      # 成果物（あれば）を安全に削除できる。
      rm -rf "${standard_jnilibs_dir}"
    fi
  fi
}

restore_refresh_rate() {
  if [ "${refresh_rate_overridden}" -eq 1 ]; then
    log "リフレッシュレート設定を復元します（min=${original_min_refresh_rate:-null}, peak=${original_peak_refresh_rate:-null}）"
    if [ -n "${original_min_refresh_rate}" ] && [ "${original_min_refresh_rate}" != "null" ]; then
      adb_cmd shell settings put system min_refresh_rate "${original_min_refresh_rate}" >/dev/null 2>&1 || true
    else
      adb_cmd shell settings delete system min_refresh_rate >/dev/null 2>&1 || true
    fi
    if [ -n "${original_peak_refresh_rate}" ] && [ "${original_peak_refresh_rate}" != "null" ]; then
      adb_cmd shell settings put system peak_refresh_rate "${original_peak_refresh_rate}" >/dev/null 2>&1 || true
    else
      adb_cmd shell settings delete system peak_refresh_rate >/dev/null 2>&1 || true
    fi
  fi
}

cleanup_on_exit() {
  restore_standard_jnilibs
  restore_refresh_rate
}
trap cleanup_on_exit EXIT
# INT/TERM は既定ではハンドラ実行後にスクリプトが継続してしまうため、
# 明示的に exit して EXIT トラップ側で後片付けを一度だけ行う。
trap 'exit 130' INT
trap 'exit 143' TERM

# --- フェーズ 1: 前提検査（fail-fast） ---------------------------------------
command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません"
[ -d "${GRADLE_HOME}" ] || fail "GRADLE_HOME（${GRADLE_HOME}）が見つかりません"
[ -d "${JAVA_HOME}" ] || fail "JAVA_HOME（${JAVA_HOME}）が見つかりません"
command -v gradle >/dev/null 2>&1 || fail "gradle が見つかりません"
command -v adb >/dev/null 2>&1 || fail "adb が見つかりません"

mkdir -p "${evidence_dir}"

# --- フェーズ 2: デバイス検出（verify-android-launch.sh と同じロジック） -----
device_lines="$(adb devices | tail -n +2 | grep -v '^\*' | sed '/^[[:space:]]*$/d')"
[ -n "${device_lines}" ] || fail "接続されている Android 端末がありません（Pixel 10a を USB 接続してください）"

device_count="$(printf '%s\n' "${device_lines}" | grep -c "device$" || true)"
if [ -z "${serial}" ] && [ "${device_count}" -gt 1 ]; then
  fail "複数台の端末が接続されています。FANDHE_ANDROID_SERIAL で対象端末を指定してください"
fi
[ "${device_count}" -ge 1 ] || fail "device 状態の端末が見つかりません（unauthorized/offline のみ検出）"

log "デバイス検出 OK（${device_count} 台、device 状態）"

# --- フェーズ 3: リフレッシュレート固定 --------------------------------------
original_min_refresh_rate="$(adb_cmd shell settings get system min_refresh_rate 2>/dev/null | tr -d '\r')"
original_peak_refresh_rate="$(adb_cmd shell settings get system peak_refresh_rate 2>/dev/null | tr -d '\r')"
refresh_rate_overridden=1

log "リフレッシュレートを 60Hz へ固定します（min_refresh_rate=60.0, peak_refresh_rate=60.0）"
adb_cmd shell settings put system min_refresh_rate 60.0
adb_cmd shell settings put system peak_refresh_rate 60.0

# 設定要求が端末へ反映されたことを証跡として残し、60Hz 近傍でなければ計測を中止する。
# 実効値が違うまま計測すると、他プラットフォームの結果と比較できない値を「60Hz 固定」
# として記録してしまうため（codex review 指摘対応、Issue #570）。
#
# dumpsys display 全体の先頭 `refreshRate=` を拾うと、非アクティブな対応モードや外部
# ディスプレイの値を掴み得る。対象は内蔵ディスプレイの active mode に限定するため、
# `DisplayDeviceInfo{...}` 行から `modeId <N>` を読み、同じ行の supportedModes に含まれる
# `{id=<N>, ..., fps=<X>}` の fps を採る。判別できない場合は fail-closed とする。
# `dumpsys display` の全文には uniqueId・物理アドレス・VirtualDisplay の所有パッケージ
# 等の端末固有情報が含まれ得るため、リポジトリ配下へは保存しない（security.md /
# 個体識別子の記載禁止方針）。証跡として残すのは、検証に必要な active mode の
# ID・fps・判定結果だけを組み立てたサニタイズ済みサマリとする。
display_dump="$(adb_cmd shell dumpsys display 2>/dev/null || true)"

# `DisplayDeviceInfo` には virtual display・外部ディスプレイの分も含まれるため、
# 件数を数える前に内蔵ディスプレイへ絞り込む（`type INTERNAL` または既定ディスプレイの
# flag を持つ行のみ）。絞り込んだ結果が 1 件でなければ判定不能として中止する。
internal_device_lines="$(printf '%s\n' "${display_dump}" \
  | grep 'DisplayDeviceInfo{' \
  | grep -E 'type INTERNAL|FLAG_DEFAULT_DISPLAY|DEFAULT_DISPLAY' || true)"
device_info_count="$(printf '%s\n' "${internal_device_lines}" | grep -c 'DisplayDeviceInfo{' || true)"
if [ "${device_info_count}" -ne 1 ]; then
  fail "dumpsys display から内蔵ディスプレイを一意に特定できません
  （type INTERNAL / DEFAULT_DISPLAY を持つ DisplayDeviceInfo 行が ${device_info_count} 件）。
  外部ディスプレイ・キャストを切断してから
  再実行してください。実効リフレッシュレートを確定できないまま計測すると、比較可能性の無い
  値を『60Hz 固定』として記録することになるため中止します。
  状況は次で確認できます（出力は端末固有情報を含むためリポジトリへ保存しないこと）:
    ${adb_hint} shell dumpsys display | grep DisplayDeviceInfo"
fi

device_info_line="$(printf '%s\n' "${internal_device_lines}" | grep -m 1 'DisplayDeviceInfo{' || true)"
# set -o pipefail 下では grep が不一致で終了 1 を返し、下の空判定 fail に到達せず
# 診断なしで落ちる。意図した診断メッセージを出すため `|| true` で受ける。
active_mode_id="$(printf '%s\n' "${device_info_line}" | grep -oE 'modeId [0-9]+' | head -n 1 | awk '{print $2}' || true)"
effective_hz=""
if [ -n "${active_mode_id}" ]; then
  effective_hz="$(printf '%s\n' "${device_info_line}" \
    | grep -oE "\\{id=${active_mode_id}, [^}]*\\}" \
    | grep -oE 'fps=[0-9.]+' | head -n 1 | cut -d= -f2 || true)"
fi
if [ -z "${effective_hz}" ]; then
  fail "dumpsys display から active mode（modeId=${active_mode_id:-未取得}）の実効リフレッシュ
  レートを特定できませんでした。60Hz 固定が端末へ反映されたことを確認できないため中止します。
  機種依存の出力形式であれば、次の出力を確認のうえ本スクリプトの DisplayDeviceInfo 解析
  パターンを実出力に合わせて修正してください（出力は端末固有情報を含むためリポジトリへ
  保存しないこと）:
    ${adb_hint} shell dumpsys display | grep DisplayDeviceInfo"
fi
log "dumpsys display 実効値: active modeId=${active_mode_id} / ${effective_hz}Hz"

# 59.0〜61.0Hz を許容範囲とする（端末は 59.94 等の実測値を返すことがある）。
if ! awk -v hz="${effective_hz}" 'BEGIN{exit !(hz>=59.0 && hz<=61.0)}'; then
  fail "実効リフレッシュレートが ${effective_hz}Hz であり 60Hz 固定が反映されていません。
  端末の設定（開発者オプションの強制リフレッシュレート・省電力モード等）を確認してから
  再実行してください。"
fi
log "実効リフレッシュレート ${effective_hz}Hz を 60Hz 固定として確認しました"
{
  echo "# TASK-5.3 実効リフレッシュレート検証サマリ"
  echo "# dumpsys display の全文は端末固有情報を含むため保存しない（security.md 準拠）。"
  echo "# 検証に必要な値のみを本ファイルへ残す。"
  echo "active_mode_id=${active_mode_id}"
  echo "effective_refresh_rate_hz=${effective_hz}"
  echo "verdict=60Hz固定として確認（許容範囲 59.0-61.0Hz）"
} > "${refresh_rate_evidence_path}"
log "実効リフレッシュレートの証跡を保存しました: ${refresh_rate_evidence_path}"

# --- フェーズ 4: クロスビルド（release、frame-metrics feature 有効） ---------
cd "${repo_root}"
rm -rf "${jnilibs_out_dir}"

declare -a cargo_ndk_args=()
# shellcheck disable=SC2206
abi_list=(${abis})
[ "${#abi_list[@]}" -gt 0 ] || fail "FANDHE_ANDROID_ABIS が空です"
for abi in "${abi_list[@]}"; do
  cargo_ndk_args+=(-t "${abi}")
done
cargo_ndk_args+=(
  --platform "${ANDROID_MIN_API}"
  -o "${jnilibs_out_dir}"
  build
  -p "${package}"
  --release
  --features frame-metrics
)

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

for abi in "${abi_list[@]}"; do
  so_path="${jnilibs_out_dir}/${abi}/lib${lib_name}.so"
  [ -f "${so_path}" ] || fail ".so が見つかりません: ${so_path}"
done

# --- フェーズ 5: APK 生成（debug、計測専用） ---------------------------------
# jniLibs 出力先を Gradle プロジェクトの通常ビルド（verify-android-launch.sh 用の
# target/android/jniLibs）とは別ディレクトリ（jniLibs-frame-metrics）にしているため、
# Gradle 側の参照先も本スクリプト専用に切り替える必要がある。
# build.gradle.kts の jniLibs.srcDirs は固定パス（target/android/jniLibs）のため、
# 計測用 .so を一時的にそこへ配置する。build-aab.sh・verify-android-launch.sh と
# 共有するパスを恒久的に上書きしないよう、上書き前の内容を退避しておき、
# EXIT トラップ（cleanup_on_exit）で必ず元へ戻す（リフレッシュレート復元と対称）。
# 中断タイミングによらずトラップが正しく判定できるよう、破壊的操作（mv）の前に
# フラグを立てる（Ctrl-C 等の割り込みが mv の直前で発生しても復元処理が働く）。
# 元データの実在は rm/mv 前の非破壊な存在確認で確定させ、`jnilibs_original_existed`
# に記録する（restore_standard_jnilibs 側のコメント参照）。
jnilibs_overridden=1
if [ -d "${standard_jnilibs_dir}" ]; then
  jnilibs_original_existed=1
  rm -rf "${standard_jnilibs_backup_dir}"
  mv "${standard_jnilibs_dir}" "${standard_jnilibs_backup_dir}"
fi
log "計測用 .so を Gradle 参照先へコピー: ${jnilibs_out_dir} -> ${standard_jnilibs_dir}（計測後に自動復元）"
cp -R "${jnilibs_out_dir}" "${standard_jnilibs_dir}"

abi_csv="$(IFS=,; echo "${abi_list[*]}")"
log "gradle -p ${gradle_project_dir} :app:assembleDebug -PfandheAndroidAbis=${abi_csv}"
gradle -p "${gradle_project_dir}" :app:assembleDebug --console=plain \
  "-PfandheAndroidAbis=${abi_csv}"

[ -f "${apk_path}" ] || fail "APK の生成に失敗しました: ${apk_path}"
log "APK 生成 OK: ${apk_path}"

# --- フェーズ 6: インストール・起動 -------------------------------------------
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true
adb_cmd logcat -c

log "adb install -r ${apk_path}"
adb_cmd install -r "${apk_path}"

log "adb shell am start -n ${application_id}/${activity_name}"
adb_cmd shell am start -n "${application_id}/${activity_name}"

# SurfaceFlinger --latency のリングバッファは直近 128 フレームのみ保持する。
# 計測窓（wait_seconds のサンプル区間）が始まる前にクリアしておかないと、
# 起動・初期化中のフレームや別操作分が drop_rate に混入し得る
# （codex P1 レビュー指摘、Issue #658）。measure-frame-drop-rate.sh 実機モードの
# クリア＋区間限定と同じ規則に揃えるため、計測窓の直前でレイヤを確定して
# --latency-clear を実行し、以降はここで確定したレイヤをフェーズ 7 でも使い回す
# （再選択すると計測窓の前後でレイヤ選択がずれ得るため）。
sf_layer=""
sf_layer_wait_seconds="${FANDHE_ANDROID_SF_LAYER_WAIT_SECONDS:-3}"
log "SurfaceFlinger レイヤ確定・latency クリアのため ${sf_layer_wait_seconds} 秒待機します"
sleep "${sf_layer_wait_seconds}"
sf_list=""
# `set -euo pipefail` 下では adb/dumpsys の失敗が代入式の外へ伝播し、
# スクリプト全体を中断させる（337・341・377 行目で謳う「presentation 間隔の
# 集計は警告してスキップし、主目的の frame-metrics 回収を阻害しない」契約に
# 反する。codex P1 レビュー指摘、Issue #658）。`if ... ; then` の条件内で
# 実行し、SurfaceFlinger 非対応端末や一時的な adb エラーでも後続の
# frame-metrics 回収（フェーズ 6 以降）を継続できるようにする。
if sf_list="$(adb_cmd shell dumpsys SurfaceFlinger --list 2>/dev/null | tr -d '\r')"; then
  if sf_layer="$(select_surfaceflinger_layer "${sf_list}" "${application_id}")"; then
    if adb_cmd shell dumpsys SurfaceFlinger --latency-clear "${sf_layer}" >/dev/null 2>&1; then
      log "  - SurfaceFlinger latency バッファをクリアしました（レイヤ: ${sf_layer}）"
    else
      log "警告: latency バッファのクリアに失敗しました（レイヤ: ${sf_layer}）。presentation 間隔の集計をスキップします"
      sf_layer=""
    fi
  else
    log "警告: SurfaceFlinger レイヤを特定できず、presentation 間隔の集計をスキップします"
  fi
else
  log "警告: SurfaceFlinger レイヤ一覧の取得に失敗しました。presentation 間隔の集計をスキップします"
fi

log "計測完了まで ${wait_seconds} 秒待機します（warmup 120 フレーム + サンプル 600 フレーム）"
sleep "${wait_seconds}"

pid="$(adb_cmd shell pidof "${application_id}" | tr -d '\r\n[:space:]' || true)"
if [ -z "${pid}" ]; then
  fail "プロセスが確認できません（起動直後にクラッシュした可能性）。logcat を確認してください。"
fi
log "  - プロセス生存確認 OK（pid=${pid}）"

# --- フェーズ 7: 結果 JSON 回収・エビデンス取得 -------------------------------
# `frame_metrics.rs::run_android` が AndroidApp::internal_data_path() 配下へ
# 書き出した frame-metrics.json を run-as 経由で回収する（Android では stdout/stderr が
# logcat に直結しないため、ファイル経由の回収が唯一の確実な手段）。
log "adb shell run-as ${application_id} cat files/frame-metrics.json"
if adb_cmd shell run-as "${application_id}" cat files/frame-metrics.json > "${result_json_path}" 2>/dev/null \
  && [ -s "${result_json_path}" ]; then
  log "計測結果 JSON を回収しました: ${result_json_path}"
  cat "${result_json_path}"
else
  rm -f "${result_json_path}"
  log "警告: 計測結果 JSON を回収できませんでした（待機秒数不足・計測未完了の可能性）。"
  log "       FANDHE_ANDROID_WAIT_SECONDS を増やして再実行してください。"
fi

log "adb exec-out screencap -p > ${screenshot_path}"
adb_cmd exec-out screencap -p > "${screenshot_path}"
[ -s "${screenshot_path}" ] || fail "スクリーンショットの取得に失敗しました: ${screenshot_path}"
log "  - スクリーンショット取得 OK: ${screenshot_path}"

# egui の更新間隔（上記 frame-metrics.json）は presentation（実表示）間隔ではないため、
# フレーム落ち率の判定には使えない（§3.2 で判定保留と記録済み）。SurfaceFlinger の
# レイヤ latency から presentation 間隔を集計する `measure-frame-drop-rate.sh --parse`
# （#647 S3 / Issue #658）を併走させ、参考値として同じ evidence_dir へ保存する。
# 失敗しても本スクリプトの主目的（frame-metrics.json 回収）を阻害しないよう警告に留める。
# レイヤは計測窓開始前（--latency-clear 実行時）に確定済みの sf_layer を使い回す
# （ここで改めて select_surfaceflinger_layer を呼び直すと、クリアした対象と読み出す
# 対象のレイヤ選択がずれ得るため）。
sf_latency_path="${evidence_dir}/android-sf-latency-$(date +%Y%m%d-%H%M%S).txt"
frame_drop_json_path="${evidence_dir}/android-frame-drop-$(date +%Y%m%d-%H%M%S).json"
if [ -n "${sf_layer}" ]; then
  # `set -euo pipefail` 下では adb 側の失敗がそのままスクリプト中断につながる
  # ため、latency 取得自体も `if ... ; then` の条件内で実行する
  # （codex P1 レビュー指摘、Issue #658）。
  if adb_cmd shell dumpsys SurfaceFlinger --latency "${sf_layer}" 2>/dev/null | tr -d '\r' > "${sf_latency_path}" \
    && [ -s "${sf_latency_path}" ] \
    && "${script_dir}/measure-frame-drop-rate.sh" --parse "${sf_latency_path}" \
      --package "${application_id}" --layer "${sf_layer}" > "${frame_drop_json_path}" 2>/dev/null; then
    log "フレーム落ち率（presentation 間隔）を保存しました: ${frame_drop_json_path}"
  else
    rm -f "${sf_latency_path}" "${frame_drop_json_path}"
    log "警告: presentation 間隔からのフレーム落ち率集計に失敗しました（レイヤ: ${sf_layer}）"
  fi
fi

# --- フェーズ 8: 後片付け ------------------------------------------------------
log "adb uninstall ${application_id}"
adb_cmd uninstall "${application_id}" >/dev/null 2>&1 || true

log "完了。結果 ${result_json_path} とスクリーンショット ${screenshot_path} を控えてください。"
log "（リフレッシュレート設定・Gradle 参照用 jniLibs はいずれも EXIT トラップにより自動復元されます）"
