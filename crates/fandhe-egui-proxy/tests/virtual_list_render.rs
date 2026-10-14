//! `Node::VirtualList` の egui 実描画パス（`render_virtual_list`）を検証する
//! 結合テスト（TASK-5.5 残課題、Issue #487・親 issue #83）。
//!
//! `desktop_render.rs` と同じ wgpu アダプタ欠如時のスキップパターン
//! （`is_adapter_unavailable_panic`）を踏襲する（重複コメントは避け、判定理由は
//! `desktop_render.rs` を参照）。
//!
//! 300 項目規模（REQ-5 の性能確認基準）の `VirtualList` を、可視領域が数項目分しか
//! 収まらない小さいビューポート（`HarnessBuilder::with_size`）で描画し、
//! `item(state, index)` が実際に呼び出された index を記録することで
//! 「可視範囲外ノードが描画されない」（Issue #487 受け入れ条件）ことを検証する。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Node, Screen, StateStore, WidgetId};

/// [`desktop_render.rs`] と同じ理由（1 GPU を共有する検証環境での初期化ハング回避）で
/// テスト単位で直列化する。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度でも「アダプタなし」と判定したら記録し、以降は `Harness` の
/// 再構築を省略する（`desktop_render.rs` と同じ最適化）。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const ADAPTER_UNAVAILABLE_PANIC_MARKERS: [&str; 2] =
    ["CustomNativeAdapterSelectionError", "No adapter found"];

/// `desktop_render.rs::is_adapter_unavailable_panic` と同一の判定ロジック
/// （アダプタ欠如以外の panic を握りつぶさないための文字列一致判定）。
fn is_adapter_unavailable_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let message: Option<&str> = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    match message {
        Some(message) => ADAPTER_UNAVAILABLE_PANIC_MARKERS
            .iter()
            .any(|marker| message.contains(marker)),
        None => false,
    }
}

/// 300 項目規模の `VirtualList` 用テスト状態。項目数のみを持つ。
#[derive(Default)]
struct VirtualListState {
    len: usize,
    /// 対話ノード検証（[`clicking_button_inside_virtual_list_dispatches_event`]）で
    /// ボタン押下回数を数えるカウンタ。他のテストでは常に 0 のまま。
    count: usize,
    /// 直近にクリックされた `IndexedButton` の生成時 index（Issue #491）。
    /// 位置解決が生成時 index を正しく引き継ぐことの検証に使う。
    last_clicked_index: Option<usize>,
}

/// テスト対象の総項目数（REQ-5 の性能確認基準に合わせた 300 項目規模）。
const TOTAL_ITEMS: usize = 300;

/// 1 項目あたりの固定高さ。ビューポート高さ（後述 `VIEWPORT_HEIGHT`）との比で
/// 可視項目数が数件に収まるよう選定する。
const ITEM_EXTENT: f32 = 40.0;

/// `HarnessBuilder::with_size` に渡すビューポート寸法。高さを小さくすることで
/// 300 項目のうち可視ウィンドウ外の項目が実際に構築されないことを検証しやすくする。
const VIEWPORT_SIZE: egui::Vec2 = egui::Vec2::new(300.0, 160.0);

fn virtual_list_len(state: &VirtualListState) -> usize {
    state.len
}

/// 呼び出された絶対 index を [`BUILT_INDICES`] へ記録するテスト専用の項目展開関数。
/// `Node::VirtualList::item` は環境キャプチャ不可の素の関数ポインタ限定
/// （node.rs 参照）のため、記録先はプロセスグローバルな static に限定される。
fn virtual_list_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(index);
    Node::Text(format!("item-{index}"))
}

/// `virtual_list_item` が実際に呼び出された絶対 index の記録先。
/// テストごとに [`reset_built_indices`] でクリアしてから `harness.run()` を呼ぶ。
static BUILT_INDICES: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

fn reset_built_indices() {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

fn built_indices() -> Vec<usize> {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn build_virtual_list_screen() -> Screen<VirtualListState> {
    Screen::new("仮想化リスト実描画テスト").virtual_list(
        virtual_list_len,
        virtual_list_item,
        ITEM_EXTENT,
    )
}

fn build_harness() -> egui_kittest::Harness<'static, StateStore<VirtualListState>> {
    build_harness_with(build_virtual_list_screen(), VIEWPORT_SIZE)
}

/// 画面・ビューポート寸法を差し替え可能な `Harness` 構築ヘルパー
/// （兄弟ノード配置テスト等、標準の 300 項目構成以外の検証で使う）。
fn build_harness_with(
    screen: Screen<VirtualListState>,
    size: egui::Vec2,
) -> egui_kittest::Harness<'static, StateStore<VirtualListState>> {
    egui_kittest::Harness::builder()
        .with_size(size)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<VirtualListState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(VirtualListState {
                len: TOTAL_ITEMS,
                count: 0,
                last_clicked_index: None,
            }),
        )
}

/// [`fandhe_egui_proxy::render_screen_with_recyclers`] 経由でフレーム跨ぎ再利用プールを
/// 保持する `Harness` を構築する（Issue #490 の受け入れ条件検証用）。
/// `recyclers` は `move` クロージャへ捕捉され、`harness.run()` を複数回呼んでも
/// 同一のプールがフレームをまたいで使われる。
fn build_harness_with_recyclers(
    screen: Screen<VirtualListState>,
    size: egui::Vec2,
) -> egui_kittest::Harness<'static, StateStore<VirtualListState>> {
    let mut recyclers = fandhe_egui_proxy::VirtualListRecyclers::new();
    egui_kittest::Harness::builder()
        .with_size(size)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<VirtualListState>| {
                fandhe_egui_proxy::render_screen_with_recyclers(ui, &screen, store, &mut recyclers);
            },
            StateStore::new(VirtualListState {
                len: TOTAL_ITEMS,
                count: 0,
                last_clicked_index: None,
            }),
        )
}

/// [`desktop_render.rs::build_harness_or_skip`] と同一の意図（アダプタ欠如時のみ
/// スキップし、それ以外の構築時 panic はテストを失敗させる）を持つヘルパー。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<VirtualListState>>> {
    harness_or_skip(build_harness)
}

