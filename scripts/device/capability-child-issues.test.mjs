/**
 * capability-child-issues.mjs の自己検査（Issue #1107）。layout 検証と本文生成の純関数だけを確かめる。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { resolveLayout, targetOs, bodyOf, matchExisting, marker, osMarker } from "./capability-child-issues.mjs";

const T = ["ios", "android", "windows"];

test("targetOs: possible かつ approval なしだけを返す", () => {
  assert.deepEqual(
    targetOs({ ios: { status: "possible" }, android: { status: "possible", approval: "x" }, web: { status: "no" } }),
    ["ios"],
  );
});

test("targetOs: deferred 付きも除く", () => {
  assert.deepEqual(targetOs({ ios: { status: "possible" }, android: { status: "possible", deferred: "x" } }), ["ios"]);
});

test("resolveLayout: windows を分けない場合は a が併合する", () => {
  assert.deepEqual(resolveLayout(["a", "b-ios", "b-android"], T).map((x) => x.os), [["windows"], ["ios"], ["android"]]);
  assert.deepEqual(resolveLayout(["a", "b", "windows"], T).map((x) => x.os), [[], ["ios", "android"], ["windows"]]);
});

test("resolveLayout: 漏れ・重複・対象外・先頭が a でないとき止まる", () => {
  assert.throws(() => resolveLayout(["a", "b-ios"], T), /覆っていない: android/);
  assert.throws(() => resolveLayout(["a", "b", "b-ios"], T), /重複/);
  assert.throws(() => resolveLayout(["a", "b-ios", "b-android", "c"], T), /対象外/);
  assert.throws(() => resolveLayout(["b", "a"], T), /a から始める/);
});

test("bodyOf: 目印・dependsOn・層ごとの追加条件が入る", () => {
  const base = { id: "x", parent: 1, check: 2 };
  const a = bodyOf({ ...base, layer: "a", os: ["windows"], dep: null });
  assert.ok(a.startsWith("<!-- capability-child: x/a -->"));
  assert.ok(!a.includes("dependsOn"));
  const b = bodyOf({ ...base, layer: "b-ios", os: ["ios"], dep: "#10" });
  assert.ok(b.includes("dependsOn: #10") && b.includes("iOS 実機確認待ち（#2）"));
});

test("bodyOf: 追加条件は担当 OS の分だけ入る（対象外の macOS を書き換えさせない）", () => {
  const base = { id: "x", parent: 1, check: 2, dep: null };
  const merged = bodyOf({ ...base, layer: "a", os: ["windows", "web"] });
  assert.ok(!merged.includes("Mac 目視待ち"));
  assert.ok(merged.includes("Windows 実機確認待ち（#2）") && merged.includes("wasm32 ビルドが通る"));
  assert.ok(merged.includes("全対象 OS の骨格を先に置く"));
  const mac = bodyOf({ ...base, layer: "a", os: ["macos"] });
  assert.ok(mac.includes("Mac 目視待ち（#2）") && !mac.includes("Windows 実機確認待ち"));
  const none = bodyOf({ ...base, layer: "a", os: [] });
  assert.ok(none.includes("対応表のセルは変えない") && !none.includes("だけを `done` にし"));
});

test("matchExisting: 既存の子の層と担当 OS を今回の layout と照合する", () => {
  const plan = resolveLayout(["a", "c"], ["macos", "web"]);
  const child = (n, layer, os) => ({ number: n, body: `${marker("x", layer)}\n${osMarker(os)}\n` });
  assert.deepEqual(matchExisting("x", [child(10, "a", ["macos"]), child(11, "c", ["web"]), { number: 9, body: "親" }], plan), { a: 10, c: 11 });
  // layout を a だけに変えて再実行: c が plan に無い
  assert.throws(() => matchExisting("x", [child(11, "c", ["web"])], resolveLayout(["a"], ["macos", "web"])), /layout に無い/);
  // 同じ層でも担当 OS が違う（a が macOS だけだった後に Web を併合した）
  assert.throws(() => matchExisting("x", [child(10, "a", ["macos"])], resolveLayout(["a"], ["macos", "web"])), /担当 OS が今回の layout と違う/);
  assert.throws(() => matchExisting("x", [child(10, "a", ["macos"]), child(12, "a", ["macos"])], plan), /重複/);
});
