//! Phase 11（端末機能の拡充ループ、親 #905）型サンプル「生体認証」（Issue #906 macOS →
//! #907 Windows・#908 iOS/Android 横展開）の effect ドライバ。
//! `fandhe_core::screens::biometric_check`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # macOS/iOS/Android/Windows: pull 型 `bridge`（OS の認証 UI はいずれもコールバック型）
//!
//! `LAContext.evaluatePolicy`（macOS/iOS）・`BiometricPrompt.AuthenticationCallback`
//! （Android）・`UserConsentVerifier`（Windows、`IAsyncOperation` の完了ハンドラ）は
//! いずれも reply が非同期に届くため、`BiometricAuth` トレイトを通さずコアの状態機械へ
//! 直接書き戻す（`device/share.rs` と同型）。[`bridge`] は `device/request_slot.rs::
//! RequestSlot` の facade で、`CompletionSender` も画面の `generation` も保持しない
//! pull 型: OS 側コールバックは結果を置くだけで、画面への反映は毎フレーム
//! [`drive_callback`] が **その時点の** `sender`/`state` を使って行う
//! （`RequestSlot`/`poll_and_finish` のモジュール doc 参照、Issue #943 で 3 機能の
//! 共通部品へ統合）。[`present`] が Apple/Android 向けの提示処理へ振り分ける
//! （Windows は `windows_biometric::present` を [`drive_callback`] から直接呼ぶ、後述）。
//!
//! # 上記 4 OS 以外（Linux）: `BiometricAuth` トレイト経由
//!
//! [`UnsupportedBiometricAuth`]（`fandhe_device::biometric_auth::UnsupportedBiometricAuth`）
//! を `BiometricAuth` トレイト経由で呼び、`Unsupported("biometric_auth")` を画面へ
//! 固定文言として表示する。

