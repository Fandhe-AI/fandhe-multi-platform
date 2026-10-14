//! Phase 11（親 #921）型サンプル「資格情報」（Issue #922・#924）の effect ドライバ。
//! `fandhe_core::screens::credential_vault`（端末依存ゼロ）の `pending` 要求を観測し、
//! `fandhe_device::secure_credential::SecureCredential` で OS のセキュアストアを
//! 保存・読み出し・削除して `CompletionSender` 経由で結果を返す（`crate::screens::
//! registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、
//! `device/clipboard.rs` と同型）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。
//!
//! # 別スレッドで OS を呼ぶ理由（codex-review P1 指摘の是正）
//!
//! Keychain Services（`SecItem*`）・Android Keystore・Windows PasswordVault は
//! いずれも同期 API だが、OS 側がユーザー確認ダイアログ（Keychain アクセス許可等）を
//! 出す経路があり、`clipboard.rs`（3 OS とも即座に返る同期 API）とは異なりブロック
//! 時間の上限が無い。`ProxyApp::update`（UI スレッド）からそのまま呼ぶとユーザー応答
//! 待ちで画面全体が固まるため、`http_fetch.rs::spawn_fetch` と同型に別スレッドへ
//! 逃がす。`credential_vault::store/load/delete`（`fandhe-core` 側）は `pending` が
//! 立っている間は二重要求を防ぐため、`tts.rs` のような複数要求の順序保証キューは
//! 不要（常に高々 1 件）。

use std::sync::Arc;

use fandhe_core::screens::credential_vault::{
    finish_delete, finish_load, finish_store, mark_dispatched, take_request, CredentialVaultState,
    VaultRequest, UNAVAILABLE_MARKER,
};
use fandhe_device::secure_credential::SecureCredential;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は [`UNAVAILABLE_MARKER`]
/// （`fandhe-core` 側が `Outcome::Unavailable` へ畳む判定に使う SSOT 文字列）へ写し、
/// それ以外は `Display` の文言のみを転送する（`DeviceError` の内部構造は漏らさない、
/// security.md A09。秘密値は `DeviceError` のどのバリアントにも含まれない、
/// `secure_credential.rs::validate_secret` は値そのものをエラーへ含めない設計）。
fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => UNAVAILABLE_MARKER.to_string(),
        other => other.to_string(),
    }
}

/// 現在のプラットフォームに応じた `SecureCredential` 実装を返す。
///
/// iOS・macOS は [`super::apple_secure_credential::AppleKeychain`]、Android は
/// [`super::android::AndroidSecureCredential`]、Windows は
/// [`super::windows_secure_credential::WindowsPasswordVault`]。それ以外（Linux・Web）は
/// `UnsupportedSecureCredential`。
fn implementation() -> Arc<dyn SecureCredential> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SecureCredential>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(super::apple_secure_credential::AppleKeychain))
            .clone()
    }
    #[cfg(target_os = "android")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SecureCredential>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(super::android::AndroidSecureCredential))
            .clone()
    }
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SecureCredential>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(super::windows_secure_credential::WindowsPasswordVault))
            .clone()
    }
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        use std::sync::OnceLock;
        static INSTANCE: OnceLock<Arc<dyn SecureCredential>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(fandhe_device::secure_credential::UnsupportedSecureCredential))
            .clone()
    }
}

/// `credential_vault` の要求 1 件を処理して別スレッドで OS を呼び、完了を送る
/// （テストから任意の実装を差し込めるよう `credential_vault_effect` 本体から
/// 切り出す。`Arc` はスレッドへ持ち出すため、上記モジュール doc「別スレッドで OS
/// を呼ぶ理由」参照）。
fn drive(
    state: &CredentialVaultState,
    sender: &CompletionSender<CredentialVaultState>,
    cred: Arc<dyn SecureCredential>,
) {
    let Some(request) = take_request(state) else {
        return;
    };

    // 起動済みマーキング（二重起動防止、`http_fetch::spawn_fetch` の `mark_loading`
    // と同型）。`ProxyApp::update` は `effect` を毎フレーム呼ぶため、これが無いと
    // Keychain の許可待ち・ロック解除待ち中に同じ要求が毎フレーム別スレッドで
    // 再起動される（cursor-bot High 指摘）。`CompletionSender` 経由の送信は次フレーム
    // 先頭の `drain_completions` で反映されるため、この effect 呼び出し内で
    // `take_request` を再度呼んでも `state` はまだ更新されない
    // （`state` は `&` のためこの関数内では書き換えられない）が、次フレームには
    // 反映済みで `take_request` が `None` を返す。
    let _ = sender.send(Box::new(mark_dispatched));

    let sender = sender.clone();
    std::thread::spawn(move || match request {
        VaultRequest::Store { key, secret } => {
            let result = cred.store(&key, &secret).map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut CredentialVaultState| {
                finish_store(s, result);
            }));
        }
        VaultRequest::Load(key) => {
            let result = cred
                .load(&key)
                .map(|opt| opt.map(|secret| secret.expose().to_string()))
                .map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut CredentialVaultState| {
                finish_load(s, result);
            }));
        }
        VaultRequest::Delete(key) => {
            let result = cred.delete(&key).map_err(to_message);
            let _ = sender.send(Box::new(move |s: &mut CredentialVaultState| {
                finish_delete(s, result);
            }));
        }
    });
}

