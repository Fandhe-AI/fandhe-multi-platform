//! `eframe::App` 実装（TASK-1.2）。デスクトップ相当（`main.rs`）・Web（`web.rs`）の
//! 双方から共有される、検証対象の 1 画面（`fandhe_core::sample_screen`）を保持する
//! アプリケーション本体。
//!
//! 実際の DSL → egui マッピングは [`crate::render_screen`] に委譲し、本モジュールは
//! `StateStore` の保持・CJK フォント読み込み・（native 限定の）自動撮影の配線のみを担う。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fandhe_core::{sample_screen, SampleState, Screen, StateStore};

use crate::recyclers::VirtualListRecyclers;
use crate::tv_focus::TvFocusController;
use crate::tv_sample::{tv_sample_screen, TvState};

/// `CentralPanel` の `inner_margin` へ加算する window insets 供給関数（Issue #579）。
///
/// `android.rs`（`cfg(target_os = "android")`）が `AndroidApp::content_rect()` を
/// 捕捉したクロージャを注入する契約で、非 Android では `None`（追加 margin なし・
/// 既存の描画結果を変えない）のまま。Android 専用の型（`android-activity` の
/// `Rect` 等）を本モジュールへ持ち込まないよう、契約は `&egui::Context -> egui::Margin`
/// にとどめている（cfg(target_os = "android") を android.rs 側に閉じ込める設計判断）。
///
/// `pub(crate)` にしているのは、レジストリ経由（`screens::build_app`）で型消去された
/// `Box<dyn ProxyAppDyn>` へも同じ供給関数を注入できるようにするため
/// （`ProxyAppDyn::set_inset_margin_supplier` / `android.rs` 参照、Issue #715）。
pub(crate) type InsetMarginSupplier = Box<dyn Fn(&egui::Context) -> egui::Margin>;

/// 外部 I/O（保存・HTTP・カメラ等、Phase 8。`fandhe-device` は未実装）の完了結果を
/// `StateStore::apply`（`fandhe_core::state::store` 参照、Issue #745）へ渡すクロージャ。
///
/// 別スレッド（I/O 完了通知元）から [`ProxyApp::completion_sender`] 経由でチャンネルへ
/// 送られ、UI スレッドの [`drain_completions`] が `update` のフレーム先頭で取り出して
/// 実行する契約。`Send` 境界により、生成元スレッドが異なっても UI スレッドへ安全に
/// 渡せることを型で保証する。
pub type Completion<S> = Box<dyn FnOnce(&mut S) + Send>;

/// 画面非依存の汎用 I/O 起動フック（Phase 8、Issue #748）。`ProxyApp::update` の
/// フレーム末尾（`dispatch_back_if_requested` の後）で `effect(store.state(), &sender)`
/// として毎フレーム呼ばれる契約。`&S` の借用のみを受け取り状態を直接変更しないため
/// （変更は必ず [`CompletionSender::send`] → `apply` 経由、`version` はここでは進まない）、
/// 「起動が必要か」を状態側のフラグ（例: `fandhe_core::screens::http_fetch::FetchPhase::
/// Requested`）で表現する画面と組み合わせて使う設計。`registry.rs` の `effect` 区画
/// （`generated_screens!` マクロ）が画面ごとのドライバ関数をここへ配線する。
pub type EffectFn<S> = fn(&S, &CompletionSender<S>);

/// [`ProxyApp::completion_sender`] が返す送信ハンドル（Issue #745、codex-review /
/// Cursor Bugbot 指摘 P1 の是正）。
///
/// 素の `mpsc::Sender` をそのまま渡すと、送信先の UI（eframe）が入力待ちでアイドル
/// 状態のとき `update` 自体が呼ばれず、次の入力が来るまで [`drain_completions`] が
/// 実行されない（自動撮影を無効にした通常起動では入力がない限りフレームが進まない）。
/// このラッパーは `send` の中で `egui::Context::request_repaint` を呼び、
/// キュー投入と UI 起床を一体化することでこの構造的欠陥を塞ぐ。
pub struct CompletionSender<S> {
    tx: std::sync::mpsc::Sender<Completion<S>>,
    ctx: egui::Context,
}

