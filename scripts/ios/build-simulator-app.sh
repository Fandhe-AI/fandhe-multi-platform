#!/usr/bin/env bash
# TASK-11.1-S2/S3（REQ-11 / M3、Issue #221・#222、親: TASK-11.1 系列 / #219）:
# `fandhe-egui-proxy`（iOS エントリ実装済み、TASK-1.3-S2 / src/ios.rs）を
# iOS シミュレータターゲットへビルドし、シミュレータ版アプリバンドル（.app）へ
# ad-hoc 署名を施した上で `Payload/` 構造の .ipa（zip）を組み立てる。
#
# 呼び出し文脈: docs/spec/03-poc/store-submission/pipeline/build-and-sign.sh（PoC-3）で
# 実証済みの手順（cargo build → .app 組み立て → codesign → Payload zip）を、
# PoC の使い捨てスクリプトから本リポジトリの実クレート・実 Info.plist を対象とする
# 本実装スクリプトへ昇格させたもの（S2 で build/bundle/ipa/verify、
# S3（本コミット）で sign フェーズと verify の署名検査を追加）。
#
# sub-issue 間のスコープ境界（重要）:
#   - ad-hoc 署名（TASK-11.1-S3 / #222）は本スクリプトのフェーズ 3 で実施する。
#     PoC-3 実証済みの「署名 → .ipa 化」の順序を維持し、フェーズ 2（バンドル組み立て）
#     完了直後・フェーズ 4（.ipa 化）着手前に置く。
#   - CLI 全体構成（TASK-11.1-S1 / #220）は本コミット時点で未マージのため、本
#     スクリプトは単独実行可能なシェルスクリプトとして実装する。S1 の CLI 側は
#     下記の環境変数インターフェースを経由して本スクリプトを呼び出せる
#     （`FANDHE_IOS_SKIP_SIGN` を含む）。
#   - macOS 実機での cargo build・codesign 実実行・`simctl install/launch` による
#     起動確認は TASK-11.1-S4（#223）・人間担当の実機検証タスクへ引き継ぐ。
#
# scripts/ios/build-cross-targets.sh・scripts/apple/build-bridge-xcframework.sh と
# 同様に、冪等（何度実行しても同じ結果）・set -euo pipefail に揃える。
#
# 環境変数インターフェース（S1 の CLI から呼び出す契約として明記）:
#   FANDHE_IOS_SIM_TARGET  既定: aarch64-apple-ios-sim
#   FANDHE_IOS_PROFILE     既定: release（release または debug のみ対応。debug は
#                          cargo の組み込みプロファイル "dev" へ変換して渡す。
#                          それ以外の値は成果物パス導出規約が崩れるため拒否する）
#   FANDHE_IOS_APP_BINARY  既定: 空（cargo build を実行して成果物を使う）。
#                          非空の場合はビルドをスキップし、指定パスのバイナリを
#                          そのままアプリ実行体として使う（Apple SDK リンカが必要な
#                          cargo build を実行できない Linux 環境上で、本スクリプトの
#                          バンドル組み立て・.ipa 化ロジックをスタブバイナリで
#                          構造検証するための注入点。build-bridge-xcframework.sh が
#                          Linux 上で XCFramework 構造を直接組み立てる先例と同方針）。
#   FANDHE_IOS_SKIP_SIGN   既定: 空（署名する）。値 "1" のみ許容し、署名フェーズを
#                          スキップする（未署名・構造検証専用。実配布不可）。
#                          それ以外の非空値は成果物の署名有無が曖昧になるため
#                          明示的に拒否する（A03/A05 対策）。codesign が存在しない
#                          Linux 環境上でバンドル・.ipa 化ロジックを検証する場合に
#                          明示 opt-in で指定する。署名要求時（既定）に codesign が
#                          見つからない場合は、黙って未署名の成果物を成功扱いに
#                          しないため exit 1 とする。
#   FANDHE_IOS_OUT_DIR     既定: target/ios-sim（本スクリプト単体実行時の従来値）。
#                          scripts/build/build-and-sign.sh（TASK-11.1-S1 / #220、
#                          Issue #486 で委譲配線）が `--output-dir` で解決した
#                          出力先をここへ渡すための注入点。CLI 経由でない単体実行時は
#                          既定値のまま従来どおり target/ios-sim/ に生成される
#                          （後方互換）。
#
# 使い方:
#   scripts/ios/build-simulator-app.sh              # 引数なし: 全フェーズを順に実行（従来どおり）
#   scripts/ios/build-simulator-app.sh <phase>       # フェーズ単体実行（build-and-sign.sh からの委譲用）
#     <phase>: build | bundle | sign | ipa | verify
#
# フェーズ分割の経緯（Issue #486）: TASK-11.1-S1（build-and-sign.sh）の終了コード体系
# （4: ビルド / 5: 署名 / 6: パッケージング）へ失敗フェーズを対応付けられるよう、
# 本スクリプトを 1 回呼べば終わる構成から、位置引数でフェーズを選んで呼べる構成へ
# 拡張する。フェーズ間の状態はすべて決定的なファイルパス（.app/.ipa の固定パス）
# 経由で受け渡されるため、個別実行しても整合する。
set -euo pipefail

