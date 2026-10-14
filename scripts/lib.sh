#!/usr/bin/env bash
# scripts/ 直下の Make 入口用スクリプトが共有するヘルパー。直接実行せず source して使う:
#   DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
#   source "${DIR}/lib.sh"
#
# 呼び出し元は `set -euo pipefail` を引き継ぐため、失敗した場合の終了コードは
# そのまま呼び出し元（人・`make`・CI）へ伝播する。
# scripts/{android,web,build}/ 配下の既存スクリプトは自前の作法を持つため、本ファイルには依存しない。
set -euo pipefail

# 本ファイルの親ディレクトリ（= リポジトリルート）を cwd に依存せず解決する。
repo_root() {
  local lib_dir
  lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  (cd "${lib_dir}/.." && pwd)
}

ROOT_DIR="$(repo_root)"
# shellcheck disable=SC2034 # source した側のスクリプトで参照する
readonly ROOT_DIR

# reject_args <script-name> "$@": 引数を取らないスクリプトで未知の入力を
# 黙って無視せず、usage を出して終了コード 2（誤用）で止める。
reject_args() {
  local script_name="$1"
  shift
  if [ "$#" -ne 0 ]; then
    echo "usage: ${script_name} (no arguments)" >&2
    exit 2
  fi
}
