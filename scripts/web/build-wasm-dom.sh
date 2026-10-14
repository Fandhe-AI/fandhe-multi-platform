#!/usr/bin/env bash
# TASK-4.4-S4（REQ-4 / Issue #82）: fandhe-web-dom の wasm32 ビルド → wasm-bindgen →
# 検証用配信ディレクトリ（web/dist）組み立てを行う冪等スクリプト。
#
# scripts/web/build-wasm-proxy.sh（TASK-1.2）と同じ作法（set -euo pipefail・冪等・
# --lib のみビルド）に揃える。demo.rs（wasm32 専用の検証用エントリポイント、
# #[wasm_bindgen] pub fn mount_sample_screen）が wasm-bindgen の処理対象になる。
#
# 使い方: scripts/web/build-wasm-dom.sh
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
crate_dir="${repo_root}/crates/fandhe-web-dom"
dist_dir="${crate_dir}/web/dist"

echo "[build-wasm-dom] cargo build --lib --target wasm32-unknown-unknown"
cargo build --manifest-path "${repo_root}/Cargo.toml" \
  -p fandhe-web-dom \
  --lib \
  --target wasm32-unknown-unknown

wasm_path="${repo_root}/target/wasm32-unknown-unknown/debug/fandhe_web_dom.wasm"
if [[ ! -f "${wasm_path}" ]]; then
  echo "[build-wasm-dom] エラー: wasm 成果物が見つからない: ${wasm_path}" >&2
  exit 1
fi

echo "[build-wasm-dom] wasm-bindgen --target web"
mkdir -p "${dist_dir}"
wasm-bindgen --target web --out-dir "${dist_dir}" --out-name fandhe_web_dom "${wasm_path}"

cp "${crate_dir}/web/index.html" "${dist_dir}/index.html"
# Issue #995 P1 対応: FANDHE_ITEMS_URL 未指定時の既定取得先（相対 URL "items.json"、
# http_fetch.rs の ITEMS_URL 参照）を配布物へ同梱する。無いと ?screen=http_fetch の
# 取得ボタンが同一オリジンへ 404 を返す。
cp "${crate_dir}/web/items.json" "${dist_dir}/items.json"
# Issue #1047: Service Worker はページと同じディレクトリ（scope）へ置く必要があるため
# dist_dir 直下へ cp する（background_task.rs::web::SW_URL の相対参照先）。
cp "${crate_dir}/web/background-task-sw.js" "${dist_dir}/background-task-sw.js"

echo "[build-wasm-dom] 完了: ${dist_dir}"
