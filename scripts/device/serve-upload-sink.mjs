#!/usr/bin/env node
// OS 管理のバックグラウンド送信（background-upload、Issue #835）の確認用の受け口。
// 開発・実機確認専用の道具で、配布物ではない。Node 標準のみを使い、npm 依存を足さない。
//
// 使い方:
//   node scripts/device/serve-upload-sink.mjs [--host <addr>] [--port <n>] [--rate <KiB/s>] [--status <4xx|5xx>]
//
// - 既定は 127.0.0.1:8789 の PUT /upload。受け取った本文は捨ててバイト数だけを数える。
// - --host: ループバックと RFC 1918 の私設 IPv4 だけを許す（0.0.0.0 と公開 IP は拒否。実機から Mac の
//   LAN IP で届かせるときだけ使う）。
// - --rate: 受信を絞る（KiB/s）。送信中の表示と取り消しを確かめるときに使う。
// - --status: 本文を読み切った後にその状態を返す（失敗系の確認用）。
// - 応答: Content-Length と受信バイト数が一致すれば 200 と {"bytes": n}、不一致は 400。
//   PUT 以外は 405、/upload 以外は 404、Content-Length が無い（chunked を含む）なら 411、
//   上限（MAX_BYTES）を超える宣言は 413。
// - ログは method・path・bytes・status だけ。ヘッダと本文は出さない。

import http from "node:http";
import { pathToFileURL } from "node:url";

export const MAX_BYTES = 8 * 1024 * 1024;
const UPLOAD_PATH = "/upload";

/** ループバック、または RFC 1918 の私設 IPv4 リテラルか。 */
export function isAllowedHost(host) {
  if (host === "127.0.0.1" || host === "::1" || host === "localhost") return true;
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host);
  if (!m) return false;
  const o = m.slice(1).map(Number);
  if (o.some((n) => n > 255)) return false;
  if (o[0] === 127) return true;
  if (o[0] === 10) return true;
  if (o[0] === 172 && o[1] >= 16 && o[1] <= 31) return true;
  return o[0] === 192 && o[1] === 168;
}

function log(method, path, bytes, status) {
  console.log(`${method} ${path} bytes=${bytes} status=${status}`);
}

/**
 * リクエスト処理。テストから直接使えるよう export する。
 * @param {{ rateKiBps?: number, forceStatus?: number }} opts
 */
export function createHandler(opts = {}) {
  const { rateKiBps, forceStatus } = opts;
  return (req, res) => {
    const method = req.method ?? "";
    const path = (req.url ?? "").split("?")[0];
    const reply = (status, body, bytes = 0) => {
      const payload = body === undefined ? "" : JSON.stringify(body);
      res.writeHead(status, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(payload) });
      res.end(payload);
      log(method, path, bytes, status);
    };
    if (path !== UPLOAD_PATH) {
      req.resume();
      return reply(404);
    }
    if (method !== "PUT") {
      req.resume();
      return reply(405);
    }
    const declared = req.headers["content-length"];
    if (declared === undefined || !/^\d+$/.test(declared)) {
      req.resume();
      return reply(411);
    }
    const expected = Number(declared);
    if (expected > MAX_BYTES) {
      req.resume();
      return reply(413);
    }
    let received = 0;
    let over = false;
    req.on("data", (chunk) => {
      received += chunk.length;
      if (received > MAX_BYTES) {
        over = true;
        req.destroy();
        return;
      }
      if (rateKiBps) {
        // 受信を一時停止して速度を絞る。
        req.pause();
        setTimeout(() => req.resume(), (chunk.length / (rateKiBps * 1024)) * 1000);
      }
    });
    req.on("end", () => {
      if (over) return;
      if (forceStatus) return reply(forceStatus, undefined, received);
      if (received !== expected) return reply(400, { error: "length mismatch" }, received);
      reply(200, { bytes: received }, received);
    });
    req.on("error", () => {});
  };
}

function parseArgs(argv) {
  const out = { host: "127.0.0.1", port: 8789 };
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    if (value === undefined) throw new Error(`${key} に値がありません`);
    switch (key) {
      case "--host":
        if (!isAllowedHost(value)) throw new Error("--host はループバックか私設 IPv4 だけです");
        out.host = value;
        break;
      case "--port": {
        const n = Number(value);
        if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error("--port は 1〜65535 です");
        out.port = n;
        break;
      }
      case "--rate": {
        const n = Number(value);
        if (!Number.isFinite(n) || n < 1 || n > 1024 * 1024) throw new Error("--rate は 1〜1048576 (KiB/s) です");
        out.rateKiBps = n;
        break;
      }
      case "--status": {
        const n = Number(value);
        if (!Number.isInteger(n) || n < 400 || n > 599) throw new Error("--status は 400〜599 です");
        out.forceStatus = n;
        break;
      }
      default:
        throw new Error(`未知の引数: ${key}`);
    }
  }
  return out;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  let args;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (e) {
    console.error(e.message);
    process.exit(2);
  }
  const server = http.createServer(createHandler(args));
  server.listen(args.port, args.host, () => {
    console.log(`upload sink: http://${args.host}:${args.port}${UPLOAD_PATH}`);
  });
}
