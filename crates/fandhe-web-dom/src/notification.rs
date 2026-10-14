//! Phase 11（端末機能の拡充ループ、親 #1003）型サンプル「通知」の Web 経路（Issue #1007）。
//!
//! `fandhe_core::screens::notification_send`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Web Notifications API
//! （`Notification`）でローカル通知を作成して結果を書き戻す。
//!
//! Web は `fandhe-device` の `Notifier` トレイトを通らない（`share.rs`/`tts.rs` と同じ
//! 理由。`fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計）。
//! 入力検証だけは [`fandhe_device::notification::NotificationRequest::new`] を共有する
//! （`share.rs::ShareRequest::new` と同じ検証の SSOT 方針）。
//!
//! # 権限状態ごとの流れ（対応表 `docs/device/capabilities/notification.json` の web 欄と対）
//!
//! | `Notification.permission`（呼び出し時点） | 動作 |
//! |---|---|
//! | API 無し・非 secure context | 何も呼ばず `Unsupported` |
//! | `granted` | 同期で `new Notification` → 表示結果を返す |
//! | `denied` | ダイアログを出さず `PermissionDenied` |
//! | `default` | 同期チェーン内で `requestPermission()` を呼び、解決を待ってから分岐 |
//!
//! **`requestPermission()` はクリックの同期チェーン内で呼ぶ**（user activation 要件、
//! `share.rs` の `navigator.share()` と同じ制約）。`await` はこの呼び出しより前に
//! 挟まない。
//!
//! # `Shown` は返さない（`Submitted` に留める）
//!
//! `new Notification` の生成成功は「ブラウザが OS へ渡した」ことしか表さない。
//! `show` イベントが発火しても、OS の通知設定（集中モード等）で抑止され得るため
//! 確認手段とはみなさない（iOS 実装 #1005 と同じ判断、`device-bridge-contracts.md`
//! 「6. 嘘の成功」参照）。将来 `show` イベントを確認手段とみなせる根拠が得られたら
//! `Shown` へ上げる余地はある。
//!
//! # 期限を張らない理由
//!
//! 権限プロンプトの待ち時間に上限を設けない（`camera_capture.rs` と同じ方針。
//! Rust 側・ブラウザ側とも期限無しで揃う、`device-bridge-contracts.md` 「3. 期限・
//! キャンセル」）。
//!
//! # ライフサイクル・世代
//!
//! `pagehide` やタブ切り替えで権限プロンプトが閉じられても、Promise は `default` 等で
//! 解決してキャンセルか失敗として確定するため、破棄処理を別途持つ必要はない
//! （`device-bridge-contracts.md` 「1. ライフサイクル境界」）。`notify()` は `Idle` の
//! ときしか要求を立てないため、`Sending` 中に次の世代は生まれず、古い完了は
//! `finish_notify` の世代照合で捨てられる（同「2. 並行性・世代」）。
//!
//! タイトル・本文・例外の `message` は console にも画面にも出さない
//! （security.md A09。エラーは例外の `name` のみを見て固定文言へ畳む）。

use fandhe_core::screens::notification_send::NotifyOutcome;
use fandhe_device::DeviceError;

/// 非対応ブラウザ・secure context 外・`Notification` 未定義時の案内。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`share.rs::UNAVAILABLE` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNAVAILABLE: &str = "この環境では通知を使えません（https か localhost が必要）";

/// `requestPermission()` が解決した値（`"granted"`/`"denied"`/`"default"`）を画面結果へ
/// 写す純関数（ブラウザ API を呼ばないため native からもテストできる。`share.rs::
/// outcome_from_error_name` と同じ理由で `wasm32` gate の外に置く）。
///
/// `granted` は表示へ進む合図として `None` を返す（呼び出し元が [`show`] 相当の処理へ
/// 続ける契約）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_permission(value: &str) -> Option<Result<NotifyOutcome, String>> {
    match value {
        "granted" => None,
        "denied" => Some(Err(
            DeviceError::PermissionDenied("notification").to_string()
        )),
        "default" => Some(Ok(NotifyOutcome::Cancelled)),
        _ => Some(Err(
            DeviceError::Internal("notification permission").to_string()
        )),
    }
}

