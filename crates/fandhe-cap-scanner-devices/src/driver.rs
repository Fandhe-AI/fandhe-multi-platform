//! Phase 11（親 #192）型サンプル「接続した機器からの取り込み」（Issue #981）の effect ドライバ。
//! `fandhe_core::screens::scanner_devices_import`（端末依存ゼロ）の要求を観測し、ワーカースレッドで
//! `crate::contract::ScannerDevices` を呼んで `CompletionSender` 経由で結果を返す
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)` として
//! 呼ばれる契約、`app::EffectFn` 参照）。契約（結果の種類・期限・守る範囲）の SSOT は
//! `crate::contract`。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる。
//!
//! # OS 別の分担（dispatch は #981 で最終形。後続の子は自分の OS のモジュールだけを埋める）
//!
//! - macOS: [`crate::apple::AppleScannerDevices`]（#981。ImageCaptureCore）
//! - iOS: `ios_scanner_devices`（#982。macOS と共用の `AppleScannerDevices` へ委譲し、許可・背景遷移は `.m` の iOS 区画）
//! - Android: `android_scanner_devices`（#983。`UsbManager` + `MtpDevice`）
//! - Windows: `windows_scanner_devices`（#984。WinRT `ImageScanner`）
//! - Web・Linux: `UnsupportedScannerDevices` 固定（ダミーの成功実装へは倒さない）
//!
//! # 期限（契約表 3）
//!
//! 期限は本ドライバだけが持つ。起点は要求を観測した時の `Instant`（単調時計）。毎フレーム、
//! 処理中で結果が未着のとき `elapsed >= deadline` なら `TimedOut` を確定してスロットを空にし、
//! まだなら `poll_and_finish` の再描画予約で次の判定に回る。確定後にワーカーが返す結果は画面側の
//! 世代・状態照合（`finish_import`）で捨てられる。ワーカー自身も期限超過を `TimedOut` にし、
//! 期限後に届いた成功を成功として扱わない。
//!
//! 成功値は 2 段の検査（`ImportedImage::from_os` とここでの復号）を通った画像だけ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::contract::{DeviceKind, IMPORT_DEADLINE, ScannerDevices, ScannerImportFailure};
use fandhe_core::screens::scanner_devices_import::{
    CoreFailure, CoreKind, Phase, ScannerDevicesImportState, ShownImage, finish_import,
    mark_running, take_request,
};

use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot, poll_and_finish};
use fandhe_runtime::{CompletionSender, image_store};

/// 復号を許す辺の最大（画素）。
const MAX_SIDE: u32 = 16384;
/// 復号中の確保上限（バイト）。
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// 表示用の複製の辺の上限（画素）。`RgbaPixels` の上限（2048）に収まる。
// ponytail: 写真選択（egui-proxy の `photo_pick`）・書類スキャン（`fandhe-cap-document-scan`）と同じ値・同じ縮小処理を
// 複製している。photo-picker を移すときに 3 か所の共有の置き場を決める。
const DISPLAY_SIDE: u32 = 1024;

/// 復号確認を通った画像。
struct Decoded {
    image: fandhe_core::RgbaPixels,
    width: u32,
    height: u32,
}

/// 2 段目の検査: 実際に復号できるかを確かめ、表示用の画素を作る。失敗は `Err(())`
/// （画像の中身・OS のエラー文は持ち出さない）。
fn decode_for_display(photo: &fandhe_device::photo_picker::PickedPhoto) -> Result<Decoded, ()> {
    use fandhe_device::photo_picker::ImageFormat;
    let format = match photo.format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        // feature を持たない。成功にせず失敗として返す。
        ImageFormat::Gif | ImageFormat::Webp => return Err(()),
    };
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(&photo.bytes[..]), format);
    // `Limits` は non_exhaustive のため、既定値から書き換える。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| ())?;
    let (width, height) = (decoded.width(), decoded.height());
    // thumbnail は小さい画像を拡大しうるため、大きいときだけ縮める。
    let shown = if width > DISPLAY_SIDE || height > DISPLAY_SIDE {
        decoded.thumbnail(DISPLAY_SIDE, DISPLAY_SIDE)
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    // 表示辺 DISPLAY_SIDE 以下なので `RGBA_MAX_SIDE` に収まる。外れたら成功にせず失敗にする。
    let image =
        fandhe_core::RgbaPixels::new(rgba.width(), rgba.height(), rgba.into_raw()).ok_or(())?;
    Ok(Decoded {
        image,
        width,
        height,
    })
}

/// 復号済みの画像。`RequestSlot` が `Clone` を要るため画素は `Arc` で持つ。
#[derive(Clone)]
struct Imported {
    decoded: Arc<Decoded>,
    kind: DeviceKind,
    found: u32,
}

type Outcome = Result<Imported, ScannerImportFailure>;

/// 現在のプラットフォームに応じた `ScannerDevices` 実装を返す（dispatch の最終形）。
/// 画面の状態型に依存しないため、フレームワークの外のアプリも使える。
pub fn platform_scanner_devices() -> Arc<dyn ScannerDevices> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(crate::apple::AppleScannerDevices)
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    #[cfg(target_os = "windows")]
    {
        crate::windows::implementation()
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )))]
    {
        Arc::new(crate::contract::UnsupportedScannerDevices)
    }
}

/// ドライバの可変状態。要求の追跡は [`RequestSlot`] に任せ、ここは期限の起点だけを持つ。
/// テストが並列で干渉しないよう static から切り出して引数で渡す
/// （`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Session {
    slot: &'static RequestSlot<Outcome>,
    /// 進行中の要求を観測した時刻（単調時計）。
    started: Mutex<Option<Instant>>,
    /// 前回ワーカーの終了待ちで発送できずにいる要求（世代, 観測した時刻）。待機にも期限を適用する起点。
    waiting: Mutex<Option<(u64, Instant)>>,
    /// OS 操作を持つワーカーが走っている間 true。期限切れ表示でスロットを空にしても、
    /// ワーカー（機器の閉鎖まで済ませてから戻る）が終わるまで次の発送を止める。
    os_active: Arc<AtomicBool>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            started: Mutex::new(None),
            waiting: Mutex::new(None),
            os_active: Arc::new(AtomicBool::new(false)),
        }
    }
}

