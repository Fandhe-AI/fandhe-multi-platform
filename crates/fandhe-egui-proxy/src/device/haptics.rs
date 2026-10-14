//! Phase 11（親 #1090）型サンプル「触覚フィードバック」（Issue #1091）の effect ドライバ。
//! `fandhe_core::screens::haptics_tap`（端末依存ゼロ）の要求を観測し、
//! `fandhe_device::haptics::Haptics` で OS へ軽い振動 1 回を要求して `CompletionSender`
//! 経由で結果を返す（`crate::screens::registry` の `effect` 区画から
//! `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn` 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。Web（wasm32）は本経路を通らず
//! `fandhe-web-dom` の `haptics.rs` が `navigator.vibrate` へ直接配線する。
//! 契約（`Ok` の意味・`Unsupported` の条件・連打）の SSOT は `fandhe_device::haptics`。
//!
//! # スレッド
//!
//! macOS / iOS は AppKit / UIKit が main スレッド前提のため、呼ばれたスレッド（egui の
//! update ループ = main）でそのまま呼ぶ（`.m` 側は main 以外なら触れずに失敗を返す）。
//! Android の `Vibrator` はスレッド制約が無く、JNI で同期に呼ぶ。Windows は WinRT の `.get()` が
//! 同期待ちになるため、UI を止めないようワーカースレッドで呼ぶ
//! （`windows_speech_recognition.rs` と同じ）。
//!
//! # 対応 OS
//!
//! macOS / iOS: [`super::apple_haptics::AppleHaptics`]、Windows: [`super::windows_haptics::
//! WindowsHaptics`]、Android: `super::android::AndroidHaptics`。それ以外（Linux）は
//! `UnsupportedHaptics` 固定（ダミーの成功実装へは倒さない）。

use std::sync::Arc;

use fandhe_core::screens::haptics_tap::{
    finish_tap, mark_sending, take_request, HapticsTapState, TapFailure,
};
use fandhe_device::haptics::Haptics;
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// 現在のプラットフォームに応じた `Haptics` 実装を返す（プロセスで 1 個）。
fn implementation() -> Arc<dyn Haptics> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Arc<dyn Haptics>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            #[cfg(target_os = "macos")]
            {
                Arc::new(super::apple_haptics::AppleHaptics)
            }
            #[cfg(target_os = "windows")]
            {
                Arc::new(super::windows_haptics::WindowsHaptics)
            }
            #[cfg(target_os = "ios")]
            {
                Arc::new(super::apple_haptics::AppleHaptics)
            }
            #[cfg(target_os = "android")]
            {
                Arc::new(super::android::AndroidHaptics)
            }
            #[cfg(not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "ios",
                target_os = "android"
            )))]
            {
                Arc::new(fandhe_device::haptics::UnsupportedHaptics)
            }
        })
        .clone()
}

/// `DeviceError` を画面の失敗種別へ写す。内部構造は `fandhe-core` へ漏らさず、
/// `Unsupported` 以外は `Display` の文言のみを転送する（security.md A09）。
fn to_failure(e: DeviceError) -> TapFailure {
    match e {
        DeviceError::Unsupported(_) => TapFailure::Unsupported,
        other => TapFailure::Failed(other.to_string()),
    }
}

