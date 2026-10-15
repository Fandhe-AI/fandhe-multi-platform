//! TV フォーカスナビゲーションのプラットフォーム統合レイヤ実装（TASK-17.2 / REQ-17）。
//!
//! `docs/architecture/tv-focus-navigation-architecture.md` §6.2 が定めるプラットフォーム
//! レイヤ責務（キーイベント → `FocusDirection` 変換、`FocusRing` 駆動、決定操作の接続、
//! reveal、ハイライト描画）をまとめて実装する。共通コア（`fandhe_core::ui::focus` /
//! `fandhe_core::activate_focus_at`）はフォーカス状態の解決までを担い、本モジュールは
//! それを実際の egui 描画・入力へ配線する（§6.2 の分界。共通コアへの変更は行わない）。
//!
//! **入力ブリッジの実装方式**: Android の GameActivity（`android-activity` クレート）が
//! 配信する `KeyEvent`（`KEYCODE_DPAD_UP`/`DOWN`/`LEFT`/`RIGHT`/`CENTER`）は、
//! `android.rs` が `eframe::run_native` へ委譲するブート経路の内部（`eframe` が使う
//! winit の Android バックエンド）で、既に `egui::Event::Key { key: egui::Key::Arrow* /
//! Enter, .. }` へ正規化されている（`egui-winit` が担う既存の変換経路であり、本クレートが
//! 独自の Android FFI・`unsafe` を追加する必要はない）。本モジュールはこの正規化済み
//! `egui::Event::Key` を [`read_navigation_input`] で読み、`FocusDirection`（十字キー）・
//! 決定操作（Enter/Space）へ変換する。デスクトップの物理キーボードの矢印キー・Enter も
//! 同じ `egui::Key` 経路を通るため、同一コードでローカル検証できる（タスク制約:
//! デスクトップでも同経路を駆動できるようにする）。
//!
//! **未検証事項**: `KEYCODE_DPAD_CENTER` が winit 経由で `egui::Key::Enter` へ正規化
//! されるかは実機/エミュレータでの確認が前提であり、設計文書 8 節確認事項 6
//! （GameActivity 経由のキーイベント受信）と同じ未検証事項として残る
//! （TASK-17.2 の完了条件、本レポートでも報告する）。
//!
//! **reveal（可視範囲追随、§4.6）の実装方式（能動的）**: `egui::Response::
//! scroll_to_me` のみに頼る受動的な実装は、対象ボタンがそのフレームで実際に
//! 描画された場合しか効果を持たない。フォーカス確定直後（`FocusRing` の三段探索
//! による再捕捉・先頭フォールバック、または連続移動）でフォーカス先が現在の描画窓
//! （前フレームの `VisibleRange`）の外側へ着地した場合、対象ボタンはそもそも
//! 描画されないため `scroll_to_me` が一度も呼ばれず、永久に窓外のまま復帰できない
//! （受動的実装だけでは reveal を保証できない理由）。
//!
//! [`render_virtual_list`] が `ScrollArea` を構築する**前**に、
//! `FocusEntry.path` の該当セグメント（このコンテナ内の対象 index）と、前フレームの
//! `VisibleRange`（[`TvRenderCtx::old_hints`]、[`WindowHints`] と同じ型・§4.4 の探索窓
//! ヒントを reveal 用に転用したもの）を比較し、対象が窓外なら [`compute_reveal_offset`]
//! で目的のスクロールオフセットを算出して `ScrollArea::vertical_scroll_offset` で
//! **その場で上書き**する。これにより `show_viewport` が今フレーム内で返す
//! `VisibleRange` 自体が対象を含むように前倒しで確定するため、対象ボタンが今フレームで
//! 実際に描画され、`scroll_to_me`（受動的な保険として残す）も正しく動く。§4.6 が要求する
//! 「上方向へ外れた場合は窓の先頭に、下方向へ外れた場合は窓の末尾に対象 index が入る
//! 位置へ揃える」は [`compute_reveal_offset`] の分岐（前フレーム窓との前後関係で判定）が
//! 直接実装する。すでに窓内なら上書きしない（無用なスナップを防ぐ、最小移動の一部）。
//!
//! ネストした `VirtualList`（`VirtualList` の項目がさらに `VirtualList` を返す構成）
//! でも、この算出は [`render_virtual_list`] が呼ばれるたびにそのコンテナ自身の `path`
//! だけを基準に独立して行うため、追加の分岐なしに各階層で個別に機能する。外側の
//! `render_virtual_list` 呼び出しが先に完了してから内側の項目（もう 1 段の
//! `VirtualList`）が描画されるという再帰描画の順序が、そのまま「外側→内側」の
//! 更新順を実現する（§4.6）。
//!
//! **決定操作の `StateStore` 経由化（#536）**: [`fandhe_core::activate_focus_at`]
//! は `state: &mut S` を直接要求し、[`fandhe_core::StateStore`] が `&mut S` を外部へ
//! 公開しない設計と噛み合わない。そのため本モジュールは `state: &mut S` ではなく
//! `store: &mut StateStore<S>` を受け取り、読み取りは `store.state()`、決定操作は
//! [`fandhe_core::Event::ButtonActivatedAt`]（id ベースの決定操作、`ButtonClickedAt` と
//! 対称）を `store.dispatch(screen, &Event::ButtonActivatedAt { .. })` へ通す設計に統一する
//! （`TvFocusApp`（`app.rs`）参照）。

use fandhe_core::{
    clamp_scroll_offset, compute_visible_range, grid_item_range, total_content_extent, Axis, Event,
    FocusDirection, FocusRing, Node, ReadFn, ReadIndexedFn, Screen, StateStore, VisibleRange,
    WidgetId, WindowHints,
};

use crate::recyclers::VirtualListRecyclers;

/// スクロール位置に応じた前後バッファ（overscan）行数。`lib.rs::VIRTUAL_LIST_OVERSCAN`
/// と同じ役割・同じ値を持つ（`Axis::Grid` は「行」単位のオーバースキャンになる点のみ
/// 異なるため、値を共有せず本モジュール専用の定数として持つ）。
const TV_VIRTUAL_LIST_OVERSCAN: usize = 3;

/// 1 フレーム分の描画で共有する可変状態（[`crate::RenderContext`] と同じ設計判断。
/// #536 で決定操作を `StateStore` 経由へ統一したことに伴い、マウス/タッチクリックも
/// 走査中に直接発火せず `pending_events` へ積み、走査完了後にまとめて dispatch する
/// 契約へ揃えた）。
struct TvRenderCtx<'a, S> {
    /// 現在フォーカス中のノードの path（[`fandhe_core::FocusEntry::path`]）。
    /// ハイライト表示の対象判定に使う。
    focused_path: Option<Vec<usize>>,
    /// 今フレームで新たに捕捉した `VirtualList` ごとの可視範囲
    /// （次フレームの [`FocusRing::rebuild`] へ渡す探索窓ヒント、§4.4）。
    new_hints: WindowHints,
    /// 前フレームで確定した `VirtualList` ごとの可視範囲（`self.window_hints` の
    /// 直前の値）。能動的 reveal（モジュール冒頭「reveal の実装方式」）が
    /// 「対象が既に窓内か・上方向/下方向のどちらから外れたか」を判定するために読む
    /// 読み取り専用の入力（`new_hints` と役割を分離するため別フィールドにする）。
    old_hints: &'a WindowHints,
    /// `VirtualList` 項目ノードのフレーム間再利用プール（`recyclers` モジュール doc）。
    recyclers: &'a mut VirtualListRecyclers<S>,
    /// `ListRecycler::acquire` のキャッシュ無効化キー。[`StateStore::version`] を
    /// そのまま使う（#536 で独自カウンタを廃止し `StateStore` の版管理へ統一）。
    version: u64,
    /// 走査中に発火したクリックイベント。走査完了後に [`TvFocusController::update`]
    /// がまとめて `store.dispatch` する（`lib.rs::RenderContext::pending_events` と
    /// 同じ契約。`&mut S` を走査中に公開しない）。
    pending_events: Vec<Event>,
}

/// TV フォーカスナビゲーション対応の入力処理・`FocusRing` 駆動・描画をまとめて行う
/// コントローラ（TASK-17.2）。`FocusRing`（共通コア）・探索窓ヒント
/// （[`WindowHints`]、§4.4）・`VirtualList` 項目のフレーム間再利用プール
/// （[`VirtualListRecyclers`]）をフレームをまたいで保持する（呼び出し側 =
/// `TvFocusApp`（`app.rs`）がフィールドとして持つ、既存の `VirtualListRecyclers` と
/// 同じ設計判断）。
pub struct TvFocusController<S> {
    ring: FocusRing,
    window_hints: WindowHints,
    recyclers: VirtualListRecyclers<S>,
}

impl<S> Default for TvFocusController<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> TvFocusController<S> {
    /// 空のフォーカス状態でコントローラを作る。
    pub fn new() -> Self {
        Self {
            ring: FocusRing::new(),
            window_hints: WindowHints::new(),
            recyclers: VirtualListRecyclers::new(),
        }
    }

