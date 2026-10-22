#!/usr/bin/env node
/**
 * 端末機能 1 件の「子 issue」（層ごとの実装 issue）をまとめて起票し、親 issue の sub_issues に付ける（Issue #1107）。
 *
 * expand-capability の Step 2 から呼ばれる。確認用 issue は作らない（先に human-check-issue.mjs を呼ぶ運用）。
 * 層の分け方（(c) を (a) へ併合する・Windows を分ける等）はここでは判断せず、--layout で受け取る。
 *
 * 使い方: node scripts/device/capability-child-issues.mjs <id> --layout <layers> [--dry-run]
 *   - layers はカンマ区切り。a | b-ios | b-android | b（iOS+Android）| c | windows。a は必須で先頭
 *   - --dry-run は読み取りだけ行い、書き込み（起票・紐付け）の内容を表示して終わる
 *   - 標準出力の最終行に `a=#N b-ios=#N ...`（dry-run で未起票の層は `#(層)`）
 *
 * gh の失敗は例外で止める（取得失敗を「該当なし」と取り違えて余計な起票をしないため）。
 *
 * 守る範囲: layout が対象 OS（status が possible かつ approval・deferred なし）を漏れなく・重複なく覆うこと・
 * 親と確認用 issue がそれぞれ open で 1 件に定まること・1 層につき open な子は 1 件（目印で再利用し、
 * 紐付けが欠けていれば付け直す）・既存の子の層と担当 OS が今回の layout と一致すること（書き込み前に照合）・
 * 追加条件は担当 OS の分だけを入れること。
 * 守らない範囲: 同じ機能を複数の手で同時に起票する競合・人が本文や親を手で書き換えた後の整合
 * （重複や配置のずれは検出して止め、直すのは人）・閉じた子の再利用（closed は既存扱いにしない）・
 * 起票直後の再実行（GitHub の検索インデックスの遅れで作ったばかりの子が見えず、二重に起票しうる）。
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const OS_NAME = { macos: "macOS", ios: "iOS", android: "Android", windows: "Windows", web: "Web" };
const LAYER_DESC = {
  a: "共通コア・trait・デスクトップ側と全対象 OS の骨格（(a)）",
  "b-ios": "iOS 側の実装（(b)）",
  "b-android": "Android 側の実装（(b)）",
  b: "iOS・Android 側の実装（(b)）",
  c: "Web 側の実装（(c)）",
  windows: "Windows 側の実装",
};

/** JSON の platforms から対象 OS（possible かつ approval・deferred なし）を取る。 */
export function targetOs(platforms) {
  return Object.entries(platforms).filter(([, p]) => p.status === "possible" && !p.approval && !p.deferred).map(([k]) => k);
}

/** layout を検証し、層ごとの担当 OS を返す。漏れ・重複・対象外 OS を含む層は例外。 */
export function resolveLayout(layers, targets) {
  if (layers[0] !== "a") throw new Error("layout は a から始める");
  const seen = new Set();
  for (const l of layers) {
    if (!LAYER_DESC[l]) throw new Error(`未知の層: ${l}`);
    if (seen.has(l)) throw new Error(`層が重複している: ${l}`);
    seen.add(l);
  }
  const fixed = { "b-ios": ["ios"], "b-android": ["android"], b: ["ios", "android"], c: ["web"], windows: ["windows"] };
  const own = {
    a: ["macos", ...(seen.has("c") ? [] : ["web"]), ...(seen.has("windows") ? [] : ["windows"])].filter((o) => targets.includes(o)),
  };
  for (const l of layers.slice(1)) {
    const bad = fixed[l].filter((o) => !targets.includes(o));
    if (bad.length) throw new Error(`層 ${l} が対象外の OS を含む: ${bad.join(", ")}（対象: ${targets.join(", ")}）`);
    own[l] = fixed[l];
  }
  const all = layers.flatMap((l) => own[l]);
  const dup = all.filter((o, i) => all.indexOf(o) !== i);
  if (dup.length) throw new Error(`OS が複数の層に重複している: ${[...new Set(dup)].join(", ")}`);
  const miss = targets.filter((o) => !all.includes(o));
  if (miss.length) throw new Error(`layout が対象 OS を覆っていない: ${miss.join(", ")}（対象: ${targets.join(", ")}）`);
  return layers.map((l) => ({ layer: l, os: own[l] }));
}