// `#[derive(Clone)]` は `S: Clone` を要求してしまうが、複製が必要なのは
// `tx`（`mpsc::Sender` は常に `Clone`）と `ctx`（内部 `Arc` で常に `Clone`）
// のみで、`S` 自体を複製するわけではない。手動実装で不要な境界を外し、
// `SampleState` 等の非 `Clone` 状態型でも I/O スレッドへハンドルを複製できるようにする
// （cursor[bot] / codex-review 指摘、Issue #745 是正）。
impl<S> Clone for CompletionSender<S> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            ctx: self.ctx.clone(),
        }
    }
}

impl<S> CompletionSender<S> {
    /// テスト専用の直接構築（Issue #748）。通常経路は [`ProxyApp::completion_sender`]
    /// のみで、本体コードから直接構築することはない（`device/http_fetch.rs` /
    /// `device/camera_capture.rs` の単体テストが `egui::Context::default()` +
    /// 自前 `mpsc::channel` から effect ドライバ単体を検証するために使う）。
    #[cfg(test)]
    pub(crate) fn new(tx: std::sync::mpsc::Sender<Completion<S>>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }

    /// 完了クロージャをキューへ積み、直後に `request_repaint` で UI スレッドを
    /// 起こす。戻り値・エラー型は `mpsc::Sender::send` に合わせている
    /// （受信側 `ProxyApp` がドロップ済みの場合のみ `Err`）。
    pub fn send(
        &self,
        completion: Completion<S>,
    ) -> Result<(), std::sync::mpsc::SendError<Completion<S>>> {
        self.tx.send(completion)?;
        self.ctx.request_repaint();
        Ok(())
    }

    /// キューへは何も積まず、`duration` 経過後に 1 回だけ `update` を起こす予約のみ行う
    /// （`egui::Context::request_repaint_after` への薄い委譲）。
    ///
    /// [`send`](Self::send) は完了と同時に即座に起床させる契約だが、こちらは
    /// 「今は送るものが無いが、外部状態（センサ等）が変わっていないか後で
    /// 再確認したい」場合に使う。pull 方式のセンサ実装（`device/tilt.rs` の
    /// iOS 経路、Issue #832 是正）が、静止判定で送信をスキップした後も定期的に
    /// センサ値を pull し直すために呼ぶ想定。
    pub fn request_repaint_after(&self, duration: std::time::Duration) {
        self.ctx.request_repaint_after(duration);
    }

