#!/bin/bash
# 子 issue に紐づく open な PR を一定間隔で確かめ、マージ条件を満たした PR・main と競合した PR・
# CI のチェックが失敗した PR が出たら 1 行ずつ出して終わる（expand-capability Step 8「マージ待ち」。run の終了を待たずにマージするため）。
#
# **このスクリプトはマージしない。** 条件確認は scripts/pr/guarded-merge.sh（SSOT）に任せ、
# READY の PR は main が guarded-merge.sh の最終行の `gh pr merge` を直接打つ（PreToolUse hook が再検証する）。
# ここで使う guarded-merge.sh は作業ツリーの版なので READY は目安にすぎず、可否は hook が origin/main の版で決める。
#
# 使い方: scripts/device/wait-pr-ready.sh <子 issue 番号>...
#   - 出力: `READY #<PR> sha=...`（条件を満たした）/ `CONFLICTING #<PR>`（main と競合）。1 件でも出たら終了 0
#     `FAILED #<PR> <失敗したチェック名をカンマ区切り>`（現在の head でチェックが fail / cancel）は終了 3。
#     pending が 1 つでも残る間は判定せず待つ（途中経過の fail を拾わない）。READY / CONFLICTING が同じ周回で
#     出ていればそれらを優先し、FAILED は出さない。FAILED の PR は修正を push してから回し直す
#   - 対象の PR: 本文の Closes が子 issue を指すもの、またはブランチ名が implement-issue-tree の規則
#     `<type>/<子 issue 番号>-...` に合うもの（run は Closes が無い PR も正常扱いするため両方で拾う）
#   - KNOWN_CONFLICTING: 競合を報告済みの PR 番号（空白区切り）。これらは CONFLICTING を出し直さないが、
#     run が競合を解いて条件を満たせば READY は出す（同じ parent の run の終了を待つ間、待機を 1 本で回すため）
#   - INTERVAL: 確認の間隔（秒、既定 60）。MAX_SEC: これを超えたら `TIMEOUT` を出して終了 2（既定 4 時間）
#   - 終了コード: 0 = READY / CONFLICTING、1 = 引数不正・GH_FAILED、2 = TIMEOUT、3 = FAILED
#   - gh の PR 一覧の取得（open な PR が 1000 件以上で取りこぼしうる場合を含む）が 10 回続けて失敗したら
#     `GH_FAILED` を出して終了 1（認証切れ等で黙って回り続けないため）
set -u -o pipefail

[ $# -gt 0 ] || { echo "使い方: $0 <子 issue 番号>..." >&2; exit 1; }
for k in "$@"; do
  case "${k}" in ''|*[!0-9]*) echo "子 issue 番号が数値でない: ${k}" >&2; exit 1 ;; esac
done

interval="${INTERVAL:-60}"
max_sec="${MAX_SEC:-14400}"
for v in "${interval}" "${max_sec}"; do
  case "${v}" in ''|*[!0-9]*) echo "INTERVAL / MAX_SEC が数値でない: ${v}" >&2; exit 1 ;; esac
done

root=$(cd "$(dirname "$0")/../.." && pwd)
deadline=$(( $(date +%s) + max_sec ))
known=" ${KNOWN_CONFLICTING:-} "
fails=0

while :; do
  # 一覧は上限 1000 件。上限に達したら取りこぼしの恐れがあるので止める（jq の error で取得失敗扱い → 終了 1）
  if prs=$(gh pr list --state open --limit 1000 --json number,closingIssuesReferences,headRefName \
    | jq -r '[$ARGS.positional[]|tonumber] as $k
        | if length >= 1000 then error("open な PR が 1000 件以上") else . end
        | .[]
        | select(any(.closingIssuesReferences[].number; IN($k[]))
            or ((.headRefName | capture("^[a-z]+/(?<n>[0-9]+)-")? | .n | tonumber) as $b | $b | IN($k[])))
        | .number' --args "$@"); then
    fails=0
  else
    fails=$((fails + 1))
    [ "${fails}" -ge 10 ] && { echo "GH_FAILED PR 一覧の取得が ${fails} 回続けて失敗"; exit 1; }
    prs=""
  fi
  hit=0
  failed=""
  for n in ${prs}; do
    out=$(bash "${root}/scripts/pr/guarded-merge.sh" "${n}" 2>&1)
    rc=$?
    if [ "${rc}" -eq 0 ]; then
      echo "READY #${n} $(printf '%s\n' "${out}" | grep '^sha=')"
      hit=1
    # 判定は契約上の状態行（`sha=` で始まる行）だけで行う。出力にはレビュー本文も混ざるため
    elif printf '%s\n' "${out}" | grep '^sha=' | grep -q ' mergeable=CONFLICTING$'; then
      case "${known}" in *" ${n} "*) continue ;; esac
      echo "CONFLICTING #${n}"
      hit=1
    else
      # 未達の PR は現在の head のチェックを見る。pending が残る間は何もしない。取得に失敗したら次の周回へ
      if checks=$(gh pr checks "${n}" --json name,bucket 2>/dev/null || true) \
        && bad=$(printf '%s' "${checks}" | jq -r 'if type == "array" and all(.[]; .bucket != "pending")
            then [.[] | select(.bucket == "fail" or .bucket == "cancel") | .name] | join(",") else "" end') \
        && [ -n "${bad}" ]; then
        failed="${failed}FAILED #${n} ${bad}"$'\n'
      fi
    fi
  done
  [ "${hit}" -eq 1 ] && exit 0
  [ -n "${failed}" ] && { printf '%s' "${failed}"; exit 3; }
  [ "$(date +%s)" -ge "${deadline}" ] && { echo "TIMEOUT ${max_sec} 秒で READY / CONFLICTING なし"; exit 2; }
  sleep "${interval}"
done