    /// 現在フォーカス中のノードの path（テスト・デバッグ用）。
    pub fn current_path(&self) -> Option<&[usize]> {
        self.ring.current().map(|entry| entry.path.as_slice())
    }

    /// 1 フレーム分の入力処理・フォーカス解決・描画をまとめて行う
    /// （`TvFocusApp::update`（`app.rs`）から毎フレーム呼ばれる想定）。
    ///
    /// 手順（設計文書 §4.4・§5・§6.2 の接続順序）:
    /// 1. 入力ブリッジで方向キー/決定操作を読む。
    /// 2. 決定操作の**前**に [`FocusRing::rebuild`] を呼ぶ（§4.4「id による二層検証」）。
    ///    前フレーム後の状態更新・リストの挿入/並べ替えで保持中の path が stale に
    ///    なっている場合があり、rebuild を先に済ませて path を最新化してから決定操作を
    ///    解決しないと、id ガードが古い path に対して false を返し、決定入力そのものが
    ///    握りつぶされる（codex-review 5 巡目 P1 指摘の再発防止。§4.4 の自己更新型
    ///    フォーカス契約どおり、rebuild を「決定操作の前段」として毎フレーム先に通す）。
    /// 3. 決定操作があれば rebuild 後の現在フォーカスに対して
    ///    `store.dispatch(screen, &Event::ButtonActivatedAt { .. })`（§5、#536 で
    ///    `StateStore` 経由へ統一）を呼ぶ。状態変化が起きた場合は、その変化でツリー
    ///    構造が変わりうるためもう一度 rebuild して構造再検証する（下記コメント参照）。
    /// 4. 方向キー入力があれば [`FocusRing::move_focus`] を呼ぶ（§4.4「軸解決
    ///    アルゴリズム」）。決定操作と方向キーが同一フレームに来た場合は決定を先に
    ///    確定し、その結果（再 rebuild 後の最新構造）に対して移動する意図的な順序。
    /// 5. 描画（ハイライト・reveal・探索窓ヒント捕捉を兼ねる、§6.2）。マウス/タッチ
    ///    クリックは走査完了後にまとめて `store.dispatch` する（`TvRenderCtx::
    ///    pending_events` 参照。決定操作と同じ `Event::ButtonActivatedAt` 経路）。
    ///    決定操作（手順 3）と異なり dispatch 後の即時 rebuild は行わず、次フレーム
    ///    冒頭の rebuild（手順 2）に委ねる。StateStore 経由化前の実装（state_changed
    ///    フラグのみ立てて rebuild しない）から意図的に踏襲した非対称性。
    pub fn update(&mut self, ui: &mut egui::Ui, screen: &Screen<S>, store: &mut StateStore<S>) {
        self.recyclers.begin_frame();
        // 画像キャッシュの掃除。通常描画経路（lib.rs）と同じくフレーム冒頭で行い、
        // 可視範囲外になった画像の画素・GPU テクスチャを解放する。
        crate::rgba_image::begin_frame(ui.ctx());

        let (direction, decide) = read_navigation_input(ui.ctx());

        // 状態変化の有無に関わらず毎フレーム呼ぶ契約（プラットフォームレイヤは
        // 「保持して呼ぶだけ」でよい、§6.2「同一の FocusRing インスタンスを保持し、
        // 状態更新後に rebuild を呼ぶ責務」）。決定操作より前に呼ぶことで、保持中の
        // path を最新のツリーへ追随させてから id ガードを通す（上記 doc コメント参照）。
        self.ring.rebuild(screen, store.state(), &self.window_hints);

        if decide {
            if let Some(entry) = self.ring.current() {
                let (path, id) = (entry.path.clone(), entry.id);
                let handled = store.dispatch(screen, &Event::ButtonActivatedAt { id, path });
                if handled {
                    // 決定操作による状態変化はツリー構造（リストの挿入・並べ替え等）を
                    // 変えうるため、直後の move_focus/描画が古い構造を見ないよう
                    // もう一度 rebuild して構造再検証する。
                    self.ring.rebuild(screen, store.state(), &self.window_hints);
                }
            }
        }

        if let Some(direction) = direction {
            self.ring.move_focus(screen, store.state(), direction);
        }

        // `ListRecycler::acquire` のキャッシュ無効化キー（`TvRenderCtx::version` doc
        // 参照）。走査開始前の一時点で確定させ、走査中の状態変化（`pending_events` を
        // 走査完了後にまとめて dispatch するため発生しない）と混同しない
        // （`lib.rs::render_screen_with_recyclers` と同じ契約）。
        let version = store.version();

        // 前フレーム確定分の探索窓ヒントを能動的 reveal（モジュール冒頭「reveal の
        // 実装方式」）の入力として取り出す。`rebuild`（上記）はこの値を使い終えて
        // おり、以降は `ctx.old_hints` としてのみ読む（`self.window_hints` は
        // 描画完了後に `ctx.new_hints` で置き換える）。
        let old_hints = std::mem::take(&mut self.window_hints);
        let focused_path = self.ring.current().map(|entry| entry.path.clone());
        let mut ctx = TvRenderCtx {
            focused_path,
            new_hints: WindowHints::new(),
            old_hints: &old_hints,
            recyclers: &mut self.recyclers,
            version,
            pending_events: Vec::new(),
        };

        ui.heading(screen.title());
        ui.separator();
        let state = store.state();
        for (index, node) in screen.iter().enumerate() {
            render_node(ui, node, state, &[index], &mut ctx);
        }

        self.window_hints = ctx.new_hints;
        for event in ctx.pending_events {
            store.dispatch(screen, &event);
        }
    }
}

/// 方向キー/D-pad・決定キーの入力を読む（モジュール冒頭「入力ブリッジの実装方式」
/// 参照）。矢印キー 4 方向のうち複数が同一フレームに来た場合は最後に処理した方向を
/// 採用する（TV リモコンの D-pad は同時押しを想定しないため十分な近似）。
fn read_navigation_input(ctx: &egui::Context) -> (Option<FocusDirection>, bool) {
    let mut direction = None;
    let mut decide = false;
    ctx.input(|input| {
        for event in &input.events {
            let egui::Event::Key {
                key, pressed: true, ..
            } = event
            else {
                continue;
            };
            match key {
                egui::Key::ArrowUp => direction = Some(FocusDirection::Up),
                egui::Key::ArrowDown => direction = Some(FocusDirection::Down),
                egui::Key::ArrowLeft => direction = Some(FocusDirection::Left),
                egui::Key::ArrowRight => direction = Some(FocusDirection::Right),
                egui::Key::Enter | egui::Key::Space => decide = true,
                _ => {}
            }
        }
    });
    (direction, decide)
}

/// [`TvFocusController::update`] の再帰描画本体。`lib.rs::render_node` と同じ
/// 「事前順走査・`List` 自身は行を描かない」契約、および「走査完了後にまとめて
/// `store.dispatch` する」契約（`ctx.pending_events`、`lib.rs::RenderContext` と
/// 同じ設計、#536 で `StateStore` 経由へ統一）を踏襲する。
fn render_node<S>(
    ui: &mut egui::Ui,
    node: &Node<S>,
    state: &S,
    path: &[usize],
    ctx: &mut TvRenderCtx<S>,
) {
    match node {
        Node::Text(text) => {
            ui.label(text);
        }
        Node::DynamicText { get } => {
            // `Node::Text` と同じ非フォーカス可能なラベル描画（Issue #581）。
            ui.label(get(state));
        }
        Node::StyledText { role, get } => {
            // `DynamicText` と同じ非フォーカス可能なラベル描画に、`text_role.rs`
            // による色解決（#813 で解決済み）を重ねる。
            crate::text_role::styled_label(ui, *role, get(state));
        }
        Node::Button { label, id, .. } => {
            render_focusable_button(ui, label, *id, path, ctx);
        }
        Node::IndexedButton { label, id, .. } => {
            render_focusable_button(ui, label, *id, path, ctx);
        }
        Node::List { expand, axis } => {
            let children = expand(state);
            let mut child_path = path.to_vec();
            child_path.push(0);
            if matches!(axis, Axis::Horizontal) {
                ui.horizontal(|ui| {
                    for (i, child) in children.iter().enumerate() {
                        if let Some(last) = child_path.last_mut() {
                            *last = i;
                        }
                        render_node(ui, child, state, &child_path, ctx);
                    }
                });
            } else {
                for (i, child) in children.iter().enumerate() {
                    if let Some(last) = child_path.last_mut() {
                        *last = i;
                    }
                    render_node(ui, child, state, &child_path, ctx);
                }
            }
        }
        Node::VirtualList {
            len,
            item,
            item_extent,
            axis,
        } => {
            render_virtual_list(ui, path, *len, *item, *item_extent, *axis, state, ctx);
        }
        Node::RgbaImage { pixels, alt } => {
            // フォーカス対象にしない非対話の表示（通常描画と同じ実装を使う）。
            crate::rgba_image::show(ui, pixels, alt, path);
        }
        Node::TextField { .. } | Node::Widget(_) => {
            // TV サンプル画面（`tv_sample.rs`）は Button/IndexedButton/List/
            // VirtualList/Text のみで構成する（設計文書 §3.1 の v1 スコープ、
            // `TextField`/`Node::Widget` はいずれもフォーカス可能ノードの定義から
            // 明示的に除外されている、§3.1「v1 スコープから除外する対象と理由」）。
        }
    }
}