/** 層に付く追加条件（担当 OS によらない）。a だけが持つ。 */
const LAYER_EXTRA = {
  a: [
    "失敗系テスト（キャンセル・権限拒否・入力不正・古い世代の完了が成功にならない、非対応 OS が成功を返さない）",
    "**全対象 OS の骨格を先に置く**: `fandhe-egui-proxy` の dispatch（`#[cfg(...)]` の分岐・`not(any(...))` の fallback 行）を全対象 OS の最終形で書き、まだ実装しない OS は `fandhe_device::<feature>::Unsupported...` を返す薄い関数 1 つのスタブを OS 別モジュールに置く。後続の子は自分の OS のモジュールだけを埋め、dispatch・fallback 行・`device.rs` の `pub mod` 列を触らない",
    "スタブの OS は対応表で `possible` のまま、`impl` に書かない",
    "スタブを置いた OS のクロスビルド（`cargo build --target ...` か `cargo ndk`）が通る",
    "共通コア画面（`crates/fandhe-core/src/screens/`）の状態・定数・文言は、OS 固有の操作 UI（登録・解除ボタン等）を含め全対象 OS 分を (a) で定義する。後続の子は共通コア画面を触らない",
    "trait doc に契約（結果の種類・守る範囲 / 守らない範囲、契約表 10・11）を書き、後続の OS も同じ契約で実装できる形にする",
  ],
};

/** 担当 OS に付く追加条件。層の名前ではなく、その層が担当する OS の分だけを足す。 */
const OS_EXTRA = {
  macos: (chk) => [
    `macOS のセルは \`detail\` を「できている（Mac 目視待ち）」、\`implStatus\` を「Mac 目視待ち（#${chk}）」にする`,
  ],
  ios: (chk) => [
    "iOS シミュレータ向けビルドが通る",
    "PR に `device-check-pending` を付ける",
    `iOS のセルは \`detail\` を「できている（iOS 実機確認待ち）」、\`implStatus\` を「iOS 実機確認待ち（#${chk}）」にする`,
    "OS コールバックが結果を入れた直後に `crate::ios::request_repaint` で次フレームを起こす",
  ],
  android: (chk) => [
    "Android のビルド（cargo-ndk + Gradle）が通る",
    "PR に `device-check-pending` を付ける",
    `Android のセルは \`detail\` を「できている（Android 実機確認待ち）」、\`implStatus\` を「Android 実機確認待ち（#${chk}）」にする`,
    "Java は機能ごとのヘルパークラスに分け、`MainActivity` への追記は呼び出し・登録の最小限に留める",
    "Activity 再生成時は保留中の要求を Rust 側と揃えて破棄し、旧結果が新しい要求の id を消費しない",
  ],
  windows: (chk) => [
    "Windows ターゲットのビルドが通る",
    "PR に `device-check-pending` を付ける",
    `Windows のセルは \`detail\` を「できている（Windows 実機確認待ち）」、\`implStatus\` を「Windows 実機確認待ち（#${chk}）」にする`,
  ],
  web: (chk) => [
    "wasm32 ビルドが通る",
    "ヘッドレス Chrome で確認できる範囲を確認する",
    `Web のセルは \`detail\` を「できている（実ブラウザ確認待ち）」、\`implStatus\` を「実ブラウザ確認待ち（#${chk}）」にする`,
  ],
};

/** 層の追加条件と、担当 OS それぞれの追加条件。担当外の OS の条件は入れない。 */
export function extraConditions(layer, os, chk) {
  return [...(LAYER_EXTRA[layer] ?? []), ...os.flatMap((o) => OS_EXTRA[o](chk))];
}

