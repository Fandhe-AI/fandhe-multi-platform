/**
 * Issue #852: 端末機能 対応表のジェネレータ。
 *
 * SSOT は docs/device/capabilities/ 配下のみで、本スクリプトが
 * capabilities.md（ピボット表）と capabilities.html（棚に置くビューア）を生成する。
 * 生成物は git に入れない（.gitignore）。各機能の PR が生成物まで書き換えると、並列の PR どうしが
 * 必ず競合するため。見るときは手元で生成する。CI（.github/workflows/ci.yml の capabilities ジョブ）は
 * --check で SSOT の検査と生成が通ることだけを確かめる。
 *
 * 1 機能 = 1 ファイル（docs/device/capabilities/<id>.json）で、共通定義は _meta.json に置く。
 * 1 機能を足すときに触るのがその 1 ファイルだけで済むようにするのが本構成の目的
 * （旧構成は 5 ファイル手同期で、1 機能の追加に全ファイルの編集を要した）。
 * 単一ファイルにまとめないのは、拡張ループを並列で回すと同じファイルへの書き込みが
 * 毎回コンフリクトするため。
 *
 * 使い方: node scripts/device/build-capabilities.mjs [--check]
 *   --check を付けると書き込まず、スキーマ・url の scheme（https のみ）・実装パス・Android 権限宣言を検査し、生成が例外なく通るかを
 *   確かめる（終了コードで通知）。
 */
import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const dir = join(repoRoot, "docs", "device");
const srcDir = join(dir, "capabilities");

/**
 * SSOT の読み込み。`_meta.json`（共通定義）+ 1 機能 1 ファイルを束ねて 1 つの data に戻す。
 *
 * 1 機能 1 ファイルにしているのは、拡張ループを並列（implement-issue-tree の parallel）で
 * 回したときに複数の実装が同じファイルを書いてコンフリクトするのを構造的に避けるため。
 */
function load() {
  const meta = JSON.parse(readFileSync(join(srcDir, "_meta.json"), "utf8"));
  // `_` 始まりは共通定義。機能ファイルはファイル名ソートで読み、環境による揺れを出さない
  // （生成物が揺れると鮮度ゲートが誤検出する）。
  const names = readdirSync(srcDir).filter((n) => n.endsWith(".json") && !n.startsWith("_")).sort();
  const features = names.map((name) => ({ name, body: JSON.parse(readFileSync(join(srcDir, name), "utf8")) }));
  validate(features, meta);
  // order / priority / bundle は計画用メタデータなので、本体へ混ぜずに落とす
  // （HTML へ埋め込む JSON にも出さない）。order の同値は上のファイル名ソートで決まる。
  const sorted = features.map((f) => f.body).sort((a, b) => a.order - b.order || (a.id < b.id ? -1 : 1));
  for (const f of sorted) for (const k of ["order", "priority", "bundle"]) delete f[k];
  return { ...meta, features: sorted };
}

/**
 * スキーマ検証。1 機能 1 ファイルではズレが入りやすいため、ここを品質ゲートにする。
 * id の重複はファイル名 == id の検査が含意する（同名ファイルは存在し得ない）。
 */
