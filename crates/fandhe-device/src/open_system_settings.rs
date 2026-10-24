//! OS の設定画面を開く機能の抽象（Phase 11 の拡充ループ、親 #171、Issue #798）。
//!
//! 範囲は「固定の 2 頁（自アプリの設定・通知の設定）を開くよう OS に頼み、OS の戻り値をそのまま
//! 画面に出す」ことだけ。権限を拒否された後に利用者を OS の設定画面へ案内する操作で、任意の
//! URL・Intent・URI を外から渡す経路は作らない。頁は [`SettingsPage`] の 2 値に限るため、入力不正
//! （`InvalidInput`）の余地は型で無くしてある。画面（`fandhe_core::screens::open_system_settings_show`、
//! 端末依存ゼロ）へは `fandhe-egui-proxy` の effect ドライバが写像する。
//!
//! # 結果の種類（[`OpenSettingsOutcome`]）
//!
//! - `HandedOff`: OS が依頼を受け付けただけ。画面が実際に出たか・利用者が何を変えたかは分からない
//! - `NotOpened`: OS が開かなかった（失敗扱いにしない）
//! - `PageUnavailable`: この OS では開けない頁。OS は呼ばない（Windows の自アプリの設定）
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | OS が受け付けた | `Ok(HandedOff)`（Windows・iOS・Android） |
//! | OS が開かなかった | `Ok(NotOpened)`（Windows の `SE_ERR_NOASSOC`・iOS の completion が `false`・Android の `ActivityNotFoundException`） |
//! | この OS では開けない頁 | `Ok(PageUnavailable)`（Windows の自アプリの設定） |
//! | 非対応（macOS・Linux・Web ほか。iOS / Android は実装が入るまで） | `Err(Unsupported("open settings"))` |
//! | 失敗（Windows のその他の戻り値・Android の非前景・未知のコード） | `Err(Internal("open settings ..."))` |
//!
//! 「OS に渡した」を「開いた」「設定した」と読める文言にしない。開けなかった頁を別の頁で代わりに
//! 開くこともしない。
//!
//! # OS ごとの写像
//!
//! - iOS: `openSettingsURLString` / `openNotificationSettingsURLString` と
//!   `open(_:options:completionHandler:)`。completion の `success` で `HandedOff` / `NotOpened`
//! - Android: `ACTION_APPLICATION_DETAILS_SETTINGS` / `ACTION_APP_NOTIFICATION_SETTINGS` を
//!   `startActivity` で開く。`ActivityNotFoundException` は `NotOpened`。RESUMED でなければ起動せず失敗
//! - Windows: 通知は `ms-settings:notifications` を `ShellExecuteW` で開く。自アプリの設定は
//!   パッケージ ファミリー名が要るため `PageUnavailable`
//!
//! OS を正にする。事前判定（`canOpenURL`・`resolveActivity`・`AssocQueryStringW`）はしない。
//! 開いた後の状態は記憶も推測もせず、前面への復帰も監視しない。タイマー・期限で結果を作らない。
//!
//! # 守る範囲 / 守らない範囲（契約表 10・11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（世代と `request_id` の照合）、`NotOpened` /
//! `PageUnavailable` / `Unsupported` を成功に畳まないこと、固定の頁以外を開かないこと。
//!
//! 守らない: OS が受け付けた後に画面が実際に前面へ出たか、利用者の操作、iOS の completion が
//! 届かない場合（期限は設けないので画面は処理中のまま残る）、Windows の頁の有無の版差・SKU 差、
//! 複数プロセス・複数窓。

use crate::DeviceError;

/// 開く頁。固定の 2 種で、任意の文字列は受けない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    /// 自アプリの設定。
    AppSettings,
    /// 通知の設定。
    Notifications,
}

/// 設定画面を開いた結果。`HandedOff` を「開いた」と見せない（モジュール doc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSettingsOutcome {
    /// OS が依頼を受け付けただけ。
    HandedOff,
    /// OS が開かなかった。失敗ではない。
    NotOpened,
    /// この OS では開けない頁。OS は呼んでいない。
    PageUnavailable,
}

/// OS の設定画面を開く。画面（effect ドライバ）から同期経路で呼ばれる。
pub trait OpenSystemSettings: Send + Sync {
    /// `page` を開くよう OS に頼み、OS の戻り値を写して返す。契約はモジュール doc。
    fn open(&self, page: SettingsPage) -> Result<OpenSettingsOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("open settings")` を返し、成功を装わない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedOpenSystemSettings;

impl OpenSystemSettings for UnsupportedOpenSystemSettings {
    fn open(&self, _page: SettingsPage) -> Result<OpenSettingsOutcome, DeviceError> {
        Err(DeviceError::Unsupported("open settings"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds_for_both_pages() {
        for p in [SettingsPage::AppSettings, SettingsPage::Notifications] {
            assert!(matches!(
                UnsupportedOpenSystemSettings.open(p),
                Err(DeviceError::Unsupported("open settings"))
            ));
        }
    }
}
