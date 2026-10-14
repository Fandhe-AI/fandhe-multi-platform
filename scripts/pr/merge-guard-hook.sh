#!/bin/bash
# Claude Code の PreToolUse hook（matcher: Bash）。`gh ... pr merge` を含むコマンドを検知し、
# origin/main の scripts/pr/guarded-merge.sh（条件確認のみ・マージはしない）が条件を満たすと
# 判定しない限り実行を deny する（Issue #949）。
#
# 契約:
#   - stdin に hook の JSON（`tool_input.command` を読む）
#   - exit 0 = 許可、exit 2 = deny（stderr の文面が Claude へ返る）
#   - `gh` と `merge` の両方のトークンを含まないコマンドは何もせず許可する（通常の作業を止めない）
#   - 含む場合は、区切り（&&, ||, ;, |, 改行）ごとに `gh [-R <repo>|--repo <repo>|--repo=<repo>] pr merge <番号>
#     ... --match-head-commit <40 桁 SHA>` の形として解析し、解析できなければ deny（fail-closed）。
#     `-R/--repo` がローカルのリポジトリと異なる、`gh` より前の環境変数指定（`GH_REPO=...`）、`cd`、
#     変数展開（`$`）、許可リスト外のオプション（`--admin` 等）も deny
#   - 条件確認は **origin/main にある guarded-merge.sh** で行う。作業ツリーの版を使うと、PR 自身が
#     ガードを緩めて同じ PR で通せてしまう（Cursor Bugbot 指摘 High、PR #952）。本 hook 自身も
#     `.claude/settings.json` 側で origin/main から取り出して実行する
#
# 脅威モデル（2026-09-22 のオーナー判断で確定した設計）:
#   本 hook が守るのは「協調的な agent の手順ミス」（条件未達の PR を誤ってマージする・SHA を固定し忘れる・
#   CODEOWNERS 対象を人間レビューなしに通す）である。**シェル構文の難読化（空の引用符の挿入・
#   `$'\x67h'`・eval 経由等）による意図的な迂回への耐性は対象外**とする。CI・未解決スレッド・mergeable は
#   サーバー側 ruleset が強制しており、hook でしか守れない CODEOWNERS の対象パスは元々人間がマージする。
#   検知は引用符を除いた文字列に対して行い、既知の型（`g""h` / `mer""ge`）は拾うが、それ以上は追わない。
#
# `.claude/settings.json` から参照される。全セッションに効くため、無関係なコマンドは即座に許可する。
set -u

deny() { printf 'merge-guard: %s\n' "$*" >&2; exit 2; }

command -v jq >/dev/null 2>&1 || deny "jq が無いため gh pr merge を許可できません"
input=$(cat)
cmd=$(printf '%s' "${input}" | jq -r '.tool_input.command // empty') || deny "hook 入力の解析に失敗"
# シェルの行継続（`\` + 改行）を空白に畳んでから区切る。`gh pr \` / `merge 123 --admin` のように
# 物理行をまたぐ 1 コマンドを別区切りと誤認して素通りさせない（codex-review 指摘 P1、PR #952）。
folded=$(printf '%s\n' "${cmd}" | sed -e ':a' -e '/\\$/{N; s/\\\n/ /; ba' -e '}')
# 末尾が `\` のまま終わる（畳んだ結果が空になる）等、正規化できない形は deny する
# （POSIX/BSD sed は EOF で `N` に失敗するとパターン空間を捨てるため、空を「対象外」と読むと fail-open。
# Cursor Bugbot 指摘 Medium、PR #952）。
if [ -n "${cmd}" ] && [ -z "${folded}" ]; then deny "行継続を正規化できないため deny"; fi
cmd="${folded}"
# 検知は引用符を除いた文字列に対して行う（`g""h` / `mer""ge` のような空の引用符で検知を外せないように）。
det=$(printf '%s' "${cmd}" | tr -d "\"'")

