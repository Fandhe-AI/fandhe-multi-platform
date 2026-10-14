//! Phase 11（端末機能の拡充ループ、親 #896）型サンプル「共有」（Issue #897 macOS →
//! #898 Windows → #899 iOS/Android 横展開）の effect ドライバ。
//! `fandhe_core::screens::share_text`（端末依存ゼロ）の要求を観測し、結果を
//! `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # macOS/iOS/Android/Windows: pull 型 `bridge`（OS の共有 UI はいずれもコールバック型）
//!
//! `NSSharingServicePicker`（macOS）/ `UIActivityViewController`（iOS）/
//! `Intent.createChooser`（Android）/ `DataTransferManager`（Windows）はいずれも
//! delegate・コールバック型（提示 → 結果を非同期通知）のため、`Share` トレイトを
//! 通さずコアの状態機械へ直接書き戻す（`device/file_pick.rs` の iOS/Android 経路と
//! 同型）。[`bridge`] は `device/request_slot.rs::RequestSlot` の facade で、
//! `CompletionSender` も画面の `generation` も保持しない pull 型: OS 側コールバックは
//! 結果を置くだけで、画面への反映は毎フレーム [`drive_callback`] が **その時点の**
//! `sender`/`state` を使って行う（`RequestSlot`/`poll_and_finish` のモジュール doc
//! 参照、Issue #943 で 3 機能の共通部品へ統合）。macOS/iOS/Android は [`present`] が
//! OS ごとの提示処理（`apple_share::present`/`android::request_share`）へ振り分ける。
//!
//! Windows のみ、`DataTransferManager` に「共有 UI を閉じた」イベントが無いため
//! （公知の制約）、`drive_callback` がフォーカス復帰ヒューリスティックでキャンセルを
//! 補う（[`cancel_due`] 参照）。
//!
//! # 上記 4 OS 以外（Linux）: `Share` トレイト経由
//!
//! Linux は [`UnsupportedShare`]（`fandhe_device::share::UnsupportedShare`）を
//! `Share` トレイト経由で呼び、`Unsupported("share")` を画面へ「この環境では共有
//! シートを使えません」として表示する。
//!
//! Web は本 Issue のスコープ外（親 #896 の子 (d)）。
//!
//! # 既知の天井（Issue #899 §2.6）
//!
//! Android/iOS とも、共有 UI 表示中に画面（`ShareTextState`）が破棄・再生成される
//! （Android の Activity 再生成が典型）と、後から届く結果は `phase != Idle` 判定で
//! 反映されず捨てられる（固着はしない）。無検証の再開 API は追加しない
//! （`file_pick` の `RestoreRequested` は本 Issue でも導入しない、実装計画 §2.6 参照）。

use fandhe_core::screens::share_text::{
    finish_share, mark_sharing, take_request, ShareOutcome as CoreShareOutcome, ShareTextState,
};
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
use fandhe_device::share::Share;
use fandhe_device::share::{ShareOutcome as DeviceShareOutcome, ShareRequest};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `Share`/`bridge::deliver` 未対応環境（Unsupported）で画面へ出す固定案内
/// （`DeviceError` の内部構造は含めない、security.md A09）。
const UNAVAILABLE: &str = "この環境では共有シートを使えません";

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言をそのまま転送する
/// （`device/file_pick.rs::to_message` と同じ方針）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE.to_string(),
        other => other.to_string(),
    }
}

/// `Result<DeviceShareOutcome, DeviceError>` を画面表示用の結果へ変換する
/// （macOS/iOS/Android/Windows 経路・`Share` トレイト経路の共通部分）。
fn to_outcome(result: Result<DeviceShareOutcome, DeviceError>) -> Result<CoreShareOutcome, String> {
    result
        .map(|outcome| match outcome {
            DeviceShareOutcome::Shared => CoreShareOutcome::Shared,
            DeviceShareOutcome::Cancelled => CoreShareOutcome::Cancelled,
            DeviceShareOutcome::NoTarget => CoreShareOutcome::NoTarget,
            DeviceShareOutcome::Unknown => CoreShareOutcome::Unknown,
        })
        .map_err(to_message)
}