static SESSION: OnceLock<Session> = OnceLock::new();

/// Activity 再生成時（`android_main` の再実行）に、保留中の要求を破棄する。
/// 画面状態は `default()`（世代 0）に戻るため、保留が残ると次の押下が発送されず飲み込まれる。
/// 古いワーカーの結果は照合で捨てられる。走っている OS 呼び出しは止めず走り切らせる。
#[cfg(target_os = "android")]
pub fn reset_pending_tracking() {
    if let Some(s) = SESSION.get() {
        s.slot.reset();
        *s.started.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *s.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    // Android の橋渡し側の slot も破棄する（保留中の要求を待つワーカーを失敗で戻す）。
    crate::android::reset_pending_tracking();
}

/// 契約上の失敗を画面の失敗種別へ写す（機器名・OS の文言は元々持たない）。
fn to_core_failure(f: ScannerImportFailure) -> CoreFailure {
    match f {
        ScannerImportFailure::Unsupported => CoreFailure::Unsupported,
        ScannerImportFailure::NoDevice => CoreFailure::NoDevice,
        ScannerImportFailure::NoImage => CoreFailure::NoImage,
        ScannerImportFailure::DeviceLocked => CoreFailure::DeviceLocked,
        ScannerImportFailure::PermissionDenied => CoreFailure::PermissionDenied,
        ScannerImportFailure::TimedOut => CoreFailure::TimedOut,
        ScannerImportFailure::Interrupted => CoreFailure::Interrupted,
        ScannerImportFailure::TooLarge => CoreFailure::TooLarge,
        ScannerImportFailure::Undecodable => CoreFailure::Undecodable,
        ScannerImportFailure::Failed => CoreFailure::Failed,
    }
}

/// ワーカーの本体: 取り込み → 期限の照合 → 復号（2 段目の検査）。
fn run_worker(imp: &dyn ScannerDevices, started: Instant, deadline: Duration) -> Outcome {
    let result = imp.import_one(deadline.saturating_sub(started.elapsed()));
    // 期限後に届いた結果は成功にしない。
    if started.elapsed() > deadline {
        return Err(ScannerImportFailure::TimedOut);
    }
    let image = result?;
    let decoded =
        decode_for_display(image.photo()).map_err(|_| ScannerImportFailure::Undecodable)?;
    // 復号に時間を取られて期限を過ぎた成功も成功にしない（成功を返す直前の再確認）。
    if started.elapsed() > deadline {
        return Err(ScannerImportFailure::TimedOut);
    }
    Ok(Imported {
        decoded: Arc::new(decoded),
        kind: image.kind(),
        found: image.found(),
    })
}

/// 完了を画面状態へ反映する。画素は世代が合うときだけ置く（古い画像が新しい画面に出ない）。
fn apply_outcome(s: &mut ScannerDevicesImportState, generation: u64, outcome: Outcome) {
    let result = match outcome {
        Ok(img) => {
            if s.phase != Phase::Running || s.generation != generation {
                return;
            }
            let d = &img.decoded;
            // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
            let source = format!("scanner:{generation}");
            image_store::put(source.clone(), d.image.clone());
            Ok(ShownImage {
                source,
                kind: match img.kind {
                    DeviceKind::Camera => CoreKind::Camera,
                    DeviceKind::Scanner => CoreKind::Scanner,
                },
                found: img.found,
                width: d.width,
                height: d.height,
            })
        }
        Err(f) => Err(to_core_failure(f)),
    };
    finish_import(s, generation, result);
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &ScannerDevicesImportState,
    sender: &CompletionSender<ScannerDevicesImportState>,
    imp: &Arc<dyn ScannerDevices>,
    session: &Session,
    deadline: Duration,
) {
    let slot = session.slot;
    if let Some(generation) = take_request(state) {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if slot.snapshot().is_some() {
            return;
        }
        // 期限の起点は要求を観測した時刻。前回ワーカーの終了待ちの間も同じ起点で測る。
        let started = {
            let mut w = session.waiting.lock().unwrap_or_else(|e| e.into_inner());
            match *w {
                Some((g, t)) if g == generation => t,
                _ => {
                    let t = Instant::now();
                    *w = Some((generation, t));
                    t
                }
            }
        };
        // 待機中に期限が来たら、OS を触らず TimedOut で完了させる。
        if started.elapsed() >= deadline {
            *session.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = sender.send(Box::new(move |s: &mut ScannerDevicesImportState| {
                mark_running(s);
                finish_import(s, generation, Err(CoreFailure::TimedOut));
            }));
            return;
        }
        // 前回のワーカーが機器を解放し終えるまで発送しない（同じ機器を二重に開かない）。
        if session.os_active.load(Ordering::Acquire) {
            sender.request_repaint_after(POLL_INTERVAL);
            return;
        }
        *session.waiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // 前回の画素を先に解放する（失敗の横に前の画像を残さない）。
        image_store::clear();
        let request_id = slot.begin();
        *session.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(started);
        let _ = sender.send(Box::new(mark_running));
        sender.request_repaint_after(POLL_INTERVAL);

        let worker_imp = imp.clone();
        session.os_active.store(true, Ordering::Release);
        let active = session.os_active.clone();
        let spawned = std::thread::Builder::new()
            .name("scanner_devices".into())
            .spawn(move || {
                let result = run_worker(&*worker_imp, started, deadline);
                active.store(false, Ordering::Release);
                slot.deliver(request_id, result);
            });
        if spawned.is_err() {
            session.os_active.store(false, Ordering::Release);
            slot.deliver(request_id, Err(ScannerImportFailure::Failed));
        }
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    // 期限判定: 単調時計。確定後に届く結果は照合で捨てられる。
    let started = *session.started.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(started), Some((request_id, None))) = (started, slot.snapshot()) {
        // 判定と除去を原子的に行う。false は deliver が先に着地済み（成功を消さず後続に任せる）。
        if started.elapsed() >= deadline && slot.clear_if_pending(request_id) {
            let _ = sender.send(Box::new(move |s: &mut ScannerDevicesImportState| {
                finish_import(s, generation, Err(CoreFailure::TimedOut));
            }));
            return;
        }
    }
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut ScannerDevicesImportState, outcome: Outcome| {
            apply_outcome(s, generation, outcome);
            true
        },
    );
}

