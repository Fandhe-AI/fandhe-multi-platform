#!/usr/bin/env bash
# TASK-11.1-S1（REQ-11 / M3、Issue #220、親 #219）: ストア提出パイプライン
# 「単一コマンド・終了コードで成否判別」（REQ-11 受け入れ基準）を満たす CLI の
# 構成・引数設計を固定化する骨格実装。
#
# 実処理（Rust クロスビルド・.app/.ipa 生成・ad-hoc 署名）は
# scripts/ios/build-simulator-app.sh（TASK-11.1-S2/S3、#221・#222）へ委譲する
# （TASK-11.1、Issue #486 で配線）。本スクリプトの 4 フェーズ関数は
# build-simulator-app.sh をフェーズ引数（build/bundle/sign/ipa+verify）付きで
# 呼び出し、失敗を CLI の終了コード体系（4/5/6）へ対応付ける薄い委譲層である。
# macOS 実環境での動作確認・文書化は TASK-11.1-S4（#223）で扱う（人間担当）。
# 設計の詳細（引数表・終了コード体系・PoC-3 実証ステップとの対応）は
# docs/architecture/build-and-sign-cli.md を参照。
#
# REQ-11 は「Android・Windows へも同型で拡張する」と規定するため、プラットフォームを
# 第 1 位置引数のサブコマンドとする（TASK-11.2 で android を追加できる構造。
# 未対応値は usage エラーとして扱う）。
#
# CI 上での位置づけ: .github/workflows/ci.yml は Linux ランナーのみで
# macOS ランナーが存在しないため（ci.md）、本スクリプトは --dry-run により
# Linux 環境でも引数解析・前提検査の構造検証が完結する設計とする。
#
# 使い方: scripts/build/build-and-sign.sh <platform> [options]
#   詳細は本スクリプトを --help 付きで実行するか docs/architecture/build-and-sign-cli.md を参照。
set -euo pipefail

# CDPATH が設定された環境では、相対パスへの `cd` が意図しないディレクトリ
# （CDPATH 側の同名ディレクトリ）へ解決されうる（`cd` は相対パスかつ `./`/`../`
# で始まらない場合に CDPATH を探索するため、`>/dev/null` で解決先の出力を
# 抑止しても探索そのものは止まらない）。本スクリプトは CDPATH に一切依存
# しないため、起点で無効化する（Issue #488 Bugbot 指摘対応 / A05 対策）。
# `--output-dir` に相対パスが指定された場合、この値はそのまま
# `FANDHE_IOS_OUT_DIR` として scripts/ios/build-simulator-app.sh へ委譲される
# ため、呼び出し側・委譲先の双方で CDPATH を無効化しておく必要がある。
unset CDPATH

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

log() {
  echo "[build-and-sign] $*"
}

# --- 終了コード体系（REQ-11「終了コードで成否を判別できる」への対応） ---
# 0: 成功（dry-run 含む） / 2: usage エラー / 3: 環境前提エラー /
# 4: ビルド失敗（S2） / 5: 署名・署名検証失敗（S3） / 6: パッケージング失敗（S2）
readonly EXIT_OK=0
readonly EXIT_USAGE=2
readonly EXIT_ENV=3
readonly EXIT_BUILD=4
readonly EXIT_SIGN=5
readonly EXIT_PACKAGE=6

