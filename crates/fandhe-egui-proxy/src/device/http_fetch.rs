//! `fandhe_core::screens::http_fetch`（Issue #748）の effect ドライバ。画面の
//! `FetchPhase::Requested` を観測し、`fandhe_device::http::UreqHttp` で
//! ローカル検証用ダミー API を別スレッドから取得、`CompletionSender` 経由で
//! `mark_loading` / `finish_fetch` を送る（`crate::screens::registry` の `effect` 区画
//! から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! 依存方向はこのモジュール → `fandhe_device` の一方向（`fandhe-core` は端末依存ゼロ）。

use fandhe_core::screens::http_fetch::{finish_fetch, mark_loading, FetchPhase, HttpFetchState};
use fandhe_device::http::{Http, UreqHttp};

use crate::app::CompletionSender;

/// ローカル検証用ダミー API の URL（受入 2: `node scripts/web/serve-brotli.mjs`
/// 等で 8787 番へ立てる想定、docs/operations/http-fetch-device-check.md 参照）。文言・URL はダミー。
/// 実機テスト（iPhone から Mac の LAN IP へ取得しに行く場合等）ではループバックへ
/// 到達できないため、ビルド時 `FANDHE_ITEMS_URL` で差し替え可能にする（Issue #660）。
/// `screens.rs` の `FANDHE_PROXY_APP`（画面切り替え）と同じ `option_env!` 方式を使うのは、
/// rustc がこの環境変数への依存をコンパイル成果物に記録し、値を変えれば再ビルドが
/// 走るようになるため（build.rs で `cargo:rerun-if-env-changed` を書く必要がない）。
const ITEMS_URL: &str = match option_env!("FANDHE_ITEMS_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8787/items.json",
};

/// `url` が `http://<host>...` 形式（スキームの後にホスト部が 1 文字以上続き、
/// ASCII 空白・制御文字を含まない）かどうかを判定する。`FANDHE_ITEMS_URL` の
/// ビルド時検証（下の `assert!`）専用の構文チェックであり、接続先ホストが
/// ループバック・私設 IP かどうかの信頼境界判定は含まない（実行時に
/// `fandhe_device::http::UreqHttp::get` が担う、security.md A02/A03）。
// `const _: () = assert!(...)` の中でしか呼ばれておらず、rustc の dead_code 解析は
// 無名 const（`_`）の初期化式を到達可能グラフに含めないため、実際には使われていても
// 誤検出される（コンパイル時アサーション定石のよくある制約）。テストからも呼ぶため
// 完全な未使用ではない。
#[allow(dead_code)]
const fn is_plain_http_url(url: &str) -> bool {
    let bytes = url.as_bytes();
    let prefix = b"http://";
    if bytes.len() <= prefix.len() {
        return false;
    }
    let mut i = 0;
    while i < prefix.len() {
        if bytes[i] != prefix[i] {
            return false;
        }
        i += 1;
    }
    // ホスト部の先頭がポート・パス・クエリ・フラグメントの区切りならホストが空。
    let first = bytes[prefix.len()];
    if first == b'/' || first == b':' || first == b'?' || first == b'#' {
        return false;
    }
    while i < bytes.len() {
        let b = bytes[i];
        if b <= 0x20 || b == 0x7f {
            return false;
        }
        i += 1;
    }
    true
}

// ビルド時検証（Issue #660）: `FANDHE_ITEMS_URL` に構文の外れた値を渡した場合、
// 実機ビルドを進めず即座に失敗させる。ここでは構文のみ検証し、接続先ホストの
// 信頼境界（ループバック・私設 IP 限定）は `fandhe_device::http::UreqHttp::get` が
// 実行時に担う。
const _: () = assert!(
    is_plain_http_url(ITEMS_URL),
    "FANDHE_ITEMS_URL は http://<host>... 形式のみ許容（ビルド時検証、Issue #660）"
);

/// `http.get(url)` の応答（JSON 配列）を `Vec<String>` へ変換する。解析・件数上限
/// （`fandhe_device::http::MAX_ITEMS`）は `fandhe-web-dom::http_fetch` と共有する
/// `fandhe_device::http::parse_items` に委譲する（Issue #985、二重管理を避ける SSOT 方針）。
/// エラーメッセージに応答本文・URL 全体は含めない（security.md A09 ログ方針、
/// `DeviceError` の踏襲）。
pub fn fetch_items(http: &dyn Http, url: &str) -> Result<Vec<String>, String> {
    let body = http.get(url).map_err(|err| err.to_string())?;
    fandhe_device::http::parse_items(&body)
}

/// `state.phase == Requested` のときだけ、`mark_loading` の完了を同期的に送り
/// （二重起動防止マーカー、`screens::http_fetch` モジュール doc 参照）、続けて
/// 別スレッドで実取得 → `finish_fetch` の完了を送る。`url` を引数化しているのは
/// テストがローカルサーバーの実ポートへ差し替えられるようにするため
/// （固定 `ITEMS_URL` へは [`http_fetch_effect`] からのみ渡す）。
fn spawn_fetch(state: &HttpFetchState, sender: &CompletionSender<HttpFetchState>, url: &str) {
    if state.phase != FetchPhase::Requested {
        return;
    }
    // 受信側（ProxyApp）がドロップ済み（アプリ終了中）の送信失敗は無視してよい
    // （coding-rust.md: panic を API 契約にしない）。
    let _ = sender.send(Box::new(mark_loading));

    let sender = sender.clone();
    let url = url.to_string();
    std::thread::spawn(move || {
        let result = fetch_items(&UreqHttp::new(), &url);
        let _ = sender.send(Box::new(move |state: &mut HttpFetchState| {
            finish_fetch(state, result);
        }));
    });
}