    /// `egui::Context` の複製を返す。全画面表示（`device/fullscreen_immersive.rs`）が
    /// `ViewportCommand::Fullscreen` の送信と `viewport().fullscreen` の読み戻しに使う専用の口で、
    /// 他の機能での流用はしない（ctx を丸ごと渡す口になるため）。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn egui_context(&self) -> egui::Context {
        self.ctx.clone()
    }

    /// このフレームでウィンドウがフォーカスを持っているか（`egui::RawInput::focused`
    /// への薄い委譲）。Windows 共有シートのキャンセル検知専用
    /// （`device/share.rs::windows_cancel_check` が「共有 UI へフォーカスを奪われた
    /// → 復帰後も結果未着」を判定するために使う。`DataTransferManager` に
    /// 「共有 UI を閉じた」イベントが無いための代替、`device/windows_share.rs`
    /// モジュール doc「キャンセル検知の天井」参照）。
    #[cfg_attr(not(all(target_os = "windows", feature = "device")), allow(dead_code))]
    pub(crate) fn focused(&self) -> bool {
        self.ctx.input(|i| i.raw.focused)
    }

    /// ポインタの主ボタン（左クリック/タップ）がこのフレームで押下中か
    /// （`egui::PointerState::primary_down` への薄い委譲）。Issue #912「文字サイズ追従」の
    /// effect ドライバ（`device/text_scale.rs::drive`）専用。スライダーのドラッグ中は
    /// `set_zoom_factor` を保留し離した後に適用するための判定に使う
    /// （Cursor Bugbot 指摘 "Slider drag fights live zoom"、PR #920 是正。ドラッグ中に
    /// `pixels_per_point` を変えると、スライダー自身が次パスで別の point 空間の矩形へ
    /// 再マップされ、指/カーソル追従を失って倍率が飛ぶため）。
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn pointer_primary_down(&self) -> bool {
        self.ctx.input(|i| i.pointer.primary_down())
    }

    /// キャンバス `id` 上で主ボタンを押下中のポインタ位置（キャンバス左上原点）。押していなければ
    /// `None`。`lib.rs` の `CanvasWidget` 分岐が毎フレーム書いた値を `ctx.data` から読むだけの
    /// 薄い委譲で、effect はフレーム末尾で呼ばれるため同じフレームの値が見える
    /// （Issue #355 `device/pen_stylus.rs` 専用）。
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn canvas_pointer(&self, id: fandhe_core::WidgetId) -> Option<(f32, f32)> {
        self.ctx
            .data(|d| d.get_temp::<Option<(f32, f32)>>(canvas_pointer_id(id.raw())))
            .flatten()
    }

    /// キャンバス `id` 上で、このフレームに主ボタンが新しく押されたか（`canvas_pointer` と同じく
    /// `lib.rs` の `CanvasWidget` 分岐が毎フレーム書く）。1 フレーム内の「離す → 押す」を
    /// 新しい接触として扱うため（Issue #355）。
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn canvas_pressed(&self, id: fandhe_core::WidgetId) -> bool {
        self.ctx
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
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn set_zoom_factor(&self, zoom: f32) {
        self.ctx.set_zoom_factor(zoom);
        // set_zoom_factor 自体は repaint を要求しない（egui::Context::set_zoom_factor の
        // 実装詳細）。effect はフレーム末尾で呼ばれるため（本モジュール doc）、ここで
        // request_repaint しないと「次パスで zoom を反映する」ための次フレームが
        // 入力待ちのアイドル状態のまま起こらず、スライダー/OS 値が変わっても描画に
        // 反映されないまま止まる（Cursor Bugbot 指摘、PR #920 是正。send が同じ理由で
        // request_repaint する設計と揃える）。
        self.ctx.request_repaint();
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
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn text_scale_last_applied(&self) -> Option<f32> {
        self.ctx
            .data_mut(|d| d.get_persisted::<f32>(text_scale_last_applied_id()))
    }

    /// Self::text_scale_last_applied の書き込み側（doc 参照）。
    #[cfg_attr(
        not(all(feature = "device", not(target_arch = "wasm32"))),
        allow(dead_code)
    )]
    pub(crate) fn set_text_scale_last_applied(&self, zoom: f32) {
        self.ctx
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
    #[cfg_attr(
        any(
            not(all(feature = "device", not(target_arch = "wasm32"))),
            target_os = "ios",
            target_os = "android"
        ),
        allow(dead_code)
    )]
    pub(crate) fn file_drag(&self) -> (bool, Vec<Option<std::path::PathBuf>>) {
        self.ctx.input(|i| {
            (
                !i.raw.hovered_files.is_empty(),
                i.raw.dropped_files.iter().map(|f| f.path.clone()).collect(),
            )
        })
    }
}

/// `CanvasWidget` のポインタ位置を `ctx.data` に置くときの `egui::Id`（`lib.rs` が書き、
/// [`CompletionSender::canvas_pointer`] が読む）。
pub(crate) fn canvas_pointer_id(widget_raw: u64) -> egui::Id {
    egui::Id::new(("fandhe_canvas_pointer", widget_raw))
}

/// `CanvasWidget` の「このフレームに押された」を `ctx.data` に置くときの `egui::Id`
/// （`lib.rs` が書き、[`CompletionSender::canvas_pressed`] が読む）。
pub(crate) fn canvas_pressed_id(widget_raw: u64) -> egui::Id {
    egui::Id::new(("fandhe_canvas_pressed", widget_raw))
}

