#!/usr/bin/env node
/**
 * 端末機能 1 件を `fandhe-egui-proxy` / `fandhe-device` から機能 crate `crates/fandhe-cap-<id>` へ移す作業の道具（Issue #1183）。
 * 手順は docs/operations/capability-crate-migration.md。機能 crate の構成の正は docs/architecture/consumer-app-structure.md。
 *
 * 使い方:
 *   node scripts/device/migrate-capability.mjs inventory <id> [--module <stem>]   移す対象と参照を表示する（読み取りのみ）
 *   node scripts/device/migrate-capability.mjs scaffold <id> [--module <stem>] [--dry-run]
 *                                                                                  git mv で機能 crate を作り、機械的に直せる記述を直す
 *   node scripts/device/migrate-capability.mjs sync                                gallery の feature・依存と build.gradle.kts の srcDirs を
 *                                                                                  `crates/fandhe-cap-*` から書き直す（冪等）
 *
 * stem は id の `-` を `_` にしたものが既定。対応表の id と stem が違う機能は --module で渡す。
 * 扱うのは 1 stem だけ。複数 stem の機能・共有ファイルに実装が埋まった古い型は inventory を見て手で移す。
 *
 * 守る範囲: id を正規表現で検証してからパスを組み立てること・git を配列引数で呼びシェルを通さないこと・
 * 既存の crate を上書きしないこと・目印が無い一覧を黙って書き換えないこと。
 * 守らない範囲: 共有ファイル（device.rs・build.rs・MainActivity.java・registry.rs 等）の編集、画面の状態型に
 * 依存するテストの書き換え、JNI を MainActivity のインスタンスメソッドから Helper の static 呼び出しへ直すこと
 * （scaffold が残件として表示する）。
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ID_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const OSES = ["android", "apple", "ios", "macos", "windows"];
const NATIVE_DIRS = ["ios", "macos", "apple"];
const JAVA_DIR = "platforms/android/app/src/main/java/com/fandhe/platform";
const ERROR_VARIANTS = {
  Internal: "内部エラーが発生しました",
  Unsupported: "このプラットフォームでは未対応です",
  PermissionDenied: "権限が拒否されました",
  Cancelled: "操作がキャンセルされました",
  InvalidInput: "入力が不正です",
};
const BEGIN = "BEGIN fandhe-cap";
const END = "END fandhe-cap";

export function checkId(id) {
  if (typeof id !== "string" || !ID_RE.test(id)) throw new Error(`不正な id: ${JSON.stringify(id)}`);
  return id;
}
export const stemOf = (id) => id.replaceAll("-", "_");
export const pascalOf = (s) => s.split(/[-_]/).map((w) => w[0].toUpperCase() + w.slice(1)).join("");

/** stem から、移動元 -> 移動先（リポジトリルートからの相対）の対応を返す。存在確認はしない。 */
export function moveTargets(id, stem) {
  const crate = `crates/fandhe-cap-${id}`;
  const helper = `${pascalOf(stem)}Helper.java`;
  return [
    [`crates/fandhe-device/src/${stem}.rs`, `${crate}/src/contract.rs`],
    [`crates/fandhe-egui-proxy/src/device/${stem}.rs`, `${crate}/src/driver.rs`],
    ...OSES.map((os) => [`crates/fandhe-egui-proxy/src/device/${os}_${stem}.rs`, `${crate}/src/${os}.rs`]),
    [`${JAVA_DIR}/${helper}`, `${crate}/android/java/com/fandhe/platform/${helper}`],
  ];
}

/** 移動した Rust ソースの機械的な書き換え。返すのは書き換え後の本文。 */
export function rewriteSource(src, id) {
  const stem = stemOf(id);
  const err = `${pascalOf(id)}Error`;
  const rules = [
    [/crate::app::\{CompletionSender, Completion\}/g, "fandhe_runtime::{Completion, CompletionSender}"],
    [/crate::app::(CompletionSender|Completion)\b/g, "fandhe_runtime::$1"],
    [/(?:crate::device|super)::request_slot\b/g, "fandhe_runtime::request_slot"],
    [/crate::(?:android|ios)::request_repaint\b/g, "fandhe_runtime::request_repaint"],
    [/crate::android::android_app\b/g, "fandhe_platform_android::android_app"],
    [/(?:super|crate::device)::android::JniCallFailed\b/g, "fandhe_platform_android::JniCallFailed"],
    [new RegExp(`fandhe_device::${stem}::`, "g"), "crate::contract::"],
    [/use fandhe_device::DeviceError;/g, `use crate::error::${err};`],
    [/\bfandhe_device::DeviceError\b/g, `crate::error::${err}`],
    // 引数つきの固定バリアントは単位バリアントへ（文言は error.rs 側が持つ）。
    [new RegExp(`\\bDeviceError::(${Object.keys(ERROR_VARIANTS).join("|")})\\((?:"[^"]*"|_)\\)`, "g"), `${err}::$1`],
    [/\bDeviceError\b/g, err],
  ];
  return rules.reduce((s, [re, to]) => s.replace(re, to), src);
}