# CDPATH が設定された環境では、相対パスへの `cd` が意図しないディレクトリ
# （CDPATH 側の同名ディレクトリ）へ解決されうる（`cd` は相対パスかつ `./`/`../`
# で始まらない場合に CDPATH を探索するため、`>/dev/null` で解決先の出力を
# 抑止しても探索そのものは止まらない）。本スクリプトは CDPATH に一切依存
# しないため、起点で無効化して以降の全 `cd` を安全にする
# （Issue #488 Bugbot 指摘対応 / A05 対策）。
unset CDPATH

# scripts/build/build-and-sign.sh と同じ CDPATH-safe パターンに揃える
# （`cd -- ...` で `--` 以降をオプションとして解釈させず、`>/dev/null` で
# 解決先パスの出力を抑止し、`pwd -P` でシンボリックリンクを解決した
# 正規パスを取得する）。
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd -P)"
repo_root="$(cd -- "${script_dir}/../.." >/dev/null 2>&1 && pwd -P)"

package="fandhe-egui-proxy"
bin_name="fandhe-egui-proxy-desktop"
app_name="FandheIosApp"
target="${FANDHE_IOS_SIM_TARGET:-aarch64-apple-ios-sim}"
profile="${FANDHE_IOS_PROFILE:-release}"
injected_binary="${FANDHE_IOS_APP_BINARY:-}"
skip_sign="${FANDHE_IOS_SKIP_SIGN:-}"
# TASK-5.3（REQ-5 / M4、Issue #238）: `scripts/ios/measure-frame-rate.sh` が
# `--features frame-metrics` を透過させるための注入点。既定（空）では従来どおり
# feature 未指定で cargo build するため、既存呼び出し元（build-and-sign.sh 等）は
# 無変更で動く。
cargo_features="${FANDHE_IOS_CARGO_FEATURES:-}"

# 英数・ハイフン・カンマのみを許容する（cargo の feature 名規約に合わせたホワイトリスト）。
# 任意文字列をそのまま `cargo build --features` へ展開しない（A03 対策、security.md 準拠）。
if [[ -n "${cargo_features}" ]] && [[ ! "${cargo_features}" =~ ^[A-Za-z0-9,-]+$ ]]; then
  echo "[build-simulator-app] エラー: FANDHE_IOS_CARGO_FEATURES に許容されない文字が含まれています: ${cargo_features}" >&2
  exit 1
fi