# 粗い前段検知: `gh` と `merge` のトークンが両方無ければ対象外（グローバルオプション付き
# `gh --repo X pr merge` も拾えるよう、`gh pr merge` の連続には依存しない）。
printf '%s' "${det}" | grep -qE '(^|[^[:alnum:]_-])gh([^[:alnum:]_-]|$)' || exit 0
printf '%s' "${det}" | grep -qE '(^|[^[:alnum:]_-])merge([^[:alnum:]_-]|$)' || exit 0

# 前段検知 2: 同じ区切りに `gh` と `merge` の両方を含む箇所が無ければ対象外
# （`git merge ... && gh pr view` のような通常操作を deny しない）。
has_merge_seg=0
while IFS= read -r seg; do
  printf '%s' "${seg}" | grep -qE '(^|[^[:alnum:]_-])gh([^[:alnum:]_-]|$)' || continue
  printf '%s' "${seg}" | grep -qE '(^|[^[:alnum:]_-])merge([^[:alnum:]_-]|$)' || continue
  has_merge_seg=1
done < <(printf '%s\n' "${det}" | sed -E 's/(&&|\|\||;|\|)/\n/g')
[ "${has_merge_seg}" = "1" ] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null)}"
[ -n "${root}" ] || deny "リポジトリを特定できないため deny"
cd "${root}" || deny "リポジトリへ移動できないため deny"

case "${cmd}" in *'$'*) deny "変数展開を解決できないため deny（PR 番号と SHA はリテラルで書く）" ;; esac
printf '%s' "${cmd}" | grep -qE '(^|[;&|[:space:]])cd([[:space:]]|$)' && deny "cd を含むコマンドは検証先が変わり得るため deny"

local_repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner) || deny "ローカルのリポジトリ特定に失敗"

# origin/main の guarded-merge.sh を取り出す（作業ツリーの版は信用しない）。解析を通った後、
# 実行直前に 1 度だけ呼ぶ。
guard=""
trap '[ -n "${guard}" ] && rm -f "${guard}"' EXIT
load_guard() {
  [ -n "${guard}" ] && return 0
  git fetch -q origin main || deny "origin/main の取得に失敗"
  guard=$(mktemp) || deny "一時ファイルの作成に失敗"
  git show origin/main:scripts/pr/guarded-merge.sh > "${guard}" 2>/dev/null || deny "origin/main に scripts/pr/guarded-merge.sh が無い"
}

unquote() { local t="$1"; t="${t#\"}"; t="${t%\"}"; t="${t#\'}"; t="${t%\'}"; printf '%s' "${t}"; }

