/**
 * TASK-4.5（REQ-4 / M4、Issue #235）: 実 CDN 配信（brotli 圧縮 + Content-Encoding: br）を
 * ローカルで模した簡易配信サーバー。measure-web-metrics.mjs のローカル計測対象として
 * scripts/web/build-wasm-proxy.sh の dist/ を配信する。
 *
 * PoC-5（docs/spec/03-poc/web-wasm/scripts/serve-brotli.js）からの移植。差分:
 *   - CommonJS → ESM 化（Node 標準機能のみ、npm 依存追加なし。security.md A06 準拠）
 *   - 127.0.0.1 バインド固定（verify-wasm-proxy.sh の作法に合わせる。security.md A05 準拠）
 *   - パストラバーサル拒否を realpath 突合に強化（`path.startsWith` だけでは
 *     `../dist-evil` のような兄弟ディレクトリ名の前方一致を誤って許可し得るため）
 *   - dist ディレクトリ配下限定（配布物ではない検証用サーバーである旨を明記）
 *
 * 使い方: node scripts/web/serve-brotli.mjs <配信対象ディレクトリ> [ポート]
 *   常駐させず、呼び出し元（measure-web-metrics.mjs 等）が計測後に必ず停止させること。
 */
import http from "node:http";
import { readFile, realpath } from "node:fs/promises";
import path from "node:path";
import { brotliCompressSync, constants as zlibConstants } from "node:zlib";

const HOST = "127.0.0.1";

const rootArg = process.argv[2];
const port = Number.parseInt(process.argv[3] ?? "8899", 10);

if (!rootArg) {
  console.error("使い方: serve-brotli.mjs <配信対象ディレクトリ> [ポート]");
  process.exit(1);
}

const root = path.resolve(rootArg);
const rootReal = await realpath(root);

const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript",
  ".wasm": "application/wasm",
  ".otf": "font/otf",
};

// brotli 圧縮対象の拡張子（CDN が実運用で圧縮する典型的な種別。フォントも含める。
// フォント込み配信サイズは measure-dist-size.sh の totalWithFonts と対応させる）。
const BROTLI_EXTENSIONS = new Set([".js", ".wasm", ".html", ".otf"]);

const brotliCache = new Map();

function compress(data) {
  return brotliCompressSync(data, {
    params: { [zlibConstants.BROTLI_PARAM_QUALITY]: zlibConstants.BROTLI_MAX_QUALITY },
  });
}

const server = http.createServer(async (req, res) => {
  try {
    let urlPath = (req.url ?? "/").split("?")[0];
    if (urlPath === "/") {
      urlPath = "/index.html";
    }

    // A03（インジェクション）対策: URL をそのまま結合せず、root 配下への正規化後の
    // realpath が root 配下に収まることを確認してから読み出す（`../` によるディレクトリ
    // 脱出・シンボリックリンク経由の脱出の双方を拒否する）。
    const candidatePath = path.join(root, decodeURIComponent(urlPath));
    let realCandidate;
    try {
      realCandidate = await realpath(candidatePath);
    } catch {
      res.writeHead(404);
      res.end("not found");
      return;
    }
    if (realCandidate !== rootReal && !realCandidate.startsWith(rootReal + path.sep)) {
      res.writeHead(403);
      res.end("forbidden");
      return;
    }

    const raw = await readFile(realCandidate);
    const ext = path.extname(realCandidate);
    const acceptsBr = (req.headers["accept-encoding"] ?? "").includes("br");

    // 先に表現（br / identity）を選び、Range は選んだ同じバイト列に適用する。
    let data = raw;
    if (acceptsBr && BROTLI_EXTENSIONS.has(ext)) {
      data = brotliCache.get(realCandidate);
      if (!data) {
        data = compress(raw);
        brotliCache.set(realCandidate, data);
      }
      res.setHeader("Content-Encoding", "br");
    }

    res.setHeader("Content-Type", MIME_TYPES[ext] ?? "application/octet-stream");
    res.setHeader("Accept-Ranges", "bytes");

    // 単一 Range（`bytes=a-b` / `a-` / `-n`）。BITS（background-download、Issue #394）が
    // 転送前に HEAD でサイズを取り、Range で GET する。Range の値は数値として検証し、
    // 応答にそのまま出さない（A03）。HEAD は Range を無視して 200 を返す（GET のみ対象）。
    const rangeHeader = req.headers.range;
    // 複数 Range（`a-b,c-d`）は未対応として無視し、全体を 200 で返す（RFC 7233）。
    if (req.method === "GET" && rangeHeader !== undefined && !rangeHeader.includes(",")) {
      const m = /^bytes=(\d*)-(\d*)$/.exec(rangeHeader);
      let start;
      let end;
      if (m && (m[1] !== "" || m[2] !== "")) {
        if (m[1] === "") {
          // 末尾 n バイト
          const n = Number.parseInt(m[2], 10);
          start = Math.max(data.length - n, 0);
          end = data.length - 1;
        } else {
          start = Number.parseInt(m[1], 10);
          end = m[2] === "" ? data.length - 1 : Math.min(Number.parseInt(m[2], 10), data.length - 1);
        }
      }
      if (start === undefined || start >= data.length || start > end) {
        res.setHeader("Content-Range", `bytes */${data.length}`);
        res.writeHead(416);
        res.end();
        return;
      }
      res.setHeader("Content-Range", `bytes ${start}-${end}/${data.length}`);
      res.setHeader("Content-Length", String(end - start + 1));
      res.writeHead(206);
      res.end(data.subarray(start, end + 1));
      return;
    }

    res.setHeader("Content-Length", String(data.length));
    res.writeHead(200);
    res.end(data);
  } catch (err) {
    res.writeHead(500);
    res.end(String(err));
  }
});

server.listen(port, HOST, () => {
  console.log(`[serve-brotli] ${rootReal} を http://${HOST}:${port} で配信開始`);
});

// 呼び出し元プロセスから SIGTERM/SIGINT を受けたら確実に停止する
// （verify-wasm-proxy.sh の trap cleanup と同じ「常駐させない」方針）。
for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => {
    server.close(() => process.exit(0));
  });
}
