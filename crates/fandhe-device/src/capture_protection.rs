//! 画面キャプチャの防止・検知（Phase 11 の拡充ループ、親 #120、Issue #582、対応表
//! `docs/device/capabilities/capture-protection.json`）。
//!
//! 扱う範囲は「自分のウィンドウを画面の撮影・録画に写さない（防止）」と「録画・ミラーリング中か、
//! スクリーンショットが撮られたかを知る（検知）」。録る側の機能は別 id の `screen_capture`。
//! `fandhe-egui-proxy` の橋渡し層が UI スレッドから [`CaptureProtection::set_protected`] を呼び、
//! [`CaptureProtection::is_protected`] / [`CaptureProtection::observe`] を [`READ_INTERVAL`] ごとに
//! 読んで画面（`fandhe_core::screens::capture_protection_show`）へ返す。
//!
//! # 契約
//!
//! - **表示は読み戻しで決める**: 「保護中」と出すのは `is_protected() == Some(true)` のときだけ。
//!   要求値で表示を作らない。解除に失敗したら保護中のまま、掛けるのに失敗したら未保護のまま出す
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **冪等**: 同じ値の再設定は `Ok`。資源を二重に確保しない
//! - **スクリーンショット**: 真偽ではなく単調に増える回数（`screenshot_count`）で渡す。読み取りの
//!   合間に起きた 2 回目を落とさないため。回数は撮影の後に届く通知から数えるので、防止ではない
//! - **読めない値は `None`**: `false` / `0` で代用しない。背景にいる間は録画状態を `None` にし、
//!   最後の値を返し続けない（契約表 1）
//! - **エラー種別**: 返すのは `Unsupported` と `Internal` だけ。検知の権限は install-time で実行時の
//!   拒否は無く、利用者がキャンセルする経路も無いので `PermissionDenied` / `Cancelled` は返さない。
//!   三値（契約表 10）のうちキャンセルは該当なしで、成功 / 失敗の二値になる
//! - **スレッド**: UI スレッド（egui の update）から同期で呼ぶ。Android は UI スレッドで適用し期限付きで
//!   待ち、適用の開始と放棄は CAS で排他にする（#584 の責務）。iOS の通知 observer の登録トークンは
//!   static の strong 参照で持つ（#583 の責務）
//! - **ログ**: 検知した事実（録画中・スクショ回数）や OS のエラーコードをログに出さない（security.md A09）
//!
//! # OS ごとの対応項目
//!
//! | OS | 防止 | 録画の検知 | スクショの検知 | 担当 |
//! | --- | --- | --- | --- | --- |
//! | Windows | あり | なし | なし | #582 |
//! | iOS | なし | あり（`sceneCaptureState`、17+） | あり（通知） | #583 |
//! | Android | `FLAG_SECURE` | 35+ | 34+ | #584 |
//! | macOS・Linux・その他 | `Unsupported` | 同左 | 同左 | - |
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは、1 プロセス内で表示が実際の状態に追いつくこと、防げない・読めない状態を「保護中」と
//! 見せないこと、解除し忘れないこと、コールバックの登録と解除が対になり資源を解放すること。
//! 守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）は、複数ウィンドウ、防止を
//! 掛ける前に始まっていた録画の撮影済みフレーム、外部カメラ・写真での撮影、他プロセスによる録画の
//! 検知、OS が検知しない経路（Android の adb や計装テストのスクリーンショット）、状態変化から
//! 次の読み取りまでの表示の遅れ・OS 側の非同期な確定との短い時間差、プロセス終了時の後片付け
//! （ウィンドウ属性はウィンドウと一緒に消える）。いずれも表示が一時的に食い違うだけで、
//! データ消失・資源漏れ・嘘の成功には至らない。

use std::time::Duration;

use crate::DeviceError;

/// 間隔付き pull の周期。最終読取からこれ以上たったときだけ OS を読む。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// 実行時に決まる対応項目（API レベル・OS 版で変わる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureSupport {
    /// 撮影・録画に写さない防止ができるか。
    pub prevent: bool,
    /// 録画・ミラーリング中かを知れるか。
    pub detect_recording: bool,
    /// スクリーンショットを検知できるか。
    pub detect_screenshot: bool,
}

/// 検知の観測値。読めない値は `None`（`false` / `0` で代用しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureObservation {
    /// 録画・ミラーリング中か。背景にいる間などは `None`。
    pub recording: Option<bool>,
    /// 起動後に検知したスクリーンショットの回数（単調増加）。
    pub screenshot_count: Option<u64>,
}

/// 画面キャプチャの防止・検知の契約（モジュール doc の「契約」節）。
pub trait CaptureProtection: Send + Sync {
    /// 対応項目。実行時に決まる。
    fn support(&self) -> CaptureSupport;

    /// `true` で防止を掛け、`false` で外す。冪等。非対応は `Err(Unsupported)`。
    fn set_protected(&self, on: bool) -> Result<(), DeviceError>;

    /// OS から読み戻した防止の状態。`None` は読めない。
    fn is_protected(&self) -> Option<bool>;

    /// 検知の観測値。読めない値は `None`。
    fn observe(&self) -> CaptureObservation;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedCaptureProtection;

impl CaptureProtection for UnsupportedCaptureProtection {
    fn support(&self) -> CaptureSupport {
        CaptureSupport::default()
    }

    fn set_protected(&self, _on: bool) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("capture-protection"))
    }

    fn is_protected(&self) -> Option<bool> {
        None
    }

    fn observe(&self) -> CaptureObservation {
        CaptureObservation::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_is_not_a_fake_success() {
        for on in [true, false] {
            let err = UnsupportedCaptureProtection.set_protected(on).unwrap_err();
            assert!(matches!(err, DeviceError::Unsupported(_)));
        }
        assert_ne!(UnsupportedCaptureProtection.is_protected(), Some(true));
        assert_eq!(
            UnsupportedCaptureProtection.observe(),
            CaptureObservation::default()
        );
        assert_eq!(
            UnsupportedCaptureProtection.support(),
            CaptureSupport::default()
        );
    }
}
