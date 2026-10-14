#!/usr/bin/env bash
# TASK-1.3-S3（REQ-1 / M3、Issue #42）: iOS 向けビルド成果物の生成確認。
#
# 前提タスク（TASK-1.3-S1: Cargo.toml ターゲット条件分岐、TASK-1.3-S2: iOS 向け
# wgpu（Metal）バックエンド初期化）は本コミット時点で未実装（Issue #40 / #41 は open）。
# そのため本スクリプトは Metal バックエンドの成果物ではなく、共通コア
# （fandhe-core、`#![forbid(unsafe_code)]`・プラットフォーム非依存）が iOS 3 ターゲット
# （実機 arm64 / シミュレータ arm64 / シミュレータ x86_64）向けに Rust ライブラリ成果物
# （.rlib）を生成できることを、Linux 環境から cross-compile することで確認するプロキシ
# 検証である。TASK-1.3-S1/S2 完了後は本スクリプトのビルド対象に該当クレートを追加し、
# 実機・Xcode 環境での最終確認（TASK-1.4、Issue #229、人間担当）へ引き継ぐ。
#
# TASK-7.2（REQ-7 / M3、Issue #94）: scripts/apple/build-watchos-visionos-targets.sh
# の先例（TASK-3.2-S2 で新設された FFI ブリッジ層 `fandhe-swiftui-bridge` を
# ビルド対象へ追加した際に導入した `FANDHE_APPLE_PACKAGES` 環境変数化）に倣い、
# 本スクリプトも対象パッケージを環境変数化し、`fandhe-swiftui-bridge` を
# 既定対象へ追加した（Apple 系ターゲットすべてで同ブリッジ層のコンパイルを
# 確認する、TASK-3.1 レポート §7 の引き継ぎ指示に対応）。
#
# scripts/android/setup-android-env.sh・scripts/web/build-wasm-proxy.sh と同様に
# 冪等（何度実行しても同じ結果）・set -euo pipefail に揃える。
#
# 使い方: scripts/ios/build-cross-targets.sh
#   環境変数 FANDHE_IOS_TARGETS で対象ターゲットを空白区切りで上書きできる
#   （既定: aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios）。
#   環境変数 FANDHE_IOS_PACKAGES で対象パッケージを空白区切りで上書きできる
#   （既定: fandhe-core fandhe-swiftui-bridge）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

# 現時点で iOS ターゲット向けにビルド可能なのは、プラットフォーム依存コードを
# 持たない共通コア（fandhe-core）と、それのみに依存する FFI ブリッジ層
# （fandhe-swiftui-bridge、TASK-3.2-S2）のみ（fandhe-egui-proxy は wgpu 依存で
# iOS 未対応の検証専用プロキシのため対象外）。
packages="${FANDHE_IOS_PACKAGES:-fandhe-core fandhe-swiftui-bridge}"
targets="${FANDHE_IOS_TARGETS:-aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios}"

for target in ${targets}; do
  echo "[build-cross-targets] rustup target add ${target}"
  rustup target add "${target}" >/dev/null

  for package in ${packages}; do
    echo "[build-cross-targets] cargo build -p ${package} --target ${target}"
    cargo build --manifest-path "${repo_root}/Cargo.toml" -p "${package}" --target "${target}"

    # fandhe-swiftui-bridge は crate-type = ["staticlib", "lib"] のため、Swift 側が
    # リンクする成果物は `.a`（staticlib）側で確認する（他パッケージは従来どおり `.rlib`）。
    if [[ "${package}" == "fandhe-swiftui-bridge" ]]; then
      artifact="${repo_root}/target/${target}/debug/lib${package//-/_}.a"
    else
      artifact="${repo_root}/target/${target}/debug/lib${package//-/_}.rlib"
    fi
    if [[ ! -f "${artifact}" ]]; then
      echo "[build-cross-targets] エラー: 期待したビルド成果物が見つからない: ${artifact}" >&2
      exit 1
    fi
    echo "[build-cross-targets] 成果物確認 OK: ${artifact}"
  done
done

echo "[build-cross-targets] 完了: packages=${packages} targets=${targets}"
