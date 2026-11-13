//! Phase 11 型サンプル「書類スキャン」（親 #145、Issue #747）の effect ドライバ（機能 crate
//! `fandhe-cap-document-scan` の中核、TASK-21.3 / Issue #1171）。
//! `fandhe_core::screens::document_scan_show`（端末依存ゼロ）の要求を観測し、
//! [`DocumentScanner`] に撮影 UI の表示を依頼して、OS 層から届く結果を
//! `fandhe_runtime::CompletionSender` 経由で画面へ返す（`fandhe-gallery` の `screens/registry.rs` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`fandhe_runtime::EffectFn` 参照）。
//! 契約（結果の三値・期限・守る範囲）の SSOT は crate ルート（`lib.rs`）。
//! 復号確認済みの画素は `fandhe_runtime::image_store` へ置き、レンダラが識別子で引いて描く。
//!
//! # OS 別の分担（dispatch・bridge・本ファイルは #747・#1124 で最終形。後続の子はここを触らない）
//!
//! - iOS: `ios`（#748。`VNDocumentCameraViewController`）
//! - Android: `android`（#1125。ML Kit Document Scanner）
//! - macOS・Windows・Linux: 対応表で `no` のため `UnsupportedDocumentScanner` 固定
//!   （ダミーの成功実装へは倒さない）。Web は対象外（gallery が wasm32 では本 crate を依存に入れない）
//!
//! # OS 層が呼ぶ受け口
//!
//! - [`try_begin_present`]: 撮影 UI を出す直前に呼ぶ。`false` なら出さない（期限切れ・古い要求・
//!   中断済み）。`true` を返した要求だけが成功値（スキャンした・キャンセル）を返せる
//! - [`complete_from_os`]: 結果を 1 回だけ送る唯一の受け口。古い要求 id・重複は捨てられる。
//!   画像は受け口の中でロックの外で復号し、復号できなければ `Undecodable` にする（2 段目の検査）
//!
//! # 期限（契約表 3）
//!
//! 撮影 UI が出るまでの [`PRESENT_DEADLINE`] だけを持つ。起点は要求を観測した時の `Instant`
//! （単調時計）。期限切れの確定（`expire_if_due` と `try_begin_present` 内）は表示開始の記録と
//! 同じロックで行うため、期限切れを確定した後に OS が撮影 UI を出すことはない。
//!
//! # スレッド
//!
//! drive は UI スレッド（毎フレーム）、`try_begin_present` と `complete_from_os` は OS 層の
//! スレッドから呼ばれる。判定・`RequestSlot::deliver`・`active` の更新は同じロックの内側で行う。
//!
//! # 秘密値（契約表 8）
//!
//! 画像の中身と OS のエラー文はログに出さない。失敗は理由の種別だけを持つ。

use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fandhe_core::RgbaPixels;
use fandhe_core::screens::document_scan_show::DocumentScanShowState;
use fandhe_core::screens::document_scan_show::{
    CoreFailure, CoreOutcome, Phase, ShownPage, finish_scan, mark_running, take_request,
};
use fandhe_runtime::request_slot::{POLL_INTERVAL, RequestSlot, poll_and_finish};
use fandhe_runtime::{CompletionSender, image_store};

use crate::{
    DocumentScanFailure, DocumentScanOutcome, DocumentScanner, PRESENT_DEADLINE, PageFormat,
    ScannedPage,
};

/// 復号を許す辺の最大（画素）。
const MAX_SIDE: u32 = 16384;
/// 復号中の確保上限（バイト）。
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// 表示用の複製の辺の上限（画素）。`RgbaPixels` の上限（2048）に収まる。
// ponytail: 写真選択（egui-proxy の photo_pick）と同じ値・同じ縮小処理を複製している。共有の置き場は #1183 で決める。
const DISPLAY_SIDE: u32 = 1024;

/// 復号確認を通った 1 ページ目。
struct Decoded {
    image: RgbaPixels,
    format: &'static str,
    size: usize,
    width: u32,
    height: u32,
}