/** ソースが使っている DeviceError のバリアント名（重複なし）。 */
export function usedErrorVariants(src) {
  return [...new Set([...src.matchAll(/\bDeviceError::([A-Za-z]+)/g)].map((m) => m[1]))];
}

/** error.rs に写せないバリアント（手作業の対象）。 */
export const unmappedVariants = (vs) => vs.filter((v) => !(v in ERROR_VARIANTS));

export function renderErrorRs(id, variants) {
  const name = `${pascalOf(id)}Error`;
  const vs = variants.filter((v) => v in ERROR_VARIANTS);
  const body = vs
    .map((v) => `    /// \`DeviceError::${v}\` に相当する。\n    #[error("${ERROR_VARIANTS[v]}: ${id}")]\n    ${v},`)
    .join("\n");
  const fixed = vs.map((v) => `            (${name}::${v}, "${ERROR_VARIANTS[v]}: ${id}"),`).join("\n");
  return `//! ${id} の失敗の種類。画面の状態型にも \`fandhe-device\` にも依存しないよう、機能 crate ごとに持つ（Issue #1183）。
//! 表示文言は移行前の \`DeviceError\` と同じ書き出しで、OS の文言は持たない（ログ・\`Debug\` に出さない契約）。

/// ${id} の失敗。使うバリアントだけを持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ${name} {
${body}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_fixed() {
        for (e, expected) in [
${fixed}
        ] {
            assert_eq!(e.to_string(), expected);
        }
    }
}
`;
}

