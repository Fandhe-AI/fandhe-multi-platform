#!/usr/bin/env bash
# TASK-3.4-S2（REQ-3 / M3、Issue #72、親: TASK-3.4 / #70）:
# `fandhe-swiftui-bridge` staticlib を watchOS/visionOS 4 ターゲット向けにクロスビルドし、
# `platforms/apple/FandheSwiftUIBridge/Package.swift` から `binaryTarget` として取り込める
# XCFramework（`FandheBridge.xcframework`）を組み立てる。
#
# 呼び出し文脈: 本スクリプトの出力（Artifacts/FandheBridge.xcframework）は
# `Package.swift` が `FileManager.fileExists` で存在確認したうえで条件付きに
# binaryTarget として配線する（本 issue 側の変更）。生成物は `.gitignore` により
# コミットしない方針のため、SPM 依存解決を試す前に必ず本スクリプトを実行すること。
#
# 設計判断（検証記録は非公開）:
# `xcodebuild -create-xcframework` は macOS 専用ツールであり、本開発環境
# （Swift ツールチェーン・Xcode 不在の Linux CI）では実行できない。
# XCFramework は「ディレクトリ + Info.plist（XML）」の規約構造であるため、
# 本スクリプトは `xcodebuild` を使わず Linux 上で構造を直接組み立てる。
#
# クロスビルド対象・ツールチェーンについて: `scripts/apple/build-watchos-visionos-targets.sh`
# （TASK-3.1 / #56）の流儀を厳密に踏襲する。4 ターゲット
# （aarch64-apple-watchos{,-sim} / aarch64-apple-visionos{,-sim}）は Rust 1.96.0 のみ
# rustup 配布されており、ワークスペースの `rust-toolchain.toml`（CI ランナー
# との一致のため 1.88.0 固定）は変更せず、本スクリプト内でのみ `cargo +1.96.0` を
# 局所使用する。
#
# スコープ境界: Xcode ビルドフェーズへの cargo 統合・実機/シミュレータでの実リンク確認は
# S3（#73）のスコープ。extern "C" シンボル実体（`ffi` モジュール、#59）は本更新時点で
# 未マージのため、本スクリプトが生成する staticlib はシンボル未定義のまま
# （コンパイル・アーカイブ自体はシンボル有無と無関係に成功する）。
#
# 使い方: scripts/apple/build-bridge-xcframework.sh
#   環境変数 FANDHE_APPLE_TARGETS で対象ターゲットを空白区切りで上書きできる
#   （既定: aarch64-apple-watchos aarch64-apple-watchos-sim
#          aarch64-apple-visionos aarch64-apple-visionos-sim）。
#   環境変数 FANDHE_APPLE_TOOLCHAIN で使用ツールチェーンを上書きできる（既定: 1.96.0）。
#   冪等: 再実行時は Artifacts/FandheBridge.xcframework を作り直す。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

package="fandhe-swiftui-bridge"
lib_name="libfandhe_swiftui_bridge.a"
targets="${FANDHE_APPLE_TARGETS:-aarch64-apple-watchos aarch64-apple-watchos-sim aarch64-apple-visionos aarch64-apple-visionos-sim}"
toolchain="${FANDHE_APPLE_TOOLCHAIN:-1.96.0}"

bridge_dir="${repo_root}/platforms/apple/FandheSwiftUIBridge"
header_src="${bridge_dir}/Sources/CFandheBridge/include/fandhe_bridge.h"
artifacts_dir="${bridge_dir}/Artifacts"
xcframework_dir="${artifacts_dir}/FandheBridge.xcframework"
# ビルド途中の不完全なディレクトリを `Package.swift` が `FileManager.fileExists`
# で「生成済み」と誤認しないよう、一時ディレクトリで組み立てたうえで全スライス・
# Info.plist が揃ってから最終パスへ原子的に mv する（Bugbot 指摘 #336 対応）。
# xcframework_dir は常に「完成状態のみ」（旧成果物 or 新成果物のどちらか）を保ち、
# 不完全な中間状態が観測されることはない（旧成果物は新成果物への mv 成功後にのみ
# 破棄し、mv 失敗時は旧成果物を復元する）。加えて、退避直後・昇格前に本スクリプトが
# kill -9 等で異常終了し xcframework_dir が失われたまま `.old` のみ残存した場合も、
# 次回実行の冒頭で `.old` から復元してから処理を続けるため、旧成果物が失われることはない。
staging_dir="${artifacts_dir}/.FandheBridge.xcframework.building"

