#!/usr/bin/env bash
# 共通コード（crates/fandhe-prototype）とプラットフォーム別コードの行数を機械的・
# 再現可能に計測し、共通コードベース比率を算出する読み取り専用スクリプト。
#
# 対応: REQ-2 / TASK-2.2-S6（docs/spec/04-requirements.md・05-tasks.md）。
# 呼び出し文脈: TASK-2.3（M3 Conditional Go 条件 2 の再計測）が実行し、出力を
# 検証記録（非公開）へ転記する想定（分類・計測規則の確定内容は
# docs/architecture/code-ratio-classification.md を参照）。unsafe-audit.sh と同方針で
# 現時点では CI へは組み込まない（ローカル実行のみ）。
#
# 分類（分類 SSOT は scripts/metrics/code-classification.env）:
#   - 共通コード（分子・分母）   : COMMON_SRC_DIRS 配下の *.rs 全体
#   - プラットフォーム別コード（分母のみ）:
#       (a) PLATFORM_FILES に列挙したファイル全体
#       (b) PLATFORM_MARKER_DIRS 配下の *.rs 中、
#           `// fandhe-prototype:begin` 〜 `// fandhe-prototype:end` のマーカー区間
#   - 母集団外: fandhe-core・fandhe-egui-proxy の既存コード（マーカー区間外）・
#     Cargo.toml / HTML / スクリプト・tests/ 配下（結合テスト）
#
# 行数の数え方:
#   - 対象は *.rs のみ（PoC-1 互換）
#   - 主指標 = 物理行（wc -l 相当）、参考指標 = 実効行（空行・コメント専用行を除く）
#   - `#[cfg(test)]` の行からファイル末尾までは対象外（同一ファイルに複数出現時は fail-closed）
#   - マーカー区間の begin/end が不対応（片方のみ・入れ子）の場合も fail-closed
#
# 使い方: bash scripts/metrics/measure-code-ratio.sh [--markdown]
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." >/dev/null 2>&1 && pwd -P)"
cd -- "${REPO_ROOT}"

CLASSIFICATION_ENV="${REPO_ROOT}/scripts/metrics/code-classification.env"
if [ ! -f "${CLASSIFICATION_ENV}" ]; then
  echo "エラー: 分類 SSOT が見つかりません: ${CLASSIFICATION_ENV}" >&2
  exit 1
fi
# shellcheck source=code-classification.env
source -- "${CLASSIFICATION_ENV}"

# 分類 SSOT が必須変数を定義しているか（未定義のまま実行し誤集計するのを防ぐ fail-closed 検査）。
for required_var in COMMON_SRC_DIRS PLATFORM_FILES PLATFORM_MARKER_DIRS; do
  if ! declare -p "${required_var}" >/dev/null 2>&1; then
    echo "エラー: 分類 SSOT に ${required_var} が定義されていません: ${CLASSIFICATION_ENV}" >&2
    exit 1
  fi
done

MARKDOWN_OUTPUT=0
for arg in "$@"; do
  case "${arg}" in
    --markdown) MARKDOWN_OUTPUT=1 ;;
    *)
      echo "エラー: 不明な引数です: ${arg}（使用可能: --markdown）" >&2
      exit 1
      ;;
  esac
done

# ファイル 1 件について「#[cfg(test)] より前（無ければ全体）」の物理行数を求める。
# #[cfg(test)] が複数出現した場合は fail-closed（awk が exit 1 し、呼び出し元の
# `|| { ...; exit 1; }` がスクリプト全体を即座にエラー終了させる。標準出力への
# 空文字出力は行わない）。
# `//` コメント行は判定対象から除外する（「#[cfg(test)] について説明する日本語コメント」
# 等が実属性と誤認され、本来の境界より手前で fail-closed・切り詰めが起きるのを防ぐため）。
# 標準出力: 正常時は物理行数（1 行）。
count_effective_end_line() {
  local file="$1"
  awk '
    {
      line = $0
      gsub(/^[ \t]+/, "", line)
      if (line ~ /^\/\//) { next }
    }
    /#\[cfg\(test\)\]/ {
      test_count++
      if (test_count == 1) { test_line = NR }
    }
    END {
      if (test_count > 1) { exit 1 }
      total = NR
      if (test_line > 0) { print test_line - 1 } else { print total }
    }
  ' "${file}"
}

