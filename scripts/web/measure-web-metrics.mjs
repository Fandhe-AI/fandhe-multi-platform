/**
 * TASK-4.5（REQ-4 / M4、Issue #235）: chromium headless を CDP（Chrome DevTools Protocol）
 * 経由で操作し、対象 URL（ローカル配信 / 実 CDN 配信どちらも可）の TTI・フレームタイム
 * p50/p95/max・転送量を計測して JSON を標準出力へ書く。
 *
 * PoC-5（docs/spec/03-poc/web-wasm/scripts/measure-tti-and-size.js）からの移植・簡略化。
 * 差分:
 *   - puppeteer-core（npm 依存）を使わず、Node 24 標準の WebSocket で CDP に直接接続する
 *     （security.md A06「脆弱な依存」対策・依存追加なし。Node 22 以降で WebSocket は
 *     グローバル安定 API）。
 *   - TTI・フレームタイムは外部プロセスの evaluate ではなく、対象ページの
 *     `crates/fandhe-egui-proxy/web/index.html` に埋め込んだページ内計装
 *     （window.__fandheMeasure、`?measure=1` 時のみ有効）が計算済みの値を
 *     Runtime.evaluate で読み出すだけにする（CDP 経由の rAF ポーリングより計装面が
 *     小さく、実機 Safari 等でも同じ値をコンソールから確認できる一貫性がある）。
 *   - `--no-sandbox` は使わない（verify-wasm-proxy.sh の作法を踏襲、security.md A05）。
 *
 * 使い方:
 *   node scripts/web/measure-web-metrics.mjs <計測対象URL> [--throttle] [--out out.json]
 *   例: node scripts/web/measure-web-metrics.mjs "http://127.0.0.1:8899/index.html?measure=1"
 *
 * URL は http(s) スキームのみ許可する（A03 インジェクション対策。任意コマンド実行には
 * つながらないが、file:// 等の想定外スキームで意図しないローカルファイル読み出しを
 * 防ぐ）。
 */
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";

const NETWORK_CONDITIONS_20MBPS_40MS = {
  offline: false,
  downloadThroughput: (20 * 1000 * 1000) / 8,
  uploadThroughput: (5 * 1000 * 1000) / 8,
  latency: 40,
};

function parseArgs(argv) {
  const args = { url: null, throttle: false, out: null, debuggingPort: 9333 };
  const rest = [...argv];
  const first = rest.shift();
  args.url = first ?? null;
  while (rest.length > 0) {
    const flag = rest.shift();
    if (flag === "--throttle") {
      args.throttle = true;
    } else if (flag === "--out") {
      args.out = rest.shift() ?? null;
    } else if (flag === "--debugging-port") {
      args.debuggingPort = Number.parseInt(rest.shift() ?? "9333", 10);
    } else {
      throw new Error(`未知の引数: ${flag}`);
    }
  }
  return args;
}

function assertHttpUrl(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error(`不正な URL: ${url}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`http(s) 以外のスキームは許可しない: ${parsed.protocol}`);
  }
  return parsed;
}

async function waitForPort(port, host, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://${host}:${port}/json/version`);
      if (res.ok) {
        return;
      }
    } catch {
      // まだ起動していない。ポーリング継続。
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`chromium の remote-debugging-port（${port}）起動確認がタイムアウトした`);
}

/** CDP クライアント。単一 WebSocket 上でリクエスト ID を管理する最小実装。 */
class CdpClient {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.eventHandlers = new Map();
    ws.addEventListener("message", (ev) => this._onMessage(ev));
    // WebSocket がエラー・切断した場合、pending 中の send() の Promise が一切
    // resolve/reject されないと、chromium がリクエスト途中で落ちたときに計測ループ内の
    // await が永久解決されずハングしうる（dump-ax-tree.mjs の rejectAllPending と同じ対処、
    // レビュー指摘）。error/close で pending 全件を reject する。
    ws.addEventListener("error", (err) => {
      this._rejectAllPending(new Error(`CDP WebSocket エラー: ${err.message ?? err}`));
    });
    ws.addEventListener("close", () => {
      this._rejectAllPending(new Error("CDP WebSocket が切断された（chromium クラッシュの可能性）"));
    });
  }

  _rejectAllPending(reason) {
    for (const { reject } of this.pending.values()) {
      reject(reason);
    }
    this.pending.clear();
  }

  _onMessage(ev) {
    const msg = JSON.parse(ev.data);
    if (msg.id !== undefined && this.pending.has(msg.id)) {
      const { resolve, reject } = this.pending.get(msg.id);
      this.pending.delete(msg.id);
      if (msg.error) {
        reject(new Error(msg.error.message));
      } else {
        resolve(msg.result);
      }
      return;
    }
    if (msg.method && this.eventHandlers.has(msg.method)) {
      for (const handler of this.eventHandlers.get(msg.method)) {
        handler(msg.params);
      }
    }
  }

  on(method, handler) {
    if (!this.eventHandlers.has(method)) {
      this.eventHandlers.set(method, []);
    }
    this.eventHandlers.get(method).push(handler);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
}