/// `new Notification(..)` が投げた例外の `name` を画面結果へ写す純関数
/// （`outcome_from_permission` と同じ理由で native からもテストできる）。
///
/// `message` は受け取らない・転送しない（モジュール doc「タイトル・本文・例外の
/// message」節、security.md A09 の方針）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn error_from_construct_name(name: Option<&str>) -> String {
    match name {
        // Chrome Android 等、対応していない環境での `new Notification` は
        // `TypeError: Illegal constructor` を投げる。
        Some("TypeError") => UNAVAILABLE.to_string(),
        _ => DeviceError::Internal("notification show").to_string(),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use fandhe_core::screens::notification_send::{
        finish_notify, mark_sending, take_request, NotificationSendState, NotifyOutcome,
    };
    use fandhe_device::notification::NotificationRequest;
    use fandhe_device::DeviceError;
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Notification, NotificationOptions, NotificationPermission};

    use super::{error_from_construct_name, outcome_from_permission, UNAVAILABLE};
    use crate::mount::Applier;

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する
    /// （`share.rs::has_function` と同じ理由。型に現れない JS 側の実態を境界で
    /// 検証する、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `NotificationPermission` を [`outcome_from_permission`] が読める文字列へ変換する。
    /// `web-sys` はブラウザ側の将来の値追加に備えて本 enum を `non_exhaustive` として
    /// 生成するため、未知の値は `"default"`（= 呼び出し元が `requestPermission()` を
    /// 試みる）へ倒す。黙って成功・失敗のどちらかへ倒すより安全（security.md A05）。
    fn permission_str(permission: NotificationPermission) -> &'static str {
        match permission {
            NotificationPermission::Granted => "granted",
            NotificationPermission::Denied => "denied",
            NotificationPermission::Default => "default",
            _ => "default",
        }
    }

    /// `new Notification` を呼び、成功なら `Submitted`（モジュール doc「`Shown` は
    /// 返さない」節参照）、例外なら [`error_from_construct_name`] で固定文言へ写す。
    fn show(parsed: &NotificationRequest) -> Result<NotifyOutcome, String> {
        let options = NotificationOptions::new();
        options.set_body(parsed.body());
        match Notification::new_with_options(parsed.title(), &options) {
            Ok(_) => Ok(NotifyOutcome::Submitted),
            Err(err) => {
                let name = Reflect::get(&err, &JsValue::from_str("name"))
                    .ok()
                    .and_then(|v| v.as_string());
                Err(error_from_construct_name(name.as_deref()))
            }
        }
    }

    /// `mount_with_effect` へ渡す effect。`notification_send` 画面の通知要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn notification_send_effect(
        state: &NotificationSendState,
        applier: &Applier<NotificationSendState>,
    ) {
        let Some(request) = take_request(state) else {
            return;
        };

        // 次の再描画から `take_request` が `None` を返すようにする
        // （`share.rs` の「二重実行防止は不要」節と同じ理由）。
        applier.apply(mark_sending);
        let generation = request.generation;

        // 入力検証は `fandhe_device::notification::NotificationRequest` の SSOT に
        // 一本化する（モジュール doc参照）。
        let parsed = match NotificationRequest::new(&request.title, &request.body) {
            Ok(parsed) => parsed,
            Err(e) => {
                let message = e.to_string();
                applier.apply(move |s| finish_notify(s, generation, Err(message)));
                return;
            }
        };

        let Some(window) = web_sys::window() else {
            applier.apply(move |s| finish_notify(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        };
        let window_value: JsValue = window.clone().into();

        // secure context 外・`Notification` グローバル未定義の環境では、型に現れない
        // JS 側の実態を境界で確認してから呼ぶ（`share.rs::has_function` と同じ理由、
        // security.md A03）。未確認のまま `Notification::permission()` 等を呼ぶと
        // 存在しないグローバルの参照で例外になり `finish_notify` に到達しない。
        if !window.is_secure_context() || !has_function(&window_value, "Notification") {
            applier.apply(move |s| finish_notify(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        }

        match permission_str(Notification::permission()) {
            "granted" => {
                let result = show(&parsed);
                applier.apply(move |s| finish_notify(s, generation, result));
            }
            "denied" => {
                // 拒否済みは再ダイアログを出せない（ブラウザ仕様）。
                let result = Err(DeviceError::PermissionDenied("notification").to_string());
                applier.apply(move |s| finish_notify(s, generation, result));
            }
            _ => {
                // "default": クリックの同期チェーン内で `requestPermission()` を呼ぶ
                // （モジュール doc「権限状態ごとの流れ」節参照。ここより前に `await` を
                // 挟まない）。
                let promise_value: JsValue = match Notification::request_permission() {
                    Ok(promise) => promise.into(),
                    Err(_) => {
                        applier.apply(move |s| {
                            finish_notify(
                                s,
                                generation,
                                Err(DeviceError::Internal("notification permission").to_string()),
                            )
                        });
                        return;
                    }
                };

                // 旧 Safari 等、コールバック版しか持たない実装は Promise を返さない
                // （型に現れない JS 側の実態を境界で検証する、security.md A03）。
                if !has_function(&promise_value, "then") {
                    applier
                        .apply(move |s| finish_notify(s, generation, Err(UNAVAILABLE.to_string())));
                    return;
                }
                let promise = js_sys::Promise::from(promise_value);

                let applier = applier.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let result = match JsFuture::from(promise).await {
                        Ok(value) => {
                            let resolved = value.as_string().unwrap_or_default();
                            match outcome_from_permission(&resolved) {
                                None => show(&parsed),
                                Some(result) => result,
                            }
                        }
                        Err(_) => Err(DeviceError::Internal("notification permission").to_string()),
                    };
                    applier.apply(move |s| finish_notify(s, generation, result));
                });
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::notification_send_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granted_permission_proceeds_to_show() {
        assert_eq!(outcome_from_permission("granted"), None);
    }

    #[test]
    fn denied_permission_is_failure_not_success() {
        let outcome = outcome_from_permission("denied");
        assert_eq!(
            outcome,
            Some(Err(
                DeviceError::PermissionDenied("notification").to_string()
            ))
        );
    }

    #[test]
    fn default_permission_is_cancelled_not_failure() {
        assert_eq!(
            outcome_from_permission("default"),
            Some(Ok(NotifyOutcome::Cancelled))
        );
    }

    #[test]
    fn unknown_permission_is_generic_failure() {
        assert_eq!(
            outcome_from_permission("prompt"),
            Some(Err(
                DeviceError::Internal("notification permission").to_string()
            ))
        );
    }

    #[test]
    fn type_error_on_show_is_unavailable() {
        assert_eq!(error_from_construct_name(Some("TypeError")), UNAVAILABLE);
    }

    #[test]
    fn unknown_or_missing_name_on_show_is_generic_failure() {
        assert_eq!(
            error_from_construct_name(None),
            DeviceError::Internal("notification show").to_string()
        );
        assert_eq!(
            error_from_construct_name(Some("SomethingElse")),
            DeviceError::Internal("notification show").to_string()
        );
    }
}