export const marker = (id, layer) => `<!-- capability-child: ${id}/${layer} -->`;
/** 担当 OS の記録。再実行時に既存の子が今回の layout と同じ OS を担当しているかを照合する。 */
export const osMarker = (os) => `<!-- capability-child-os: ${[...os].sort().join(",")} -->`;

/**
 * 既存の子（open・目印付き）を今回の plan と照合し、層 → 番号を返す。plan に無い層・担当 OS の不一致・
 * 同じ層の重複は止める（layout を変えて再実行したときの OS の漏れ・重複を防ぐ）。
 */
export function matchExisting(id, issues, plan) {
  const want = new Map(plan.map(({ layer, os }) => [layer, osMarker(os)]));
  const re = new RegExp(`<!-- capability-child: ${id}/([a-z-]+) -->`);
  const got = {};
  for (const { number, body } of issues) {
    const layer = (body ?? "").match(re)?.[1];
    if (!layer) continue;
    if (!want.has(layer)) throw new Error(`既存の子 #${number}（${layer}）が今回の layout に無い。人間が整理する`);
    if (!(body ?? "").includes(want.get(layer))) {
      throw new Error(`既存の子 #${number}（${layer}）の担当 OS が今回の layout と違う。人間が整理する`);
    }
    if (got[layer]) throw new Error(`子 issue ${layer} が重複している: #${got[layer]}, #${number}。人間が 1 件に整理する`);
    got[layer] = number;
  }
  return got;
}

/** a が OS を持たない layout（macOS が対象外で Windows を分けた場合等）では「共通コアのみ」と表示する。 */
export const osText = (os, sep) => (os.length ? os.map((o) => OS_NAME[o]).join(sep) : "共通コアのみ");

export const titleOf = (name, id, layer, os) =>
  `feat(device): ${name} を ${osText(os, " / ")} で使えるようにする（${id}・${layer}）`;

/** dep は a の番号表記（"#12" など）。a 自身では null。 */
export function bodyOf({ id, layer, os, parent, check, dep }) {
  const cells = osText(os, " / ");
  return [
    marker(id, layer),
    osMarker(os),
    "## 対象",
    "",
    `親 #${parent} の ${LAYER_DESC[layer]}。対象 OS: ${osText(os, "・")}。`,
    ...(dep ? ["", `dependsOn: ${dep}`] : []),
    "",
    "## 受け入れ条件",
    "",
    os.length
      ? `- [ ] \`possible\` かつ \`approval\`・\`deferred\` なしだった ${cells} だけを \`done\` にし、\`impl\` に実装パスを入れる。他の OS のセル（申請待ち・保留を含む）は触らない`
      : "- [ ] 対応表のセルは変えない（この層は共通コアと全対象 OS の骨格だけを置く）",
    "- [ ] `node scripts/device/build-capabilities.mjs --check` が通る（生成物はコミットしない）",
    `- [ ] 人手の確認（Mac 目視・実機）は完了条件に含めない。確認用 issue #${check} で追う`,
    ...extraConditions(layer, os, check).map((c) => `- [ ] ${c}`),
    "",
    "実装前に `AGENTS.md` と `.claude/rules/device-bridge-contracts.md` を読むこと。",
    "",
  ].join("\n");
}