/// フォーカス可能な葉ノード（`Button`/`IndexedButton`）1 件を描画する共通処理。
/// ハイライト装飾（§6.2「加算的な装飾」、`widget.rs` の既存方針を踏襲）・reveal
/// （§4.6、モジュール冒頭参照）・マウス/タッチクリックの受付をまとめて担う。
///
/// クリックは `id`/`path` から組み立てた [`fandhe_core::Event::ButtonActivatedAt`] を
/// `ctx.pending_events` へ積むだけに留め、即時発火はしない（#536 で決定操作を
/// `StateStore::dispatch` へ統一したことに伴い、キーボード決定操作と同じ経路へ揃えた。
/// 実際の dispatch は [`TvFocusController::update`] が走査完了後にまとめて行う）。
fn render_focusable_button<S>(
    ui: &mut egui::Ui,
    label: &str,
    id: WidgetId,
    path: &[usize],
    ctx: &mut TvRenderCtx<S>,
) {
    let is_focused = ctx.focused_path.as_deref() == Some(path);

    let mut button = egui::Button::new(label);
    if is_focused {
        button = button.stroke(egui::Stroke::new(3.0, egui::Color32::YELLOW));
    }
    let response = ui.add(button);

    if is_focused {
        // reveal（§4.6）: 対象が可視域外なら囲む `ScrollArea` を最小移動でスクロールする
        // （モジュール冒頭「reveal の実装方式」参照。egui 標準機能に委譲する）。
        response.scroll_to_me(None);
    }

    if response.clicked() {
        ctx.pending_events.push(Event::ButtonActivatedAt {
            id,
            path: path.to_vec(),
        });
    }
}

/// `Node::VirtualList` を描画する（`lib.rs::render_virtual_list` と同じ
/// `ScrollArea::show_viewport` + `compute_visible_range` の組み合わせ契約を踏襲）。
///
/// `Axis::Grid { columns }` は `columns` 列の縦スクロールグリッド、`Axis::Vertical` は
/// 1 列の縦スクロールリストとして描画する（[`render_virtual_list_vertical_or_grid`]）。
/// `Axis::Horizontal` は `FocusRing`（`fandhe_core::ui::focus`）が Left/Right で移動を
/// 実装している配置軸と表示・操作を一致させるため、項目を横一列に並べ
/// `ScrollArea::horizontal` で横スクロールする専用経路（[`render_virtual_list_horizontal`]）
/// へ分岐する（codex-review 4 巡目 P1 指摘: 宣言した配置軸と実際の表示・操作が
/// 食い違っていた欠陥の修正）。可視範囲計算はいずれの軸も [`compute_visible_range`] /
/// [`total_content_extent`] / [`clamp_scroll_offset`]（軸非依存の純関数）を再利用し、
/// 求めた項目 index 範囲を設計文書 §4.4「探索窓ヒント」どおり [`TvRenderCtx::new_hints`]
/// へ記録する契約は両経路で共通にする。
#[allow(clippy::too_many_arguments)]
fn render_virtual_list<S>(
    ui: &mut egui::Ui,
    path: &[usize],
    len: ReadFn<S, usize>,
    item: ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    axis: Axis,
    state: &S,
    ctx: &mut TvRenderCtx<S>,
) {
    let total_len = len(state);
    let item_extent = if item_extent.is_finite() && item_extent > 0.0 {
        item_extent
    } else {
        0.0
    };

    if matches!(axis, Axis::Horizontal) {
        render_virtual_list_horizontal(ui, path, total_len, item, item_extent, state, ctx);
        return;
    }

    render_virtual_list_vertical_or_grid(ui, path, total_len, item, item_extent, axis, state, ctx);
}

/// `Axis::Vertical`/`Axis::Grid { columns }` の描画経路（既存実装、[`render_virtual_list`]
/// のドキュメント参照）。
#[allow(clippy::too_many_arguments)]
fn render_virtual_list_vertical_or_grid<S>(
    ui: &mut egui::Ui,
    path: &[usize],
    total_len: usize,
    item: ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    axis: Axis,
    state: &S,
    ctx: &mut TvRenderCtx<S>,
) {
    let columns = match axis {
        Axis::Grid { columns } => columns.max(1),
        _ => 1,
    };
    let rows = total_len.div_ceil(columns.max(1));

    // 能動的 reveal（モジュール冒頭「reveal の実装方式」§4.6）: このコンテナがフォーカス
    // 中ノードの直近の親（`path` の 1 段先が対象 index）である場合のみ、`ScrollArea` を
    // 構築する前にオフセットの上書きを検討する。対象が居ないコンテナ・既に前フレームの
    // 可視範囲内のコンテナは何もしない（無用なスナップを防ぐ）。
    // フォーカス先がこのコンテナの直下（1 段下）とは限らない。ネストした
    // `VirtualList` の内側の葉、`VirtualList` → `List` → button 等、より深い子孫に
    // ある場合でも、このコンテナ自身の窓は「path 上の全 `VirtualList` 祖先を
    // 外側→内側順に更新する」契約（§4.6）どおり reveal を発動させる必要がある
    // （Cursor Bugbot High 指摘: 厳密一致 `== path.len() + 1` だと 2 段以上深い
    // フォーカスで外側コンテナの reveal が発動せず窓外着地に詰む）。そのため
    // prefix 一致（`focused.len() > path.len()`）へ緩め、`focused[path.len()]`
    // （このコンテナ内での子 index）を使う。
    let focus_index_here = ctx.focused_path.as_ref().and_then(|focused| {
        if focused.len() > path.len() && focused[..path.len()] == *path {
            Some(focused[path.len()])
        } else {
            None
        }
    });
    // `ScrollArea` 構築前は実際のビューポート高さを知り得ないため、直前に確保できる
    // 利用可能高さで近似する（`ScrollArea` はこの直後にこの高さをほぼそのまま消費する、
    // `auto_shrink([false, true])` の意味論）。下端揃えの目標算出にのみ使う近似値であり、
    // 多少の誤差があっても `compute_visible_range` の overscan が吸収するため対象が
    // 窓外になることはない。
    let viewport_height_estimate = ui.available_height();
    let forced_offset = focus_index_here.and_then(|focus_index| {
        compute_reveal_offset(
            focus_index,
            columns,
            item_extent,
            ctx.old_hints.get(path),
            viewport_height_estimate,
        )
    });

    let mut scroll_area = egui::ScrollArea::vertical()
        .id_salt(("fandhe-tv-virtual-list", path.to_vec()))
        .auto_shrink([false, true]);
    if let Some(offset) = forced_offset {
        // `ScrollArea::vertical_scroll_offset` は今フレームの `state.offset` を
        // 上書きしてから `viewport`（`show_viewport` へ渡す値）を算出するため、直後の
        // `item_range` 計算に反映される（egui 0.33.3 `scroll_area.rs` の実装、
        // モジュール冒頭「reveal の実装方式」参照）。
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    }

    scroll_area.show_viewport(ui, |ui, viewport| {
        let total_extent = total_content_extent(rows, item_extent);
        let offset = clamp_scroll_offset(viewport.min.y, total_extent, viewport.height());
        let row_range = compute_visible_range(
            viewport.height(),
            offset,
            item_extent,
            rows,
            TV_VIRTUAL_LIST_OVERSCAN,
        );
        // `usize` overflow を避けるため、共通実装（`fandhe_core::grid_item_range` の
        // doc 参照、Issue #557 で非 TV 側と共通化）へ委譲する。
        let item_range = grid_item_range(&row_range, columns, total_len);
        ctx.new_hints.insert(path.to_vec(), item_range.clone());

        ui.spacing_mut().item_spacing.y = 0.0;
        ui.set_height(total_extent);
        ui.add_space(row_range.start as f32 * item_extent);

        let mut recycler = ctx.recyclers.take(path);
        let nodes = recycler.acquire(item_range.clone(), ctx.version, state, item);
        let mut nodes_iter = nodes.into_iter().zip(item_range.clone());

        let mut child_path = path.to_vec();
        child_path.push(0);
        for _row in row_range.clone() {
            let (row_rect, _response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), item_extent),
                egui::Sense::hover(),
            );
            // 列は `row_rect.width() / columns` の均等セル幅で固定する（Issue #558）。
            // 行 `Ui` 1 つへ自然幅のまま `left_to_right` で積むと、前の列の内容幅で
            // 後続列の x が決まり、内容が長い項目のある行だけ列がずれて隣セルへ重なる。
            // 非 TV 側 `render_virtual_list_grid`（`lib.rs`、#551）と同じく、セル矩形を
            // 自前で確保し `new_child` + `set_clip_rect` で幅を固定・クリップする。
            // `columns == 1`（`Axis::Vertical`）でもセル幅 = 行幅となり、単純な行 `Ui`
            // と同じ配置になる（横はみ出しのクリップのみが追加される）。
            let cell_width = row_rect.width() / columns as f32;
            for col in 0..columns {
                let Some((child, index)) = nodes_iter.next() else {
                    break;
                };
                if let Some(last) = child_path.last_mut() {
                    *last = index;
                }
                let cell_rect = egui::Rect::from_min_size(
                    row_rect.min + egui::vec2(col as f32 * cell_width, 0.0),
                    egui::vec2(cell_width, item_extent),
                );
                let mut cell_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(cell_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Min)),
                );
                // `new_child` の `max_rect` はレイアウト制約のヒントであり描画・
                // hit-testing をクリップしないため、セル幅を超える内容が隣列へ視覚的・
                // 操作的にはみ出さないよう明示的にクリップする（同ファイルの
                // `render_virtual_list_horizontal` と同じ理由）。
                cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                cell_ui.push_id(("fandhe-tv-item", index), |ui| {
                    render_node(ui, child, state, &child_path, ctx);
                });
            }
        }

        ctx.recyclers.restore(path, recycler);

        let trailing_rows = rows.saturating_sub(row_range.end);
        ui.add_space(trailing_rows as f32 * item_extent);
    });
}

