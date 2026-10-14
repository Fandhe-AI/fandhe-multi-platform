//! Phase 11 の拡充ループ（親 #1145）型サンプル「OS 搭載フォント」（Issue #1376）の effect ドライバ。
//! `fandhe_core::screens::local_fonts_show`（端末依存ゼロ）の要求を `take_request` で観測し、
//! `fandhe_device::local_fonts::LocalFonts` を呼んで、結果を `CompletionSender` 経由で注入する
//! （`crate::screens::registry` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる
//! 契約、`app::EffectFn` 参照）。契約の SSOT は `fandhe_device::local_fonts`。
//!
//! # OS 別実装の差し込み口
//!
//! [`platform_local_fonts`] の cfg 分岐が全対象 OS の最終形で、各 OS は `implementation()` を
//! 持つモジュールだけを埋める（macOS: `macos_local_fonts`、Windows: `windows_local_fonts`、
//! iOS: `ios_local_fonts`（#1377）、Android: `android_local_fonts`（#1378））。それ以外は
//! `UnsupportedLocalFonts` 固定で、ダミーの成功実装へは倒さない。Web は本ドライバを通らない（#1379）。
//!
//! # 同期とワーカー
//!
//! macOS / iOS / Android は UI スレッドから同期で 1 回呼ぶ（[`drive_sync`]）。Windows は列挙が
//! 戻らない場合の備えのため、ワーカーで呼び 5 秒の期限を付ける（[`drive_worker`]）。期限切れ後に
//! 遅れて届いた結果は捨て、前回の OS 呼び出しが未終了の間は新しい呼び出しを起こさず失敗で確定する。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use fandhe_core::screens::local_fonts_show::{
    finish_read, mark_reading, take_request, LocalFontsReading, LocalFontsShowState, ReadFailure,
};
#[cfg(not(any(
    target_os = "windows",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
use fandhe_device::local_fonts::UnsupportedLocalFonts;
#[cfg(any(target_os = "macos", target_os = "ios", test))]
use fandhe_device::local_fonts::{families_from, require_named};
use fandhe_device::local_fonts::{LocalFontFamilies, LocalFonts};
use fandhe_device::DeviceError;

use crate::app::CompletionSender;

/// Windows のワーカーが結果を待つ上限。
const LIST_DEADLINE: Duration = Duration::from_secs(5);

/// 画面へ出す固定の失敗文言（OS の生コード・名前は出さない、security.md A09）。
const FAILED_MESSAGE: &str = "フォント一覧の読み取り";

/// OS 呼び出し（内側スレッド）が実行中か。期限切れで画面を失敗にしても `list_families()` が戻る
/// までは true のままで、その間は新しい OS スレッドを起こさない（戻らない実装でスレッドが累積しない）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static OS_CALL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 現在のプラットフォームに応じた `LocalFonts` 実装を返す（状態を持たないため呼び出しごとに構築）。
/// `Arc` なのは Windows でワーカーへ move するため。
fn platform_local_fonts() -> Arc<dyn LocalFonts> {
    #[cfg(target_os = "windows")]
    {
        super::windows_local_fonts::implementation()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_local_fonts::implementation()
    }
    #[cfg(target_os = "ios")]
    {
        super::ios_local_fonts::implementation()
    }
    #[cfg(target_os = "android")]
    {
        super::android_local_fonts::implementation()
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        Arc::new(UnsupportedLocalFonts)
    }
}

/// device の値を画面用の値へ写す。
fn to_reading(f: LocalFontFamilies) -> LocalFontsReading {
    LocalFontsReading {
        families: f.families,
        truncated: f.truncated,
    }
}

/// エラーを画面用の失敗へ写す。`PermissionDenied` は Web だけが画面側で使うため、ここでは作らない。
fn to_failure(e: DeviceError) -> ReadFailure {
    match e {
        DeviceError::Unsupported(_) => ReadFailure::Unsupported,
        _ => ReadFailure::Failed(FAILED_MESSAGE.to_string()),
    }
}

fn send_finish(
    sender: &CompletionSender<LocalFontsShowState>,
    generation: u64,
    result: Result<LocalFontsReading, ReadFailure>,
) {
    // 受信側がドロップ済み（終了中）の送信失敗は無視してよい。
    let _ = sender.send(Box::new(move |s: &mut LocalFontsShowState| {
        finish_read(s, generation, result);
    }));
}

