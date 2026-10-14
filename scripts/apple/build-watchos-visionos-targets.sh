#!/usr/bin/env bash
# TASK-3.1（REQ-3 / M3、Issue #56）: watchOS/visionOS 向けビルド成果物の生成確認。
#
# PoC-2（docs/spec/03-poc/watchos-visionos/README.md）で、使い捨ての最小ライブラリ
# `mpf-minimal` が aarch64-apple-watchos / aarch64-apple-watchos-sim /
# aarch64-apple-visionos / aarch64-apple-visionos-sim の 4 ターゲットへ
# `rustup target add` のみ（`-Zbuild-std` 不要）でコンパイル成功することを確認済み。
# 本スクリプトはこれを本実装コードベース（fandhe-core、プラットフォーム非依存の
# 共通コア、`#![forbid(unsafe_code)]`）で再現する検証手順を恒久化したものであり、
# scripts/ios/build-cross-targets.sh（TASK-1.3-S3）の流儀（冪等・set -euo pipefail・
# 成果物存在確認）に厳密に倣う。
#
# ツールチェーンについて: ルートの rust-toolchain.toml は CI ランナーとの
# バージョン一致のため 1.88.0 に固定されている（PR #306）が、1.88.0 には
# watchOS/visionOS ターゲットが rustup 配布されていない（実機確認済み）。
# 1.96.0 では 4 ターゲットすべてが rustup 配布されており（PoC-2 も 1.96.0 で実施）、
# `cargo +<toolchain>` の `+toolchain` 指定は rust-toolchain.toml の固定より優先される
# 仕様のため、本スクリプト内でのみ 1.96.0 を局所使用し、ワークスペース全体の
# ツールチェーン固定は変更しない。
#
# スコープ境界: TASK-3.2-S2（Issue #59）で `fandhe-swiftui-bridge`（extern "C" エクス
# ポート層・staticlib 化）が新設されたため、本スクリプトのビルド対象へ追加した。
# Swift 側ラッパ API（S5・#62、`platforms/apple/` 配下）等、Rust クレート境界の外側
# にあるものは引き続き本スクリプトの対象外。ビルド対象を fandhe-core /
# fandhe-swiftui-bridge に限定するのは、fandhe-egui-proxy が wgpu 依存で PoC-2 の
# 知見どおり watchOS/visionOS 未対応のため。
#
# 使い方: scripts/apple/build-watchos-visionos-targets.sh
#   環境変数 FANDHE_APPLE_TARGETS で対象ターゲットを空白区切りで上書きできる
#   （既定: aarch64-apple-watchos aarch64-apple-watchos-sim
#          aarch64-apple-visionos aarch64-apple-visionos-sim）。
#   環境変数 FANDHE_APPLE_TOOLCHAIN で使用ツールチェーンを上書きできる（既定: 1.96.0）。
#   環境変数 FANDHE_APPLE_PACKAGES で対象パッケージを空白区切りで上書きできる
#   （既定: fandhe-core fandhe-swiftui-bridge）。
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

# 現時点で watchOS/visionOS ターゲット向けにビルド可能なのは、プラットフォーム依存
# コードを持たない共通コア（fandhe-core）と、それのみに依存する FFI ブリッジ層
# （fandhe-swiftui-bridge、TASK-3.2-S2）のみ（fandhe-egui-proxy は wgpu 依存で対象外）。
packages="${FANDHE_APPLE_PACKAGES:-fandhe-core fandhe-swiftui-bridge}"
targets="${FANDHE_APPLE_TARGETS:-aarch64-apple-watchos aarch64-apple-watchos-sim aarch64-apple-visionos aarch64-apple-visionos-sim}"
toolchain="${FANDHE_APPLE_TOOLCHAIN:-1.96.0}"

echo "[build-watchos-visionos-targets] rustup toolchain install ${toolchain}（導入済みなら no-op）"
rustup toolchain install "${toolchain}" >/dev/null

for target in ${targets}; do
  echo "[build-watchos-visionos-targets] rustup target add --toolchain ${toolchain} ${target}"
  rustup target add --toolchain "${toolchain}" "${target}" >/dev/null

  for package in ${packages}; do
    echo "[build-watchos-visionos-targets] cargo +${toolchain} build -p ${package} --target ${target}"
    cargo "+${toolchain}" build --manifest-path "${repo_root}/Cargo.toml" -p "${package}" --target "${target}"

    # fandhe-swiftui-bridge は crate-type = ["staticlib", "lib"] のため、Swift 側が
    # リンクする成果物は `.a`（staticlib）側で確認する（他パッケージは従来どおり `.rlib`）。
    if [[ "${package}" == "fandhe-swiftui-bridge" ]]; then
      artifact="${repo_root}/target/${target}/debug/lib${package//-/_}.a"
    else
      artifact="${repo_root}/target/${target}/debug/lib${package//-/_}.rlib"
    fi
    if [[ ! -f "${artifact}" ]]; then
      echo "[build-watchos-visionos-targets] エラー: 期待したビルド成果物が見つからない: ${artifact}" >&2
      exit 1
    fi
    echo "[build-watchos-visionos-targets] 成果物確認 OK: ${artifact}"
  done
done

echo "[build-watchos-visionos-targets] 完了: packages=${packages} targets=${targets}"