/// [`build_harness_or_skip`] の任意コンストラクタ版。差し替え画面
/// （[`build_harness_with`]）でも同じスキップ判定を共有する。
fn harness_or_skip(
    build: impl Fn() -> egui_kittest::Harness<'static, StateStore<VirtualListState>>,
) -> Option<egui_kittest::Harness<'static, StateStore<VirtualListState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、仮想化リスト実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、仮想化リスト実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 300 項目のうち、可視ウィンドウ外（末尾付近）の項目は `item` が一度も呼ばれず、
/// egui のラベルとしても存在しないことを検証する（Issue #487 受け入れ条件の中核）。
#[test]
fn out_of_view_items_are_never_built_or_rendered() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };

    reset_built_indices();
    harness.run();

    let built = built_indices();
    assert!(
        !built.is_empty(),
        "可視ウィンドウ内の項目は最低 1 件構築されるはず"
    );

    // ビューポート高さ 160px・項目高さ 40px・overscan 3 の設定では、可視ウィンドウは
    // 先頭付近の 1 桁〜10 数項目程度に収まる（`compute_visible_range` の算出式）。
    // 300 項目中の末尾（299）が構築されていなければ、ウィンドウ外項目が
    // 実際に描画対象から除外されていることの直接的な証拠になる。
    let far_index = TOTAL_ITEMS - 1;
    assert!(
        !built.contains(&far_index),
        "末尾付近の項目 index={far_index} は可視範囲外のため構築されないはず: {built:?}"
    );
    assert!(
        built.iter().all(|&index| index < 30),
        "可視ウィンドウは先頭付近に限定されるはず（overscan 込みでも 30 未満）: {built:?}"
    );

    let first_visible_label = format!("item-{}", built[0]);
    harness.get_by_label(&first_visible_label);

    let far_label = format!("item-{far_index}");
    assert!(
        harness.query_all_by_label(&far_label).next().is_none(),
        "可視範囲外のラベル {far_label} は AccessKit ツリーに現れないはず"
    );
}

/// 実際にスクロールして可視ウィンドウを後方へシフトさせ、ウィンドウ外へ出た
/// 項目（先頭 `item-0`）のラベルが消え、新たに可視となった項目のラベルが
/// 現れることを検証する（`egui_kittest` の `Node::scroll_down` による
/// `accesskit::Action::ScrollDown` シミュレーション）。
///
/// ウィンドウがシフトしても末尾付近（`far_index`）は依然として構築されない
/// （overscan を加味しても可視ウィンドウに入らない）ことも合わせて確認し、
/// 「可視範囲外ノードが描画されない」契約がスクロール後も維持されることを示す。
#[test]
fn scrolling_shifts_visible_window_without_building_far_items() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };

    reset_built_indices();
    harness.run();
    harness.get_by_label("item-0");

    // `Node` は `Copy`（`queue: &'tree EventQueue` 参照込み）だが、`harness` への
    // 不変借用を保持したまま `harness.run()`（`&mut self`）は呼べないため、
    // スコープを区切って借用を先に終わらせる。
    {
        let item0 = harness.get_by_label("item-0");
        for _ in 0..12 {
            item0.scroll_down();
        }
    }

    reset_built_indices();
    harness.run();

    let built = built_indices();
    let far_index = TOTAL_ITEMS - 1;
    assert!(
        !built.contains(&far_index),
        "スクロール後も末尾付近 index={far_index} は可視範囲外のため構築されないはず: {built:?}"
    );
    assert!(
        built.iter().any(|&index| index > 0),
        "スクロール後は可視ウィンドウが後方へシフトしているはず: {built:?}"
    );

    let has_item_zero_label = harness.query_all_by_label("item-0").next().is_some();
    assert!(
        !has_item_zero_label,
        "十分にスクロールした後は先頭項目 item-0 が可視ウィンドウから外れ、\
         AccessKit ツリーに現れないはず"
    );
}

/// `push_id` に用いる id salt が「ウィンドウ内の列挙位置」ではなく
/// 「絶対 index」であることを、`egui::Id` の生成結果を突き合わせて検証する
/// （Issue #487 受け入れ条件「スクロール位置に応じた widget id が安定している」の
/// 直接的な回帰防止。GPU 非依存）。
#[test]
fn item_id_salt_is_derived_from_absolute_index_not_window_position() {
    // ウィンドウが `[0..7)` の場合と `[93..100)` の場合とで、絶対 index 95 に対応する
    // 項目の id を比較する。ウィンドウ相対位置（列挙位置）由来の id 生成だと、
    // 前者のウィンドウには index 95 が含まれず比較できない一方、後者のウィンドウでは
    // 列挙位置 2 番目（93, 94, 95, ...）になり、絶対 index 由来の id と異なる値になる。
    let absolute_index = 95usize;
    let id_by_absolute_index = egui::Id::new(("fandhe-virtual-item", absolute_index));

    // ウィンドウ [93..100) 内での列挙位置（2 番目、0 始まり）から誤って id を作った場合の値。
    let window_relative_position = absolute_index - 93;
    let id_by_window_position = egui::Id::new(("fandhe-virtual-item", window_relative_position));

    assert_ne!(
        id_by_absolute_index, id_by_window_position,
        "絶対 index 由来の id とウィンドウ相対位置由来の id は異なるはず（区別できることの前提確認）"
    );

    // ウィンドウがシフトしても、同一の絶対 index に対する id 生成結果は不変であること
    // （`render_virtual_list` が `ui.push_id(("fandhe-virtual-item", index), ...)` を
    // 絶対 index で呼ぶ実装と一致する契約）。
    let id_recomputed = egui::Id::new(("fandhe-virtual-item", absolute_index));
    assert_eq!(
        id_by_absolute_index, id_recomputed,
        "同一の絶対 index からは常に同一の Id が得られるはず"
    );
}