/// `registry.rs` の `effect` 区画から `credential_vault` 画面用に配線される
/// effect ドライバ。
pub fn credential_vault_effect(
    state: &CredentialVaultState,
    sender: &CompletionSender<CredentialVaultState>,
) {
    drive(state, sender, implementation());
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::credential_vault::{delete, load, set_input, set_key, store};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn sender_with_channel() -> (
        CompletionSender<CredentialVaultState>,
        std::sync::mpsc::Receiver<crate::app::Completion<CredentialVaultState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    /// インメモリモック（`fandhe-device` 側テストと同じ最小形）。
    #[derive(Default)]
    struct MemCredential(Arc<Mutex<HashMap<String, String>>>);

    impl SecureCredential for MemCredential {
        fn store(&self, key: &str, secret: &str) -> Result<(), DeviceError> {
            self.0
                .lock()
                .expect("lock は成功する")
                .insert(key.to_string(), secret.to_string());
            Ok(())
        }

        fn load(
            &self,
            key: &str,
        ) -> Result<Option<fandhe_device::secure_credential::Secret>, DeviceError> {
            Ok(self
                .0
                .lock()
                .expect("lock は成功する")
                .get(key)
                .cloned()
                .map(fandhe_device::secure_credential::Secret::new))
        }

        fn delete(&self, key: &str) -> Result<(), DeviceError> {
            self.0.lock().expect("lock は成功する").remove(key);
            Ok(())
        }
    }

    /// 完了を一切送らないダミー実装（`drive_does_not_respawn_while_dispatched` 用）。
    /// `MemCredential` は即座に完了するため、バックグラウンドスレッドの完了通知が
    /// 2 回目の `drive` 呼び出し・`try_recv` の直前に届く競合があり flaky だった
    /// （cursor-bot Low 指摘）。`store` を意図的にブロックし続けて応答待ち状態を
    /// 保つことで、テストが観測する「起動済みマーク以外は届かない」ことを
    /// タイミング非依存にする。
    struct BlockingCredential;

    impl SecureCredential for BlockingCredential {
        fn store(&self, _key: &str, _secret: &str) -> Result<(), DeviceError> {
            loop {
                std::thread::park();
            }
        }
        fn load(
            &self,
            _key: &str,
        ) -> Result<Option<fandhe_device::secure_credential::Secret>, DeviceError> {
            loop {
                std::thread::park();
            }
        }
        fn delete(&self, _key: &str) -> Result<(), DeviceError> {
            loop {
                std::thread::park();
            }
        }
    }

    /// 常に `Unsupported` を返す実装（`Unavailable` への畳み込みを検証する）。
    struct FailingCredential;

    impl SecureCredential for FailingCredential {
        fn store(&self, _key: &str, _secret: &str) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("secure_credential"))
        }
        fn load(
            &self,
            _key: &str,
        ) -> Result<Option<fandhe_device::secure_credential::Secret>, DeviceError> {
            Err(DeviceError::Unsupported("secure_credential"))
        }
        fn delete(&self, _key: &str) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("secure_credential"))
        }
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        let state = CredentialVaultState::default();
        drive(&state, &sender, Arc::new(MemCredential::default()));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drive_round_trips_store_load_delete() {
        let cred: Arc<dyn SecureCredential> = Arc::new(MemCredential::default());
        let (sender, rx) = sender_with_channel();
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "secret-value".to_string());

        store(&mut state);
        drive(&state, &sender, cred.clone());
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        assert!(fandhe_core::screens::credential_vault::take_request(&state).is_none());
        rx.recv().expect("保存完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::credential_vault::status_text(&state),
            "保存しました"
        );

        load(&mut state);
        drive(&state, &sender, cred.clone());
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("読み出し完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::credential_vault::status_text(&state),
            "読み出しました"
        );

        delete(&mut state);
        drive(&state, &sender, cred.clone());
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("削除完了が送られる")(&mut state);

        set_key(&mut state, "demo".to_string());
        load(&mut state);
        drive(&state, &sender, cred.clone());
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("読み出し完了が送られる")(&mut state);
        assert_eq!(
            fandhe_core::screens::credential_vault::status_text(&state),
            fandhe_core::screens::credential_vault::MISSING_STATUS
        );
    }

    /// 応答待ち中に `drive` が毎フレーム呼ばれても、起動済みの要求は再起動しない
    /// （cursor-bot High 指摘の是正）。
    #[test]
    fn drive_does_not_respawn_while_dispatched() {
        let (sender, rx) = sender_with_channel();
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "secret-value".to_string());
        store(&mut state);

        drive(&state, &sender, Arc::new(BlockingCredential));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        assert!(
            fandhe_core::screens::credential_vault::take_request(&state).is_none(),
            "起動済みマーク後は take_request が None を返す"
        );

        // 応答が来る前に毎フレームの effect 呼び出しを模倣しても、
        // 起動済みのため 2 回目以降は何も送られない。`BlockingCredential` は完了を
        // 送らないため、以降の `try_recv` は完了通知との競合なく安定して Err になる。
        drive(&state, &sender, Arc::new(BlockingCredential));
        assert!(
            rx.try_recv().is_err(),
            "起動済みの要求を再起動してはならない"
        );
    }

    #[test]
    fn drive_reports_unavailable_on_unsupported() {
        let (sender, rx) = sender_with_channel();
        let mut state = CredentialVaultState::default();
        set_key(&mut state, "demo".to_string());
        set_input(&mut state, "value".to_string());
        store(&mut state);

        drive(&state, &sender, Arc::new(FailingCredential));
        rx.recv().expect("起動済みマークが送られる")(&mut state);
        rx.recv().expect("完了は送られる")(&mut state);

        assert_eq!(
            fandhe_core::screens::credential_vault::status_text(&state),
            UNAVAILABLE_MARKER
        );
    }
}