usage() {
  cat <<'EOF'
使い方: scripts/build/build-and-sign.sh <platform> [options]

platform:
  ios              iOS シミュレータ版 .app/.ipa 生成 + ad-hoc 署名
                   （android は TASK-11.2 で追加予定。未対応値は usage エラー）

options:
  --release          リリースビルド（既定でこの動作。TASK-11.1-S2（#221、
                     build-simulator-app.sh）の FANDHE_IOS_PROFILE 既定値
                     release に合わせている）
  --debug            デバッグビルド（既定は release のため、debug が必要な場合に指定）
  --output-dir <dir> 成果物出力先（既定: target/build-and-sign/<platform>/）
  --install          シミュレータへのインストール・起動確認を行う（既定: 行わない）
  --dry-run          実行せず、実行予定ステップとコマンド計画を表示して exit 0
                     （Linux CI・非 macOS 環境での構造検証用）
  -h, --help         このヘルプを表示して exit 0

環境変数（CLI オプション > 環境変数 > 既定値の優先順位）:
  FANDHE_IOS_SIM_TARGET   既定: aarch64-apple-ios-sim
                          （scripts/ios/build-simulator-app.sh と同名・同既定値。
                          対象パッケージは fandhe-egui-proxy 固定）
  FANDHE_IOS_PROFILE      既定: release（release または debug のみ対応）
  FANDHE_IOS_APP_BINARY   既定: 空（cargo build を実行）。非空の場合は build_app_bundle が
                          ビルドをスキップし、指定パスのバイナリをそのまま使う
                          （scripts/ios/build-simulator-app.sh の注入点をそのまま踏襲）
  FANDHE_BUILD_OUTPUT_DIR 既定: target/build-and-sign/<platform>/
  FANDHE_IOS_SKIP_SIGN    既定: 空（署名する）。値 1 のみ許容し、ad-hoc 署名を
                          明示 opt-in でスキップする（未署名・構造検証専用）。
                          非 macOS + FANDHE_IOS_APP_BINARY 指定時は自動で 1 になる。

非 macOS 環境での挙動:
  FANDHE_IOS_APP_BINARY 指定時のみ「構造検証モード」として続行し、実ビルド・
  実署名を明示スキップする（未署名・実配布不可）。未指定の場合は exit 3。

委譲先: scripts/ios/build-simulator-app.sh（フェーズ引数 build/bundle/sign/ipa/verify
で呼び出す。単体実行時は FANDHE_IOS_OUT_DIR で成果物出力先を指定できる）。

詳細は docs/architecture/build-and-sign-cli.md を参照。
EOF
}

# --- 引数解析 ---
# eval を使わず case によるホワイトリスト検証のみで判定する（A03 インジェクション対策）。
platform=""
release=0
debug=0
output_dir=""
do_install=0
dry_run=0

if [[ $# -eq 0 ]]; then
  echo "[build-and-sign] ERROR: platform を指定してください" >&2
  usage >&2
  exit "${EXIT_USAGE}"
fi

case "$1" in
  -h|--help)
    usage
    exit "${EXIT_OK}"
    ;;
  ios)
    platform="$1"
    shift
    ;;
  *)
    echo "[build-and-sign] ERROR: 未対応の platform です: $1（対応: ios）" >&2
    usage >&2
    exit "${EXIT_USAGE}"
    ;;
esac

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      release=1
      shift
      ;;
    --debug)
      debug=1
      shift
      ;;
    --output-dir)
      if [[ $# -lt 2 ]]; then
        echo "[build-and-sign] ERROR: --output-dir には引数が必要です" >&2
        exit "${EXIT_USAGE}"
      fi
      output_dir="$2"
      shift 2
      ;;
    --install)
      do_install=1
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit "${EXIT_OK}"
      ;;
    *)
      echo "[build-and-sign] ERROR: 未知のオプションです: $1" >&2
      usage >&2
      exit "${EXIT_USAGE}"
      ;;
  esac
done

# --- 既定値の解決（CLI オプション > 環境変数 > 既定値） ---
# 環境変数名・既定値は scripts/ios/build-simulator-app.sh（TASK-11.1-S2/S3、
# #221・#222）の呼び出し契約に合わせている（FANDHE_IOS_SIM_TARGET / FANDHE_IOS_PROFILE /
# FANDHE_IOS_APP_BINARY / FANDHE_IOS_OUT_DIR）。
: "${FANDHE_IOS_SIM_TARGET:=aarch64-apple-ios-sim}"
: "${FANDHE_IOS_PROFILE:=release}"
: "${FANDHE_IOS_APP_BINARY:=}"
: "${FANDHE_BUILD_OUTPUT_DIR:=}"
: "${FANDHE_IOS_SKIP_SIGN:=}"

if [[ -z "${output_dir}" ]]; then
  if [[ -n "${FANDHE_BUILD_OUTPUT_DIR}" ]]; then
    output_dir="${FANDHE_BUILD_OUTPUT_DIR}"
  else
    output_dir="${repo_root}/target/build-and-sign/${platform}"
  fi