export function renderCargoToml(id, { android, apple, java }) {
  return `[package]
name = "fandhe-cap-${id}"
version = "0.2.0"
edition = "2024"
description = "fandhe-multi-platform の ${id} の機能 crate（契約・OS 別実装・宣言。レンダラ非依存）"
license = "MIT OR Apache-2.0"
repository = "https://github.com/Fandhe-AI/fandhe-multi-platform"
# 公開対象だが、fandhe-core も現在 publish = false のため揃えている。公開設定は #1216 の範囲。
publish = false

[dependencies]
# 画面の状態関数を使う（画面の状態型を参照してよいのは driver.rs だけ）。
fandhe-core = { path = "../fandhe-core" }
# 完了通知の送り口（CompletionSender）・再描画。egui には依存しない。
fandhe-runtime = { path = "../fandhe-runtime" }
thiserror = { workspace = true }
${android ? `\n[target.'cfg(target_os = "android")'.dependencies]\nfandhe-platform-android = { path = "../fandhe-platform-android" }\njni = { workspace = true }\n` : ""}${apple ? `\n[build-dependencies]\n# iOS・macOS の ObjC を build.rs でコンパイルする helper。通常のビルドには cc を持ち込まない。\nfandhe-platform-apple = { path = "../fandhe-platform-apple", features = ["build"] }\n` : ""}
# 機能宣言（capability-declaration.md）。権限・usage description・framework は手で足す（cargo fandhe check が検証する）。
[package.metadata.fandhe]
id = "${id}"
${java ? '\n[package.metadata.fandhe.android]\njava = ["android/java"]\n' : ""}
[lints]
workspace = true
`;
}

/** n 番目の目印（BEGIN/END fandhe-cap）の間を makeLines(インデント) で置き換える。目印が無ければ例外。 */
export function replaceBlock(text, n, makeLines) {
  const ls = text.split("\n");
  let seen = -1;
  for (let b = 0; b < ls.length; b++) {
    if (!ls[b].includes(BEGIN) || ++seen !== n) continue;
    const e = ls.findIndex((l, i) => i > b && l.includes(END));
    if (e < 0) throw new Error(`"${END}" が見つからない`);
    const indent = ls[b].match(/^\s*/)[0];
    return [...ls.slice(0, b + 1), ...makeLines(indent), ...ls.slice(e)].join("\n");
  }
  throw new Error(`${n + 1} 番目の目印 "${BEGIN}" が見つからない`);
}

export const featureLines = (ids) => (ind) => ids.map((id) => `${ind}"dep:fandhe-cap-${id}",`);
export const depLines = (ids) => (ind) =>
  ids.map((id) => `${ind}fandhe-cap-${id} = { path = "../fandhe-cap-${id}", optional = true }`);
export const gradleLines = (caps) => (ind) =>
  caps.flatMap(({ id, java, res }) => [
    ...(java ? [`${ind}@Suppress("DEPRECATION")`, `${ind}java.srcDirs("../../../crates/fandhe-cap-${id}/android/java")`] : []),
    ...(res ? [`${ind}@Suppress("DEPRECATION")`, `${ind}res.srcDirs("../../../crates/fandhe-cap-${id}/android/res")`] : []),
  ]);

// ---- 実体 ----

const git = (R, ...args) => execFileSync("git", args, { cwd: R, encoding: "utf8" });
const read = (R, p) => readFileSync(join(R, p), "utf8");

function listCaps(R) {
  return readdirSync(join(R, "crates"))
    .filter((d) => d.startsWith("fandhe-cap-") && existsSync(join(R, "crates", d, "Cargo.toml")))
    .sort()
    .map((d) => ({
      id: d.slice("fandhe-cap-".length),
      java: existsSync(join(R, "crates", d, "android/java")),
      res: existsSync(join(R, "crates", d, "android/res")),
    }));
}

function sync(R) {
  const caps = listCaps(R);
  const ids = caps.map((c) => c.id);
  const edits = [
    ["crates/fandhe-gallery/Cargo.toml", (t) => replaceBlock(replaceBlock(t, 0, featureLines(ids)), 1, depLines(ids))],
    ["platforms/android/app/build.gradle.kts", (t) => replaceBlock(t, 0, gradleLines(caps))],
  ];
  for (const [p, f] of edits) {
    const before = read(R, p);
    const after = f(before);
    if (after !== before) writeFileSync(join(R, p), after);
    console.log(`${after === before ? "変更なし" : "更新"}: ${p}`);
  }
}

function nativeFiles(R, stem) {
  return NATIVE_DIRS.flatMap((d) => {
    const dir = `crates/fandhe-egui-proxy/${d}`;
    if (!existsSync(join(R, dir))) return [];
    return readdirSync(join(R, dir)).filter((f) => f.startsWith(`fandhe_${d}_${stem}.`)).map((f) => [d, f]);
  });
}

function sharedRefs(R, id, stem) {
  const files = git(R, "ls-files", "crates", "scripts", "docs", "platforms", "Cargo.toml")
    .split("\n")
    .filter((f) => f && !f.startsWith("crates/fandhe-cap-") && !f.startsWith("docs/spec/") && !f.startsWith("scripts/device/migrate-capability"));
  const needles = [stem, pascalOf(stem), id];
  const hits = [];
  for (const f of files) {
    if (!/\.(rs|java|kt|kts|m|h|swift|toml|md|json|sh|mjs|xml|yml|plist)$/.test(f)) continue;
    let t;
    try { t = read(R, f); } catch { continue; }
    t.split("\n").forEach((l, i) => {
      if (needles.some((n) => l.includes(n))) hits.push(`${f}:${i + 1}: ${l.trim().slice(0, 120)}`);
    });
  }
  return hits;
}

function inventory(R, id, stem) {
  const cap = JSON.parse(read(R, `docs/device/capabilities/${id}.json`));
  console.log(`# ${id}（stem=${stem}）`);
  for (const [os, v] of Object.entries(cap.platforms ?? {})) {
    console.log(`  ${os}: ${v.status}`);
    for (const m of String(v.impl ?? "").matchAll(/(?:crates|platforms|scripts)\/[^\s`+、,]+/g))
      console.log(`    impl: ${m[0]} ${existsSync(join(R, m[0])) ? "(実在)" : "(**無い**)"}`);
  }
  if (cap.platforms?.windows?.status === "done" || cap.platforms?.web?.status === "done")
    console.log("注意: Windows・Web の実装がある。#1184・#1185 の後で移す");
  console.log("## 移動先");
  for (const [from, to] of moveTargets(id, stem)) if (existsSync(join(R, from))) console.log(`  ${from} -> ${to}`);
  for (const [d, f] of nativeFiles(R, stem)) console.log(`  crates/fandhe-egui-proxy/${d}/${f} -> crates/fandhe-cap-${id}/${d}/${f}`);
  const helper = pascalOf(stem);
  if (existsSync(join(R, JAVA_DIR)))
    for (const f of readdirSync(join(R, JAVA_DIR)).filter((f) => f.startsWith(helper) && f !== `${helper}Helper.java`))
      console.log(`  候補（自動では動かさない）: ${JAVA_DIR}/${f}`);
  const dev = `crates/fandhe-device/src/${stem}.rs`;
  if (existsSync(join(R, dev))) {
    const src = read(R, dev);
    console.log(`## DeviceError のバリアント: ${usedErrorVariants(src).join(", ") || "なし"}`);
    const dir = "crates/fandhe-device/src";
    const users = readdirSync(join(R, dir)).filter((f) => f.endsWith(".rs") && f !== `${stem}.rs` && read(R, `${dir}/${f}`).includes(`crate::${stem}::`));
    const uses = [...new Set([...src.matchAll(/crate::([a-z_0-9]+)::/g)].map((m) => m[1]))];
    console.log(`## 機能間の共有\n  このモジュールが使う: ${uses.join(", ") || "なし"}\n  このモジュールを使う: ${users.join(", ") || "なし"}`);
  }
  console.log("## 共有ファイル内の参照（手作業で消す・直す）");
  for (const h of sharedRefs(R, id, stem)) console.log(`  ${h}`);
}

function scaffold(R, id, stem, dry) {
  const crate = `crates/fandhe-cap-${id}`;
  if (existsSync(join(R, crate))) throw new Error(`${crate} が既にある`);
  const moves = [
    ...moveTargets(id, stem).filter(([f]) => existsSync(join(R, f))),
    ...nativeFiles(R, stem).map(([d, f]) => [`crates/fandhe-egui-proxy/${d}/${f}`, `${crate}/${d}/${f}`]),
  ];
  if (!moves.some(([, t]) => /\/(contract|driver)\.rs$/.test(t))) throw new Error(`stem=${stem} の契約も driver も見つからない。--module を確認する`);
  for (const [f, t] of moves) console.log(`${dry ? "[dry-run] " : ""}git mv ${f} ${t}`);
  if (dry) return;
  for (const [f, t] of moves) {
    mkdirSync(join(R, dirname(t)), { recursive: true });
    git(R, "mv", f, t);
  }
  const variants = new Set();
  const unsafeOs = new Set();
  for (const [, t] of moves.filter(([, t]) => t.endsWith(".rs"))) {
    const src = read(R, t);
    usedErrorVariants(src).forEach((v) => variants.add(v));
    if (/\bunsafe\b/.test(src.replace(/\/\/.*$/gm, ""))) unsafeOs.add(t.match(/src\/([a-z]+)\.rs$/)?.[1] ?? "android");
    writeFileSync(join(R, t), rewriteSource(src, id));
  }
  const has = (re) => moves.some(([, t]) => re.test(t));
  writeFileSync(join(R, crate, "Cargo.toml"), renderCargoToml(id, { android: has(/\/src\/android\.rs$/), apple: has(/\.m$/), java: has(/\.java$/) }));
  writeFileSync(join(R, crate, "src/error.rs"), renderErrorRs(id, [...variants]));
  if (has(/\.m$/)) {
    const ms = moves.filter(([, t]) => t.endsWith(".m")).map(([, t]) => t.slice(crate.length + 1));
    writeFileSync(
      join(R, crate, "build.rs"),
      `//! ObjC 実装のコンパイル。コンパイルとリンクの手順は \`fandhe-platform-apple\`（feature \`build\`）の helper に集約している。\n//! framework の宣言は \`Cargo.toml\` の \`[package.metadata.fandhe.ios]\` と一致させる。\n\nuse fandhe_platform_apple::build::{compile_objc, target_is_ios};\n\nfn main() {\n    if target_is_ios() {\n${ms.map((m) => `        compile_objc("${m}", &["Foundation", "UIKit"]);`).join("\n")}\n    }\n}\n`,
    );
  }
  // mod の cfg は旧 device.rs の宣言に付いていた属性をそのまま使う。
  const deviceRs = existsSync(join(R, "crates/fandhe-egui-proxy/src/device.rs")) ? read(R, "crates/fandhe-egui-proxy/src/device.rs").split("\n") : [];
  const modLines = [];
  for (const os of OSES.filter((o) => has(new RegExp(`/src/${o}\\.rs$`)))) {
    const i = deviceRs.findIndex((l) => l.trim() === `pub mod ${os}_${stem};`);
    const attrs = [];
    for (let k = i - 1; i >= 0 && k >= 0 && /^\s*(#\[|\/\/\/)/.test(deviceRs[k]); k--) if (deviceRs[k].trim().startsWith("#[")) attrs.unshift(deviceRs[k].trim());
    modLines.push(...(attrs.length ? attrs : [`#[cfg(target_os = "${os}")]`]), `mod ${os};`);
  }
  const osCfg = [...unsafeOs].map((o) => `target_os = "${o}"`).join(", ");
  const unsafeAttr = unsafeOs.size
    ? `// unsafe は OS 実装の FFI・JNI エクスポートに限る（\`// SAFETY:\` を付ける）。該当 OS だけ deny へ緩める。\n#![cfg_attr(not(any(${osCfg})), forbid(unsafe_code))]\n#![cfg_attr(any(${osCfg}), deny(unsafe_code))]`
    : "#![forbid(unsafe_code)]";
  writeFileSync(
    join(R, crate, "src/lib.rs"),
    `//! ${id} の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。TODO: 役割・呼び出し元・守る範囲 / 守らない範囲を書く。\n//!\n//! 契約（\`contract\`）・失敗の種類（\`error\`）・effect ドライバ（\`driver\`）・OS 実装を 1 つに集める。\n//! 画面の状態型を参照してよいのは \`driver\` だけ。入口 \`platform_${stem}()\` は状態型に依存しないため、\n//! フレームワークの外のアプリも使える。\n\n${unsafeAttr}\n\nmod contract;\nmod driver;\nmod error;\n${modLines.join("\n")}\n\npub use contract::*;\npub use driver::*;\npub use error::${pascalOf(id)}Error;\n`,
  );
  sync(R);
  console.log("\n## 残件（手作業）");
  console.log(`  - 未写像の DeviceError バリアント: ${unmappedVariants([...variants]).join(", ") || "なし"}`);
  for (const [, t] of moves.filter(([, t]) => t.endsWith(".rs"))) {
    read(R, t).split("\n").forEach((l, i) => {
      if (/(crate::(?!contract|error)|super::|fandhe_device::|egui)/.test(l)) console.log(`  - ${t}:${i + 1}: ${l.trim().slice(0, 100)}`);
    });
  }
  console.log("  - JNI が MainActivity のインスタンスメソッドを呼ぶなら Helper の static 呼び出し（with_activity_class）へ直す");
  console.log("  - 共有ファイル（device.rs・fandhe-egui-proxy/build.rs・registry.rs・fandhe-device/src/lib.rs・MainActivity.java・対応表の impl）");
  console.log("  - lib.rs の crate doc、宣言の権限・framework、画面の状態型に依存するテスト");
}

function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  try {
    const R = git(process.cwd(), "rev-parse", "--show-toplevel").trim();
    if (cmd === "sync") return sync(R);
    if (cmd !== "inventory" && cmd !== "scaffold") {
      console.error("使い方: migrate-capability.mjs <inventory|scaffold> <id> [--module <stem>] [--dry-run] | sync");
      process.exit(2);
    }
    const id = checkId(rest[0]);
    const mi = rest.indexOf("--module");
    const stem = mi >= 0 ? rest[mi + 1] : stemOf(id);
    if (!/^[a-z0-9_]+$/.test(stem ?? "")) throw new Error(`不正な stem: ${stem}`);
    if (!existsSync(join(R, `docs/device/capabilities/${id}.json`))) throw new Error(`対応表に ${id} が無い`);
    if (cmd === "inventory") inventory(R, id, stem);
    else scaffold(R, id, stem, rest.includes("--dry-run"));
  } catch (e) {
    console.error(`エラー: ${e.message}`);
    process.exit(1);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