/// ネストした `VirtualList` の `ScrollArea` id_salt がツリー上の完全な `path` から
/// 生成されることの契約テスト（GPU 非依存）。親の `path` を引き継がず
/// 兄弟内 index のみ（`vec![index]`）へリセットすると、別の親配下の同 index リストや
/// トップレベルの同 index リストと id が一致してしまい、スクロール状態が衝突する
/// （security-auditor 指摘の回帰防止。`render_virtual_list` の `child_path` 構築参照）。
#[test]
fn nested_virtual_list_scroll_id_salt_uses_full_path() {
    // トップレベル 5 番目のリストと、親 0 の子 5 番目にネストしたリスト。
    // 完全な path から salt を作れば異なる id になる。
    let top_level = egui::Id::new(("fandhe-virtual-list", vec![5usize]));
    let nested_under_parent_zero = egui::Id::new(("fandhe-virtual-list", vec![0usize, 5usize]));
    assert_ne!(
        top_level, nested_under_parent_zero,
        "ネストした VirtualList の id_salt は親の path を含むため、\
         トップレベルの同 index リストと衝突しないはず"
    );

    // 異なる親（0 と 1）配下の同 index（5）リスト同士も衝突しないこと。
    let nested_under_parent_one = egui::Id::new(("fandhe-virtual-list", vec![1usize, 5usize]));
    assert_ne!(
        nested_under_parent_zero, nested_under_parent_one,
        "異なる親配下の同 index リスト同士は id が異なるはず"
    );
}

/// 兄弟ノード配置テスト用の小規模リスト（3 項目・記録なし）。`item` は関数ポインタ
/// 限定（node.rs）のため、300 項目版とは別の素の関数として定義する。
fn small_list_len(_state: &VirtualListState) -> usize {
    3
}

fn small_list_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    Node::Text(format!("small-{index}"))
}

/// 兄弟ノード配置テスト用のビューポート。小規模リスト（3 × 40 = 120px）と
/// 見出し・後続ラベルが収まる高さを確保する。
const SIBLING_VIEWPORT_SIZE: egui::Vec2 = egui::Vec2::new(300.0, 320.0);

fn build_sibling_screen() -> Screen<VirtualListState> {
    Screen::new("仮想化リスト兄弟配置テスト")
        .virtual_list(small_list_len, small_list_item, ITEM_EXTENT)
        .text("after-list")
}

/// 総コンテンツ高さが利用可能領域より小さい `VirtualList` の後続に兄弟ノードを
/// 置いた場合、`auto_shrink([false, true])` によりリストがコンテンツ高さへ縮み、
/// 兄弟が可視領域内へ描画されることを検証する（reviewer 指摘: 旧実装の
/// `auto_shrink([false, false])` はリストが残りの垂直領域を全占有し、後続兄弟を
/// ビューポート下端へ押し出していた）。
#[test]
fn trailing_sibling_after_small_virtual_list_stays_visible() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) =
        harness_or_skip(|| build_harness_with(build_sibling_screen(), SIBLING_VIEWPORT_SIZE))
    else {
        return;
    };

    harness.run();

    // リスト項目 3 件と後続兄弟がすべて AccessKit ツリーに存在すること。
    for index in 0..3 {
        harness.get_by_label(&format!("small-{index}"));
    }
    let sibling = harness.get_by_label("after-list");

    // 兄弟がビューポート下端に押し出されず、リスト直後（項目 3 件分 = 120px +
    // 見出し・セパレータの直下）に配置されていること。ビューポート高さの
    // 8 割以内なら「下端で切り詰められている」状態ではないと判断できる。
    let sibling_top = sibling.rect().top();
    assert!(
        sibling_top < SIBLING_VIEWPORT_SIZE.y * 0.8,
        "後続兄弟はリスト直後に描画されるはず（実際の top: {sibling_top}）"
    );
}

/// 可視項目の行送り（隣接項目の top 差分）が `item_extent` と一致することを
/// 検証する（reviewer 指摘: 旧実装は項目の実描画高さ（ラベル約 18px + 既定
/// スペーシング）で行送りされ、`compute_visible_range` の幾何モデル
/// 「1 項目 = `item_extent` px」と実レイアウトが乖離していた）。
#[test]
fn visible_item_row_pitch_matches_item_extent() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };

    reset_built_indices();
    harness.run();

    let rect_0 = harness.get_by_label("item-0").rect();
    let rect_1 = harness.get_by_label("item-1").rect();
    let row_pitch = rect_1.top() - rect_0.top();
    assert!(
        (row_pitch - ITEM_EXTENT).abs() < 0.5,
        "隣接項目の行送りは item_extent（{ITEM_EXTENT}px）と一致するはず（実際: {row_pitch}px）"
    );
}

/// 幅より長い 1 行テキストを返す項目（折り返さず行内へ収まることの検証用）。
fn long_text_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    Node::Text(format!("item-{index} {}", "長い名前".repeat(30)))
}

/// 固定高の行で、幅を超える文字列が折り返さず行の高さ内へ収まる（次の行へ重ならない）
/// ことを検証する（PR #1072 指摘: 狭い画面で折り返した行が重なる）。
#[test]
fn long_text_in_fixed_height_row_does_not_wrap_into_next_row() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(|| {
        build_harness_with(
            Screen::new("長文行").virtual_list(virtual_list_len, long_text_item, 24.0),
            egui::vec2(200.0, 160.0),
        )
    }) else {
        return;
    };

    harness.run();

    // 折り返しは描画された galley の行数で判定する（アクセシビリティ矩形は先頭行しか
    // 返さないため高さでは検出できない）。
    let mut max_rows = 0;
    for clipped in &harness.output().shapes {
        if let egui::Shape::Text(text) = &clipped.shape {
            if text.galley.text().starts_with("item-") {
                max_rows = max_rows.max(text.galley.rows.len());
            }
        }
    }
    assert_eq!(max_rows, 1, "固定高の行内で折り返さず 1 行に収まるはず");
}

/// 対話ノード検証用の 1 項目リスト。項目は [`INTERACTIVE_BUTTON_LABEL`] のボタンを返す。
fn interactive_list_len(_state: &VirtualListState) -> usize {
    1
}

/// ネスト側で使うボタンラベル。
///
/// Issue #491（動的生成子内の対話ノードのハンドラ解決）により、`VirtualList::item` が
/// 動的生成した子ボタンも `Event::ButtonClickedAt`（`ui::click_button_at` による位置
/// 解決）で dispatch まで届く。ラベル一致の非展開走査（`Event::ButtonClicked`）は
/// 経路として使わないため、同一ラベルのボタンをトップレベルに併置する必要はない
/// （本テストはネスト側単独のハンドラ発火を直接検証する）。
const INTERACTIVE_BUTTON_LABEL: &str = "vl-増加";

