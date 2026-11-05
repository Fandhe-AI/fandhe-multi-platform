//! egui 描画アプリの本体（TASK-1.2 / TASK-21.2 #1164）。`StateStore` を保持して毎フレーム
//! [`crate::render_screen`] へ委譲する [`ProxyApp`] と、起床口（[`EguiWaker`]）、
//! 端末機能ドライバ向けの egui 読み書き口（[`EguiSenderExt`]）を置く。
//!
//! 起動（eframe）は [`crate::launch`] が担い、本モジュールは eframe に依存しない
//! （[`EguiApp`] を実装するだけ）。`#[doc(hidden)]` の項目は旧構造（`fandhe-egui-proxy` の
//! 端末機能の配線）専用の暫定の口で、#1194 で撤去する。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fandhe_core::{sample_screen, SampleState, Screen, StateStore};

use crate::launch::EguiApp;
use crate::recyclers::VirtualListRecyclers;

/// `CentralPanel` の `inner_margin` へ加算する window insets 供給関数（Issue #579）。
///
/// `android.rs`（`cfg(target_os = "android")`）が `AndroidApp::content_rect()` を
/// 捕捉したクロージャを注入する契約で、非 Android では `None`（追加 margin なし・
/// 既存の描画結果を変えない）のまま。Android 専用の型（`android-activity` の
/// `Rect` 等）を本モジュールへ持ち込まないよう、契約は `&egui::Context -> egui::Margin`
/// にとどめている（cfg(target_os = "android") を android.rs 側に閉じ込める設計判断）。
///
/// 公開しているのは、レジストリ経由（`screens::build_app`）で型消去された
/// `Box<dyn ProxyAppDyn>` へも同じ供給関数を注入できるようにするため
/// （`ProxyAppDyn::set_inset_margin_supplier` / `android.rs` 参照、Issue #715）。
pub type InsetMarginSupplier = Box<dyn Fn(&egui::Context) -> egui::Margin>;

use fandhe_runtime::{drain_completions, Renderer, RepaintWaker};
/// 起床と完了通知の型は egui 非依存の `fandhe-runtime` が持つ（Issue #1162）。
/// `crate::app::CompletionSender` などの既存パスを保つための再公開。
pub use fandhe_runtime::{Completion, CompletionSender, EffectFn};

/// egui の描画ループを起こす [`RepaintWaker`] 実装。`ProxyApp` と iOS / Android の起床口
/// （`fandhe_runtime::set_repaint_waker`）が使う。
pub struct EguiWaker(egui::Context);

impl EguiWaker {
    /// `ctx` を起こす起床口を作る。旧構造（`fandhe-egui-proxy` の iOS / Android 入口）が
    /// `set_repaint_waker` へ渡すための口（#1194 で撤去）。
    #[doc(hidden)]
    pub fn new(ctx: egui::Context) -> Self {
        Self(ctx)
    }
}

impl RepaintWaker for EguiWaker {
    fn wake(&self) {
        self.0.request_repaint();
    }
    fn wake_after(&self, delay: std::time::Duration) {
        self.0.request_repaint_after(delay);
    }
}

/// テスト専用の直接構築（旧構造の `device/*` のテストが使う。#1194 で撤去）。`device/*` の effect ドライバ単体テストが、自前の
/// `mpsc::channel` と `egui::Context::default()` から [`CompletionSender`] を作るために使う。
#[doc(hidden)]
pub fn egui_sender<S>(
    tx: std::sync::mpsc::Sender<Completion<S>>,
    ctx: egui::Context,
) -> CompletionSender<S> {
    CompletionSender::new(tx, Arc::new(EguiWaker(ctx)))
}

/// [`CompletionSender`] が持つ egui 固有の読み書き口。起床以外の egui 依存が解消するまでの
/// 暫定の橋渡しで、機能 crate からは使えない（一覧は Issue #1162 のコメント参照）。
/// `sender.focused()` のような呼び出し側の書き方は従来のまま。
#[doc(hidden)]
pub trait EguiSenderExt {
    /// sender の起床口が持つ `egui::Context`。
    fn egui_ctx(&self) -> &egui::Context;

    /// `egui::Context` の複製を返す。全画面表示（`device/fullscreen_immersive.rs`）が
    /// `ViewportCommand::Fullscreen` の送信と `viewport().fullscreen` の読み戻しに使う専用の口で、
    /// セーフエリア（`device/safe_area_insets.rs`）が Android の px→dp 換算に使う。
    /// 他の機能での流用はしない（ctx を丸ごと渡す口になるため）。
    #[cfg(not(target_arch = "wasm32"))]
    fn egui_context(&self) -> egui::Context {
        self.egui_ctx().clone()
    }

    /// このフレームでウィンドウがフォーカスを持っているか（`egui::RawInput::focused`
    /// への薄い委譲）。Windows 共有シートのキャンセル検知専用
    /// （`device/share.rs::windows_cancel_check` が「共有 UI へフォーカスを奪われた
    /// → 復帰後も結果未着」を判定するために使う。`DataTransferManager` に
    /// 「共有 UI を閉じた」イベントが無いための代替、`device/windows_share.rs`
    /// モジュール doc「キャンセル検知の天井」参照）。
    ///
    /// 2 人目の利用者は `device/app_progress_attention.rs` の注意喚起の待機で、「フォーカスを
    /// 失った最初のフレーム」の判定に読む（タイマーやアプリ内の記憶を持たず OS の観測を正にする）。
    fn focused(&self) -> bool {
        self.egui_ctx().input(|i| i.raw.focused)
    }

