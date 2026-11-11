#!/usr/bin/env bash
# iOS 専用コード（cfg(target_os = "ios")）を aarch64-apple-ios の lib として型検査する定型手順。
# ubuntu の CI では iOS SDK が無く検査できないため手元で実行する（REQ-21 / TASK-21.1）。
# 条件と手順: docs/operations/ios-build-pipeline.md §8
#
# 検査対象は本スクリプトが置かれた worktree（カレントディレクトリではない）。
# 使い方: scripts/ios/check-lib.sh（引数なし。CARGO_TARGET_DIR は cargo がそのまま解釈する）
set -euo pipefail

if [[ $# -ne 0 ]]; then
  echo "usage: $(basename "$0") (no arguments)" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
target="aarch64-apple-ios"

# build script が xcrun で iOS SDK を探す。検査を黙って飛ばさず、前提不足は失敗にする。
if ! xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
  echo "[check-lib:ios] エラー: macOS と Xcode（iphoneos SDK）が必要" >&2
  exit 1
fi

# rust-toolchain.toml の版へ target を入れるため、先に移動する。
cd "${repo_root}"
rustup target add "${target}" >/dev/null

echo "[check-lib:ios] 検査対象: ${repo_root}"
# --all-features は計測 feature がコードを外すため使わない（既定の device を検査する）。
# fandhe-platform-apple は iOS 専用の `is_main_thread`（unsafe）を持つため、同じターゲットで検査する。
# 機能 crate（crates/fandhe-cap-*）はディレクトリから導出する（1 機能ごとの行追加は並列の移行作業で衝突する）。
cap_args=()
for cap_dir in "${repo_root}"/crates/fandhe-cap-*/; do
  [ -f "${cap_dir}Cargo.toml" ] || continue
  cap_args+=(-p "$(basename "${cap_dir}")")
done
if [ "${#cap_args[@]}" -eq 0 ]; then
  echo "[check-lib:ios] エラー: crates/fandhe-cap-* が見つからない" >&2
  exit 1
fi
cargo clippy --locked -p fandhe-gallery -p fandhe-egui-proxy -p fandhe-render-egui -p fandhe-platform-apple "${cap_args[@]}" --lib --target "${target}" -- -D warnings
