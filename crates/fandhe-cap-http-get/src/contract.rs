//! HTTP 取得（GET 限定）の契約と共通実装（http-get。機能 crate へ移行、Issue #1183・#1186）。
//!
//! [`Http`] は固定 URL へ `GET` する最小契約、[`UreqHttp`] は `ureq` + rustls による desktop 向けの共通実装。
//! 取得先 URL の検証（`fandhe_device::http::check_url`）と本文上限（`MAX_BODY_BYTES`）は、
//! 未移行の背景ダウンロード・アップロードも使うため `fandhe-device` に残し、ここから参照する
//! （共有コードの扱いは `docs/architecture/consumer-app-structure.md` §8）。
//! `UreqHttp` は wasm32 では存在しない（`getrandom` が解決できないため）。Web は `web` モジュールが
//! ブラウザの `fetch` で実装し、[`TIMEOUT`]・[`MAX_ITEMS`]・[`parse_items`] は desktop と共有する。

#[cfg(not(target_arch = "wasm32"))]
use fandhe_device::http::{MAX_BODY_BYTES, check_url};

use crate::error::HttpGetError;

/// URL 取得の契約。実装はスレッド間で共有されうる（`Storage` と同じ理由、#745 参照）。
pub trait Http: Send + Sync {
    /// `url` へ `GET` する。許可されるのは `https://` 全般と、ループバックホスト
    /// （`127.0.0.1` / `::1` / `localhost`）または RFC 1918 私設 IPv4 リテラル宛ての
    /// `http://` のみ（それ以外は `HttpGetError::Http` を返す、security.md A02 準拠。
    /// 私設 IPv4 許可の経緯は Issue #660。この平文許可は開発・実機検証用の固定 URL
    /// を前提とし、実行時のユーザー入力 URL には適用しない。SSRF の到達範囲が
    /// 私設網まで広がるため）。応答本文には実装ごとの上限があり、
    /// リダイレクトは追跡しない契約（`UreqHttp` のコメント参照）。
    fn get(&self, url: &str) -> Result<Vec<u8>, HttpGetError>;
}

/// タイムアウト（無応答での無限待機防止、security.md A05）。`fandhe-web-dom` の Web 実装
/// （Issue #985、`AbortController` の期限に使う）と共有するため wasm32 も含めた
/// 全ターゲットへ公開する。`Duration` は `fandhe-core` 同様プラットフォーム非依存の型。
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// 一覧の表示件数上限（`fandhe-egui-proxy::device::http_fetch` から移設、Issue #985）。
// ponytail: 8MB 上限の body を全行描画しない保険。件数が要件になったら VirtualList 化。
pub const MAX_ITEMS: usize = 100;

/// `http.get(url)` の応答（JSON 配列）を `Vec<String>` へ変換する（`fandhe-egui-proxy` と
/// `fandhe-web-dom` の両方が呼ぶ共有ロジック、Issue #985）。エラーメッセージに応答本文・
/// URL 全体は含めない（security.md A09 ログ方針）。`serde_json` のエラー文字列は入力値
/// （応答本文の一部）を含み得るため `err.to_string()` をそのまま返さず、固定の解析失敗
/// メッセージへ変換する（画面の `status_text` へそのまま表示されるため、応答本文の
/// 漏えいを防ぐ）。
pub fn parse_items(body: &[u8]) -> Result<Vec<String>, String> {
    let items: Vec<String> =
        serde_json::from_slice(body).map_err(|_| "応答の解析に失敗しました".to_string())?;
    Ok(items.into_iter().take(MAX_ITEMS).collect())
}

/// `ureq` + rustls（ring バックエンド）による共通 `Http` 実装（desktop 向け）。
/// 選定理由はルート `Cargo.toml` の `ureq` エントリのコメントを参照。
#[cfg(not(target_arch = "wasm32"))]
pub struct UreqHttp {
    agent: ureq::Agent,
}