echo "[build-bridge-xcframework] rustup toolchain install ${toolchain}（導入済みなら no-op）"
rustup toolchain install "${toolchain}" >/dev/null

# 連想配列（bash 4 以降が必要）は使わない。実リンク確認（S3 / #73）が macOS 実機環境
# （システム標準の `/bin/bash` は 3.2 系で連想配列非対応）で本スクリプトを再利用する
# 前提のため、`case` によるルックアップで bash 3.2 互換を保つ。
slice_for_target() {
  case "$1" in
    aarch64-apple-watchos) echo "watchos-arm64" ;;
    aarch64-apple-watchos-sim) echo "watchos-arm64-simulator" ;;
    aarch64-apple-visionos) echo "xros-arm64" ;;
    aarch64-apple-visionos-sim) echo "xros-arm64-simulator" ;;
    *) echo "" ;;
  esac
}
platform_for_target() {
  case "$1" in
    aarch64-apple-watchos|aarch64-apple-watchos-sim) echo "watchos" ;;
    aarch64-apple-visionos|aarch64-apple-visionos-sim) echo "xros" ;;
    *) echo "" ;;
  esac
}
variant_for_target() {
  case "$1" in
    aarch64-apple-watchos-sim|aarch64-apple-visionos-sim) echo "simulator" ;;
    *) echo "" ;;
  esac
}

# 再実行時の冪等性を保つため、既存の Artifacts・一時ディレクトリを作り直す。
# 完成後にのみ最終パス（xcframework_dir）へ rename するため、ここでは
# xcframework_dir 自体には触れない（既存の完成品は mv 直前まで残る）。
rm -rf "${staging_dir}"
mkdir -p "${staging_dir}"
trap 'rm -rf "${staging_dir}"' EXIT

libraries_plist_entries=()

