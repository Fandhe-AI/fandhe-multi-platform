#!/usr/bin/env bash
# TASK-1.5-S3（REQ-1 / M3、Issue #46）: Android 向けクロスビルド成果物の生成確認。
#
# 前提タスク（TASK-1.5-S1: cargo-ndk ビルド設定・ターゲット追加 Issue #44、
# TASK-1.5-S2: Android Vulkan バックエンド初期化 Issue #45）は本コミット時点で
# いずれも未マージ（open）。そのため #44 が追加する scripts/android/build-android.sh
# には依存せず、本スクリプトは rustup ターゲット追加・cargo ndk 呼び出しを自己完結で
# 行う（iOS 側ミラータスク TASK-1.3-S3 / Issue #42 / PR #311 が確立した前例に倣う）。
# #44 マージ後は本スクリプトを build-android.sh 呼び出しへ置き換え、cargo-ndk 引数の
# 二重管理を解消することが望ましい（out-of-scope-tracking.md に記録。TASK-11.2-S4
# 時点でも #44/#45 は既にマージ済みだが、本二重管理自体は未解消のまま残っている）。
#
# TASK-11.2-S4（REQ-11 / M3、Issue #228）: fandhe-egui-proxy（cdylib、TASK-1.5-S2）の
# .so 検証を追加した（下記 PACKAGES 参照）。fandhe-core は rlib のみ、fandhe-egui-proxy は
# 3 ABI 分の libfandhe_egui_proxy.so を検証する。cargo-ndk 呼び出しは package ごとに
# 対象成果物種別（rlib / so）が異なるため、本スクリプトの単一ループで両方を処理する
# （新規スクリプトを並置しない）。
#
# `--aab` フラグ（TASK-11.2-S4 で追加）: TASK-11.2-S2（Issue #226、Gradle 統合・
# build-aab.sh）が本コミット時点でマージ済みのため実装した。build-aab.sh が生成する
# platforms/android/app/build/outputs/bundle/release/app-release.aab を対象に、
# 「生成済み成果物の独立した事後検証」（zip 構造・AndroidManifest.xml 収載・3 ABI 分の
# .so 収載）を行う。build-aab.sh 自身も生成直後に同様の検証を内蔵しているが、本フラグは
# 別プロセス・別タイミング（CI の別ステップ、生成物を退避しての異常系確認等）から
# 独立して再検証できることに意味があるため、生成ロジックとは責務を分けて二重管理しない
# （検証ロジックのみを本スクリプトに置き、cargo-ndk 呼び出し等の生成ロジックは
# build-aab.sh 側に閉じる）。
#
# 呼び出し元: ローカル開発者が直接実行する。.github/workflows/ci.yml の android-build
# ジョブへは TASK-11.2-S4 で `bash scripts/android/build-aab.sh` の後段ステップとして
# `--release --aab` 付きで組み込み済み。
# 前提: scripts/android/setup-android-env.sh 実行済み（JDK・NDK・cargo-ndk 導入済み）。
# `--aab` を付ける場合は事前に scripts/android/build-aab.sh の実行が必要（未生成時は
# 案内メッセージ付きで非 0 終了する）。
#
# scripts/android/setup-android-env.sh・scripts/ios/build-cross-targets.sh と同様に
# 冪等（何度実行しても同じ結果）・set -euo pipefail に揃える。
#
# 使い方: scripts/android/verify-android-artifacts.sh [--release] [--aab]
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

log() {
  echo "[verify-android-artifacts] $*"
}

fail() {
  echo "[verify-android-artifacts] ERROR: $*" >&2
  exit 1
}

release=0
aab=0
# --release / --aab はどちらの順序で渡しても解釈できるよう単純な走査で処理する
# （引数は 2 つのみで getopts 導入は過剰なため既存の素朴な流儀を維持）。
for arg in "$@"; do
  case "${arg}" in
    --release) release=1 ;;
    --aab) aab=1 ;;
    *) fail "不明な引数です: ${arg}（使い方: verify-android-artifacts.sh [--release] [--aab]）" ;;
  esac
