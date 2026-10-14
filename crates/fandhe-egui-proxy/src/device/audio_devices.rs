//! Phase 11 の拡充ループ（親 #1136）型サンプル「音声デバイス」（Issue #1330）の effect ドライバ。
//! `fandhe_core::screens::audio_devices_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `fandhe_device::audio_devices::AudioDevices` を呼んで、結果を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる
//! 契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::audio_devices`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_audio_devices`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を
//! 持つモジュールだけを埋める（macOS: `macos_audio_devices`、Windows: `windows_audio_devices`、
//! iOS: `ios_audio_devices`（#1331）、Android: `android_audio_devices`（#1332））。それ以外は
//! `UnsupportedAudioDevices` 固定で、ダミーの成功実装へは倒さない。
//!
//! # 同期とワーカー
//!
//! macOS / iOS / Android は UI スレッドから同期で 1 回呼ぶ（[`drive_sync`]）。Windows は COM
//! 初期化の都合（UI スレッドは STA、実装は MTA）と戻らない場合の備えのため、ワーカーで呼び
//! 5 秒の期限を付ける（[`drive_worker`]）。期限切れ後に遅れて届いた結果は捨て、前回の OS 呼び出しが未終了の間は新しい呼び出しを起こさず失敗で確定する。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use fandhe_core::screens::audio_devices_show::{
    finish_read, mark_reading, take_request, AudioDevicesReading, AudioDevicesShowState,
    DeviceGroup, ReadFailure,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::audio_devices::UnsupportedAudioDevices;
use fandhe_device::audio_devices::{AudioDeviceList, AudioDevices, AudioDirection};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// Windows のワーカーが結果を待つ上限。
const LIST_DEADLINE: Duration = Duration::from_secs(5);

/// OS 呼び出し（内側スレッド）が実行中か。期限切れで画面を失敗にしても `list()` が戻るまでは
/// true のままで、その間は新しい OS スレッドを起こさない（戻らない実装でスレッドが累積しない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static OS_CALL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 現在のプラットフォームに応じた `AudioDevices` 実装を返す（状態を持たないため呼び出しごとに構築）。
/// `Arc` なのは Windows でワーカーへ move するため。
fn platform_audio_devices() -> Arc<dyn AudioDevices> {
    #[cfg(target_os = "windows")]
    {
        super::windows_audio_devices::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_audio_devices::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_audio_devices::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_audio_devices::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Arc::new(UnsupportedAudioDevices)
    }
}

fn to_group(d: AudioDirection) -> DeviceGroup {
    match d {
        AudioDirection::Listed(v) => DeviceGroup::Listed(v.into_iter().map(|d| d.name).collect()),
        AudioDirection::Unavailable => DeviceGroup::Unavailable,
        AudioDirection::OnlyWhileMicActive => DeviceGroup::OnlyWhileMicActive,
    }
}

/// device の値を画面用の値へ写す（`AfterMicPermission` は Web 専用なのでここでは作らない）。
fn to_reading(l: AudioDeviceList) -> AudioDevicesReading {
    AudioDevicesReading {
        inputs: to_group(l.inputs),
        outputs: to_group(l.outputs),
    }
}

/// エラーを画面用の失敗へ写す。画面へは固定文言のみ（OS の生コードは出さない、security.md A09）。
fn to_failure(e: DeviceError) -> ReadFailure {
    match e {
        DeviceError::Unsupported(_) => ReadFailure::Unsupported,
        _ => ReadFailure::Failed("音声デバイスの読み取り".to_string()),
    }
}

fn send_finish(
    sender: &CompletionSender<AudioDevicesShowState>,
    generation: u64,
    result: Result<AudioDevicesReading, ReadFailure>,
) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut AudioDevicesShowState| {
        finish_read(s, generation, result);
    }));
}

/// 要求 1 件を UI スレッドで同期処理して完了を送る（macOS / iOS / Android 用。テストから任意の
/// 実装を差し込めるよう切り出す）。
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn drive_sync(
    state: &AudioDevicesShowState,
    sender: &CompletionSender<AudioDevicesShowState>,
    devices: &dyn AudioDevices,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let result = devices.list().map(to_reading).map_err(to_failure);
    send_finish(sender, request.generation, result);
}