    /// このフレームのビューポート（描画領域）の大きさ（`egui::InputState::viewport_rect` への
    /// 薄い委譲）。画面の向きの effect ドライバ（`device/screen_orientation.rs::drive`）専用で、
    /// 回転による大きさの変化を契機に向きを即時に読み直すために使う。
    fn viewport_size(&self) -> (f32, f32) {
        let size = self.egui_ctx().input(|i| i.viewport_rect().size());
        (size.x, size.y)
    }

    /// 次のフレームの描画を `tag` 付きで読み戻すよう依頼する（`ViewportCommand::Screenshot`、
    /// egui 0.33 は要求の次のフレームを撮る）。画面の印刷（`device/print.rs` の print-screen 経路）
    /// 専用で、タグは [`Self::tagged_screenshot`] で自分の読み戻しだけを選ぶために使う。
    /// `egui_context()` を流用しないのは、ctx を丸ごと渡す口を増やさないため。
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    fn request_screenshot(&self, tag: u64) {
        self.egui_ctx()
            .send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                ScreenshotTag(tag),
            )));
        self.egui_ctx().request_repaint();
    }

    /// このフレームに届いた、`tag` 付きの読み戻し（[`Self::request_screenshot`] の応答）を返す。
    /// 他のタグ・タグ無し（自動撮影）の読み戻しは返さない。
    fn tagged_screenshot(&self, tag: u64) -> Option<Arc<egui::ColorImage>> {
        self.egui_ctx()
            .input(|i| find_tagged_screenshot(&i.events, tag))
    }

    /// ポインタの主ボタン（左クリック/タップ）がこのフレームで押下中か
    /// （`egui::PointerState::primary_down` への薄い委譲）。Issue #912「文字サイズ追従」の
    /// effect ドライバ（`device/text_scale.rs::drive`）専用。スライダーのドラッグ中は
    /// `set_zoom_factor` を保留し離した後に適用するための判定に使う
    /// （Cursor Bugbot 指摘 "Slider drag fights live zoom"、PR #920 是正。ドラッグ中に
    /// `pixels_per_point` を変えると、スライダー自身が次パスで別の point 空間の矩形へ
    /// 再マップされ、指/カーソル追従を失って倍率が飛ぶため）。
    fn pointer_primary_down(&self) -> bool {
        self.egui_ctx().input(|i| i.pointer.primary_down())
    }

    /// キャンバス `id` 上で主ボタンを押下中のポインタ位置（キャンバス左上原点）。押していなければ
    /// `None`。`lib.rs` の `CanvasWidget` 分岐が毎フレーム書いた値を `ctx.data` から読むだけの
    /// 薄い委譲で、effect はフレーム末尾で呼ばれるため同じフレームの値が見える
    /// （Issue #355 `device/pen_stylus.rs` 専用）。
    fn canvas_pointer(&self, id: fandhe_core::WidgetId) -> Option<(f32, f32)> {
        self.egui_ctx()
            .data(|d| d.get_temp::<Option<(f32, f32)>>(canvas_pointer_id(id.raw())))
            .flatten()
    }

    /// キャンバス `id` 上で、このフレームに接触が終わった位置（キャンバス左上原点）。押下から解放までが
    /// 1 フレームに収まった短いタップも含む。終わっていなければ `None`（Issue #355）。
    fn canvas_released(&self, id: fandhe_core::WidgetId) -> Option<(f32, f32)> {
        self.egui_ctx()
            .data(|d| d.get_temp::<Option<(f32, f32)>>(canvas_released_id(id.raw())))
            .flatten()
    }

    /// キャンバス `id` 上で、このフレームに主ボタンが新しく押されたか（`canvas_pointer` と同じく
    /// `lib.rs` の `CanvasWidget` 分岐が毎フレーム書く）。1 フレーム内の「離す → 押す」を
    /// 新しい接触として扱うため（Issue #355）。
    fn canvas_pressed(&self, id: fandhe_core::WidgetId) -> bool {
        self.egui_ctx()
            .data(|d| d.get_temp::<bool>(canvas_pressed_id(id.raw())))
            .unwrap_or(false)
    }

    /// 実描画の拡大率を差し替える（`egui::Context::set_zoom_factor` への薄い委譲。
    /// Issue #912「文字サイズ追従」の effect ドライバ
    /// （`device/text_scale.rs::drive`）専用）。
    ///
    /// egui は次パス先頭で `pixels_per_point = zoom_factor × native_ppp` に反映する
    /// （egui 0.33.3 `Context::set_zoom_factor` doc）。値域 clamp・「変わったフレーム
    /// だけ適用するか」の判断は呼び出し側（`device/text_scale.rs`）が担い、本メソッドは
    /// 委譲のみに留める。
    fn set_zoom_factor(&self, zoom: f32) {
        self.egui_ctx().set_zoom_factor(zoom);
        // set_zoom_factor 自体は repaint を要求しない（egui::Context::set_zoom_factor の
        // 実装詳細）。effect はフレーム末尾で呼ばれるため（本モジュール doc）、ここで
        // request_repaint しないと「次パスで zoom を反映する」ための次フレームが
        // 入力待ちのアイドル状態のまま起こらず、スライダー/OS 値が変わっても描画に
        // 反映されないまま止まる（Cursor Bugbot 指摘、PR #920 是正。send が同じ理由で
        // request_repaint する設計と揃える）。
        self.egui_ctx().request_repaint();
    }

    /// device/text_scale.rs::drive が「直前に set_zoom_factor を適用した値」を
    /// egui::Context 単位（ctx.data の一時領域）で読み書きするための薄い委譲。
    ///
    /// プロセス全体で共有する static に持たせると、egui::Context を複数持つ場面
    /// （アプリ再構築・複数ウィンドウ等）で Context A に適用した倍率が Context B にも
    /// 適用済みと誤判定され、B が既定倍率のまま止まる（codex-review 指摘 P2、PR #920
    /// 是正）。ctx.data_mut の一時領域はこの ctx（＝1 つの実描画コンテキスト）に閉じる
    /// ため、Context 単位でキャッシュが分離される（lib.rs::dispatch_back_if_requested の
    /// text_input_focused_last_frame_id と同じ、ctx.data の一時領域を薄いキャッシュとして
    /// 使うパターン）。
    ///
    /// `get_temp`/`insert_temp` ではなく `get_persisted`/`insert_persisted` を使う
    /// （Cursor Bugbot 指摘、PR #920 是正）。`get_temp` はシリアライズ対象外の値として
    /// 扱われ、`persistence` feature 有効時に `egui::Memory` の保存・復元サイクルを
    /// 経由すると読み直せなくなる。`egui` は本クレートで `default-features = false`
    /// のため今は該当しないが、`persistence` を将来有効化しても drive の
    /// skip-if-unchanged 判定が壊れないよう、意図的に持続想定の API 側へ寄せる
    /// （このクレートに `persistence` feature が無いため実際のシリアライズは発生しない）。
    fn text_scale_last_applied(&self) -> Option<f32> {
        self.egui_ctx()
            .data_mut(|d| d.get_persisted::<f32>(text_scale_last_applied_id()))
    }

    /// Self::text_scale_last_applied の書き込み側（doc 参照）。
    fn set_text_scale_last_applied(&self, zoom: f32) {
        self.egui_ctx()
            .data_mut(|d| d.insert_persisted(text_scale_last_applied_id(), zoom));
    }

    /// このフレームのドラッグ & ドロップ入力（`egui::RawInput::hovered_files`/
    /// `dropped_files` への薄い委譲）。`device/drag_drop.rs::drag_drop_effect` 専用
    /// （Issue #1014）。
    ///
    /// 戻り値の第 1 要素はホバー中のファイルがあるか、第 2 要素はこのフレームで
    /// ドロップされたパスの一覧（`dropped_files[].path` は Web では常に `None`。
    /// デスクトップでは OS からパスが渡らない特殊なドロップ元で `None` になり得る）。
    /// egui-winit はドロップ一覧を `RawInput::take` で 1 度だけ消費するため、
    /// ドロップされたイベントは次フレーム以降には現れない（`device/drag_drop.rs`
    /// モジュール doc「egui-winit 0.33.3 の挙動」参照）。
    ///
    /// iOS/Android では呼ばれない（`drag_drop_effect` の iOS/Android 分岐は egui-winit を
    /// 経由しない `ios_drag_drop.rs`・`device/android.rs`/`drop_session.rs` 経由の
    /// 橋渡しを使う、Issue #1015・#1016。`cfg_attr` は iOS/Android ビルドでの
    /// `dead_code` 誤検知を避けるため）。
    fn file_drag(&self) -> (bool, Vec<Option<std::path::PathBuf>>) {
        self.egui_ctx().input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.iter().map(|f| f.path.clone()).collect(),
            )
        })
    }
}