done
profile="debug"
if [ "${release}" -eq 1 ]; then
  profile="release"
fi

# android-env.sh が versions.env の読み込みと ANDROID_NDK_HOME 等の export を行う
# （cargo-ndk は ANDROID_NDK_HOME からしか NDK を発見できないため必須の前提）。
# shellcheck source=android-env.sh
# shellcheck disable=SC1091
. "${script_dir}/android-env.sh"

command -v cargo >/dev/null 2>&1 || fail "cargo が見つかりません"
command -v cargo-ndk >/dev/null 2>&1 || fail "cargo-ndk が見つかりません（scripts/android/setup-android-env.sh を先に実行してください）"
[ -d "${ANDROID_NDK_HOME}" ] \
  || fail "ANDROID_NDK_HOME（${ANDROID_NDK_HOME}）が見つかりません。scripts/android/setup-android-env.sh を先に実行してください"

# 検証対象パッケージ・ABI・成果物名。
# fandhe-core: 共通コア（#![forbid(unsafe_code)]・プラットフォーム非依存）の rlib のみ。
# fandhe-egui-proxy: crate-type = ["cdylib", "rlib"]（TASK-1.5-S2）。Android 分岐は
# `android-activity` の NativeActivity 方式 + wgpu Backends::VULKAN 固定
# （crates/fandhe-egui-proxy/src/android.rs）で cdylib（libfandhe_egui_proxy.so）を
# 生成する。--lib（後述 cargo ndk 呼び出し）を付けて `[[bin]]
# fandhe-egui-proxy-desktop`（src/main.rs、native/デスクトップ向け。Android では
# no-op 化済み、src/main.rs 冒頭コメント参照）をビルド対象から除外する。TASK-11.2-S4 で
# `--lib` 無指定でも当該 bin は no-op 化により実際にはビルドが壊れないことを実機確認
# 済みだが、本検証スクリプトは cdylib/rlib 成果物の生成確認のみが責務のため、対象外の
# bin ビルドを含めない最小構成として --lib を明示する（設計判断）。
# ここに新たな package を追加する場合は package_artifact_suffixes にも同名キーで
# 対応する成果物種別（rlib / so、スペース区切りで複数可）を追加する。
packages="fandhe-core fandhe-egui-proxy"
declare -A package_artifact_suffixes=(
  [fandhe-core]="rlib"
  [fandhe-egui-proxy]="so"
)
# ABI（cargo-ndk -t 引数）と rustup/cargo の triple の対応表。
# triple 列は versions.env の RUST_ANDROID_TARGETS を単一の SSOT として参照する
# （android-env.sh が set -a 付きで source 済みのためここでは export 済みの値を読むだけ）。
# ABI 表記（arm64-v8a 等）は cargo-ndk 固有であり versions.env には対応表現が
# 存在しないため、triple から ABI への対応のみここで明示する。
abis="arm64-v8a armeabi-v7a x86_64"
: "${RUST_ANDROID_TARGETS:?RUST_ANDROID_TARGETS が未設定です（android-env.sh の source に失敗している可能性）}"
triples="${RUST_ANDROID_TARGETS}"
# Android 最小 API レベル（TASK-11.2-S4 で versions.env の ANDROID_MIN_API を単一の
# SSOT として参照する形へ統一。build-android.sh と同じ参照方法）。
: "${ANDROID_MIN_API:?ANDROID_MIN_API が未設定です（android-env.sh の source に失敗している可能性）}"
android_min_api="${ANDROID_MIN_API}"

cd "${repo_root}"