function validate(features, meta) {
  const errors = [];
  const allowed = new Set(Object.keys(meta.statusLabels));
  const bundles = new Set(Object.keys(meta.bundleLabels));
  for (const { name, body } of features) {
    const at = `docs/device/capabilities/${name}`;
    for (const key of ["id", "name", "group"]) {
      if (typeof body[key] !== "string" || !body[key]) errors.push(`${at}: ${key} が無い`);
    }
    if (!Number.isInteger(body.order)) errors.push(`${at}: order が整数でない`);
    // priority = 着手順（expand-capability が昇順に消化する）。任意だが、あるなら正の整数。
    // 無い機能は「後回し判断済み」を意味するため、型を緩めると候補外の機能が拾われる。
    if (body.priority !== undefined && (!Number.isInteger(body.priority) || body.priority < 1)) {
      errors.push(`${at}: priority が正の整数でない (${body.priority})`);
    }
    // bundle = 橋渡し経路の束。並列度の判定に使うため、綴り違いを弾く。
    if (body.bundle !== undefined && !bundles.has(body.bundle)) {
      errors.push(`${at}: bundle が不正 (${body.bundle})`);
    }
    if (body.id !== name.slice(0, -5)) errors.push(`${at}: id (${body.id}) がファイル名と一致しない`);
    // capture は任意フィールド。無い機能があっても通すが、書くなら実在する png を指させる
    // （リンク切れのサムネイルを棚用 HTML へ出さないため）。
    if (body.capture !== undefined) {
      if (body.capture !== `captures/${body.id}.png`) {
        errors.push(`${at}: capture は "captures/${body.id}.png" である必要がある (${body.capture})`);
      } else if (!existsSync(join(dir, body.capture))) {
        errors.push(`${at}: capture の指す ${body.capture} が存在しない`);
      }
    }
    for (const os of meta.platforms) {
      const p = body.platforms?.[os];
      if (!p) {
        errors.push(`${at}: platforms.${os} が無い`);
      } else if (!allowed.has(p.status)) {
        errors.push(`${at}: platforms.${os}.status が不正 (${p.status})`);
      } else if (p.approval !== undefined && (typeof p.approval !== "string" || !p.approval || p.status !== "possible")) {
        // approval = OS ベンダーへの申請・承認が要るセル。expand-capability はこれを候補から外すため、
        // 「できそう」以外に付いていると意味が通らない。
        errors.push(`${at}: platforms.${os}.approval は status=possible のセルにのみ文字列で書く`);
      }
      for (const u of badUrls(p)) {
        errors.push(`${at}: platforms.${os}.url は https:// で始まる必要がある (${JSON.stringify(u)})`);
      }
    }
  }
  // excluded = 一覧に載せない機能の区分。無ければ空扱い、あれば各フィールドが文字列。
  const isStr = (v) => typeof v === "string" && v !== "";
  (meta.excluded ?? []).forEach((c, i) => {
    const at = `_meta.json: excluded[${i}]`;
    if (!isStr(c.reason) || !isStr(c.detail) || !Array.isArray(c.items)) errors.push(`${at}: reason / detail / items が不正`);
    (Array.isArray(c.items) ? c.items : []).forEach((it, j) => {
      for (const k of ["name", "api", "why"]) if (!isStr(it?.[k])) errors.push(`${at}.items[${j}]: ${k} が文字列でない`);
    });
  });
  if (errors.length) {
    for (const e of errors) console.error(`[schema] ${e}`);
    process.exit(1);
  }
}

// --- --check 専用の追加検査（Issue #948） ----------------------------------
// スキーマ検証（validate）は「型が正しいか」までで、impl・perm の自由文が指す実体
// （ファイル・Android 権限宣言）までは見ない。ここは通常生成時には走らせず、
// --check でのみ実行する。

// impl 自由文からリポジトリ相対パスのトークンを抜き出す正規表現。
// crates/ platforms/ scripts/ docs/ benchmarks/ で始まり対象拡張子で終わる並びのみを拾う。
// 文字クラスに `:`（関数名の `::`）や `（` `)` を含めないため、関数名や補足の丸括弧は
// 自然に対象外になる。
// 拡張子側に `\b` を付けるのは、例えば "voiceover.md" の "m" が単独の拡張子候補
// （Objective-C の .m）とも一致してしまい、貪欲マッチのバックトラックで "d" を
// 取りこぼす（`....md` → `....m` に化ける）誤マッチを防ぐため。
const IMPL_PATH_RE =
  /(?:crates|platforms|scripts|docs|benchmarks)\/[\w./-]+\.(?:rs|m|java|kt|swift|mjs|js|ts|html|md|toml|yml|yaml|sh|ps1)\b/g;

/** impl 自由文からパス候補を列挙する（1 つの impl に複数パスが並ぶ場合がある）。 */
function extractImplPaths(text) {
  return [...String(text ?? "").matchAll(IMPL_PATH_RE)].map((m) => m[0]);
}