/// CompletionSender::text_scale_last_applied / set_text_scale_last_applied が使う
/// egui::Id（doc 参照）。
#[cfg_attr(
    not(all(feature = "device", not(target_arch = "wasm32"))),
    allow(dead_code)
)]
fn text_scale_last_applied_id() -> egui::Id {
    egui::Id::new("fandhe_egui_proxy_text_scale_last_applied")
}

/// キューに溜まった完了クロージャを非ブロッキングで取り出し、`store.apply` へ順に渡す
/// （FIFO、Issue #745）。`ProxyApp::update` のフレーム先頭から呼ばれる想定だが、
/// GPU（`eframe::App`）を要さない純粋なロジックとして独立させ、ユニットテストで
/// 直接検証できるようにしている。
///
/// キューが空の場合は `try_recv` が即座に `Err` を返すため何もしない（`version` は
/// 進まない）。送信側の不具合で完了が大量に滞留した場合、1 フレームでの drain 処理
/// 時間が伸びメモリも増加しうるが、`std::sync::mpsc::channel`（無制限キュー）を使う
/// 制約として許容する（Phase 8 で実際の送信元が実装された段階で必要なら
/// `sync_channel` への変更を検討。out-of-scope-tracking.md 対象になり得る）。
fn drain_completions<S>(store: &mut StateStore<S>, rx: &std::sync::mpsc::Receiver<Completion<S>>) {
    while let Ok(completion) = rx.try_recv() {
        store.apply(completion);
    }
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
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn insets_px_to_margin(
    insets_px: (f32, f32, f32, f32),
    pixels_per_point: f32,
) -> egui::Margin {
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
fn central_panel_with_insets(
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
    /// [`CompletionSender::send`] が `request_repaint` を呼ぶために保持する
    /// egui コンテキスト（Issue #745 是正）。`egui::Context` は内部 `Arc` で
    /// 安価に `Clone` できるため、`CreationContext` から複製して持つだけで済む。
    egui_ctx: egui::Context,
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
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_screen(cc, sample_screen(), SampleState::default())
    }

    /// native 自動撮影モードを有効にした状態でアプリを構築する
    /// （`FANDHE_PROXY_AUTOSHOT=<path>` 指定時に `main.rs` から呼ばれる）。
    ///
    /// `save_failed` は `main.rs` が保持する共有フラグで、PNG 保存に失敗した場合に
    /// `true` がセットされる。`run_native` 完了後に `main.rs` 側で読み出し、
    /// プロセスの終了コードへ反映する契約（`AutoShot::save_failed` 参照）。
    pub fn new_with_autoshot(
        cc: &eframe::CreationContext<'_>,
        output_path: std::path::PathBuf,
        save_failed: Arc<AtomicBool>,
    ) -> Self {
        let mut app = Self::new(cc);
        app.set_autoshot(output_path, save_failed);
        app
    }
}

impl<S> ProxyApp<S> {
    /// 任意の `Screen<S>`・初期状態からアプリを構築する（Issue #715）。
    /// `screens::build_app`（画面名 → 画面のレジストリ解決）が呼ぶ汎用エントリで、
    /// [`ProxyApp::<SampleState>::new`] はこの薄いラッパー。
    pub fn with_screen(cc: &eframe::CreationContext<'_>, screen: Screen<S>, state: S) -> Self {
        Self::with_source(cc, ScreenSource::Fixed(screen), state)
    }

    /// 状態駆動で毎フレーム画面を引き直すアプリを構築する（Issue #726）。
    /// `current` は `ScreenStack<S>` を状態に持つ画面（`fandhe_core::screens::list_detail`
    /// 等）の `current_screen(&S) -> Screen<S>` のような入口関数を渡す想定
    /// （[`screens::build_app`] の `nav` 区画から呼ばれる、`screens.rs` 参照）。
    pub fn with_screen_fn(
        cc: &eframe::CreationContext<'_>,
        current: fn(&S) -> Screen<S>,
        state: S,
    ) -> Self {
        Self::with_source(cc, ScreenSource::FromState(current), state)
    }

    /// [`Self::with_screen`] / [`Self::with_screen_fn`] が共有する構築本体。
    fn with_source(cc: &eframe::CreationContext<'_>, source: ScreenSource<S>, state: S) -> Self {
        crate::font::install_cjk_font(&cc.egui_ctx);
        // Issue #898: Windows 共有シート（`IDataTransferManagerInterop::GetForWindow`）が
        // 要求する HWND は `CreationContext` からしか得られない。effect ドライバ
        // （`device/share.rs`）はここへ触れられないため、構築時に 1 度だけ static へ
        // 控えておく（`device/windows_share.rs::remember_window` doc 参照）。
        // Issue #907: 生体認証（Windows Hello、`device/windows_biometric.rs`）も
        // `windows_share::window()` 経由で同じ HWND を使う。
        #[cfg(all(target_os = "windows", feature = "device"))]
        crate::device::windows_share::remember_window(cc);
        let (completions_tx, completions_rx) = std::sync::mpsc::channel();
        Self {
            source,
            store: StateStore::new(state),
            recyclers: VirtualListRecyclers::new(),
            autoshot: None,
            inset_margin: None,
            completions_tx,
            completions_rx,
            egui_ctx: cc.egui_ctx.clone(),
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
        CompletionSender {
            tx: self.completions_tx.clone(),
            ctx: self.egui_ctx.clone(),
        }
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

// TASK-2.3（#54）計測マーカー: 以下はプロトタイプ描画対象切替（TASK-2.2-S5 / #52）の
// 専用型。共通/プラットフォーム別コード比率計測（分母）としてマーカー登録する
// （docs/architecture/code-ratio-classification.md・scripts/metrics/code-classification.env 参照）。
// fandhe-prototype:begin
/// プロトタイプアプリ（`fandhe-prototype`、TASK-2.2）の実描画対象。
///
/// `sample_screen()` を固定で保持する [`ProxyApp`] と異なり、`AppState::route` の
/// 変化に追従して表示画面を毎フレーム引き直す必要があるため、`Screen<AppState>` を
/// キャッシュせず `update` のたびに `fandhe_prototype::current_screen` へ問い合わせる
/// （設計書 4 章のシーケンス「dispatch → version 進行 → current_screen 引き直し →
/// 再描画」の実配線、S5 #52 のスコープ）。
pub struct PrototypeApp {
    store: fandhe_core::StateStore<fandhe_prototype::AppState>,
    /// `Node::VirtualList` の項目ノードをフレームをまたいで再利用するプール（#490）。
    /// `current_screen` が毎フレーム画面を引き直す設計（本構造体の doc 参照）でも、
    /// 「同一 version → 同一画面」の純関数契約さえ保たれれば `path` ベースの
    /// プール識別（`recyclers` モジュール doc 参照）はそのまま機能する
    /// （route 遷移は dispatch 経由でのみ起き、その際は version が進むため
    /// `ListRecycler::acquire` が全キャッシュを無効化し、stale ノードは混入しない）。
    recyclers: VirtualListRecyclers<fandhe_prototype::AppState>,
}

impl PrototypeApp {
    /// 既定の初期状態（一覧画面・空の項目一覧）でアプリを構築する。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::font::install_cjk_font(&cc.egui_ctx);
        Self {
            store: fandhe_core::StateStore::new(fandhe_prototype::AppState::default()),
            recyclers: VirtualListRecyclers::new(),
        }
    }
}

impl eframe::App for PrototypeApp {
    /// 現在の `AppState` から表示画面を引き直し、`render_screen_with_recyclers` へ
    /// 委譲する（フレーム跨ぎ再利用プールを保持する版、#490）。
    ///
    /// `render_screen_with_recyclers` は描画中に発火したイベントを走査完了後に
    /// まとめて `store.dispatch` するため、遷移ハンドラ（`nav::go_to_*` 等）による
    /// `route` の書き換えは次フレームの `current_screen` 呼び出しに反映される
    /// （即時の画面差し替えではなく、既存の version 駆動再描画の枠組みに従う）。
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let screen = fandhe_prototype::current_screen(self.store.state());
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::render_screen_with_recyclers(ui, &screen, &mut self.store, &mut self.recyclers);
        });
    }
}