/// 2 段目の検査: 実際に復号できるかを確かめ、表示用の画素を作る。失敗は `None`
/// （画像の中身・エラー文は持ち出さない）。
fn decode_for_display(page: &ScannedPage) -> Option<Decoded> {
    let (format, name) = match page.format() {
        PageFormat::Png => (image::ImageFormat::Png, "PNG"),
        PageFormat::Jpeg => (image::ImageFormat::Jpeg, "JPEG"),
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(page.bytes()), format);
    // `Limits` は non_exhaustive のため、既定値から書き換える。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    // thumbnail は小さい画像を拡大しうるため、大きいときだけ縮める。
    let shown = if width > DISPLAY_SIDE || height > DISPLAY_SIDE {
        decoded.thumbnail(DISPLAY_SIDE, DISPLAY_SIDE)
    } else {
        decoded
    };
    let rgba = shown.to_rgba8();
    let image = RgbaPixels::new(rgba.width(), rgba.height(), rgba.into_raw())?;
    Some(Decoded {
        image,
        format: name,
        size: page.bytes().len(),
        width,
        height,
    })
}

/// 復号済みの 1 ページ目とページ数。`RequestSlot` が `Clone` を要るため画素は `Arc` で持つ。
#[derive(Clone)]
struct ScanDecoded {
    decoded: Arc<Decoded>,
    page_count: u32,
}

/// bridge が画面側へ渡す成功側の結果。
#[derive(Clone)]
enum CoreScan {
    Scanned(ScanDecoded),
    Cancelled,
}

/// OS 層から届いて検査を終えた結果。
type Outcome = Result<CoreScan, DocumentScanFailure>;

/// 現在のプラットフォームに応じた `DocumentScanner` 実装を返す（dispatch の最終形）。
/// 状態型に依存しない入口（consumer-app-structure.md §8.2）。フレームワークの外のアプリも、自分の状態型の effect から使える。
/// 結果は trait の契約どおり bridge を経由して [`document_scan_show_effect`] へ届き、bridge を介さずフレームワークの外で受け取る口はない。
pub fn platform_document_scan() -> Arc<dyn DocumentScanner> {
    #[cfg(target_os = "ios")]
    {
        crate::ios::implementation()
    }
    #[cfg(target_os = "android")]
    {
        crate::android::implementation()
    }
    // macOS・Windows・Linux は対応表で no。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        Arc::new(crate::UnsupportedDocumentScanner)
    }
}

/// 進行中の要求。
struct Active {
    request_id: u64,
    /// 要求を観測した時刻（単調時計）。期限の起点。
    started: Instant,
    /// OS が撮影 UI を出し始めたことを `try_begin_present` で記録済みか。
    presenting: bool,
}

/// 要求の追跡と期限・表示開始の排他。テストが並列で干渉しないよう static から切り出して
/// 引数で渡す（`poll_and_finish` が `'static` を要るためスロットは leak）。
struct Bridge {
    slot: &'static RequestSlot<Outcome>,
    active: Mutex<Option<Active>>,
    deadline: Duration,
}

