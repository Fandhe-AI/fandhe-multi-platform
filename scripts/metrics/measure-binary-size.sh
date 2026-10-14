#!/usr/bin/env bash
# モバイル配布物・cdylib のバイナリサイズを機械的・再現可能に計測する読み取り専用
# スクリプト（ビルドは行わない。渡された成果物のサイズを記録するのみ）。
#
# 対応: REQ-5（`docs/spec/04-requirements.md` バイナリサイズは絶対値 20MB 以内を
# 主基準、ネイティブ実装比の相対基準は参考指標に格下げ）/ TASK-5.4（`docs/spec/05-tasks.md`）/
# M4 完了基準（`docs/spec/06-roadmap.md`）/ Conditional Go 条件 3、Issue #239。
#
# 呼び出し文脈: バイナリサイズ検証記録（非公開）の作成時に、
# `scripts/android/build-aab.sh` が生成する `.aab`・cargo-ndk 出力の ABI 別
# `libfandhe_egui_proxy.so` に対して個別に実行し、CSV 出力をレポートへ転記する。
# measure-idle-memory.sh・measure-code-ratio.sh と同方針で、現時点では CI へは
# 組み込まない（ローカル実行のみ）。
#
# 使い方:
#   bash scripts/metrics/measure-binary-size.sh <成果物パス> [<成果物パス> ...]
#
# 出力: 標準出力に CSV（列: path,bytes,mib,sha256）を書く。各ファイルについて
# 20MB（20 * 1024 * 1024 バイト）以内か否かの判定を標準エラーへ出力する。
# `.aab` / `.apk` を渡された場合は、`unzip -l`（無ければ `python3 -m zipfile -l`）に
# よる ABI 別 `lib/<abi>/*.so` の内訳サイズも標準エラーへ追加出力する。この内訳は
# 圧縮・整列によるコンテナオーバーヘッドを含まない zip エントリ単位の概算であり、
# 実際のダウンロードサイズとは一致しない近似値である旨に注意すること
# （bundletool 等の新規バイナリ導入はしない。ci.md の「野良インストール禁止」・
# security.md A06「脆弱な依存」対策）。
set -euo pipefail

readonly TWENTY_MIB_BYTES=$((20 * 1024 * 1024))

if [ "$#" -lt 1 ]; then
  echo "使い方: bash scripts/metrics/measure-binary-size.sh <成果物パス> [<成果物パス> ...]" >&2
  exit 1
fi

# sha256 コマンドの可搬性吸収（Linux: sha256sum、macOS: shasum -a 256）。
sha256_of() {
  local target="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "${target}" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "${target}" | awk '{print $1}'
  else
    echo "エラー: sha256sum も shasum も見つかりません" >&2
    exit 1
  fi
}

# バイト数を MiB（小数第 2 位）へ変換する。A03 対策として算術展開のみを使い、
# 外部入力（ファイルパス）を式へ直接埋め込まない。
bytes_to_mib() {
  local b="$1"
  awk -v bytes="${b}" 'BEGIN { printf "%.2f", bytes / 1048576 }'
}

# .aab / .apk（zip 形式）に対して ABI 別 lib/<abi>/*.so の内訳サイズを標準エラーへ
# 出力する。unzip -l と python3 -m zipfile -l は列順が異なる
# （scripts/android/build-aab.sh のフェーズ 3 検証コメント参照: unzip -l は
# 「Length ... Name」でファイル名が末尾、python3 -m zipfile -l は
# 「File Name Modified Size」でファイル名が先頭）ため、ツールごとに
# サイズ・ファイル名の抽出列を分ける。
print_abi_breakdown() {
  local target="$1"
  local listing_lines=()

  if command -v unzip >/dev/null 2>&1; then
    # unzip -l の行形式: "  <length>  <date> <time>   <name>"
    while IFS= read -r line; do
      listing_lines+=("${line}")
    done < <(unzip -l -- "${target}" 2>/dev/null | awk '{ size=$1; name=$4; for (i=5; i<=NF; i++) name = name " " $i; if (name ~ /lib\/[^\/]+\/.*\.so$/) print size "\t" name }')
  elif command -v python3 >/dev/null 2>&1; then
    # python3 -m zipfile -l の行形式: "<name>  <modified>  <size>"（1 行目はヘッダー）
    while IFS= read -r line; do
      listing_lines+=("${line}")
    done < <(python3 -m zipfile -l "${target}" 2>/dev/null | tail -n +2 | awk '{ size=$NF; name=$1; if (name ~ /lib\/[^\/]+\/.*\.so$/) print size "\t" name }')
  else
    echo "  警告: unzip も python3 も見つからないため ABI 別内訳は計測できません" >&2
    return
  fi

  if [ "${#listing_lines[@]}" -eq 0 ]; then
    echo "  ABI 別内訳: lib/<abi>/*.so エントリが見つかりませんでした" >&2
    return
  fi

  echo "  ABI 別内訳（zip エントリサイズの近似値。実ダウンロードサイズとは不一致）:" >&2
  # ABI ごとにサイズを集計する（連想配列は bash 4 以降必須。CI 非組み込み・
  # ローカル実行のみのため bash バージョン要件はここに閉じる）。
  declare -A abi_totals=()
  local entry size name abi
  for entry in "${listing_lines[@]}"; do
    size="$(printf '%s' "${entry}" | cut -f1)"
    name="$(printf '%s' "${entry}" | cut -f2-)"
    if ! [[ "${size}" =~ ^[0-9]+$ ]]; then
      continue
    fi
    abi="$(printf '%s' "${name}" | sed -nE 's#.*lib/([^/]+)/.*#\1#p')"
    [ -z "${abi}" ] && continue
    abi_totals["${abi}"]=$(( ${abi_totals["${abi}"]:-0} + size ))
  done
  for abi in "${!abi_totals[@]}"; do
    printf '    %s: %s bytes (%s MiB)\n' "${abi}" "${abi_totals[${abi}]}" "$(bytes_to_mib "${abi_totals[${abi}]}")" >&2
  done
}

echo "path,bytes,mib,sha256"

for target in "$@"; do
  if [ ! -f "${target}" ]; then
    echo "エラー: ファイルが見つかりません: ${target}" >&2
    exit 1
  fi

  bytes="$(wc -c < "${target}" | tr -d '[:space:]')"
  mib="$(bytes_to_mib "${bytes}")"
  digest="$(sha256_of "${target}")"

  echo "${target},${bytes},${mib},${digest}"

  if [ "${bytes}" -le "${TWENTY_MIB_BYTES}" ]; then
    echo "判定 OK: ${target} は 20MiB 以内です（${mib} MiB）" >&2
  else
    echo "判定 NG: ${target} は 20MiB を超えています（${mib} MiB）" >&2
  fi

  case "${target}" in
    *.aab | *.apk)
      print_abi_breakdown "${target}"
      ;;
  esac
done