function main() {
  const args = process.argv.slice(2);
  const id = args[0];
  const li = args.indexOf("--layout");
  const layout = li > 0 ? args[li + 1] : undefined;
  const rest = args.filter((_, i) => i > 0 && i !== li && i !== li + 1);
  const dry = rest[0] === "--dry-run";
  // 未知のオプション（綴り違いの --dryrun 等）で書き込みモードに倒れないよう、引数は厳密に検証する
  if (!/^[a-z0-9-]+$/.test(id ?? "") || !layout || rest.length > 1 || (rest.length === 1 && !dry)) {
    console.error("使い方: node scripts/device/capability-child-issues.mjs <id> --layout <layers> [--dry-run]");
    process.exit(2);
  }
  const R = "repos/{owner}/{repo}";
  const LIMIT = 30; // 検索の取得上限。到達したら取りこぼしの恐れがあるので止める
  const cap = JSON.parse(readFileSync(new URL(`../../docs/device/capabilities/${id}.json`, import.meta.url), "utf8"));
  const plan = resolveLayout(layout.split(","), targetOs(cap.platforms));

  /** gh を実行して stdout を返す。非ゼロ終了は例外になる。 */
  const gh = (...a) => execFileSync("gh", a, { encoding: "utf8", stdio: ["pipe", "pipe", "inherit"] }).trim();
  const ghIn = (input, ...a) => execFileSync("gh", a, { encoding: "utf8", input, stdio: ["pipe", "pipe", "inherit"] }).trim();
  const write = (desc, fn) => (dry ? (console.log(`[dry-run] ${desc}`), 0) : fn());
  const created = (url) => {
    const n = Number(url.match(/\/(\d+)$/)?.[1]);
    if (!n) throw new Error(`起票結果から番号を取れない: ${url}`);
    return n;
  };
  const link = (parent, child) =>
    write(`#${child} を #${parent} の子にする`, () =>
      gh("api", "-X", "POST", `${R}/issues/${parent}/sub_issues`, "-F", `sub_issue_id=${gh("api", `${R}/issues/${child}`, "--jq", ".id")}`));
  const parentOf = (n) => {
    try {
      return Number(execFileSync("gh", ["api", `${R}/issues/${n}/parent`, "--jq", ".number"], { encoding: "utf8", stdio: ["pipe", "pipe", "pipe"] }));
    } catch (e) {
      if (String(e.stderr).includes("HTTP 404")) return null;
      throw e;
    }
  };
  // タイトルに id を含む open な issue を 1 回だけ取り、親・確認用・既存の子をそこから探す
  const hits = JSON.parse(gh("issue", "list", "--state", "open", "--search", `${id} in:title`, "--limit", String(LIMIT), "--json", "number,body"));
  if (hits.length >= LIMIT) throw new Error(`検索結果が上限 ${LIMIT} 件に達した。取りこぼしの恐れがあるので止める`);
  /** 本文に目印を持つ issue の番号。 */
  const find = (mark) => hits.filter((i) => (i.body ?? "").includes(mark)).map((i) => i.number);
  const one = (mark, label) => {
    const f = find(mark);
    if (f.length !== 1) throw new Error(`${label}が ${f.length} 件（目印 ${mark}）。1 件に定まらないので止める`);
    return f[0];
  };

  const parent = one(`<!-- capability: ${id} -->`, "親 issue");
  const check = one(`<!-- human-check: ${id} -->`, "確認用 issue");

  // 書き込む前に既存の子をすべて今回の plan と照合する（不一致なら 1 件も書かずに止まる）
  const existing = matchExisting(id, hits, plan);
  const nums = {};
  for (const { layer, os } of plan) {
    let n = existing[layer];
    if (n) {
      const p = parentOf(n);
      if (p === null) link(parent, n);
      else if (p !== parent) throw new Error(`子 issue #${n} が親 #${parent} 以外（#${p}）の子になっている。人間が整理する`);
    } else {
      const dep = layer === "a" ? null : nums.a ? `#${nums.a}` : "#(a)";
      const body = bodyOf({ id, layer, os, parent, check, dep });
      const title = titleOf(cap.name, id, layer, os);
      n = write(`子 issue ${layer} を起票する: ${title}\n${body}`, () =>
        created(ghIn(body, "issue", "create", "--title", title, "--body-file", "-")));
      if (n) link(parent, n);
      else console.log(`[dry-run] 起票した issue を親 #${parent} の子にする`);
    }
    nums[layer] = n;
  }
  console.log(plan.map(({ layer }) => `${layer}=${nums[layer] ? `#${nums[layer]}` : `#(${layer})`}`).join(" "));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