impl<S> EguiSenderExt for CompletionSender<S> {
    fn egui_ctx(&self) -> &egui::Context {
        let waker: &dyn std::any::Any = self.waker();
        &waker
            .downcast_ref::<EguiWaker>()
            .expect("本 crate の CompletionSender は EguiWaker でのみ構築される")
            .0
    }
}

/// `CanvasWidget` のポインタ位置を `ctx.data` に置くときの `egui::Id`（`lib.rs` が書き、
/// [`EguiSenderExt::canvas_pointer`] が読む）。
#[doc(hidden)]
pub fn canvas_pointer_id(widget_raw: u64) -> egui::Id {
    egui::Id::new(("fandhe_canvas_pointer", widget_raw))
}

/// `CanvasWidget` の「このフレームに押された」を `ctx.data` に置くときの `egui::Id`
/// （`lib.rs` が書き、[`EguiSenderExt::canvas_pressed`] が読む）。
#[doc(hidden)]
pub fn canvas_pressed_id(widget_raw: u64) -> egui::Id {
    egui::Id::new(("fandhe_canvas_pressed", widget_raw))
}

/// `CanvasWidget` の「このフレームに接触が終わった位置」を `ctx.data` に置くときの `egui::Id`
/// （`lib.rs` が書き、[`EguiSenderExt::canvas_released`] が読む）。
#[doc(hidden)]
pub fn canvas_released_id(widget_raw: u64) -> egui::Id {
    egui::Id::new(("fandhe_canvas_released", widget_raw))
}

/// CompletionSender::text_scale_last_applied / set_text_scale_last_applied が使う
/// egui::Id（doc 参照）。
fn text_scale_last_applied_id() -> egui::Id {
    egui::Id::new("fandhe_egui_proxy_text_scale_last_applied")
}

/// 画面の印刷が読み戻しに付けるタグ（`UserData` の中身）。自動撮影の `UserData::default()` と
/// 取り違えないための私有の型。
struct ScreenshotTag(u64);