# FANDHE_IOS_SKIP_SIGN は空（署名する）または "1"（明示 opt-in でスキップ）のみを
# 許容する。それ以外の非空値（"0"/"false"/"yes" 等）は「署名したのかどうか」が
# 呼び出し側の意図と一致しない状態で成果物が生成されるのを防ぐため拒否する
# （A03 対策: 外部入力を無検証で分岐条件へ使わない）。
case "${skip_sign}" in
  "" | "1") ;;
  *)
    echo "[build-simulator-app] エラー: FANDHE_IOS_SKIP_SIGN は空または 1 のみ対応: ${skip_sign}" >&2
    exit 1
    ;;
esac

info_plist_src="${repo_root}/platforms/apple/FandheIosApp/Resources/Info.plist"
# FANDHE_IOS_OUT_DIR は build-and-sign.sh（CLI）からの委譲時に --output-dir を
# 一致させるための注入点（Issue #486）。単体実行時は既定値のまま従来どおり。
out_dir="${FANDHE_IOS_OUT_DIR:-${repo_root}/target/ios-sim}"
# out_dir を絶対パスへ正規化する（Issue #488 Bugbot 指摘対応）。
# build-and-sign.sh の --output-dir は相対パス指定を許すため、正規化しないまま
# ipa_path（相対パス）を後段で使うと、phase_ipa 内で ipa_staging_dir へ `cd`
# した後に相対パスとして再解決されてしまい、意図しない場所へ .ipa が生成される
# （zip コマンド使用時、A05 対策: パス解決の曖昧さを起点で潰す）。
mkdir -p "${out_dir}"
# scripts/build/build-and-sign.sh と同様の CDPATH-safe パターンを使う（Issue #488
# Bugbot 指摘対応）。素の `cd`/`pwd` は CDPATH が設定された環境で意図しない
# ディレクトリへ解決されうる（CDPATH 自体はファイル先頭の `unset CDPATH` で
# 無効化済みだが、それに依存せず個々の `cd` も同一パターンへ揃える）。
# `cd -- ...` で `--` 以降をオプションとして解釈させず、`>/dev/null` で
# 解決先パスの出力を抑止し、`pwd -P` でシンボリックリンクを解決した
# 正規パスを取得する。
out_dir="$(cd -- "${out_dir}" >/dev/null 2>&1 && pwd -P)"
app_dir="${out_dir}/${app_name}.app"
ipa_staging_dir="${out_dir}/ipa"
ipa_path="${out_dir}/${app_name}-simulator.ipa"

if [[ ! -f "${info_plist_src}" ]]; then
  echo "[build-simulator-app] エラー: Info.plist が見つからない: ${info_plist_src}" >&2
  exit 1
fi

# --- 位置引数によるフェーズ選択（Issue #486） -------------------------------
# 未知のフェーズ名は case によるホワイトリスト検証で拒否する（A03 対策・eval 不使用）。
# 引数なしは従来どおり全フェーズ実行（後方互換）。
requested_phase="${1:-all}"
case "${requested_phase}" in
  all | build | bundle | sign | ipa | verify) ;;
  *)
    echo "[build-simulator-app] エラー: 未知のフェーズです: ${requested_phase}（対応: build/bundle/sign/ipa/verify、省略時は全フェーズ）" >&2
    exit 1
    ;;
esac

# build フェーズの成果物パスは target/profile から決定的に導出できるため、
# フェーズを分割実行しても bundle 側は build を再実行せず同じパスを参照できる
# （FANDHE_IOS_APP_BINARY 注入時はそのパスをそのまま使う）。
resolve_built_binary_path() {
  if [[ -n "${injected_binary}" ]]; then
    echo "${injected_binary}"
    return 0
  fi

  case "${profile}" in
    release) echo "${repo_root}/target/${target}/release/${bin_name}" ;;
    debug) echo "${repo_root}/target/${target}/debug/${bin_name}" ;;
    *)
      echo "[build-simulator-app] エラー: FANDHE_IOS_PROFILE は release または debug のみ対応: ${profile}" >&2
      exit 1
      ;;
  esac
}