// fandhe-prototype:end

/// TV フォーカスナビゲーション検証用アプリケーション本体（TASK-17.2 / REQ-17）。
///
/// `ProxyApp`/`PrototypeApp` と同じく `StateStore` 経由で状態を保持する（#536 で
/// [`TvFocusController::update`] の決定操作・クリックが `StateStore::dispatch`
/// （`Event::ButtonActivatedAt`）へ統一されたことに伴い、直接所有をやめた）。
/// `tv_sample_screen()`（`tv_sample.rs`）を固定で保持し、実描画・入力処理は
/// [`TvFocusController::update`] へ委譲する。
pub struct TvFocusApp {
    screen: Screen<TvState>,
    store: StateStore<TvState>,
    controller: TvFocusController<TvState>,
    /// window insets 供給関数（Issue #579、`ProxyApp::inset_margin` と同じ契約）。
    inset_margin: Option<InsetMarginSupplier>,
}

impl TvFocusApp {
    /// 既定の初期状態でアプリを構築する。
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::font::install_cjk_font(&cc.egui_ctx);
        Self {
            screen: tv_sample_screen(),
            store: StateStore::new(TvState::default()),
            controller: TvFocusController::new(),
            inset_margin: None,
        }
    }

    /// window insets 供給関数を注入する（Issue #579、`ProxyApp::with_inset_margin_supplier`
    /// と同じ契約。`android.rs` からのみ呼ばれる）。
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn with_inset_margin_supplier(
        mut self,
        supplier: impl Fn(&egui::Context) -> egui::Margin + 'static,
    ) -> Self {
        self.inset_margin = Some(Box::new(supplier));
        self
    }
}