impl Bridge {
    fn new(deadline: Duration) -> Self {
        Self {
            slot: Box::leak(Box::new(RequestSlot::new())),
            active: Mutex::new(None),
            deadline,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 発送済み（OS 呼び出し中、または結果が画面へ未反映）の要求があるか。
    fn has_pending(&self) -> bool {
        self.lock().is_some() || self.slot.snapshot().is_some()
    }

    /// 新しい要求を登録して id を返す。OS へ渡す前に呼び、即時の完了も照合できるようにする。
    fn begin(&self) -> u64 {
        let mut active = self.lock();
        let request_id = self.slot.begin();
        *active = Some(Active {
            request_id,
            started: Instant::now(),
            presenting: false,
        });
        request_id
    }

    /// 表示開始の記録。現在の要求で、未確定で、期限前のときだけ `true`。
    /// 期限を過ぎていれば `TimedOut` を確定して `false`。
    #[allow(dead_code)] // iOS 以外のビルドでは単体テストだけが呼ぶ。
    fn try_begin_present(&self, request_id: u64) -> bool {
        let mut active = self.lock();
        let Some(a) = active.as_mut().filter(|a| a.request_id == request_id) else {
            return false;
        };
        if a.presenting {
            return false;
        }
        if a.started.elapsed() >= self.deadline {
            self.slot
                .deliver(request_id, Err(DocumentScanFailure::TimedOut));
            *active = None;
            return false;
        }
        a.presenting = true;
        true
    }

    /// OS 層からの唯一の結果の受け口。
    ///
    /// 先に id を照合して古い・確定済みの完了を重い復号の前に捨てる。復号はロックの外で行い、
    /// 届ける前にもう一度照合する（復号中に期限切れ・差し替えが起きうるため）。
    fn complete_from_os(
        &self,
        request_id: u64,
        result: Result<DocumentScanOutcome, DocumentScanFailure>,
    ) {
        if !self.is_current(request_id) {
            return;
        }
        let result: Outcome = match result {
            Ok(DocumentScanOutcome::Cancelled) => Ok(CoreScan::Cancelled),
            Ok(DocumentScanOutcome::Scanned(page)) => {
                // 2 段目の検査。復号できない画像は成功にしない。
                match decode_for_display(&page) {
                    Some(d) => Ok(CoreScan::Scanned(ScanDecoded {
                        decoded: Arc::new(d),
                        page_count: page.page_count(),
                    })),
                    None => Err(DocumentScanFailure::Undecodable),
                }
            }
            Err(f) => Err(f),
        };
        let mut active = self.lock();
        let Some(a) = active.as_ref().filter(|a| a.request_id == request_id) else {
            return;
        };
        // 表示開始を記録していない要求の成功値は嘘の成功になるため成功にしない。
        let result = match result {
            Ok(_) if !a.presenting => Err(DocumentScanFailure::Internal),
            r => r,
        };
        self.slot.deliver(request_id, result);
        *active = None;
    }

    /// `request_id` が現在の未確定の要求か。
    fn is_current(&self, request_id: u64) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|a| a.request_id == request_id)
    }

    /// 表示開始前に期限を過ぎていれば `TimedOut` を確定する。
    fn expire_if_due(&self) {
        let mut active = self.lock();
        let due = active
            .as_ref()
            .is_some_and(|a| !a.presenting && a.started.elapsed() >= self.deadline);
        if due && let Some(a) = active.take() {
            self.slot
                .deliver(a.request_id, Err(DocumentScanFailure::TimedOut));
        }
    }

    fn reset(&self) {
        let mut active = self.lock();
        *active = None;
        self.slot.reset();
    }
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| Bridge::new(PRESENT_DEADLINE))
}

/// OS 層が撮影 UI を出す直前に呼ぶ。`false` なら出さない（モジュール doc 参照）。
// iOS の受け口、Android は #1125 の JNI 受け口が呼ぶ。デスクトップとスタブのうちは未使用。
#[allow(dead_code)]
pub(crate) fn try_begin_present(request_id: u64) -> bool {
    bridge().try_begin_present(request_id)
}

/// OS 層が結果を 1 回だけ送る受け口（モジュール doc 参照）。
// iOS の受け口、Android は #1125 の JNI 受け口が呼ぶ。デスクトップとスタブのうちは未使用。
#[allow(dead_code)]
pub(crate) fn complete_from_os(
    request_id: u64,
    result: Result<DocumentScanOutcome, DocumentScanFailure>,
) {
    bridge().complete_from_os(request_id, result);
}

/// 画面の作り直し時に保留中の要求を破棄する。画面状態は `default()`（世代 0）に戻るため、
/// 保留が残ると次の要求が発送されず飲み込まれる。旧要求の遅延完了は id 不一致で捨てられる。
/// Android の再生成経路（アプリの `android_main`）が、新しい画面状態を作る前に呼ぶ。
/// iOS・デスクトップには再生成経路がなく呼ばなくてよい。
pub fn reset_pending_tracking() {
    if let Some(b) = BRIDGE.get() {
        b.reset();
    }
}

/// OS 層の失敗を画面の失敗種別へ写す。画像・OS の文言は渡さない。
fn to_core_failure(f: DocumentScanFailure) -> CoreFailure {
    match f {
        DocumentScanFailure::Unsupported => CoreFailure::Unsupported,
        DocumentScanFailure::PermissionDenied => CoreFailure::PermissionDenied,
        DocumentScanFailure::Empty => CoreFailure::Empty,
        DocumentScanFailure::TooLarge => CoreFailure::TooLarge,
        DocumentScanFailure::Undecodable => CoreFailure::Undecodable,
        DocumentScanFailure::NotForeground => CoreFailure::NotForeground,
        DocumentScanFailure::TimedOut => CoreFailure::TimedOut,
        DocumentScanFailure::Interrupted => CoreFailure::Interrupted,
        DocumentScanFailure::Internal => CoreFailure::Internal,
    }
}

