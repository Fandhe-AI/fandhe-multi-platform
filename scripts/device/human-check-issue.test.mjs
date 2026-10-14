/**
 * human-check-issue.mjs の自己検査。チェック項目の差分計算 missingItems だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { missingItems } from "./human-check-issue.mjs";

test("既にある項目は、チェック済みでも足さない", () => {
  assert.deepEqual(missingItems("- [x] Mac 目視\n- [ ] iOS", "- [ ] Mac 目視\n- [ ] iOS"), []);
});

test("足りない項目だけを未チェックで返す", () => {
  assert.deepEqual(missingItems("- [x] Mac 目視", "- [ ] Mac 目視\n- [x] Android\n説明文"), ["- [ ] Android"]);
});

test("大文字の [X] も既存として照合し、足す行は未チェックに直す", () => {
  assert.deepEqual(missingItems("- [X] Mac 目視", "- [X] Mac 目視\n- [X] iOS"), ["- [ ] iOS"]);
});