/// `registry.rs` の `effect` 区画から `scanner_devices_import` 画面用に配線される effect ドライバ。
pub fn scanner_devices_import_effect(
    state: &ScannerDevicesImportState,
    sender: &CompletionSender<ScannerDevicesImportState>,
) {
    static IMPL: OnceLock<Arc<dyn ScannerDevices>> = OnceLock::new();
    let imp = IMPL.get_or_init(platform_scanner_devices);
    drive(
        state,
        sender,
        imp,
        SESSION.get_or_init(Session::default),
        IMPORT_DEADLINE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ImportedImage, UnsupportedScannerDevices};
    use fandhe_core::screens::scanner_devices_import::{
        ScannerDevicesImportState as State, image_source, request_import,
    };
    use std::sync::mpsc;

    /// 起床を捨てるだけの waker（テストは受信側で完了を直接確かめる）。
    struct NoopWaker;
    impl fandhe_runtime::RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    type Rx = mpsc::Receiver<fandhe_runtime::Completion<State>>;
    type Result_ = Result<ImportedImage, ScannerImportFailure>;

    fn channel() -> (CompletionSender<State>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("PNG を書ける");
        out
    }

    fn good() -> Result_ {
        ImportedImage::from_os(png(), DeviceKind::Camera, 2)
    }

    /// 指定の結果を返し、`sleep` だけ待つフェイク。
    struct Fake {
        result: fn() -> Result_,
        sleep: Duration,
    }
    impl ScannerDevices for Fake {
        fn import_one(&self, _: Duration) -> Result_ {
            std::thread::sleep(self.sleep);
            (self.result)()
        }
    }

    fn fake_with(result: fn() -> Result_, sleep: Duration) -> Arc<dyn ScannerDevices> {
        Arc::new(Fake { result, sleep })
    }

    fn fake(result: fn() -> Result_) -> Arc<dyn ScannerDevices> {
        fake_with(result, Duration::ZERO)
    }

    /// フレームを回して届いた完了を適用する。`Running` を抜けるか 5 秒で打ち切る。
    fn pump(
        state: &mut State,
        sender: &CompletionSender<State>,
        rx: &Rx,
        imp: &Arc<dyn ScannerDevices>,
        session: &Session,
        deadline: Duration,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, session, deadline);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if !matches!(state.phase, Phase::Requested | Phase::Running) || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn run(imp: Arc<dyn ScannerDevices>) -> State {
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = State::default();
        request_import(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_secs(30),
        );
        state
    }

    fn failure(s: &State) -> CoreFailure {
        match &s.result {
            Some(Err(f)) => *f,
            r => panic!("失敗のはず: {r:?}"),
        }
    }

    #[test]
    fn no_request_does_nothing() {
        let (sender, rx) = channel();
        drive(
            &State::default(),
            &sender,
            &fake(good),
            &Session::default(),
            Duration::from_secs(1),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn success_shows_image() {
        let s = run(fake(good));
        let Some(Ok(img)) = &s.result else {
            panic!("成功のはず: {:?}", s.result)
        };
        assert_eq!(
            (img.kind, img.found, img.width, img.height),
            (CoreKind::Camera, 2, 8, 4)
        );
        assert_eq!(image_source(&s), img.source);
        image_store::clear();
    }

    #[test]
    fn failures_map_one_to_one() {
        use ScannerImportFailure as F;
        let cases: [(fn() -> Result_, CoreFailure); 8] = [
            (|| Err(F::Unsupported), CoreFailure::Unsupported),
            (|| Err(F::NoDevice), CoreFailure::NoDevice),
            (|| Err(F::NoImage), CoreFailure::NoImage),
            (|| Err(F::DeviceLocked), CoreFailure::DeviceLocked),
            (|| Err(F::PermissionDenied), CoreFailure::PermissionDenied),
            (|| Err(F::Interrupted), CoreFailure::Interrupted),
            (|| Err(F::TooLarge), CoreFailure::TooLarge),
            (|| Err(F::Failed), CoreFailure::Failed),
        ];
        for (f, expect) in cases {
            assert_eq!(failure(&run(fake(f))), expect);
        }
    }

    #[test]
    fn zero_devices_from_os_is_not_success() {
        // `from_os` が 0 台を `NoDevice` にするため、実装が 0 台の画像を返しても成功にならない。
        let s = run(fake(|| {
            ImportedImage::from_os(png(), DeviceKind::Camera, 0)
        }));
        assert_eq!(failure(&s), CoreFailure::NoDevice);
    }

    #[test]
    fn undecodable_image_is_not_success() {
        // 先頭だけ PNG で中身が壊れた画像は 1 段目を通り、2 段目の復号で落ちる。
        let s = run(fake(|| {
            let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
            b.extend_from_slice(&[0xFF; 32]);
            ImportedImage::from_os(b, DeviceKind::Camera, 1)
        }));
        assert_eq!(failure(&s), CoreFailure::Undecodable);
        assert_eq!(image_source(&s), "");
    }

    #[test]
    fn worker_past_deadline_is_timeout_even_if_success() {
        let imp = fake_with(good, Duration::from_millis(60));
        let (sender, rx) = channel();
        let mut state = State::default();
        request_import(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &Session::default(),
            Duration::from_millis(20),
        );
        assert_eq!(failure(&state), CoreFailure::TimedOut);
    }

    #[test]
    fn frame_deadline_times_out_and_late_worker_result_is_ignored() {
        let imp = fake_with(good, Duration::from_millis(300));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = State::default();
        request_import(&mut state);
        let deadline = Duration::from_millis(30);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        // 遅れて届いたワーカーの結果は無視される（スロットは空・追加の完了なし）
        std::thread::sleep(Duration::from_millis(350));
        drive(&state, &sender, &imp, &session, deadline);
        assert!(rx.try_recv().is_err());
        assert_eq!(failure(&state), CoreFailure::TimedOut);
    }

    #[test]
    fn new_request_waits_until_previous_worker_finishes() {
        let imp = fake_with(good, Duration::from_millis(200));
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = State::default();
        request_import(&mut state);
        pump(
            &mut state,
            &sender,
            &rx,
            &imp,
            &session,
            Duration::from_millis(20),
        );
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        // ワーカーはまだ走っている: 再押下しても発送されない。
        request_import(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err());
        std::thread::sleep(Duration::from_millis(250));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn request_waiting_for_previous_worker_times_out_without_overlapping() {
        use std::sync::atomic::AtomicUsize;
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        struct Hang;
        impl ScannerDevices for Hang {
            fn import_one(&self, _: Duration) -> Result_ {
                CALLS.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(500));
                good()
            }
        }
        let imp: Arc<dyn ScannerDevices> = Arc::new(Hang);
        let (sender, rx) = channel();
        let session = Session::default();
        let mut state = State::default();
        let deadline = Duration::from_millis(30);
        request_import(&mut state);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        // ワーカーは戻っていない。再押下も期限で完了し、OS 呼び出しは重ならない。
        request_import(&mut state);
        pump(&mut state, &sender, &rx, &imp, &session, deadline);
        assert_eq!(failure(&state), CoreFailure::TimedOut);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_generation_completion_is_dropped_without_image() {
        let mut state = State::default();
        request_import(&mut state);
        mark_running(&mut state);
        let img = Imported {
            decoded: Arc::new(decode_for_display(good().unwrap().photo()).unwrap()),
            kind: DeviceKind::Camera,
            found: 1,
        };
        apply_outcome(&mut state, 99, Ok(img));
        assert_eq!(state.phase, Phase::Running);
        assert!(state.result.is_none());
    }

    #[test]
    fn double_drive_before_mark_applied_runs_once() {
        let (sender, rx) = channel();
        let imp = fake(good);
        let session = Session::default();
        let mut state = State::default();
        request_import(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        // mark_running 1 件のみ（2 回目は発送されない）
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1);
    }

    #[test]
    fn reset_lets_a_fresh_screen_dispatch_again() {
        let (sender, rx) = channel();
        let imp = fake(good);
        let session = Session::default();
        session.slot.begin();
        let mut state = State::default();
        request_import(&mut state);
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.try_recv().is_err(), "記録が残ると飲み込まれる");
        session.slot.reset();
        drive(&state, &sender, &imp, &session, Duration::from_secs(30));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn unsupported_impl_is_unsupported() {
        let s = run(Arc::new(UnsupportedScannerDevices));
        assert_eq!(failure(&s), CoreFailure::Unsupported);
    }

    #[test]
    fn fallback_platform_is_unsupported_on_linux() {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "windows"
        )))]
        assert_eq!(
            platform_scanner_devices().import_one(Duration::ZERO),
            Err(ScannerImportFailure::Unsupported)
        );
    }
}
