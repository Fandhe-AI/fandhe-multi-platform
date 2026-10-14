#!/usr/bin/env bash
# TASK-7.2（REQ-7 / M3、Issue #94）: 全対象プラットフォームの Rust ターゲットへ
# `rustup target add` のみ（`-Zbuild-std` 不要）でコンパイルできることを一括確認する。
#
# 「全対象」は docs/spec/04-requirements.md の段階戦略に基づき、v1 スコープ
# （第 1 段階: iOS/Android/Web、第 2 段階: watchOS/visionOS）の 11 ターゲットとする。
# 第 3 段階（Android TV・iPad・macOS・Windows・Linux）は REQ-19 で Won't（v1）判定済み
# のため対象外（検証記録は非公開）。
#
# 前提タスク（TASK-1.3: iOS、TASK-1.5: Android、TASK-3.1: watchOS/visionOS）は
# いずれもマージ済みで、各プラットフォームの検証スクリプトが既に存在する:
#   - scripts/ios/build-cross-targets.sh（iOS 3 ターゲット）
#   - scripts/apple/build-watchos-visionos-targets.sh（watchOS/visionOS 4 ターゲット）
# 本スクリプトはこの 2 つを呼び出し、加えて Android 3 ターゲット・Web 1 ターゲットの
# コンパイル確認を追加することで、11 ターゲット全体を一括実行できるようにする
# エントリポイントである。
#
# Android について: cargo-ndk 経由で `.so` を生成する scripts/android/build-android.sh
# とは目的が異なる。本スクリプトは NDK リンカを用いない rlib 生成のみの
# 「コンパイル確認」に限定し、REQ-7 受け入れ基準（`rustup target add` のみでの
# コンパイル成功）の確認に閉じる。対象ターゲット一覧は
# scripts/android/versions.env の RUST_ANDROID_TARGETS を SSOT として source する。
#
# Web について: wasm-bindgen CLI 依存を持ち込む scripts/web/build-wasm-proxy.sh
# （TASK-1.2）は呼ばず、`cargo build --target wasm32-unknown-unknown` によるコンパイル
# 確認のみに限定する。
#
# 使い方: scripts/build/verify-all-targets.sh
#   個別プラットフォームのターゲット・パッケージ上書きは各下位スクリプトの環境変数
#   （FANDHE_IOS_TARGETS / FANDHE_IOS_PACKAGES / FANDHE_APPLE_TARGETS /
#   FANDHE_APPLE_PACKAGES / FANDHE_APPLE_TOOLCHAIN）でも可能。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

# 結果サマリ（プラットフォーム名 のみを蓄積し、最後に一覧表示する。
# 失敗した時点で set -e により即座に終了するため、ここに積まれるのは成功分のみ）。
summary=()

echo "[verify-all-targets] === iOS（3 ターゲット） ==="
"${script_dir}/../ios/build-cross-targets.sh"
summary+=("iOS: aarch64-apple-ios / aarch64-apple-ios-sim / x86_64-apple-ios — OK")

echo "[verify-all-targets] === watchOS/visionOS（4 ターゲット） ==="
"${script_dir}/../apple/build-watchos-visionos-targets.sh"
summary+=("watchOS/visionOS: aarch64-apple-watchos(-sim) / aarch64-apple-visionos(-sim) — OK")

echo "[verify-all-targets] === Android（3 ターゲット） ==="
# RUST_ANDROID_TARGETS 等の SSOT を source する。versions.env は固定リテラルのみを
# 含む前提（ファイル冒頭のコメント参照）で、eval は行わない。
# scripts/android/setup-android-env.sh と同一の source 記法に揃える。
# shellcheck source=scripts/android/versions.env
set -a
# shellcheck disable=SC1091
. "${repo_root}/scripts/android/versions.env"
set +a
android_package="fandhe-core"
for target in ${RUST_ANDROID_TARGETS}; do
  echo "[verify-all-targets] rustup target add ${target}"
  rustup target add "${target}" >/dev/null

  echo "[verify-all-targets] cargo build -p ${android_package} --target ${target}"
  cargo build --manifest-path "${repo_root}/Cargo.toml" -p "${android_package}" --target "${target}"

  artifact="${repo_root}/target/${target}/debug/lib${android_package//-/_}.rlib"
  if [[ ! -f "${artifact}" ]]; then
    echo "[verify-all-targets] エラー: 期待したビルド成果物が見つからない: ${artifact}" >&2
    exit 1
  fi
  echo "[verify-all-targets] 成果物確認 OK: ${artifact}"
done
summary+=("Android: ${RUST_ANDROID_TARGETS} — OK")

echo "[verify-all-targets] === Web（1 ターゲット） ==="
web_package="fandhe-core"
web_target="wasm32-unknown-unknown"
echo "[verify-all-targets] rustup target add ${web_target}"
rustup target add "${web_target}" >/dev/null

echo "[verify-all-targets] cargo build -p ${web_package} --target ${web_target}"
cargo build --manifest-path "${repo_root}/Cargo.toml" -p "${web_package}" --target "${web_target}"

web_artifact="${repo_root}/target/${web_target}/debug/lib${web_package//-/_}.rlib"
if [[ ! -f "${web_artifact}" ]]; then
  echo "[verify-all-targets] エラー: 期待したビルド成果物が見つからない: ${web_artifact}" >&2
  exit 1
fi
echo "[verify-all-targets] 成果物確認 OK: ${web_artifact}"
summary+=("Web: ${web_target} — OK")

echo ""
echo "[verify-all-targets] === サマリ ==="
for line in "${summary[@]}"; do
  echo "  - ${line}"
done
echo "[verify-all-targets] 完了: 全対象プラットフォームのコンパイル確認に成功しました。"
