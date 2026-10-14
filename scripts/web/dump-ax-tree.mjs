// TASK-4.4-S4（REQ-4 / Issue #82）: Chrome DevTools Protocol
// （Accessibility.getFullAXTree）を Node 標準の WebSocket クライアント（Node 24 組み込み）
// のみで叩き、指定 URL のページのブラウザ標準 Accessibility Tree を取得・検査する。
//
// npm 依存を持ち込まず native fetch/WebSocket のみで完結させる方針（security.md
// 「A06 脆弱な依存」の最小依存方針）。scripts/web/verify-dom-a11y.sh から
// 「chromium --remote-debugging-port=<port> --user-data-dir=<...> <url>」で
// 起動済みの headless chromium を対象に呼び出される想定。
//
// 使い方: node dump-ax-tree.mjs <cdp-host:port> <検証対象 URL> <結果 JSON 出力パス>
//
// 期待ロール（design dom-output-architecture.md §2.2 のマッピング表 + sample_screen()
// の構成）: heading（name にタイトルを含む）/ textbox（name="メモ"）/ button×2/
// list / listitem×2。1 つでも欠けていれば非 0 終了する（PoC-5 の「意味情報 0 件」との
// 対比が本 issue の検証の骨子であり、期待ロールの欠落を検知漏れにしない）。
// heading は role の有無だけでなく、accessible name に EXPECTED_HEADING_NAME
// （sample_screen() のタイトル文字列）を含むことまで確認する（レビュー指摘:
// textbox は name="メモ" を既に検査しているのに heading だけ空・誤った
// accessible name の h1 でも通ってしまっていた）。

const [, , cdpAddress, targetUrl, outPath] = process.argv;

if (!cdpAddress || !targetUrl || !outPath) {
  console.error(
    "使い方: node dump-ax-tree.mjs <cdp-host:port> <検証対象 URL> <結果 JSON 出力パス>",
  );
  process.exit(1);
}

// sample_screen() の Screen タイトル文字列（crates/fandhe-core/src/ui/sample.rs の
// Screen::new("サンプル画面")）。dom_tree.rs（Screen::title() → h1）により
// h1 の accessible name として現れる。heading の accessible name に
// この文字列が含まれることを検証する。
const EXPECTED_HEADING_NAME = "サンプル画面";

/** ページ側の文字列を eval せず、CDP の JSON レスポンスのみを比較・保存する
 * （A03 対策: このスクリプトは検証対象ページの内容を実行しない）。 */
async function findPageTarget(cdpAddress) {
  const listRes = await fetch(`http://${cdpAddress}/json/list`);
  if (!listRes.ok) {
    throw new Error(`/json/list の取得に失敗した: HTTP ${listRes.status}`);
  }
  const targets = await listRes.json();
  const page = targets.find((t) => t.type === "page");
  if (!page) {
    throw new Error("page 種別の CDP ターゲットが見つからない");
  }
  return page;
}

function connect(webSocketDebuggerUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(webSocketDebuggerUrl);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", (err) => reject(err));
  });
}

function makeRpc(ws) {
  let nextId = 0;
  const pending = new Map();
  // WebSocket がエラー・切断した場合、pending 中の call() の Promise が
  // 一切 reject/resolve されないと chromium クラッシュ時にタイムアウトなく
  // ハングしうる（レビュー指摘）。error/close で pending 全件を reject する。
  const rejectAllPending = (reason) => {
    for (const { reject } of pending.values()) {
      reject(reason);
    }
    pending.clear();
  };
  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id !== undefined && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) {
        reject(new Error(`CDP エラー: ${JSON.stringify(msg.error)}`));
      } else {
        resolve(msg.result);
      }
    }
  });
  ws.addEventListener("error", (err) => {
    rejectAllPending(new Error(`CDP WebSocket エラー: ${err.message ?? err}`));
  });
  ws.addEventListener("close", () => {
    rejectAllPending(new Error("CDP WebSocket が切断された（chromium クラッシュの可能性）"));
  });
  return function call(method, params = {}) {
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  };
}

/** 起動直後は wasm ロード・マウントが未完了のため、期待ロールが揃うまで
 * ポーリングする（固定 sleep ではなく状態確認、既存 verify-wasm-proxy.sh の
 * 「サーバー起動をポーリングで待つ」作法を AX ツリー取得へ適用したもの）。 */
async function waitForExpectedRoles(call, maxAttempts) {
  let lastNodes = [];
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    await call("Accessibility.enable");
    const result = await call("Accessibility.getFullAXTree");
    lastNodes = result.nodes ?? [];
    const roles = lastNodes.map((n) => n.role?.value);
    const hasHeading = lastNodes.some(
      (n) => n.role?.value === "heading" && n.name?.value?.includes(EXPECTED_HEADING_NAME),
    );
    const hasButtons = roles.filter((r) => r === "button").length >= 2;
    const hasList = roles.includes("list");
    const hasListItems = roles.filter((r) => r === "listitem").length >= 2;
    const hasTextbox = lastNodes.some(
      (n) => n.role?.value === "textbox" && n.name?.value === "メモ",
    );
    if (hasHeading && hasButtons && hasList && hasListItems && hasTextbox) {
      return { nodes: lastNodes, ready: true };
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  return { nodes: lastNodes, ready: false };
}

async function main() {
  const page = await findPageTarget(cdpAddress);
  const ws = await connect(page.webSocketDebuggerUrl);
  const call = makeRpc(ws);

  await call("Page.navigate", { url: targetUrl });
  // ページ遷移直後は Accessibility ドメインの再 enable が必要な場合があるため、
  // ここでも明示的に enable してからポーリングへ入る。
  await call("Accessibility.enable");

  const { nodes, ready } = await waitForExpectedRoles(call, 20);

  const roles = nodes.map((n) => ({
    role: n.role?.value ?? null,
    name: n.name?.value ?? null,
  }));

  const { writeFileSync } = await import("node:fs");
  writeFileSync(
    outPath,
    JSON.stringify(
      {
        url: targetUrl,
        capturedAtUtc: new Date().toISOString(),
        ready,
        roles,
      },
      null,
      2,
    ),
  );

  ws.close();

  if (!ready) {
    console.error(
      `[dump-ax-tree] エラー: 期待ロール（heading(name に "${EXPECTED_HEADING_NAME}" を含む)/textbox(name=メモ)/button×2/list/listitem×2）が揃わなかった。結果は ${outPath} に保存済み。`,
    );
    process.exit(1);
  }

  console.log(`[dump-ax-tree] 期待ロールすべて確認: ${outPath}`);
}

main().catch((err) => {
  console.error(`[dump-ax-tree] エラー: ${err.message ?? err}`);
  process.exit(1);
});
