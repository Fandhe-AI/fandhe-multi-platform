//! 指定した Wi-Fi への接続（SSID とパスワードを指定して接続を 1 回要求し、OS の結果を受ける）の抽象
//! （Phase 11 の拡充ループ、対応表 `docs/device/capabilities/wifi-connect.json`、親 #165、Issue #791）。
//!
//! `fandhe-egui-proxy` の `device/wifi_connect.rs` がワーカースレッドで [`WifiConnect::connect`] を
//! 呼ぶ（ブロッキングの契約）。対象は macOS（#791）・Windows（#792）・Android（API 29 以上）。iOS は対応表で
//! `deferred`、Web は `no`。入力は [`validate`] を通した [`ConnectRequest`] だけが OS へ渡る。
//! 今つながっている Wi-Fi を読む `wifi_info` とは別で、本機能は書き込み側。
//!
//! # 契約
//!
//! - 結果は成功か [`DeviceError`]。写像は次のとおり
//!
//! | 結果 | 意味 |
//! | --- | --- |
//! | `Ok(())` | OS が、この要求に対して接続したと返した |
//! | `Internal(`[`NOT_FOUND`]`)` | 指定の SSID が見つからない |
//! | `PermissionDenied(`[`FEATURE`]`)` | 許可がない（位置情報サービスがオフの場合を含む） |
//! | `Disabled(`[`RADIO_OFF`]`)` | Wi-Fi の電源がオフ |
//! | `Unsupported(`[`FEATURE`]`)` | 端末に Wi-Fi アダプターが無い・OS が非対応 |
//! | `InvalidInput(`[`FEATURE`]`)` | [`validate`] が拒んだ入力 |
//! | `Internal(`[`UNSUPPORTED_SECURITY`]`)` | 扱わない認証方式（エンタープライズ等）しか見つからない |
//! | `Internal(`[`CONNECT_FAILED`]`)` | 上記以外の失敗（パスワード違い・管理者パスワードの取り消しを含む） |
//!
//! - **OS を正にする**。接続できるかを事前に判定せず要求を出し、OS の戻り値を写す。電源・許可を
//!   OS に読み直すのは、失敗したときに理由を分けるためだけに限る
//! - 「接続した」は、その要求に対する OS の結果にすぎない。いま接続中だとは主張せず、アプリ側に
//!   接続中の記憶を持たない（今の接続は `wifi_info` で OS に問う）。Android の `Ok(())` は、このアプリだけが
//!   使える接続を OS が張ったという意味で、端末全体の接続ではなくインターネットにも使われない。
//!   `wifi_info` の今の接続に出ないことがある
//! - **要求の保持（Android のみ）**: OS は接続要求を解放するまで接続を保ち、解放すると接続を切りうる。
//!   成功した要求は、次の [`WifiConnect::connect`] の直前か Activity の破棄（画面回転を含む）で解放し、
//!   それ以外では放さない。保留中（OS の結果を待っている）の要求は、Activity の破棄で
//!   `Internal(`[`CONNECT_FAILED`]`)` に確定して取り下げる。握るのは OS の要求だけで、接続中かどうかは記憶しない
//! - 期限とポーリングを持たない。待ちが終わるのは、OS の結果が返るかアプリが終了するときだけ
//! - **三値**（類型 10）: 利用者が取り消せるのは macOS の管理者パスワードのダイアログだけ。OS が
//!   この取り消しを識別できる値で返すかは未確認のため、いまは [`CONNECT_FAILED`] に含めて失敗として
//!   出す（画面の文言は利用者を責めない言い方にする）。識別できる値が見つかれば後で分ける
//!   （Mac 目視 #782 の確認項目）。Android は拒否・見つからない・接続の失敗・前景でない要求がすべて同じ
//!   `onUnavailable` で届くため、Wi-Fi の電源を読み直してオフだけを分け、残りは [`CONNECT_FAILED`] にする
//!   （[`NOT_FOUND`] は返さない）
//! - SSID・パスワード・OS のエラー値（`CWErr` / `NSError` / HRESULT）をログ・エラー・`Debug` に出さない
//!   （[`WifiSsid`]・[`WifiPassword`] の `Debug` は伏せ字で、`Display` は無い）
//! - 失敗・見つからない・許可なし・電源オフ・設定を受け付けただけの状態を「接続した」に畳まない。
//!   初期化に失敗したときも [`UnsupportedWifiConnect`] へ倒し、ダミーの成功へは倒さない
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 嘘の成功を出さないこと、資源を漏らさないこと、秘密値を出力しないこと。
//!
//! 守らない（表示が一時的に食い違うだけのもの）:
//! - 接続した後の切断・別ネットワークへの切り替え
//! - OS 呼び出しが戻らない場合
//! - 管理者パスワードのダイアログで待つ時間
//! - Windows で他のアプリが同時に接続を要求したときの通知の取り違え
//! - 複数アダプターで 1 つだけを使うこと、複数プロセスからの同時の要求
//! - Android で画面を離れた後も、次の要求か Activity の破棄まで接続が残ること
//! - Android で画面回転（Activity の再生成）により接続が切れること
//! - メモリ上のパスワードの消去（保証しない）
//!
//! # OS ごとの写像
//!
//! - macOS（#791）: directed scan で SSID のバイト一致する network だけを残し、暗号なし・
//!   WPA/WPA2/WPA3 Personal 系だけに `associateToNetwork:password:error:` を呼ぶ。許可は未決定の
//!   ときだけ要求する。失敗したときに電源・許可を読み直して理由を分ける
//! - Windows（#792 への約束）: `WlanConnect` の戻り値だけでは成功にせず、`connection_complete`
//!   の通知を待つ。登録 → 接続 → 解除 → context の解放 → ハンドルを閉じる、の順を全経路で守る。
//!   プロファイル XML は SSID を 16 進で入れ、パスワードは XML エスケープする。WPA2-Personal（AES）と
//!   暗号なしだけを扱う
//! - Android: `WifiNetworkSpecifier`（`setSsid` + `setWpa2Passphrase`、パスワードが空なら暗号なし）で
//!   `requestNetwork` を出し、最初に届いた `onAvailable` を `Ok(())`、`onUnavailable` を失敗にする。
//!   API 28 以下は `Unsupported`。16 進 64 文字の鍵は Specifier が ASCII のパスフレーズとして扱うため
//!   OS へ渡さず [`CONNECT_FAILED`]。WPA3 だけのネットワークは [`CONNECT_FAILED`]。要求の生成・登録で
//!   例外（`TooManyRequestsException`・`SecurityException` 等）が出たら [`CONNECT_FAILED`]