/// 要求 1 件を処理して完了を送る（テストから任意の実装を差し込めるよう切り出す）。
fn drive(
    state: &HapticsTapState,
    sender: &CompletionSender<HapticsTapState>,
    haptics: Arc<dyn Haptics>,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    // 受信側のドロップ（アプリ終了中）による送信失敗は無視してよい。
    let _ = sender.send(Box::new(mark_sending));

    #[cfg(target_os = "windows")]
    {
        let worker_sender = sender.clone();
        let spawned = std::thread::Builder::new()
            .name("haptics-tap".to_string())
            .spawn(move || {
                let result = haptics.tap().map_err(to_failure);
                let _ = worker_sender.send(Box::new(move |s: &mut HapticsTapState| {
                    finish_tap(s, generation, result);
                }));
            });
        if spawned.is_err() {
            // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
            let _ = sender.send(Box::new(move |s: &mut HapticsTapState| {
                finish_tap(
                    s,
                    generation,
                    Err(TapFailure::Failed(
                        DeviceError::Internal("haptics worker").to_string(),
                    )),
                );
            }));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let result = haptics.tap().map_err(to_failure);
        let _ = sender.send(Box::new(move |s: &mut HapticsTapState| {
            finish_tap(s, generation, result);
        }));
    }
}

/// `registry.rs` の `effect` 区画から `haptics_tap` 画面用に配線される effect ドライバ。
pub fn haptics_tap_effect(state: &HapticsTapState, sender: &CompletionSender<HapticsTapState>) {
    drive(state, sender, implementation());
}

/// Java 側 `MainActivity.hapticsTap()` の戻り値（0 = 要求を渡した / 1 = 振動子なし /
/// それ以外 = 失敗）を契約のエラー種別へ写す。ホストでも検証できるよう OS 非依存の純関数。
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn decode_haptics_result(code: i32) -> Result<(), DeviceError> {
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::Unsupported("haptics")),
        _ => Err(DeviceError::Internal("haptics")),
    }
}

/// `SupportedFeedback` の波形 ID 列から使う要素の添字を選ぶ（Windows 実装の判定ロジック。
/// 実機なしで検証できるよう OS 非依存の純関数にしてある）。`click` と一致するものだけを選ぶ。
/// 無ければ `None`（呼び出し側が `Unsupported` にする）。先頭波形などへは落とさない
/// （長い・連続する振動が止まらず `tap` の「軽い単発」契約を破るため）。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(crate) fn select_feedback_index(waveforms: &[u16], click: u16) -> Option<usize> {
    waveforms.iter().position(|w| *w == click)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::haptics_tap::{tap, REQUESTED_STATUS, UNSUPPORTED_STATUS};
    use fandhe_device::haptics::UnsupportedHaptics;

    fn sender_with_channel() -> (
        CompletionSender<HapticsTapState>,
        std::sync::mpsc::Receiver<crate::app::Completion<HapticsTapState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    struct Fake(Result<(), fn() -> DeviceError>);

    impl Haptics for Fake {
        fn tap(&self) -> Result<(), DeviceError> {
            self.0.map_err(|f| f())
        }
    }

    /// 要求を立てて `drive` し、届いた完了をすべて適用した状態を返す。
    /// Windows のワーカー経路でも完了が届くまで待つ。
    fn run(haptics: Arc<dyn Haptics>) -> HapticsTapState {
        let (sender, rx) = sender_with_channel();
        let mut state = HapticsTapState::default();
        tap(&mut state);
        drive(&state, &sender, haptics);
        for _ in 0..2 {
            let c = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("mark_sending と完了が届く");
            c(&mut state);
        }
        state
    }

    #[test]
    fn drive_is_noop_without_pending_request() {
        let (sender, rx) = sender_with_channel();
        drive(&HapticsTapState::default(), &sender, Arc::new(Fake(Ok(()))));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_reports_requested() {
        let state = run(Arc::new(Fake(Ok(()))));
        assert_eq!(state.status, REQUESTED_STATUS);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_failure_not_success() {
        let state = run(Arc::new(UnsupportedHaptics));
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert!(!state.status.contains("要求しました"));
        assert_eq!(
            state.phase,
            fandhe_core::screens::haptics_tap::HapticsPhase::Idle
        );
    }

    #[test]
    fn internal_error_is_failure_with_prefix() {
        let state = run(Arc::new(Fake(Err(|| DeviceError::Internal("haptics")))));
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.status, "");
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            implementation().tap(),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn decode_haptics_result_maps_codes() {
        assert!(decode_haptics_result(0).is_ok());
        assert!(matches!(
            decode_haptics_result(1),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            decode_haptics_result(2),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            decode_haptics_result(-1),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn select_feedback_only_click_else_none() {
        assert_eq!(select_feedback_index(&[], 7), None);
        assert_eq!(select_feedback_index(&[3, 7, 9], 7), Some(1));
        assert_eq!(select_feedback_index(&[3, 9], 7), None);
    }
}