/// `Axis::Horizontal` の描画経路（[`render_virtual_list`] のドキュメント参照）。
/// [`render_virtual_list_vertical_or_grid`] の縦版と対を成す構造で、`columns` 概念が
/// 無い代わりに項目を 1 行 N 列（横一列）に並べる。可視範囲計算は
/// [`compute_visible_range`]/[`total_content_extent`]/[`clamp_scroll_offset`] を
/// `extent = item_extent`（項目幅）としてそのまま再利用し（軸非依存の純関数）、
/// `columns = 1` として [`compute_reveal_offset`] を呼ぶ（`focus_row` = `focus_index`
/// と一致するため縦版と同じ算出式で成立する）。
///
/// 戻り値は項目を並べた横一列（`ui.horizontal`）の実際の消費幅（`Response::rect.width()`）。
/// 呼び出し元（[`render_virtual_list`]）は通常無視するが、単体テストが「幅が
/// `item_extent` 単位で厳密に消費されているか」（Cursor Bugbot 指摘 Medium）を
/// `egui::Ui` 抜きに検証できるよう公開する。
fn render_virtual_list_horizontal<S>(
    ui: &mut egui::Ui,
    path: &[usize],
    total_len: usize,
    item: ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    state: &S,
    ctx: &mut TvRenderCtx<S>,
) -> f32 {
    // 能動的 reveal（縦版と同じ契約、§4.6）。`viewport_height_estimate` に相当する値は
    // 横方向では `ui.available_width()`（横スクロールの可視幅の近似）になる。
    let focus_index_here = ctx.focused_path.as_ref().and_then(|focused| {
        if focused.len() > path.len() && focused[..path.len()] == *path {
            Some(focused[path.len()])
        } else {
            None
        }
    });
    let viewport_width_estimate = ui.available_width();
    let forced_offset = focus_index_here.and_then(|focus_index| {
        compute_reveal_offset(
            focus_index,
            1,
            item_extent,
            ctx.old_hints.get(path),
            viewport_width_estimate,
        )
    });

    // 縦版（`auto_shrink([false, true])`、上のコメント参照）と対称なポリシー:
    // 主軸（横スクロールの幅 = index 0）は false（縦版の交差軸=幅と同じ「利用可能幅を
    // 埋める」）で shelf の横幅いっぱいに広がり、交差軸（高さ = index 1）は true
    // （縦版の主軸=高さと同じ「内容に合わせて縮む」）で 1 行分の高さに収まる。
    // Cursor Bugbot 指摘（Medium）: 逆の `[true, false]` だと交差軸（高さ）が
    // `ui.available_height()` を埋めてしまい、横リストを縦に積む shelf 構成
    // （TV 定番レイアウト）で親パネルの全高を専有してしまっていた。
    let mut scroll_area = egui::ScrollArea::horizontal()
        .id_salt(("fandhe-tv-virtual-list", path.to_vec()))
        .auto_shrink([false, true]);
    if let Some(offset) = forced_offset {
        // 縦版の `vertical_scroll_offset` と同じ理由で、`show_viewport` へ渡る
        // `viewport`（今フレームの `item_range` 計算の元）に反映させる。
        scroll_area = scroll_area.horizontal_scroll_offset(offset);
    }

    let output = scroll_area.show_viewport(ui, |ui, viewport| {
        let total_extent = total_content_extent(total_len, item_extent);
        let offset = clamp_scroll_offset(viewport.min.x, total_extent, viewport.width());
        let item_range = compute_visible_range(
            viewport.width(),
            offset,
            item_extent,
            total_len,
            TV_VIRTUAL_LIST_OVERSCAN,
        );
        ctx.new_hints.insert(path.to_vec(), item_range.clone());

        let mut recycler = ctx.recyclers.take(path);
        let nodes = recycler.acquire(item_range.clone(), ctx.version, state, item);
        let nodes_iter = nodes.into_iter().zip(item_range.clone());

        let mut child_path = path.to_vec();
        child_path.push(0);
        let row_response = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.set_width(total_extent);
            ui.add_space(item_range.start as f32 * item_extent);

            for (child, index) in nodes_iter {
                if let Some(last) = child_path.last_mut() {
                    *last = index;
                }
                // 幅は仮想化の前提（`item_extent` = 列幅。スペーサー・
                // `compute_visible_range`・reveal オフセットの計算単位）どおり厳密固定し、
                // 高さのみ内容依存にする（縦版の行 = 主軸を固定・交差軸を内容依存にする
                // 設計と対称）。`ui.allocate_ui_with_layout` は desired_size を
                // 「ヒント」としてしか使わず、内容がそれを超えると実際に使った幅
                // （`min_rect`）をそのまま親のカーソル前進量に使ってしまう
                // （egui 0.33 `Ui::allocate_ui` doc「If the contents overflow, more
                // space will be allocated」）。そのため `item_extent` より広い内容
                // （長いラベルのボタン等）があると、この項目だけでなく後続の
                // スペーサー・仮想化計算が実レイアウトとずれる
                // （Cursor Bugbot 指摘 Medium）。回避のため子 `Ui` を `new_child` で
                // 自前に作り、実際の使用幅を無視して `advance_cursor_after_rect` で
                // 常に `item_extent` 幅として親のカーソルを進める（高さのみ子 `Ui` の
                // 実測値 `min_rect().height()` を使い、内容依存を維持する）。
                let cell_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(item_extent, ui.available_height().max(0.0)),
                );
                let mut cell_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(cell_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Min)),
                );
                // `new_child` の `max_rect` はレイアウト制約のヒントであり描画・
                // hit-testing をクリップしない。`item_extent` を超える内容が隣セルへ
                // 視覚的・操作的にはみ出さないよう、セル矩形（親のクリップとの交差）で
                // 明示的にクリップする（Cursor Bugbot 指摘 Medium）。
                cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                cell_ui.push_id(("fandhe-tv-item", index), |ui| {
                    render_node(ui, child, state, &child_path, ctx);
                });
                ui.advance_cursor_after_rect(egui::Rect::from_min_size(
                    cell_rect.min,
                    egui::vec2(item_extent, cell_ui.min_rect().height()),
                ));
            }

            let trailing = total_len.saturating_sub(item_range.end);
            ui.add_space(trailing as f32 * item_extent);
        });

        ctx.recyclers.restore(path, recycler);
        row_response.response.rect.width()
    });
    output.inner
}

