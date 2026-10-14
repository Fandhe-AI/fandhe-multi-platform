/**
 * human-check-add-pr.mjs の自己検査。本文を組み立てる純関数 withPr だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { withPr } from "./human-check-add-pr.mjs";

const BASE = "<!-- human-check: x -->\n## 確認すること\n\n- [ ] Mac 目視\n\n## マージ済みの PR\n\n（子 PR のマージ後に書き足す）\n";

test("仮置き行を消し、見出しとの間に空行を置いて足す", () => {
  assert.equal(
    withPr(BASE, 12, "(a)"),
    "<!-- human-check: x -->\n## 確認すること\n\n- [ ] Mac 目視\n\n## マージ済みの PR\n\n- #12（(a)）\n",
  );
});

test("2 件目は末尾に続け、同じ PR は二重に足さない", () => {
  const one = withPr(BASE, 12, "(a)");
  const two = withPr(one, 13, "(b)");
  assert.match(two, /- #12（\(a\)）\n- #13（\(b\)）\n$/);
  assert.equal(withPr(two, 13, "別の説明"), two);
  // #1 は #12 の前方一致と取り違えない
  assert.match(withPr(two, 1, "c"), /- #1（c）\n$/);
});

test("節が無ければ作り、後ろの節は残す", () => {
  assert.equal(withPr("本文\n", 5, "d"), "本文\n\n## マージ済みの PR\n\n- #5（d）\n");
  const mid = "## マージ済みの PR\n\n- #1（a）\n\n## メモ\n\n残す\n";
  assert.equal(withPr(mid, 2, "b"), "## マージ済みの PR\n\n- #1（a）\n- #2（b）\n\n## メモ\n\n残す\n");
});