impl eframe::App for TvFocusApp {
    /// 矢印キー/D-pad・決定操作の受付から描画までを [`TvFocusController::update`] へ
    /// 一括委譲する（入力受付には `ui.ctx()` が必要なため `CentralPanel` の内側で
    /// 呼ぶ、`TvFocusController::update` の doc 参照）。
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        central_panel_with_insets(ctx, &self.inset_margin).show(ctx, |ui| {
            self.controller.update(ui, &self.screen, &mut self.store);
        });
    }
}

/// レジストリ（`screens::build_app`）が返す型消去済み `ProxyApp<S>`（Issue #715）。
///
/// `S` はレジストリ登録時に画面ビルダー関数の戻り型から推論されるため、
/// 呼び出し元（`main.rs`/`web.rs`/`ios.rs`/`android.rs`）は具象の `S` を知らないまま
/// `Box<dyn eframe::App>` として扱いつつ、Android 限定の window insets 供給関数
/// （Issue #579）だけは本トレイト越しに注入できる。
pub trait ProxyAppDyn: eframe::App {
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

impl<S: 'static> eframe::App for ProxyApp<S> {
    /// 自動撮影の配線（`ViewportCommand::Screenshot` の要求・応答受け取り）と
    /// 画面描画（`render_screen` への委譲）をまとめて担う。
    ///
    /// eframe 0.33 の `App` トレイトは `logic`/`ui` に分割されておらず本 `update` 一本
    /// のため、自動撮影の状態遷移（`ctx` 直接アクセスが必要）と描画（`ui` 経由）を
    /// この 1 メソッド内で順に行う（PR #306 CI 是正: rustc 1.88 互換の egui/eframe
    /// 0.33 系へダウングレードしたことに伴う API 差分の追随）。
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            crate::render_screen_with_recyclers(ui, &screen, &mut self.store, &mut self.recyclers);
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