/// `events` から、タグが `tag` に一致する `Event::Screenshot` の画像を探す純関数。
fn find_tagged_screenshot(events: &[egui::Event], tag: u64) -> Option<Arc<egui::ColorImage>> {
    events.iter().find_map(|e| match e {
        egui::Event::Screenshot {
            user_data, image, ..
        } if user_data
            .data
            .as_ref()
            .and_then(|d| d.downcast_ref::<ScreenshotTag>())
            .is_some_and(|t| t.0 == tag) =>
        {
            Some(image.clone())
        }
        _ => None,
    })
}

/// window insets（物理 px、left/top/right/bottom）を `CentralPanel` の内側余白
/// （論理 pt の `egui::Margin`）へ変換する純関数（Issue #579）。
///
/// `insets_px` は `MainActivity`（Java 側 `OnApplyWindowInsetsListener`）が
/// `WindowInsetsCompat.Type.systemBars() | Type.displayCutout()` から取得し、
/// JNI 経由で渡す値（`android.rs` の `android_inset_margin` 参照。旧実装の
/// `AndroidApp::content_rect()` 方式は GameActivity 経路で insets を反映しないことを
/// 実機で確認したため廃止した、PR #596 是正）。負値（想定外の入力）は 0 に、
/// `egui::Margin`（`i8`）の表現上限（127pt）は飽和させてクランプする。
/// `pixels_per_point` が 0 以下（想定外の入力）の場合は 1.0 として扱い、ゼロ除算を避ける。
///
/// 呼び出し元: `android.rs` の `android_inset_margin`（`InsetMarginSupplier` 契約）。
/// ユニットテストで境界値を検証する。
///
/// 実呼び出し元（`android.rs`）は Android ターゲットのみコンパイル対象になるため、
/// それ以外のターゲットの通常ビルドでは未使用になる（`cfg(test)` のユニットテストからは
/// 使われる。純関数として全ターゲットでテスト可能にしておくための意図的な構成）。
#[doc(hidden)]
pub fn insets_px_to_margin(insets_px: (f32, f32, f32, f32), pixels_per_point: f32) -> egui::Margin {
    let (left, top, right, bottom) = insets_px;
    let ppp = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let to_pt = |px: f32| (px.max(0.0) / ppp).round().clamp(0.0, i8::MAX as f32) as i8;

    egui::Margin {
        left: to_pt(left),
        top: to_pt(top),
        right: to_pt(right),
        bottom: to_pt(bottom),
    }
}

/// `CentralPanel` を描画する。`inset_margin` が `Some` の場合、既定の `central_panel`
/// 余白（8pt）に window insets 由来の余白を加算した `Frame` を使う
/// （`Margin` は `saturating_add`、`egui::Frame::central_panel` の既定値との合成）。
/// `inset_margin` が `None`（非 Android 等）の場合は既定のまま挙動を変えない。
#[doc(hidden)]
pub fn central_panel_with_insets(
    ctx: &egui::Context,
    inset_margin: &Option<InsetMarginSupplier>,
) -> egui::CentralPanel {
    match inset_margin {
        Some(supplier) => {
            let base = egui::Frame::central_panel(&ctx.style());
            let margin = base.inner_margin + supplier(ctx);
            egui::CentralPanel::default().frame(base.inner_margin(margin))
        }
        None => egui::CentralPanel::default(),
    }
}

/// 検証用アプリケーション本体。既定では `sample_screen()`（REQ-1 の受け入れ基準:
/// テキスト・テキスト入力・ボタン・リストを含む 1 画面）を `StateStore` 経由で保持するが、
/// `S` を差し替えることで画面レジストリに登録した画面（`fandhe_core::screens::*`）も
/// 同じ描画経路（`render_screen_with_recyclers`）に載せられる（Issue #715）。
/// 画面名からの解決は `screens::build_app`（`generated_screens!` マクロ）が担い、
/// 本構造体自体は具象の `S` を知る必要がある呼び出し元（テスト・[`ProxyApp::new`]）
/// からのみ直接構築される。
pub struct ProxyApp<S = SampleState> {
    source: ScreenSource<S>,
    store: StateStore<S>,
    /// `Node::VirtualList` の項目ノードをフレームをまたいで再利用するプール（#490）。
    /// `render_screen_with_recyclers` へ毎フレーム渡す契約（`recyclers` モジュール doc
    /// 参照。egui 側の永続領域は使わず、呼び出し側であるここが保持する設計判断）。
    recyclers: VirtualListRecyclers<S>,
    /// native 自動撮影モード（`main.rs` の `FANDHE_PROXY_AUTOSHOT`）専用の状態。
    /// wasm 側では常に `None` で本フィールドは使われない。
    autoshot: Option<AutoShot>,
    /// window insets 供給関数（Issue #579）。`android.rs` が `with_inset_margin_supplier`
    /// 経由で注入する。他プラットフォームでは `None`（`central_panel_with_insets` 参照）。
    inset_margin: Option<InsetMarginSupplier>,
    /// 外部 I/O 完了キューの送信側（Issue #745）。将来 `fandhe-device`（未実装）等の
    /// 別スレッドが [`ProxyApp::completion_sender`] で clone して保持し、I/O 完了時に
    /// `Completion<S>` を送る想定。
    completions_tx: std::sync::mpsc::Sender<Completion<S>>,
    /// 外部 I/O 完了キューの受信側（Issue #745）。`update` のフレーム先頭で
    /// [`drain_completions`] へ渡す（単一コンシューマ、UI スレッド限定）。
    completions_rx: std::sync::mpsc::Receiver<Completion<S>>,
    /// [`CompletionSender::send`] が描画ループを起こすための起床口（Issue #745 是正）。
    /// `completion_sender` は毎フレーム呼ばれうるため、構築時に 1 度だけ作って共有する。
    waker: Arc<EguiWaker>,
    /// 画面非依存の汎用 I/O 起動フック（Issue #748）。`registry.rs` の `effect` 区画から
    /// [`Self::with_effect`] 経由で注入される。既定 `None`（大半の画面は端末 I/O を
    /// 持たないため、フックなしのまま何もしない）。
    effect: Option<EffectFn<S>>,
}

