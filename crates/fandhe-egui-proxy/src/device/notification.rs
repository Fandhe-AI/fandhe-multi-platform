//! Phase 11（親 #1003）型サンプル「通知」（Issue #1004）の effect ドライバ。
//! `fandhe_core::screens::notification_send`（端末依存ゼロ）の `pending` 要求を観測し、
//! `fandhe_device::notification::Notifier` で OS へローカル通知の表示を依頼して
//! `CompletionSender` 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`device/secure_credential.rs` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # 別スレッドで OS を呼ぶ理由
//!
//! macOS の `osascript`（子プロセス起動、最大 [`fandhe_device::notification`] の
//! `SEND_TIMEOUT` だけブロック）・Windows の WinRT Toast はいずれも
//! `ProxyApp::update`（UI スレッド）から直接呼ぶと画面が固まるため、
//! `secure_credential.rs::drive` と同型に別スレッドへ逃がす。`notification_send`
//! （`fandhe-core` 側）は `Requested`/`Sending` の間は二重要求を防ぐため、
//! 複数要求の順序保証キュー（`tts.rs`）は不要（常に高々 1 件）。
//!
//! # macOS が `UNUserNotificationCenter` を使わない理由
//!
//! `UNUserNotificationCenter` はアプリバンドル外（署名済み `.app` を経ないプロセス）
//! から呼ぶと例外で落ちる。本フレームワークの desktop 実行体は `cargo run` で
//! バンドル無しのまま起動するため使えない。`osascript display notification` は
//! バンドル無しでも動く代替経路（`crates/fandhe-device/src/notification.rs`
//! モジュール doc の設計判断参照）。
//!
//! # iOS/Android: pull 型 `bridge`（OS の許可ダイアログはコールバック型、Issue #1005・#1006）
//!
//! iOS はバンドル化されたプロセスから起動するため `UNUserNotificationCenter` を
//! 使える（上記「macOS が使わない理由」と対照）。`requestAuthorizationWithOptions:`
//! （権限ダイアログ）は利用者の操作を待つため時間に上限がなく、`Notifier` trait
//! （同期呼び出し）+ ワーカースレッドでブロックする形は採れない。`device/
//! biometric_auth.rs` の Apple 経路と同型（[`bridge`] は `device/request_slot.rs::
//! RequestSlot` の facade、[`drive_callback`] が毎フレーム観測する pull 型）にする。
//! Android は `ActivityResultContracts.RequestPermission`（`POST_NOTIFICATIONS`、
//! Android 13+ のみ）が同じくコールバック型のため、iOS と同じ [`bridge`]/
//! [`drive_callback`] へ相乗りする（`biometric_auth.rs` が Apple/Android で
//! `bridge` を共有しているのと同型、Issue #1006）。iOS/Android の分岐だけをこの形に
//! し、macOS/Windows/その他の既存 [`drive`]（スレッド + `Notifier`）は変えない。
//!
//! 前面表示（`willPresent`）で Banner + List を返す（`ios/fandhe_ios_notification.m`
//! 参照）が、それでも `Shown` は返さない: `willPresent` で表示オプションを返しても、
//! 実際にバナーが出るかは利用者の OS 通知設定（バナーをオフにしている等）次第で
//! 確かめられないため。画面の文言は既存の [`fandhe_core::screens::notification_send::
//! SUBMITTED_STATUS`]（「送信しました（表示されるかは OS の通知設定次第です）」）の
//! ままで、「表示しました」とは言い切らない。通知タップへの応答
//! （`didReceiveNotificationResponse`）は実装しない（タップ時はアプリが開くだけ、
//! 実装計画のスコープ外節参照）。Android も同様に `contentIntent`（`PendingIntent`）を
//! 設定しない（スコープ外、security.md A01 の観点でも mutable な `PendingIntent` を
//! 作らずに済む）。
//!
//! # Android: `Cancelled`（コード 1）は返らない（既知の制約）
//!
//! `ActivityResultContracts.RequestPermission` は「拒否」と「ダイアログを閉じた
//! だけ」をどちらも `granted == false` で返し区別できない（Android の契約上の制約、
//! mic-record の Android 実装と同じ理由）。そのため Android は [`outcome_from_code`]
//! の `Cancelled`（1）を使わず、非許可はすべて `PermissionDenied`（2）に倒す
//! （`docs/device/capabilities/notification.json` の android note にも明記）。
//!
//! 期限は設けない: 権限ダイアログの応答時間は利用者次第で、OS は completion を
//! 必ず呼ぶ契約（iOS の生体認証経路と同じ判断）。Android も同じ理由で期限を掛けない。

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use std::sync::Arc;

