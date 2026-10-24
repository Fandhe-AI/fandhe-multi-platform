//! 端末・アプリの真正性証明の抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/app-attest.json`、
//! 親 #132、Issue #677）。
//!
//! 扱う範囲は「要求ごとに新しい鍵を作り、乱数の challenge を結び付けた証明データ
//! （Android は Key Attestation の証明書チェーン）を 1 回取得し、件数とサイズだけを表示する」こと。
//! 証明データの検証・信頼の判断・assertion・鍵の持ち越し・サーバーとの通信は扱わない
//! （判断はサーバーの責務で、端末上では根や security level を解釈しない）。対象は Android のみ
//! （`AndroidKeyStore` の `setAttestationChallenge`、API 24 以上、GMS 不要）。iOS は有料の開発者登録が
//! 切れているため deferred、macOS・Windows・Web は no。それ以外は `Unsupported` 固定。
//! 一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は UI を止めない
//! ようワーカースレッドから呼ぶ。
//!
//! challenge の乱数は本クレートでは作らない。OS 層の CSPRNG（Android は Java の `SecureRandom`）を
//! [`AppAttest::fill_random`] 越しに使う（新しい依存は足さない）。
//!
//! # 契約
//!
//! - **成功**: 空でない証明書チェーンだけ（[`check_result`]）。空のチェーン・0 バイトの証明書・
//!   上限超過は成功にしない。上限は切り詰めず失敗にする
//! - **失敗の区別**（写像表。iOS は deferred のため再開時の指針）:
//!
//! | 状況 | Android | iOS（deferred） | 返す値 |
//! | --- | --- | --- | --- |
//! | 非対応（API 24 未満・Android 以外） | `Build.VERSION` | `featureUnsupported` | `Unsupported` |
//! | challenge が不正 | — | `invalidInput` | `InvalidInput`（[`validate_challenge`]） |
//! | 鍵ストアの失敗 | 鍵の生成・取得の例外 | `invalidKey` | `Internal(`[`KEYSTORE`]`)` |
//! | 空のチェーン | チェーンが無い・空 | — | `Internal(`[`EMPTY`]`)` |
//! | 上限超過 | 件数・合計バイト超過 | — | `Internal(`[`TOO_LARGE`]`)` |
//! | 期限切れ | Rust の期限 | Rust の期限 | `Internal(`[`TIMEOUT`]`)` |
//! | `serverUnavailable` | — | `DCError.serverUnavailable` | `Internal(`[`SERVER_UNAVAILABLE`]`)`。同じ鍵・同じ hash での再試行が公式の指示だが、本範囲では鍵を持ち越さないため再試行しない |
//! | それ以外 | 他の例外 | 他のエラー | `Internal("app_attest")` |
//!
//! - **三値**（契約表 10）: 利用者の取り消し操作が無いため成功 / 失敗の 2 値
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`ATTEST_DEADLINE`]）。起点は要求を
//!   観測した時の `Instant`（単調時計）。期限後に届いた成功は捨てる。期限後も OS の鍵生成は
//!   走り切るが、実装側は finally で鍵を消す
//! - **並行性**（契約表 2）: 世代は画面が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: Android は Activity 再生成でドライバの記録を捨てる
//! - **資源の解放**（契約表 9）: 鍵は要求ごとに一意な名前で作り、成功・失敗・期限切れの全経路で消す
//! - **秘密値**（契約表 8）: challenge・証明データ・鍵名・keyId をログ・画面・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、鍵エントリの解放、秘密値の非表示。
//! 守らない範囲: 取得した証明の信頼性（判断はサーバーの責務）、証明データの検証、assertion と
//! 鍵の持ち越し、OS ごとの所要時間の差、期限後も OS の鍵生成が走り切ること、複数プロセス。
//! 画面には「取得した（サーバーで検証するまで未確定）」とだけ出し、「正規」「改ざんなし」とは出さない。

use std::fmt;
use std::time::Duration;

use crate::DeviceError;

/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ）。仮の値で、実機確認で見直す。
pub const ATTEST_DEADLINE: Duration = Duration::from_secs(30);
/// ドライバが作る challenge の長さ（バイト）。
pub const CHALLENGE_LEN: usize = 32;
/// challenge の最大バイト数（Android の `setAttestationChallenge` の上限）。
pub const MAX_CHALLENGE_BYTES: usize = 128;
/// 証明書チェーンの最大件数。仮の値。
pub const MAX_CERTS: usize = 10;
/// 証明書チェーン全体の最大バイト数。仮の値。
pub const MAX_TOTAL_BYTES: usize = 64 * 1024;

/// 空のチェーン、または 0 バイトの証明書を含むときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "app_attest empty";
/// 鍵の生成・取得に失敗したときの `Internal` の識別子（SSOT）。
pub const KEYSTORE: &str = "app_attest keystore";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "app_attest timeout";
/// 上限超過のときの `Internal` の識別子（SSOT）。
pub const TOO_LARGE: &str = "app_attest too_large";
/// iOS 再開時の `DCError.serverUnavailable` 用の `Internal` の識別子（SSOT。現状どの実装も返さない）。
pub const SERVER_UNAVAILABLE: &str = "app_attest server_unavailable";

/// 検査済みの challenge。`Debug` は値を伏せる（契約表 8）。
#[derive(Clone, PartialEq, Eq)]
pub struct Challenge(Vec<u8>);