/// 要求を登録して OS へ渡す。同期失敗はその場で確定する。
fn start_request(bridge: &Bridge, imp: &dyn DocumentScanner) {
    let request_id = bridge.begin();
    if let Err(f) = imp.scan(request_id) {
        bridge.complete_from_os(request_id, Err(f));
    }
}

/// 1 フレームぶんの処理。要求の発送・期限判定・完了の画面反映を行う。
fn drive(
    state: &DocumentScanShowState,
    sender: &CompletionSender<DocumentScanShowState>,
    imp: &Arc<dyn DocumentScanner>,
    bridge: &Bridge,
) {
    if take_request(state).is_some() {
        // 保留ありは発送済み（`mark_running` の適用待ち）。二重に発送しない。
        if bridge.has_pending() {
            return;
        }
        // 前回の画素を先に解放する（失敗・キャンセルの横に前の画像を残さない）。
        image_store::clear();
        let _ = sender.send(Box::new(mark_running));
        start_request(bridge, &**imp);
        sender.request_repaint_after(POLL_INTERVAL);
        return;
    }
    if state.phase != Phase::Running {
        return;
    }
    let generation = state.generation;
    bridge.expire_if_due();
    let slot = bridge.slot;
    let Some((pending_id, _)) = slot.snapshot() else {
        return;
    };
    poll_and_finish(
        slot,
        sender,
        |_| {},
        move |s: &mut DocumentScanShowState, outcome: Outcome| {
            let applied = apply_outcome(s, generation, outcome);
            // 反映できない完了（世代違い）は捨てる。残すとスロットが埋まったままになり、
            // `has_pending` が真のまま以降の要求が発送されない。
            if !applied {
                slot.clear(pending_id);
            }
            applied
        },
    );
}

/// 完了を画面状態へ反映する。反映できなければ `false`（呼び出し側がスロットを片付ける）。
fn apply_outcome(s: &mut DocumentScanShowState, generation: u64, outcome: Outcome) -> bool {
    let result = match outcome {
        Ok(CoreScan::Cancelled) => Ok(CoreOutcome::Cancelled),
        Ok(CoreScan::Scanned(scan)) => {
            let d = &scan.decoded;
            // 世代が違う完了は画素を置かない（古い画像が新しい画面に出ない）。
            if s.phase != Phase::Running || s.generation != generation {
                return false;
            }
            // 画素を先に置いてから成功を届ける（届いた時点で描ける）。
            let source = format!("scan:{generation}");
            image_store::put(source.clone(), d.image.clone());
            Ok(CoreOutcome::Scanned(ShownPage {
                source,
                format: d.format.to_string(),
                size: d.size,
                width: d.width,
                height: d.height,
                page_count: scan.page_count,
            }))
        }
        Err(f) => Err(to_core_failure(f)),
    };
    finish_scan(s, generation, result)
}