/**
 * impl パスの実在検証。実装移動・削除に伴う対応表の更新漏れを検出する。
 * 戻り値は「機能 id / OS / パス」の文字列配列（空なら異常なし）。
 */
function checkImplPaths(features, root) {
  const errors = [];
  for (const f of features) {
    for (const os of Object.keys(f.platforms)) {
      for (const p of extractImplPaths(f.platforms[os].impl)) {
        if (!existsSync(join(root, p))) errors.push(`${f.id} / ${os} / ${p}`);
      }
    }
  }
  return errors;
}

/**
 * Android 権限宣言の検証。status=done かつ perm 欄に `android.permission.X` を
 * 含む機能について、AndroidManifest.xml 側に対応する `<uses-permission>` があるかを見る。
 * perm が短縮形（例: `USE_BIOMETRIC`）のみで完全修飾名を含まない機能は対象外
 * （Issue #948 の検査対象は perm 欄が完全修飾名を書いている場合に限る）。
 */
function checkAndroidPermissions(features, manifestPath) {
  let manifest = "";
  try {
    manifest = readFileSync(manifestPath, "utf8");
  } catch {
    manifest = "";
  }
  // XML コメントを除いてから `<uses-permission ...>` 要素を拾い、属性順に依らず
  // `android:name` の値を集める（コメントアウトされた宣言を「宣言済み」と誤判定しない、
  // `android:maxSdkVersion` が先に来る宣言を見落とさない。codex-review 指摘 P2、PR #951）。
  const declared = new Set();
  const stripped = manifest.replace(/<!--[\s\S]*?-->/g, "");
  for (const el of stripped.matchAll(/<uses-permission(?:-sdk-23)?\b([^>]*)>/g)) {
    const name = /android:name\s*=\s*"([^"]+)"/.exec(el[1]);
    if (name) declared.add(name[1]);
  }
  const errors = [];
  for (const f of features) {
    const a = f.platforms.android;
    if (!a || a.status !== "done" || !a.perm) continue;
    const perms = [...String(a.perm).matchAll(/android\.permission\.[A-Z_]+/g)].map((m) => m[0]);
    for (const perm of perms) {
      if (!declared.has(perm)) errors.push(`${f.id} / android / ${perm}`);
    }
  }
  return errors;
}

const data = load();

const { platforms, platformLabels, statusLabels, statusDefinitions, verifiedDefinitions, features, notes } = data;
const excluded = data.excluded ?? [];
const excludedCount = excluded.reduce((n, c) => n + c.items.length, 0);

/** 状態 → バッジの CSS クラス。凡例（生成時）とセル描画（ブラウザ側）で同じ対応表を使う。 */
const APPROVAL_MARK = "🔒申請待ち";
const STATUS_CLASS = { done: "s-done", possible: "s-possible", no: "s-no", "n-a": "s-na", unrated: "s-unrated" };
const GENERATED = "このファイルは生成物です。編集は `docs/device/capabilities/<id>.json`（共通定義は `_meta.json`）に対して行い、`node scripts/device/build-capabilities.mjs` で再生成してください。キャプチャは画像を埋め込まず相対参照するため、配布時は `captures/` ディレクトリを添えてください。";

/**
 * 生成 HTML の href と Markdown の自動リンクへそのまま出す url のうち、https:// で始まらないものを返す。
 * validate() から呼ばれ、javascript: 等の実行系 scheme を生成の入口で止める（許可リスト方式）。
 */
function badUrls(p) {
  return urls(p).filter((u) => typeof u !== "string" || !u.startsWith("https://"));
}

/** url は単一ソースなら文字列、複数ソース行が 1 機能へ統合された場合は配列になる。 */
function urls(p) {
  if (!p?.url) return [];
  return Array.isArray(p.url) ? p.url : [p.url];
}

/** 表示用の状態ラベル。detail（元セルの原文）があればそちらを優先して情報を落とさない。 */
function cellText(p) {
  const t = p.detail ?? statusLabels[p.status] ?? p.status;
  return p.approval ? `${t}（${APPROVAL_MARK}）` : t;
}