impl Challenge {
    /// challenge のバイト列。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Challenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Challenge(<redacted>, {} bytes)", self.0.len())
    }
}

/// challenge を検査する。空または [`MAX_CHALLENGE_BYTES`] 超過は `InvalidInput`。
pub fn validate_challenge(bytes: &[u8]) -> Result<Challenge, DeviceError> {
    if bytes.is_empty() || bytes.len() > MAX_CHALLENGE_BYTES {
        return Err(DeviceError::InvalidInput("app_attest challenge"));
    }
    Ok(Challenge(bytes.to_vec()))
}

/// OS が返した証明書チェーン（DER の列）。中身は解釈しない。`Debug` は件数と合計バイト数だけを出す。
#[derive(Clone, PartialEq, Eq)]
pub struct Attestation {
    /// 証明書（葉から順）。
    pub certs: Vec<Vec<u8>>,
}

impl Attestation {
    /// 証明書の件数。
    pub fn count(&self) -> usize {
        self.certs.len()
    }

    /// 全証明書の合計バイト数。
    pub fn total_bytes(&self) -> usize {
        self.certs.iter().map(Vec::len).sum()
    }
}

impl fmt::Debug for Attestation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Attestation({} certs, {} bytes)",
            self.count(),
            self.total_bytes()
        )
    }
}

/// 証明データを検査する。空のチェーンや 0 バイトの証明書は `Internal(EMPTY)`、上限超過は
/// `Internal(TOO_LARGE)`。正常値はそのまま返す（中身は解釈しない）。
pub fn check_result(a: Attestation) -> Result<Attestation, DeviceError> {
    if a.certs.is_empty() || a.certs.iter().any(Vec::is_empty) {
        return Err(DeviceError::Internal(EMPTY));
    }
    if a.count() > MAX_CERTS || a.total_bytes() > MAX_TOTAL_BYTES {
        return Err(DeviceError::Internal(TOO_LARGE));
    }
    Ok(a)
}

/// 端末・アプリの真正性証明の契約（モジュール doc の「契約」節）。
pub trait AppAttest: Send + Sync {
    /// OS の CSPRNG で `buf` を埋める（challenge 用）。作れなければ失敗にし、固定値で埋めない。
    fn fill_random(&self, buf: &mut [u8]) -> Result<(), DeviceError>;

    /// 要求ごとに鍵を作り、`challenge` を結び付けた証明データを返す。鍵は全経路で消す。
    /// `remaining` は期限までの残り時間。
    fn attest(
        &self,
        challenge: &Challenge,
        remaining: Duration,
    ) -> Result<Attestation, DeviceError>;
}

/// 対象外 OS・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedAppAttest;

impl AppAttest for UnsupportedAppAttest {
    fn fill_random(&self, _: &mut [u8]) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("app_attest"))
    }

    fn attest(&self, _: &Challenge, _: Duration) -> Result<Attestation, DeviceError> {
        Err(DeviceError::Unsupported("app_attest"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(certs: &[usize]) -> Attestation {
        Attestation {
            certs: certs.iter().map(|n| vec![0xAB; *n]).collect(),
        }
    }

    #[test]
    fn challenge_boundaries() {
        assert!(validate_challenge(&[]).is_err());
        assert!(validate_challenge(&[1]).is_ok());
        assert!(validate_challenge(&[1; MAX_CHALLENGE_BYTES]).is_ok());
        assert!(matches!(
            validate_challenge(&[1; MAX_CHALLENGE_BYTES + 1]),
            Err(DeviceError::InvalidInput("app_attest challenge"))
        ));
    }

    #[test]
    fn check_result_rejects_bad_chains() {
        for bad in [att(&[]), att(&[10, 0]), att(&[0])] {
            assert!(matches!(
                check_result(bad),
                Err(DeviceError::Internal(EMPTY))
            ));
        }
        assert!(matches!(
            check_result(att(&[1; MAX_CERTS + 1])),
            Err(DeviceError::Internal(TOO_LARGE))
        ));
        assert!(matches!(
            check_result(att(&[MAX_TOTAL_BYTES + 1])),
            Err(DeviceError::Internal(TOO_LARGE))
        ));
    }

    #[test]
    fn check_result_keeps_valid_chain() {
        let a = check_result(att(&[100, 200])).unwrap();
        assert_eq!((a.count(), a.total_bytes()), (2, 300));
        assert!(check_result(att(&[MAX_TOTAL_BYTES])).is_ok());
    }

    #[test]
    fn debug_never_shows_values() {
        let c = validate_challenge(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        let s = format!("{c:?}");
        assert!(s.contains("redacted") && s.contains("4 bytes"), "{s}");
        assert!(!s.contains("222") && !s.contains("173") && !s.contains("190"));
        let a = Attestation {
            certs: vec![vec![171, 205, 239]],
        };
        let s = format!("{a:?}");
        assert!(!s.contains("171") && !s.contains("205"), "{s}");
        assert!(s.contains("1 certs") && s.contains("3 bytes"));
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let c = validate_challenge(&[1; CHALLENGE_LEN]).unwrap();
        assert!(matches!(
            UnsupportedAppAttest.attest(&c, ATTEST_DEADLINE),
            Err(DeviceError::Unsupported("app_attest"))
        ));
        assert!(matches!(
            UnsupportedAppAttest.fill_random(&mut [0; 4]),
            Err(DeviceError::Unsupported("app_attest"))
        ));
    }
}