    // `eframe::App::on_exit` のシグネチャは eframe crate 自身の "glow" feature の
    // 有効/無効で変わる（`eframe::epi::App` の doc・`#[cfg(feature = "glow")]` 参照）。
    // Cargo の `cfg(feature = "...")` は自クレートの feature しか見えないため、
    // 本クレートの `Cargo.toml` が eframe へ "glow" を渡しているターゲット集合
    // （native デスクトップ + wasm32、`not(any(ios, android))`）をそのままここでも
    // 判定条件に使う（`Cargo.toml` の eframe 依存の target 分岐コメント参照）。
    //
    // ウィンドウを閉じる（プロセスは直後に終了する）タイミングで、`static` に
    // 保持しているため `Drop` が走らない端末リソース（TTS の子プロセス等）を
    // 明示的に止める（Issue #865 の codex-review P1 指摘: `CommandTts` を
    // `OnceLock` に保持しているため `Drop` が実行されず、発話中の子プロセスが
    // 残り続ける）。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        shutdown_devices();
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    fn on_exit(&mut self) {
        shutdown_devices();
    }
}

/// [`ProxyApp::on_exit`] から呼ぶ端末リソースの解放処理。`fandhe-device`（feature
/// `device`、既定 on）を持たないビルド（wasm32 は依存自体が入らない、`device.rs`
/// モジュール doc 参照）では何もしない。
#[cfg(all(feature = "device", not(target_arch = "wasm32")))]
fn shutdown_devices() {
    crate::device::shutdown();
}

#[cfg(not(all(feature = "device", not(target_arch = "wasm32"))))]
fn shutdown_devices() {}

#[cfg(test)]
mod completion_queue_tests {
    use super::{drain_completions, Completion, CompletionSender};
    use fandhe_core::StateStore;

    /// テスト用の最小状態: カウンタとメモ欄を持つ（`fandhe_core::state::store::tests`
    /// の `Counter` と同じ構成、GPU 不要でロジックのみ検証する）。
    #[derive(Default)]
    struct Counter {
        count: u32,
        note: String,
    }

    /// キューに積んだ完了クロージャが `apply` され、`version` が進むことを確認する。
    #[test]
    fn drain_completions_applies_queued_completion_and_bumps_version() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = std::sync::mpsc::channel::<Completion<Counter>>();
        tx.send(Box::new(|state: &mut Counter| state.count += 1))
            .expect("受信側が生きているため送信は成功する");

        drain_completions(&mut store, &rx);

        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
    }

    /// 複数の完了が FIFO 順（送信順）で適用されることを確認する。
    #[test]
    fn drain_completions_applies_multiple_completions_in_order() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = std::sync::mpsc::channel::<Completion<Counter>>();
        tx.send(Box::new(|state: &mut Counter| state.note.push('a')))
            .expect("受信側が生きているため送信は成功する");
        tx.send(Box::new(|state: &mut Counter| state.note.push('b')))
            .expect("受信側が生きているため送信は成功する");

        drain_completions(&mut store, &rx);

        assert_eq!(store.state().note, "ab");
        assert_eq!(store.version(), 2);
    }

    /// キューが空の場合は何もせず、`version` も変化しないことを確認する。
    #[test]
    fn drain_completions_is_noop_when_queue_empty() {
        let mut store = StateStore::new(Counter::default());
        let (_tx, rx) = std::sync::mpsc::channel::<Completion<Counter>>();

        drain_completions(&mut store, &rx);

        assert_eq!(store.version(), 0);
    }

    /// 別スレッドから送信した完了も UI スレッド側の `drain_completions` で反映できる
    /// ことを確認する（`Completion<S>` の `Send` 境界の実挙動確認、Issue #745）。
    #[test]
    fn completion_sender_can_send_from_another_thread() {
        let mut store = StateStore::new(Counter::default());
        let (tx, rx) = std::sync::mpsc::channel::<Completion<Counter>>();

        let handle = std::thread::spawn(move || {
            tx.send(Box::new(|state: &mut Counter| state.count += 1))
                .expect("受信側が生きているため送信は成功する");
        });
        handle.join().expect("送信スレッドは panic せず完了する");

        drain_completions(&mut store, &rx);

        assert_eq!(store.state().count, 1);
        assert_eq!(store.version(), 1);
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
        let sender = CompletionSender { tx, ctx };

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
