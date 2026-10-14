#!/bin/bash
# PR のマージ条件を確認する（expand-capability Step 8 の SSOT。Issue #949）。
# **このスクリプトはマージを実行しない。** 条件を満たすと、実行すべき `gh pr merge` コマンドを
# 最終行に出力する。実行は必ずそのコマンドを直接打つ。`.claude/settings.json` の PreToolUse hook
# （scripts/pr/merge-guard-hook.sh）がそれを検知し、**origin/main にある本スクリプト**で再検証してから
# 通す。スクリプト自身が `gh pr merge` を呼ぶ設計にすると、作業ツリーで書き換えた版の内部呼び出しに
# hook が掛からず素通りできてしまう（codex-review 指摘 P1、PR #952）。
#
# 使い方:
#   scripts/pr/guarded-merge.sh <PR 番号>
#   （`DRY_RUN=1` は後方互換のため受け付けるが挙動は同じ）
#
# 確認する条件（すべて同一の head SHA に束縛する）:
#   1. CI にブロッカーなし（bucket が fail / pending / cancel の check が 0 件。skipping は数えない）
#   2. 未解決レビュースレッド 0（GraphQL を hasNextPage が false になるまで全ページ取得）
#   3. CODEOWNERS 対象パスの変更 0（CODEOWNERS は PR 側ではなく base 側のコミットから読む）
#   4. mergeable == MERGEABLE（main との競合なし）
#   5. 確認中に head が動いていない（動いていたら確認をやり直す）
#
# 取得・解析に失敗した場合はすべて「条件未達」へ倒す（fail-closed）。値が取れないことを
# 「0 件」と読み替えると、サーバー側で強制されていない CODEOWNERS ガードが静かに無効化されるため。
# 標準出力の `sha=... checks=... unresolved=... codeowners=... mergeable=...` 行は hook が読む契約。
# 形式を変えるときは hook も直す。終了コード: 0 = 条件を満たす、それ以外 = 満たさない / 取得失敗。
set -u

N="${1:-}"
case "${N}" in
  ''|*[!0-9]*) echo "使い方: $0 <PR 番号>" >&2; exit 1 ;;
esac

R=$(gh repo view --json nameWithOwner --jq .nameWithOwner) || { echo "リポジトリの特定に失敗。マージしない"; exit 1; }
OWNER="${R%%/*}"
NAME="${R##*/}"

git fetch -q origin main || { echo "origin/main の取得に失敗。マージしない"; exit 1; }
base=$(git rev-parse origin/main) || { echo "base SHA 取得に失敗。マージしない"; exit 1; }
sha=$(gh pr view "${N}" -R "${R}" --json headRefOid --jq .headRefOid) \
  || { echo "head SHA 取得に失敗。マージしない"; exit 1; }
git fetch -q origin "${sha}" 2>/dev/null || git fetch -q origin "+refs/pull/${N}/head:refs/remotes/pr/${N}" \
  || { echo "head の取得に失敗。マージしない"; exit 1; }

# `--json` 形式は fail や pending があっても終了コード 0 を返すため、`||` は取得失敗だけを捕まえる。
checks=$(gh pr checks "${N}" -R "${R}" --json bucket \
  --jq '[.[]|select(.bucket=="fail" or .bucket=="pending" or .bucket=="cancel")]|length') \
  || { echo "checks 取得に失敗。マージしない"; exit 1; }

mb=$(git merge-base "${base}" "${sha}") || { echo "merge-base の算出に失敗。マージしない"; exit 1; }
codeowners=$(git show "${base}:.github/CODEOWNERS") \
  || { echo "base の CODEOWNERS 取得に失敗。人間レビュー必須として扱いマージしない"; exit 1; }