for target in ${targets}; do
  slice="$(slice_for_target "${target}")"
  if [[ -z "${slice}" ]]; then
    echo "[build-bridge-xcframework] エラー: 未知のターゲット: ${target}" >&2
    exit 1
  fi

  echo "[build-bridge-xcframework] rustup target add --toolchain ${toolchain} ${target}"
  rustup target add --toolchain "${toolchain}" "${target}" >/dev/null

  echo "[build-bridge-xcframework] cargo +${toolchain} build -p ${package} --release --target ${target}"
  cargo "+${toolchain}" build --manifest-path "${repo_root}/Cargo.toml" -p "${package}" --release --target "${target}"

  built_lib="${repo_root}/target/${target}/release/${lib_name}"
  if [[ ! -f "${built_lib}" ]]; then
    echo "[build-bridge-xcframework] エラー: 期待したビルド成果物が見つからない: ${built_lib}" >&2
    exit 1
  fi

  slice_dir="${staging_dir}/${slice}"
  mkdir -p "${slice_dir}/Headers"
  cp "${built_lib}" "${slice_dir}/${lib_name}"
  # ヘッダの SSOT は `Sources/CFandheBridge/include/fandhe_bridge.h`。
  # 各スライスへはビルド成果物として複製するのみで、編集対象は常にソース側とする。
  cp "${header_src}" "${slice_dir}/Headers/fandhe_bridge.h"

  variant="$(variant_for_target "${target}")"
  platform="$(platform_for_target "${target}")"
  if [[ -n "${variant}" ]]; then
    libraries_plist_entries+=("      <dict>
        <key>LibraryIdentifier</key>
        <string>${slice}</string>
        <key>LibraryPath</key>
        <string>${lib_name}</string>
        <key>HeadersPath</key>
        <string>Headers</string>
        <key>SupportedArchitectures</key>
        <array>
          <string>arm64</string>
        </array>
        <key>SupportedPlatform</key>
        <string>${platform}</string>
        <key>SupportedPlatformVariant</key>
        <string>${variant}</string>
      </dict>")
  else
    libraries_plist_entries+=("      <dict>
        <key>LibraryIdentifier</key>
        <string>${slice}</string>
        <key>LibraryPath</key>
        <string>${lib_name}</string>
        <key>HeadersPath</key>
        <string>Headers</string>
        <key>SupportedArchitectures</key>
        <array>
          <string>arm64</string>
        </array>
        <key>SupportedPlatform</key>
        <string>${platform}</string>
      </dict>")
  fi

  echo "[build-bridge-xcframework] スライス組み立て OK: ${slice_dir}"
done

libraries_plist="$(IFS=$'\n'; echo "${libraries_plist_entries[*]}")"

cat > "${staging_dir}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundlePackageType</key>
  <string>XFWK</string>
  <key>XCFrameworkFormatVersion</key>
  <string>1.0</string>
  <key>AvailableLibraries</key>
  <array>
${libraries_plist}
  </array>
</dict>
</plist>
EOF

# 全スライス・Info.plist が揃った完成状態でのみ最終パスへ原子的に配置する。
# 同一ファイルシステム内の `mv` はアトミックであり、`Package.swift` 側の
# `FileManager.fileExists` が不完全な中間状態を観測することはない。
# 旧成果物は mv 成功後にのみ削除する（Bugbot 指摘対応: 先に `rm -rf` してから
# `mv` すると、mv 失敗時に EXIT トラップがステージングツリーごと削除してしまい、
# 完全な再ビルドが成功するまで XCFramework が存在しない状態になっていた）。
xcframework_backup_dir="${xcframework_dir}.old"
# 前回実行が「旧成果物を .old へ退避した直後・新成果物への昇格前」に
# 異常終了（kill -9・OOM 等、EXIT トラップの及ばない終了）した場合、
# 正規パス（xcframework_dir）は存在せず .old にのみ直前の正常ビルド成果物が
# 残っている状態になりうる（Bugbot 指摘 #336 対応）。この状態を検知せず
# 無条件に `rm -rf` すると、直前の正常ビルド成果物を今回のビルドが
# 成功するかどうかに関わらず失ってしまうため、まず .old から復元してから
# 通常フロー（今回のビルド結果を退避・昇格）へ進める。
if [[ ! -e "${xcframework_dir}" && ! -L "${xcframework_dir}" ]] \
  && [[ -e "${xcframework_backup_dir}" || -L "${xcframework_backup_dir}" ]]; then
  echo "[build-bridge-xcframework] 警告: 前回実行の異常終了を検知。旧成果物（.old）から復元する: ${xcframework_dir}" >&2
  mv "${xcframework_backup_dir}" "${xcframework_dir}"
fi
rm -rf "${xcframework_backup_dir}"
# 通常はディレクトリだが、想定外にファイル等が残っていても待避対象にする
# （`-e`/`-L` で存在確認し、旧 `rm -rf` 相当の自己修復性を保つ）。
if [[ -e "${xcframework_dir}" || -L "${xcframework_dir}" ]]; then
  mv "${xcframework_dir}" "${xcframework_backup_dir}"
fi
if ! mv "${staging_dir}" "${xcframework_dir}"; then
  echo "[build-bridge-xcframework] エラー: 最終パスへの mv に失敗。旧成果物を復元する: ${xcframework_dir}" >&2
  # mv 失敗時に中途半端な残骸が xcframework_dir に残っている可能性があるため、
  # 復元の有無に関わらず必ず削除してから待避先を書き戻す。
  rm -rf "${xcframework_dir}"
  if [[ -e "${xcframework_backup_dir}" || -L "${xcframework_backup_dir}" ]]; then
    mv "${xcframework_backup_dir}" "${xcframework_dir}"
  fi
  exit 1
fi
rm -rf "${xcframework_backup_dir}"
trap - EXIT

echo "[build-bridge-xcframework] 完了: ${xcframework_dir}"