/// OS 側の結果コード（iOS/macOS/Android の 3 経路で共通、Issue #899 §2.2 の表参照）を
/// [`fandhe_device::share::ShareOutcome`] へ写す純関数。`cfg` 無しで全ターゲットから
/// テストできる（`apple_share.rs::outcome_from_code`（#897）をここへ統合、iOS/Android を
/// 同じ表に載せるため）。未知のコードは `Internal("share outcome")` として扱う
/// （偽の成功・偽のキャンセルを作らない）。Windows（#898）は
/// `DataTransferManager`/`windows_share::present` 側で `Result` を直接返すため、
/// このコード表には乗らない。
///
/// | code | 意味 | iOS | macOS | Android |
/// |---|---|---|---|---|
/// | 0 | `Ok(Shared)` | completed==YES | service 非 nil | 選択 broadcast 受信 |
/// | 1 | `Ok(Cancelled)` | completed==NO&&error==nil | service nil | （使わない） |
/// | 2 | `Ok(NoTarget)` | （使わない） | services 空 | （使わない） |
/// | 3 | `Err(Internal("share present"))` | VC 不在/error!=nil | window 不在等 | launch() 例外 |
/// | 4 | `Ok(Unknown)` | （使わない） | （使わない） | chooser 終了・broadcast 未受信 |
///
/// 呼び出し元（`apple_share.rs`/`android.rs`）が macOS/iOS/Android の 3 OS にのみ
/// 存在する（`device.rs` の `mod` cfg 参照）ため、それ以外のターゲット（例:
/// Linux CI runner 上の clippy）では本関数も未使用になり `dead_code` で
/// `-D warnings` に落ちる。呼び出し元と同じ cfg を付けて対象を揃える。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) fn outcome_from_code(code: i32) -> Result<DeviceShareOutcome, DeviceError> {
    match code {
        0 => Ok(DeviceShareOutcome::Shared),
        1 => Ok(DeviceShareOutcome::Cancelled),
        2 => Ok(DeviceShareOutcome::NoTarget),
        3 => Err(DeviceError::Internal("share present")),
        4 => Ok(DeviceShareOutcome::Unknown),
        _ => Err(DeviceError::Internal("share outcome")),
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
    use fandhe_core::screens::share_text::ShareOutcome as CoreShareOutcome;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    /// 画面へ反映する完了結果（`super::to_outcome` 後の形）。
    pub(crate) type Outcome = Result<CoreShareOutcome, String>;

    pub(crate) static SLOT: RequestSlot<Outcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果（`fandhe_device::share::ShareOutcome` を経由した生の
    /// `Result`）を `super::to_outcome` で画面用へ変換してから置く（任意スレッド。
    /// 実際にはメインスレッドから届く契約、`apple_share.rs`/`android.rs` の doc
    /// 参照）。`request_id` 不一致・未登録・既に結果ありは捨てて `false`。
    pub(crate) fn deliver(
        request_id: u64,
        result: Result<fandhe_device::share::ShareOutcome, DeviceError>,
    ) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }
}

/// OS へ提示を依頼する（Apple 系は `apple_share::present`、Android は
/// `android::request_share`）。戻り値は「提示依頼を受理できたか」（`false` は
/// 呼び出し元が `Err(Internal("share present"))` として畳む）。Windows は戻り値の形が
/// 異なる（`Result` で失敗理由まで返す）ため、この `present` には含めず
/// [`drive_callback`] から `windows_share::present` を直接呼ぶ。
#[cfg(target_os = "android")]
fn present(request_id: u64, request: &ShareRequest) -> bool {
    super::android::request_share(request_id, request)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64, request: &ShareRequest) -> bool {
    super::apple_share::present(request_id, request)
}

/// Windows 限定: フォーカス喪失 → 復帰後にこの猶予を過ぎても結果未着ならキャンセル扱い
/// にする（`cancel_due` 参照）。
#[cfg(target_os = "windows")]
const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// `TargetApplicationChosen` の到着を待つ間、直前に共有 UI へフォーカスを奪われた
/// （`lost_focus`）状態から復帰してもなお `grace` 経過して未着なら `true`（キャンセル
/// 扱い）を返す純関数（OS 非依存、cfg 無しでテストできる。ponytail: 実イベント未提供の
/// 代替ヒューリスティックで、上流が「共有 UI を閉じた」イベントを提供するようになれば
/// 撤去できる）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn cancel_due(
    lost_focus: bool,
    regained_at: Option<std::time::Instant>,
    now: std::time::Instant,
    grace: std::time::Duration,
) -> bool {
    lost_focus && regained_at.is_some_and(|t| now.duration_since(t) >= grace)
}

/// Windows 限定: 直近の要求に対するフォーカス復帰キャンセル検知の状態
/// （`cancel_due` へ渡す入力を貯める）。`request_id` が不一致になったら新しい要求の
/// ものとして状態をリセットする（前回要求の残骸を持ち越さない）。
#[cfg(target_os = "windows")]
struct CancelTracking {
    request_id: u64,
    lost_focus: bool,
    regained_at: Option<std::time::Instant>,
}

#[cfg(target_os = "windows")]
static CANCEL_TRACKING: std::sync::Mutex<Option<CancelTracking>> = std::sync::Mutex::new(None);