/// `registry.rs` の `effect` 区画から `http_fetch` 画面用に配線される effect ドライバ。
pub fn http_fetch_effect(state: &HttpFetchState, sender: &CompletionSender<HttpFetchState>) {
    spawn_fetch(state, sender, ITEMS_URL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::http_fetch::start_fetch;
    use fandhe_device::DeviceError;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// テスト用の固定応答 `Http` 実装（実ネットワークに出ない）。
    struct MockHttp(Result<Vec<u8>, String>);

    impl Http for MockHttp {
        fn get(&self, _url: &str) -> Result<Vec<u8>, DeviceError> {
            self.0.clone().map_err(DeviceError::Http)
        }
    }

    #[test]
    fn is_plain_http_url_accepts_valid_forms() {
        assert!(is_plain_http_url("http://192.168.1.2:8787/items.json"));
        assert!(is_plain_http_url(ITEMS_URL));
    }

    #[test]
    fn is_plain_http_url_rejects_non_http_scheme() {
        assert!(!is_plain_http_url("https://192.168.1.2:8787/items.json"));
    }

    #[test]
    fn is_plain_http_url_rejects_empty_host() {
        assert!(!is_plain_http_url("http://"));
        assert!(!is_plain_http_url("http:///x"));
        assert!(!is_plain_http_url("http://:8787/items.json"));
        assert!(!is_plain_http_url("http://?x"));
        assert!(!is_plain_http_url("http://#f"));
    }

    #[test]
    fn is_plain_http_url_rejects_whitespace() {
        assert!(!is_plain_http_url("http://192.168.1.2 /items.json"));
        assert!(!is_plain_http_url("http://192.168.1.2\t/items.json"));
    }

    #[test]
    fn fetch_items_parses_json_array() {
        let http = MockHttp(Ok(r#"["りんご","みかん","ぶどう"]"#.as_bytes().to_vec()));
        let items = fetch_items(&http, "http://127.0.0.1/items.json").expect("成功する");
        assert_eq!(
            items,
            vec![
                "りんご".to_string(),
                "みかん".to_string(),
                "ぶどう".to_string()
            ]
        );
    }

    #[test]
    fn fetch_items_rejects_invalid_json() {
        let http = MockHttp(Ok(b"not json".to_vec()));
        assert!(fetch_items(&http, "http://127.0.0.1/items.json").is_err());
    }

    #[test]
    fn fetch_items_propagates_http_error() {
        let http = MockHttp(Err("boom".to_string()));
        assert!(fetch_items(&http, "http://127.0.0.1/items.json").is_err());
    }

    /// 固定 JSON 応答を 1 回返すローカルサーバー（`fandhe_device::http::UreqHttp` の
    /// テストと同じ決定的手法。ポート番号は OS 割り当てのため衝突しない）。
    fn spawn_fixed_json_server(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind は成功する");
        let port = listener.local_addr().expect("local_addr は成功する").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    /// `Requested` 状態で `spawn_fetch` を呼ぶと、`mark_loading` の完了が同期的に届き、
    /// 続いて別スレッドからの `finish_fetch(Ok)` の完了が届くことを確認する
    /// （effect ドライバの実スレッド起動・完了配線の結合テスト）。
    #[test]
    fn spawn_fetch_delivers_loading_then_loaded_completions() {
        let port = spawn_fixed_json_server(r#"["x","y"]"#);
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = HttpFetchState::default();
        start_fetch(&mut state);
        assert_eq!(state.phase, FetchPhase::Requested);

        spawn_fetch(&state, &sender, &format!("http://127.0.0.1:{port}/"));

        let mark_loading_completion = rx.recv().expect("mark_loading の完了が届く");
        mark_loading_completion(&mut state);
        assert_eq!(state.phase, FetchPhase::Loading);

        let finish_completion = rx.recv().expect("finish_fetch の完了が届く");
        finish_completion(&mut state);
        assert_eq!(
            state.phase,
            FetchPhase::Loaded(vec!["x".to_string(), "y".to_string()])
        );
    }

    /// 接続を拒否するポートへの取得は `Failed` へ落ちることを確認する（失敗経路）。
    #[test]
    fn spawn_fetch_delivers_failed_completion_on_connection_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind は成功する");
        let port = listener.local_addr().expect("local_addr は成功する").port();
        drop(listener);

        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = HttpFetchState::default();
        start_fetch(&mut state);

        spawn_fetch(&state, &sender, &format!("http://127.0.0.1:{port}/"));

        rx.recv().expect("mark_loading の完了が届く")(&mut state);
        assert_eq!(state.phase, FetchPhase::Loading);

        rx.recv().expect("finish_fetch の完了が届く")(&mut state);
        assert!(matches!(state.phase, FetchPhase::Failed(_)));
    }

    /// `Idle`（`Requested` 以外）では何も送らない（二重起動防止・無関係な起動の抑止）。
    #[test]
    fn spawn_fetch_is_noop_when_not_requested() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel::<crate::app::Completion<HttpFetchState>>();
        let sender = CompletionSender::new(tx, ctx);
        let state = HttpFetchState::default();

        spawn_fetch(&state, &sender, "http://127.0.0.1:1/");

        assert!(rx.try_recv().is_err());
    }
}
