//! Phase 11（端末機能の拡充ループ、親 #1117）型サンプル「連絡先選択」（Issue #1220 macOS）の
//! effect ドライバ。`fandhe_core::screens::contact_pick`（端末依存ゼロ）の要求を観測し、
//! 結果を `CompletionSender` 経由で返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。契約本体は
//! `fandhe_device::contact_picker` のモジュール doc。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # macOS / iOS: pull 型 `bridge`
//!
//! `CNContactPicker`（macOS）/ `CNContactPickerViewController`（iOS、Issue #1221）のデリゲートは非同期に届くため、`ContactPicker` トレイトを通さず
//! `biometric_auth` と同じ `device/request_slot.rs::RequestSlot` の facade で受ける。
//! 画面への反映は毎フレーム [`drive_callback`] が行う。
//!
//! # Android（Issue #1222）
//!
//! `bridge` は macOS / iOS と共有。提示は `device/android.rs::request_contact_pick`、結果は
//! `nativeOnContactPickerResult` が Java のコード 0（選択）/ 1（キャンセル）/ 3（提示・
//! 読み取り失敗）で届ける（インテント経由なので 2 = 権限拒否は送られない）。Activity 再生成で
//! 保留中の要求は復元せず破棄する（`android_main` が [`reset_pending_tracking`] を呼ぶ）。
//!
//! # macOS / iOS / Android 以外
//!
//! Web は兄弟 issue（#1223）が実装するまで、
//! `UnsupportedContactPicker` 経由で `Unsupported` を画面へ出す（成功にしない）。

use fandhe_core::screens::contact_pick::{ContactPickOutcome, ContactPickState};
use fandhe_device::contact_picker::{validate_contact_name, PickedContact};
use fandhe_device::DeviceError;

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
use fandhe_core::screens::contact_pick::{finish_pick, mark_picking, take_request};
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
use fandhe_device::contact_picker::ContactPicker;

use crate::app::CompletionSender;

/// `Result<PickedContact, DeviceError>` を画面用の結果へ畳む。表示文字列に名前は入らない
/// （`DeviceError` のメッセージは内部識別子のみ）。
fn to_outcome(result: Result<PickedContact, DeviceError>) -> ContactPickOutcome {
    match result {
        Ok(c) => ContactPickOutcome::Picked(c.display_name),
        Err(DeviceError::Cancelled(_)) => ContactPickOutcome::Cancelled,
        Err(DeviceError::PermissionDenied(_)) => ContactPickOutcome::PermissionDenied,
        Err(DeviceError::Unsupported(_)) => ContactPickOutcome::Unsupported,
        Err(e) => ContactPickOutcome::Failed(e.to_string()),
    }
}

/// `.m` 側の生コード → 結果の写像（SSOT）。0 = 選択（名前なしは `Internal`、空名は
/// [`validate_contact_name`] が `Internal`）、1 = キャンセル、2 = 権限拒否、
/// 3 と未知コード = `Internal`（偽のキャンセル・偽の成功を作らない）。
/// 呼び出し元は macOS / iOS / Android 限定のため、ホストでのテスト用に `cfg` は割らない。
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "android")),
    allow(dead_code)
)]
pub(crate) fn outcome_from_code(
    code: i32,
    name: Option<String>,
) -> Result<PickedContact, DeviceError> {
    match code {
        0 => match name {
            Some(n) => validate_contact_name(n),
            None => Err(DeviceError::Internal("contact_picker name")),
        },
        1 => Err(DeviceError::Cancelled("contact_picker")),
        2 => Err(DeviceError::PermissionDenied("contact_picker")),
        3 => Err(DeviceError::Internal("contact_picker present")),
        _ => Err(DeviceError::Internal("contact_picker outcome")),
    }
}

/// macOS / iOS / Android: OS コールバックとの橋渡し（pull 型、`RequestSlot` の facade）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
pub(crate) mod bridge {
    use fandhe_core::screens::contact_pick::ContactPickOutcome;
    use fandhe_device::contact_picker::PickedContact;
    use fandhe_device::DeviceError;

    use crate::device::request_slot::RequestSlot;

    pub(crate) static SLOT: RequestSlot<ContactPickOutcome> = RequestSlot::new();

    /// 提示直前に呼ぶ。戻り値の `request_id` を OS へ渡す。
    pub(crate) fn begin() -> u64 {
        SLOT.begin()
    }

    /// OS からの結果を画面用へ変換して置く（main queue から届く契約）。`request_id` 不一致・
    /// 未登録・既に結果ありは捨てて `false`（古い世代・二重通知が成功へ化けない）。
    pub(crate) fn deliver(request_id: u64, result: Result<PickedContact, DeviceError>) -> bool {
        SLOT.deliver(request_id, super::to_outcome(result))
    }

    /// Activity 再生成直後に `android_main` から呼ぶ。保留中の要求を捨て、以後届く旧
    /// `request_id` の [`deliver`] を不一致で無視させる（契約は「復元せず破棄」）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn reset() {
        SLOT.reset()
    }
}

