//! 近くの端末との直接通信の抽象（Phase 11 の拡充ループ、親 #110、Issue #511。
//! 対応表 `docs/device/capabilities/local-peer-connect.json`）。
//!
//! 扱う範囲は「同じアプリを開いた近くの 2 台が [`EXCHANGE_WINDOW_MS`] の窓の間に互いを見つけ、
//! [`MAX_TEXT_BYTES`] 以内の固定文字列を 1 回ずつ送り合い、相手の表示名と受け取った文字列を
//! 返す」ことだけ。3 台以上・ファイル転送・継続チャット・異なる OS 同士の接続は扱わない。
//! iOS / macOS は MultipeerConnectivity、Android は Wi-Fi Aware。Windows は保留、Web は対象外。
//!
//! 呼び出し元は `fandhe-egui-proxy` の `device/local_peer.rs`（request/response 型のため
//! `RequestSlot` で要求追跡する）。各 OS 実装は相手の名前と受信バイト列を非信頼入力として
//! [`is_valid_peer_name`] / [`sanitize_received`] で検証してから結果にする。完了コードの写像は
//! [`decode_code`]、招待側の決め方は [`should_invite`] が SSOT で、OS 側で二重に書かない。
//!
//! # 契約
//!
//! - **相手が見つからなければ `Ok(None)`**（成功）。ダミーの文字列は作らない
//!   （`device-bridge-contracts.md` 6 節）
//! - **失敗の区別**: 許可なし → `PermissionDenied`、Wi-Fi オフなど → `Disabled`、非対応 OS →
//!   `Unsupported`、中断・起動失敗 → `Internal`。失敗時は受け取り済みの結果も捨てる
//! - **三値**: 利用者が取り消す操作は無いため `Cancelled` は返さない。中断（背景遷移・
//!   Activity 破棄など）は `Internal`（同 10 節）
//! - **再要求は置き換え**: 古い世代の結果・完了通知は照合して捨てる（同 2 節）
//! - **期限**: 窓は [`EXCHANGE_WINDOW_MS`] だけ。実装側が呼び出しを起点に単調時計で測る（同 3 節）
//! - **解放**: 戻る前に advertise・browse・session を解放する（同 9 節）
//! - **受付**: OS ごとに直列キュー 1 本へ集める（Apple は serial queue、Android は main Handler。同 11 節）
//!
//! # 長すぎる入力は切り詰めずに捨てる
//!
//! 自分のアプリの送り手は [`MAX_TEXT_BYTES`] を超えない。超えた入力は正しい相手のものではないため、
//! 文字境界で切って部分表示するより捨てる（[`sanitize_received`] は `None`）。
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは 1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、全経路での資源の解放。
//! 守らない範囲:
//!
//! - 3 台以上が居るときの相手の選び方
//! - 自分の文字列が相手に届いた保証
//! - 窓の端での非対称（片方だけ受け取る）
//! - Wi-Fi Aware の欠落・重複
//! - 同じサービス種別を名乗る別インスタンスの文字列（相手は認証しない）
//! - 異なる OS 同士の接続
//! - 背景遷移中の取りこぼし
//! - 複数プロセスの同時交換
//! - 双方のトークンが一致したときの `Ok(None)`
//!
//! 表示名と受け取った文字列はログに出さない（同 8 節、security.md A09）。

use crate::DeviceError;

/// Bonjour のサービス種別（15 文字以内。Apple 実装が advertise / browse に使う）。
pub const SERVICE_TYPE: &str = "fandhe-p2p";
/// 1 回の交換窓（ミリ秒。SSOT。OS 側へ渡す）。
pub const EXCHANGE_WINDOW_MS: u64 = 30_000;
/// 送受信する文字列の最大バイト数。
pub const MAX_TEXT_BYTES: usize = 200;

const NAME_PREFIX: &str = "fandhe-";
const NAME_HEX_LEN: usize = 8;

/// 交換の結果 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerExchange {
    /// 相手の表示名（[`is_valid_peer_name`] を通った乱数トークン）。
    pub peer_name: String,
    /// 相手から受け取った文字列（[`sanitize_received`] 済み）。
    pub received: String,
}