/// Xvfb 上での自動起動確認用: 起動から一定フレーム後にスクリーンショットを要求し、
/// 応答を受け取った時点で PNG 保存して終了コマンドを送る（`main.rs` からのみ使用）。
struct AutoShot {
    output_path: std::path::PathBuf,
    frames_waited: u32,
    requested: bool,
    done: bool,
    /// PNG 保存の成否を `main.rs` 側へ伝える共有フラグ。
    /// `eframe::App::logic` は戻り値でプロセス終了コードへ介入できないため、
    /// `run_native` 完了後に `main.rs` がこのフラグを読み、失敗時は非ゼロで終了する
    /// （Bugbot 指摘: 保存失敗時も成功終了していた不整合の是正）。
    save_failed: Arc<AtomicBool>,
}

/// [`ProxyApp`] が描画対象の `Screen<S>` をどう得るかの区別（Issue #726）。
///
/// `Fixed` は構築時に固定した 1 画面を毎フレームそのまま描く。`FromState` は
/// `ScreenStack<S>`（`fandhe_core::ScreenStack`）のような
/// 状態駆動の画面遷移に対応するため、`ProxyApp::update` が毎フレーム `f(store.state())`
/// を呼び直して現在画面を引き直す（[`app::PrototypeApp::update`] が
/// `fandhe_prototype::current_screen` を直接呼んでいたパターンの一般化）。
enum ScreenSource<S> {
    Fixed(Screen<S>),
    FromState(fn(&S) -> Screen<S>),
}

impl<S> ScreenSource<S> {
    /// `state` の現在値から描画対象の `Screen<S>` を得る。`Fixed` は保持している画面を
    /// 借用するだけ（`ProxyApp::update` が 1 フレームに 2 回呼ぶため、固定画面を
    /// 毎回クローンしない。codex-review 指摘 P1、PR #733）。`FromState` は
    /// `current(state)` の呼び出しごとに新しい `Screen<S>` を構築する必要があるため
    /// 所有値を返す。
    fn build(&self, state: &S) -> std::borrow::Cow<'_, Screen<S>> {
        match self {
            ScreenSource::Fixed(screen) => std::borrow::Cow::Borrowed(screen),
            ScreenSource::FromState(current) => std::borrow::Cow::Owned(current(state)),
        }
    }
}

impl ProxyApp<SampleState> {
    /// 既定の初期状態（`sample_screen()`）でアプリを構築する。
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_screen(ctx, sample_screen(), SampleState::default())
    }

    /// native 自動撮影モードを有効にした状態でアプリを構築する
    /// （`FANDHE_PROXY_AUTOSHOT=<path>` 指定時に `main.rs` から呼ばれる）。
    ///
    /// `save_failed` は `main.rs` が保持する共有フラグで、PNG 保存に失敗した場合に
    /// `true` がセットされる。`run_native` 完了後に `main.rs` 側で読み出し、
    /// プロセスの終了コードへ反映する契約（`AutoShot::save_failed` 参照）。
    pub fn new_with_autoshot(
        ctx: &egui::Context,
        output_path: std::path::PathBuf,
        save_failed: Arc<AtomicBool>,
    ) -> Self {
        let mut app = Self::new(ctx);
        app.set_autoshot(output_path, save_failed);
        app
    }
}

impl<S> ProxyApp<S> {
    /// 任意の `Screen<S>`・初期状態からアプリを構築する（Issue #715）。
    /// `screens::build_app`（画面名 → 画面のレジストリ解決）が呼ぶ汎用エントリで、
    /// [`ProxyApp::<SampleState>::new`] はこの薄いラッパー。
    pub fn with_screen(ctx: &egui::Context, screen: Screen<S>, state: S) -> Self {
        Self::with_source(ctx, ScreenSource::Fixed(screen), state)
    }

    /// 状態駆動で毎フレーム画面を引き直すアプリを構築する（Issue #726）。
    /// `current` は `ScreenStack<S>` を状態に持つ画面（`fandhe_core::screens::list_detail`
    /// 等）の `current_screen(&S) -> Screen<S>` のような入口関数を渡す想定
    /// （`screens::build_app` の `nav` 区画から呼ばれる、`screens.rs` 参照）。
    pub fn with_screen_fn(ctx: &egui::Context, current: fn(&S) -> Screen<S>, state: S) -> Self {
        Self::with_source(ctx, ScreenSource::FromState(current), state)
    }

    /// [`Self::with_screen`] / [`Self::with_screen_fn`] が共有する構築本体。
    fn with_source(ctx: &egui::Context, source: ScreenSource<S>, state: S) -> Self {
        crate::font::install_cjk_font(ctx);
        let (completions_tx, completions_rx) = std::sync::mpsc::channel();
        Self {
            source,
            store: StateStore::new(state),
            recyclers: VirtualListRecyclers::new(),
            autoshot: None,
            inset_margin: None,
            completions_tx,
            completions_rx,
            waker: Arc::new(EguiWaker(ctx.clone())),
            effect: None,
        }
    }

    /// 画面非依存の汎用 I/O 起動フックを注入する（Issue #748）。
    /// `registry.rs` の `effect` 区画（`generated_screens!` マクロ）が
    /// `with_screen_fn(...).with_effect(driver)` の形で呼ぶ契約。
    pub fn with_effect(mut self, effect: EffectFn<S>) -> Self {
        self.effect = Some(effect);
        self
    }

