//! lan-discovery（同じネットワークの `_http._tcp` サービスを一定時間探して一覧で返す）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! iOS・macOS は Network.framework（`nw_browser`）と dns_sd、Android は `NsdManager`（Java 側 `LanDiscoveryHelper`）、Windows は Win32 `DnsServiceBrowse`。
//! 呼び出し元は `fandhe-gallery` の画面登録（`registry.rs` の `effect` 区画）が登録する effect ドライバ（`driver`）と、終了フックから呼ばれる [`stop_shared_discovery`]。
//!
//! 守る範囲: 失敗（許可なし・未接続・非対応・中断）を空リストの成功へ畳まないこと、新しい要求が古い要求を置き換えること、期限（[`BROWSE_DURATION_MS`]）の単調時計での計測、全経路での探索の停止と資源の解放。
//! 守らない範囲: 窓の間に公開をやめたサービスの残り、窓内に解決が済まない場合の欠落（IP は 1 件のみ）、ファイアウォール由来の 0 件、複数プロセスの結果差。詳細は `contract.rs` の契約を参照。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_lan_discovery()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "windows")]
mod windows;

// iOS・macOS: `apple/fandhe_apple_lan_discovery.m` が network-status の `.m` の記号
// （`fandhe_apple_network_status_read`）を呼ぶ。依存先の rlib とその `.m` がリンク対象から外れないよう、
// 依存先の公開関数を実際に参照する。
#[cfg(any(target_os = "ios", target_os = "macos"))]
const _: fn() -> std::sync::Arc<dyn fandhe_cap_network_status::NetworkStatus> =
    fandhe_cap_network_status::platform_network_status;

pub use contract::*;
pub use driver::*;
pub use error::LanDiscoveryError;