/// 近くの端末との交換の契約（モジュール doc の「契約」節）。
pub trait LocalPeer: Send + Sync {
    /// 約 [`EXCHANGE_WINDOW_MS`] の間に相手を探して `outgoing` を送り、相手の文字列を受け取る。
    /// 相手が居なければ `Ok(None)`。
    fn exchange(&self, outgoing: &str) -> Result<Option<PeerExchange>, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedLocalPeer;

impl LocalPeer for UnsupportedLocalPeer {
    fn exchange(&self, _outgoing: &str) -> Result<Option<PeerExchange>, DeviceError> {
        Err(DeviceError::Unsupported("local_peer"))
    }
}

/// 種から表示名（`fandhe-` + 小文字 hex 8 桁）を作る。表示用で秘密値ではない。
pub fn peer_token(seed: u64) -> String {
    format!("{NAME_PREFIX}{:08x}", (seed ^ (seed >> 32)) as u32)
}

/// 相手の表示名が [`peer_token`] の形か。相手の名前は信頼できない入力なので、この形だけを通す。
pub fn is_valid_peer_name(name: &str) -> bool {
    name.strip_prefix(NAME_PREFIX).is_some_and(|hex| {
        hex.len() == NAME_HEX_LEN && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// 自分が相手を招待する側か（バイト順で `own > peer`。等しければ双方 false）。
/// 双方が同じ規則で片側だけ招待する（二重招待を避ける）。`.m` 側の `strcmp` も同じ規則（SSOT はここ）。
pub fn should_invite(own: &str, peer: &str) -> bool {
    own.as_bytes() > peer.as_bytes()
}

/// 送る固定文字列（`hello from <OS> #<4 桁>`）。[`MAX_TEXT_BYTES`] 以内。
pub fn outgoing_text(os: &str, seed: u64) -> String {
    let mut s = format!("hello from {os} #{:04}", seed % 10_000);
    while s.len() > MAX_TEXT_BYTES {
        s.pop();
    }
    s
}

/// 相手から受けたバイト列を表示用の文字列へ検証する。空・[`MAX_TEXT_BYTES`] 超・不正な UTF-8・
/// 制御文字を除くと空になるものは `None`（切り詰めない。理由はモジュール doc）。
pub fn sanitize_received(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_TEXT_BYTES {
        return None;
    }
    let s: String = std::str::from_utf8(bytes)
        .ok()?
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    (!s.is_empty()).then_some(s)
}

/// Apple・Android 実装の完了コード → 契約のエラー（SSOT。OS 側で二重に写像しない）。
///
/// | code | 意味 | 写像 |
/// |---|---|---|
/// | 0 | 成功（相手の有無を問わない） | `Ok(())` |
/// | 1 | 許可なし | `PermissionDenied` |
/// | 2 | Wi-Fi オフなど今は使えない | `Disabled` |
/// | 3 | 非対応 | `Unsupported` |
/// | 4 | 中断 | `Internal` |
/// | 5 | サービス種別の未宣言 | `Internal` |
/// | それ以外 | 未知 | `Internal` |
pub fn decode_code(code: i32) -> Result<(), DeviceError> {
    const F: &str = "local_peer";
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::PermissionDenied(F)),
        2 => Err(DeviceError::Disabled(F)),
        3 => Err(DeviceError::Unsupported(F)),
        4 => Err(DeviceError::Internal("local_peer interrupted")),
        5 => Err(DeviceError::Internal("local_peer not declared")),
        _ => Err(DeviceError::Internal("local_peer outcome")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_invalid_oversized_and_empty() {
        assert_eq!(sanitize_received(&[]), None);
        assert_eq!(sanitize_received(&[0xff, 0xfe]), None);
        assert_eq!(sanitize_received(&[b'a'; MAX_TEXT_BYTES + 1]), None);
        assert_eq!(sanitize_received(b"\x00\x07\n"), None);
    }

    #[test]
    fn sanitize_strips_controls_and_keeps_normal() {
        assert_eq!(sanitize_received(b"a\x00b\x1b"), Some("ab".into()));
        assert_eq!(
            sanitize_received("こんにちは".as_bytes()),
            Some("こんにちは".into())
        );
        assert!(sanitize_received(&[b'a'; MAX_TEXT_BYTES]).is_some());
    }

    #[test]
    fn peer_name_validation() {
        assert!(is_valid_peer_name(&peer_token(0x1234_5678_9abc_def0)));
        for bad in [
            "",
            "fandhe-",
            "fandhe-1234567",
            "fandhe-123456789",
            "fandhe-1234567G",
            "fandhe-ABCDEF01",
            "x-12345678",
        ] {
            assert!(!is_valid_peer_name(bad), "{bad}");
        }
    }

    #[test]
    fn should_invite_is_antisymmetric() {
        assert!(should_invite("fandhe-00000002", "fandhe-00000001"));
        assert!(!should_invite("fandhe-00000001", "fandhe-00000002"));
        assert!(!should_invite("fandhe-00000001", "fandhe-00000001"));
    }

    #[test]
    fn decode_code_maps_every_code() {
        assert!(decode_code(0).is_ok());
        assert!(matches!(
            decode_code(1),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(decode_code(2), Err(DeviceError::Disabled(_))));
        assert!(matches!(decode_code(3), Err(DeviceError::Unsupported(_))));
        for c in [4, 5, 6, -1] {
            assert!(
                matches!(decode_code(c), Err(DeviceError::Internal(_))),
                "{c}"
            );
        }
    }

    #[test]
    fn outgoing_text_fits_and_passes_sanitize() {
        let t = outgoing_text("macOS", u64::MAX);
        assert!(t.len() <= MAX_TEXT_BYTES);
        assert_eq!(sanitize_received(t.as_bytes()), Some(t));
    }

    #[test]
    fn unsupported_is_error_not_empty_success() {
        assert!(matches!(
            UnsupportedLocalPeer.exchange("x"),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