# --- フェーズ 1: build ---------------------------------------------------
# Apple SDK リンカが必要なため cargo build は macOS 上でのみ成功する。
# FANDHE_IOS_APP_BINARY 指定時はビルドをスキップし、Linux 上でも
# バンドル組み立て・.ipa 化ロジック（フェーズ 2 以降）を検証できるようにする。
phase_build() {
  if [[ -n "${injected_binary}" ]]; then
    if [[ ! -f "${injected_binary}" ]]; then
      echo "[build-simulator-app] エラー: FANDHE_IOS_APP_BINARY で指定されたバイナリが見つからない: ${injected_binary}" >&2
      exit 1
    fi
    echo "[build-simulator-app] FANDHE_IOS_APP_BINARY 指定によりビルドをスキップ: ${injected_binary}"
    return 0
  fi

  echo "[build-simulator-app] rustup target add ${target}"
  rustup target add "${target}" >/dev/null

  # FANDHE_IOS_PROFILE は「利用者が見る名前」として release/debug のみを受け付ける。
  # cargo の組み込みプロファイル名は release は "release" だが debug 相当は "dev"
  # （出力ディレクトリのみ `target/<target>/debug/` になる）という非対称があるため、
  # ここで cargo へ渡すプロファイル名と成果物ディレクトリ名を個別に導出する。
  # それ以外の値（カスタムプロファイル等）は成果物パスの導出規約が崩れるため、
  # A03 対策も兼ねて明示的に拒否する。
  local cargo_profile
  case "${profile}" in
    release) cargo_profile="release" ;;
    debug) cargo_profile="dev" ;;
    *)
      echo "[build-simulator-app] エラー: FANDHE_IOS_PROFILE は release または debug のみ対応: ${profile}" >&2
      exit 1
      ;;
  esac

  # macOS 標準の bash 3.2 系では `set -u` 下で空配列に対する `"${arr[@]}"` 展開が
  # unbound variable エラーとなる（本リポジトリの他スクリプトで既知、
  # scripts/build/build-and-sign.sh の同種コメント参照）。`feature_args` は
  # `FANDHE_IOS_CARGO_FEATURES` 未設定時に空配列となりうるため、配列展開自体を
  # 避け、features 指定の有無で cargo build 呼び出しを分岐させる。
  if [[ -n "${cargo_features}" ]]; then
    echo "[build-simulator-app] cargo build -p ${package} --bin ${bin_name} --target ${target} --profile ${cargo_profile} --features ${cargo_features}"
    cargo build --manifest-path "${repo_root}/Cargo.toml" -p "${package}" --bin "${bin_name}" \
      --target "${target}" --profile "${cargo_profile}" --features "${cargo_features}"
  else
    echo "[build-simulator-app] cargo build -p ${package} --bin ${bin_name} --target ${target} --profile ${cargo_profile}"
    cargo build --manifest-path "${repo_root}/Cargo.toml" -p "${package}" --bin "${bin_name}" \
      --target "${target}" --profile "${cargo_profile}"
  fi

  local built_binary
  built_binary="$(resolve_built_binary_path)"
  if [[ ! -f "${built_binary}" ]]; then
    echo "[build-simulator-app] エラー: 期待したビルド成果物が見つからない: ${built_binary}" >&2
    exit 1
  fi
}