fi

# build-simulator-app.sh（委譲先）は環境変数経由でのみ入力を受け取るため、
# CLI 側で解決した値をここで export する（Issue #486）。
# FANDHE_IOS_OUT_DIR: CLI の --output-dir（既定 target/build-and-sign/<platform>/）と
# build-simulator-app.sh 単体実行時の固定出力先（target/ios-sim/）の不整合を
# 解消するための注入点。
export FANDHE_IOS_SIM_TARGET
export FANDHE_IOS_APP_BINARY
export FANDHE_IOS_OUT_DIR="${output_dir}"

if [[ "${release}" -eq 1 && "${debug}" -eq 1 ]]; then
  echo "[build-and-sign] ERROR: --release と --debug は同時に指定できません" >&2
  exit "${EXIT_USAGE}"
fi

# build-simulator-app.sh（S2）は FANDHE_IOS_PROFILE の既定を release としているため、
# 本 CLI も既定を release に揃える（--debug 指定時のみ debug）。
build_mode="${FANDHE_IOS_PROFILE}"
if [[ "${debug}" -eq 1 ]]; then
  build_mode="debug"
elif [[ "${release}" -eq 1 ]]; then
  build_mode="release"
fi

case "${build_mode}" in
  release | debug) ;;
  *)
    echo "[build-and-sign] ERROR: FANDHE_IOS_PROFILE は release または debug のみ対応: ${build_mode}" >&2
    exit "${EXIT_USAGE}"
    ;;
esac

# CLI オプション > 環境変数の優先順位で解決した結果を FANDHE_IOS_PROFILE 自体へ反映する。
# こうしておかないと、build_mode（解決済みの値）と FANDHE_IOS_PROFILE（環境変数の生値）が
# 食い違って表示・伝播しうる（例: FANDHE_IOS_PROFILE=release の環境で --debug を指定した場合）。
# S2（scripts/ios/build-simulator-app.sh）へは本変数を経由して渡す契約のため、export する。
export FANDHE_IOS_PROFILE="${build_mode}"

