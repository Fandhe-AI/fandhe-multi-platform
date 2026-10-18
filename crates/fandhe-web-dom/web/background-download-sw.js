// Issue #393: 型サンプル「バックグラウンドダウンロード」の Background Fetch 実体。
// 完了・失敗・取り消しの 3 イベントだけを扱い、結果を Cache Storage に書く。ページ側
// （crates/fandhe-web-dom/src/background_download.rs）が同じ key を読んで状態を決める。
//
// キャッシュ名・key・結果の形はページ側と一致させること。世代は `current` の id で照合し、
// 書いた後でもう一度 `current` を読んで食い違えば本文を消す（取り消し・置き換えと競合しても
// 古い世代の完了が残らない）。`fetch` ハンドラは持たない（他画面の通信に介入しない）。
// 合否の判定は Rust 側の `completion_status` が行う（SSOT）。ここでは観測値を記録するだけ。
// `importScripts`・外部 CDN は使わない（security.md A08）。

const CACHE_NAME = "fandhe-background-download";
const KEY_CURRENT = "fandhe-background-download-current";
const KEY_RESULT = "fandhe-background-download-result";
const KEY_FILE = "background-download";

async function currentId(cache) {
  const r = await cache.match(KEY_CURRENT);
  if (!r) return null;
  try {
    return (await r.json()).id ?? null;
  } catch {
    return null;
  }
}

function putJson(cache, key, value) {
  return cache.put(key, new Response(JSON.stringify(value)));
}

// 完了結果を書く。2xx かつ 1 バイト以上のときだけ本文を保存する。
async function onSuccess(reg) {
  const cache = await caches.open(CACHE_NAME);
  if ((await currentId(cache)) !== reg.id) return;
  const record = (await reg.matchAll())[0];
  const response = record && (await record.responseReady);
  const result = { id: reg.id, kind: "failed", reason: "fetch-error" };
  if (response) {
    const blob = await response.blob();
    Object.assign(result, {
      kind: "completed",
      status: response.status,
      bytes: blob.size,
      // 最終 URL（リダイレクト後）。画面には出さず、Rust 側が検証し直す。
      url: response.url || record.request.url,
    });
    delete result.reason;
    if (response.status >= 200 && response.status < 300 && blob.size > 0) {
      await cache.put(KEY_FILE, new Response(blob));
    }
  }
  // 取り消しの「cancelled」記録を古い結果で上書きしないよう、書く直前に再確認する。
  if ((await currentId(cache)) !== reg.id) {
    await cache.delete(KEY_FILE);
    return;
  }
  await putJson(cache, KEY_RESULT, result);
}

async function onFail(reg) {
  const cache = await caches.open(CACHE_NAME);
  if ((await currentId(cache)) !== reg.id) return;
  await putJson(cache, KEY_RESULT, {
    id: reg.id,
    kind: "failed",
    reason: reg.failureReason,
  });
}

// ページの取り消しは先に `current` を差し替えるため、ここへ来る（id が一致する）のは
// ブラウザ UI からの取り消しだけ。
async function onAbort(reg) {
  const cache = await caches.open(CACHE_NAME);
  if ((await currentId(cache)) !== reg.id) return;
  await cache.delete(KEY_FILE);
  await putJson(cache, KEY_RESULT, { id: reg.id, kind: "cancelled" });
}

self.addEventListener("backgroundfetchsuccess", (event) => {
  const reg = event.registration;
  event.waitUntil(
    onSuccess(reg).catch(() => onFail({ id: reg.id, failureReason: "" })),
  );
});
self.addEventListener("backgroundfetchfail", (event) => {
  event.waitUntil(onFail(event.registration));
});
self.addEventListener("backgroundfetchabort", (event) => {
  event.waitUntil(onAbort(event.registration));
});