# CODEOWNERS の内容は heredoc ではなく引数で渡す（heredoc は python 本体に使う）。
# 変更ファイル一覧は `git diff -z --no-renames` を python 側で直接読む（`--name-only` の
# 非 ASCII パスの引用符付き出力と、リネーム時の移動元欠落を避ける）。
owned=$(python3 - "${codeowners}" "${mb}" "${sha}" <<'PY'
import re, subprocess, sys
owners, unknown = [], []
for raw in sys.argv[1].splitlines():
    line = raw.split('#', 1)[0].strip()
    if not line:
        continue
    pat = line.split()[0]
    # 解釈するのは「先頭 / = ルート起点」「末尾 / = ディレクトリ配下」の 2 記法だけ。
    # glob・否定などが現れたら fail-closed（人間レビュー必須と判断する）。
    if not pat.startswith('/') or re.search(r'[*?\[\]!]', pat):
        unknown.append(pat)
        continue
    owners.append(pat)
if unknown:
    print(f'UNPARSEABLE {unknown}', file=sys.stderr)
    sys.exit(2)
if not owners:
    print('CODEOWNERS にパスが 1 件も無い', file=sys.stderr)
    sys.exit(2)
out = subprocess.run(['git', 'diff', '-z', '--no-renames', '--name-only', sys.argv[2], sys.argv[3]],
                     capture_output=True, check=True).stdout
files = [f.decode('utf-8', 'surrogateescape') for f in out.split(b'\0') if f]
hits = [f for f in files for p in owners
        if (p.endswith('/') and f.startswith(p[1:])) or (not p.endswith('/') and f == p[1:])]
print(len(hits))
PY
) || { echo "CODEOWNERS の解釈に失敗。人間レビュー必須として扱いマージしない"; exit 1; }

# 未解決レビュースレッドを全ページ数える。JSON は echo ではなく printf '%s' で渡す
# （本文中の `\n` を echo が展開して JSON を壊すため）。
unresolved=0
cursor=""
while :; do
  # bash 3.2（macOS 標準）+ set -u では空配列の展開が unbound variable になるため、
  # 展開側で ${cargs[@]+"${cargs[@]}"} の形を使う。
  if [ -n "${cursor}" ]; then cargs=(-F c="${cursor}"); else cargs=(); fi
  page=$(gh api graphql -f query='
    query($o:String!,$r:String!,$n:Int!,$c:String){repository(owner:$o,name:$r){pullRequest(number:$n){
      reviewThreads(first:100, after:$c){
        nodes{id isResolved path line comments(first:1){nodes{author{login} body}}}
        pageInfo{hasNextPage endCursor}}}}}' \
    -F o="${OWNER}" -F r="${NAME}" -F n="${N}" ${cargs[@]+"${cargs[@]}"}) || { echo "スレッド取得に失敗。マージしない"; exit 1; }
  cnt=$(printf '%s' "${page}" | jq '[.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false)]|length') \
    || { echo "スレッド解析に失敗。マージしない"; exit 1; }
  printf '%s' "${page}" | jq -r '.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false)
    |"UNRESOLVED \(.id) \(.path):\(.line) \(.comments.nodes[0].author.login): \(.comments.nodes[0].body[0:300])"'
  unresolved=$((unresolved + cnt))
  info=$(printf '%s' "${page}" | jq -r '.data.repository.pullRequest.reviewThreads.pageInfo | .hasNextPage, (.endCursor // "")') \
    || { echo "pageInfo の解析に失敗。マージしない"; exit 1; }
  has=$(printf '%s\n' "${info}" | sed -n 1p)
  cursor=$(printf '%s\n' "${info}" | sed -n 2p)
  case "${has}" in
    true) ;;
    false) break ;;
    *) echo "pageInfo.hasNextPage が想定外: ${has}。マージしない"; exit 1 ;;
  esac
done

mergeable=$(gh pr view "${N}" -R "${R}" --json mergeable --jq .mergeable) \
  || { echo "mergeable 取得に失敗。マージしない"; exit 1; }

# 確認の途中で push されていないか。SHA が動いていたら確認をやり直す。
now=$(gh pr view "${N}" -R "${R}" --json headRefOid --jq .headRefOid) \
  || { echo "head SHA 再取得に失敗。マージしない"; exit 1; }
if [ "${sha}" != "${now}" ]; then
  echo "確認中に head が ${sha} -> ${now} へ動いた。確認をやり直す"
  exit 1
fi

echo "sha=${sha} checks=${checks} unresolved=${unresolved} codeowners=${owned} mergeable=${mergeable}"
if [ "${checks}" = "0" ] && [ "${unresolved}" = "0" ] && [ "${owned}" = "0" ] && [ "${mergeable}" = "MERGEABLE" ]; then
  echo "条件を満たす。次を実行する（hook が origin/main の本スクリプトで再検証する）:"
  echo "gh pr merge ${N} -R ${R} --squash --delete-branch --match-head-commit ${sha}"
  exit 0
fi
echo "条件未達（マージしない）"
exit 3
