//! LAN のサービス探索（mDNS / Bonjour）の抽象（Phase 11 の拡充ループ、親 #109、
//! 対応表 `docs/device/capabilities/lan-discovery.json`）。
//!
//! 扱う範囲は「同じネットワークで `_http._tcp` を公開している機器を一定時間
//! （[`BROWSE_DURATION_MS`]）探し、サービス名・IP アドレス 1 件・ポートを一覧で返す」ことだけ。
//! 接続・サービスの公開（register）・任意のサービス種別の探索は扱わない。対象は
//! iOS / Android / macOS / Windows。Web は公開 API が無く対象外。
//!
//! BLE スキャン（`bluetooth_le`）と同じ「固定窓の探索」で、窓が閉じたら一覧を 1 回だけ返す。
//! 継続配信はしない。request/response 型のため、橋渡し層（`fandhe-egui-proxy` の `device/`）は
//! `RequestSlot` で要求追跡を行う前提。各 OS 実装が集めた生の結果は [`normalize_services`] で
//! 契約の形（同名まとめ・名前昇順・件数上限）に整える。後続の OS 実装（Windows・iOS・Android）も
//! この契約のまま実装できる。
//!
//! # 契約
//!
//! - **結果の 1 件**: サービス名・IP アドレス（無ければ `None`）・ポート（無ければ `None`）。
//!   未解決のサービスは `addr` / `port` が `None` の成功行として返す（0 などの番兵にしない、
//!   `device-bridge-contracts.md`「8. 秘密値」）
//! - **同じ名前は 1 件**: インタフェースごとに同じサービスが届くため名前でまとめる。先に得た
//!   `Some` を残し、後から来た `None` で消さない
//! - **並び**: 名前の昇順。**件数上限**: 並べた後の先頭 [`MAX_SERVICES`] 件
//! - **0 件は成功**: 空リストを `Ok` で返す。失敗にしない
//! - **失敗の区別**: ローカルネットワークの許可なし → `Err(PermissionDenied)`、ネットワーク未接続 →
//!   `Err(Disabled)`、非対応 OS → `Err(Unsupported)`、中断・起動失敗 → `Err(Internal)`。
//!   いずれも空リストの成功へ畳まない（同「6. 嘘の成功」）。失敗時は部分結果を捨てる
//! - **キャンセル・三値**: 利用者が取り消す操作は無い。このため結果は成功（0 件を含む）と失敗の
//!   2 値で、`Cancelled` は返さない。中断（Activity の破棄・非前景での開始など）は失敗
//!   （`Internal`）として確定する（同「10. 三値」）
//! - **再要求は置き換え**: 探索中の新しい要求は古い要求を置き換え、古い世代の結果・完了通知は
//!   照合して捨てる（同「2. 並行性・世代」）
//! - **期限**: 窓は [`BROWSE_DURATION_MS`] だけで、実装側が呼び出しの開始を起点に単調時計で
//!   測る（同「3. 期限・キャンセル」）。呼び出し側で別の期限を持たない
//! - **画面を離れる・終了時**: 実装は進行中の探索を止め、確保したハンドルを解放する
//!   （同「1」「9」）
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、全経路での資源の解放。
//! 受付は OS ごとに 1 本の直列キューへ集める（Apple は serial queue、Android は main Handler、
//! Windows はワーカー。同「11」）。守らない範囲:
//!
//! - 窓の間に公開をやめたサービスが残ること
//! - 窓の終わりまでに解決が済まないこと、IP を 1 件しか出さないこと
//! - iOS で許可ダイアログの回答前に窓が閉じると「許可なし」と出ること
//! - ファイアウォール・ネットワークの設定で 0 件になること
//! - 複数プロセスが同時に探索したときの結果の違い
//!
//! サービス名とアドレスはログに出さない（同「8」、security.md A09）。

use std::collections::HashMap;
use std::net::IpAddr;

use crate::DeviceError;

/// 探索するサービス種別（固定。任意の種別は受けない）。
pub const SERVICE_TYPE: &str = "_http._tcp";
/// 1 回の探索窓（ミリ秒）。
pub const BROWSE_DURATION_MS: u64 = 10_000;
/// 返すサービス数の上限。
pub const MAX_SERVICES: usize = 50;
/// サービス名の最大バイト数（DNS-SD のインスタンス名は 63 バイトだが、余裕を持たせた上限）。
pub const MAX_NAME_BYTES: usize = 255;

/// 探索で見つかったサービス 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanService {
    /// サービスのインスタンス名（人が読む名前）。
    pub name: String,
    /// 解決できた IP アドレス 1 件（IPv4 優先）。未解決は `None`。
    pub addr: Option<IpAddr>,
    /// 解決できたポート。未解決は `None`（0 も `None`）。
    pub port: Option<u16>,
}