# --- 前提検査 ---
# dry-run 時は非 macOS 環境（Linux CI 等）でも構造検証だけを完結させるため、
# 不足していても警告表示に留め exit 0 で継続する（実 macOS 実行時は不足即 exit 3）。
#
# 非 dry-run 時の分岐（Issue #486、設計 §3.3）:
#   - macOS: 従来どおり必須コマンド不足を exit 3 とする。ただし cargo/rustup は
#     FANDHE_IOS_APP_BINARY 未指定時のみ、codesign は署名実施時
#     （FANDHE_IOS_SKIP_SIGN != 1）のみ必須とする（過剰な前提要求をしない）。
#   - 非 macOS + FANDHE_IOS_APP_BINARY 指定あり: 「構造検証モード」として続行する。
#     Xcode/macOS 必須部分（実ビルド・実署名）をスキップする旨をログ出力し、
#     FANDHE_IOS_SKIP_SIGN=1 を export して委譲する（構造検証専用として明示する。
#     security.md A05 対策: 黙って署名を省略しない）。
#   - 非 macOS + FANDHE_IOS_APP_BINARY 未指定: 実ビルド不能のため exit 3
#     （黙って偽の成果物を作らない。安全側に倒す）。
check_environment() {
  local os_name
  os_name="$(uname -s)"

  # FANDHE_IOS_SKIP_SIGN はユーザーが明示指定した非 "1" の値を無検証で
  # 下流（build-simulator-app.sh）へ委譲しない（A03/A05 対策。下流側でも
  # 再検証されるが、CLI 側の前提検査でも早期に弾く）。
  case "${FANDHE_IOS_SKIP_SIGN}" in
    "" | "1") ;;
    *)
      echo "[build-and-sign] ERROR: FANDHE_IOS_SKIP_SIGN は空または 1 のみ対応: ${FANDHE_IOS_SKIP_SIGN}" >&2
      exit "${EXIT_ENV}"
      ;;
  esac

  if [[ "${dry_run}" -eq 1 ]]; then
    local missing=()
    local structure_mode=0
    # 非 macOS + FANDHE_IOS_APP_BINARY 指定時は、実行時には check_environment が
    # 「構造検証モード」として続行する（下記の非 dry-run 分岐と同じ判定）。
    # dry-run 側だけが無条件に macOS 必須を報告すると、実際の実行結果（continue）と
    # 矛盾するため、FANDHE_IOS_APP_BINARY 未指定の場合のみ不足として扱う
    # （Issue #488 Bugbot 指摘対応: dry-run が構造検証モードを無視していた）。
    if [[ "${platform}" == "ios" && "${os_name}" != "Darwin" ]]; then
      if [[ -z "${FANDHE_IOS_APP_BINARY}" ]]; then
        missing+=("非 macOS 環境（uname -s = ${os_name}）: iOS ビルドには macOS が必須（FANDHE_IOS_APP_BINARY 未指定のため構造検証モードにも入れない）")
      else
        structure_mode=1
      fi
    fi
    # 非 dry-run の check_environment（macOS 分岐・非 macOS 構造検証モード分岐）と
    # 同じ条件式で必須コマンドを絞り込む（Issue #488 Bugbot 指摘対応: dry-run が
    # 構造検証モードの緩和ルールを無視し、無関係なツール不足を警告していた）。
    #   - cargo/rustup: FANDHE_IOS_APP_BINARY 未指定時のみ必須（実 cargo build を行う場合）
    #   - codesign: 実際に署名する場合のみ必須。構造検証モードは自動で
    #     FANDHE_IOS_SKIP_SIGN=1 になる（下記で dry-run 側にも同じ値を反映する）ため対象外
    #   - python3: bundle/ipa/verify フェーズで常に使用するため常に必須
    if [[ -z "${FANDHE_IOS_APP_BINARY}" ]]; then
      for cmd in cargo rustup; do
        if ! command -v "${cmd}" >/dev/null 2>&1; then
          missing+=("コマンド未検出: ${cmd}")
        fi
      done
    fi
    if ! command -v python3 >/dev/null 2>&1; then
      missing+=("コマンド未検出: python3")
    fi
    if [[ "${structure_mode}" -eq 1 ]]; then
      # 実行時と同じく、構造検証モードでは署名を自動スキップする（黙って
      # 署名済み扱いにしないよう、以降のログ・実行計画にも同じ値を反映する）。
      export FANDHE_IOS_SKIP_SIGN=1
      log "警告: 非 macOS 環境（uname -s = ${os_name}）を検出しました。"
      log "      FANDHE_IOS_APP_BINARY が指定されているため、実行時は構造検証モード"
      log "      （cargo build 実ビルド・codesign 署名を明示スキップ）として続行します。"
    elif [[ "${FANDHE_IOS_SKIP_SIGN}" != "1" ]] && ! command -v codesign >/dev/null 2>&1; then
      missing+=("コマンド未検出: codesign")
    fi
    # "${missing[*]:-}" で存在確認する（bash 3.2 系 macOS 標準 bash では set -u 下で
    # 空配列に対する ${#missing[@]} が unbound variable エラーとなるため、
    # ${#arr[@]} ではなくパラメータ展開のデフォルト値付き参照で判定する）。
    if [[ -n "${missing[*]:-}" ]]; then
      log "警告: 環境前提を満たしていません（dry-run のため継続します）"
      for m in "${missing[@]}"; do
        log "  - ${m}"
      done
    else
      log "環境前提検査 OK"
    fi
    return 0
  fi

  if [[ "${os_name}" == "Darwin" ]]; then
    local missing=()
    if [[ -z "${FANDHE_IOS_APP_BINARY}" ]]; then
      for cmd in cargo rustup; do
        if ! command -v "${cmd}" >/dev/null 2>&1; then
          missing+=("コマンド未検出: ${cmd}")
        fi
      done
    fi
    if [[ "${FANDHE_IOS_SKIP_SIGN}" != "1" ]] && ! command -v codesign >/dev/null 2>&1; then
      missing+=("コマンド未検出: codesign")
    fi
    if ! command -v python3 >/dev/null 2>&1; then
      missing+=("コマンド未検出: python3")
    fi
    if [[ -n "${missing[*]:-}" ]]; then
      echo "[build-and-sign] ERROR: 環境前提を満たしていません:" >&2
      for m in "${missing[@]}"; do
        echo "  - ${m}" >&2
      done
      exit "${EXIT_ENV}"
    fi
    log "環境前提検査 OK（macOS）"
    return 0
  fi

  # 非 macOS: FANDHE_IOS_APP_BINARY 指定時のみ「構造検証モード」として続行する。
  if [[ -n "${FANDHE_IOS_APP_BINARY}" ]]; then
    # 構造検証モードでも bundle/ipa/verify フェーズ（build-simulator-app.sh 委譲先）は
    # python3 を必要とするため、ここで存在確認する。未検出のまま続行すると、後段の
    # 委譲フェーズ内で失敗し、環境前提エラー（exit 3）ではなく EXIT_BUILD(4)/EXIT_PACKAGE(6)
    # にマッピングされてしまう（Issue #488 Bugbot 指摘対応）。
    if ! command -v python3 >/dev/null 2>&1; then
      echo "[build-and-sign] ERROR: 構造検証モードでも python3 が必要です（bundle/ipa/verify フェーズで使用）が見つかりません。" >&2
      exit "${EXIT_ENV}"
    fi
    log "警告: 非 macOS 環境（uname -s = ${os_name}）を検出しました。"
    log "      FANDHE_IOS_APP_BINARY が指定されているため、Xcode/macOS 必須部分"
    log "      （cargo build 実ビルド・codesign 署名）を明示スキップし、"
    log "      構造検証モードで続行します（未署名・実配布不可）。"
    export FANDHE_IOS_SKIP_SIGN=1
    return 0
  fi

  echo "[build-and-sign] ERROR: 非 macOS 環境（uname -s = ${os_name}）では iOS の実ビルドができません。" >&2
  echo "[build-and-sign]        構造検証のみ行う場合は FANDHE_IOS_APP_BINARY にスタブバイナリのパスを" >&2
  echo "[build-and-sign]        指定して再実行してください（スタブ作成例: 中身が exit 0 のみの実行可能" >&2
  echo "[build-and-sign]        シェルスクリプトを用意し、FANDHE_IOS_APP_BINARY=<stub のパス> ${0} ${platform} を実行）。" >&2
  exit "${EXIT_ENV}"
}