/// Windows 限定: 今フレームのフォーカス状態を観測してキャンセル判定を更新する。
/// キャンセルと判定した場合のみ `true` を返す（呼び出し元が `bridge::deliver` する）。
#[cfg(target_os = "windows")]
fn windows_cancel_check(request_id: u64, focused: bool) -> bool {
    let now = std::time::Instant::now();
    let mut guard = CANCEL_TRACKING.lock().unwrap_or_else(|e| e.into_inner());
    let tracking = guard.get_or_insert_with(|| CancelTracking {
        request_id,
        lost_focus: false,
        regained_at: None,
    });
    if tracking.request_id != request_id {
        *tracking = CancelTracking {
            request_id,
            lost_focus: false,
            regained_at: None,
        };
    }
    if !focused {
        tracking.lost_focus = true;
        tracking.regained_at = None;
    } else if tracking.lost_focus && tracking.regained_at.is_none() {
        tracking.regained_at = Some(now);
    }
    cancel_due(tracking.lost_focus, tracking.regained_at, now, CANCEL_GRACE)
}

/// macOS/iOS/Android/Windows 経路の共有 effect ドライバ本体（旧 `drive_mac`。Windows
/// 追加により OS 中立の名前へ改名、モジュール doc「pull 型」参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
fn drive_callback(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    use fandhe_core::screens::share_text::SharePhase;

    if let Some(request) = take_request(state) {
        let _ = sender.send(Box::new(mark_sharing));

        let request_id = bridge::begin();
        match ShareRequest::new(&request.text, request.url.as_deref()) {
            Ok(share_request) => {
                #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
                if !present(request_id, &share_request) {
                    // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
                    bridge::deliver(request_id, Err(DeviceError::Internal("share present")));
                }
                #[cfg(target_os = "windows")]
                if let Err(e) = super::windows_share::present(request_id, &share_request) {
                    // 提示依頼自体が同期的に失敗した（`TargetApplicationChosen` は
                    // 二度と来ない）。
                    bridge::deliver(request_id, Err(e));
                }
            }
            Err(e) => {
                // 入力検証で弾かれた（共有 UI は開かない）。
                bridge::deliver(request_id, Err(e));
            }
        }
        return;
    }

    if state.phase != SharePhase::Sharing {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {
            // Windows 限定: `DataTransferManager` に「共有 UI を閉じた」イベントが
            // 無いため、フォーカス復帰ヒューリスティックでキャンセルを補う
            // （モジュール doc・`cancel_due` 参照）。
            #[cfg(target_os = "windows")]
            if windows_cancel_check(_request_id, sender.focused()) {
                bridge::deliver(
                    _request_id,
                    Ok(fandhe_device::share::ShareOutcome::Cancelled),
                );
            }
        },
        move |s: &mut ShareTextState, outcome| {
            // 反映できる状態か（`Idle` へ既に戻っていないか）を確かめてから
            // `finish_share` し、反映できたときだけ `true` を返す（`RequestSlot` 側が
            // それを見て `clear` する）（`device/file_pick.rs` の「キュー投入成功 ≠
            // 反映」対策と同じ判断）。
            if s.phase != SharePhase::Idle {
                finish_share(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// 上記 4 OS 以外（Linux）向け: `Share` トレイト経由の同期呼び出し。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
fn drive(state: &ShareTextState, sender: &CompletionSender<ShareTextState>, share: &dyn Share) {
    let Some(request) = take_request(state) else {
        return;
    };

    let _ = sender.send(Box::new(mark_sharing));

    let result = ShareRequest::new(&request.text, request.url.as_deref())
        .and_then(|share_request| share.share(&share_request));
    let outcome = to_outcome(result);
    let generation = request.generation;
    let _ = sender.send(Box::new(move |s: &mut ShareTextState| {
        finish_share(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `share_text` 画面用に配線される effect ドライバ
/// （macOS/iOS/Android/Windows 版、モジュール doc「pull 型」参照）。
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
))]
pub fn share_text_effect(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    drive_callback(state, sender);
}

/// `registry.rs` の `effect` 区画から `share_text` 画面用に配線される effect ドライバ
/// （上記 4 OS 以外版。Linux は常に `Unsupported`）。
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
pub fn share_text_effect(state: &ShareTextState, sender: &CompletionSender<ShareTextState>) {
    drive(state, sender, &fandhe_device::share::UnsupportedShare);
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
    use fandhe_core::screens::share_text::{set_input, set_url, share};

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    fn sender_with_channel() -> (
        CompletionSender<ShareTextState>,
        std::sync::mpsc::Receiver<crate::app::Completion<ShareTextState>>,
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
        state: &mut ShareTextState,
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<ShareTextState>>,
    ) {
        while let Ok(apply) = rx.try_recv() {
            apply(state);
        }
    }

    #[test]
    fn to_outcome_maps_all_variants() {
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Shared)),
            Ok(CoreShareOutcome::Shared)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Cancelled)),
            Ok(CoreShareOutcome::Cancelled)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::NoTarget)),
            Ok(CoreShareOutcome::NoTarget)
        );
        assert_eq!(
            to_outcome(Ok(DeviceShareOutcome::Unknown)),
            Ok(CoreShareOutcome::Unknown)
        );
    }

    #[test]
    fn to_outcome_maps_unsupported_to_fixed_message() {
        let result = to_outcome(Err(DeviceError::Unsupported("share")));
        assert_eq!(result, Err(UNAVAILABLE.to_string()));
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn outcome_from_code_maps_known_codes() {
        assert!(matches!(
            outcome_from_code(0),
            Ok(DeviceShareOutcome::Shared)
        ));
        assert!(matches!(
            outcome_from_code(1),
            Ok(DeviceShareOutcome::Cancelled)
        ));
        assert!(matches!(
            outcome_from_code(2),
            Ok(DeviceShareOutcome::NoTarget)
        ));
        assert!(matches!(
            outcome_from_code(3),
            Err(DeviceError::Internal("share present"))
        ));
        assert!(matches!(
            outcome_from_code(4),
            Ok(DeviceShareOutcome::Unknown)
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn outcome_from_code_treats_unknown_code_as_internal_error() {
        assert!(matches!(
            outcome_from_code(99),
            Err(DeviceError::Internal("share outcome"))
        ));
    }

    #[test]
    fn cancel_due_requires_lost_focus_and_grace_elapsed() {
        let now = std::time::Instant::now();
        let grace = std::time::Duration::from_millis(500);
        assert!(!cancel_due(false, Some(now), now, grace), "未喪失は対象外");
        assert!(!cancel_due(true, None, now, grace), "未復帰は対象外");
        assert!(
            !cancel_due(true, Some(now), now, grace),
            "復帰直後（猶予未経過）は対象外"
        );
        let past = now.checked_sub(grace).expect("減算できる");
        assert!(
            cancel_due(true, Some(past), now, grace),
            "喪失 → 復帰 → 猶予経過でキャンセル扱い"
        );
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
        let state = ShareTextState::default();
        drive(&state, &sender, &fandhe_device::share::UnsupportedShare);
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
        let mut state = ShareTextState::default();
        set_input(&mut state, "hello".to_string());
        share(&mut state);

        drive(&state, &sender, &fandhe_device::share::UnsupportedShare);
        apply_all(&mut state, &rx);

        assert!(state.error.contains(UNAVAILABLE), "{}", state.error);
        assert_eq!(state.status, "");
        assert_ne!(
            state.status,
            fandhe_core::screens::share_text::SHARED_STATUS
        );
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    #[test]
    fn drive_reports_invalid_url_as_error() {
        struct AlwaysShared;
        impl Share for AlwaysShared {
            fn share(&self, _request: &ShareRequest) -> Result<DeviceShareOutcome, DeviceError> {
                Ok(DeviceShareOutcome::Shared)
            }
        }

        let (sender, rx) = sender_with_channel();
        let mut state = ShareTextState::default();
        set_input(&mut state, "hello".to_string());
        set_url(&mut state, "file:///etc/passwd".to_string());
        share(&mut state);

        drive(&state, &sender, &AlwaysShared);
        apply_all(&mut state, &rx);

        assert!(state.error.contains("入力が不正です"), "{}", state.error);
        assert_ne!(
            state.status,
            fandhe_core::screens::share_text::SHARED_STATUS
        );
    }
}

/// [`bridge`] の単体テスト。要求の追跡そのもの（採番・first-wins・照合）は
/// `device/request_slot.rs::RequestSlot` 側で検証済みのため、ここでは facade が
/// 担う「`deliver` 境界での `to_outcome` 変換」だけを固定する（Issue #943 で
/// 3 機能の `bridge_tests` を統合、`biometric_auth.rs` も同型）。OS 側実装
/// （`.m`/`.java`/Windows COM interop）のビルドを要さず host 上の `cargo test` で
/// 回せる。iOS/Android の実装は本 host（macOS/Windows 前提の開発機）では
/// コンパイル対象外だが、`bridge` モジュール自体は `cfg(any(...))` で 4 OS 共通の
/// ため、macOS/Windows 上のテストでロジックを検証できる。
#[cfg(test)]
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod bridge_tests {
    use super::bridge;

    #[test]
    fn deliver_converts_raw_result_via_to_outcome() {
        let request_id = bridge::begin();
        assert!(bridge::deliver(
            request_id,
            Ok(fandhe_device::share::ShareOutcome::Shared)
        ));
        let (_, ready) = bridge::SLOT.snapshot().expect("begin 済みなので Some");
        assert_eq!(
            ready,
            Some(Ok(fandhe_core::screens::share_text::ShareOutcome::Shared))
        );
        bridge::SLOT.clear(request_id);
    }
}
