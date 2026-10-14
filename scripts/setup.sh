#!/usr/bin/env bash
# 開発環境の初期セットアップ（`make setup` の実体）。再実行しても安全。
#   1. docs/spec サブモジュールの取得（非公開のため任意。失敗しても警告して続行）
#   2. wasm32 ターゲットの追加
#   3. lefthook の導入（未導入ならインストール方法を案内するだけで失敗にしない）
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./lib.sh
source "${DIR}/lib.sh"

reject_args "$(basename "$0")" "$@"

cd "${ROOT_DIR}"

# docs/spec は非公開リポジトリ。取得できない環境（外部の利用者等）でも
# ビルド・テストには不要なため、警告だけ出して続行する。
# GIT_TERMINAL_PROMPT=0: 認証情報の対話入力で止まらないようにする。
if ! GIT_TERMINAL_PROMPT=0 git submodule update --init; then
  echo "警告: docs/spec サブモジュール（非公開の仕様書）を取得できませんでした。ビルド・テストには不要なため続行します。" >&2
fi
rustup target add wasm32-unknown-unknown

if command -v lefthook >/dev/null 2>&1; then
  lefthook install
else
  echo "lefthook が見つかりません。導入方法は .claude/skills/lefthook/references/installation/README.md を参照してください（例: brew install lefthook）。"
  echo "lefthook 導入後に 'make setup' を再実行するか、'lefthook install' を手動実行してください。"
fi