/// 要求 1 件をワーカーで処理して完了を送る（Windows 用）。`deadline` を過ぎたら失敗を確定し、
/// 遅れて届いた結果は受信側が drop 済みのため捨てる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn drive_worker(
    state: &AudioDevicesShowState,
    sender: &CompletionSender<AudioDevicesShowState>,
    devices: Arc<dyn AudioDevices>,
    deadline: Duration,
    in_flight: &'static AtomicBool,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;

    // 前回の OS 呼び出しが未終了なら追加で起こさず、失敗として確定する（資源の累積防止）。
    if in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        send_finish(
            sender,
            generation,
            Err(ReadFailure::Failed("音声デバイスの読み取り".to_string())),
        );
        return;
    }

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("audio-devices-list".to_string())
        .spawn(move || {
            let (tx, rx) = mpsc::channel();
            // 内側スレッドが OS を呼ぶ。期限切れで受信側を drop すると、遅れた結果は送れず捨てられる。
            let inner = std::thread::Builder::new()
                .name("audio-devices-os".to_string())
                .spawn(move || {
                    let r = devices.list();
                    // list() が戻ってから解放する（期限切れ後でも戻るまでは次を起こさない）。
                    in_flight.store(false, Ordering::SeqCst);
                    let _ = tx.send(r);
                });
            let result = match inner {
                Ok(_) => match rx.recv_timeout(deadline) {
                    Ok(r) => r.map(to_reading).map_err(to_failure),
                    Err(_) => Err(ReadFailure::Failed("音声デバイスの読み取り".to_string())),
                },
                Err(_) => {
                    in_flight.store(false, Ordering::SeqCst);
                    Err(ReadFailure::Failed("音声デバイスの読み取り".to_string()))
                }
            };
            send_finish(&worker_sender, generation, result);
        });
    if spawned.is_err() {
        in_flight.store(false, Ordering::SeqCst);
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        send_finish(
            sender,
            generation,
            Err(ReadFailure::Failed("音声デバイスの読み取り".to_string())),
        );
    }
}