# --- フェーズ 2: bundle ----------------------------------------------------
# `.app/` を毎回作り直し（冪等）、バイナリを CFBundleExecutable と同名で配置する。
# 分割実行時は build フェーズが別呼び出しで完了している前提のため、ここでは
# ビルドを再実行せず、決定的パスの成果物が存在することのみを検査する。
phase_bundle() {
  local built_binary
  built_binary="$(resolve_built_binary_path)"
  if [[ ! -f "${built_binary}" ]]; then
    echo "[build-simulator-app] エラー: ビルド成果物が見つからない: ${built_binary}（先に build フェーズを実行してください）" >&2
    exit 1
  fi

  echo "[build-simulator-app] アプリバンドル組み立て: ${app_dir}"
  rm -rf "${app_dir}"
  mkdir -p "${app_dir}"
  cp "${built_binary}" "${app_dir}/${app_name}"
  cp "${info_plist_src}" "${app_dir}/Info.plist"
  cp "${info_plist_src%/*}/PrivacyInfo.xcprivacy" "${app_dir}/PrivacyInfo.xcprivacy"

  # CFBundleExecutable（Info.plist）とバンドル直下の実行体名が一致していることを検証する。
  # 不一致のまま .ipa 化すると、シミュレータが起動時に実行体を解決できず
  # インストール/起動に失敗する（PoC-3 の Info.plist 契約を参照）。
  local plist_executable
  plist_executable="$(python3 - "${app_dir}/Info.plist" <<'PYEOF'
import plistlib
import sys

with open(sys.argv[1], "rb") as f:
    print(plistlib.load(f)["CFBundleExecutable"])
PYEOF
  )"
  if [[ "${plist_executable}" != "${app_name}" ]]; then
    echo "[build-simulator-app] エラー: Info.plist の CFBundleExecutable（${plist_executable}）とバンドル実行体名（${app_name}）が不一致" >&2
    exit 1
  fi
  echo "[build-simulator-app] バンドル組み立て OK: ${app_dir}"
}

# --- フェーズ 3: sign（TASK-11.1-S3 / #222） --------------------------------
# PoC-3（docs/spec/03-poc/store-submission/pipeline/build-and-sign.sh L27・L45〜50）
# で実証済みの ad-hoc 署名手順（`codesign --sign - --force` → `--verify`）を
# ここへ昇格させる。証明書・署名鍵・Apple ID を使わない ad-hoc 署名のため、
# 認証情報は一切扱わない（security.md A02 参照）。
# 分割実行時は bundle フェーズが別呼び出しで完了している前提のため、
# .app の存在を前提検査として行う。
phase_sign() {
  if [[ ! -d "${app_dir}" ]]; then
    echo "[build-simulator-app] エラー: アプリバンドルが見つからない: ${app_dir}（先に bundle フェーズを実行してください）" >&2
    exit 1
  fi

  if [[ "${skip_sign}" == "1" ]]; then
    echo "[build-simulator-app] FANDHE_IOS_SKIP_SIGN=1 のため署名フェーズをスキップ（未署名・構造検証専用）"
    return 0
  fi

  if ! command -v codesign >/dev/null 2>&1; then
    echo "[build-simulator-app] エラー: codesign が見つからない（署名は macOS 専用ツールが必要）。" >&2
    echo "[build-simulator-app]       Linux 上で構造検証のみ行う場合は FANDHE_IOS_SKIP_SIGN=1 を指定すること。" >&2
    exit 1
  fi

  echo "[build-simulator-app] ad-hoc 署名: codesign --sign - --force ${app_dir}"
  codesign --sign - --force "${app_dir}"

  # 署名の成否を明示的に検証する。未検証のまま .ipa 化すると、署名が壊れた
  # 成果物を「成功」扱いにしてしまう（security.md A08 対策）。
  echo "[build-simulator-app] 署名検証: codesign --verify --verbose=2 ${app_dir}"
  codesign --verify --verbose=2 "${app_dir}"

  # 署名情報の表示はデバッグ用途であり、失敗しても致命的ではないため
  # 非致命（|| true）で扱う（PoC-3 と同方針）。
  codesign -dv "${app_dir}" || true

  echo "[build-simulator-app] ad-hoc 署名 OK: ${app_dir}"
}

