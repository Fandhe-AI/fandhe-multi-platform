#!/usr/bin/env bash
# TASK-1.2（REQ-1 / M2）: fandhe-egui-proxy の wasm32 ビルド → wasm-bindgen → 検証用
# 配信ディレクトリ（web/dist）組み立てを行う冪等スクリプト。
#
# scripts/android/setup-android-env.sh と同様の作法（set -euo pipefail・冪等）に揃える。
#
# TASK-10.1（REQ-10 / M3、Issue #218）: CJK フォント（Noto Sans JP）はシステムフォントへの
# 依存をやめ、`crates/fandhe-fonts` にリポジトリ同梱済みの資産（SHA-256 等は
# crates/fandhe-fonts/assets/README.md 参照）から dist/fonts/ へコピーする。
# native 分岐（`fandhe-egui-proxy` の `cfg(not(target_arch = "wasm32"))`）は
# `include_bytes!` で直接埋め込むが、wasm32 は配信サイズ（REQ-4 の 3MB 基準）と
# 衝突するため埋め込まず、本スクリプトによる `fonts/` への配置 + `web.rs` の
# `fetch` で自己完結させる（crates/fandhe-egui-proxy/src/web.rs のモジュールコメント参照）。
#
# 使い方: scripts/web/build-wasm-proxy.sh
#   環境変数 FANDHE_PROXY_CJK_FONT でコピー元フォントパスを上書きできる
#   （検証用途。未設定時は同梱資産を使う）。
#
# TASK-4.5（REQ-4 / M4、Issue #235）: PoC-5 の性能計測（配信サイズ/TTI/フレームタイム）は
# ヘッドレス Chrome + debug wasm による簡易値であり、REQ-4 受け入れ基準は実機・実 CDN
# 配信での再計測を要求する。再計測には release + wasm-opt 最適化を経た成果物が要る
# （debug 成果物は配信サイズが最適化後より大きく、基準（3MB 以内）との比較材料にならない）。
# `FANDHE_WASM_RELEASE=1` を指定すると release ビルド → wasm-bindgen → `wasm-opt -Os`
# の経路で dist/ を組み立てる。既定（未設定）では従来どおり debug 経路を維持し、
# 開発時のビルド速度を優先する（後方互換）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
crate_dir="${repo_root}/crates/fandhe-egui-proxy"
dist_dir="${crate_dir}/web/dist"
font_src="${FANDHE_PROXY_CJK_FONT:-${repo_root}/crates/fandhe-fonts/assets/NotoSansJP-Regular.otf}"
release_build="${FANDHE_WASM_RELEASE:-0}"

if [[ "${release_build}" == "1" ]]; then
  echo "[build-wasm-proxy] cargo build --release --lib --target wasm32-unknown-unknown"
  cargo build --manifest-path "${repo_root}/Cargo.toml" \
    -p fandhe-egui-proxy \
    --release \
    --lib \
    --target wasm32-unknown-unknown
  wasm_path="${repo_root}/target/wasm32-unknown-unknown/release/fandhe_egui_proxy.wasm"
else
  echo "[build-wasm-proxy] cargo build --lib --target wasm32-unknown-unknown"
  # `--lib` で cdylib（wasm-bindgen が処理する対象、crates/fandhe-egui-proxy/Cargo.toml の
  # [lib] 参照）のみをビルドする。native 用 bin（fandhe-egui-proxy-desktop）は wasm32 では
  # no-op であり、同時ビルドしても使わないため除外する。
  cargo build --manifest-path "${repo_root}/Cargo.toml" \
    -p fandhe-egui-proxy \
    --lib \
    --target wasm32-unknown-unknown
  wasm_path="${repo_root}/target/wasm32-unknown-unknown/debug/fandhe_egui_proxy.wasm"
fi

if [[ ! -f "${wasm_path}" ]]; then
  echo "[build-wasm-proxy] エラー: wasm 成果物が見つからない: ${wasm_path}" >&2
  exit 1
fi

echo "[build-wasm-proxy] wasm-bindgen --target web"
mkdir -p "${dist_dir}"
wasm-bindgen --target web --out-dir "${dist_dir}" --out-name fandhe_egui_proxy "${wasm_path}"

if [[ "${release_build}" == "1" ]]; then
  bindgen_wasm="${dist_dir}/fandhe_egui_proxy_bg.wasm"
  if [[ ! -f "${bindgen_wasm}" ]]; then
    echo "[build-wasm-proxy] エラー: wasm-bindgen 成果物が見つからない: ${bindgen_wasm}" >&2
    exit 1
  fi
  echo "[build-wasm-proxy] wasm-opt -Os（配信サイズ最適化、REQ-4 の 3MB 基準対象）"
  wasm-opt -Os "${bindgen_wasm}" -o "${bindgen_wasm}.opt"
  mv "${bindgen_wasm}.opt" "${bindgen_wasm}"
fi

cp "${crate_dir}/web/index.html" "${dist_dir}/index.html"

font_dist_name="noto-sans-jp-regular.otf"
if [[ -f "${font_src}" ]]; then
  mkdir -p "${dist_dir}/fonts"
  cp "${font_src}" "${dist_dir}/fonts/${font_dist_name}"
  echo "[build-wasm-proxy] CJK フォントを配置した: ${font_src} -> ${dist_dir}/fonts/${font_dist_name}"
else
  # TASK-10.1 で同梱資産が既定の取得元になったため、見つからないのは
  # 「同梱資産が壊れている／FANDHE_PROXY_CJK_FONT の指定ミス」であり、
  # 既定フォントへ黙って続行せず失敗させる（A08 整合性・気づかない tofu 化の防止）。
  echo "[build-wasm-proxy] エラー: CJK フォントが見つからない（${font_src}）" >&2
  exit 1
fi

echo "[build-wasm-proxy] 完了: ${dist_dir}"
