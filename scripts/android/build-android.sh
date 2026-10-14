#!/usr/bin/env bash
# fandhe-core を Android 向けにクロスビルドするラッパースクリプト。
#
# 対応: REQ-1 / TASK-1.5-S1（docs/spec/05-tasks.md）。
# 呼び出し元: .github/workflows/ci.yml の android-build ジョブ、およびローカル開発者が
# 直接実行する（cargo-ndk・rustup Android ターゲットは
# scripts/android/setup-android-env.sh で事前導入済みであることが前提）。
#
# 対象パッケージは当面 fandhe-core のみ（rlib のみでリンクを伴わない）。
# eframe/wgpu を用いる Android バックエンド初期化（cdylib 化・android-activity 統合）は
# TASK-1.5-S2 の範囲であり、対象パッケージが増えた際は本スクリプトの -p 指定を拡張する。
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd -P)"

# android-env.sh が versions.env の読み込みと ANDROID_NDK_HOME 等の export を行う
# （setup-android-env.sh が導入した JDK・SDK・NDK・Gradle への環境変数はここでのみ定義され、
# cargo-ndk は ANDROID_NDK_HOME からしか NDK を発見できないため必須の前提）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${SCRIPT_DIR}/android-env.sh"

RELEASE=0
if [ "${1:-}" = "--release" ]; then
  RELEASE=1
fi

log() {
  echo "[build-android] $*"
}

fail() {
  echo "[build-android] ERROR: $*" >&2
  exit 1
}

command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません（rustup 導入後に再実行してください）"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
# cargo-ndk は ANDROID_NDK_HOME（android-env.sh が export）からのみ NDK を発見できるため、
# ここで前提を明示的に検査し、cargo-ndk 側の分かりにくいエラーより早く原因を特定できるようにする。
[ -d "${ANDROID_NDK_HOME}" ] \
  || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"

# rust-toolchain.toml（rustc 1.88.0 固定）を確実に効かせるため、リポジトリルートで実行する
# （rustup target はカレントディレクトリの active toolchain に紐づくため、
# 呼び出し元のカレントディレクトリに依存させない）。
cd "${REPO_ROOT}"

# --- cargo-ndk 呼び出し ---
# -t: arm64-v8a / armeabi-v7a / x86_64 の 3 ABI（RUST_ANDROID_TARGETS と対応）。
# --platform: versions.env の ANDROID_MIN_API（cargo-ndk の --platform フラグ）。
declare -a cargo_ndk_args=(
  -t arm64-v8a
  -t armeabi-v7a
  -t x86_64
  --platform "${ANDROID_MIN_API}"
  build
  -p fandhe-core
)
if [ "${RELEASE}" -eq 1 ]; then
  cargo_ndk_args+=(--release)
fi

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

log "Android クロスビルド完了"
