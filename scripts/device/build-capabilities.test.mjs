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
import { checkImplPaths, checkAndroidPermissions, extractImplPaths } from "./build-capabilities.mjs";

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

test("checkAndroidPermissions: done かつ perm 記載があるのに Manifest 未宣言なら検出する", () => {
  const root = mkdtempSync(join(tmpdir(), "cap-perm-"));
  try {
    const manifestPath = join(root, "AndroidManifest.xml");
    writeFileSync(
      manifestPath,
      '<manifest><uses-permission android:name="android.permission.INTERNET" /></manifest>',
    );
    const features = [
      { id: "http-get", platforms: { android: { status: "done", perm: "android.permission.INTERNET" } } },
      { id: "camera", platforms: { android: { status: "done", perm: "android.permission.CAMERA" } } },
      // possible（未達成）は対象外
      { id: "future", platforms: { android: { status: "possible", perm: "android.permission.NFC" } } },
      // perm が「なし」等で完全修飾名を含まない場合は対象外
      { id: "clipboard", platforms: { android: { status: "done", perm: "なし" } } },
    ];
    const errors = checkAndroidPermissions(features, manifestPath);
    assert.deepEqual(errors, ["camera / android / android.permission.CAMERA"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("checkAndroidPermissions: コメントアウトは未宣言、属性順が違っても宣言済みと判定する", () => {
  const dir = mkdtempSync(join(tmpdir(), "cap-perm-"));
  try {
    const manifest = join(dir, "AndroidManifest.xml");
    writeFileSync(
      manifest,
      [
        "<manifest>",
        '<!-- <uses-permission android:name="android.permission.CAMERA" /> -->',
        '<uses-permission android:maxSdkVersion="32" android:name="android.permission.INTERNET" />',
        "</manifest>",
      ].join("\n"),
    );
    const features = [
      { id: "a", platforms: { android: { status: "done", perm: "android.permission.CAMERA" } } },
      { id: "b", platforms: { android: { status: "done", perm: "android.permission.INTERNET" } } },
    ];
    assert.deepEqual(checkAndroidPermissions(features, manifest), ["a / android / android.permission.CAMERA"]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