/** 申請待ちのセル（approval あり）の一覧。申請できる状態になったときの見直し対象。 */
const approvals = features.flatMap((f) =>
  platforms.filter((o) => f.platforms[o].approval).map((o) => ({ f, os: o, text: f.platforms[o].approval })));

/** 「できている」の集計。分母は対象外・未評価を除いた、その OS で評価済みの機能数。 */
function tally(os) {
  let done = 0;
  let rated = 0;
  for (const f of features) {
    const s = f.platforms[os].status;
    if (s === "n-a" || s === "unrated") continue;
    rated += 1;
    if (s === "done") done += 1;
  }
  return { done, rated };
}

/**
 * 対象の全 OS で動く機能の数。done が 1 つ以上あり、残りがすべて「できない」か「対象外」の機能を数える。
 */
const allOsDone = features.filter((f) => {
  const ss = platforms.map((os) => f.platforms[os].status);
  return ss.includes("done") && ss.every((s) => s === "done" || s === "no" || s === "n-a");
}).length;

// --- Markdown ---------------------------------------------------------------

/** セル内の `|` は表を壊すためエスケープする。改行は 1 行に畳む。 */
function md(s) {
  return String(s ?? "").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
}

function buildMarkdown() {
  const L = [];
  L.push("# 端末機能 対応表（OS × 機能）", "");
  L.push(`> ${GENERATED}`, "");
  L.push(notes.intro, "");

  L.push("## 1. 読み方", "");
  L.push("### 1.1 状態の定義", "");
  L.push("| 状態 | 識別子 | 意味 |", "| --- | --- | --- |");
  for (const [k, v] of Object.entries(statusLabels)) {
    L.push(`| ${md(v)} | \`${k}\` | ${md(statusDefinitions[k] ?? "")} |`);
  }
  L.push("");
  L.push("### 1.2 実機確認の表記（「できている」のみ）", "");
  L.push("| 表記 | 意味 |", "| --- | --- |");
  for (const [k, v] of Object.entries(verifiedDefinitions)) {
    L.push(`| \`verified: ${k}\` | ${md(v)} |`);
  }
  L.push("");
  L.push("### 1.3 備考の読み方", "");
  L.push(notes.reading, "");

  L.push("## 2. 進捗", "");
  L.push("| OS | できている / 評価済み |", "| --- | --- |");
  for (const os of platforms) {
    const { done, rated } = tally(os);
    L.push(`| ${md(platformLabels[os])} | ${done} / ${rated} |`);
  }
  L.push("", `全 ${features.length} 機能。「評価済み」は対象外・未評価を除いた数。対象の全 OS（できない・対象外を除く）で動く機能は ${allOsDone}。`, "");

  L.push("## 3. 対応表", "");
  const groups = [...new Set(features.map((f) => f.group))];
  for (const g of groups) {
    L.push(`### ${md(g)}`, "");
    L.push(`| 機能 | ${platforms.map((o) => md(platformLabels[o])).join(" | ")} |`);
    L.push(`| --- | ${platforms.map(() => "---").join(" | ")} |`);
    for (const f of features.filter((x) => x.group === g)) {
      const cells = platforms.map((o) => md(cellText(f.platforms[o])));
      L.push(`| ${md(f.name)} | ${cells.join(" | ")} |`);
    }
    L.push("");
  }

  L.push("## 4. 機能ごとの詳細", "");
  for (const f of features) {
    L.push(`### ${md(f.name)}`, "");
    // 横断の根拠は 1000 字を超える行があるため箇条書きではなく表に入れる
    // （markdownlint の MD013 は表を対象外にしており、原文を折り返さずに載せられる）。
    L.push("| 項目 | 内容 |", "| --- | --- |");
    L.push(`| id | \`${f.id}\` |`);
    L.push(`| 区分 | ${md(f.group)} |`);
    if (f.capture) L.push(`| キャプチャ | [${md(f.capture)}](${f.capture})（Mac の正常系） |`);
    if (f.note) L.push(`| 横断の根拠 | ${md(f.note)} |`);
    L.push("");
    L.push("| OS | 状態 | API・フレームワーク | 権限・設定キー | 公式ドキュメント | 備考 |");
    L.push("| --- | --- | --- | --- | --- | --- |");
    for (const os of platforms) {
      const p = f.platforms[os];
      const notesCol = [p.approval ? `申請: ${p.approval}` : null, p.implStatus ? `実態: ${p.implStatus}` : null, p.note,
        p.safari ? `Safari: ${p.safari}` : null, p.impl ? `根拠: ${p.impl}` : null]
        .filter(Boolean).join(" / ");
      L.push(
        `| ${md(platformLabels[os])} | ${md(cellText(p))} | ${md(p.api ?? "—")} | ${md(p.perm ?? "—")} | ` +
          `${urls(p).map((u) => `<${u}>`).join(" / ") || "—"} | ${md(notesCol || "—")} |`,
      );
    }
    L.push("");
  }

  L.push("## 5. 1 機能の実装に要るもの（Phase 10 の実例）", "");
  L.push(notes.implementationSteps, "");
  L.push("## 6. 本表の限界", "");
  L.push(notes.limitations, "");
  if (excluded.length) {
    L.push(`## 7. 載せない機能（${excludedCount} 件）`, "");
    L.push("各 OS の端末機能を洗い出したうえで、一覧に入れなかったもの。理由の区分ごとにまとめる。", "");
    for (const c of excluded) {
      L.push(`### ${md(c.reason)}（${c.items.length} 件）`, "", md(c.detail), "");
      L.push("| 機能 | 主な API | 理由 |", "| --- | --- | --- |");
      for (const it of c.items) L.push(`| ${md(it.name)} | ${md(it.api)} | ${md(it.why)} |`);
      L.push("");
    }
  }
  if (approvals.length) {
    L.push(`## 8. 申請待ち（${approvals.length} 件）`, "");
    L.push(`OS ベンダーへの申請・承認が要るため、実装を見送っているセル（表では「${APPROVAL_MARK}」）。expand-capability の候補から外れる。申請できる状態になったら、この一覧から見直す。`, "");
    L.push("| 機能 | OS | 要る申請 |", "| --- | --- | --- |");
    for (const a of approvals) L.push(`| ${md(a.f.name)} | ${md(platformLabels[a.os])} | ${md(a.text)} |`);
    L.push("");
  }
  return `${L.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd()}\n`;
}

