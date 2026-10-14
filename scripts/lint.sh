#!/usr/bin/env bash
# 静的検査（`make lint` の実体）: cargo fmt --check と cargo clippy（警告をエラー扱い）。
# CI の fmt / clippy ジョブ（.github/workflows/ci.yml）と同じコマンドを手元で再現する。
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./lib.sh
source "${DIR}/lib.sh"

reject_args "$(basename "$0")" "$@"

cd "${ROOT_DIR}"

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