/// `android_main` から呼ぶ薄いエイリアス（[`bridge::reset`] 参照）。
#[cfg(target_os = "android")]
pub(crate) fn reset_pending_tracking() {
    bridge::reset();
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn present(request_id: u64) -> bool {
    super::apple_contact_picker::present(request_id)
}

#[cfg(target_os = "android")]
fn present(request_id: u64) -> bool {
    super::android::request_contact_pick(request_id)
}

/// macOS / iOS / Android の effect ドライバ本体（`biometric_auth::drive_callback` と同型）。
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
fn drive_callback(state: &ContactPickState, sender: &CompletionSender<ContactPickState>) {
    use fandhe_core::screens::contact_pick::{finish_pick, mark_picking, take_request, PickPhase};

    if take_request(state).is_some() {
        let _ = sender.send(Box::new(mark_picking));
        let request_id = bridge::begin();
        if !present(request_id) {
            // 提示依頼自体が同期的に失敗した（コールバックは二度と来ない）。
            bridge::deliver(
                request_id,
                Err(DeviceError::Internal("contact_picker present")),
            );
        }
        return;
    }

    if state.phase != PickPhase::Picking {
        return;
    }
    let generation = state.generation;
    super::request_slot::poll_and_finish(
        &bridge::SLOT,
        sender,
        |_request_id| {},
        move |s: &mut ContactPickState, outcome| {
            // `Idle` へ既に戻っていたら反映せず、`RequestSlot` を残す（clear しない）。
            if s.phase != PickPhase::Idle {
                finish_pick(s, generation, outcome);
                true
            } else {
                false
            }
        },
    );
}

/// macOS / iOS / Android 以外: `ContactPicker` トレイト経由の同期呼び出し。
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
fn drive(
    state: &ContactPickState,
    sender: &CompletionSender<ContactPickState>,
    picker: &dyn ContactPicker,
) {
    let Some(generation) = take_request(state) else {
        return;
    };
    let _ = sender.send(Box::new(mark_picking));
    let outcome = to_outcome(picker.pick());
    let _ = sender.send(Box::new(move |s: &mut ContactPickState| {
        finish_pick(s, generation, outcome);
    }));
}

/// `registry.rs` の `effect` 区画から `contact_pick` 画面用に配線される effect ドライバ。
pub fn contact_pick_effect(state: &ContactPickState, sender: &CompletionSender<ContactPickState>) {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    drive_callback(state, sender);
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    drive(
        state,
        sender,
        &fandhe_device::contact_picker::UnsupportedContactPicker,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(name: &str) -> PickedContact {
        PickedContact {
            display_name: name.to_string(),
        }
    }

    #[test]
    fn to_outcome_maps_all_branches() {
        assert_eq!(
            to_outcome(Ok(picked("A"))),
            ContactPickOutcome::Picked("A".into())
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Cancelled("contact_picker"))),
            ContactPickOutcome::Cancelled
        );
        assert_eq!(
            to_outcome(Err(DeviceError::PermissionDenied("contact_picker"))),
            ContactPickOutcome::PermissionDenied
        );
        assert_eq!(
            to_outcome(Err(DeviceError::Unsupported("contact_picker"))),
            ContactPickOutcome::Unsupported
        );
        assert!(matches!(
            to_outcome(Err(DeviceError::Internal("x"))),
            ContactPickOutcome::Failed(_)
        ));
    }

    #[test]
    fn outcome_from_code_table() {
        assert_eq!(outcome_from_code(0, Some("A".into())).unwrap(), picked("A"));
        for bad in [
            outcome_from_code(0, None),
            outcome_from_code(0, Some(" ".into())),
        ] {
            assert!(matches!(bad, Err(DeviceError::Internal(_))));
        }
        assert!(matches!(
            outcome_from_code(1, None),
            Err(DeviceError::Cancelled(_))
        ));
        assert!(matches!(
            outcome_from_code(2, None),
            Err(DeviceError::PermissionDenied(_))
        ));
        for code in [3, -1, 99] {
            assert!(matches!(
                outcome_from_code(code, Some("A".into())),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
    #[test]
    fn stale_and_duplicate_delivery_never_becomes_success() {
        let id = bridge::begin();
        assert!(!bridge::deliver(id.wrapping_add(1), Ok(picked("A"))));
        assert!(bridge::deliver(
            id,
            Err(DeviceError::Cancelled("contact_picker"))
        ));
        // 二重通知は first-wins で、成功へ化けない。
        assert!(!bridge::deliver(id, Ok(picked("A"))));
        assert_eq!(
            bridge::SLOT.snapshot(),
            Some((id, Some(ContactPickOutcome::Cancelled)))
        );
        // 再生成後の遅延完了（reset 後の同じ id）も成功へ化けない。
        let id = bridge::begin();
        bridge::reset();
        assert!(!bridge::deliver(id, Ok(picked("A"))));
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
    #[test]
    fn unsupported_is_reflected_not_success() {
        use fandhe_core::screens::contact_pick::{request_pick, PickPhase};
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sender = CompletionSender::new(tx, ctx);
        let mut state = ContactPickState::default();
        request_pick(&mut state);
        contact_pick_effect(&state, &sender);
        while let Ok(apply) = rx.try_recv() {
            apply(&mut state);
        }
        assert_eq!(state.phase, PickPhase::Idle);
        assert!(state.picked.is_none());
        assert_eq!(
            state.error,
            fandhe_core::screens::contact_pick::UNSUPPORTED_TEXT
        );
    }
}