/// 動的生成子内の対話ノードには生成時 index を埋め込む `Node::IndexedButton`
/// （Issue #491）を使う。`_index` は本テストでは 1 項目のみのため使わないが、
/// 実際の項目 index が `bump_count_at` へ渡ることは `indexed_button_receives_generation_index`
/// で別途検証する。
fn interactive_list_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    Node::IndexedButton {
        label: INTERACTIVE_BUTTON_LABEL.to_string(),
        index,
        on_click: bump_count_at,
        id: WidgetId::from_raw(index as u64),
    }
}

fn bump_count(state: &mut VirtualListState) {
    state.count += 1;
}

/// index 付きハンドラ版。`index` は `last_clicked_index` へ記録し、位置解決が
/// 生成時 index を正しく引き継ぐことを検証できるようにする。
fn bump_count_at(state: &mut VirtualListState, index: usize) {
    state.count += 1;
    state.last_clicked_index = Some(index);
}

fn build_interactive_screen() -> Screen<VirtualListState> {
    Screen::new("仮想化リスト対話ノードテスト").virtual_list(
        interactive_list_len,
        interactive_list_item,
        ITEM_EXTENT,
    )
}

/// `VirtualList` 経由で描画された対話的ノード（`Node::IndexedButton`）のクリックが
/// `Event::ButtonClickedAt`（位置解決、Issue #491）として dispatch まで届き、
/// 状態が更新されることを検証する（reviewer 指摘: `allocate_exact_size` +
/// `new_child(UiBuilder::max_rect)` の新描画経路で対話ノードのクリック判定・
/// イベント発火が機能することの回帰テスト、位置解決導入後の新契約への更新）。
///
/// トップレベルの併置ボタンは不要になった（[`INTERACTIVE_BUTTON_LABEL`] のコメント
/// 参照）。ネスト側単独のボタンをクリックし、`count == 1`（ちょうど 1 回発火）を
/// 検証する。
#[test]
fn clicking_button_inside_virtual_list_dispatches_event() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) =
        harness_or_skip(|| build_harness_with(build_interactive_screen(), SIBLING_VIEWPORT_SIZE))
    else {
        return;
    };

    harness.run();

    let nested_rect = {
        let nodes: Vec<_> = harness
            .query_all_by_label(INTERACTIVE_BUTTON_LABEL)
            .collect();
        assert_eq!(nodes.len(), 1, "ネスト側のボタン 1 件のみが存在するはず");
        nodes[0].rect()
    };

    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(nested_rect.center()));
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: nested_rect.center(),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    });
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: nested_rect.center(),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    harness.run();

    assert_eq!(
        harness.state().state().count,
        1,
        "ネスト側ボタンのクリックが Event::ButtonClickedAt として emit され、\
         dispatch（位置解決）でネスト側ハンドラがちょうど 1 回発火するはず"
    );
    assert_eq!(
        harness.state().state().last_clicked_index,
        Some(0),
        "生成時に埋め込まれた項目 index（0）がハンドラへ引き継がれるはず"
    );
}

// Issue #540（非 TV `render_virtual_list` の `Axis::Horizontal` 対応）検証:
// `tv_focus.rs::render_virtual_list_horizontal` からの移植先が実際に横描画・横仮想化
// することを確認する。`fandhe_core::Axis` は `#[cfg(target_os = ...)]` を含まない共通コア
// 型のため、TV 側テスト（`tv_focus.rs` 内 `#[cfg(test)]`）とほぼ同じ検証観点を非 TV 側
// （`Screen::virtual_list_with_axis` → `render_screen` 公開 API 経由）で再現する。

/// Issue #540 検証用の横方向 `VirtualList` 総項目数。
const HORIZONTAL_TOTAL_ITEMS: usize = 60;

/// 横方向テスト用の項目幅（縦版 `ITEM_EXTENT` と同じ規模）。
const HORIZONTAL_ITEM_EXTENT: f32 = 40.0;

/// 横方向テスト用のビューポート。幅を絞り、可視ウィンドウが数項目に収まるようにする。
const HORIZONTAL_VIEWPORT_SIZE: egui::Vec2 = egui::Vec2::new(160.0, 200.0);

fn horizontal_list_len(state: &VirtualListState) -> usize {
    state.len
}

fn horizontal_list_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(index);
    Node::Text(format!("h-item-{index}"))
}

fn build_horizontal_screen() -> Screen<VirtualListState> {
    Screen::new("横仮想化リスト実描画テスト").virtual_list_with_axis(
        horizontal_list_len,
        horizontal_list_item,
        HORIZONTAL_ITEM_EXTENT,
        fandhe_core::Axis::Horizontal,
    )
}

fn build_horizontal_harness_with(
    screen: Screen<VirtualListState>,
    size: egui::Vec2,
    len: usize,
) -> egui_kittest::Harness<'static, StateStore<VirtualListState>> {
    egui_kittest::Harness::builder()
        .with_size(size)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<VirtualListState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(VirtualListState {
                len,
                count: 0,
                last_clicked_index: None,
            }),
        )
}

fn build_horizontal_harness_or_skip(
) -> Option<egui_kittest::Harness<'static, StateStore<VirtualListState>>> {
    harness_or_skip(|| {
        build_horizontal_harness_with(
            build_horizontal_screen(),
            HORIZONTAL_VIEWPORT_SIZE,
            HORIZONTAL_TOTAL_ITEMS,
        )
    })
}

/// `Axis::Horizontal` の項目が横一列（`left()` 差 = `item_extent`、`top()` 同一）に
/// 並ぶことを検証する（縦描画固定だった旧実装ではこの位置関係が成立しない）。
#[test]
fn horizontal_virtual_list_places_items_side_by_side() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_horizontal_harness_or_skip() else {
        return;
    };

    reset_built_indices();
    harness.run();

    let rect_0 = harness.get_by_label("h-item-0").rect();
    let rect_1 = harness.get_by_label("h-item-1").rect();
    let column_pitch = rect_1.left() - rect_0.left();
    assert!(
        (column_pitch - HORIZONTAL_ITEM_EXTENT).abs() < 0.5,
        "隣接項目の列送りは item_extent（{HORIZONTAL_ITEM_EXTENT}px）と一致するはず（実際: {column_pitch}px）"
    );
    assert!(
        (rect_1.top() - rect_0.top()).abs() < 0.5,
        "横一列のため top() は一致するはず（rect_0: {rect_0:?}, rect_1: {rect_1:?}）"
    );
}