#[cfg(not(target_arch = "wasm32"))]
impl UreqHttp {
    /// 既定タイムアウト（`TIMEOUT`）・自動リダイレクト無効の `Agent` を構築する。
    /// 前提条件なし（引数を取らず、失敗しない構築）。
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // リダイレクト先 URL はスキーム/ホスト検証（`get` 冒頭）を経由しないため、
            // ureq の既定の自動追跡（既定 10 回）を無効化する。追跡したままだと
            // 許可したループバック URL が 302 で外部ホストの平文 http へ誘導した場合に
            // 検証を迂回して通信してしまう（P1 指摘対応、security.md A02）。
            .max_redirects(0)
            // ureq の既定は `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 環境変数を読み、
            // それらが設定された環境では検証済みホストではなく外部プロキシへ転送されて
            // しまい、ループバック限定チェック（`get` 冒頭）を迂回しうる。プロキシを
            // 明示的に無効化する（Bugbot 指摘対応、security.md A02）。
            .proxy(None)
            .build()
            .into();
        Self { agent }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// `fandhe_device::DeviceError::Http(msg)` の `msg` だけを取り出す（`check_url` の失敗を契約のエラーへ写す）。
/// `Http` 以外のバリアントは `check_url` から返らないが、握りつぶさず `Display` の文言を残す。
#[cfg(not(target_arch = "wasm32"))]
fn http_message(e: fandhe_device::DeviceError) -> String {
    match e {
        fandhe_device::DeviceError::Http(m) => m,
        other => other.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, HttpGetError> {
        check_url(url).map_err(|e| HttpGetError::Http(http_message(e)))?;
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| HttpGetError::Http(e.to_string()))?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|e| HttpGetError::Http(e.to_string()))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 固定 HTTP/1.1 応答を 1 回返すローカルサーバー（ダミー URL・実ネットワーク非依存）。
    fn spawn_fixed_response_server(body: &'static str) -> u16 {
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

    #[test]
    fn get_returns_body_from_local_server() {
        let port = spawn_fixed_response_server("hello");
        let http = UreqHttp::new();
        let body = http
            .get(&format!("http://127.0.0.1:{port}/"))
            .expect("get は成功する");
        assert_eq!(body, b"hello");
    }

    #[test]
    fn get_fails_on_connection_refused() {
        // OS に割り当てさせたポートへ即座に接続を試み、listen していないため拒否される
        // ことを期待する（固定ポート番号への依存を避ける）。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind は成功する");
        let port = listener.local_addr().expect("local_addr は成功する").port();
        drop(listener);
        let http = UreqHttp::new();
        assert!(http.get(&format!("http://127.0.0.1:{port}/")).is_err());
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        let http = UreqHttp::new();
        assert!(matches!(
            http.get("ftp://example.invalid/"),
            Err(HttpGetError::Http(_))
        ));
    }

    #[test]
    fn external_plain_http_is_rejected() {
        // security.md A02: 通信は TLS 前提。外部宛ての平文 http はネットワークへ
        // 出る前に拒否する（P1 指摘の回帰テスト）。
        let http = UreqHttp::new();
        assert!(matches!(
            http.get("http://example.com/"),
            Err(HttpGetError::Http(_))
        ));
    }

    #[test]
    fn redirect_to_external_host_is_not_followed() {
        // ループバック宛ての最初のリクエストは許可されるが、応答が外部ホストへの
        // 302 を返しても ureq の自動追跡を無効化しているため外部へは遷移しない
        // （P1 指摘の回帰テスト。追跡していれば body に "external" が入ってしまう）。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind は成功する");
        let port = listener.local_addr().expect("local_addr は成功する").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = "HTTP/1.1 302 Found\r\nLocation: http://example.com/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let http = UreqHttp::new();
        let body = http
            .get(&format!("http://127.0.0.1:{port}/"))
            .expect("追跡を無効化した 302 応答自体の取得は成功する");
        assert!(body.is_empty());
    }

    #[test]
    fn proxy_is_disabled() {
        // Bugbot 指摘の回帰テスト: ureq の既定は `HTTP_PROXY` 等の環境変数からプロキシを
        // 読む（`Proxy::try_from_env()`）ため、明示的に無効化していないとループバック
        // 限定チェックを迂回して外部プロキシへ転送されうる。crate 全体が
        // `#![forbid(unsafe_code)]` のためプロセス環境変数の書き換え（`unsafe` 必須）は
        // 使わず、構築した `Agent` の設定を直接検査する。
        let http = UreqHttp::new();
        assert!(http.agent.config().proxy().is_none());
    }
}

#[cfg(test)]
mod parse_items_tests {
    use super::*;

    #[test]
    fn parses_string_array() {
        let body = r#"["りんご","みかん"]"#.as_bytes();
        assert_eq!(
            parse_items(body),
            Ok(vec!["りんご".to_string(), "みかん".to_string()])
        );
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_items(b"not json").is_err());
    }

    #[test]
    fn truncates_to_max_items() {
        let items: Vec<String> = (0..MAX_ITEMS + 1).map(|i| i.to_string()).collect();
        let body = serde_json::to_vec(&items).expect("serialize は成功する");
        let parsed = parse_items(&body).expect("成功する");
        assert_eq!(parsed.len(), MAX_ITEMS);
    }
}
