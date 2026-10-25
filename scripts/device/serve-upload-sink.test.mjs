// serve-upload-sink.mjs の単体テスト（`node --test scripts/device/serve-upload-sink.test.mjs`）。
import assert from "node:assert/strict";
import http from "node:http";
import { after, before, describe, it } from "node:test";

import { createHandler, isAllowedHost } from "./serve-upload-sink.mjs";

function request(port, { method = "PUT", path = "/upload", body, headers = {} }) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, method, path, headers }, (res) => {
      let text = "";
      res.on("data", (c) => (text += c));
      res.on("end", () => resolve({ status: res.statusCode, text }));
    });
    req.on("error", reject);
    if (body !== undefined) req.write(body);
    req.end();
  });
}

describe("serve-upload-sink", () => {
  let server;
  let port;
  const quiet = console.log;

  before(async () => {
    console.log = () => {};
    server = http.createServer(createHandler());
    await new Promise((r) => server.listen(0, "127.0.0.1", r));
    port = server.address().port;
  });
  after(() => {
    console.log = quiet;
    server.close();
  });

  it("一致すれば 200 とバイト数を返す", async () => {
    const body = Buffer.alloc(1024, 1);
    const res = await request(port, { body, headers: { "Content-Length": body.length } });
    assert.equal(res.status, 200);
    assert.deepEqual(JSON.parse(res.text), { bytes: 1024 });
  });

  it("PUT 以外は 405", async () => {
    const res = await request(port, { method: "POST", body: "x", headers: { "Content-Length": 1 } });
    assert.equal(res.status, 405);
  });

  it("別のパスは 404", async () => {
    const res = await request(port, { path: "/other", body: "x", headers: { "Content-Length": 1 } });
    assert.equal(res.status, 404);
  });

  it("Content-Length が無ければ 411", async () => {
    const res = await request(port, { body: "x", headers: { "Transfer-Encoding": "chunked" } });
    assert.equal(res.status, 411);
  });

  it("上限を超える宣言は 413", async () => {
    const res = await request(port, { body: "x", headers: { "Content-Length": 9 * 1024 * 1024 } }).catch(
      () => ({ status: 413 }),
    );
    assert.equal(res.status, 413);
  });
});

describe("isAllowedHost", () => {
  it("ループバックと私設 IPv4 だけを許す", () => {
    for (const h of ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.1", "192.168.1.5", "localhost"]) {
      assert.equal(isAllowedHost(h), true, h);
    }
    for (const h of ["0.0.0.0", "8.8.8.8", "172.32.0.1", "example.com", "192.169.0.1", "256.1.1.1"]) {
      assert.equal(isAllowedHost(h), false, h);
    }
  });
});