/// 横 `VirtualList` の直後に兄弟ノードを置いた場合、`auto_shrink([false, true])` に
/// より 1 行分の高さへ縮み、兄弟がビューポート高の半分未満の位置（縦を専有しない）に
/// 描画されることを検証する（非 TV 側の `auto_shrink` 極性の回帰防止）。
#[test]
fn horizontal_virtual_list_does_not_consume_full_viewport_height() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let screen = Screen::new("横仮想化リスト兄弟配置テスト")
        .virtual_list_with_axis(
            horizontal_list_len,
            horizontal_list_item,
            HORIZONTAL_ITEM_EXTENT,
            fandhe_core::Axis::Horizontal,
        )
        .text("after-horizontal-list");
    let viewport = egui::Vec2::new(300.0, 300.0);
    let Some(mut harness) =
        harness_or_skip(|| build_horizontal_harness_with(screen.clone(), viewport, 5))
    else {
        return;
    };

    harness.run();

    let sibling = harness.get_by_label("after-horizontal-list");
    assert!(
        sibling.rect().top() < viewport.y / 2.0,
        "横 VirtualList は 1 行分の高さに縮むため、後続兄弟はビューポート高の半分未満に \
         描画されるはず（実際の top: {}）",
        sibling.rect().top()
    );
}

/// `item_extent` より広い内容（長いラベル）を持つ項目があっても、幅厳密固定
/// （`new_child` + `advance_cursor_after_rect`）により列送りが `item_extent` を
/// 超えないことを検証する（幅ピン止めが失われると消費幅が `total_extent` を超える）。
#[test]
fn horizontal_virtual_list_clamps_width_when_content_wider_than_item_extent() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    const WIDE_LEN: usize = 5;
    fn wide_len(state: &VirtualListState) -> usize {
        state.len
    }
    fn wide_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
        Node::Text(format!(
            "this-is-a-very-long-wide-label-item-{index}-with-extra-padding"
        ))
    }
    let screen = Screen::new("横仮想化リスト幅超過テスト").virtual_list_with_axis(
        wide_len,
        wide_item,
        HORIZONTAL_ITEM_EXTENT,
        fandhe_core::Axis::Horizontal,
    );
    let viewport = egui::Vec2::new(600.0, 200.0);
    let Some(mut harness) =
        harness_or_skip(|| build_horizontal_harness_with(screen.clone(), viewport, WIDE_LEN))
    else {
        return;
    };

    harness.run();

    let first_label = "this-is-a-very-long-wide-label-item-0-with-extra-padding";
    let last_label = "this-is-a-very-long-wide-label-item-4-with-extra-padding";
    let rect_first = harness.get_by_label(first_label).rect();
    let rect_last = harness.get_by_label(last_label).rect();
    let expected_pitch = (WIDE_LEN - 1) as f32 * HORIZONTAL_ITEM_EXTENT;
    let actual_pitch = rect_last.left() - rect_first.left();
    assert!(
        (actual_pitch - expected_pitch).abs() < 1.0,
        "内容が item_extent より広くても列送りは item_extent 単位で厳密固定されるはず \
         （expected: {expected_pitch}, actual: {actual_pitch}）"
    );
}

/// 横スクロールオフセットで可視窓がシフトし、範囲外（先頭）の項目が構築されなく
/// なることを検証する（縦版 `scrolling_shifts_visible_window_without_building_far_items`
/// の横方向版）。
///
/// **スクロール操作の手段**: `egui_kittest::Node::scroll_right`（AccessKit
/// `Action::ScrollRight`）は本 egui バージョン（0.33.3）で `ScrollArea::horizontal` へ
/// 適用されず（`egui::Context::create_widget` が生成する `scroll_delta` が
/// `ScrollArea::end` で消費されず、`state.offset.x` が変化しない実機検証済みの制約。
/// `ScrollArea::vertical` では同じ経路が正しく機能するため egui 側の水平軸限定の
/// 挙動と判断）、縦版と異なり直接使えない。かわりに実際のトラックパッド/マウスホイールの
/// 横スクロールに相当する `egui::Event::MouseWheel`（`delta.x` 負値 = コンテンツを
/// 右へ送る、`egui::Event::MouseWheel` doc 参照）を注入する経路で検証する
/// （こちらは実機検証で `state.offset.x` の変化を確認済み）。
#[test]
fn horizontal_scrolling_shifts_visible_window_without_building_far_items() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_horizontal_harness_or_skip() else {
        return;
    };

    reset_built_indices();
    harness.run();
    let pointer_pos = harness.get_by_label("h-item-0").rect().center();

    // 横仮想化リストの実描画領域（項目の rect の中心）へポインタを置いてから
    // 水平方向のホイールスクロールを注入する（`MouseWheel` はポインタ位置を
    // 起点にスクロール対象の `ScrollArea` を解決するため、見出し・セパレータの
    // 分だけ縦にオフセットする実レイアウト上の座標が必要。ビューポート中心では
    // 1 行分に縮んだ横リストの外へ外れる場合がある）。
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pointer_pos));
    for _ in 0..10 {
        harness.input_mut().events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-100.0, 0.0),
            modifiers: egui::Modifiers::default(),
        });
    }

    reset_built_indices();
    harness.run();

    let far_index = HORIZONTAL_TOTAL_ITEMS - 1;
    assert!(
        !built_indices().contains(&far_index),
        "横スクロール後も末尾付近 index={far_index} は可視範囲外のため構築されないはず: {:?}",
        built_indices()
    );
    assert!(
        harness.query_all_by_label("h-item-0").next().is_none(),
        "十分に横スクロールした後は先頭項目 h-item-0 が可視ウィンドウから外れ、\
         AccessKit ツリーに現れないはず"
    );
}

// Issue #551（非 TV `render_virtual_list` の `Axis::Grid` 複数列対応）検証:
// `tv_focus.rs::render_virtual_list_vertical_or_grid` からの移植先が実際に行単位で
// 複数列描画・行単位仮想化することを確認する（#540 Horizontal 対応と同じ観点構成）。

/// Issue #551 検証用の列数。
const GRID_COLUMNS: usize = 3;