use fandhe_core::screens::biometric_check::{
    finish_auth, mark_authenticating, take_request, AuthOutcome as CoreAuthOutcome,
    BiometricCheckState,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::biometric_auth::BiometricAuth;
use fandhe_device::biometric_auth::{validate_reason, AuthOutcome as DeviceAuthOutcome};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `BiometricAuth`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`DeviceError` の内部構造は含めない、security.md A09）。
const UNAVAILABLE: &str = "この環境では生体認証を使えません";

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する
/// （`device/share.rs::to_message` と同じ方針）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// `Result<DeviceAuthOutcome, DeviceError>` を画面表示用の結果へ変換する
/// （macOS/iOS 経路・`BiometricAuth` トレイト経路の共通部分）。
fn to_outcome(result: Result<DeviceAuthOutcome, DeviceError>) -> Result<CoreAuthOutcome, String> {
    result
        .map(|outcome| match outcome {
            DeviceAuthOutcome::Authenticated => CoreAuthOutcome::Authenticated,
            DeviceAuthOutcome::Cancelled => CoreAuthOutcome::Cancelled,
            DeviceAuthOutcome::Unavailable => CoreAuthOutcome::Unavailable,
            DeviceAuthOutcome::Failed => CoreAuthOutcome::Failed,
        })
        .map_err(to_message)
}

/// `LAError` の生コード（`success`/`la_code`）→ 結果の写像（iOS/macOS 共通の SSOT、
/// `.m` 側は二重に写像しない）。`success == true` かつ `la_code == 0`（`LAContext` の
/// 契約上の正常成功）のときだけ `Authenticated` にする。それ以外の `success == true`
/// （契約上起きないはずの組み合わせ）は偽の成功を作らず `Internal` として畳む。
/// 未知の `la_code` も失敗側（`Internal`）に倒す（偽のキャンセル・偽の成功を作らない）。
///
/// | la_code | 意味（`LAError`） | 写像 |
/// |---|---|---|
/// | -2 | `userCancel` | `Cancelled` |
/// | -3 | `userFallback`（パスコードフォールバックは不使用のため到達時もキャンセル扱い） | `Cancelled` |
/// | -4 | `systemCancel` | `Cancelled` |
/// | -9 | `appCancel` | `Cancelled` |
/// | -5 | `passcodeNotSet` | `Unavailable` |
/// | -6 | `biometryNotAvailable` | `Unavailable` |
/// | -7 | `biometryNotEnrolled` | `Unavailable` |
/// | -1 | `authenticationFailed`（不一致） | `Failed` |
/// | -8 | `biometryLockout` | `Failed` |
/// | それ以外 | 未知（`invalidContext`/`notInteractive`/二重呼び出し等を含む） | `Err(Internal)` |
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn outcome_from_reply(
    success: bool,
    la_code: i64,
) -> Result<DeviceAuthOutcome, DeviceError> {
    if success && la_code == 0 {
        return Ok(DeviceAuthOutcome::Authenticated);
    }
    if success {
        // `success == true` かつ `la_code != 0` は `LAContext` の契約上起きないが、
        // `success` を優先せず偽の成功を作らない方向へ倒す。
        return Err(DeviceError::Internal("biometric outcome"));
    }
    match la_code {
        -2 | -3 | -4 | -9 => Ok(DeviceAuthOutcome::Cancelled),
        -7..=-5 => Ok(DeviceAuthOutcome::Unavailable),
        -1 | -8 => Ok(DeviceAuthOutcome::Failed),
        _ => Err(DeviceError::Internal("biometric outcome")),
    }
}

/// `UserConsentVerificationResult`（Windows Hello、`windows_biometric.rs` の
/// `IAsyncOperation` 完了ハンドラから渡される生コード）→ 結果の写像。`code` は
/// `UserConsentVerificationResult` の内部 `i32`（`windows_biometric::present` の
/// `r.0`）。完了ハンドラ側で `AsyncStatus != Completed`・`GetResults()` 失敗は
/// 既に `Err(Internal)` へ畳まれているため、この関数へは `Completed` かつ
/// `GetResults()` 成功時のみ到達する。
///
/// # `Verified`（code 0）は生体一致限定ではない（既知の制約）
///
/// `IUserConsentVerifierInterop::RequestVerificationForWindowAsync` は Windows Hello の
/// 統合認証 UI であり、利用者が指紋・顔ではなく PIN（端末所有者認証）で完了しても同じ
/// `Verified` を返す（Microsoft の API 契約:
/// <https://learn.microsoft.com/en-us/windows/win32/api/userconsentverifierinterop/nf-userconsentverifierinterop-iuserconsentverifierinterop-requestverificationforwindowasync>）。
/// WinRT の公開 API には生体照合のみへ絞るオプションが無いため、コード側でこれ以上
/// 狭めることはできない（macOS/iOS の
/// `LAPolicyDeviceOwnerAuthenticationWithBiometrics` のような厳密指定に相当する API が
/// Windows 側に存在しない）。`CoreAuthOutcome::Authenticated` は Windows 上では
/// 「生体認証に成功した」ではなく「Windows Hello の端末所有者認証に成功した（PIN を
/// 含みうる）」という意味になる（`biometric_check::AuthOutcome::Authenticated` の doc・
/// `docs/device/capabilities/biometric-auth.json` の windows note も参照）。
///
/// | code | 意味（`UserConsentVerificationResult`） | 写像 |
/// |---|---|---|
/// | 0 | `Verified`（生体または PIN） | `Ok(Authenticated)` |
/// | 6 | `Canceled` | `Ok(Cancelled)` |
/// | 1 | `DeviceNotPresent` | `Ok(Unavailable)` |
/// | 2 | `NotConfiguredForUser` | `Ok(Unavailable)` |
/// | 3 | `DisabledByPolicy` | `Ok(Unavailable)` |
/// | 4 | `DeviceBusy` | `Ok(Unavailable)` |
/// | 5 | `RetriesExhausted` | `Ok(Failed)` |
/// | それ以外 | 未知 | `Err(Internal("biometric outcome"))` |
///
/// 呼び出し元（`windows_biometric.rs`）が Windows 限定モジュールのため、
/// 本関数自体には `cfg` を付けず全ターゲットでコンパイル・テスト対象にする
/// （macOS ホスト・Linux CI runner 上の `cargo test`/clippy でも失敗系を
/// 検証できるようにする。呼ばれない環境では `dead_code` にならないよう pub(crate)
/// のまま外へ公開する。呼び出し元が存在しない非 Windows ターゲットでは
/// `dead_code` を許容する（`device/share.rs::cancel_due` と同じ ponytail 方針:
/// 全ターゲットでテストするためだけに `cfg` を割らない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn outcome_from_verification(code: i32) -> Result<DeviceAuthOutcome, DeviceError> {
    match code {
        0 => Ok(DeviceAuthOutcome::Authenticated),
        6 => Ok(DeviceAuthOutcome::Cancelled),
        1..=4 => Ok(DeviceAuthOutcome::Unavailable),
        5 => Ok(DeviceAuthOutcome::Failed),
        _ => Err(DeviceError::Internal("biometric outcome")),
    }
}