    /// 外部 I/O 完了キューへの送信ハンドルを複製して返す（Issue #745。
    /// codex-review / Cursor Bugbot 指摘 P1 是正で素の `mpsc::Sender` から
    /// [`CompletionSender`] へ変更した）。
    ///
    /// 呼び出し元（将来の `fandhe-device` 等、未実装）は返り値を別スレッドへ渡し、
    /// I/O 完了時に `Completion<S>`（`Box::new(move |state: &mut S| { ... })`）を
    /// `send` する想定。`send` が `request_repaint` を呼ぶため、アイドル中（入力待ち）
    /// の UI でも送信直後に `update` が呼ばれ、フレーム先頭で [`drain_completions`] が
    /// 取り出して `StateStore::apply` へ渡す。
    pub fn completion_sender(&self) -> CompletionSender<S> {
        CompletionSender::new(self.completions_tx.clone(), self.waker.clone())
    }

    /// window insets 供給関数を注入する（Issue #579、`android.rs` からのみ呼ばれる）。
    /// `supplier` は毎フレーム `update` 内で呼ばれるため、高コストな処理を含めない契約。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn with_inset_margin_supplier(
        mut self,
        supplier: impl Fn(&egui::Context) -> egui::Margin + 'static,
    ) -> Self {
        self.inset_margin = Some(Box::new(supplier));
        self
    }

    /// native 自動撮影モードを有効化する（Issue #715: レジストリ経由の名前指定画面にも
    /// 同じ撮影経路を注入できるよう、[`ProxyApp::new_with_autoshot`] からロジックを
    /// 切り出した）。`&mut self` を取るのは [`ProxyAppDyn::set_autoshot`] から
    /// 型消去済みの `Box<dyn ProxyAppDyn>` 越しに呼べるようにするため。
    fn set_autoshot(&mut self, output_path: std::path::PathBuf, save_failed: Arc<AtomicBool>) {
        self.autoshot = Some(AutoShot {
            output_path,
            frames_waited: 0,
            requested: false,
            done: false,
            save_failed,
        });
    }
}

/// レジストリ（`screens::build_app`）が返す型消去済み `ProxyApp<S>`（Issue #715）。
///
/// `S` はレジストリ登録時に画面ビルダー関数の戻り型から推論されるため、
/// 呼び出し元（`main.rs`/`web.rs`/`ios.rs`/`android.rs`）は具象の `S` を知らないまま
/// `Box<dyn EguiApp>` として扱いつつ、Android 限定の window insets 供給関数
/// （Issue #579）だけは本トレイト越しに注入できる。
pub trait ProxyAppDyn: EguiApp {
    /// [`ProxyApp::with_inset_margin_supplier`] の dyn 越し版。
    ///
    /// `&mut self` を取るのは、`screens::build_app` が返す時点ですでに
    /// `Box<dyn ProxyAppDyn>` として所有権が固定されており、所有権を消費する
    /// builder メソッド（`self -> Self`）を trait object 越しには呼べないため。
    fn set_inset_margin_supplier(&mut self, supplier: InsetMarginSupplier);

    /// [`ProxyApp::set_autoshot`] の dyn 越し版（Issue #715）。名前指定で解決した
    /// 生成画面にも native 自動撮影（`FANDHE_PROXY_AUTOSHOT`）を配線できるようにする。
    fn set_autoshot(&mut self, output_path: std::path::PathBuf, save_failed: Arc<AtomicBool>);
}

impl<S: 'static> ProxyAppDyn for ProxyApp<S> {
    fn set_inset_margin_supplier(&mut self, supplier: InsetMarginSupplier) {
        self.inset_margin = Some(supplier);
    }

    fn set_autoshot(&mut self, output_path: std::path::PathBuf, save_failed: Arc<AtomicBool>) {
        ProxyApp::set_autoshot(self, output_path, save_failed);
    }
}

