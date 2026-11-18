/**
 * Issue #948 の自己検査。checkImplPaths / checkAndroidPermissions が
 * 「実在しない impl パス」「宣言のない Android 権限」を検出して非ゼロ終了相当
 * （エラー配列が空でない）になることを確認する。fixture は一時ディレクトリに作り、
 * リポジトリには置かない。
 */
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";
import { checkImplPaths, checkAndroidPermissions, declaredAndroidPermissions, extractImplPaths, badUrls } from "./build-capabilities.mjs";

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

test("checkAndroidPermissions: done かつ perm 記載があるのに宣言に無ければ検出する", () => {
  const declared = new Set(["android.permission.INTERNET"]);
  const features = [
    { id: "http-get", platforms: { android: { status: "done", perm: "android.permission.INTERNET" } } },
    { id: "camera", platforms: { android: { status: "done", perm: "android.permission.CAMERA" } } },
    // possible（未達成）は対象外
    { id: "future", platforms: { android: { status: "possible", perm: "android.permission.NFC" } } },
    // perm が「なし」等で完全修飾名を含まない場合は対象外
    { id: "clipboard", platforms: { android: { status: "done", perm: "なし" } } },
  ];
  assert.deepEqual(checkAndroidPermissions(features, declared), ["camera / android / android.permission.CAMERA"]);
});

test("declaredAndroidPermissions: crate の宣言から集め、TOML のコメントは除く", () => {
  const root = mkdtempSync(join(tmpdir(), "cap-decl-"));
  try {
    mkdirSync(join(root, "fandhe-cap-a"));
    writeFileSync(
      join(root, "fandhe-cap-a", "Cargo.toml"),
      [
        "[package.metadata.fandhe.android]",
        '# permissions = [{ name = "android.permission.CAMERA" }]',
        'permissions = [{ name = "android.permission.INTERNET", max-sdk = 32 }]',
      ].join("\n"),
    );
    mkdirSync(join(root, "no-manifest"));
    assert.deepEqual([...declaredAndroidPermissions(root)], ["android.permission.INTERNET"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
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