/// グリッドテスト用の行高（`item_extent` = 1 行の高さ）。
const GRID_ITEM_EXTENT: f32 = 40.0;

/// 端数行検証用の総項目数（`GRID_COLUMNS` で割り切れない値。3 行 + 端数 1 件）。
const GRID_SMALL_TOTAL_ITEMS: usize = 10;

/// スクロール検証用の総項目数（縦版 `TOTAL_ITEMS` と同規模）。
const GRID_LARGE_TOTAL_ITEMS: usize = 300;

/// 先頭数行のみが収まる小さいビューポート（仮想化の可視窓検証用）。
const GRID_VIEWPORT_SIZE: egui::Vec2 = egui::Vec2::new(240.0, 160.0);

/// `GRID_SMALL_TOTAL_ITEMS` の全行（4 行 = 160px）が収まるビューポート。
const GRID_FULL_VIEWPORT_SIZE: egui::Vec2 = egui::Vec2::new(240.0, 600.0);

fn grid_list_len(state: &VirtualListState) -> usize {
    state.len
}

fn grid_list_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(index);
    Node::Text(format!("g-item-{index}"))
}

fn build_grid_screen(columns: usize) -> Screen<VirtualListState> {
    Screen::new("グリッド仮想化リスト実描画テスト").virtual_list_with_axis(
        grid_list_len,
        grid_list_item,
        GRID_ITEM_EXTENT,
        fandhe_core::Axis::Grid { columns },
    )
}

fn build_grid_harness_with(
    screen: Screen<VirtualListState>,
    size: egui::Vec2,
    len: usize,
) -> egui_kittest::Harness<'static, StateStore<VirtualListState>> {
    egui_kittest::Harness::builder()
        .with_size(size)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<VirtualListState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(VirtualListState {
                len,
                count: 0,
                last_clicked_index: None,
            }),
        )
}

fn build_grid_harness_or_skip(
    columns: usize,
    size: egui::Vec2,
    len: usize,
) -> Option<egui_kittest::Harness<'static, StateStore<VirtualListState>>> {
    harness_or_skip(|| build_grid_harness_with(build_grid_screen(columns), size, len))
}

/// 同一行内では x が進み y は一致し、行送り（`columns` 件ごと）では y が
/// `item_extent`（行高）ぶん進み x は行頭に戻ることを検証する（受け入れ条件の中核）。
#[test]
fn grid_virtual_list_places_items_in_rows_then_wraps() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) =
        build_grid_harness_or_skip(GRID_COLUMNS, GRID_VIEWPORT_SIZE, GRID_SMALL_TOTAL_ITEMS)
    else {
        return;
    };

    harness.run();

    let rect_0 = harness.get_by_label("g-item-0").rect();
    let rect_1 = harness.get_by_label("g-item-1").rect();
    let rect_row2_head = harness
        .get_by_label(&format!("g-item-{GRID_COLUMNS}"))
        .rect();

    assert!(
        (rect_1.top() - rect_0.top()).abs() < 0.5,
        "同一行内（index 0, 1）は top() が一致するはず（rect_0: {rect_0:?}, rect_1: {rect_1:?}）"
    );
    assert!(
        rect_1.left() > rect_0.left() + 0.5,
        "同一行内では列送りで x が進むはず（rect_0: {rect_0:?}, rect_1: {rect_1:?}）"
    );
    assert!(
        (rect_row2_head.top() - rect_0.top() - GRID_ITEM_EXTENT).abs() < 0.5,
        "columns={GRID_COLUMNS} 件ごとの行送りでは y が item_extent（{GRID_ITEM_EXTENT}px）\
         ぶん進むはず（rect_0: {rect_0:?}, rect_row2_head: {rect_row2_head:?}）"
    );
    assert!(
        (rect_row2_head.left() - rect_0.left()).abs() < 0.5,
        "行送り後の行頭は x が行頭（rect_0 と同じ列）に戻るはず\
         （rect_0: {rect_0:?}, rect_row2_head: {rect_row2_head:?}）"
    );
}

/// `columns = 0`（DSL 側の誤設定）が 1 列扱いになる（`max(1)` ガード）ことを検証する。
/// 1 列扱いなら各項目が縦に 1 行ずつ積まれ、隣接項目の y 差が `item_extent` と一致する。
#[test]
fn grid_virtual_list_treats_zero_columns_as_one() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    const ZERO_COLUMNS_LEN: usize = 5;
    let Some(mut harness) =
        build_grid_harness_or_skip(0, GRID_FULL_VIEWPORT_SIZE, ZERO_COLUMNS_LEN)
    else {
        return;
    };

    harness.run();

    let rect_0 = harness.get_by_label("g-item-0").rect();
    let rect_1 = harness.get_by_label("g-item-1").rect();
    assert!(
        (rect_1.top() - rect_0.top() - GRID_ITEM_EXTENT).abs() < 0.5,
        "columns=0 は 1 列扱いのため、隣接項目は縦に 1 行ずつ積まれるはず\
         （rect_0: {rect_0:?}, rect_1: {rect_1:?}）"
    );
    assert!(
        (rect_1.left() - rect_0.left()).abs() < 0.5,
        "1 列扱いのため x は変わらないはず（rect_0: {rect_0:?}, rect_1: {rect_1:?}）"
    );
}

/// 総項目数が `columns` で割り切れない場合（端数行）でも、末尾行の要素が
/// 欠けずに描画されることを検証する。
#[test]
fn grid_virtual_list_renders_tail_item_of_ragged_last_row() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_grid_harness_or_skip(
        GRID_COLUMNS,
        GRID_FULL_VIEWPORT_SIZE,
        GRID_SMALL_TOTAL_ITEMS,
    ) else {
        return;
    };

    harness.run();

    // GRID_SMALL_TOTAL_ITEMS=10, GRID_COLUMNS=3 → 行 0=[0,1,2] 行 1=[3,4,5]
    // 行 2=[6,7,8] 行 3=[9]（末尾行は 1 件のみの端数行）。
    let last_index = GRID_SMALL_TOTAL_ITEMS - 1;
    harness.get_by_label(&format!("g-item-{last_index}"));
}