# PR #483 CI 是正（Issue #228 フォローアップ、run 30658486536 / 30659149425 /
# 30659770016 / 30660444124 で継続的に再現）: 当初はメインの `target/` を
# そのまま使い、(a) fandhe-core を先に -p 単独ビルド、(b) cargo clean してから
# 再ビルド、(c) fandhe-core と fandhe-egui-proxy を単一呼び出しの root unit に
# まとめる、の 3 段階で対策したが、CI runner の実 CI ではいずれも
# `target/<triple>/release/libfandhe_core.rlib` が見つからない事象を解消できな
# かった（fandhe-core は fandhe-egui-proxy の依存クレートでもあるため、直前の
# ステップ（build-aab.sh の `-p fandhe-egui-proxy -o <jniLibs dir>` 呼び出し）が
# メインの `target/` に対して行った処理の影響を受けていると推測されるが、
# CI runner 環境でしか再現せず、cargo・cargo-ndk 側の詳細な原因は
# 特定できていない）。
#
# 対策として、本スクリプトの release 検証ビルドはメインの `target/` を一切
# 共有せず、`--target-dir` で専用ディレクトリ（`target/android-verify`）を
# 指定する。build-aab.sh・build-android.sh 等の他ステップの cargo 呼び出しが
# 一切触れないディレクトリのため、cargo の uplift 挙動（他ステップの呼び出し
# 内容に左右される）に依存せず、本スクリプト自身が要求した `-p` 対象の
# ビルド結果のみが確実に uplift された状態で存在する（ローカル再現で確認済み）。
# 初回実行時は実コンパイルが発生する分コストが増えるが（fandhe-egui-proxy は
# eframe/egui 等の重い依存を含むため数分規模）、本スクリプトを繰り返し実行する
# 限りは `target/android-verify` 自体がキャッシュとして機能し、2 回目以降は
# 通常の fresh 判定で高速に完了する（冪等性は保たれる）。
verify_target_dir="${repo_root}/target/android-verify"

# --- cargo-ndk 呼び出し（全 package を単一呼び出しで --lib ビルド） ---
# 3 ABI を一括ビルド（-t 複数指定）し、その後 triple ごとに成果物の存在を検証する。
# `fandhe-core` と `fandhe-egui-proxy` を同一 cargo 呼び出しの `-p` root unit として
# 一緒にビルドする（package ごとに呼び出しを分けない。上記 `--target-dir` 隔離が
# 主対策のため必須ではなくなったが、cargo-ndk への呼び出し回数を減らせるため維持する）。
declare -a cargo_ndk_args=()
for abi in ${abis}; do
  cargo_ndk_args+=(-t "${abi}")
done
cargo_ndk_args+=(--platform "${android_min_api}" build)
for package in ${packages}; do
  cargo_ndk_args+=(-p "${package}")
done
cargo_ndk_args+=(--lib --target-dir "${verify_target_dir}")
if [ "${release}" -eq 1 ]; then
  cargo_ndk_args+=(--release)
fi

log "cargo ndk ${cargo_ndk_args[*]}"
cargo ndk "${cargo_ndk_args[@]}"

# --- 成果物検証（存在のみ） ---
# 上の `cargo ndk ... build` は set -euo pipefail 下で実行されており、
# ビルドが失敗すれば非 0 で exit してこの行には到達しない。したがって
# ここに到達した時点で cargo 自身が「今回の呼び出しで成果物が有効である」ことを
# 保証済みであり、本スクリプトが独自に鮮度（mtime）を再検証する必要はない。
#
# 過去の実装は「ビルド開始直前に作成したマーカーより成果物の mtime が新しいか」
# を追加で検証していたが、ソース差分がなく cargo-ndk 呼び出しが実質 no-op
# （リンクのみで既存 .rlib の mtime を更新しない）で完了するケースで、
# 有効な成果物が存在するにもかかわらず -nt 比較が失敗し、ウォームな
# ローカル再実行やキャッシュ有効な CI リトライで偽陽性の失敗を起こしていた
# （冪等性の要件（本ファイル冒頭コメント参照）に反する）。cargo の終了コードで
# 実行成否を判定できるため、ここでの検証は「期待するパスに成果物が存在するか」
# のみに絞る（`--target-dir` 隔離により、上記の uplift 欠落問題自体を踏まない）。
for package in ${packages}; do
  lib_name="lib${package//-/_}"
  artifact_suffixes="${package_artifact_suffixes[${package}]}"
  for triple in ${triples}; do
    for suffix in ${artifact_suffixes}; do
      artifact="${verify_target_dir}/${triple}/${profile}/${lib_name}.${suffix}"
      if [[ ! -f "${artifact}" ]]; then
        fail "期待したビルド成果物が見つからない: ${artifact}"
      fi
      log "成果物確認 OK: ${artifact}"
    done
  done
