#!/usr/bin/env node
/**
 * 確認用 issue（human-check-issue.mjs が起票したもの）の「## マージ済みの PR」節へ、子 PR 1 件の行を足す。
 *
 * expand-capability の Step 8 から、子 PR をマージするたびに呼ばれる。手で `gh issue edit --body` すると
 * 本文の上書き・空行抜けが起きる（8 周目）ため、追記をここに閉じる。
 *
 * 使い方: node scripts/device/human-check-add-pr.mjs <確認用 issue 番号> <PR 番号> <説明> [--dry-run]
 *   - 足す行は `- #<PR>（<説明>）`。説明に改行は入れられない（節の構造を壊すため exit 2）。同じ PR 番号の行が既にあれば何もしない（何度呼んでも同じ本文になる）
 *   - 節が無ければ本文末尾に作る。起票時の仮置き行「（子 PR のマージ後に書き足す）」は消す
 *
 * 守らない範囲: 同じ issue を複数の手で同時に書き換える競合（後勝ちになる）。
 */
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HEAD = "## マージ済みの PR";
const PLACEHOLDER = "（子 PR のマージ後に書き足す）";

/** 本文へ PR 行を足した結果を返す。既に同じ PR 番号の行があれば本文をそのまま返す。 */
export function withPr(body, pr, desc) {
  const lines = body.replace(/\s+$/, "").split("\n").filter((l) => l.trim() !== PLACEHOLDER);
  let h = lines.indexOf(HEAD);
  if (h < 0) {
    lines.push("", HEAD);
    h = lines.length - 1;
  }
  // 節の終わり = 次の見出しの手前
  let end = lines.findIndex((l, i) => i > h && /^#{1,6} /.test(l));
  if (end < 0) end = lines.length;
  const section = lines.slice(h + 1, end);
  if (section.some((l) => new RegExp(`^- #${pr}(?!\\d)`).test(l))) return body;
  const items = section.filter((l) => l.trim() !== "");
  const rest = lines.slice(end);
  return [...lines.slice(0, h + 1), "", ...items, `- #${pr}（${desc}）`, ...(rest.length ? ["", ...rest] : [])].join("\n") + "\n";
}

function main() {
  const args = process.argv.slice(2);
  const [issue, pr, desc, flag] = args;
  const dry = flag === "--dry-run";
  // 未知のオプション（綴り違いの --dryrun 等）で書き込みモードに倒れないよう、引数は厳密に検証する
  if (!/^\d+$/.test(issue ?? "") || !/^\d+$/.test(pr ?? "") || !desc || /[\r\n]/.test(desc) || args.length > 4 || (flag !== undefined && !dry)) {
    console.error("使い方: node scripts/device/human-check-add-pr.mjs <確認用 issue 番号> <PR 番号> <説明> [--dry-run]");
    process.exit(2);
  }
  const gh = (...a) => execFileSync("gh", a, { encoding: "utf8", stdio: ["pipe", "pipe", "inherit"] });
  const cur = gh("api", `repos/{owner}/{repo}/issues/${issue}`, "--jq", ".body");
  if (!cur.includes("<!-- human-check: ")) throw new Error(`#${issue} は確認用 issue ではない（目印が無い）`);
  const next = withPr(cur, pr, desc);
  if (next === cur) return console.log(`#${issue} には #${pr} の行が既にある`);
  if (dry) return console.log(`[dry-run] #${issue} の本文を次にする\n${next}`);
  execFileSync("gh", ["issue", "edit", issue, "--body-file", "-"], { input: next, stdio: ["pipe", "ignore", "inherit"] });
  console.log(`#${issue} に #${pr} を足した`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
