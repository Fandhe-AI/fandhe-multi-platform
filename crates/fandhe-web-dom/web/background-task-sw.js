// Issue #1047: 型サンプル「バックグラウンド処理」の Background Synchronization API
// 実体。`sync` イベントで予約中の印（PENDING_KEY）を確認し、あれば実行記録
// （RECORD_KEY）を書いて印を消す。判定と更新は 1 つの readwrite トランザクション内で
// 行う（取り消し（ページ側が PENDING_KEY を消す）と競合しても IndexedDB のトランザクション
// が直列化するため、どちらが先でも嘘の実行記録は残らない。
// crates/fandhe-web-dom/src/background_task.rs モジュール doc「保存先」参照）。
//
// DB 名・store 名・key・tag・記録形式は Rust 側（background_task.rs::web の
// DB_NAME/STORE_NAME/PENDING_KEY、fandhe_device::background_task::RECORD_KEY/
// encode_record）と一致させること。`fetch` ハンドラは持たない（他画面の通信に
// 介入しない、crates/fandhe-web-dom のモジュール doc 参照）。`importScripts` ・
// 外部 CDN は使わない（security.md A08）。

const DB_NAME = "fandhe-background-task";
const STORE_NAME = "kv";
const PENDING_KEY = "background-task.pending";
const RECORD_KEY = "background-task.last-run";
const SYNC_TAG = "fandhe-background-task";

function openDb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function requestToPromise(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

// `PENDING_KEY` があれば `RECORD_KEY` を書いて `PENDING_KEY` を削除する。無ければ
// 取り消し済み・未予約として無視する（べき等。device-bridge-contracts.md
// 「10. キャンセル・正常終了・失敗の三値」）。既存の記録より新しい実行のみ残す
// （オーナー承認済みの方針、10/2。複数の sync が入れ替わって届いても古い方の
// 記録で新しい実行結果を上書きしない）。
async function runOnce() {
  const db = await openDb();
  // 成功・失敗のどの経路でも接続を閉じる（PR #1051 codex 指摘）。close() は
  // 実行中のトランザクションの完了を待ってから閉じる。
  try {
    await runOnceWith(db);
  } finally {
    db.close();
  }
}

async function runOnceWith(db) {
  const tx = db.transaction(STORE_NAME, "readwrite");
  const store = tx.objectStore(STORE_NAME);

  const pending = await requestToPromise(store.get(PENDING_KEY));
  if (pending === undefined) {
    return;
  }

  const existingRaw = await requestToPromise(store.get(RECORD_KEY));
  const ranAtUnixMs = Date.now();
  let shouldWrite = true;
  if (typeof existingRaw === "string") {
    // Rust の `decode_record` と同じ形（version 1・0 以上の整数の ranAtUnixMs・
    // result が completed / expired）と確認できた記録だけ時刻を比べる。それ以外
    // （`null` や未対応の形式を含む）は壊れた記録として上書きする（PR #1051 codex 指摘）。
    let existing = null;
    try {
      existing = JSON.parse(existingRaw);
    } catch {
      existing = null;
    }
    const valid =
      existing !== null &&
      typeof existing === "object" &&
      existing.version === 1 &&
      Number.isInteger(existing.ranAtUnixMs) &&
      existing.ranAtUnixMs >= 0 &&
      (existing.result === "completed" || existing.result === "expired");
    if (valid && existing.ranAtUnixMs > ranAtUnixMs) {
      shouldWrite = false;
    }
  }
  if (shouldWrite) {
    const record = JSON.stringify({
      version: 1,
      ranAtUnixMs,
      result: "completed",
    });
    store.put(record, RECORD_KEY);
  }
  store.delete(PENDING_KEY);

  await new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    // error を伴わない中断（abort）でも Promise を確定させ、失敗をブラウザの
    // 再試行へ伝える（PR #1051 codex 指摘）。
    tx.onabort = () => reject(tx.error ?? new Error("transaction aborted"));
  });
}

self.addEventListener("sync", (event) => {
  if (event.tag !== SYNC_TAG) {
    return;
  }
  // 失敗は成功を装わずそのまま reject する。ブラウザの再試行に任せる
  // （device-bridge-contracts.md「6. 嘘の成功」）。
  event.waitUntil(runOnce());
});