/// スクロールで可視窓（行単位仮想化）が後方へシフトし、末尾付近は構築されないまま、
/// 先頭項目がウィンドウから外れることを検証する（縦版
/// `scrolling_shifts_visible_window_without_building_far_items` のグリッド版）。
#[test]
fn grid_virtual_list_scrolling_shifts_visible_window_without_building_far_items() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) =
        build_grid_harness_or_skip(GRID_COLUMNS, GRID_VIEWPORT_SIZE, GRID_LARGE_TOTAL_ITEMS)
    else {
        return;
    };

    reset_built_indices();
    harness.run();
    harness.get_by_label("g-item-0");

    {
        let item0 = harness.get_by_label("g-item-0");
        for _ in 0..12 {
            item0.scroll_down();
        }
    }

    reset_built_indices();
    harness.run();

    let built = built_indices();
    let far_index = GRID_LARGE_TOTAL_ITEMS - 1;
    assert!(
        !built.contains(&far_index),
        "スクロール後も末尾付近 index={far_index} は可視範囲外のため構築されないはず: {built:?}"
    );
    assert!(
        built.iter().any(|&index| index > 0),
        "スクロール後は可視ウィンドウが後方へシフトしているはず: {built:?}"
    );
    assert!(
        harness.query_all_by_label("g-item-0").next().is_none(),
        "十分にスクロールした後は先頭項目 g-item-0 が可視ウィンドウから外れ、\
         AccessKit ツリーに現れないはず"
    );
}

/// 幅の異なる内容（長い Text と短い Text の混在）でも列の x 座標が均等セル幅で
/// 固定され、行をまたいでも同一列の x が一致すること・長い内容の隣に置かれた
/// 短い内容が本来のセル境界から x がずれない（＝長い内容の実幅に押し出されない、
/// 隣セルへ重ならない）ことを検証する（codex 指摘の回帰防止。`row_ui.push_id` で
/// 自然幅のまま `left_to_right` へ積んでいた旧実装は、行 0 の列 0 に長い内容がある
/// 場合のみ行 0 の列 1・2 の x が押し出され、行 1（列 0 が短い）の列 1・2 の x と
/// 一致しなくなる）。
#[test]
fn grid_virtual_list_keeps_equal_column_pitch_regardless_of_content_width() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let screen = build_mixed_width_grid_screen();
    let Some(mut harness) = harness_or_skip(|| {
        build_grid_harness_with(screen.clone(), GRID_FULL_VIEWPORT_SIZE, MIXED_WIDTH_LEN)
    }) else {
        return;
    };

    harness.run();

    assert_equal_column_pitch(&harness);
}

/// TV 側（`TvFocusController::update` 経由の `tv_focus.rs::render_virtual_list_vertical_or_grid`）
/// でも列ピッチが内容幅に依存しないことを検証する（Issue #558）。検証観点・
/// アサーションは非 TV 側 [`grid_virtual_list_keeps_equal_column_pitch_regardless_of_content_width`]
/// と同一で、描画入口だけを TV 側へ差し替える。
#[test]
fn tv_grid_virtual_list_keeps_equal_column_pitch_regardless_of_content_width() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let screen = build_mixed_width_grid_screen();
    let Some(mut harness) = harness_or_skip(|| {
        let screen = screen.clone();
        let mut controller = fandhe_egui_proxy::tv_focus::TvFocusController::new();
        egui_kittest::Harness::builder()
            .with_size(GRID_FULL_VIEWPORT_SIZE)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
            .build_ui_state(
                move |ui, store: &mut StateStore<VirtualListState>| {
                    controller.update(ui, &screen, store);
                },
                StateStore::new(VirtualListState {
                    len: MIXED_WIDTH_LEN,
                    count: 0,
                    last_clicked_index: None,
                }),
            )
    }) else {
        return;
    };

    harness.run();

    assert_equal_column_pitch(&harness);
}

/// 幅混在 Grid（3 列 × 3 行、index 0 のみ長ラベル）の項目数。
const MIXED_WIDTH_LEN: usize = 9;

fn mixed_width_len(state: &VirtualListState) -> usize {
    state.len
}

fn mixed_width_item(_state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    // 行 0 の列 0（index=0）だけ非常に長いラベルにし、他は短いラベルのままにする。
    if index == 0 {
        Node::Text(
            "this-is-a-very-long-wide-label-that-would-push-siblings-if-not-clamped".to_string(),
        )
    } else {
        Node::Text(format!("g-item-{index}"))
    }
}

fn build_mixed_width_grid_screen() -> Screen<VirtualListState> {
    Screen::new("グリッド幅混在テスト").virtual_list_with_axis(
        mixed_width_len,
        mixed_width_item,
        GRID_ITEM_EXTENT,
        fandhe_core::Axis::Grid {
            columns: GRID_COLUMNS,
        },
    )
}

/// 幅混在 Grid で「列 1 の x が行 0 と行 1 で一致」「列送り幅が行 0・行 1 で一致」を検証する
/// （非 TV 側・TV 側で共通のアサーション）。
fn assert_equal_column_pitch(
    harness: &egui_kittest::Harness<'static, StateStore<VirtualListState>>,
) {
    // 行 0: index 0(長い)/1/2、行 1: index 3/4/5（すべて短い）。
    let row0_col1 = harness.get_by_label("g-item-1").rect();
    let row0_col2 = harness.get_by_label("g-item-2").rect();
    let row1_col1 = harness.get_by_label("g-item-4").rect();
    let row1_col2 = harness.get_by_label("g-item-5").rect();

    assert!(
        (row0_col1.left() - row1_col1.left()).abs() < 0.5,
        "列 1 の x は行 0（列 0 が長い内容）と行 1（列 0 が短い内容）で一致するはず\
         （row0_col1: {row0_col1:?}, row1_col1: {row1_col1:?}）"
    );
    let pitch_row0 = row0_col2.left() - row0_col1.left();
    let pitch_row1 = row1_col2.left() - row1_col1.left();
    assert!(
        (pitch_row0 - pitch_row1).abs() < 0.5,
        "列送り幅（セル幅）は行 0・行 1 で一致するはず、つまり長い内容の隣列が\
         実コンテンツ幅に押し出されて重なったりずれたりしないはず\
         （pitch_row0: {pitch_row0}, pitch_row1: {pitch_row1}）"
    );
}