/// 能動的 reveal（§4.6、モジュール冒頭「reveal の実装方式」参照）のオフセット算出
/// （レンダラ非依存の純関数として切り出し、`egui::Ui` なしで単体テストできるようにする）。
///
/// `focus_index` が `old_range`（このコンテナの前フレームの可視 item index 範囲、
/// [`TvRenderCtx::old_hints`]）に含まれる場合は `None`（オフセット変更不要。無用な
/// スナップを防ぐ、最小移動の一部）。含まれない場合、`old_range` との位置関係から
/// 「上方向へ外れたか・下方向へ外れたか」を判定し、上方向なら対象行を窓の先頭に、
/// 下方向（または `old_range` が `None` の初回描画）なら窓の末尾に対象 index が入る
/// 位置へ揃えたオフセットを返す（§4.6「上方向へフォーカスが外れた場合は窓の先頭に、
/// 下方向へ外れた場合は窓の末尾に対象 index が入る位置へ揃える」）。
///
/// `old_range` が `None`（初回描画、または直前フレームでこのコンテナ自体が描画されな
/// かった場合）は下端揃えを既定にする。対象が先頭付近なら結果のオフセットは
/// `ScrollArea` 側のクランプで自然に 0 へ丸められるため実害はない。
///
/// スクロール主軸に依存しない純関数であり、縦・グリッド
/// （[`render_virtual_list_vertical_or_grid`]）と横
/// （[`render_virtual_list_horizontal`]、`columns = 1` で行 = 項目 index）の両方から
/// 呼ばれる。本文の「上/下」「行」は主軸に沿った前後方向・スクロール単位と読み替える。
fn compute_reveal_offset(
    focus_index: usize,
    columns: usize,
    item_extent: f32,
    old_range: Option<&VisibleRange>,
    viewport_height_estimate: f32,
) -> Option<f32> {
    if let Some(range) = old_range {
        if range.contains(&focus_index) {
            return None;
        }
    }
    let columns = columns.max(1);
    let focus_row = focus_index / columns;
    let approaching_from_above = old_range
        .map(|range| focus_index < range.start)
        .unwrap_or(false);
    let offset = if approaching_from_above {
        focus_row as f32 * item_extent
    } else {
        ((focus_row + 1) as f32 * item_extent - viewport_height_estimate).max(0.0)
    };
    Some(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::WidgetId;

    #[derive(Default)]
    struct CounterState {
        count: u32,
    }

    fn increment(state: &mut CounterState) {
        state.count += 1;
    }

    fn tree() -> Screen<CounterState> {
        Screen::new("test").button(WidgetId::from_raw(1), "increment", increment)
    }

    /// `TvFocusController::new` は初期状態でフォーカスを持たない。
    #[test]
    fn new_controller_has_no_focus() {
        let controller: TvFocusController<CounterState> = TvFocusController::new();
        assert!(controller.current_path().is_none());
    }

    /// `read_navigation_input` は `egui::Context` を持たない単体テストが書きにくいため、
    /// 入力なしの `egui::Context::input(|_| {})` 相当の空イベント列でも panic せず
    /// `(None, false)` を返すことのみを最小確認する（実際のキー変換は結合テスト
    /// （エミュレータ実機検証、TASK-17.2）でカバーする）。
    #[test]
    fn read_navigation_input_returns_none_without_events() {
        let ctx = egui::Context::default();
        let (direction, decide) = read_navigation_input(&ctx);
        assert!(direction.is_none());
        assert!(!decide);
    }

    /// `render_focusable_button` がクリックで積む `Event::ButtonActivatedAt`
    /// （#536、`ctx.pending_events`）を `store.dispatch` すると、実際に状態が
    /// 更新されることを確認する（決定操作と同じ経路へ統一したクリック処理の検証。
    /// egui のクリック応答自体の発火判定は egui_kittest 相当の統合検証が必要なため
    /// ホスト側の単体テストの範囲外。ここでは `pending_events → dispatch` の
    /// 配線が正しく機能することのみを検証する）。
    #[test]
    fn pending_activate_event_updates_state_via_store_dispatch() {
        let screen = tree();
        let mut store = StateStore::new(CounterState::default());

        let handled = store.dispatch(
            &screen,
            &Event::ButtonActivatedAt {
                id: WidgetId::from_raw(1),
                path: vec![0],
            },
        );

        assert!(handled);
        assert_eq!(store.state().count, 1);
    }

    /// TV 経路でも `update` 冒頭で画像キャッシュが掃除され、描画されなくなった
    /// 画像の画素・テクスチャが解放される（codex P1 の再発防止）。
    #[test]
    fn update_releases_image_cache_for_images_no_longer_drawn() {
        let pixels = fandhe_core::RgbaPixels::new(1, 1, vec![255u8; 4]).expect("有効な画素");
        let with_image: Screen<CounterState> = Screen::new("img").rgba_image(pixels, "a");
        let without_image = tree();
        let ctx = egui::Context::default();
        let mut controller: TvFocusController<CounterState> = TvFocusController::new();
        let mut store = StateStore::new(CounterState::default());
        let mut pass = |screen: &Screen<CounterState>| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    controller.update(ui, screen, &mut store);
                });
            });
        };
        pass(&with_image);
        assert_eq!(crate::rgba_image::cached_len(&ctx), 1);
        // 画像のない画面へ切り替えて数パス回すと解放される。
        pass(&without_image);
        pass(&without_image);
        pass(&without_image);
        assert_eq!(crate::rgba_image::cached_len(&ctx), 0);
    }

    /// `tree()` ヘルパーが単純なボタン 1 個の画面を構築できることの前提確認
    /// （`Screen<CounterState>` が本モジュールのジェネリック関数群と型検査上
    /// 整合することの確認を兼ねる）。
    #[test]
    fn tree_builds_single_button_screen() {
        let screen = tree();
        assert_eq!(screen.len(), 1);
    }

    // ------------------------------------------------------------------
    // `compute_reveal_offset`（能動的 reveal、reviewer 指摘 high の修正）のテスト。
    // ------------------------------------------------------------------

    /// 対象が前フレームの可視範囲内なら `None`（オフセット変更不要）を返す。
    #[test]
    fn compute_reveal_offset_returns_none_when_already_visible() {
        let old_range: VisibleRange = 5..15;
        assert!(compute_reveal_offset(10, 1, 20.0, Some(&old_range), 200.0).is_none());
    }

    /// reviewer 指摘 high の核心: 前フレームの可視範囲外（下方向）へフォーカスが
    /// 確定した場合、[`compute_reveal_offset`] が返すオフセットで
    /// [`compute_visible_range`] を再計算すると、次フレームの `item_range` が
    /// 対象 index を実際に含むことを確認する（「窓外へ着地すると永久に復帰できない」
    /// 欠陥の再発防止）。
    #[test]
    fn compute_reveal_offset_targets_row_when_outside_old_window_below() {
        let old_range: VisibleRange = 0..10;
        let offset = compute_reveal_offset(100, 1, 20.0, Some(&old_range), 200.0)
            .expect("窓外なので Some を返すはず");

        let revealed = compute_visible_range(200.0, offset, 20.0, 1000, TV_VIRTUAL_LIST_OVERSCAN);
        assert!(
            revealed.contains(&100),
            "reveal 後の可視範囲に focus index が含まれるはず: {revealed:?}"
        );
    }

    /// 上方向（前フレームの窓より手前）へフォーカスが外れた場合も、同様に次フレームの
    /// `item_range` が対象を含むことを確認する。
    #[test]
    fn compute_reveal_offset_targets_row_when_outside_old_window_above() {
        let old_range: VisibleRange = 90..100;
        let offset = compute_reveal_offset(10, 1, 20.0, Some(&old_range), 200.0)
            .expect("窓外なので Some を返すはず");

        let revealed = compute_visible_range(200.0, offset, 20.0, 1000, TV_VIRTUAL_LIST_OVERSCAN);
        assert!(
            revealed.contains(&10),
            "reveal 後の可視範囲に focus index が含まれるはず: {revealed:?}"
        );
    }

    /// 前フレーム情報が無い（初回描画・直前フレームでこのコンテナが描画されなかった）
    /// 場合でも、返したオフセットで対象が可視になることを確認する
    /// （`FocusRing` の三段探索による再捕捉・先頭フォールバックが最初から窓外へ
    /// 着地するケースの検証）。
    #[test]
    fn compute_reveal_offset_targets_row_when_no_old_window() {
        let offset = compute_reveal_offset(500, 1, 20.0, None, 200.0)
            .expect("前フレーム情報が無くても Some を返すはず");

        let revealed = compute_visible_range(200.0, offset, 20.0, 1000, TV_VIRTUAL_LIST_OVERSCAN);
        assert!(
            revealed.contains(&500),
            "reveal 後の可視範囲に focus index が含まれるはず: {revealed:?}"
        );
    }

    /// `Axis::Grid` の `columns` を考慮しても対象行が正しく可視になることを確認する
    /// （行 = index / columns、モジュール冒頭の算出式参照）。
    #[test]
    fn compute_reveal_offset_accounts_for_grid_columns() {
        let old_range: VisibleRange = 0..9; // 3 列 × 3 行分。
                                            // index=50 は columns=5 のとき行 10（0 始まり）に相当する。
        let offset = compute_reveal_offset(50, 5, 20.0, Some(&old_range), 100.0)
            .expect("窓外なので Some を返すはず");

        let rows = 1000usize.div_ceil(5);
        let revealed_rows =
            compute_visible_range(100.0, offset, 20.0, rows, TV_VIRTUAL_LIST_OVERSCAN);
        let revealed_row = 50 / 5;
        assert!(
            revealed_rows.contains(&revealed_row),
            "reveal 後の可視「行」範囲に対象行が含まれるはず: {revealed_rows:?}"
        );
    }

    // ------------------------------------------------------------------
    // ネストした `VirtualList`（reviewer 指摘 medium）での多段 reveal の結合テスト。
    // ------------------------------------------------------------------

    /// `tv_sample_screen()` のネストしたリスト section（`VirtualList` の項目が
    /// さらに `VirtualList` を返す 1 段ネスト構造）の奥深く（既定の可視ウィンドウ外）
    /// までフォーカスを移動し、`TvFocusController::update` を 1 フレーム呼ぶだけで
    /// 外側・内側**両方**のコンテナの `window_hints`（= 今フレームの `item_range`）が
    /// 対象を含むように更新されることを確認する（reviewer 指摘 medium: high の修正が
    /// ネストでも成立することの検証）。
    ///
    /// `TvFocusController::ring`/`window_hints` は本モジュールの子である `tests` から
    /// 直接アクセスできる（Rust のプライバシー規則: 親モジュールの private フィールドは
    /// 子孫モジュールから参照できる）ため、`FocusRing::move_focus` を直接呼んで
    /// フォーカスを深部へ進める（実際の入力シミュレーションを要する `egui_kittest` の
    /// wgpu 依存を避け、`egui::Context::run`（レンダラ不要）のみで検証する）。
    #[test]
    fn nested_virtual_list_reveal_updates_both_levels_after_moving_focus_deep() {
        use crate::tv_sample::{tv_sample_screen, TvState};

        let screen = tv_sample_screen();
        let mut store = StateStore::new(TvState::default());
        let mut controller: TvFocusController<TvState> = TvFocusController::new();

        // 初回 rebuild でツリー先頭（棚 1 の先頭ボタン）へフォーカスする。
        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, store.state(), &hints);

        // ネストしたリスト section（3 セグメントの path: 外側コンテナ index・
        // グループ index・内側 index）へ到達するまで Down を押し続ける
        // （画面構成の変更に左右されない探索。棚 2 段を抜けた直後にネスト構造の
        // 先頭項目へ descend するはず）。
        let mut reached_nested = false;
        for _ in 0..20 {
            controller
                .ring
                .move_focus(&screen, store.state(), FocusDirection::Down);
            if controller.ring.current().map(|e| e.path.len()) == Some(3) {
                reached_nested = true;
                break;
            }
        }
        assert!(
            reached_nested,
            "ネスト構造（3 セグメント path）へ到達できなかった"
        );

        // 内側リストの奥（既定の可視ウィンドウ外）まで移動する。
        for _ in 0..25 {
            controller
                .ring
                .move_focus(&screen, store.state(), FocusDirection::Down);
        }
        let leaf_path = controller
            .ring
            .current()
            .expect("フォーカスが存在するはず")
            .path
            .clone();
        assert_eq!(
            leaf_path.len(),
            3,
            "ネスト葉ノードの path は 3 セグメントのはず"
        );

        let outer_container_path = leaf_path[..1].to_vec();
        let inner_container_path = leaf_path[..2].to_vec();

        // 実際の描画パス（`update`）を 1 フレーム分呼ぶ。矢印キー入力は与えないため
        // 現在の `ring.current`（上で直接進めた深部の葉ノード）はそのまま維持される。
        let egui_ctx = egui::Context::default();
        let _ = egui_ctx.run(egui::RawInput::default(), |ectx| {
            egui::CentralPanel::default().show(ectx, |ui| {
                controller.update(ui, &screen, &mut store);
            });
        });

        let outer_range = controller
            .window_hints
            .get(&outer_container_path)
            .unwrap_or_else(|| {
                panic!("外側コンテナ（{outer_container_path:?}）の window_hints が無い")
            });
        assert!(
            outer_range.contains(&leaf_path[1]),
            "外側コンテナの可視範囲にグループ index が含まれるはず: {outer_range:?}"
        );

        let inner_range = controller
            .window_hints
            .get(&inner_container_path)
            .unwrap_or_else(|| {
                panic!("内側コンテナ（{inner_container_path:?}）の window_hints が無い")
            });
        assert!(
            inner_range.contains(&leaf_path[2]),
            "内側コンテナの可視範囲に葉ノード index が含まれるはず（多段 reveal の検証）: {inner_range:?}"
        );
    }

    // ------------------------------------------------------------------
    // Cursor Bugbot High 指摘: `focus_index_here` の厳密一致
    // （`focused.len() == path.len() + 1`）だと、フォーカスが外側コンテナの
    // ちょうど 1 段下ではなくより深い子孫にある場合に外側コンテナの reveal が
    // 発動しない。上記 `nested_virtual_list_reveal_...` が検出できなかった理由:
    // `tv_sample_screen()` のネストしたリスト section は外側グループ数が
    // `NESTED_GROUP_COUNT = 3` のみで、`compute_visible_range` の overscan（3）
    // だけで全 3 グループが常に可視範囲に収まってしまい、外側コンテナの
    // forced_offset が実際には不要だった（受動的な既定窓のままでもテストの
    // assertion が偶然通っていた）。以下のテストは外側コンテナの項目数を
    // 十分大きくし、既定の描画窓（overscan 込み）では確実に対象を含まない
    // 位置までフォーカスを進めることで、この見落としを再現する。
    // ------------------------------------------------------------------

    /// 上記 2 テスト共通の最小状態（全項目が同一の関数ポインタを共有するため、
    /// `WidgetId` の全域一意性はこのテストの検証対象外＝不要）。
    #[derive(Default)]
    struct DeepNestState;

    /// 外側コンテナの総項目数。既定のビューポート（`egui::Context::run` に
    /// `screen_rect` を指定しない場合の egui 既定サイズ）+ overscan（3）でも
    /// 到底収まらない件数にすることで、「既定窓に偶然含まれてテストが見落とす」
    /// （上記コメント参照）を再発させない。
    const DEEP_OUTER_LEN: usize = 200;

    fn deep_outer_len(_state: &DeepNestState) -> usize {
        DEEP_OUTER_LEN
    }

    fn deep_nest_no_op(_state: &mut DeepNestState, _index: usize) {}

    // --- ケース 1: VirtualList → VirtualList（内側リストの葉、3 セグメント path） ---

    const DEEP_INNER_LEN: usize = 5;

    fn deep_inner_len(_state: &DeepNestState) -> usize {
        DEEP_INNER_LEN
    }

    fn deep_inner_item(_state: &DeepNestState, index: usize) -> Node<DeepNestState> {
        Node::IndexedButton {
            label: format!("inner-{index}"),
            index,
            on_click: deep_nest_no_op,
            id: WidgetId::from_raw(index as u64),
        }
    }

    /// 外側 `VirtualList` の各項目がさらに `VirtualList`（内側、[`DEEP_INNER_LEN`] 件）を
    /// 返す 1 段ネスト構造（全グループが同一の内側リストを共有する最小版）。
    fn deep_outer_nested_virtual_list_item(
        _state: &DeepNestState,
        _group_index: usize,
    ) -> Node<DeepNestState> {
        Node::VirtualList {
            len: deep_inner_len,
            item: deep_inner_item,
            item_extent: 20.0,
            axis: Axis::Vertical,
        }
    }

    fn deep_nested_virtual_list_tree() -> Screen<DeepNestState> {
        Screen::new("deep-nested-virtual-list").virtual_list_with_axis(
            deep_outer_len,
            deep_outer_nested_virtual_list_item,
            20.0,
            Axis::Vertical,
        )
    }

    // --- ケース 2: VirtualList → List → button（2 段深い葉、3 セグメント path） ---

    fn deep_single_button_list(_state: &DeepNestState) -> Vec<Node<DeepNestState>> {
        vec![Node::IndexedButton {
            label: "leaf".to_string(),
            index: 0,
            on_click: deep_nest_no_op,
            id: WidgetId::from_raw(0),
        }]
    }

    /// 外側 `VirtualList` の各項目が `Node::List`（非仮想化、1 ボタンのみ）を返す構造。
    /// 葉ノードの path は外側コンテナ自身の path から 2 セグメント深い
    /// （`VirtualList` の item index + `List` の展開 index）。
    fn deep_outer_list_item(_state: &DeepNestState, _group_index: usize) -> Node<DeepNestState> {
        Node::List {
            expand: deep_single_button_list,
            axis: Axis::Vertical,
        }
    }

    fn deep_virtual_list_list_button_tree() -> Screen<DeepNestState> {
        Screen::new("deep-virtual-list-list-button").virtual_list_with_axis(
            deep_outer_len,
            deep_outer_list_item,
            20.0,
            Axis::Vertical,
        )
    }

    /// `screen` に対して `Down` を `presses` 回押し続け（境界で no-op になっても
    /// 継続する、末尾での決定的な着地が目的）、最終的な `FocusEntry.path` を返す。
    fn move_focus_down_many<S>(
        screen: &Screen<S>,
        state: &S,
        controller: &mut TvFocusController<S>,
        presses: usize,
    ) -> Vec<usize> {
        for _ in 0..presses {
            controller
                .ring
                .move_focus(screen, state, FocusDirection::Down);
        }
        controller
            .ring
            .current()
            .expect("フォーカスが存在するはず")
            .path
            .clone()
    }

    /// `controller.update` を、明示的に小さい `screen_rect` を与えた 1 フレームだけ
    /// 実行する。`egui::Context::run` は `RawInput::default()`（`screen_rect: None`）だと
    /// 初回フレームの `viewport_rect` に `10_000×10_000`（`egui-0.33.3` の
    /// `InputState::default` 既定値）という巨大な既定値を使うため、既定（受動的）
    /// スクロールのままでも 200 行程度の `VirtualList` なら overscan なしで全件が
    /// 可視範囲に収まってしまい、能動的 reveal が無くても偶然テストが通ってしまう
    /// （本欠陥をテストが見落とした一因）。実機の TV 画面相当の現実的な小さい
    /// ビューポートを明示することで、能動的な reveal が実際に必要な状況を再現する。
    fn run_frame_with_small_viewport<S>(
        screen: &Screen<S>,
        store: &mut StateStore<S>,
        controller: &mut TvFocusController<S>,
    ) {
        let egui_ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(300.0, 200.0),
            )),
            ..Default::default()
        };
        let _ = egui_ctx.run(input, |ectx| {
            egui::CentralPanel::default().show(ectx, |ui| {
                controller.update(ui, screen, store);
            });
        });
    }

    /// ケース 1（VirtualList → VirtualList）: 外側コンテナの既定窓では確実に
    /// 含まれない深部（末尾付近）までフォーカスを進めても、1 フレームの
    /// `update` で外側・内側両方の `window_hints` が対象を含むことを確認する。
    /// 修正前（厳密一致条件）はこの assertion が外側側で失敗する
    /// （Cursor Bugbot High 指摘の再現・修正確認）。
    #[test]
    fn nested_virtual_list_reveal_reaches_far_outer_group() {
        let screen = deep_nested_virtual_list_tree();
        let state = DeepNestState;
        let mut controller: TvFocusController<DeepNestState> = TvFocusController::new();

        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, &state, &hints);

        // 内側リストの境界到達のたびに外側グループが 1 つ進む
        // （`DEEP_INNER_LEN` 回 Down するごとに次のグループへ escape する）ため、
        // 末尾（外側 index = DEEP_OUTER_LEN - 1）へ確実に到達できる回数を押す。
        let leaf_path = move_focus_down_many(
            &screen,
            &state,
            &mut controller,
            DEEP_OUTER_LEN * DEEP_INNER_LEN + DEEP_INNER_LEN,
        );
        // `leaf_path[0]` は Screen 直下（唯一のトップレベルノード = 外側
        // `VirtualList` 自身）の index で常に 0（外側コンテナ自身の path は
        // `leaf_path[..1]`）。外側グループ index は `leaf_path[1]`。
        assert_eq!(leaf_path.len(), 3, "内側リストの葉は 3 セグメントのはず");
        assert_eq!(
            leaf_path[1],
            DEEP_OUTER_LEN - 1,
            "境界到達を繰り返せば末尾グループへ着地するはず"
        );

        let mut store = StateStore::new(state);
        run_frame_with_small_viewport(&screen, &mut store, &mut controller);

        let outer_path = leaf_path[..1].to_vec();
        let outer_range = controller
            .window_hints
            .get(&outer_path)
            .unwrap_or_else(|| panic!("外側コンテナ（{outer_path:?}）の window_hints が無い"));
        assert!(
            outer_range.contains(&leaf_path[1]),
            "外側コンテナの可視範囲に末尾グループ index が含まれるはず: {outer_range:?}"
        );

        let inner_path = leaf_path[..2].to_vec();
        let inner_range = controller
            .window_hints
            .get(&inner_path)
            .unwrap_or_else(|| panic!("内側コンテナ（{inner_path:?}）の window_hints が無い"));
        assert!(
            inner_range.contains(&leaf_path[2]),
            "内側コンテナの可視範囲に葉 index が含まれるはず: {inner_range:?}"
        );
    }

    /// ケース 2（VirtualList → List → button）: `VirtualList` の直下ではなく
    /// `List` を挟んだ 2 段深い葉でも、外側 `VirtualList` の reveal が発動する
    /// ことを確認する（ネストが別の `VirtualList` に限らないことの検証）。
    #[test]
    fn nested_virtual_list_reveal_reaches_far_outer_group_through_list() {
        let screen = deep_virtual_list_list_button_tree();
        let state = DeepNestState;
        let mut controller: TvFocusController<DeepNestState> = TvFocusController::new();

        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, &state, &hints);

        // 内側 `List` は項目 1 件のみのため、Down 1 回ごとに必ず次の外側グループへ
        // escape する。末尾（外側 index = DEEP_OUTER_LEN - 1）へ確実に到達できる
        // 回数を押す。
        let leaf_path = move_focus_down_many(&screen, &state, &mut controller, DEEP_OUTER_LEN + 5);
        // `leaf_path[0]` は外側 `VirtualList` 自身の Screen 直下 index（常に 0）、
        // `leaf_path[1]` が外側グループ index、`leaf_path[2]` が `List` 内の局所
        // index（3 セグメント = 外側コンテナ自身の path から 2 段深い葉、
        // 「VirtualList → List → button」）。
        assert_eq!(
            leaf_path.len(),
            3,
            "VirtualList → List → button の葉は 3 セグメントのはず"
        );
        assert_eq!(
            leaf_path[1],
            DEEP_OUTER_LEN - 1,
            "境界到達を繰り返せば末尾グループへ着地するはず"
        );

        let mut store = StateStore::new(state);
        run_frame_with_small_viewport(&screen, &mut store, &mut controller);

        let outer_path = leaf_path[..1].to_vec();
        let outer_range = controller
            .window_hints
            .get(&outer_path)
            .unwrap_or_else(|| panic!("外側コンテナ（{outer_path:?}）の window_hints が無い"));
        assert!(
            outer_range.contains(&leaf_path[1]),
            "List を挟んでも外側コンテナの可視範囲に末尾グループ index が含まれるはず: {outer_range:?}"
        );
    }

    // ------------------------------------------------------------------
    // codex-review 5 巡目 P1 指摘: 決定操作を rebuild より先に処理すると、直前の
    // 状態更新でフォーカス対象の path がずれていた場合に決定入力が破棄される
    // （id ガードが stale な path に対して false を返す）。`update` 内で
    // rebuild → decide の順に直したことの検証。
    // ------------------------------------------------------------------

    /// 安定な `id`（`items` の値）を保ちつつ順序が変わりうる `List` 1 本のみを持つ
    /// テスト用状態。`fired_id` は決定操作で実際に発火した項目の `id` を記録する
    /// （`activate_focus_at` が正しい対象へ発火したかを id ベースで検証するため）。
    #[derive(Default)]
    struct StaleFocusState {
        items: Vec<u64>,
        fired_id: Option<u64>,
    }

    fn stale_focus_on_click(state: &mut StaleFocusState, index: usize) {
        state.fired_id = Some(state.items[index]);
    }

    fn stale_focus_expand(state: &StaleFocusState) -> Vec<Node<StaleFocusState>> {
        state
            .items
            .iter()
            .enumerate()
            .map(|(index, &id)| Node::IndexedButton {
                label: format!("item-{id}"),
                index,
                on_click: stale_focus_on_click,
                id: WidgetId::from_raw(id),
            })
            .collect()
    }

    fn stale_focus_tree() -> Screen<StaleFocusState> {
        Screen::new("stale-focus").list_with_axis(stale_focus_expand, Axis::Vertical)
    }

    /// `egui::Context::run` へ Enter キー押下（決定操作）のみを含む `RawInput` を渡し、
    /// `controller.update` を 1 フレーム実行する。
    fn run_frame_with_decide<S>(
        screen: &Screen<S>,
        store: &mut StateStore<S>,
        controller: &mut TvFocusController<S>,
    ) {
        let egui_ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = egui_ctx.run(input, |ectx| {
            egui::CentralPanel::default().show(ectx, |ui| {
                controller.update(ui, screen, store);
            });
        });
    }

    /// codex-review 5 巡目 P1 指摘の核心: 前フレーム後の状態更新でフォーカス対象の
    /// path がずれた直後に決定入力を送っても、破棄されず（rebuild が先に path を
    /// 最新化するため）正しい対象（id ベース）で発火することを確認する。
    #[test]
    fn decide_fires_correct_target_after_focused_path_shifts() {
        let mut state = StaleFocusState {
            items: vec![1, 2, 3],
            fired_id: None,
        };
        let screen = stale_focus_tree();
        let mut controller: TvFocusController<StaleFocusState> = TvFocusController::new();

        // フォーカスを id=2（path=[0,1]）へ進める。
        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, &state, &hints);
        controller
            .ring
            .move_focus(&screen, &state, FocusDirection::Down);
        assert_eq!(controller.ring.current().unwrap().id, WidgetId::from_raw(2));
        assert_eq!(controller.ring.current().unwrap().path, vec![0, 1]);

        // `update` を経由しない外部要因（バックグラウンド更新等）で並べ替えが起こり、
        // 保持中の path=[0,1] が指す項目が id=2 から id=1 へ変わる（stale 化）。
        // このとき `controller.ring.current()` はまだ古い FocusEntry{path:[0,1], id:2}
        // のままである（rebuild を挟んでいないため）。`StateStore` へ載せる前の
        // 生の `S` に対する直接変更として表現する（`dispatch` 経由の書き込みとは
        // 別の外部要因を模す意図的な直接アクセス）。
        state.items = vec![2, 1, 3];

        // 1 フレームで決定操作（Enter）を送る。rebuild が decide より先に走ることで
        // path が id=2 の新しい位置（[0,0]）へ追随してから決定操作が解決される。
        let mut store = StateStore::new(state);
        run_frame_with_decide(&screen, &mut store, &mut controller);

        assert_eq!(
            store.state().fired_id,
            Some(2),
            "決定入力が破棄されず、stale path 更新後の正しい対象（id=2）へ発火するはず"
        );
        assert_eq!(
            controller.ring.current().unwrap().path,
            vec![0, 0],
            "rebuild により id=2 の新しい path（並べ替え後の先頭）へ更新されているはず"
        );
    }

    // ------------------------------------------------------------------
    // codex-review 4 巡目 P1 指摘: `Axis::Horizontal` の `render_virtual_list`
    // （[`render_virtual_list_horizontal`]）が横スクロール・横方向の可視範囲計算を
    // 使うことの検証（縦スクロール実装のまま軸だけ `Horizontal` を許していた欠陥の
    // 再発防止）。
    // ------------------------------------------------------------------

    const HORIZONTAL_LEN: usize = 200;

    fn horizontal_len(_state: &DeepNestState) -> usize {
        HORIZONTAL_LEN
    }

    fn horizontal_item(_state: &DeepNestState, index: usize) -> Node<DeepNestState> {
        Node::IndexedButton {
            label: format!("h-{index}"),
            index,
            on_click: deep_nest_no_op,
            id: WidgetId::from_raw(index as u64),
        }
    }

    fn horizontal_virtual_list_tree() -> Screen<DeepNestState> {
        Screen::new("horizontal-virtual-list").virtual_list_with_axis(
            horizontal_len,
            horizontal_item,
            20.0,
            Axis::Horizontal,
        )
    }

    /// `Axis::Horizontal` の `VirtualList` を末尾付近まで `Right` で移動したあと 1
    /// フレーム `update` すると、`window_hints`（横スクロールオフセットに応じた
    /// item index 範囲）に対象 index が含まれることを確認する。縦スクロールの
    /// `ScrollArea::vertical` のまま `columns = 1` で扱っていた場合、横方向の
    /// `Right` 移動そのものは `FocusRing` 側で成立するため一見動くように見えるが、
    /// 表示は縦一列のまま実際の見た目と操作方向が食い違う（今回の修正対象）。
    /// この結合テストは可視範囲ヒントの整合性のみを機械的に検証する
    /// （見た目の横並びは目視/実機確認が前提、レポートで明示する）。
    #[test]
    fn horizontal_virtual_list_reveals_far_item_after_moving_right() {
        let screen = horizontal_virtual_list_tree();
        let state = DeepNestState;
        let mut controller: TvFocusController<DeepNestState> = TvFocusController::new();

        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, &state, &hints);

        for _ in 0..(HORIZONTAL_LEN - 1) {
            controller
                .ring
                .move_focus(&screen, &state, FocusDirection::Right);
        }
        let leaf_path = controller
            .ring
            .current()
            .expect("フォーカスが存在するはず")
            .path
            .clone();
        assert_eq!(leaf_path, vec![0, HORIZONTAL_LEN - 1]);

        let mut store = StateStore::new(state);
        run_frame_with_small_viewport(&screen, &mut store, &mut controller);

        let container_path = leaf_path[..1].to_vec();
        let range = controller
            .window_hints
            .get(&container_path)
            .unwrap_or_else(|| panic!("コンテナ（{container_path:?}）の window_hints が無い"));
        assert!(
            range.contains(&leaf_path[1]),
            "横スクロール後の可視範囲に末尾 index が含まれるはず: {range:?}"
        );
    }

    /// Cursor Bugbot 指摘（Medium）の再発防止: 横 `VirtualList`（shelf）が親パネルの
    /// 全高を専有せず、内容（1 行分のボタン高さ）に収まることを検証する。修正前は
    /// `auto_shrink([true, false])` + 各項目を `ui.available_height()` で確保していたため、
    /// 交差軸（高さ）がビューポート高さ近くまで消費されていた（横リストを縦に積む
    /// shelf 構成が壊れる原因）。`CentralPanel` は `ui.expand_to_include_rect` で
    /// 常にパネル全体まで `min_rect` を広げる（egui の意図的な panel 仕様）ため、
    /// ここでは内容に合わせて自動サイズする `egui::Area` で計測する
    /// （`ScrollArea` 自身の `auto_shrink` 適用結果を直接反映する）。
    #[test]
    fn horizontal_virtual_list_does_not_consume_full_viewport_height() {
        let screen = horizontal_virtual_list_tree();
        let mut store = StateStore::new(DeepNestState);
        let mut controller: TvFocusController<DeepNestState> = TvFocusController::new();

        let hints = controller.window_hints.clone();
        controller.ring.rebuild(&screen, store.state(), &hints);

        let egui_ctx = egui::Context::default();
        let viewport_height = 200.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(300.0, viewport_height),
            )),
            ..Default::default()
        };
        let mut content_height = None;
        let _ = egui_ctx.run(input, |ectx| {
            let response =
                egui::Area::new(egui::Id::new("horizontal-shelf-height-probe")).show(ectx, |ui| {
                    controller.update(ui, &screen, &mut store);
                });
            content_height = Some(response.response.rect.height());
        });

        let content_height = content_height.expect("Area の内容が描画されているはず");
        assert!(
            content_height < viewport_height / 2.0,
            "横 VirtualList（shelf）はビューポート全高を消費せず 1 行分の高さに収まるはず: \
             content_height={content_height}, viewport_height={viewport_height}"
        );
    }

    // ------------------------------------------------------------------
    // Cursor Bugbot 指摘（Medium）の再発防止: `item_extent` より広い内容（長い
    // ラベルのボタン）でも仮想化の幅計算（スペーサー・可視範囲）が実レイアウトと
    // 一致することの検証。
    // ------------------------------------------------------------------

    const WIDE_ITEM_EXTENT: f32 = 20.0;
    const WIDE_ITEM_LEN: usize = 50;

    /// `item_extent`（20.0）よりはるかに広い長いラベルのボタンを返す
    /// （仮想化の前提が幅固定で成立することを検証する対象）。
    fn wide_horizontal_item(_state: &DeepNestState, index: usize) -> Node<DeepNestState> {
        Node::IndexedButton {
            label: format!("this-is-a-very-long-button-label-{index}"),
            index,
            on_click: deep_nest_no_op,
            id: WidgetId::from_raw(index as u64),
        }
    }

    /// 幅が `item_extent` を超える内容を持つ横 `VirtualList` を 1 フレーム描画しても、
    /// 項目を並べた横一列の実際の消費幅（[`render_virtual_list_horizontal`] の戻り値）が
    /// `total_extent`（`total_len * item_extent`）を超えないことを確認する
    /// （3dda8a3 で `allocate_ui_with_layout` の desired size を `(item_extent, 0.0)` へ
    /// 変えた際、幅ピン止めが失われて各セルが内容分だけ拡張してしまっていた欠陥の
    /// 再発防止。egui の `Ui::allocate_ui` は内容が desired_size を超えると実際に
    /// 使った幅をそのまま親へ伝播するため（0.33 doc「If the contents overflow, more
    /// space will be allocated」）、外側の `Area`/`Panel` の見た目の幅ではなく行 `Ui`
    /// 自身の消費幅を直接見ることで幅超過を確実に検出する。修正前はこの assertion が
    /// 失敗する）。
    #[test]
    fn horizontal_virtual_list_clamps_width_when_content_wider_than_item_extent() {
        let state = DeepNestState;
        let path = vec![0usize];
        let mut recyclers = VirtualListRecyclers::new();
        let old_hints = WindowHints::new();

        let egui_ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(300.0, 200.0),
            )),
            ..Default::default()
        };
        let mut row_width = None;
        let _ = egui_ctx.run(input, |ectx| {
            egui::CentralPanel::default().show(ectx, |ui| {
                let mut ctx = TvRenderCtx {
                    focused_path: None,
                    new_hints: WindowHints::new(),
                    old_hints: &old_hints,
                    recyclers: &mut recyclers,
                    version: 0,
                    pending_events: Vec::new(),
                };
                row_width = Some(render_virtual_list_horizontal(
                    ui,
                    &path,
                    WIDE_ITEM_LEN,
                    wide_horizontal_item,
                    WIDE_ITEM_EXTENT,
                    &state,
                    &mut ctx,
                ));
            });
        });

        let row_width = row_width.expect("横一列の消費幅を取得できているはず");
        let total_extent = WIDE_ITEM_LEN as f32 * WIDE_ITEM_EXTENT;
        let expected_max_width = total_extent + 1.0; // 丸め誤差の許容。

        assert!(
            row_width <= expected_max_width,
            "内容が item_extent より広くても消費幅は total_extent（N * item_extent）を \
             超えないはず: row_width={row_width}, expected_max_width={expected_max_width}"
        );
    }
}