done

log "完了: ${triples}"

# --- .aab 検証（--aab 指定時のみ） -------------------------------------------
# build-aab.sh が生成した未署名 .aab に対する独立した事後検証。上記の cargo-ndk
# ループとは異なるプロセス（Gradle）が生成した成果物を対象とするため、ここまでの
# rlib/so 検証とは節を分けて実行する。
if [ "${aab}" -eq 1 ]; then
  gradle_project_dir="${repo_root}/platforms/android"
  aab_path="${gradle_project_dir}/app/build/outputs/bundle/release/app-release.aab"
  lib_name="fandhe_egui_proxy"

  if [ ! -f "${aab_path}" ]; then
    fail ".aab が見つかりません: ${aab_path}（先に bash scripts/android/build-aab.sh を実行してください）"
  fi
  log ".aab 存在確認 OK: ${aab_path}"

  # build-aab.sh と同じ SIGPIPE 対策（unzip -l | grep は set -o pipefail 下で
  # grep が先に終了すると unzip 側が SIGPIPE で終了しうるため、一覧をいったん
  # 変数へ capture してから照合する）。
  #
  # さらに、capture 後の照合であっても `printf '%s\n' "${var}" | grep -q ...` の
  # 形は同じ問題を再発させる（scripts/android/build-aab.sh で発覚した CI 実障害。
  # Issue #236 PR #498 レビュー指摘。詳細は build-aab.sh 側のコメント参照）:
  # `grep -q` が最初の一致で読み取りを止めると上流の `printf` が SIGPIPE を受けて
  # 非 0 終了し、`set -o pipefail` 下ではパイプ全体の終了ステータスがそれに
  # 引きずられて偽陰性になる。ここでは here-string（`<<<`）で変数を直接渡し、
  # パイプを経由させないことで同じ再発を防ぐ。
  if command -v unzip >/dev/null 2>&1; then
    aab_listing="$(unzip -l "${aab_path}")"
  elif command -v python3 >/dev/null 2>&1; then
    aab_listing="$(python3 -m zipfile -l "${aab_path}")"
  else
    fail "unzip も python3 も見つからないため .aab の構造検証ができません"
  fi

  # zip 構造妥当性: AndroidManifest.xml がベースモジュールに収載されていること。
  if ! grep -q "base/manifest/AndroidManifest.xml" <<<"${aab_listing}"; then
    fail ".aab 内に base/manifest/AndroidManifest.xml が見つかりません（zip 構造が不正、または Gradle 生成物ではない可能性）"
  fi
  log "  - base/manifest/AndroidManifest.xml 収載確認 OK"

  # 3 ABI 分の .so が収載されていること（abis は上の cargo-ndk 検証と同じ SSOT を使う）。
  for abi in ${abis}; do
    if ! grep -q "base/lib/${abi}/lib${lib_name}\.so" <<<"${aab_listing}"; then
      fail ".aab 内に base/lib/${abi}/lib${lib_name}.so が見つかりません"
    fi
    log "  - base/lib/${abi}/lib${lib_name}.so 収載確認 OK"
  done

  log ".aab 検証完了: ${aab_path}"
fi