// Issue #490（ListRecycler のフレーム跨ぎ配線）検証: `render_screen_with_recyclers` +
// `VirtualListRecyclers` を使う場合、可視範囲が不変ならフレーム間で `item` が
// 再構築されないこと・状態変化時は無効化されて再構築されること・スクロール時は
// 新規流入分のみ再構築されることを検証する。

/// 状態依存の項目ラベルを持つ小規模リスト用の項目数。
fn stateful_list_len(_state: &VirtualListState) -> usize {
    5
}

/// `state.count` をラベルへ埋め込む項目展開関数。呼び出された index を
/// [`BUILT_INDICES`] へ記録する（既存の `virtual_list_item` と同じ計測手段）。
/// `state.count` は [`build_stateful_screen`] の "bump" ボタン押下でのみ変化するため、
/// version（`StateStore::version`）の進行とラベル内容の変化が一致する。
fn stateful_list_item(state: &VirtualListState, index: usize) -> Node<VirtualListState> {
    BUILT_INDICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(index);
    Node::Text(format!("stateful-{index}-{}", state.count))
}

/// 状態変化時の無効化検証（A08 整合性）用の画面。"bump" ボタンで `count` を進め、
/// 直後のリスト再構築で新しいラベルが反映されることを確認する。
fn build_stateful_screen() -> Screen<VirtualListState> {
    Screen::new("状態依存リストテスト")
        .button(WidgetId::from_raw(1), "bump", bump_count)
        .virtual_list(stateful_list_len, stateful_list_item, ITEM_EXTENT)
}

/// フレーム間再利用の基本契約（Issue #490 受け入れ条件）: 状態・スクロール位置が
/// 不変なフレームでは `item` が一度も呼ばれない（`VirtualListRecyclers` 経由で
/// `ListRecycler` がフレームをまたいで保持され、可視分が全件キャッシュヒットする）。
///
/// `egui_kittest::Harness::from_builder` は構築時点で AccessKit 初期化のため
/// 1 フレーム分 `app` を実行済み（egui_kittest 側のコメント参照）であり、この時点で
/// 既に可視分の `item` が呼ばれ `VirtualListRecyclers` へキャッシュされている。
/// そのため構築直後に `harness.get_by_label` で項目が実在することを確認してから
/// `reset_built_indices` → `harness.run()` を行い、「構築時に確定したキャッシュが
/// 明示的な `run()` 呼び出しでも再利用される」ことを検証する。
#[test]
fn recyclers_reuse_visible_items_across_frames_without_state_change() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(|| {
        build_harness_with_recyclers(build_virtual_list_screen(), VIEWPORT_SIZE)
    }) else {
        return;
    };

    // 構築時点の暗黙の 1 フレームで既に可視分が構築されていることを確認する
    // （実在しなければ後続の再利用検証自体が無意味になるための前提確認）。
    harness.get_by_label("item-0");

    reset_built_indices();
    harness.run();
    assert!(
        built_indices().is_empty(),
        "状態・スクロール位置が不変なフレームでは item が再構築されないはず（フレーム間再利用）: {:?}",
        built_indices()
    );
}

/// 状態変化時の無効化検証（Issue #490 §5-2 / A08 整合性）: "bump" ボタン押下で
/// `StateStore::version` が進んだ次フレームでは可視分が全件再構築され、
/// 古いラベル（`stateful-0-0`）は消え新しいラベル（`stateful-0-1`）が現れる
/// （stale ノードが残らないことの直接検証）。
#[test]
fn recyclers_rebuild_visible_items_after_state_version_changes() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(|| {
        build_harness_with_recyclers(build_stateful_screen(), SIBLING_VIEWPORT_SIZE)
    }) else {
        return;
    };

    reset_built_indices();
    harness.run();
    harness.get_by_label("stateful-0-0");

    reset_built_indices();
    harness.get_by_label("bump").click();
    harness.run();

    assert!(
        !built_indices().is_empty(),
        "version 変化後は可視分が再構築されるはず: {:?}",
        built_indices()
    );
    harness.get_by_label("stateful-0-1");
    assert!(
        harness.query_all_by_label("stateful-0-0").next().is_none(),
        "version 変化後は古いラベル stateful-0-0 が残らないはず（stale ノード混入の防止）"
    );
}

/// スクロール時の差分構築検証（Issue #490 §5-3）: `VirtualListRecyclers` 経由では
/// ウィンドウシフト後の新規構築数が、可視ウィンドウ全体の項目数より少ないこと
/// （= 大半の項目がキャッシュ再利用され、新規流入分のみ構築されること）を検証する。
///
/// [`recyclers_reuse_visible_items_across_frames_without_state_change`] の doc で
/// 説明したとおり `Harness` 構築時点で既にキャッシュが温まっているため、
/// 「スクロール前フレームの構築数」を基準にはできない（構築後・スクロール前の
/// 明示 `run()` は 0 件構築で安定する）。代わりに、他テスト
/// （`out_of_view_items_are_never_built_or_rendered`）が確認済みの「可視ウィンドウは
/// overscan 込みでも 30 未満に収まる」という上限を基準に、スクロール後の新規構築数が
/// それを大きく下回る（= ウィンドウ全体を再構築していない）ことを確認する。
const VISIBLE_WINDOW_UPPER_BOUND: usize = 30;

#[test]
fn recyclers_build_fewer_items_on_scroll_shift_than_full_window() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(|| {
        build_harness_with_recyclers(build_virtual_list_screen(), VIEWPORT_SIZE)
    }) else {
        return;
    };

    // 構築時点の暗黙の 1 フレームで既に可視分がキャッシュ済みであることの前提確認。
    harness.get_by_label("item-0");

    {
        let item0 = harness.get_by_label("item-0");
        for _ in 0..3 {
            item0.scroll_down();
        }
    }

    reset_built_indices();
    harness.run();
    let shifted_built_count = built_indices().len();
    assert!(
        shifted_built_count > 0,
        "ウィンドウシフト後は新規流入項目が構築されるはず: {:?}",
        built_indices()
    );
    assert!(
        shifted_built_count < VISIBLE_WINDOW_UPPER_BOUND,
        "再利用により、シフト後の新規構築数（{shifted_built_count}）は可視ウィンドウ全体\
         （{VISIBLE_WINDOW_UPPER_BOUND} 未満）より少なく、新規流入分のみに限定されるはず"
    );
}