/// LAN のサービス探索の契約（モジュール doc の「契約」節）。
pub trait LanDiscovery: Send + Sync {
    /// 約 [`BROWSE_DURATION_MS`] 探索して [`normalize_services`] 済みの一覧を返す。
    fn browse(&self) -> Result<Vec<LanService>, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedLanDiscovery;

impl LanDiscovery for UnsupportedLanDiscovery {
    fn browse(&self) -> Result<Vec<LanService>, DeviceError> {
        Err(DeviceError::Unsupported("lan_discovery"))
    }
}

/// 観測順の生の結果を契約の形（同名を 1 件にまとめ、名前昇順、上限 [`MAX_SERVICES`]）へ整える。
/// 先に得た `Some` を残し、後から来た `None` で消さない。
pub fn normalize_services(observed: impl IntoIterator<Item = LanService>) -> Vec<LanService> {
    let mut merged: HashMap<String, LanService> = HashMap::new();
    for s in observed {
        match merged.get_mut(&s.name) {
            Some(prev) => {
                prev.addr = prev.addr.or(s.addr);
                prev.port = prev.port.or(s.port);
            }
            None => {
                merged.insert(s.name.clone(), s);
            }
        }
    }
    let mut out: Vec<LanService> = merged.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(MAX_SERVICES);
    out
}

/// OS から受けたアドレス文字列を検証する。不正（空・IP でない）なら `None`。
pub fn parse_addr(s: &str) -> Option<IpAddr> {
    s.trim().parse().ok()
}

/// ポート 0（未取得）を `None` にする。
pub fn port_from(raw: u16) -> Option<u16> {
    (raw != 0).then_some(raw)
}

/// 名前を [`MAX_NAME_BYTES`] 以内に文字境界で切る。
pub fn clamp_name(mut name: String) -> String {
    if name.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    name
}

/// Apple・Android 実装の完了コード → 契約のエラー（SSOT。OS 側で二重に写像しない）。
/// Windows は独自のステータスを windows の子が別に写す。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 成功 | `Ok(())` |
/// | 1 | 許可なし | `PermissionDenied` |
/// | 2 | ネットワーク未接続 | `Disabled` |
/// | 3 | 非対応 | `Unsupported` |
/// | 4 | 中断 | `Internal` |
/// | 5 | 種別が未宣言（NoAuth） | `Internal` |
/// | それ以外 | 未知 | `Internal` |
pub fn decode_code(code: i32) -> Result<(), DeviceError> {
    const F: &str = "lan_discovery";
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::PermissionDenied(F)),
        2 => Err(DeviceError::Disabled(F)),
        3 => Err(DeviceError::Unsupported(F)),
        4 => Err(DeviceError::Internal("lan_discovery interrupted")),
        5 => Err(DeviceError::Internal("lan_discovery not declared")),
        _ => Err(DeviceError::Internal("lan_discovery outcome")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, addr: Option<&str>, port: Option<u16>) -> LanService {
        LanService {
            name: name.into(),
            addr: addr.and_then(parse_addr),
            port,
        }
    }

    #[test]
    fn merges_same_name_keeping_some_and_not_erasing_with_none() {
        let out = normalize_services([
            svc("a", Some("192.168.0.2"), Some(80)),
            svc("a", None, None),
        ]);
        assert_eq!(out, vec![svc("a", Some("192.168.0.2"), Some(80))]);
        let out = normalize_services([svc("a", None, None), svc("a", Some("10.0.0.1"), Some(8))]);
        assert_eq!(out, vec![svc("a", Some("10.0.0.1"), Some(8))]);
    }

    #[test]
    fn sorts_by_name_and_caps() {
        let out = normalize_services([svc("b", None, None), svc("a", None, None)]);
        assert_eq!(out[0].name, "a");
        assert!(normalize_services([]).is_empty());
        let many = (0..MAX_SERVICES + 5).map(|i| svc(&format!("{i:03}"), None, None));
        assert_eq!(normalize_services(many).len(), MAX_SERVICES);
    }

    #[test]
    fn parse_addr_rejects_invalid() {
        assert!(parse_addr("").is_none());
        assert!(parse_addr("999.1.1.1").is_none());
        assert!(parse_addr("not-an-ip; rm -rf").is_none());
        assert!(parse_addr("192.168.0.1").is_some());
        assert!(parse_addr("fe80::1").is_some());
    }

    #[test]
    fn port_zero_is_none() {
        assert_eq!(port_from(0), None);
        assert_eq!(port_from(80), Some(80));
    }

    #[test]
    fn clamp_name_cuts_on_char_boundary() {
        let long = "あ".repeat(200); // 600 バイト
        let out = clamp_name(long);
        assert!(out.len() <= MAX_NAME_BYTES);
        assert!(out.chars().all(|c| c == 'あ'));
        assert_eq!(clamp_name("short".into()), "short");
    }

    #[test]
    fn decode_code_table() {
        assert!(decode_code(0).is_ok());
        assert!(matches!(
            decode_code(1),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(decode_code(2), Err(DeviceError::Disabled(_))));
        assert!(matches!(decode_code(3), Err(DeviceError::Unsupported(_))));
        for c in [4, 5, 99, -1] {
            assert!(matches!(decode_code(c), Err(DeviceError::Internal(_))));
        }
    }

    #[test]
    fn unsupported_browse_is_error_not_empty() {
        assert!(matches!(
            UnsupportedLanDiscovery.browse(),
            Err(DeviceError::Unsupported("lan_discovery"))
        ));
    }
}
