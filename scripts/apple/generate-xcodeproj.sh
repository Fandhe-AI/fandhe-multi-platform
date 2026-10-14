#!/usr/bin/env bash
# REQ-3 / TASK-3.4-S1（Issue #71）: `platforms/apple/project.yml`（SSOT）から
# `platforms/apple/Fandhe.xcodeproj` を生成する。
#
# 本スクリプトは XcodeGen（https://github.com/yonaskolb/XcodeGen）のラッパーであり、
# `.xcodeproj` 本体をコミットしない方針（project.yml のコメント参照）を実行可能にする。
# scripts/apple/build-watchos-visionos-targets.sh（TASK-3.1）の流儀（冪等・
# set -euo pipefail・成果物存在確認・詳細ヘッダコメント）に倣う。
#
# 実行環境について: `.xcodeproj` 生成・検証には Xcode ツールチェーンが必要なため macOS 限定。
# 本開発環境（Linux, CI 含む）では実行できないため、Linux 実行時は手順を案内して
# 明示的に失敗する（ci.md「グローバル状態を汚す処理・野良インストールを書かない」準拠のため、
# xcodegen の自動インストールはしない。存在確認のみ行う）。
#
# 使い方: scripts/apple/generate-xcodeproj.sh
#   前提: `brew install xcodegen` 等で xcodegen 導入済みであること（自動インストールしない）。
#
# 【重要】初回実行前に XCFramework を先行生成すること（TASK-3.4-S3 / #73）:
#   `FandheSwiftUIBridge/Package.swift` の binaryTarget は SwiftPM マニフェスト評価
#   （依存解決）時点で `Artifacts/FandheBridge.xcframework` の存在を判定する。
#   project.yml の `aggregateTargets:` 配下の buildScripts（`xcode-prebuild-bridge.sh`
#   経由の cargo 再ビルド。PR #352 の Cursor Bugbot 指摘対応によりアプリターゲット直下の
#   preBuildScripts から専用の aggregate ターゲットへ切り出し済み。project.yml の
#   aggregateTargets: 直前のコメント参照）は「Xcode がビルドを開始した後」にしか
#   走らないため、依存解決自体が先に走る初回だけは間に合わない。二段構えの役割分担:
#     1. 初回: `bash scripts/apple/build-bridge-xcframework.sh` を先に実行してから
#        xcodegen generate（本スクリプト）/ Xcode を開く。
#     2. 2 回目以降: aggregate ターゲット経由の buildScripts が鮮度維持を担うため、
#        都度の先行生成は不要（アプリターゲットがパッケージグラフを解決する前に
#        ターゲット依存として完了することが保証されるため、常に最新の XCFramework が
#        リンクされる）。
#   本スクリプトは XCFramework 未生成（`Artifacts/FandheBridge.xcframework` 不在）を
#   検知した場合、生成を止めずに警告のみ表示する（依存解決自体は #72 の設計により
#   XCFramework 不在でも壊れないため）。
#
# 生成後の確認手順（macOS・人間担当）:
#   xcodegen --version
#   xcodebuild -list -project platforms/apple/Fandhe.xcodeproj
#     → FandheWatchApp / FandheVisionApp の 2 ターゲットが列挙されることを確認する。
#   xcodebuild build -project platforms/apple/Fandhe.xcodeproj -scheme FandheWatchApp \
#     -destination 'generic/platform=watchOS Simulator'
#     → #72（packages/binaryTarget）・#73（preBuildScripts/dependencies）の配線が
#     完成しているため、ビルド成功を確認できる（visionOS も同様）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
apple_dir="${repo_root}/platforms/apple"
spec_file="${apple_dir}/project.yml"
xcodeproj_path="${apple_dir}/Fandhe.xcodeproj"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "[generate-xcodeproj] エラー: 本スクリプトは macOS 上でのみ実行できます（Xcode ツールチェーン必須）。" >&2
  echo "[generate-xcodeproj] 手順: macOS 上で以下を実行してください。" >&2
  echo "[generate-xcodeproj]   1. brew install xcodegen" >&2
  echo "[generate-xcodeproj]   2. scripts/apple/generate-xcodeproj.sh" >&2
  echo "[generate-xcodeproj]   3. xcodebuild -list -project platforms/apple/Fandhe.xcodeproj" >&2
  exit 1
fi

if ! command -v xcodegen >/dev/null 2>&1; then
  echo "[generate-xcodeproj] エラー: xcodegen が見つかりません。" >&2
  echo "[generate-xcodeproj] 導入手順（自動インストールは行いません — ci.md 準拠）: brew install xcodegen" >&2
  exit 1
fi

echo "[generate-xcodeproj] xcodegen --version"
xcodegen --version

xcframework_info_plist="${apple_dir}/FandheSwiftUIBridge/Artifacts/FandheBridge.xcframework/Info.plist"
if [[ ! -f "${xcframework_info_plist}" ]]; then
  echo "[generate-xcodeproj] 警告: Artifacts/FandheBridge.xcframework が未生成です。" >&2
  echo "[generate-xcodeproj] 初回は 'bash scripts/apple/build-bridge-xcframework.sh' を先に実行してください（binaryTarget がパッケージグラフに現れません）。" >&2
fi

echo "[generate-xcodeproj] xcodegen generate --spec ${spec_file}"
xcodegen generate --spec "${spec_file}" --project "${apple_dir}"

if [[ ! -d "${xcodeproj_path}" ]]; then
  echo "[generate-xcodeproj] エラー: 期待した生成物が見つかりません: ${xcodeproj_path}" >&2
  exit 1
fi
echo "[generate-xcodeproj] 生成確認 OK: ${xcodeproj_path}"

echo "[generate-xcodeproj] 完了。次の手順で 2 ターゲットの列挙を確認してください:"
echo "[generate-xcodeproj]   xcodebuild -list -project ${xcodeproj_path}"