/// `registry.rs` の `effect` 区画から `audio_devices_show` 画面用に配線される effect ドライバ。
pub fn audio_devices_show_effect(
    state: &AudioDevicesShowState,
    sender: &CompletionSender<AudioDevicesShowState>,
) {
    #[cfg(target_os = "windows")]
    drive_worker(
        state,
        sender,
        platform_audio_devices(),
        LIST_DEADLINE,
        &OS_CALL_IN_FLIGHT,
    );
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows ではワーカー経路の期限定数は使わない。
        let _ = LIST_DEADLINE;
        drive_sync(state, sender, platform_audio_devices().as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::audio_devices_show::{ReadPhase, READING_STATUS, UNSUPPORTED_STATUS};
    use fandhe_device::audio_devices::{listed, UnsupportedAudioDevices as Unsupported};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        result: Option<AudioDeviceList>,
        delay: Duration,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Option<AudioDeviceList>) -> Arc<Self> {
            Self::slow(result, Duration::ZERO)
        }
        fn slow(result: Option<AudioDeviceList>, delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                result,
                delay,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl AudioDevices for Fake {
        fn list(&self) -> Result<AudioDeviceList, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.result
                .clone()
                .ok_or(DeviceError::Internal("audio_devices list"))
        }
    }

    /// テストごとに独立した実行中フラグ（static を共有すると並列テストが干渉する）。
    fn flag() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    fn sample() -> AudioDeviceList {
        AudioDeviceList {
            inputs: listed([Some("Mic".into())]),
            outputs: AudioDirection::Unavailable,
        }
    }

    type Rx = mpsc::Receiver<crate::app::Completion<AudioDevicesShowState>>;

    fn channel() -> (CompletionSender<AudioDevicesShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 起動時の要求を `drive_sync` し、届いた完了をすべて適用した状態を返す。
    fn run_sync(devices: &dyn AudioDevices) -> AudioDevicesShowState {
        let (sender, rx) = channel();
        let mut state = AudioDevicesShowState::default();
        drive_sync(&state, &sender, devices);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(sample()));
        let idle = AudioDevicesShowState {
            phase: ReadPhase::Idle,
            ..Default::default()
        };
        drive_sync(&idle, &sender, fake.as_ref());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_mapped() {
        let state = run_sync(Fake::new(Some(sample())).as_ref());
        let r = state.reading.expect("成功");
        assert_eq!(r.inputs, DeviceGroup::Listed(vec![Some("Mic".into())]));
        assert_eq!(r.outputs, DeviceGroup::Unavailable);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let state = run_sync(&Unsupported);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
        assert_eq!(state.status, "");
    }

    #[test]
    fn internal_failure_is_not_success() {
        let state = run_sync(Fake::new(None).as_ref());
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.reading, None);
    }

    #[test]
    fn stale_generation_completion_is_dropped() {
        let (sender, rx) = channel();
        let mut state = AudioDevicesShowState::default();
        drive_sync(&state, &sender, Fake::new(Some(sample())).as_ref());
        // 完了が届く前に状態が別の世代へ進んだ想定。
        state.phase = ReadPhase::Idle;
        fandhe_core::screens::audio_devices_show::read(&mut state);
        let before = state.clone();
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state, before);
    }

    #[test]
    fn worker_marks_reading_first_then_finishes() {
        let (sender, rx) = channel();
        let mut state = AudioDevicesShowState::default();
        let fake = Fake::new(Some(sample()));
        let flag = flag();
        drive_worker(&state, &sender, fake.clone(), Duration::from_secs(5), flag);
        let first = rx.recv_timeout(Duration::from_secs(5)).expect("mark");
        first(&mut state);
        assert_eq!(state.status, READING_STATUS);
        // 処理中は再度ドライブしても OS を呼ばない。
        drive_worker(&state, &sender, fake.clone(), Duration::from_secs(5), flag);
        let second = rx.recv_timeout(Duration::from_secs(5)).expect("finish");
        second(&mut state);
        assert!(state.reading.is_some());
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn worker_deadline_is_failure_and_late_result_is_not_success() {
        let (sender, rx) = channel();
        let mut state = AudioDevicesShowState::default();
        let fake = Fake::slow(Some(sample()), Duration::from_millis(200));
        drive_worker(&state, &sender, fake, Duration::from_millis(20), flag());
        let mut applied = 0;
        while let Ok(c) = rx.recv_timeout(Duration::from_millis(500)) {
            c(&mut state);
            applied += 1;
        }
        assert_eq!(applied, 2, "mark_reading と失敗の 2 件だけ");
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        assert_eq!(state.reading, None);
    }

    #[test]
    fn worker_does_not_spawn_while_previous_call_is_unfinished() {
        let flag = flag();
        let fake = Fake::slow(Some(sample()), Duration::from_millis(300));
        let (sender, rx) = channel();
        let mut state = AudioDevicesShowState::default();
        drive_worker(
            &state,
            &sender,
            fake.clone(),
            Duration::from_millis(20),
            flag,
        );
        while let Ok(c) = rx.recv_timeout(Duration::from_millis(150)) {
            c(&mut state);
        }
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        // 期限切れ後も前回の list() は未終了。再取得しても OS を呼ばず失敗になる。
        fandhe_core::screens::audio_devices_show::read(&mut state);
        drive_worker(
            &state,
            &sender,
            fake.clone(),
            Duration::from_millis(20),
            flag,
        );
        while let Ok(c) = rx.recv_timeout(Duration::from_millis(100)) {
            c(&mut state);
        }
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert!(state.error.starts_with("失敗しました: "), "{}", state.error);
        // 前回が戻れば次を起動できる。
        std::thread::sleep(Duration::from_millis(400));
        fandhe_core::screens::audio_devices_show::read(&mut state);
        drive_worker(&state, &sender, fake.clone(), Duration::from_secs(5), flag);
        while let Ok(c) = rx.recv_timeout(Duration::from_millis(800)) {
            c(&mut state);
        }
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
        assert!(state.reading.is_some());
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn unsupported_os_implementation_is_unsupported() {
        assert!(matches!(
            platform_audio_devices().list(),
            Err(DeviceError::Unsupported("audio_devices"))
        ));
    }
}