use fandhe_core::screens::notification_send::{
    finish_notify, mark_sending, take_request, NotificationSendState, NotifyOutcome as CoreOutcome,
};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
use fandhe_device::notification::Notifier;
use fandhe_device::notification::NotifyOutcome;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言のみを転送する（`DeviceError` の内部構造は漏らさない、
/// security.md A09。タイトル・本文は `DeviceError` のどのバリアントにも含まれない、
/// `NotificationRequest::new` はタイトル・本文をエラーへ含めない設計）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => "この環境では通知を使えません".to_string(),
        other => other.to_string(),
    }
}

/// `fandhe_device::notification::NotifyOutcome` を core 側の 3 腕へ写す
/// （`fandhe-core` は `fandhe-device` に依存しないため、写像は本クレート側の責務）。
fn to_outcome(outcome: NotifyOutcome) -> CoreOutcome {
    match outcome {
        NotifyOutcome::Shown => CoreOutcome::Shown,
        NotifyOutcome::Submitted => CoreOutcome::Submitted,
        NotifyOutcome::Cancelled => CoreOutcome::Cancelled,
    }
}

/// 現在のプラットフォームに応じた `Notifier` 実装を返す。
///
/// macOS は [`fandhe_device::notification::OsascriptNotifier`]（初期化失敗時は
/// `UnsupportedNotifier` へ、ダミーの成功実装へは倒さない。`device-bridge-contracts.md`
/// の「6. 嘘の成功」参照）、Windows は [`super::windows_notification::WindowsToastNotifier`]。
/// それ以外（Linux・Web）は `UnsupportedNotifier`。iOS/Android は [`bridge`]/
/// [`drive_callback`] 経由の別経路（モジュール doc 参照）のため本関数の対象外にする
/// （cfg を揃える、`device-bridge-contracts.md`「7. 対応表・権限」の cfg 一致チェック）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn implementation() -> Arc<dyn Notifier> {
    #[cfg(target_os = "macos")]
    {
        use fandhe_device::notification::OsascriptNotifier;
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn Notifier>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| match OsascriptNotifier::new() {
                Ok(n) => Arc::new(n),
                Err(_) => Arc::new(fandhe_device::notification::UnsupportedNotifier),
            })
            .clone()
    }
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn Notifier>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(super::windows_notification::WindowsToastNotifier))
            .clone()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn Notifier>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(fandhe_device::notification::UnsupportedNotifier))
            .clone()
    }
}

/// `.m` 側の結果コード（[`super::ios_notification::fandhe_rust_notification_outcome`]
/// から渡される）、および Android 側の `MainActivity.NOTIFICATION_RESULT_*` 定数
/// （`nativeOnNotificationResult`）→ 画面へ反映する結果の写像（iOS/Android 共通の
/// SSOT。cfg は付けず全ターゲットでコンパイル・テスト対象にする
/// （`biometric_auth.rs::outcome_from_verification` と同じ理由。呼び出し元
/// （[`bridge::deliver`] 経由）は iOS/Android 限定だが、host（macOS/Windows/Linux）上の
/// `cargo test`/clippy でも失敗系を検証できるようにする）。
///
/// | コード | 状況（iOS） | 状況（Android） | 写像 |
/// |---|---|---|---|
/// | 0 | `addNotificationRequest` が error 無しで完了 | `notify()` が例外無しで完了
///     | `Ok(Submitted)` |
/// | 1 | 許可が未決定のまま閉じた（`requestAuthorization` が granted=NO かつ
///     `getNotificationSettings` で `authorizationStatus == NotDetermined`） |
///     **Android では出ない**（`ActivityResultContracts.RequestPermission` は
///     「拒否」と「閉じただけ」を区別できず、Android は非許可をすべてコード 2 へ
///     倒す。モジュール doc「Android: `Cancelled` は返らない」参照） | `Ok(Cancelled)` |
/// | 2 | 許可されていない（`Denied`） | 権限拒否・通知オフ（`areNotificationsEnabled`
///     が false）・チャンネルが `IMPORTANCE_NONE`・`SecurityException` |
///     `Err(PermissionDenied("notification"))` |
/// | 3 | `requestAuthorization` の error が非 nil | 権限ダイアログの `launch()` 自体が
///     例外を送出、または非前景での要求 | `Err(Internal("notification
///     authorization"))` |
/// | 4 | `addNotificationRequest` の error が非 nil | `notify()` が
///     `SecurityException` 以外の `RuntimeException` を送出 | `Err(Internal("notification
///     add"))` |
/// | それ以外 | 未知のコード | 未知のコード | `Err(Internal("notification"))`（偽の成功・
///     偽のキャンセルを作らない、`outcome_from_reply` と同じ方針） |
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
pub(crate) fn outcome_from_code(code: i32) -> Result<NotifyOutcome, DeviceError> {
    match code {
        0 => Ok(NotifyOutcome::Submitted),
        1 => Ok(NotifyOutcome::Cancelled),
        2 => Err(DeviceError::PermissionDenied("notification")),
        3 => Err(DeviceError::Internal("notification authorization")),
        4 => Err(DeviceError::Internal("notification add")),
        _ => Err(DeviceError::Internal("notification")),
    }
}