/// 要求 1 件を UI スレッドで同期処理して完了を送る（macOS / iOS / Android 用。テストから任意の
/// 実装を差し込めるよう切り出す）。
#[cfg_attr(target_os = "windows", allow(dead_code))]
fn drive_sync(
    state: &LocalFontsShowState,
    sender: &CompletionSender<LocalFontsShowState>,
    fonts: &dyn LocalFonts,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let result = fonts.list_families().map(to_reading).map_err(to_failure);
    send_finish(sender, request.generation, result);
}

/// 要求 1 件をワーカーで処理して完了を送る（Windows 用）。`deadline` を過ぎたら失敗を確定し、
/// 遅れて届いた結果は受信側が drop 済みのため捨てる。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn drive_worker(
    state: &LocalFontsShowState,
    sender: &CompletionSender<LocalFontsShowState>,
    fonts: Arc<dyn LocalFonts>,
    deadline: Duration,
    in_flight: &'static AtomicBool,
) {
    let Some(request) = take_request(state) else {
        return;
    };
    let generation = request.generation;
    let failed = || Err(ReadFailure::Failed(FAILED_MESSAGE.to_string()));

    // 前回の OS 呼び出しが未終了なら追加で起こさず、失敗として確定する（資源の累積防止）。
    if in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        send_finish(sender, generation, failed());
        return;
    }

    // 次の再描画から `take_request` が `None` を返すようにし、二重に OS へ渡さない。
    let _ = sender.send(Box::new(mark_reading));

    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("local-fonts-list".to_string())
        .spawn(move || {
            let (tx, rx) = mpsc::channel();
            // 内側スレッドが OS を呼ぶ。期限切れで受信側を drop すると、遅れた結果は送れず捨てられる。
            let inner = std::thread::Builder::new()
                .name("local-fonts-os".to_string())
                .spawn(move || {
                    let r = fonts.list_families();
                    // list_families() が戻ってから解放する（期限切れ後でも戻るまでは次を起こさない）。
                    in_flight.store(false, Ordering::SeqCst);
                    let _ = tx.send(r);
                });
            let result = match inner {
                Ok(_) => match rx.recv_timeout(deadline) {
                    Ok(r) => r.map(to_reading).map_err(to_failure),
                    Err(_) => failed(),
                },
                Err(_) => {
                    in_flight.store(false, Ordering::SeqCst);
                    failed()
                }
            };
            send_finish(&worker_sender, generation, result);
        });
    if spawned.is_err() {
        in_flight.store(false, Ordering::SeqCst);
        // スレッドを起こせなくても要求を残さない（ボタンが戻らなくなるため）。
        send_finish(sender, generation, failed());
    }
}

/// `registry.rs` の `effect` 区画から `local_fonts_show` 画面用に配線される effect ドライバ。
pub fn local_fonts_show_effect(
    state: &LocalFontsShowState,
    sender: &CompletionSender<LocalFontsShowState>,
) {
    #[cfg(target_os = "windows")]
    drive_worker(
        state,
        sender,
        platform_local_fonts(),
        LIST_DEADLINE,
        &OS_CALL_IN_FLIGHT,
    );
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows ではワーカー経路の期限定数は使わない。
        let _ = LIST_DEADLINE;
        drive_sync(state, sender, platform_local_fonts().as_ref());
    }
}