# 区切りごとに 1 件として解析する。`gh` と `merge` を含まない区切りは無視する。
while IFS= read -r seg; do
  segdet=$(printf '%s' "${seg}" | tr -d "\"'")
  printf '%s' "${segdet}" | grep -qE '(^|[^[:alnum:]_-])gh([^[:alnum:]_-]|$)' || continue
  # この区切りに merge トークンが無ければ `gh pr view` 等の通常操作なので対象外。
  printf '%s' "${segdet}" | grep -qE '(^|[^[:alnum:]_-])merge([^[:alnum:]_-]|$)' || continue
  read -ra toks <<< "${seg}"
  i=0; n=${#toks[@]}
  # `gh` より前のトークンは環境変数指定（`GH_REPO=other/repo gh ...`）等で検証先を変えられるため、
  # 何かあれば deny する（codex-review 指摘 P1、PR #952）。
  while [ "${i}" -lt "${n}" ] && [ "$(unquote "${toks[$i]}")" != "gh" ]; do
    deny "gh より前にトークン ${toks[$i]} があるため deny（環境変数指定・別コマンドとの連結は不可）"
  done
  [ "${i}" -lt "${n}" ] || deny "gh と merge を含むが gh の位置を特定できないため deny"
  i=$((i + 1))
  repo=""
  # グローバルオプション（-R/--repo のみ許可）→ pr → merge → <番号> の順を要求する。
  while [ "${i}" -lt "${n}" ]; do
    t=$(unquote "${toks[$i]}")
    case "${t}" in
      -R|--repo) i=$((i + 1)); [ "${i}" -lt "${n}" ] || deny "-R の値が無い"; repo=$(unquote "${toks[$i]}"); i=$((i + 1)) ;;
      --repo=*) repo="${t#--repo=}"; i=$((i + 1)) ;;
      pr) break ;;
      *) deny "gh の形を解析できないため deny（許可する形: gh [-R owner/repo] pr merge <番号> ... --match-head-commit <SHA>）" ;;
    esac
  done
  [ "${i}" -lt "${n}" ] && [ "$(unquote "${toks[$i]}")" = "pr" ] || deny "gh の後に pr が無いため deny"
  i=$((i + 1))
  [ "${i}" -lt "${n}" ] || deny "pr の後にサブコマンドが無いため deny"
  sub=$(unquote "${toks[$i]}")
  [ "${sub}" = "merge" ] || deny "gh と merge を含むが gh pr merge の形ではないため deny（意図した操作なら分けて実行する）"
  i=$((i + 1))
  [ "${i}" -lt "${n}" ] || deny "PR 番号が無いため deny"
  num=$(unquote "${toks[$i]}")
  case "${num}" in ''|*[!0-9]*) deny "PR 番号を取り出せないため deny（gh pr merge <番号> の形で書く）" ;; esac
  i=$((i + 1))
  sha=""
  # PR 番号以降は許可リストのオプションだけを受け付け、未知のトークン（`--admin=true`・
  # `-Rother/repo`・`--auto` 等）は deny する（codex-review 指摘 P1・Cursor Bugbot 指摘 High、PR #952）。
  while [ "${i}" -lt "${n}" ]; do
    t=$(unquote "${toks[$i]}")
    case "${t}" in
      --squash|--delete-branch|-d) ;;
      -R|--repo) i=$((i + 1)); [ "${i}" -lt "${n}" ] || deny "-R の値が無い"; repo=$(unquote "${toks[$i]}") ;;
      --repo=*) repo="${t#--repo=}" ;;
      --match-head-commit) i=$((i + 1)); [ "${i}" -lt "${n}" ] || deny "--match-head-commit の値が無い"; sha=$(unquote "${toks[$i]}") ;;
      --match-head-commit=*) sha="${t#--match-head-commit=}" ;;
      *) deny "許可していないオプション ${t} を含むため deny（許可: --squash --delete-branch -d -R/--repo --match-head-commit）" ;;
    esac
    i=$((i + 1))
  done
  printf '%s' "${sha}" | grep -qE '^[0-9a-f]{40}$' || deny "--match-head-commit <40 桁 SHA> が無いため deny"
  if [ -n "${repo}" ] && [ "${repo}" != "${local_repo}" ]; then
    deny "-R ${repo} がローカルのリポジトリ ${local_repo} と異なるため deny（検証先とマージ先を一致させる）"
  fi
  load_guard
  out=$(bash "${guard}" "${num}" 2>&1)
  rc=$?
  checked=$(printf '%s\n' "${out}" | grep -E '^sha=' | tail -1 | sed -n 's/^sha=\([0-9a-f]\{40\}\).*/\1/p')
  if [ "${rc}" != "0" ] || [ -z "${checked}" ]; then
    deny "PR #${num} は条件未達のため deny
${out}"
  fi
  [ "${checked}" = "${sha}" ] || deny "PR #${num} の --match-head-commit（${sha}）が確認した head（${checked}）と一致しないため deny"
done < <(printf '%s\n' "${cmd}" | sed -E 's/(&&|\|\||;|\|)/\n/g')

# `gh` と `merge` を同じ区切りに含む箇所が無ければ、ここまでで何も deny されていない = 対象外。
exit 0