/// iOS/Android 共通: OS 側コールバックとの橋渡し（pull 型、`device/request_slot.rs::
/// RequestSlot` の facade。モジュール doc 参照、`biometric_auth::bridge` と同型）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::notification_send::NotifyOutcome as CoreOutcome;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome`/`super::to_message` 後の形）。
    pub(crate) type Outcome = Result<CoreOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果（[`super::outcome_from_code`] 経由の生の `Result`）を
    /// `super::to_outcome`/`super::to_message` で画面用へ変換してから置く
    /// （任意スレッド。実際には `.m` 側は main queue、Android は UI スレッドから
    /// 呼ぶ契約、`ios_notification.rs`/`android.rs` の doc 参照）。`request_id`
    /// 不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<super::NotifyOutcome, fandhe_device::DeviceError>,
    ) -> bool {
        SLOT.deliver(
            request_id,
            result.map(super::to_outcome).map_err(super::to_message),
        )
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ（`biometric_auth::
    /// bridge::reset` と同じ位置づけ）。[`SLOT`] はプロセス static のため Activity
    /// 再生成をまたいで残るが、画面状態（`NotificationSendState`）は再生成のたびに
    /// `Default`（`Idle`）で作り直される。放置すると、再生成前に開始した権限確認が
    /// その後完了しても [`super::drive_callback`] は新しい `Idle` 状態を見て早期
    /// return するため、結果が画面へ一切反映されず黙って失われる（`biometric_auth::
    /// bridge::reset` doc の codex-review 指摘と同じ理由）。ここで [`SLOT`] を丸ごと
    /// 捨て、以後届く旧 `request_id` の [`deliver`] を確実に無視させる。
    ///
    /// 呼び出し元（`android::android_main`）は Android にのみ存在するため、それ以外の
    /// ターゲット（host 上の `cargo test`）では未使用になるが、`outcome_from_code` と
    /// 同じ理由で `cfg` を割らず `allow(dead_code)` に留める。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// OS へ提示を依頼する（iOS は `ios_notification::present`、Android は
/// `android::request_notification`）。戻り値は「提示依頼を受理できたか」（`false` は
/// 呼び出し元が `Err(Internal("notification present"))` として畳む、
/// `biometric_auth::present` と同型）。
#[cfg(target_os = "ios")]
fn present(request_id: u64, title: &str, body: &str) -> bool {
    super::ios_notification::present(request_id, title, body)
}

#[cfg(target_os = "android")]
fn present(request_id: u64, title: &str, body: &str) -> bool {
    super::android::request_notification(request_id, title, body)
}

/// iOS/Android 共通: `notification_send` の要求 1 件を処理する effect ドライバ本体
/// （`biometric_auth::drive_callback` と同型）。
#[cfg(any(target_os = "ios", target_os = "android"))]
fn drive_callback(state: &NotificationSendState, sender: &CompletionSender<NotificationSendState>) {
    use fandhe_device::notification::NotificationRequest as DeviceRequest;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_sending));

        let request_id = bridge::begin();
        match DeviceRequest::new(&request.title, &request.body) {
            Ok(validated) => {
                if !present(request_id, validated.title(), validated.body()) {
                    // 提示依頼自体が同期的に失敗した（completion は二度と来ない）。
                    bridge::deliver(
                        request_id,
                        Err(fandhe_device::DeviceError::Internal("notification present")),
                    );
                }
            }
            Err(e) => {
                // 入力検証で弾かれた（通知 UI は開かない）。
                bridge::deliver(request_id, Err(e));
            }
        }
        return;
    }

    if state.phase != fandhe_core::screens::notification_send::NotificationPhase::Sending {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut NotificationSendState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_notify` し、反映できたときだけ `true` を返す（`RequestSlot`
            // 側がそれを見て `clear` する）。
            if s.phase != fandhe_core::screens::notification_send::NotificationPhase::Idle {
                finish_notify(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// `notification_send` の要求 1 件を処理して別スレッドで OS を呼び、完了を送る
/// （テストから任意の実装を差し込めるよう `notification_send_effect` 本体から切り出す）。
/// iOS/Android は [`drive_callback`] 経由の別経路（モジュール doc 参照）のため対象外にする。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn drive(
    state: &NotificationSendState,
    sender: &CompletionSender<NotificationSendState>,
    notifier: Arc<dyn Notifier>,
) {
    let Some(request) = take_request(state) else {
        return;
    };

    // 起動済みマーキング（二重起動防止、`secure_credential.rs::drive` と同型）。
    let _ = sender.send(Box::new(mark_sending));

    let generation = request.generation;
    let sender = sender.clone();
    std::thread::spawn(move || {
        let result =
            fandhe_device::notification::NotificationRequest::new(&request.title, &request.body)
                .and_then(|r| notifier.notify(&r))
                .map(to_outcome)
                .map_err(to_message);
        let _ = sender.send(Box::new(move |s: &mut NotificationSendState| {
            finish_notify(s, generation, result);
        }));
    });
}

/// `registry.rs` の `effect` 区画から `notification_send` 画面用に配線される
/// effect ドライバ（iOS/Android 版、モジュール doc「pull 型」節参照）。
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn notification_send_effect(
    state: &NotificationSendState,
    sender: &CompletionSender<NotificationSendState>,
) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `notification_send` 画面用に配線される
/// effect ドライバ（iOS/Android 以外版: macOS/Windows/Linux）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn notification_send_effect(
    state: &NotificationSendState,
    sender: &CompletionSender<NotificationSendState>,
) {
    drive(state, sender, implementation());
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照。
/// `device::biometric_auth::reset_pending_tracking` と同じ命名・位置づけ）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

#[cfg(test)]
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod tests {
    use super::*;
    use fandhe_core::screens::notification_send::{notify, set_body, set_title};

    fn sender_with_channel() -> (
        CompletionSender<NotificationSendState>,
        std::sync::mpsc::Receiver<crate::app::Completion<NotificationSendState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// 常に `Submitted` を返すモック（正常系の写像を確認する）。
    struct AlwaysSubmittedNotifier;

    impl Notifier for AlwaysSubmittedNotifier {
        fn notify(
            &self,
            _request: &fandhe_device::notification::NotificationRequest,
        ) -> Result<NotifyOutcome, DeviceError> {
            Ok(NotifyOutcome::Submitted)
        }
    }

    /// 常に `Unsupported` を返すモック（固定文言への畳み込みを確認する）。
    struct UnsupportedMockNotifier;

    impl Notifier for UnsupportedMockNotifier {
        fn notify(
            &self,
            _request: &fandhe_device::notification::NotificationRequest,
        ) -> Result<NotifyOutcome, DeviceError> {
            Err(DeviceError::Unsupported("notification"))
        }
    }

    /// 常に `PermissionDenied` を返すモック（成功文言へ化けないことを確認する）。
    struct DeniedMockNotifier;

    impl Notifier for DeniedMockNotifier {
        fn notify(
            &self,
            _request: &fandhe_device::notification::NotificationRequest,
        ) -> Result<NotifyOutcome, DeviceError> {
            Err(DeviceError::PermissionDenied("notification"))
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = NotificationSendState::default();
        drive(&state, &sender, Arc::new(AlwaysSubmittedNotifier));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_reports_submitted_status() {
        let (sender, rx) = sender_with_channel();
        let mut state = NotificationSendState::default();
        set_title(&mut state, "title".to_string());
        set_body(&mut state, "body".to_string());
        notify(&mut state);

        drive(&state, &sender, Arc::new(AlwaysSubmittedNotifier));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        assert!(
            fandhe_core::screens::notification_send::take_request(&state).is_none(),
            "起動済みマーク後は take_request が None を返す"
        );
        rx.recv().expect("完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::notification_send::status_text(&state),
            fandhe_core::screens::notification_send::SUBMITTED_STATUS
        );
    }

    #[test]
    fn drive_reports_unsupported_as_fixed_message() {
        let (sender, rx) = sender_with_channel();
        let mut state = NotificationSendState::default();
        set_title(&mut state, "title".to_string());
        notify(&mut state);

        drive(&state, &sender, Arc::new(UnsupportedMockNotifier));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::notification_send::error_text(&state),
            "失敗しました: この環境では通知を使えません"
        );
    }

    #[test]
    fn drive_empty_title_is_rejected_by_request_new() {
        // 画面側は空タイトルで要求を立てないが、直接要求を注入した場合でも
        // `NotificationRequest::new` の検証で弾かれることを確認する
        // （SSOT 方針: 検証は effect ドライバ側の 1 箇所に集約する）。
        let (sender, rx) = sender_with_channel();
        let mut state = NotificationSendState {
            title: "hello".to_string(),
            ..Default::default()
        };
        notify(&mut state);
        // タイトルを Requested 後に空へ書き換えて要求だけ不正にする。
        state.title = "   ".to_string();

        drive(&state, &sender, Arc::new(AlwaysSubmittedNotifier));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        assert!(
            fandhe_core::screens::notification_send::error_text(&state).contains("入力が不正です")
        );
    }

    #[test]
    fn drive_reports_permission_denied_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = NotificationSendState::default();
        set_title(&mut state, "title".to_string());
        notify(&mut state);

        drive(&state, &sender, Arc::new(DeniedMockNotifier));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了が送られる")(&mut state);
        let error = fandhe_core::screens::notification_send::error_text(&state);
        assert!(error.starts_with("失敗しました: "));
        assert!(!error.contains("送信しました"));
    }
}

/// [`outcome_from_code`] の単体テスト（iOS 専用の写像表、モジュール doc 参照）。
/// `outcome_from_code` 自体に cfg を付けていないため host（macOS/Windows/Linux）上の
/// `cargo test` で回せる（`biometric_auth::outcome_from_verification` と同じ理由）。
#[cfg(test)]
mod outcome_from_code_tests {
    use super::*;

    #[test]
    fn maps_zero_to_submitted() {
        assert!(matches!(outcome_from_code(0), Ok(NotifyOutcome::Submitted)));
    }

    #[test]
    fn maps_one_to_cancelled() {
        assert!(matches!(outcome_from_code(1), Ok(NotifyOutcome::Cancelled)));
    }

    #[test]
    fn maps_two_to_permission_denied() {
        assert!(matches!(
            outcome_from_code(2),
            Err(DeviceError::PermissionDenied("notification"))
        ));
    }

    #[test]
    fn maps_three_to_internal_authorization_error() {
        assert!(matches!(
            outcome_from_code(3),
            Err(DeviceError::Internal("notification authorization"))
        ));
    }

    #[test]
    fn maps_four_to_internal_add_error() {
        assert!(matches!(
            outcome_from_code(4),
            Err(DeviceError::Internal("notification add"))
        ));
    }

    #[test]
    fn maps_unknown_codes_to_internal_error() {
        for code in [-1, 5, 42, i32::MAX] {
            assert!(
                matches!(
                    outcome_from_code(code),
                    Err(DeviceError::Internal("notification"))
                ),
                "code={code}"
            );
        }
    }

    /// `PermissionDenied` が `to_message` を通っても成功の文言にならないことを
    /// 固定する（実装計画 §5「PermissionDenied が to_message を通っても成功の
    /// 文言にならないこと」）。
    #[test]
    fn permission_denied_message_does_not_contain_success_wording() {
        let Err(e) = outcome_from_code(2) else {
            panic!("code=2 は Err のはず");
        };
        let message = to_message(e);
        assert!(!message.contains("送信しました"));
        assert!(!message.contains("表示しました"));
    }
}
