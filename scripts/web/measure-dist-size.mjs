/**
 * TASK-4.5（REQ-4 / M4、Issue #235）: measure-dist-size.sh から呼ばれる実体。
 * dist/ 配下のファイルを brotli 圧縮した後のバイト数を集計し、
 * 「wasm 本体（REQ-4 の 3MB 基準対象）」と「fonts/ 込み合計（TASK-10.2 参照）」を
 * 分離した JSON を標準出力へ書く。
 *
 * A03（インジェクション）対策: 引数はそのまま fs 呼び出しへ渡すのみでシェル展開や
 * 子プロセス生成を行わない。dist ディレクトリ配下のみを走査し、シンボリックリンク経由の
 * ディレクトリ外参照は追わない（realpath 突合で dist_dir 配下に収まることを確認する）。
 */
import { readFileSync, readdirSync, statSync, realpathSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import { brotliCompressSync, constants as zlibConstants } from "node:zlib";

const distDirArg = process.argv[2];
if (!distDirArg) {
  console.error("使い方: measure-dist-size.mjs <dist ディレクトリ>");
  process.exit(1);
}

const distDir = resolve(distDirArg);
const distDirReal = realpathSync(distDir);

// wasm 本体（REQ-4 の 3MB 基準対象）: index.html + wasm-bindgen 生成物（.js/.wasm）。
// フォント（fonts/）は同基準の対象外（TASK-10.1 コメント・build-wasm-proxy.sh 参照）。
const CORE_EXTENSIONS = new Set([".html", ".js", ".wasm"]);

function walk(dir) {
  const entries = readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isSymbolicLink()) {
      // dist 配下を超えるシンボリックリンクは無視する（A08 整合性・想定外ファイル混入防止）。
      // `startsWith(distDirReal)` だけでは "dist" に対する兄弟パス "dist-evil" 等も
      // マッチしてしまうため、path.sep で区切った境界比較にする
      // （serve-brotli.mjs の realpath 突合と同じ対処、レビュー指摘）。
      const real = realpathSync(fullPath);
      if (real !== distDirReal && !real.startsWith(distDirReal + sep)) {
        continue;
      }
    }
    const st = statSync(fullPath);
    if (st.isDirectory()) {
      files.push(...walk(fullPath));
    } else if (st.isFile()) {
      files.push(fullPath);
    }
  }
  return files;
}

const allFiles = walk(distDirReal);

const brotliOptions = {
  params: {
    [zlibConstants.BROTLI_PARAM_QUALITY]: zlibConstants.BROTLI_MAX_QUALITY,
  },
};

let coreRawBytes = 0;
let coreBrotliBytes = 0;
let totalRawBytes = 0;
let totalBrotliBytes = 0;
const perFile = [];

for (const filePath of allFiles) {
  const relPath = relative(distDirReal, filePath);
  const raw = readFileSync(filePath);
  const compressed = brotliCompressSync(raw, brotliOptions);
  const isCore = Array.from(CORE_EXTENSIONS).some((ext) => filePath.endsWith(ext));

  totalRawBytes += raw.length;
  totalBrotliBytes += compressed.length;
  if (isCore) {
    coreRawBytes += raw.length;
    coreBrotliBytes += compressed.length;
  }

  perFile.push({
    path: relPath,
    rawBytes: raw.length,
    brotliBytes: compressed.length,
    core: isCore,
  });
}

perFile.sort((a, b) => b.brotliBytes - a.brotliBytes);

const REQ4_SIZE_LIMIT_BYTES = 3 * 1024 * 1024;

const result = {
  distDir: distDirReal,
  core: {
    rawBytes: coreRawBytes,
    brotliBytes: coreBrotliBytes,
    req4LimitBytes: REQ4_SIZE_LIMIT_BYTES,
    withinReq4Limit: coreBrotliBytes <= REQ4_SIZE_LIMIT_BYTES,
  },
  totalWithFonts: {
    rawBytes: totalRawBytes,
    brotliBytes: totalBrotliBytes,
  },
  files: perFile,
};

console.log(JSON.stringify(result, null, 2));
