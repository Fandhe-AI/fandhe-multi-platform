# 型サンプルの実機確認（http_fetch）

`http_fetch` 型サンプルを実機（iPhone・Android 実機）で動かし、ループバックのダミー API を届かせるための手順を定める。

`http_fetch` 型サンプルはループバック（`127.0.0.1:8787`）のダミー API を既定の
取得先にしているため、実機（iPhone・Android 実機）からはそのままでは届きません
（Issue #660）。

1. Mac でダミー API を起動する: `node scripts/web/serve-brotli.mjs
   crates/fandhe-egui-proxy/fixtures/http_fetch 8787`（127.0.0.1 固定）。iPhone から
   直接 LAN 経由で取りに来る場合は `python3 -m http.server 8787 --bind 0.0.0.0
   --directory crates/fandhe-egui-proxy/fixtures/http_fetch` を使う。信頼できる LAN
   でのみ実行し、確認後は停止する
2. Android: `adb reverse tcp:8787 tcp:8787` で端末の `127.0.0.1:8787` を Mac へ
   転送する（コード変更不要）。ビルドは `FANDHE_PROXY_APP=http_fetch`。確認後は
   `adb reverse --remove tcp:8787` で転送を外す
3. iPhone: `FANDHE_PROXY_APP=http_fetch FANDHE_ITEMS_URL=http://<Mac の LAN
   IP>:8787/items.json scripts/ios/verify-device-launch.sh`。`FANDHE_ITEMS_URL`
   指定時のみ `.app` の Info.plist に LAN 限定の ATS 例外
   （`NSAllowsLocalNetworking`）が入る。ビルド後は `unset FANDHE_ITEMS_URL`