impl<S: 'static> EguiApp for ProxyApp<S> {
    /// 自動撮影の配線（`ViewportCommand::Screenshot` の要求・応答受け取り）と
    /// 画面描画（`render_screen` への委譲）をまとめて担う。
    ///
    /// eframe 0.33 の `App` トレイトは `logic`/`ui` に分割されておらず本 `update` 一本
    /// のため、自動撮影の状態遷移（`ctx` 直接アクセスが必要）と描画（`ui` 経由）を
    /// この 1 メソッド内で順に行う（PR #306 CI 是正: rustc 1.88 互換の egui/eframe
    /// 0.33 系へダウングレードしたことに伴う API 差分の追随）。
    fn update(&mut self, ctx: &egui::Context) {
        // 外部 I/O 完了の反映はフレーム先頭・単一スレッド（UI スレッド）に限定し、
        // `dispatch`（描画内のボタン処理等）との書き込み競合を避ける（Issue #745）。
        drain_completions(&mut self.store, &self.completions_rx);

        if let Some(autoshot) = &mut self.autoshot {
            // Xvfb には入力イベントが来ないため、`request_repaint` で明示的に
            // フレームを進めないと再度呼ばれず撮影に至らない。
            ctx.request_repaint();

            if !autoshot.done {
                if !autoshot.requested {
                    autoshot.frames_waited += 1;
                    // 初回数フレームは glow の初期化・レイアウト確定を待つための猶予。
                    if autoshot.frames_waited >= 5 {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                            egui::UserData::default(),
                        ));
                        autoshot.requested = true;
                    }
                } else {
                    for event in &ctx.input(|i| i.events.clone()) {
                        if let egui::Event::Screenshot { image, .. } = event {
                            if let Err(err) = crate::save_color_image(image, &autoshot.output_path)
                            {
                                // 自動運転モード: 失敗を panic にせず標準エラーへ記録した上で、
                                // `save_failed` フラグを立てて `main.rs` に伝播させる。
                                // フラグを立てるだけでは `done`/`Close` の配線は変えず、
                                // ウィンドウは通常どおり閉じつつプロセスの終了コードで
                                // 失敗を検知できるようにする（coding-rust.md の
                                // 「panic を API 契約にしない」方針・Bugbot 指摘の是正）。
                                eprintln!("スクリーンショットの保存に失敗した: {err}");
                                autoshot.save_failed.store(true, Ordering::SeqCst);
                            }
                            autoshot.done = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            }
        }

        // `ScreenSource::FromState`（`ScreenStack<S>` 等の状態駆動画面、Issue #726）は
        // 毎フレーム引き直す必要があるため、描画のたびに `source.build` を呼ぶ
        // （`Fixed` は `Cow::Borrowed` で保持画面を借用するだけ、クローンしない。
        // codex-review 指摘 P1、PR #733）。
        let screen = self.source.build(self.store.state());

        central_panel_with_insets(ctx, &self.inset_margin).show(ctx, |ui| {
            crate::EguiRenderer::new(ui, &mut self.recyclers).render(&screen, &mut self.store);
        });

        // OS の戻る操作（Esc/Android バックキー）は、描画中のボタン処理で
        // `store` の画面スタックが変わった可能性があるため、描画に使った
        // `screen` をそのまま渡さず `source.build` で `store` の最新状態から
        // 再取得した画面へ dispatch する（同一フレームに戻るボタンと
        // BrowserBack 等の複合入力が来ても、1 フレーム内で一貫した画面へ
        // イベントを送るため。PR #733 レビュー是正）。`Fixed` はここでも
        // `Cow::Borrowed` を返すため 2 回目の呼び出しでも複製は発生しない。
        //
        // `ScreenSource::FromState` は Esc/BrowserBack が来ていないフレームでも毎回
        // 呼ぶ（Esc/BrowserBack の有無で事前に呼び分けることも検討したが、
        // `dispatch_back_if_requested` はテキスト入力のフォーカス状態を 1 フレーム分
        // 持ち越すブックキーピングを毎フレーム行う契約になっており（同関数のコメント
        // 参照）、呼び出しを間引くとその情報が古いフレームのまま取り残されて
        // 誤判定を招く。`Fixed` はゼロコピーになった以上、この 2 回目の呼び出し自体の
        // コストは小さく単純化を優先した。codex-review 指摘 P1 の判断根拠）。
        let screen_after_render = self.source.build(self.store.state());
        crate::dispatch_back_if_requested(ctx, &screen_after_render, &mut self.store);

        // 汎用 I/O 起動フック（Issue #748）。`drain_completions` で確定した最新状態
        // （上記 `dispatch_back_if_requested` までの反映を経た `store.state()`）を渡し、
        // フレーム末尾で 1 回だけ呼ぶ。`&S` の借用のみのため `effect` 自身は状態を
        // 変更できず、変更が必要な場合は `CompletionSender::send` → 次フレーム先頭の
        // `drain_completions` を経由する（同一フレーム内で二重に起動要求を観測しない
        // ための順序契約、`EffectFn` の doc 参照）。
        if let Some(effect) = self.effect {
            effect(self.store.state(), &self.completion_sender());
        }
    }

    /// ウィンドウを閉じる（プロセスは直後に終了する）タイミングで、登録済みの終了フック
    /// （[`set_teardown_hooks`]）を呼ぶ。`static` に保持しているため `Drop` が走らない端末リソース
    /// （TTS の子プロセス等）を明示的に止めるためのもの（Issue #865 の codex-review P1 指摘）。
    /// 何を止めるかは端末機能の配線を持つ側（`fandhe-egui-proxy`）が登録する。
    fn on_exit(&mut self) {
        if let Some((on_exit, _)) = TEARDOWN_HOOKS.get() {
            on_exit();
        }
    }
}

/// 画面（`ProxyApp`）の破棄時に、停止操作なしで残りうる購読を解放する（同一プロセスで別画面へ
/// 切り替えても guard・通知登録が残らないようにする。メモリ逼迫の通知、Issue #911）。
/// 実際の解放処理は登録済みの終了フック（[`set_teardown_hooks`]）の `on_drop`。
impl<S> Drop for ProxyApp<S> {
    fn drop(&mut self) {
        if let Some((_, on_drop)) = TEARDOWN_HOOKS.get() {
            on_drop();
        }
    }
}

/// [`set_teardown_hooks`] が保持する (on_exit, on_drop)。プロセスで 1 組。
static TEARDOWN_HOOKS: std::sync::OnceLock<TeardownHooks> = std::sync::OnceLock::new();

/// (on_exit, on_drop)。
type TeardownHooks = (fn(), fn());

/// [`ProxyApp`] の終了・破棄時に呼ぶ解放処理を登録する（プロセスで 1 回。2 回目以降は無視）。
///
/// 端末機能（`fandhe-device` 等）を知らない描画層から、端末リソースの解放を呼べるようにする口で、
/// `fandhe_runtime::set_repaint_waker` と同じ形。`on_exit` はウィンドウを閉じる経路
/// （[`EguiApp::on_exit`]。モバイルでは届かない）、`on_drop` は `ProxyApp` の `Drop` で呼ぶ。
/// 未登録なら何もしない。Android は `android_main` が再実行されうるため、2 回目以降を無視する。
pub fn set_teardown_hooks(on_exit: fn(), on_drop: fn()) {
    let _ = TEARDOWN_HOOKS.set((on_exit, on_drop));
}