use crate::DeviceError;

/// `Unsupported` / `PermissionDenied` / `InvalidInput` の機能名。
pub const FEATURE: &str = "wifi_connect";
/// `Disabled` の識別子（Wi-Fi の電源オフ）。
pub const RADIO_OFF: &str = "wifi_connect radio";
/// `Internal` の識別子（指定の SSID が見つからない）。
pub const NOT_FOUND: &str = "wifi_connect not_found";
/// `Internal` の識別子（扱わない認証方式しか見つからない）。
pub const UNSUPPORTED_SECURITY: &str = "wifi_connect security";
/// `Internal` の識別子（上記以外の失敗）。
pub const CONNECT_FAILED: &str = "wifi_connect failed";
/// SSID の最大バイト数（IEEE 802.11）。
pub const SSID_MAX_BYTES: usize = 32;

/// 接続先の名前。位置を推定できるため `Debug` は伏せ字、`Display` は実装しない。生成は [`validate`] のみ。
#[derive(Clone, PartialEq, Eq)]
pub struct WifiSsid(Vec<u8>);

impl std::fmt::Debug for WifiSsid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WifiSsid(<redacted>)")
    }
}

impl WifiSsid {
    /// OS へ渡す生バイト。FFI へ渡す箇所以外で使わない。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// 接続先のパスワード。`Debug` は伏せ字、`Display` は実装しない。生成は [`validate`] のみ。
#[derive(Clone, PartialEq, Eq)]
pub struct WifiPassword(String);

impl std::fmt::Debug for WifiPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WifiPassword(<redacted>)")
    }
}

impl WifiPassword {
    /// OS へ渡す文字列。FFI へ渡す箇所以外で使わない。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// 検査済みの接続要求。`password` が `None` なら暗号なしの意味。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    /// 接続先。
    pub ssid: WifiSsid,
    /// パスワード（暗号なしは `None`）。
    pub password: Option<WifiPassword>,
}

