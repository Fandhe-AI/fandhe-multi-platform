/**
 * Issue #948 の自己検査。checkImplPaths が「実在しない impl パス」と、
 * joinDeclarations / permText が対応表と crate 宣言の結合を検出して非ゼロ終了相当
 * （エラー配列が空でない）になることを確認する。fixture は一時ディレクトリに作り、
 * リポジトリには置かない。
 */
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";
import { checkImplPaths, extractImplPaths, badUrls, joinDeclarations, permText } from "./build-capabilities.mjs";

test("extractImplPaths: 対象拡張子のパスのみ抽出し、関数名・拡張子の取りこぼしをしない", () => {
  const text =
    "`crates/fandhe-egui-proxy/src/ios.rs`（`eframe::wgpu::Backends::METAL` に固定）、" +
    "`docs/architecture/web-accessibility-constraints.md` 参照、Issue #1";
  assert.deepEqual(extractImplPaths(text), [
    "crates/fandhe-egui-proxy/src/ios.rs",
    "docs/architecture/web-accessibility-constraints.md",
  ]);
});

test("checkImplPaths: 実在しないパスを機能 id / OS / パスの形式で検出する", () => {
  const root = mkdtempSync(join(tmpdir(), "cap-impl-"));
  try {
    mkdirSync(join(root, "crates"), { recursive: true });
    writeFileSync(join(root, "crates", "exists.rs"), "");
    const features = [
      {
        id: "example",
        platforms: {
          ios: { impl: "`crates/exists.rs`" },
          android: { impl: "`crates/missing.rs`、Issue #1" },
        },
      },
    ];
    const errors = checkImplPaths(features, root);
    assert.deepEqual(errors, ["example / android / crates/missing.rs"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

const cell = (status) => ({ status });
const feat = (id, status, extra = {}) => ({ id, platforms: { android: cell(status) }, ...extra });

test("joinDeclarations: 正常系（同名・別名・基盤）は違反なし", () => {
  const decls = new Map([["a", {}], ["file-picker", {}]]);
  const fs = [feat("a", "done"), feat("file-picker", "done"), feat("tsv", "done", { crate: "file-picker" }), feat("base", "done", { crate: null }), feat("plan", "possible")];
  assert.deepEqual(joinDeclarations(fs, decls), []);
});

test("joinDeclarations: crate に対応表が無い・存在しない crate・基盤の null 欠落・perm 再流入・自己参照を拒否する", () => {
  const decls = new Map([["a", {}], ["orphan", {}]]);
  const fs = [
    feat("a", "done", { platforms: { android: { status: "done", perm: "x" } } }),
    feat("b", "done", { crate: "nope" }),
    feat("c", "done"),
    feat("d", "done", { crate: 1 }),
    feat("a2", "possible", { crate: "a2" }),
  ];
  const errs = joinDeclarations(fs, decls).join("\n");
  assert.match(errs, /crate orphan に対応する/);
  assert.match(errs, /a\.json: platforms\.android\.perm/);
  assert.match(errs, /b\.json: crate nope が存在しない/);
  assert.match(errs, /c\.json: done のセルがあるのに/);
  assert.match(errs, /d\.json: crate は/);
  assert.match(errs, /a2\.json: crate が自身の id と同じ/);
});

test("permText: 非 done は —、OS ごとに宣言から並べ、空は なし", () => {
  const d = {
    ios: { "usage-descriptions": ["NSCameraUsageDescription"], entitlements: { "com.apple.developer.healthkit": true }, extensions: [{ "app-groups": ["widget"], "usage-descriptions": ["NSCameraUsageDescription"] }] },
    android: { permissions: [{ name: "android.permission.CAMERA" }, { name: "android.permission.READ", "max-sdk": 32 }, { name: "android.permission.SCAN", flags: ["neverForLocation"] }] },
  };
  assert.equal(permText("android", "possible", d), "—");
  assert.equal(permText("android", "done", d), "android.permission.CAMERA、android.permission.READ（maxSdkVersion 32）、android.permission.SCAN（neverForLocation）");
  assert.equal(permText("ios", "done", d), "NSCameraUsageDescription、com.apple.developer.healthkit、App Group: widget");
  assert.equal(permText("web", "done", d), "なし");
  assert.equal(permText("macos", "done", undefined), "なし");
});

test("badUrls: https 以外の url（大文字 scheme・非文字列を含む）だけを返す", () => {
  assert.deepEqual(badUrls({ url: "https://example.com" }), []);
  assert.deepEqual(badUrls({ url: "" }), []);
  assert.deepEqual(badUrls({}), []);
  assert.deepEqual(badUrls(undefined), []); // platforms.<os> 欠落でも例外にしない
  assert.deepEqual(badUrls({ url: ["https://a.example", "javascript:alert(1)", "http://b.example"] }), [
    "javascript:alert(1)",
    "http://b.example",
  ]);
  assert.deepEqual(badUrls({ url: "JAVASCRIPT:alert(1)" }), ["JAVASCRIPT:alert(1)"]);
  assert.deepEqual(badUrls({ url: [1] }), [1]);
});