/// `registry.rs` の `effect` 区画から `document_scan_show` 画面用に配線される effect ドライバ
/// （`fandhe_runtime::EffectFn` の形）。
pub fn document_scan_show_effect(
    state: &DocumentScanShowState,
    sender: &CompletionSender<DocumentScanShowState>,
) {
    static IMPL: OnceLock<Arc<dyn DocumentScanner>> = OnceLock::new();
    drive(
        state,
        sender,
        IMPL.get_or_init(platform_document_scan),
        bridge(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnsupportedDocumentScanner;
    use fandhe_core::screens::document_scan_show::{
        error_text, image_source, request_scan, status_text,
    };
    use fandhe_runtime::{Completion, RepaintWaker};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    type Rx = mpsc::Receiver<Completion<DocumentScanShowState>>;

    /// 起床を何もしない（テストは `rx` を直接読む）。
    struct NoopWaker;
    impl RepaintWaker for NoopWaker {
        fn wake(&self) {}
        fn wake_after(&self, _delay: Duration) {}
    }

    fn channel() -> (CompletionSender<DocumentScanShowState>, Rx) {
        let (tx, rx) = mpsc::channel();
        (CompletionSender::new(tx, Arc::new(NoopWaker)), rx)
    }

    fn new_bridge(deadline: Duration) -> &'static Bridge {
        Box::leak(Box::new(Bridge::new(deadline)))
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn page(bytes: Vec<u8>, pages: u32) -> ScannedPage {
        ScannedPage::from_os(bytes, pages).unwrap()
    }

    #[derive(Clone)]
    enum Mode {
        /// 同期で失敗を返す。
        Fail(DocumentScanFailure),
        /// 何も起きない（OS が応答しない）。
        Silent,
        /// 表示を始め、指定の画像・ページ数でスキャンを送る。
        PresentThenScan(Vec<u8>, u32),
        /// 表示を始め、`ms` 待ってからキャンセルを送る。
        PresentThenCancel(u64),
        /// 表示を始めずにスキャンを送る（契約違反の OS）。
        ScanWithoutPresent,
        /// 表示を始めずにキャンセルを送る（契約違反の OS）。
        CancelWithoutPresent,
        /// 表示を始めて権限拒否を送る。
        PermissionDenied,
        /// 表示を始めずに中断を送る。
        Interrupt,
        /// 表示を始めず、非同期に失敗を送る（Android の GMS 判定・前面判定の失敗）。
        AsyncFail(DocumentScanFailure),
        /// 表示を始めた後に中断を送る（Android の `onDestroy` で保留中）。
        PresentThenInterrupt,
    }

    struct Fake {
        mode: Mode,
        calls: AtomicUsize,
        bridge: &'static Bridge,
    }

    impl DocumentScanner for Fake {
        fn scan(&self, request_id: u64) -> Result<(), DocumentScanFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let b = self.bridge;
            match self.mode.clone() {
                Mode::Fail(f) => return Err(f),
                Mode::Silent => {}
                Mode::PresentThenScan(bytes, pages) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(
                                request_id,
                                Ok(DocumentScanOutcome::Scanned(page(bytes, pages))),
                            );
                        }
                    });
                }
                Mode::PresentThenCancel(ms) => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            std::thread::sleep(Duration::from_millis(ms));
                            b.complete_from_os(request_id, Ok(DocumentScanOutcome::Cancelled));
                        }
                    });
                }
                Mode::ScanWithoutPresent => {
                    b.complete_from_os(
                        request_id,
                        Ok(DocumentScanOutcome::Scanned(page(png(2, 2), 1))),
                    );
                }
                Mode::CancelWithoutPresent => {
                    b.complete_from_os(request_id, Ok(DocumentScanOutcome::Cancelled));
                }
                Mode::PermissionDenied => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(
                                request_id,
                                Err(DocumentScanFailure::PermissionDenied),
                            );
                        }
                    });
                }
                Mode::Interrupt => {
                    b.complete_from_os(request_id, Err(DocumentScanFailure::Interrupted));
                }
                Mode::AsyncFail(f) => {
                    std::thread::spawn(move || b.complete_from_os(request_id, Err(f)));
                }
                Mode::PresentThenInterrupt => {
                    std::thread::spawn(move || {
                        if b.try_begin_present(request_id) {
                            b.complete_from_os(request_id, Err(DocumentScanFailure::Interrupted));
                        }
                    });
                }
            }
            Ok(())
        }
    }

    fn fake(mode: Mode, bridge: &'static Bridge) -> Arc<Fake> {
        Arc::new(Fake {
            mode,
            calls: AtomicUsize::new(0),
            bridge,
        })
    }

    /// フレームを回して届いた完了を適用する。`Idle` に戻るか 5 秒で打ち切る。
    fn pump(
        state: &mut DocumentScanShowState,
        sender: &CompletionSender<DocumentScanShowState>,
        rx: &Rx,
        imp: &Arc<dyn DocumentScanner>,
        bridge: &Bridge,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            drive(state, sender, imp, bridge);
            while let Ok(c) = rx.try_recv() {
                c(state);
            }
            if state.phase == Phase::Idle || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run(mode: Mode, deadline: Duration) -> (DocumentScanShowState, Arc<Fake>) {
        let bridge = new_bridge(deadline);
        let f = fake(mode, bridge);
        let imp: Arc<dyn DocumentScanner> = f.clone();
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        (state, f)
    }

    const LONG: Duration = Duration::from_secs(30);

    #[test]
    fn no_request_does_nothing() {
        let bridge = new_bridge(Duration::from_secs(1));
        let imp: Arc<dyn DocumentScanner> = fake(Mode::Silent, bridge);
        let (sender, rx) = channel();
        drive(&DocumentScanShowState::default(), &sender, &imp, bridge);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unsupported_impl_is_unsupported_not_success() {
        let bridge = new_bridge(LONG);
        let imp: Arc<dyn DocumentScanner> = Arc::new(UnsupportedDocumentScanner);
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::Unsupported)));
        assert_eq!(image_source(&state), "");
    }

    #[test]
    fn fallback_platform_is_unsupported_off_ios_and_android() {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        assert_eq!(
            platform_document_scan().scan(1),
            Err(DocumentScanFailure::Unsupported)
        );
    }

    #[test]
    fn permission_denied_is_a_failure() {
        let (s, _) = run(Mode::PermissionDenied, LONG);
        assert_eq!(s.result, Some(Err(CoreFailure::PermissionDenied)));
        assert_ne!(error_text(&s), "");
    }

    #[test]
    fn cancel_is_not_a_failure() {
        let (s, _) = run(Mode::PresentThenCancel(0), LONG);
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn sync_failures_are_reported() {
        for (f, expect) in [
            (
                DocumentScanFailure::NotForeground,
                CoreFailure::NotForeground,
            ),
            (DocumentScanFailure::Empty, CoreFailure::Empty),
            (DocumentScanFailure::Internal, CoreFailure::Internal),
        ] {
            let (s, _) = run(Mode::Fail(f), LONG);
            assert_eq!(s.result, Some(Err(expect)));
        }
    }

    #[test]
    fn interrupted_is_a_failure() {
        let (s, _) = run(Mode::Interrupt, LONG);
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
    }

    #[test]
    fn async_failures_without_present_are_failures_with_no_image() {
        for (f, expect) in [
            (DocumentScanFailure::Unsupported, CoreFailure::Unsupported),
            (
                DocumentScanFailure::NotForeground,
                CoreFailure::NotForeground,
            ),
        ] {
            let (s, _) = run(Mode::AsyncFail(f), LONG);
            assert_eq!(s.result, Some(Err(expect)));
            assert_eq!(image_source(&s), "");
        }
    }

    #[test]
    fn interrupt_after_present_is_a_failure() {
        let (s, _) = run(Mode::PresentThenInterrupt, LONG);
        assert_eq!(s.result, Some(Err(CoreFailure::Interrupted)));
        assert_eq!(image_source(&s), "");
    }

    #[test]
    fn success_values_without_present_are_internal() {
        for mode in [Mode::ScanWithoutPresent, Mode::CancelWithoutPresent] {
            let (s, _) = run(mode, LONG);
            assert_eq!(s.result, Some(Err(CoreFailure::Internal)));
            assert_eq!(image_source(&s), "");
        }
    }

    #[test]
    fn deadline_before_present_times_out_and_blocks_later_present() {
        let bridge = new_bridge(Duration::from_millis(20));
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn DocumentScanner> = f.clone();
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        pump(&mut state, &sender, &rx, &imp, bridge);
        assert_eq!(state.result, Some(Err(CoreFailure::TimedOut)));
        assert!(!bridge.try_begin_present(1));
    }

    #[test]
    fn present_attempt_after_deadline_times_out_atomically() {
        let bridge = new_bridge(Duration::from_millis(10));
        let id = bridge.begin();
        std::thread::sleep(Duration::from_millis(30));
        assert!(!bridge.try_begin_present(id));
        let timed_out = |b: &Bridge| {
            matches!(
                b.slot.snapshot(),
                Some((i, Some(Err(DocumentScanFailure::TimedOut)))) if i == id
            )
        };
        assert!(timed_out(bridge));
        // 期限後に届いた成功は捨てられる（active は除去済み）。
        bridge.complete_from_os(id, Ok(DocumentScanOutcome::Cancelled));
        assert!(timed_out(bridge));
    }

    #[test]
    fn after_present_deadline_no_longer_applies() {
        let (s, _) = run(Mode::PresentThenCancel(120), Duration::from_millis(30));
        assert_eq!(s.result, Some(Ok(CoreOutcome::Cancelled)));
    }

    #[test]
    fn stale_request_id_is_dropped_and_duplicates_first_wins() {
        let bridge = new_bridge(LONG);
        let id = bridge.begin();
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id + 1, Err(DocumentScanFailure::Empty));
        assert!(matches!(bridge.slot.snapshot(), Some((i, None)) if i == id));
        bridge.complete_from_os(id, Ok(DocumentScanOutcome::Cancelled));
        bridge.complete_from_os(id, Err(DocumentScanFailure::Internal));
        assert!(matches!(
            bridge.slot.snapshot(),
            Some((i, Some(Ok(CoreScan::Cancelled)))) if i == id
        ));
        bridge.reset();
        let id2 = bridge.begin();
        assert!(bridge.try_begin_present(id2));
        assert!(!bridge.try_begin_present(id2));
    }

    #[test]
    fn stale_generation_completion_is_ignored() {
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        mark_running(&mut state);
        assert!(!finish_scan(&mut state, 99, Ok(CoreOutcome::Cancelled)));
        assert_eq!(state.phase, Phase::Running);
    }

    #[test]
    fn broken_images_are_undecodable_not_success() {
        let truncated = {
            let mut b = png(8, 8);
            b.truncate(b.len() / 2);
            b
        };
        for bytes in [truncated, vec![0xFF, 0xD8, 0xFF]] {
            let (s, _) = run(Mode::PresentThenScan(bytes, 1), LONG);
            assert_eq!(s.result, Some(Err(CoreFailure::Undecodable)));
            assert_eq!(image_source(&s), "");
        }
    }

    #[test]
    fn valid_png_is_placed_and_reported() {
        let (s, _) = run(Mode::PresentThenScan(png(8, 4), 3), LONG);
        let Some(Ok(CoreOutcome::Scanned(p))) = &s.result else {
            panic!("{:?}", s.result);
        };
        assert_eq!((p.width, p.height, p.page_count), (8, 4, 3));
        assert_eq!(p.source, format!("scan:{}", s.generation));
        assert_eq!(image_source(&s), p.source);
        assert!(status_text(&s).contains("3 ページ中"));
    }

    #[test]
    fn double_drive_before_mark_applied_scans_once() {
        let bridge = new_bridge(LONG);
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn DocumentScanner> = f.clone();
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        drive(&state, &sender, &imp, bridge);
        drive(&state, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 1);
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 1, "mark_running は 1 件のみ");
    }

    #[test]
    fn mismatched_generation_completion_clears_slot_and_unblocks_dispatch() {
        let bridge = new_bridge(LONG);
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn DocumentScanner> = f.clone();
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        let id = bridge.slot.snapshot().unwrap().0;
        assert!(bridge.try_begin_present(id));
        bridge.complete_from_os(id, Ok(DocumentScanOutcome::Cancelled));
        // 世代がずれた画面（再生成など）へ完了が届く。
        state.generation += 1;
        drive(&state, &sender, &imp, bridge);
        while let Ok(c) = rx.try_recv() {
            c(&mut state);
        }
        assert!(
            bridge.slot.snapshot().is_none(),
            "不一致でもスロットを片付ける"
        );
        assert!(!bridge.has_pending());
    }

    #[test]
    fn reset_drops_old_completion_and_allows_next_dispatch() {
        let bridge = new_bridge(LONG);
        let f = fake(Mode::Silent, bridge);
        let imp: Arc<dyn DocumentScanner> = f.clone();
        let (sender, rx) = channel();
        let mut state = DocumentScanShowState::default();
        request_scan(&mut state);
        drive(&state, &sender, &imp, bridge);
        let old_id = bridge.slot.snapshot().unwrap().0;
        bridge.reset();
        bridge.complete_from_os(old_id, Ok(DocumentScanOutcome::Cancelled));
        assert!(bridge.slot.snapshot().is_none());
        let mut fresh = DocumentScanShowState::default();
        request_scan(&mut fresh);
        while rx.try_recv().is_ok() {}
        drive(&fresh, &sender, &imp, bridge);
        assert_eq!(f.calls.load(Ordering::SeqCst), 2);
    }
}
