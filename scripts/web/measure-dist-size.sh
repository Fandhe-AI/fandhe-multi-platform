#!/usr/bin/env bash
# TASK-4.5（REQ-4 / M4、Issue #235）: build-wasm-proxy.sh（FANDHE_WASM_RELEASE=1）が
# 組み立てた web/dist の brotli 圧縮後サイズを集計する。
#
# REQ-4 受け入れ基準の「配信サイズ 3MB 以内」は wasm 本体（fandhe_egui_proxy_bg.wasm +
# fandhe_egui_proxy.js + index.html）を対象とし、fonts/（TASK-10.1 の CJK フォント同梱）は
# 別枠として扱う（TASK-10.2 / Issue #245 のフォント同梱影響計測で本スクリプトを再利用する
# 前提のため、wasm 単体と fonts 込み合計を分離して出力する）。
#
# サイズは実行環境（CPU・回線）に依存しない確定値であり、TTI・フレームタイムのような
# 実機差のある計測値とは性質が異なる（検証記録は非公開）。
#
# brotli CLI を前提にせず Node 標準の node:zlib（brotliCompressSync）で圧縮するため、
# 追加の npm 依存もシステムパッケージも不要（security.md A06 準拠、依存追加なし）。
#
# 使い方: scripts/web/measure-dist-size.sh [dist ディレクトリ]
#   省略時は crates/fandhe-egui-proxy/web/dist を対象にする。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
dist_dir="${1:-${repo_root}/crates/fandhe-egui-proxy/web/dist}"

if [[ ! -d "${dist_dir}" ]]; then
  echo "[measure-dist-size] エラー: ${dist_dir} が無い。先に build-wasm-proxy.sh を実行すること。" >&2
  exit 1
fi

node "${script_dir}/measure-dist-size.mjs" "${dist_dir}"