/// OS 実装（`.m`）が詰めた NUL 区切りの名前列を契約の値へ写す。macOS / iOS 実装が共用する。
/// UTF-8 として不正な項目は捨てる。`count` は OS が返したファミリー数、`overflowed` はバッファ不足で
/// 取りこぼしたか。
#[cfg(any(target_os = "macos", target_os = "ios", test))]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(super) fn families_from_nul_buffer(
    buf: &[u8],
    count: u32,
    overflowed: bool,
) -> Result<LocalFontFamilies, DeviceError> {
    let names = buf
        .split(|&b| b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .map(str::to_owned);
    let mut f = require_named(count as usize, families_from(names))?;
    // 取りこぼしを成功の全件に見せない。
    f.truncated |= overflowed;
    Ok(f)
}

#[cfg(test)]
mod buffer_tests {
    use super::*;

    #[test]
    fn splits_cleans_sorts() {
        let f = families_from_nul_buffer(b"Beta\0Alpha\0Alpha\0", 3, false).unwrap();
        assert_eq!(f.families, ["Alpha", "Beta"]);
        assert!(!f.truncated);
    }

    #[test]
    fn empty_with_zero_count_is_ok_empty() {
        let f = families_from_nul_buffer(b"", 0, false).unwrap();
        assert!(f.families.is_empty());
    }

    #[test]
    fn no_names_with_positive_count_is_internal() {
        assert!(matches!(
            families_from_nul_buffer(b"", 2, false),
            Err(DeviceError::Internal("local_fonts list"))
        ));
        assert!(families_from_nul_buffer(b"\0\0", 2, false).is_err());
    }

    #[test]
    fn invalid_utf8_item_is_dropped() {
        let f = families_from_nul_buffer(b"A\0\xff\xfe\0B\0", 3, false).unwrap();
        assert_eq!(f.families, ["A", "B"]);
    }

    #[test]
    fn overflow_marks_truncated() {
        let f = families_from_nul_buffer(b"A\0", 5, true).unwrap();
        assert!(f.truncated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::local_fonts_show::{
        fetch, ReadPhase, READING_STATUS, UNSUPPORTED_STATUS,
    };
    use fandhe_device::local_fonts::{families_from, UnsupportedLocalFonts as Unsupported};
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        result: Option<LocalFontFamilies>,
        delay: Duration,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(result: Option<LocalFontFamilies>) -> Arc<Self> {
            Self::slow(result, Duration::ZERO)
        }
        fn slow(result: Option<LocalFontFamilies>, delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                result,
                delay,
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl LocalFonts for Fake {
        fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.result
                .clone()
                .ok_or(DeviceError::Internal("local_fonts list"))
        }
    }

    /// テストごとに独立した実行中フラグ（static を共有すると並列テストが干渉する）。
    fn flag() -> &'static AtomicBool {
        Box::leak(Box::new(AtomicBool::new(false)))
    }

    fn sample() -> LocalFontFamilies {
        families_from(["Alpha".to_string(), "Beta".to_string()])
    }

    type Rx = mpsc::Receiver<crate::app::Completion<LocalFontsShowState>>;

    fn channel() -> (CompletionSender<LocalFontsShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, egui::Context::default()), rx)
    }

    /// 要求済みの状態を作る（画面はボタン押下で要求を立てる）。
    fn requested() -> LocalFontsShowState {
        let mut state = LocalFontsShowState::default();
        fetch(&mut state);
        state
    }

    /// 要求を `drive_sync` し、届いた完了をすべて適用した状態を返す。
    fn run_sync(fonts: &dyn LocalFonts) -> LocalFontsShowState {
        let (sender, rx) = channel();
        let mut state = requested();
        drive_sync(&state, &sender, fonts);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        state
    }

    #[test]
    fn no_request_does_not_call_os() {
        let (sender, rx) = channel();
        let fake = Fake::new(Some(sample()));
        let idle = LocalFontsShowState::default();
        drive_sync(&idle, &sender, fake.as_ref());
        assert!(rx.try_recv().is_err());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn success_is_mapped() {
        let state = run_sync(Fake::new(Some(sample())).as_ref());
        let r = state.reading.expect("成功");
        assert_eq!(r.families, ["Alpha", "Beta"]);
        assert!(!r.truncated);
        assert_eq!(state.error, "");
    }

    #[test]
    fn unsupported_is_error_not_success() {
        let state = run_sync(&Unsupported);
        assert_eq!(state.error, UNSUPPORTED_STATUS);
        assert_eq!(state.reading, None);
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
        let mut state = requested();
        drive_sync(&state, &sender, Fake::new(Some(sample())).as_ref());
        // 完了が届く前に状態が別の世代へ進んだ想定。
        state.phase = ReadPhase::Idle;
        fetch(&mut state);
        let before = state.clone();
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert_eq!(state, before);
    }

    #[test]
    fn worker_marks_reading_first_then_finishes() {
        let (sender, rx) = channel();
        let mut state = requested();
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
        let mut state = requested();
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
        let mut state = requested();
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
        // 期限切れ後も前回の list_families() は未終了。再取得しても OS を呼ばず失敗になる。
        fetch(&mut state);
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
        fetch(&mut state);
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
            platform_local_fonts().list_families(),
            Err(DeviceError::Unsupported("local_fonts"))
        ));
    }
}