/// macOS/iOS/Android/Windows 共通: OS 側コールバックとの橋渡し（pull 型、
/// `device/request_slot.rs::RequestSlot` の facade。モジュール doc 参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
pub(crate) mod bridge {
    use fandhe_core::screens::biometric_check::AuthOutcome as CoreAuthOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome` 後の形）。
    pub(crate) type Outcome = Result<CoreAuthOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果（`fandhe_device::biometric_auth::AuthOutcome` を経由した生の
    /// `Result`）を `super::to_outcome` で画面用へ変換してから置く（任意スレッド。
    /// 実際には macOS/iOS はメインスレッド、Android は UI スレッドから届く契約、
    /// `apple_biometric.rs`/`android.rs` の doc 参照）。`request_id` 不一致・未登録・
    /// 既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<fandhe_device::biometric_auth::AuthOutcome, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Android 専用: Activity 再生成直後に `android_main` から呼ぶ
    /// （`device::location::reset_retry_tracking` と同じ位置づけ）。[`SLOT`] は
    /// プロセス static のため Activity 再生成をまたいで残るが、画面状態
    /// （`BiometricCheckState`）は再生成のたびに `Default`（`Idle`）で作り直される。
    /// 放置すると、再生成前に開始した認証が OS 側（`MainActivity::biometricPrompt`、
    /// `FragmentManager` 経由で再接続される契約）でその後完了しても、
    /// [`super::drive_callback`] は新しい `Idle` 状態を見て早期 return するため、
    /// 結果が画面へ一切反映されず黙って失われる
    /// （codex-review 指摘、PR #917。#885 の camera/tts とは異なり Java 側が
    /// 完了を確定済みではなく実際に取りこぼす点が違う）。ここで [`SLOT`] を
    /// 丸ごと捨て、以後届く旧 `request_id` の [`deliver`] を `request_id` 不一致で
    /// 確実に無視させる。対になる後始末として Java 側
    /// （`MainActivity::onCreate`）は保留中の `BiometricPrompt` を
    /// `cancelAuthentication()` で明示的に閉じる。両者の呼び出し順に依存しない
    /// （[`deliver`] の照合はどちらが先でも安全）。
    ///
    /// 呼び出し元（`android::android_main`）は Android にのみ存在するため、それ以外の
    /// ターゲット（macOS/Windows の host 上の `cargo test`）では未使用になるが、
    /// `outcome_from_android_code` と同じ理由で `cfg` を割らず `allow(dead_code)` に
    /// 留め、[`bridge_tests`](super::bridge_tests) から host 上で検証できるようにする。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// OS へ提示を依頼する（Apple 系は `apple_biometric::present`、Android は
/// `android::request_biometric`）。戻り値は「提示依頼を受理できたか」（`false` は
/// 呼び出し元が `Err(Internal("biometric present"))` として畳む、`device/share.rs::present`
/// と同型）。Windows は戻り値が `Result`（`windows_biometric::present`）のため本関数には
/// 含めず、[`drive_callback`] 内で別途 `cfg(target_os = "windows")` 分岐として呼ぶ。
#[cfg(target_os = "android")]
fn present(request_id: u64, reason: &str) -> bool {
    super::android::request_biometric(request_id, reason)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, reason: &str) -> bool {
    super::apple_biometric::present(request_id, reason)
}

/// macOS/iOS/Android/Windows 経路の認証 effect ドライバ本体
/// （`device/share.rs::drive_callback` と同型）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
fn drive_callback(state: &BiometricCheckState, sender: &CompletionSender<BiometricCheckState>) {
    use fandhe_core::screens::biometric_check::AuthPhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_authenticating));

        let request_id = bridge::begin();
        match validate_reason(&request.reason) {
            Ok(reason) => {
                #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
                if !present(request_id, reason) {
                    // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                    bridge::deliver(request_id, Err(DeviceError::Internal("biometric present")));
                }
                #[cfg(target_os = "windows")]
                if let Err(e) = super::windows_biometric::present(request_id, reason) {
                    // 提示依頼自体が同期的に失敗した（完了ハンドラは二度と呼ばれない）。
                    bridge::deliver(request_id, Err(e));
                }
            }
            Err(e) => {
                // 入力検証で弾かれた（認証 UI は開かない）。
                bridge::deliver(request_id, Err(e));
            }
        }
        return;
    }

    if state.phase != AuthPhase::Authenticating {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut BiometricCheckState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_auth` し、反映できたときだけ `true` を返す（`RequestSlot` 側が
            // それを見て `clear` する）（`device/share.rs` の「キュー投入成功 ≠ 反映」
            // 対策と同じ判断）。
            if s.phase != AuthPhase::Idle {
                finish_auth(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// 上記 4 OS 以外（Linux）向け: `BiometricAuth` トレイト経由の同期呼び出し。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
fn drive(
    state: &BiometricCheckState,
    sender: &CompletionSender<BiometricCheckState>,
    auth: &dyn BiometricAuth,
) {
    let Some(request) = take_request(state) else {
        return;
    };

    let _ = sender.send(Box::new(mark_authenticating));

    let result = validate_reason(&request.reason).and_then(|reason| auth.authenticate(reason));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut BiometricCheckState| {
        finish_auth(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `biometric_check` 画面用に配線される effect
/// ドライバ（macOS/iOS/Android/Windows 版、モジュール doc「pull 型」参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
pub fn biometric_check_effect(
    state: &BiometricCheckState,
    sender: &CompletionSender<BiometricCheckState>,
) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `biometric_check` 画面用に配線される effect
/// ドライバ（macOS/iOS/Android/Windows 以外版。常に `Unsupported`）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
pub fn biometric_check_effect(
    state: &BiometricCheckState,
    sender: &CompletionSender<BiometricCheckState>,
) {
    drive(
        state,
        sender,
        &fandhe_device::biometric_auth::UnsupportedBiometricAuth,
    );
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照。
/// `device::location::reset_retry_tracking` と同じ命名・位置づけ）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

/// Android 側 `MainActivity` の結果コード（`nativeOnBiometricResult`）→
/// [`fandhe_device::biometric_auth::AuthOutcome`] の写像（Java 側の
/// `BIOMETRIC_RESULT_*` 定数・`biometricErrorToResultCode` が SSOT、
/// `MainActivity.java` のコメント参照）。未知のコードは失敗側（`Internal`）に倒す
/// （偽の成功・偽のキャンセルを作らない、`outcome_from_reply` と同じ方針）。
///
/// | code | 意味 |
/// |---|---|
/// | 0 | 認証した（`Authenticated`） |
/// | 1 | キャンセル（`Cancelled`） |
/// | 2 | 使えない（未登録・非対応ハードウェア等、`Unavailable`） |
/// | 3 | 失敗（不一致・ロックアウト等、`Failed`） |
/// | 4 | 内部エラー（提示依頼自体の失敗、`Err(Internal("biometric present"))`） |
/// | それ以外 | 未知（`Err(Internal("biometric outcome"))`） |
///
/// 呼び出し元（`android.rs::Java_..._nativeOnBiometricResult`）は Android にのみ
/// 存在する（`device.rs` の `mod android` cfg 参照）ため、それ以外のターゲット（例:
/// Linux CI runner 上の clippy）では本関数も未使用になり `dead_code` で
/// `-D warnings` に落ちる。`device/share.rs::cancel_due` と同じ理由で
/// `cfg_attr(allow(dead_code))` を付け、host（macOS/Windows/Linux）上でも
/// `cargo test` で写像表を検証できるようにする。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn outcome_from_android_code(code: i32) -> Result<DeviceAuthOutcome, DeviceError> {
    match code {
        0 => Ok(DeviceAuthOutcome::Authenticated),
        1 => Ok(DeviceAuthOutcome::Cancelled),
        2 => Ok(DeviceAuthOutcome::Unavailable),
        3 => Ok(DeviceAuthOutcome::Failed),
        4 => Err(DeviceError::Internal("biometric present")),
        _ => Err(DeviceError::Internal("biometric outcome")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    use fandhe_core::screens::biometric_check::authenticate;

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    fn sender_with_channel() -> (
        CompletionSender<BiometricCheckState>,
        std::sync::mpsc::Receiver<crate::app::Completion<BiometricCheckState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    fn apply_all(
        state: &mut BiometricCheckState,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<BiometricCheckState>>,
    ) {
        while let Ok(apply) = rx.try_recv() {
            apply(state);
        }
    }

    #[test]
    fn to_outcome_maps_all_variants() {
        assert_eq!(
            to_outcome(Ok(DeviceAuthOutcome::Authenticated)),
            Ok(CoreAuthOutcome::Authenticated)
        );
        assert_eq!(
            to_outcome(Ok(DeviceAuthOutcome::Cancelled)),
            Ok(CoreAuthOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(DeviceAuthOutcome::Unavailable)),
            Ok(CoreAuthOutcome::Unavailable)
        );
        assert_eq!(
            to_outcome(Ok(DeviceAuthOutcome::Failed)),
            Ok(CoreAuthOutcome::Failed)
        );
    }

    #[test]
    fn to_outcome_maps_unsupported_to_fixed_message() {
        let result = to_outcome(Err(DeviceError::Unsupported("biometric_auth")));
        assert_eq!(result, Err(UNAVAILABLE.to_string()));
    }

    #[test]
    fn outcome_from_verification_verified_is_authenticated() {
        assert!(matches!(
            outcome_from_verification(0),
            Ok(DeviceAuthOutcome::Authenticated)
        ));
    }

    #[test]
    fn outcome_from_verification_maps_canceled() {
        assert!(matches!(
            outcome_from_verification(6),
            Ok(DeviceAuthOutcome::Cancelled)
        ));
    }

    #[test]
    fn outcome_from_verification_maps_unavailable_codes() {
        for code in [1, 2, 3, 4] {
            assert!(
                matches!(
                    outcome_from_verification(code),
                    Ok(DeviceAuthOutcome::Unavailable)
                ),
                "code={code}"
            );
        }
    }

    #[test]
    fn outcome_from_verification_maps_retries_exhausted_to_failed() {
        assert!(matches!(
            outcome_from_verification(5),
            Ok(DeviceAuthOutcome::Failed)
        ));
    }

    #[test]
    fn outcome_from_verification_treats_unknown_code_as_internal_error() {
        for code in [-1, 7, 42, i32::MAX] {
            assert!(
                matches!(
                    outcome_from_verification(code),
                    Err(DeviceError::Internal("biometric outcome"))
                ),
                "code={code}"
            );
        }
    }

    /// 全コード（-1..=7）を総当たりし、`Authenticated` になるのは 0 だけであることを
    /// 固定する（偽の成功を作らない、モジュール doc 参照）。
    #[test]
    fn outcome_from_verification_only_zero_is_authenticated() {
        for code in -1..=7 {
            let is_authenticated = matches!(
                outcome_from_verification(code),
                Ok(DeviceAuthOutcome::Authenticated)
            );
            assert_eq!(is_authenticated, code == 0, "code={code}");
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_success_with_zero_code_is_authenticated() {
        assert!(matches!(
            outcome_from_reply(true, 0),
            Ok(DeviceAuthOutcome::Authenticated)
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_success_with_nonzero_code_is_internal_error() {
        // 契約上起きないはずの組み合わせ。偽の成功を作らない。
        assert!(matches!(
            outcome_from_reply(true, -1),
            Err(DeviceError::Internal("biometric outcome"))
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_maps_cancel_codes() {
        for code in [-2, -3, -4, -9] {
            assert!(
                matches!(
                    outcome_from_reply(false, code),
                    Ok(DeviceAuthOutcome::Cancelled)
                ),
                "code={code}"
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_maps_unavailable_codes() {
        for code in [-5, -6, -7] {
            assert!(
                matches!(
                    outcome_from_reply(false, code),
                    Ok(DeviceAuthOutcome::Unavailable)
                ),
                "code={code}"
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_maps_failed_codes() {
        for code in [-1, -8] {
            assert!(
                matches!(
                    outcome_from_reply(false, code),
                    Ok(DeviceAuthOutcome::Failed)
                ),
                "code={code}"
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn outcome_from_reply_treats_unknown_code_as_internal_error() {
        for code in [0, -10, -1004, 42] {
            assert!(
                matches!(
                    outcome_from_reply(false, code),
                    Err(DeviceError::Internal("biometric outcome"))
                ),
                "code={code}"
            );
        }
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = BiometricCheckState::default();
        drive(
            &state,
            &sender,
            &fandhe_device::biometric_auth::UnsupportedBiometricAuth,
        );
        assert!(rx.try_recv().is_err());
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_reports_unavailable_without_success_wording() {
        let (sender, rx) = sender_with_channel();
        let mut state = BiometricCheckState::default();
        authenticate(&mut state);

        drive(
            &state,
            &sender,
            &fandhe_device::biometric_auth::UnsupportedBiometricAuth,
        );
        apply_all(&mut state, &rx);

        assert!(state.error.contains(UNAVAILABLE), "{}", state.error);
        assert_eq!(state.status, "");
        assert_ne!(
            state.status,
            fandhe_core::screens::biometric_check::AUTHENTICATED_STATUS
        );
    }

    #[test]
    fn outcome_from_android_code_maps_table() {
        assert!(matches!(
            outcome_from_android_code(0),
            Ok(DeviceAuthOutcome::Authenticated)
        ));
        assert!(matches!(
            outcome_from_android_code(1),
            Ok(DeviceAuthOutcome::Cancelled)
        ));
        assert!(matches!(
            outcome_from_android_code(2),
            Ok(DeviceAuthOutcome::Unavailable)
        ));
        assert!(matches!(
            outcome_from_android_code(3),
            Ok(DeviceAuthOutcome::Failed)
        ));
        assert!(matches!(
            outcome_from_android_code(4),
            Err(DeviceError::Internal("biometric present"))
        ));
        for code in [5, -1] {
            assert!(
                matches!(
                    outcome_from_android_code(code),
                    Err(DeviceError::Internal("biometric outcome"))
                ),
                "code={code}"
            );
        }
    }
}

/// [`bridge`] の単体テスト。要求の追跡そのもの（採番・first-wins・照合・
/// Android 再生成時の `reset` 回帰、PR #917）は `device/request_slot.rs::RequestSlot`
/// 側で検証済みのため、ここでは facade が担う「`deliver` 境界での `to_outcome`
/// 変換」だけを固定する（Issue #943 で 3 機能の `bridge_tests` を統合、`share.rs` も
/// 同型）。OS 側実装（`.m`/`.java`/`windows_biometric.rs`）のビルドを要さず host 上の
/// `cargo test` で回せる。`bridge` モジュール自体は `cfg(any(...))` で 4 OS 共通だが、
/// iOS/Android はホスト上でテストを回せないため macOS/Windows のみを対象にする
/// （既存の macOS 限定と同じ理由）。
#[cfg(test)]
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod bridge_tests {
    use super::bridge;

    #[test]
    fn deliver_converts_raw_result_via_to_outcome() {
        let request_id = bridge::begin();
        assert!(bridge::deliver(
            request_id,
            Ok(fandhe_device::biometric_auth::AuthOutcome::Authenticated)
        ));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済みなので Some");
        assert_eq!(
            ready,
            Some(Ok(
                fandhe_core::screens::biometric_check::AuthOutcome::Authenticated
            ))
        );
        bridge::SLOT.clear(request_id);
    }
}