# --- フェーズ 4: ipa -------------------------------------------------------
# `Payload/<app>.app` 構造を組み立てて zip 化する（.ipa の実体は zip アーカイブ）。
# 分割実行時は bundle（・必要なら sign）フェーズが別呼び出しで完了している
# 前提のため、.app の存在を前提検査として行う。
phase_ipa() {
  if [[ ! -d "${app_dir}" ]]; then
    echo "[build-simulator-app] エラー: アプリバンドルが見つからない: ${app_dir}（先に bundle フェーズを実行してください）" >&2
    exit 1
  fi

  echo "[build-simulator-app] .ipa 組み立て: ${ipa_path}"
  rm -rf "${ipa_staging_dir}"
  mkdir -p "${ipa_staging_dir}/Payload"
  cp -R "${app_dir}" "${ipa_staging_dir}/Payload/"
  rm -f "${ipa_path}"

  # macOS では zip コマンドが標準搭載だが、本 Linux 開発環境には存在しないため、
  # 標準ライブラリのみで完結する python3 の zipfile モジュールへフォールバックする
  # （A03 対策: 外部入力を経由しない固定パス・固定引数のみを扱う）。
  if command -v zip >/dev/null 2>&1; then
    (cd "${ipa_staging_dir}" && zip -qr "${ipa_path}" Payload)
  elif command -v python3 >/dev/null 2>&1; then
    python3 - "${ipa_staging_dir}" "${ipa_path}" <<'PYEOF'
import os
import sys
import zipfile

staging_dir, ipa_path = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(ipa_path, "w", zipfile.ZIP_DEFLATED) as zf:
    for root, _dirs, files in os.walk(os.path.join(staging_dir, "Payload")):
        for name in files:
            full_path = os.path.join(root, name)
            arcname = os.path.relpath(full_path, staging_dir)
            zf.write(full_path, arcname)
PYEOF
  else
    echo "[build-simulator-app] エラー: zip も python3 も見つからないため .ipa を生成できない" >&2
    exit 1
  fi

  if [[ ! -f "${ipa_path}" ]]; then
    echo "[build-simulator-app] エラー: .ipa の生成に失敗した: ${ipa_path}" >&2
    exit 1
  fi
  echo "[build-simulator-app] .ipa 生成 OK: ${ipa_path}"
}