async function openTargetWebSocket(host, port) {
  const versionRes = await fetch(`http://${host}:${port}/json/new?about:blank`, { method: "PUT" });
  const target = await versionRes.json();
  return { ws: new WebSocket(target.webSocketDebuggerUrl), targetId: target.id };
}

async function closeTarget(host, port, targetId) {
  await fetch(`http://${host}:${port}/json/close/${targetId}`).catch(() => {});
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.url) {
    console.error(
      "使い方: measure-web-metrics.mjs <計測対象URL> [--throttle] [--out out.json] [--debugging-port N]",
    );
    process.exit(1);
  }
  const parsedUrl = assertHttpUrl(args.url);

  const host = "127.0.0.1";
  const port = args.debuggingPort;

  // verify-wasm-proxy.sh の作法（--no-sandbox を使わない・swiftshader 固定）を踏襲する。
  const chromiumArgs = [
    "--headless=new",
    "--use-gl=angle",
    "--use-angle=swiftshader",
    `--remote-debugging-port=${port}`,
    `--remote-debugging-address=${host}`,
    "--window-size=480,360",
  ];
  const chromiumProc = spawn("chromium", chromiumArgs, { stdio: ["ignore", "ignore", "pipe"] });
  let chromiumStderr = "";
  chromiumProc.stderr.on("data", (d) => {
    chromiumStderr += d.toString();
  });

  let exitCode = 0;
  try {
    await waitForPort(port, host, 15000);

    const { ws, targetId } = await openTargetWebSocket(host, port);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve, { once: true });
      ws.addEventListener("error", reject, { once: true });
    });
    const client = new CdpClient(ws);

    await client.send("Page.enable");
    await client.send("Network.enable");
    await client.send("Runtime.enable");

    if (args.throttle) {
      await client.send("Network.emulateNetworkConditions", NETWORK_CONDITIONS_20MBPS_40MS);
    }

    let totalEncodedBytes = 0;
    const resources = [];
    client.on("Network.loadingFinished", (p) => {
      totalEncodedBytes += p.encodedDataLength ?? 0;
    });
    client.on("Network.responseReceived", (p) => {
      resources.push({
        url: p.response.url,
        mimeType: p.response.mimeType,
        status: p.response.status,
      });
    });

    const wallStart = Date.now();
    // `Page.loadEventFired` はコマンドではなくイベントのため、send() ではなく
    // イベントリスナーで待ち受ける（送信するとサーバー側で存在しないメソッドとして
    // エラー扱いになり、navigate 直後に無条件で先へ進んでしまうバグを是正）。
    const loadEventPromise = new Promise((resolve) => {
      client.on("Page.loadEventFired", resolve);
    });
    await client.send("Page.navigate", { url: parsedUrl.toString() });
    await Promise.race([loadEventPromise, new Promise((r) => setTimeout(r, 10000))]);

    // window.__fandheMeasure.done（index.html の計装、?measure=1 必須）を最大 15 秒ポーリングする。
    const measureDeadline = Date.now() + 15000;
    let measureResult = null;
    while (Date.now() < measureDeadline) {
      const evalRes = await client.send("Runtime.evaluate", {
        expression:
          "window.__fandheMeasure && window.__fandheMeasure.done ? JSON.stringify(window.__fandheMeasure.result) : null",
        returnByValue: true,
      });
      if (evalRes.result?.value) {
        measureResult = JSON.parse(evalRes.result.value);
        break;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    const wallToMeasureDoneMs = Date.now() - wallStart;

    if (measureResult === null) {
      throw new Error(
        "window.__fandheMeasure.done がタイムアウトまでに true にならなかった。" +
          " 計測対象 URL に ?measure=1 が付いているか、index.html の計装が読み込まれているか確認すること。",
      );
    }

    await closeTarget(host, port, targetId);

    const output = {
      url: parsedUrl.toString(),
      throttle: args.throttle
        ? { downloadMbps: 20, uploadMbps: 5, latencyMs: 40 }
        : null,
      wallToMeasureDoneMs,
      pageMeasurement: measureResult,
      network: {
        totalEncodedBytes,
        resourceCount: resources.length,
        resources,
      },
    };

    const json = JSON.stringify(output, null, 2);
    if (args.out) {
      await writeFile(args.out, json);
      console.error(`[measure-web-metrics] 出力: ${args.out}`);
    }
    console.log(json);
  } catch (err) {
    console.error("[measure-web-metrics] エラー:", err.message);
    if (chromiumStderr) {
      console.error("[measure-web-metrics] chromium stderr:", chromiumStderr.slice(-2000));
    }
    exitCode = 1;
  } finally {
    chromiumProc.kill("SIGTERM");
  }

  process.exit(exitCode);
}

main();