# --- フェーズ関数（scripts/ios/build-simulator-app.sh への委譲、Issue #486） ---
# build-simulator-app.sh はフェーズ位置引数（build/bundle/sign/ipa/verify）を
# 受け取れるよう TASK-11.1-S2/S3 の既存 5 フェーズを関数化済み（本 Issue で対応）。
# CLI 側は各フェーズを個別に呼び出し、失敗を CLI の終了コード体系（4/5/6）へ
# マップする。環境変数の解決・export は既定値解決ブロックで完了済み。
delegate_phase() {
  local phase="$1"
  local exit_code="$2"
  local sim_script="${repo_root}/scripts/ios/build-simulator-app.sh"

  if [[ "${dry_run}" -eq 1 ]]; then
    log "[dry-run] ${phase}: 実行予定 -> bash ${sim_script} ${phase}"
    log "[dry-run]   環境変数: FANDHE_IOS_SIM_TARGET=${FANDHE_IOS_SIM_TARGET} FANDHE_IOS_PROFILE=${FANDHE_IOS_PROFILE} FANDHE_IOS_APP_BINARY=${FANDHE_IOS_APP_BINARY:-（未指定）} FANDHE_IOS_OUT_DIR=${FANDHE_IOS_OUT_DIR} FANDHE_IOS_SKIP_SIGN=${FANDHE_IOS_SKIP_SIGN:-（未指定）}"
    if [[ "$(uname -s)" != "Darwin" && -z "${FANDHE_IOS_APP_BINARY}" ]]; then
      log "[dry-run]   注意: 非 macOS + FANDHE_IOS_APP_BINARY 未指定のため、実行時は check_environment で exit ${EXIT_ENV} になります"
    fi
    return 0
  fi

  if ! bash "${sim_script}" "${phase}"; then
    echo "[build-and-sign] ERROR: ${phase} フェーズが失敗しました（scripts/ios/build-simulator-app.sh ${phase}）" >&2
    exit "${exit_code}"
  fi
}