// --- HTML -------------------------------------------------------------------

function esc(s) {
  return String(s ?? "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function buildHtml() {
  // JSON をそのまま埋め込んで単一ファイルで完結させる（file:// から開けるように fetch を使わない）。
  // `<` を < へ逃がし、本文中の </script> で早期終了しないようにする。
  const embedded = JSON.stringify(data).replace(/</g, "\\u003c");
  const summary = platforms
    .map((os) => {
      const { done, rated } = tally(os);
      return `<div class="stat"><b>${esc(platformLabels[os])}</b><span>${done} / ${rated}</span></div>`;
    })
    .join("");
  return `<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>端末機能 対応表（OS × 機能）</title>
<!-- ${esc(GENERATED)} -->
<style>
:root { color-scheme: light dark; --bd:#d0d7de; --mut:#57606a; --bg:#fff; --fg:#1f2328; --head:#f6f8fa; }
@media (prefers-color-scheme: dark) {
  :root { --bd:#30363d; --mut:#9198a1; --bg:#0d1117; --fg:#e6edf3; --head:#161b22; }
}
* { box-sizing: border-box; }
body { margin:0; padding:24px; background:var(--bg); color:var(--fg);
  font:14px/1.6 system-ui,-apple-system,"Hiragino Sans","Noto Sans JP",sans-serif; }
h1 { font-size:20px; margin:0 0 4px; }
p.lead { color:var(--mut); margin:0 0 20px; white-space:pre-line; }
.stats { display:flex; flex-wrap:wrap; gap:8px; margin-bottom:20px; }
.stat { border:1px solid var(--bd); border-radius:8px; padding:8px 14px; }
.stat b { display:block; font-size:12px; color:var(--mut); font-weight:600; }
.stat span { font-size:18px; font-variant-numeric:tabular-nums; }
.bar { display:flex; flex-wrap:wrap; gap:10px; align-items:center; margin-bottom:16px;
  padding:12px; border:1px solid var(--bd); border-radius:8px; }
.bar label { font-size:12px; color:var(--mut); }
input,select { font:inherit; padding:5px 8px; border:1px solid var(--bd); border-radius:6px;
  background:var(--bg); color:var(--fg); }
input[type=search] { min-width:220px; }
table { border-collapse:collapse; width:100%; }
th,td { border:1px solid var(--bd); padding:6px 9px; text-align:left; vertical-align:top; font-size:13px; }
th { background:var(--head); position:sticky; top:0; }
td.name { font-weight:600; }
a.cap { display:block; margin:6px 0 2px; }
a.cap img { display:block; width:160px; max-width:100%; border:1px solid var(--bd); border-radius:4px; }
td.grp { color:var(--mut); font-size:12px; white-space:nowrap; }
.s { display:inline-block; padding:1px 7px; border-radius:10px; font-size:12px; white-space:nowrap; }
.s-done { background:#1a7f37; color:#fff; }
.s-possible { background:#9a6700; color:#fff; }
.s-no { background:#cf222e; color:#fff; }
.s-na { background:#6e7781; color:#fff; }
.s-unrated { background:transparent; color:var(--mut); border:1px dashed var(--bd); }
.d { display:block; color:var(--mut); font-size:11px; margin-top:2px; }
details { margin-top:4px; }
summary { cursor:pointer; color:var(--mut); font-size:12px; }
dl { margin:6px 0 0; font-size:12px; }
dt { font-weight:600; margin-top:6px; }
dd { margin:0 0 0 12px; color:var(--mut); overflow-wrap:anywhere; }
.legend { margin:10px 0 4px; }
.legend > summary { font-size:13px; }
.legend p { color:var(--mut); font-size:12px; white-space:pre-line; margin:8px 0 0; }
#empty { color:var(--mut); padding:20px 0; }
</style>
</head>
<body>
<h1>端末機能 対応表（OS × 機能）</h1>
<p class="lead">${esc(notes.intro)}</p>
<div class="stats">${summary}<div class="stat"><b>機能数</b><span>${features.length}</span></div><div class="stat"><b>全 OS で動く</b><span>${allOsDone}</span></div></div>
<details class="legend"><summary>読み方（状態・実機確認の定義）</summary>
<dl>
${Object.entries(statusLabels).map(([k, v]) =>
  `<dt><span class="s ${STATUS_CLASS[k]}">${esc(v)}</span></dt><dd>${esc(statusDefinitions[k] ?? "")}</dd>`).join("\n")}
${Object.entries(verifiedDefinitions).map(([k, v]) =>
  `<dt><code>verified: ${esc(k)}</code></dt><dd>${esc(v)}</dd>`).join("\n")}
</dl>
<p>${esc(notes.reading)}</p>
</details>
<details class="legend"><summary>1 機能の実装に要るもの（Phase 10 の実例）</summary>
<p>${esc(notes.implementationSteps)}</p>
</details>
<details class="legend"><summary>本表の限界</summary>
<p>${esc(notes.limitations)}</p>
</details>
${excluded.length ? `<details class="legend" id="excluded"><summary>載せない機能（${excludedCount} 件）</summary>
<p>各 OS の端末機能を洗い出したうえで、一覧に入れなかったもの。理由の区分ごとにまとめる。</p>
${excluded.map((c) => `<details><summary>${esc(c.reason)}（${c.items.length} 件）</summary>
<p>${esc(c.detail)}</p>
<table><thead><tr><th>機能</th><th>主な API</th><th>理由</th></tr></thead><tbody>
${c.items.map((it) => `<tr><td>${esc(it.name)}</td><td>${esc(it.api)}</td><td>${esc(it.why)}</td></tr>`).join("\n")}
</tbody></table>
</details>`).join("\n")}
</details>` : ""}
${approvals.length ? `<details class="legend" id="approval"><summary>申請待ち（${approvals.length} 件）</summary>
<p>OS ベンダーへの申請・承認が要るため、実装を見送っているセル（表では「${APPROVAL_MARK}」）。expand-capability の候補から外れる。申請できる状態になったら、この一覧から見直す。</p>
<table><thead><tr><th>機能</th><th>OS</th><th>要る申請</th></tr></thead><tbody>
${approvals.map((x) => `<tr><td>${esc(x.f.name)}</td><td>${esc(platformLabels[x.os])}</td><td>${esc(x.text)}</td></tr>`).join("\n")}
</tbody></table>
</details>` : ""}
<div class="bar">
  <label>検索 <input type="search" id="q" placeholder="機能名・API・備考"></label>
  <label>区分 <select id="g"><option value="">すべて</option></select></label>
  <label>状態 <select id="s"><option value="">すべて</option></select></label>
  <label>OS <select id="o"><option value="">すべて</option></select></label>
  <span id="count"></span>
</div>
<table id="cap"><thead><tr><th>機能</th><th>区分</th>${platforms
    .map((o) => `<th>${esc(platformLabels[o])}</th>`)
    .join("")}</tr></thead><tbody id="tb"></tbody></table>
<p id="empty" hidden>条件に一致する機能はありません。</p>
<script type="application/json" id="data">${embedded}</script>
<script>
const D = JSON.parse(document.getElementById('data').textContent);
const { platforms: OS, platformLabels: PL, statusLabels: SL, features: F } = D;
const CLS = ${JSON.stringify(STATUS_CLASS)};
const AM = ${JSON.stringify(APPROVAL_MARK)};
const el = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;' }[c]));

for (const g of [...new Set(F.map((f) => f.group))]) el('g').add(new Option(g, g));
for (const [k, v] of Object.entries(SL)) el('s').add(new Option(v, k));
for (const o of OS) el('o').add(new Option(PL[o], o));

// 検索対象は機能名だけでなく API・権限・備考まで含める（該当機能を名前で思い出せなくても辿れるように）。
// sourceName は移行前の OS 別文書での旧称を残したもので、旧名で探す人が辿り着けるよう haystack に含める。
const hay = (f) => [f.name, f.id, f.group, f.note,
  ...OS.flatMap((o) => { const p = f.platforms[o]; return [p.api, p.perm, p.note, p.safari, p.impl, p.implStatus, p.detail, p.url, p.sourceName].flat(); })
].filter(Boolean).join(' ').toLowerCase();

function cell(p) {
  const label = SL[p.status] ?? p.status;
  // detail は「できている（実機）」のように状態語を含むため、バッジと重複する先頭を削る
  const trimmed = p.detail && p.detail.startsWith(label) ? p.detail.slice(label.length) : p.detail;
  const detail = trimmed ? '<span class="d">' + esc(trimmed) + '</span>' : '';
  // url は単一なら文字列、複数ソースが統合された機能では配列
  const links = (p.url ? (Array.isArray(p.url) ? p.url : [p.url]) : [])
    .map((u) => '<a href="' + esc(u) + '" rel="noreferrer">' + esc(u) + '</a>').join('<br>');
  const ap = p.approval ? '<span class="d" title="' + esc(p.approval) + '">' + AM + '</span>' : '';
  const rows = [['申請', esc(p.approval)], ['実態', esc(p.implStatus)], ['API', esc(p.api)], ['権限', esc(p.perm)],
    ['ドキュメント', links], ['備考', esc(p.note)], ['Safari', esc(p.safari)], ['根拠', esc(p.impl)]]
    .filter(([, v]) => v);
  const more = rows.length
    ? '<details><summary>詳細</summary><dl>' + rows.map(([k, v]) =>
        '<dt>' + k + '</dt><dd>' + v + '</dd>').join('') + '</dl></details>'
    : '';
  return '<span class="s ' + CLS[p.status] + '">' + esc(label) + '</span>' + ap + detail + more;
}

function render() {
  const q = el('q').value.trim().toLowerCase();
  const g = el('g').value, s = el('s').value, o = el('o').value;
  const shown = F.filter((f) => {
    if (g && f.group !== g) return false;
    if (q && !hay(f).includes(q)) return false;
    // OS と状態は組で効かせる: OS 指定時はその OS の状態、未指定ならどれかの OS が一致すれば残す
    if (s) { if (o ? f.platforms[o].status !== s : !OS.some((x) => f.platforms[x].status === s)) return false; }
    return true;
  });
  const cols = o ? [o] : OS;
  document.querySelectorAll('#cap thead th').forEach((th, i) => {
    if (i >= 2) th.hidden = !cols.includes(OS[i - 2]);
  });
  el('tb').innerHTML = shown.map((f) =>
    '<tr><td class="name">' + esc(f.name) +
      // キャプチャは画像本体を同梱せず相対参照する（base64 埋め込みだと 100 機能で HTML が
      // 10MB を超えるため）。単一ファイルでは動かず captures/ とセットで配布する。
      (f.capture ? '<a class="cap" href="' + esc(f.capture) + '" target="_blank" rel="noopener">' +
        '<img src="' + esc(f.capture) + '" alt="' + esc(f.name) + ' の画面" loading="lazy"></a>' : '') +
      (f.note ? '<details><summary>横断の根拠</summary><dl><dd>' + esc(f.note) + '</dd></dl></details>' : '') +
      '</td><td class="grp">' + esc(f.group) + '</td>' +
    OS.map((x) => '<td' + (cols.includes(x) ? '' : ' hidden') + '>' + cell(f.platforms[x]) + '</td>').join('') + '</tr>'
  ).join('');
  el('count').textContent = shown.length + ' / ' + F.length + ' 件';
  el('empty').hidden = shown.length > 0;
}
for (const id of ['q', 'g', 's', 'o']) el(id).addEventListener('input', render);
render();
</script>
</body>
</html>
`;
}

// --- entry ------------------------------------------------------------------
// `node --test` からのテストは checkImplPaths / checkAndroidPermissions を直接呼ぶだけで、
// このスクリプトを実行ファイルとしては起動しない。import 時に生成物を書き換えないよう、
// CLI として直接実行されたときだけ以下を走らせる。判定は realpath で正規化して比較する
// （symlink・相対パス経由の起動で文字列が食い違い、CI の検査が何もせず成功する
// fail-open を避けるため）。
const invokedAsCli = (() => {
  try {
    return realpathSync(process.argv[1] ?? "") === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
})();
if (invokedAsCli) {
  const outputs = [
    ["capabilities.md", buildMarkdown()],
    ["capabilities.html", buildHtml()],
  ];
  // 生成（outputs の組み立て）は --check でも走らせ、生成時の例外を CI で拾う。書き込みは通常時だけ
  const check = process.argv.includes("--check");
  if (!check) {
    for (const [name, body] of outputs) {
      writeFileSync(join(dir, name), body);
      console.log(`generated: docs/device/${name}`);
    }
  }
  if (check) {
    const pathErrors = checkImplPaths(features, repoRoot);
    for (const e of pathErrors) console.error(`[impl-path] 実装パスが存在しません: ${e}`);
    const manifestPath = join(repoRoot, "platforms", "android", "app", "src", "main", "AndroidManifest.xml");
    const permErrors = checkAndroidPermissions(features, manifestPath);
    for (const e of permErrors) console.error(`[android-perm] AndroidManifest.xml に uses-permission がありません: ${e}`);
    if (pathErrors.length > 0 || permErrors.length > 0) process.exit(1);
    console.log(`docs/device/capabilities/ の検査を通過（${features.length} 機能）。`);
  }
}

// テスト（scripts/device/build-capabilities.test.mjs）専用のエクスポート。
export { checkImplPaths, checkAndroidPermissions, extractImplPaths, badUrls };
