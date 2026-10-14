#!/usr/bin/env bash
# TASK-3.4-S3（REQ-3 / M3、Issue #73、親: TASK-3.4 / #70）:
# Xcode の `preBuildScripts`（`platforms/apple/project.yml` 参照）から呼ばれる薄いラッパ。
# `fandhe-swiftui-bridge` staticlib（XCFramework）を毎回のビルド前に最新化することで、
# `FandheSwiftUIBridge/Package.swift`（#72）が binaryTarget として取り込む成果物の鮮度を保つ。
#
# 呼び出し文脈:
# - 呼び出し元: Xcode（XcodeGen が project.yml から生成した `.xcodeproj` の
#   buildScripts フェーズ）。PR #352 の Cursor Bugbot 指摘対応（#73 追補）により、
#   アプリターゲット（FandheWatchApp / FandheVisionApp）自身の preBuildScripts ではなく、
#   `aggregateTargets:` 配下の専用ターゲット（FandheWatchBridgeBuild /
#   FandheVisionBridgeBuild）の buildScripts フェーズから実行される。アプリターゲットは
#   当該 aggregate ターゲットへターゲット依存を持つため、アプリターゲットが SwiftPM
#   パッケージグラフ（FandheBridgeBinary を含む）を解決する前に本スクリプトが完了する
#   ことが保証される（詳細は project.yml の aggregateTargets: 直前のコメント参照）。
# - 呼び出し先: `scripts/apple/build-bridge-xcframework.sh`（TASK-3.4-S2 / #72 実装）を
#   そのまま呼び出す。ビルドロジック自体は重複させず、本スクリプトは実行環境差分の
#   吸収のみを担う。
#
# 実行環境差分の吸収（Xcode ビルドフェーズ固有の事情）:
# - Xcode がビルドフェーズスクリプトを実行する際の PATH は最小構成であり、
#   通常のログインシェルと異なり `~/.cargo/bin`（rustup 経由の cargo/rustup 実体）が
#   含まれないことが多い。ここで明示的に前置し、`$HOME/.cargo/env` が存在すれば
#   source して rustup 由来の環境変数（RUSTUP_HOME 等）も反映する。
# - 二段構えの一段目（初回生成）は `scripts/apple/generate-xcodeproj.sh` の案内に従い
#   人間が事前に実行する。本スクリプトは二段目（2 回目以降のビルドでの鮮度維持）を担う
#   （役割分担の詳細は generate-xcodeproj.sh 冒頭コメント・検証レポート参照）。
#
# スコープ境界: 本スクリプトは cargo 呼び出しの入口を提供するのみで、クロスビルド対象・
# ツールチェーン選定・XCFramework 構造の組み立てロジックは持たない（build-bridge-xcframework.sh
# 側の一元管理を維持し、二重管理を避ける — code-comment-style.md「何を書かないか」準拠）。
#
# 実行環境: macOS 上の Xcode ビルドフェーズでのみ実行される想定（Linux 実行時も cargo 部分は
# 動作するため、動作確認は Linux 上でも `bash scripts/apple/xcode-prebuild-bridge.sh` として
# 実施できる）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Xcode のビルドフェーズは最小 PATH で起動するため、rustup 導入時の既定インストール先を
# 明示的に前置する。既に PATH に含まれる環境（通常のシェル実行・CI）では実害がない。
if [[ -f "${HOME}/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "${HOME}/.cargo/env"
fi
export PATH="${HOME}/.cargo/bin:${PATH}"

echo "[xcode-prebuild-bridge] build-bridge-xcframework.sh を呼び出して staticlib / XCFramework を最新化する"
exec "${script_dir}/build-bridge-xcframework.sh"
