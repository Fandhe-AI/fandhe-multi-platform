//! 取得先 URL の検証と応答本文の上限（`check_url`・`MAX_BODY_BYTES`）。
//!
//! HTTP 取得の契約・共通実装は機能 crate `fandhe-cap-http-get` へ移した（Issue #1186）。ここに残すのは、
//! 未移行の背景ダウンロード・アップロード（`background_download`・`background_upload`）も使う共有部分だけ
//! （`docs/architecture/consumer-app-structure.md` §8。移行が済めば機能 crate 側へ寄せる）。wasm32 でも使える。

use crate::DeviceError;

/// 応答本文の上限（メモリ枯渇防止、security.md A05）。`fandhe-web-dom` の Web 実装
/// （Issue #985）と共有するため wasm32 も含めた全ターゲットへ公開する。
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

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

/// `parse_items` は wasm32 の `fandhe-cap-http-get` の Web 実装からも呼ばれるため、
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
mod host_tests {
    use super::*;

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
