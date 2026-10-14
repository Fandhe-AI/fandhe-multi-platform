#!/usr/bin/env node
/**
 * 端末機能 1 件の「人手の確認」issue を 1 回だけ起票して番号を出す（Issue #1107）。
 *
 * expand-capability の Step 7 から呼ばれる。確認用 issue は親を持たず機能 issue の子にもしない
 * （verify-close が確認を待って機能のクローズを止めないため）。`device-check-pending` ラベルと本文先頭の
 * 目印でまとめ、一覧は `gh issue list --label device-check-pending --state open` で取る。
 * 何度呼んでも同じ番号を返す。
 *
 * 使い方: node scripts/device/human-check-issue.mjs <id> <本文ファイル> [--dry-run]
 *   - 本文ファイルはチェックリスト（Mac 目視・キャプチャ・各 OS の実機）。目印はこのスクリプトが先頭に足す
 *   - --dry-run は読み取りだけ行い、書き込み（起票・追記）の内容を表示して終わる
 *
 * gh の失敗は例外で止める（取得失敗を「該当なし」と取り違えて余計な起票をしないため）。
 *
 * 守る範囲: 1 機能につき open な確認用 issue は 1 件・本文ファイルのチェック項目はすべて issue に載る
 * （既存のチェック状態は残し、足りない項目だけ足す）。
 * 守らない範囲: 同じ機能を複数の手で同時に起票する競合・人が本文を手で書き換えた後の整合
 * （重複は検出して止め、直すのは人）・チェック項目の削除や並べ替え。
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const LIMIT = 500; // 検索の取得上限。到達したら取りこぼしの恐れがあるので止める
const isItem = (l) => /^\s*- \[[ xX]\]/.test(l);
const item = (l) => l.replace(/^\s*- \[[ xX]\]\s*/, "").trim();

/** want のチェック項目のうち cur に無いものを、未チェックの行にして返す。照合はチェック欄を除いた文言で行う。 */
export function missingItems(cur, want) {
  const have = new Set(cur.split("\n").filter(isItem).map(item));
  return want.split("\n").filter((l) => isItem(l) && !have.has(item(l))).map((l) => l.replace(/\[[xX]\]/, "[ ]"));
}

/** gh を実行して stdout を返す。非ゼロ終了は例外になる。 */
const gh = (...args) => execFileSync("gh", args, { encoding: "utf8", stdio: ["pipe", "pipe", "inherit"] }).trim();
const ghIn = (input, ...args) => execFileSync("gh", args, { encoding: "utf8", input, stdio: ["pipe", "pipe", "inherit"] }).trim();

function main() {
  const args = process.argv.slice(2);
  const [id, bodyFile, flag] = args;
  const dry = flag === "--dry-run";
  // 未知のオプション（綴り違いの --dryrun 等）で書き込みモードに倒れないよう、引数は厳密に検証する
  if (!/^[a-z0-9-]+$/.test(id ?? "") || !bodyFile || args.length > 3 || (flag !== undefined && !dry)) {
    console.error("使い方: node scripts/device/human-check-issue.mjs <id> <本文ファイル> [--dry-run]");
    process.exit(2);
  }
  const MARK = `<!-- human-check: ${id} -->`;
  const NOTE = "確認が済んだら対応表の `implStatus` を書き直す PR を出し、全 OS 済みで閉じる。一覧は `device-check-pending` ラベルの open issue。";
  const name = JSON.parse(readFileSync(new URL(`../../docs/device/capabilities/${id}.json`, import.meta.url), "utf8")).name;
  const write = (desc, fn) => (dry ? (console.log(`[dry-run] ${desc}`), 0) : fn());

  // 使い回すのは open な確認用 issue だけ。閉じた後に対象 OS が増えた（申請待ちが外れた等）ときは、
  // 古い issue を開け直さず新しい確認用 issue を起票する（閉じた issue の番号を返して確認済み扱いにしない）
  const hits = JSON.parse(gh("issue", "list", "--state", "open", "--search", `"人手の確認（${id}）" in:title`,
    "--limit", String(LIMIT), "--json", "number,body"));
  if (hits.length >= LIMIT) throw new Error(`検索結果が上限 ${LIMIT} 件に達した。取りこぼしの恐れがあるので止める`);
  const found = hits.filter((i) => (i.body ?? "").includes(MARK)).map((i) => i.number);
  if (found.length > 1) throw new Error(`確認用 issue が重複している: #${found.join(", #")}。人間が 1 件に整理する`);

  let n = found[0];
  if (n) {
    // あとから対象になった OS の項目を足す。既存のチェック状態は残す
    const cur = gh("api", `repos/{owner}/{repo}/issues/${n}`, "--jq", ".body");
    const add = missingItems(cur, readFileSync(bodyFile, "utf8"));
    if (add.length) {
      write(`#${n} に確認項目を ${add.length} 件足す`, () =>
        ghIn(`${cur.trimEnd()}\n${add.join("\n")}\n`, "issue", "edit", String(n), "--body-file", "-"));
    }
  } else {
    const body = `${MARK}\n${NOTE}\n\n${readFileSync(bodyFile, "utf8")}`;
    n = write(`確認用 issue「${name} の人手の確認（${id}）」を起票する`, () => {
      const url = ghIn(body, "issue", "create", "--label", "device-check-pending",
        "--title", `chore(device): ${name} の人手の確認（${id}）`, "--body-file", "-");
      const num = Number(url.match(/\/(\d+)$/)?.[1]);
      if (!num) throw new Error(`起票結果から番号を取れない: ${url}`);
      return num;
    });
  }
  console.log(n);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