# ファイルの先頭から指定行数までを対象に、物理行数と実効行数（空行・コメント専用行を除く）を求める。
# 標準出力: "<物理行数> <実効行数>"
count_physical_and_effective() {
  local file="$1" end_line="$2"
  if [ "${end_line}" -le 0 ]; then
    echo "0 0"
    return 0
  fi
  head -n "${end_line}" -- "${file}" | awk '
    {
      physical++
      line = $0
      gsub(/^[ \t]+/, "", line)
      if (line == "" ) { next }
      if (line ~ /^\/\//) { next }
      effective++
    }
    END { printf "%d %d\n", physical + 0, effective + 0 }
  '
}

# ディレクトリ配下の *.rs をパス昇順・NUL 区切りで列挙する。
# `find -print0 | sort -z` は GNU sort 前提であり、macOS 標準の BSD sort は `-z` 未対応で
# `set -o pipefail` 下ではこの時点でスクリプトが異常終了する。ここでは一旦改行区切りへ
# 変換して `sort` を掛け、NUL 区切りへ戻すことで GNU/BSD 双方の sort で動作させる
# （対象はソースツリー配下の *.rs パスのみのため、ファイル名に改行文字を含まない前提）。
list_rs_files_sorted() {
  local dir="$1"
  find "${dir}" -type f -name '*.rs' -print0 | tr '\0' '\n' | LC_ALL=C sort | tr '\n' '\0'
}

# COMMON_SRC_DIRS 配下の *.rs 全体を共通コードとして集計する。
# ファイル一覧は find -print0 + null 区切りで安全に受け渡す（A03 インジェクション対策）。
common_physical_total=0
common_effective_total=0
common_file_count=0
declare -a common_file_report=()

for common_dir in "${COMMON_SRC_DIRS[@]}"; do
  if [ ! -d "${common_dir}" ]; then
    echo "エラー: 共通コード対象ディレクトリが存在しません: ${common_dir}" >&2
    exit 1
  fi
  while IFS= read -r -d '' rs_file; do
    end_line="$(count_effective_end_line "${rs_file}")" || {
      echo "エラー: ${rs_file} に #[cfg(test)] が複数出現しています（fail-closed）" >&2
      exit 1
    }
    read -r phys eff <<<"$(count_physical_and_effective "${rs_file}" "${end_line}")"
    common_physical_total=$((common_physical_total + phys))
    common_effective_total=$((common_effective_total + eff))
    common_file_count=$((common_file_count + 1))
    common_file_report+=("${rs_file}"$'\t'"${phys}"$'\t'"${eff}")
  done < <(list_rs_files_sorted "${common_dir}")
done

# PLATFORM_FILES（ファイル全体が対象）を集計する。
# code-classification.env に同一相対パスが重複して列挙されている場合、二重計上により
# 分母が水増しされるのを防ぐため fail-closed でエラー終了する（`platform_files_seen` で判定）。
platform_physical_total=0
platform_effective_total=0
declare -a platform_file_report=()
# 連想配列（declare -A）は Bash 4+ 限定であり、macOS 標準 /bin/bash（3.2 系）では
# `declare: -A: invalid option` で fail する。本スクリプトは Bash 3.2 でも動作させる
# ため、通常配列 + 線形探索の `platform_file_is_seen` で「既に計上済みか」を判定する
# （PLATFORM_FILES・PLATFORM_MARKER_DIRS はいずれも少数のためコスト上問題にならない）。
declare -a platform_files_seen=()

# platform_files_seen に指定パスが既に記録済みかを判定する。
# 標準出力なし。終了ステータス 0=記録済み / 1=未記録。
platform_file_is_seen() {
  local target="$1" seen
  for seen in "${platform_files_seen[@]:-}"; do
    if [ "${seen}" = "${target}" ]; then
      return 0
    fi
  done
  return 1
}

for platform_file in "${PLATFORM_FILES[@]:-}"; do
  [ -z "${platform_file}" ] && continue
  if platform_file_is_seen "${platform_file}"; then
    echo "エラー: PLATFORM_FILES に同一パスが重複して列挙されています（fail-closed）: ${platform_file}" >&2
    exit 1
  fi
  if [ ! -f "${platform_file}" ]; then
    echo "エラー: PLATFORM_FILES に列挙されたファイルが存在しません: ${platform_file}" >&2
    exit 1
  fi
  end_line="$(count_effective_end_line "${platform_file}")" || {
    echo "エラー: ${platform_file} に #[cfg(test)] が複数出現しています（fail-closed）" >&2
    exit 1
  }
  read -r phys eff <<<"$(count_physical_and_effective "${platform_file}" "${end_line}")"
  platform_physical_total=$((platform_physical_total + phys))
  platform_effective_total=$((platform_effective_total + eff))
  platform_file_report+=("${platform_file}"$'\t'"${phys}"$'\t'"${eff}"$'\t'"ファイル全体")
  platform_files_seen+=("${platform_file}")
done

# PLATFORM_MARKER_DIRS 配下の *.rs から `// fandhe-prototype:begin` 〜
# `// fandhe-prototype:end` のマーカー区間を走査する（PLATFORM_FILES で
# 既にファイル全体を計上済みのファイルは二重計上を避けるため対象外）。
# begin/end の不対応（片方のみ・入れ子）は fail-closed（awk の exit 1 をそのまま伝播）。
scan_marker_regions() {
  local file="$1" end_line="$2"
  head -n "${end_line}" -- "${file}" | awk '
    /\/\/[ \t]*fandhe-prototype:begin/ {
      if (in_marker) { print "MARKER_ERROR:nested_begin" > "/dev/stderr"; exit 1 }
      in_marker = 1
      next
    }
    /\/\/[ \t]*fandhe-prototype:end/ {
      if (!in_marker) { print "MARKER_ERROR:end_without_begin" > "/dev/stderr"; exit 1 }
      in_marker = 0
      next
    }
    in_marker {
      physical++
      line = $0
      gsub(/^[ \t]+/, "", line)
      if (line == "") { next }
      if (line ~ /^\/\//) { next }
      effective++
    }
    END {
      if (in_marker) { print "MARKER_ERROR:unterminated_begin" > "/dev/stderr"; exit 1 }
      printf "%d %d\n", physical + 0, effective + 0
    }
  '
}

for marker_dir in "${PLATFORM_MARKER_DIRS[@]:-}"; do
  [ -z "${marker_dir}" ] && continue
  if [ ! -d "${marker_dir}" ]; then
    echo "エラー: マーカー走査対象ディレクトリが存在しません: ${marker_dir}" >&2
    exit 1
  fi
  while IFS= read -r -d '' rs_file; do
    # PLATFORM_FILES で既にファイル全体を計上済みなら二重計上を避けるためスキップする。
    if platform_file_is_seen "${rs_file}"; then
      continue
    fi
    end_line="$(count_effective_end_line "${rs_file}")" || {
      echo "エラー: ${rs_file} に #[cfg(test)] が複数出現しています（fail-closed）" >&2
      exit 1
    }
    marker_result="$(scan_marker_regions "${rs_file}" "${end_line}")" || {
      echo "エラー: ${rs_file} の fandhe-prototype:begin/end マーカーが不対応です（fail-closed）" >&2
      exit 1
    }
    read -r phys eff <<<"${marker_result}"
    if [ "${phys}" -gt 0 ]; then
      platform_physical_total=$((platform_physical_total + phys))
      platform_effective_total=$((platform_effective_total + eff))
      platform_file_report+=("${rs_file}"$'\t'"${phys}"$'\t'"${eff}"$'\t'"マーカー区間")
    fi
  done < <(list_rs_files_sorted "${marker_dir}")
done

total_physical=$((common_physical_total + platform_physical_total))
total_effective=$((common_effective_total + platform_effective_total))

# 比率計算（awk で小数点第 1 位まで。分母 0 は「計測準備段階」として N/A 扱いにする）。
ratio_str() {
  local numerator="$1" denominator="$2"
  if [ "${denominator}" -eq 0 ]; then
    echo "N/A"
    return 0
  fi
  awk -v n="${numerator}" -v d="${denominator}" 'BEGIN { printf "%.1f%%", (n / d) * 100 }'
}

physical_ratio="$(ratio_str "${common_physical_total}" "${total_physical}")"
effective_ratio="$(ratio_str "${common_effective_total}" "${total_effective}")"

echo "=== 共通/プラットフォーム別コード行数計測（TASK-2.2-S6 / REQ-2） ==="
echo "対象コミット: $(git rev-parse HEAD 2>/dev/null || echo '不明（git 管理外）')"
echo

echo "--- 共通コード（${common_file_count} ファイル） ---"
for entry in "${common_file_report[@]:-}"; do
  [ -z "${entry}" ] && continue
  IFS=$'\t' read -r f phys eff <<<"${entry}"
  echo "  ${f}: 物理行 ${phys} / 実効行 ${eff}"
done
echo "  小計: 物理行 ${common_physical_total} / 実効行 ${common_effective_total}"
echo

echo "--- プラットフォーム別コード（${#platform_file_report[@]} 件） ---"
if [ "${#platform_file_report[@]}" -eq 0 ]; then
  echo "  （計測準備段階: 分母のプラットフォーム側差分がまだありません。"
  echo "   code-classification.env の PLATFORM_FILES / PLATFORM_MARKER_DIRS を追記してください）"
else
  for entry in "${platform_file_report[@]}"; do
    IFS=$'\t' read -r f phys eff kind <<<"${entry}"
    echo "  ${f} [${kind}]: 物理行 ${phys} / 実効行 ${eff}"
  done
fi
echo "  小計: 物理行 ${platform_physical_total} / 実効行 ${platform_effective_total}"
echo

echo "=== 集計 ==="
echo "総物理行数: ${total_physical}（共通 ${common_physical_total} / プラットフォーム別 ${platform_physical_total}）"
echo "総実効行数: ${total_effective}（共通 ${common_effective_total} / プラットフォーム別 ${platform_effective_total}）"
echo "共通コードベース比率（物理行、主指標）: ${physical_ratio}"
echo "共通コードベース比率（実効行、参考指標）: ${effective_ratio}"

if [ "${platform_physical_total}" -eq 0 ]; then
  echo
  echo "注意: 計測準備段階です（プラットフォーム別コードの分母がまだ 0 のため、比率は"
  echo "      共通コードのみで 100% になります。分類 SSOT を追記後に再計測してください）。"
fi

if [ "${MARKDOWN_OUTPUT}" -eq 1 ]; then
  echo
  echo "### 計測結果（Markdown）"
  echo
  echo "| 区分 | 物理行 | 実効行 |"
  echo "|------|--------|--------|"
  echo "| 共通コード | ${common_physical_total} | ${common_effective_total} |"
  echo "| プラットフォーム別コード | ${platform_physical_total} | ${platform_effective_total} |"
  echo "| 合計 | ${total_physical} | ${total_effective} |"
  echo "| 共通コードベース比率 | ${physical_ratio} | ${effective_ratio} |"
fi

exit 0
