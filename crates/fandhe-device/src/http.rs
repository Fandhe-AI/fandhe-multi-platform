//! HTTP 取得の抽象（Phase 8 / Issue #746）。`fandhe-egui-proxy` 側の画面が
//! 固定 URL（ダミー）へ `GET` するだけの最小契約。
//!
//! `UreqHttp` は wasm32 では `getrandom` が解決できず存在しない
//! （ルート Cargo.toml の `ureq` 選定コメント参照）。wasm32 では `Http` トレイトの
//! 実装自体を提供しない（呼び出し元は feature `device` ごと wasm32 に持ち込まない）。
//! Web（`fandhe-web-dom::http_fetch`）はブラウザの `fetch` API 経由で独自に実装するが、
//! 本文上限・タイムアウト・件数上限・JSON 解析（[`MAX_BODY_BYTES`] / [`TIMEOUT`] /
//! [`MAX_ITEMS`] / [`parse_items`]）は desktop 実装と同じ値・同じロジックを共有する
//! （Issue #985、二重管理を避ける SSOT 方針）。

use crate::DeviceError;

/// URL 取得の契約。実装はスレッド間で共有されうる（`Storage` と同じ理由、#745 参照）。
pub trait Http: Send + Sync {
    /// `url` へ `GET` する。許可されるのは `https://` 全般と、ループバックホスト
    /// （`127.0.0.1` / `::1` / `localhost`）または RFC 1918 私設 IPv4 リテラル宛ての
    /// `http://` のみ（それ以外は `DeviceError::Http` を返す、security.md A02 準拠。
    /// 私設 IPv4 許可の経緯は Issue #660。この平文許可は開発・実機検証用の固定 URL
    /// を前提とし、実行時のユーザー入力 URL には適用しない。SSRF の到達範囲が
    /// 私設網まで広がるため）。応答本文には実装ごとの上限があり、
    /// リダイレクトは追跡しない契約（`UreqHttp` のコメント参照）。
    fn get(&self, url: &str) -> Result<Vec<u8>, DeviceError>;
}

/// 応答本文の上限（メモリ枯渇防止、security.md A05）。`fandhe-web-dom` の Web 実装
/// （Issue #985）と共有するため wasm32 も含めた全ターゲットへ公開する。
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

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

/// 取得先 URL の検証（security.md A02・A03）。許可するのは `https://`（ホストが空でないもの）と、
/// ループバック・RFC 1918 私設 IPv4 宛ての `http://`（実機テストで Mac の LAN IP から取得するため、
/// Issue #660）のみ。ASCII の空白・制御文字を含む URL は拒否する。エラーに URL 全体は含めない（A09）。
/// `UreqHttp::get` と `background_download`（OS 管理の転送、Issue #390）が共有し、wasm32 でも使える。
pub fn check_url(url: &str) -> Result<(), DeviceError> {
    let ok = !url
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        && match extract_host(url) {
            Some(_) if url.starts_with("https://") => true,
            Some(host) if url.starts_with("http://") => is_local_plain_http_host(host),
            _ => false,
        };
    if ok {
        Ok(())
    } else {
        Err(DeviceError::Http("unsupported scheme or host".to_string()))
    }
}

/// `url` の authority 部分（ユーザー情報・ポートを除いたホスト名）を取り出す。
/// スキームを含まない・authority が空等の不正な形式は `None`。
fn extract_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next()
    } else {
        let host = authority.split(':').next().unwrap_or(authority);
        (!host.is_empty()).then_some(host)
    }
}

/// ループバックホスト（ローカル検証専用サーバー）かどうかを判定する。
fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost")
}

/// ループバック、または RFC 1918 私設 IPv4 リテラル（`10.0.0.0/8` /
/// `172.16.0.0/12` / `192.168.0.0/16`）宛てかどうかを判定する。実機テストで
/// iPhone/Android 実機から Mac の LAN IP へ平文 http で取得しに行くケースを
/// 許可するため（Issue #660）。ホスト名（`.local` 含む）・公開 IP・IPv6 の
/// 私設アドレスは対象外（`Ipv4Addr` の parse に失敗し `false` になる）。
fn is_local_plain_http_host(host: &str) -> bool {
    is_loopback_host(host)
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|addr| addr.is_private())
}

#[cfg(not(target_arch = "wasm32"))]
impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, DeviceError> {
        check_url(url)?;
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| DeviceError::Http(e.to_string()))?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|e| DeviceError::Http(e.to_string()))
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
            Err(DeviceError::Http(_))
        ));
    }

    #[test]
    fn external_plain_http_is_rejected() {
        // security.md A02: 通信は TLS 前提。外部宛ての平文 http はネットワークへ
        // 出る前に拒否する（P1 指摘の回帰テスト）。
        let http = UreqHttp::new();
        assert!(matches!(
            http.get("http://example.com/"),
            Err(DeviceError::Http(_))
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

    #[test]
    fn loopback_plain_http_is_allowed_by_scheme_check() {
        // ローカル検証用サーバー宛ての平文 http はスキーム/ホスト検証を通過する
        // （実際に接続できることは get_returns_body_from_local_server で確認済み）。
        assert!(is_loopback_host(
            extract_host("http://127.0.0.1:8080/").unwrap()
        ));
        assert!(is_loopback_host(
            extract_host("http://localhost/path").unwrap()
        ));
        assert!(!is_loopback_host(
            extract_host("http://example.com/").unwrap()
        ));
    }

    #[test]
    fn private_ipv4_plain_http_is_allowed() {
        // 実機テストで Mac の LAN IP（RFC 1918）宛てに取得しに行くケースを許可する
        // （Issue #660）。
        assert!(is_local_plain_http_host("192.168.1.2"));
        assert!(is_local_plain_http_host("10.0.0.5"));
        assert!(is_local_plain_http_host("172.16.0.1"));
    }

    #[test]
    fn non_private_hosts_remain_rejected() {
        // 172.16.0.0/12 の範囲外（172.32.0.0 以降）・公開 IP・ホスト名（mDNS の
        // .local 含む）は許可対象に含めない。
        assert!(!is_local_plain_http_host("172.32.0.1"));
        assert!(!is_local_plain_http_host("8.8.8.8"));
        assert!(!is_local_plain_http_host("example.com"));
        assert!(!is_local_plain_http_host("mac.local"));
        // リンクローカル・CGNAT は `Ipv4Addr::is_private()` の範囲外。手書き判定へ
        // 置き換えた際の回帰検知用に明示しておく。
        assert!(!is_local_plain_http_host("169.254.1.1"));
        assert!(!is_local_plain_http_host("100.64.0.1"));
    }
}

/// `parse_items` は wasm32 の `fandhe-web-dom::http_fetch` 実装からも呼ばれるため、
/// desktop 限定の `mod tests`（`UreqHttp` を使う）とは別に全ターゲットで通す
/// （Issue #985）。
#[cfg(test)]
mod check_url_tests {
    use super::*;

    #[test]
    fn allows_https_loopback_and_private_http() {
        for u in [
            "https://example.com/a",
            "http://127.0.0.1:8787/items.json",
            "http://localhost/x",
            "http://192.168.0.5/x",
        ] {
            assert!(check_url(u).is_ok(), "{u}");
        }
    }

    #[test]
    fn rejects_public_http_other_schemes_empty_host_and_whitespace() {
        for u in [
            "http://example.com/",
            "ftp://127.0.0.1/",
            "https:///x",
            "http://127.0.0.1/a b",
            "https://example.com/\n",
        ] {
            assert!(check_url(u).is_err(), "{u}");
        }
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