cross_build_rust() {
  log "cross_build_rust: sim_target=${FANDHE_IOS_SIM_TARGET} package=fandhe-egui-proxy profile=${build_mode}"
  delegate_phase "build" "${EXIT_BUILD}"
}

build_app_bundle() {
  log "build_app_bundle: output_dir=${output_dir} app_binary=${FANDHE_IOS_APP_BINARY:-（cargo build 成果物を使用）}"
  delegate_phase "bundle" "${EXIT_BUILD}"
}

sign_adhoc() {
  log "sign_adhoc: ad-hoc 署名（codesign --sign -）+ codesign --verify（FANDHE_IOS_SKIP_SIGN=${FANDHE_IOS_SKIP_SIGN:-（未指定）}）"
  delegate_phase "sign" "${EXIT_SIGN}"
}

package_ipa() {
  log "package_ipa: Payload/ 構造で .ipa 生成 + 構造検証"
  delegate_phase "ipa" "${EXIT_PACKAGE}"
  delegate_phase "verify" "${EXIT_PACKAGE}"
}

# --- フェーズ関数スタブ（install_simulator のみ残置） ---
# simctl install/launch は macOS 実機・シミュレータランタイムが前提のため、
# TASK-11.1-S4（#223）・人間担当の実機検証タスクへ引き継ぐ（out-of-scope-tracking.md 対応）。
not_implemented() {
  local phase="$1"
  local issue="$2"
  local exit_code="$3"
  if [[ "${dry_run}" -eq 1 ]]; then
    log "[dry-run] ${phase}: 未実装（${issue} で実装予定）。実行時は exit ${exit_code} で失敗します"
    return 0
  fi
  echo "[build-and-sign] ERROR: ${phase} は未実装です（${issue} で実装予定）" >&2
  exit "${exit_code}"
}

install_simulator() {
  if [[ "${do_install}" -eq 0 ]]; then
    log "install_simulator: --install 未指定のためスキップ"
    return 0
  fi
  log "install_simulator: simctl install / launch（検証用、既定 skip）"
  not_implemented "install_simulator" "TASK-11.1-S4（#223）" "${EXIT_BUILD}"
}

# --- 実行計画表示（dry-run 共通） ---
show_plan() {
  log "実行計画:"
  log "  platform      : ${platform}"
  log "  build_mode    : ${build_mode}"
  log "  output_dir    : ${output_dir}"
  log "  install       : ${do_install}"
  log "  ios_sim_target: ${FANDHE_IOS_SIM_TARGET}"
  log "  ios_profile   : ${FANDHE_IOS_PROFILE}"
  log "  フェーズ順    : check_environment -> cross_build_rust -> build_app_bundle -> sign_adhoc -> package_ipa -> install_simulator"
  log "  委譲先        : scripts/ios/build-simulator-app.sh <phase>（build/bundle/sign/ipa/verify）"
}

main() {
  if [[ "${dry_run}" -eq 1 ]]; then
    log "dry-run モード: 実行予定ステップとコマンド計画のみ表示します"
  fi

  show_plan
  check_environment
  cross_build_rust
  build_app_bundle
  sign_adhoc
  package_ipa
  install_simulator

  # 非 macOS + FANDHE_IOS_APP_BINARY 指定時（check_environment が構造検証モードへ
  # 切り替えた場合）は、成果物が未署名・実配布不可であることを完了サマリでも
  # 明示する（security.md A05 対策: 黙って署名省略を成功扱いにしない）。
  if [[ "${dry_run}" -eq 0 && "$(uname -s)" != "Darwin" ]]; then
    log "注意: 本実行は非 macOS 構造検証モードのため、成果物は未署名・実配布不可です。"
  fi

  log "完了（dry-run=${dry_run}）"
}

main