#[cfg(test)]
mod completion_queue_tests {
    use super::{egui_sender, Completion, EguiSenderExt};

    /// テスト用の最小状態: カウンタとメモ欄を持つ（`fandhe_core::state::store::tests`
    /// の `Counter` と同じ構成、GPU 不要でロジックのみ検証する）。
    #[derive(Default)]
    struct Counter {
        count: u32,
    }

    /// `egui_ctx` の downcast が実際の `EguiWaker` へ届くこと（`&Arc<dyn _>` を
    /// `&dyn Any` へ誤って強制すると実行時に失敗する）を固定する。
    #[test]
    fn egui_sender_ext_reaches_context() {
        let (tx, _rx) = std::sync::mpsc::channel::<Completion<Counter>>();
        let sender = egui_sender(tx, egui::Context::default());
        let _ = sender.focused(); // downcast に失敗すると panic する
    }

    /// [`CompletionSender::send`] が `request_repaint` を呼び、アイドル中（入力待ち）の
    /// egui コンテキストにも再描画要求が伝わることを確認する（codex-review /
    /// Cursor Bugbot 指摘 P1 の是正、Issue #745）。`Context::has_requested_repaint` は
    /// 新規 `Context` でも常に true を返す実装（初期状態で `outstanding` が非ゼロ）の
    /// ため差分検知に使えず、代わりに `set_request_repaint_callback` で
    /// `request_repaint` が実際に呼ばれたことを直接観測する。
    #[test]
    fn completion_sender_send_requests_repaint() {
        let (tx, _rx) = std::sync::mpsc::channel::<Completion<Counter>>();
        let ctx = egui::Context::default();
        let repaint_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let repaint_requested_writer = repaint_requested.clone();
        ctx.set_request_repaint_callback(move |_| {
            repaint_requested_writer.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let sender = egui_sender(tx, ctx);

        assert!(!repaint_requested.load(std::sync::atomic::Ordering::SeqCst));

        sender
            .send(Box::new(|state: &mut Counter| state.count += 1))
            .expect("受信側が生きているため送信は成功する");

        assert!(repaint_requested.load(std::sync::atomic::Ordering::SeqCst));
    }
}

#[cfg(test)]
mod inset_margin_tests {
    use super::insets_px_to_margin;

    /// insets なし（すべて 0px）なら margin もゼロ（Issue #579）。
    #[test]
    fn no_insets_yields_zero_margin() {
        let margin = insets_px_to_margin((0.0, 0.0, 0.0, 0.0), 3.0);
        assert_eq!(margin, egui::Margin::ZERO);
    }

    /// ステータスバー分の top inset を pixels_per_point で論理 pt へ換算する。
    #[test]
    fn top_inset_converts_to_logical_points() {
        // top inset = 72px, pixels_per_point = 3.0 -> 24pt。
        let margin = insets_px_to_margin((0.0, 72.0, 0.0, 0.0), 3.0);
        assert_eq!(margin.top, 24);
        assert_eq!(margin.left, 0);
        assert_eq!(margin.right, 0);
        assert_eq!(margin.bottom, 0);
    }

    /// ナビゲーションバー分の bottom inset を換算する。
    #[test]
    fn bottom_inset_converts_to_logical_points() {
        let margin = insets_px_to_margin((0.0, 0.0, 0.0, 48.0), 3.0);
        assert_eq!(margin.bottom, 16);
    }

    /// 想定外の負値入力は 0 にクランプする（JNI 経由の値なので理論上は非負のはずだが、
    /// 純関数として不正入力にも安全側で倒す）。
    #[test]
    fn negative_values_clamp_to_zero() {
        let margin = insets_px_to_margin((-10.0, -5.0, 0.0, 0.0), 3.0);
        assert_eq!(margin.left, 0);
        assert_eq!(margin.top, 0);
    }

    /// `egui::Margin` は `i8` 格納のため、想定外に大きな inset でも飽和させる（127pt 上限）。
    #[test]
    fn large_inset_saturates_at_i8_max() {
        let margin = insets_px_to_margin((0.0, 100_000.0, 0.0, 200_000.0), 1.0);
        assert_eq!(margin.top, i8::MAX);
    }

    /// `pixels_per_point` が 0 以下の想定外入力ではゼロ除算せず 1.0 として扱う。
    #[test]
    fn non_positive_pixels_per_point_falls_back_to_one() {
        let margin = insets_px_to_margin((0.0, 24.0, 0.0, 0.0), 0.0);
        assert_eq!(margin.top, 24);
    }
}

#[cfg(test)]
mod tagged_screenshot_tests {
    use super::*;

    fn shot(user_data: egui::UserData) -> egui::Event {
        egui::Event::Screenshot {
            viewport_id: egui::ViewportId::ROOT,
            user_data,
            image: Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::RED)),
        }
    }

    #[test]
    fn only_matching_tag_is_returned() {
        let events = vec![
            shot(egui::UserData::default()),
            shot(egui::UserData::new(ScreenshotTag(1))),
        ];
        assert!(find_tagged_screenshot(&events, 2).is_none());
        assert!(find_tagged_screenshot(&events[..1], 1).is_none());
        assert!(find_tagged_screenshot(&events, 1).is_some());
    }
}