/// 画面の入力を検査して [`ConnectRequest`] にする。失敗は `InvalidInput(`[`FEATURE`]`)`。
///
/// - SSID: 1〜[`SSID_MAX_BYTES`] バイトの UTF-8。NUL と制御文字は不可。**trim しない**
///   （`" home "` は `"home"` と別の SSID。空白だけの入力は不可）
/// - パスワード: 空（暗号なし）・印字可能な ASCII（0x20〜0x7E）で 8〜63 文字・16 進 64 文字のどれか。
///   **trim しない**。WEP（5・13 文字等）は扱わないため不可
pub fn validate(ssid: &str, password: &str) -> Result<ConnectRequest, DeviceError> {
    let invalid = || DeviceError::InvalidInput(FEATURE);
    if ssid.is_empty()
        || ssid.len() > SSID_MAX_BYTES
        || ssid.chars().any(char::is_control)
        || ssid.chars().all(char::is_whitespace)
    {
        return Err(invalid());
    }
    let password = if password.is_empty() {
        None
    } else {
        let printable = password.bytes().all(|b| (0x20..=0x7e).contains(&b));
        let passphrase = printable && (8..=63).contains(&password.len());
        let hex_key = password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit());
        if !(passphrase || hex_key) {
            return Err(invalid());
        }
        Some(WifiPassword(password.to_string()))
    };
    Ok(ConnectRequest {
        ssid: WifiSsid(ssid.as_bytes().to_vec()),
        password,
    })
}

/// 指定した Wi-Fi への接続の契約（モジュール doc の「契約」節）。
pub trait WifiConnect: Send + Sync {
    /// 接続を 1 回要求し、OS の結果を返す。呼び出し側がワーカースレッドで呼ぶブロッキング呼び出し。
    fn connect(&self, request: &ConnectRequest) -> Result<(), DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedWifiConnect;

impl WifiConnect for UnsupportedWifiConnect {
    fn connect(&self, _request: &ConnectRequest) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported(FEATURE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad(ssid: &str, pw: &str) -> bool {
        matches!(validate(ssid, pw), Err(DeviceError::InvalidInput(FEATURE)))
    }

    #[test]
    fn ssid_length_boundaries() {
        assert!(bad("", ""));
        assert!(validate("a", "").is_ok());
        assert!(validate(&"a".repeat(32), "").is_ok());
        assert!(bad(&"a".repeat(33), ""));
        // バイト数で数える（"あ" は 3 バイト）。
        assert!(validate(&"あ".repeat(10), "").is_ok());
        assert!(bad(&"あ".repeat(11), ""));
    }

    #[test]
    fn ssid_rejects_control_and_blank() {
        assert!(bad("a\0b", ""));
        assert!(bad("a\nb", ""));
        assert!(bad("   ", ""));
    }

    #[test]
    fn ssid_is_not_trimmed() {
        let r = validate(" home ", "").unwrap();
        assert_eq!(r.ssid.as_bytes(), b" home ");
    }

    #[test]
    fn password_boundaries() {
        assert!(validate("s", "").unwrap().password.is_none());
        assert!(bad("s", &"a".repeat(7)));
        assert!(validate("s", &"a".repeat(8)).is_ok());
        assert!(validate("s", &"a".repeat(63)).is_ok());
        assert!(bad("s", &"g".repeat(64)));
        assert!(validate("s", &"f".repeat(64)).is_ok());
        assert!(bad("s", &"f".repeat(65)));
        assert!(bad("s", "passwordあ"));
        assert!(bad("s", "pass\tword"));
    }

    #[test]
    fn password_is_not_trimmed() {
        let r = validate("s", " pass word ").unwrap();
        assert_eq!(r.password.unwrap().expose(), " pass word ");
    }

    #[test]
    fn debug_hides_secrets() {
        let r = validate("secret-ssid", "secret-password").unwrap();
        let d = format!("{r:?}");
        assert!(!d.contains("secret"), "{d}");
    }

    #[test]
    fn unsupported_never_succeeds() {
        let r = validate("s", "").unwrap();
        assert!(matches!(
            UnsupportedWifiConnect.connect(&r),
            Err(DeviceError::Unsupported(FEATURE))
        ));
    }
}