# --- フェーズ 5: verify（構造検証） -----------------------------------------
# バンドル・.ipa のファイル配置を検査する。署名が要求されていた場合
# （skip_sign != "1"）は `_CodeSignature/CodeResources` の存在も必須として検査し、
# フェーズ 3 の `codesign --verify` に加えて成果物側の構造からも署名の痕跡を
# 確認する。署名済みかどうかは成果物の状態（ファイルの有無）からではなく、
# 呼び出し側の意図（FANDHE_IOS_SKIP_SIGN）から導出する（A08 対策）。
# 状態から逆算すると、署名要求時に codesign が実際には CodeResources を
# 生成できなかった壊れた成果物を「未署名として黙って成功」させてしまい、
# 「署名済みのはずが見つからない」というアサーションが機能しなくなるため。
# 分割実行時は ipa フェーズが別呼び出しで完了している前提のため、.ipa の
# 存在を前提検査として行う。
phase_verify() {
  if [[ ! -f "${ipa_path}" ]]; then
    echo "[build-simulator-app] エラー: .ipa が見つからない: ${ipa_path}（先に ipa フェーズを実行してください）" >&2
    exit 1
  fi

  echo "[build-simulator-app] 構造検証:"
  if [[ ! -f "${app_dir}/${app_name}" ]]; then
    echo "[build-simulator-app] エラー: バンドル直下に実行体が存在しない: ${app_dir}/${app_name}" >&2
    exit 1
  fi
  if [[ ! -f "${app_dir}/Info.plist" ]]; then
    echo "[build-simulator-app] エラー: バンドル直下に Info.plist が存在しない: ${app_dir}/Info.plist" >&2
    exit 1
  fi
  # `unzip -l | grep -q` は set -o pipefail 下で grep が先にマッチ終了すると
  # unzip 側が SIGPIPE（141）で終了し、パイプライン全体の終了コードが 141 になり
  # 実際は正しい構造でも「見つからない」と誤判定しうる。一覧をいったん変数へ
  # captureしてから照合することでこれを避ける（unzip 自体の実行はここで完結させる）。
  # 併せて phase 4（zip 生成）と同じく unzip が存在しない環境向けに python3 の
  # zipfile へフォールバックする（unzip を無条件の必須ツールにしない）。
  local ipa_listing
  if command -v unzip >/dev/null 2>&1; then
    ipa_listing="$(unzip -l "${ipa_path}")"
  elif command -v python3 >/dev/null 2>&1; then
    ipa_listing="$(python3 -m zipfile -l "${ipa_path}")"
  else
    echo "[build-simulator-app] エラー: unzip も python3 も見つからないため .ipa の構造検証ができない" >&2
    exit 1
  fi
  if ! printf '%s\n' "${ipa_listing}" | grep -q "Payload/${app_name}\.app/${app_name}"; then
    echo "[build-simulator-app] エラー: .ipa 内に Payload/${app_name}.app/${app_name} が見つからない" >&2
    exit 1
  fi
  echo "[build-simulator-app]   - ${app_dir}/${app_name} 存在確認 OK"
  echo "[build-simulator-app]   - ${app_dir}/Info.plist 存在確認 OK"
  echo "[build-simulator-app]   - ${ipa_path} 内 Payload/${app_name}.app 構造確認 OK"

  # 署名が要求されていた場合（skip_sign != "1"）のみ、codesign が生成する
  # _CodeSignature/CodeResources の存在をバンドル直下・.ipa 内の両方で必須検査する。
  # codesign --verify（フェーズ 3）は署名の正当性を検査するが、ここでは
  # 「.ipa 化の過程で署名済みバンドルが壊れずに取り込まれたか」を成果物側から
  # 独立に確認する。署名済みのはずが痕跡が見つからない場合は、未署名扱いで
  # 黙って成功させず exit 1 とする（A08 対策。security.md 参照）。
  local signed=0
  if [[ "${skip_sign}" != "1" ]]; then
    signed=1
    if [[ ! -f "${app_dir}/_CodeSignature/CodeResources" ]]; then
      echo "[build-simulator-app] エラー: 署名済みのはずが _CodeSignature/CodeResources が見つからない: ${app_dir}/_CodeSignature/CodeResources" >&2
      exit 1
    fi
    if ! printf '%s\n' "${ipa_listing}" | grep -q "Payload/${app_name}\.app/_CodeSignature/CodeResources"; then
      echo "[build-simulator-app] エラー: .ipa 内に Payload/${app_name}.app/_CodeSignature/CodeResources が見つからない" >&2
      exit 1
    fi
    echo "[build-simulator-app]   - ${app_dir}/_CodeSignature/CodeResources 存在確認 OK"
    echo "[build-simulator-app]   - ${ipa_path} 内 _CodeSignature/CodeResources 構造確認 OK"
  fi

  echo "[build-simulator-app] 完了: ${app_dir} / ${ipa_path}"
  if [[ "${signed}" -eq 1 ]]; then
    echo "[build-simulator-app] 本成果物は ad-hoc 署名済み（codesign --sign - --force）。"
    echo "[build-simulator-app]       実配布署名・notarization は REQ-12（別スコープ）で扱う。"
  else
    echo "[build-simulator-app] 注意: FANDHE_IOS_SKIP_SIGN=1 指定により本成果物は未署名であり、"
    echo "[build-simulator-app]       構造検証専用（シミュレータ実インストール・実配布不可）。"
  fi
}

# --- ディスパッチ -----------------------------------------------------------
# 引数なし（all）は従来どおり 5 フェーズを順に実行する（後方互換）。
case "${requested_phase}" in
  all)
    phase_build
    phase_bundle
    phase_sign
    phase_ipa
    phase_verify
    ;;
  build) phase_build ;;
  bundle) phase_bundle ;;
  sign) phase_sign ;;
  ipa) phase_ipa ;;
  verify) phase_verify ;;
esac
