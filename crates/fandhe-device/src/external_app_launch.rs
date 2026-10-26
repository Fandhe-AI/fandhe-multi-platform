//! 電話発信・地図など他アプリの起動の抽象（Phase 11 の拡充ループ、親 #176、Issue #861）。
//!
//! 範囲は「固定の 2 操作（電話・地図）を OS に渡し、OS の戻り値をそのまま画面に出す」ことだけ。
//! 電話は固定番号 [`PHONE_URI`]（北米の架空用予約帯）、地図は固定地点（東京駅）。任意の URL・番号・
//! 座標を外から渡す経路は作らない。操作は [`LaunchTarget`] の 2 値に限るため、入力不正
//! （`InvalidInput`）の余地は型で無くしてある。画面（`fandhe_core::screens::external_app_launch_show`、
//! 端末依存ゼロ）へは `fandhe-egui-proxy` の effect ドライバが写像する。
//!
//! # 結果の種類（[`LaunchOutcome`]）
//!
//! - `HandedOff`: OS が依頼を受け付けただけ。相手のアプリが出たか・発信したかは分からない
//! - `NoHandler`: 開けるアプリが無い（失敗扱いにしない）
//! - `Cancelled`: 利用者が取り消した（Windows のみ取れる。失敗扱いにしない）
//! - `TargetUnavailable`: この OS ではその操作を開けない。OS は呼ばない（Windows の地図）
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | OS が受け付けた | `Ok(HandedOff)`（macOS・Windows・iOS・Android・Web） |
//! | 開けるアプリが無い | `Ok(NoHandler)`（macOS の `openURL:` が NO・Windows の `ERROR_NO_ASSOCIATION`・iOS の completion が `false`・Android の `ActivityNotFoundException`） |
//! | 利用者のキャンセル | `Ok(Cancelled)`（Windows の `ERROR_CANCELLED`） |
//! | この OS では開けない | `Ok(TargetUnavailable)`（Windows の地図） |
//! | 非対応（Linux ほか。iOS / Android は実装が入るまで） | `Err(Unsupported("external app launch"))` |
//! | 失敗（Windows のその他の失敗・Android の非前景・未知のコード） | `Err(Internal("external app ..."))` |
//!
//! 「OS に渡した」を「開いた」「発信した」と読める文言にしない。開けなかった操作を別の URL で
//! 代わりに開くこともしない。
//!
//! # OS ごとの写像
//!
//! - macOS: 電話・地図とも `NSWorkspace openURL:`。地図は [`APPLE_MAPS_URL`]
//! - Windows: 電話は [`PHONE_URI`] を `ShellExecuteExW`（`SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI`）で
//!   開く。地図は Windows の地図アプリが非推奨のため OS を呼ばず `TargetUnavailable`
//! - iOS: `UIApplication.open(_:options:completionHandler:)`。地図は [`APPLE_MAPS_URL`]。
//!   completion の `success` で `HandedOff` / `NoHandler`
//! - Android: 電話は `ACTION_DIAL`、地図は `ACTION_VIEW` + [`GEO_URI`]。`ActivityNotFoundException` は
//!   `NoHandler`。RESUMED でなければ起動せず失敗
//! - Web: `location.href` へ [`PHONE_URI`] / [`GEO_URI`] を代入。`HandedOff` しか返せない
//!
//! OS を正にする。事前判定（`canOpenURL`・`resolveActivity`・`AssocQueryStringW`）はしない。
//! `LSApplicationQueriesSchemes`・`<queries>` は足さない。開いた後の状態は記憶も推測もせず、
//! 前面への復帰も監視しない。タイマー・期限で結果を作らない。
//!
//! # 守る範囲 / 守らない範囲（契約表 10・11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（世代と `request_id` の照合）、`NoHandler` /
//! `Cancelled` / `TargetUnavailable` / `Unsupported` を成功に畳まないこと、固定値以外を開かないこと。
//!
//! 守らない: OS が受け付けた後に相手のアプリが出たか・利用者が発信したか、iOS の確認ダイアログでの
//! 取り消し、iOS の completion が届かない場合（期限は設けないので画面は処理中のまま残る）、
//! Windows で `SEE_MASK_FLAG_NO_UI` 下でも「アプリを選ぶ」が出る場合、Web のハンドラ有無・ブラウザ差、
//! 複数プロセス・複数窓。

use crate::DeviceError;

/// 電話の固定 URI。北米の架空用予約帯（555-01xx）で、実在の番号へ発信を促さない。
/// `*` `#` を含めない（iOS は含むと発信しない）。
pub const PHONE_URI: &str = "tel:+1-202-555-0100";

/// 地図の固定 URL（iOS・macOS 用の Apple 地図の統一 URL。東京駅）。
pub const APPLE_MAPS_URL: &str =
    "https://maps.apple.com/place?coordinate=35.681236,139.767125&name=Tokyo%20Station";

/// 地図の固定 URI（Android・Web 用の `geo:`。東京駅。`q` 付きでピンを立てる）。
pub const GEO_URI: &str = "geo:35.681236,139.767125?q=35.681236,139.767125(Tokyo%20Station)";

/// 起動する操作。固定の 2 種で、任意の文字列は受けない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchTarget {
    /// 電話（[`PHONE_URI`]）。
    Phone,
    /// 地図（東京駅）。
    Map,
}

/// 起動した結果。`HandedOff` を「開いた」「発信した」と見せない（モジュール doc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// OS が依頼を受け付けただけ。
    HandedOff,
    /// 開けるアプリが無い。失敗ではない。
    NoHandler,
    /// 利用者が取り消した。失敗ではない。
    Cancelled,
    /// この OS では開けない操作。OS は呼んでいない。
    TargetUnavailable,
}

/// 他アプリを起動する。画面（effect ドライバ）から同期経路で呼ばれる。
pub trait ExternalAppLaunch: Send + Sync {
    /// `target` を OS に渡し、OS の戻り値を写して返す。契約はモジュール doc。
    fn launch(&self, target: LaunchTarget) -> Result<LaunchOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("external app launch")` を返し、成功を装わない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedExternalAppLaunch;

impl ExternalAppLaunch for UnsupportedExternalAppLaunch {
    fn launch(&self, _target: LaunchTarget) -> Result<LaunchOutcome, DeviceError> {
        Err(DeviceError::Unsupported("external app launch"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds_for_both_targets() {
        for t in [LaunchTarget::Phone, LaunchTarget::Map] {
            assert!(matches!(
                UnsupportedExternalAppLaunch.launch(t),
                Err(DeviceError::Unsupported("external app launch"))
            ));
        }
    }

    #[test]
    fn fixed_values_have_expected_shape() {
        assert!(PHONE_URI.starts_with("tel:"));
        assert!(APPLE_MAPS_URL.starts_with("https://maps.apple.com/"));
        assert!(GEO_URI.starts_with("geo:"));
        assert!(!PHONE_URI.contains(['*', '#']));
        for v in [PHONE_URI, APPLE_MAPS_URL, GEO_URI] {
            assert!(!v.contains(char::is_whitespace), "{v}");
        }
    }
}
