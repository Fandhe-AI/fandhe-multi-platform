//! TASK-5.5-S4（REQ-5 / #87）: 300 項目規模の性能確認テスト。
//!
//! `docs/architecture/list-virtualization-architecture.md` §3.5 が定義する S4 の
//! 検証観点 2 点をテストケース化する:
//!   1. 展開ノード数がウィンドウ幅（`visible_count + overscan * 2` 程度）に比例し、
//!      総項目数（300 等）に比例しないこと（決定的アサーション、通常の CI で実行）。
//!   2. フレームタイム計測（REQ-5 の 95 パーセンタイル 16.6ms 基準との比較。
//!      `#[ignore]` を付け、手動 release 実行のみで走らせる。debug ビルド・
//!      CI runner の負荷変動による flaky 化を避けるため）。
//!
//! 実測は `--ignored` の release 計測テストで手動実行する。
//!
//! ## テスト構成
//!
//! - **ウィンドウ算出**: `fandhe_core::compute_visible_range`（クレートルート
//!   re-export、Issue #85 のコア実装）をそのまま呼び出す。
//! - **ウィンドウ内ノード生成**: `Node::List::expand` は `fn(&S) -> Vec<Node<S>>`（index を
//!   受け取らない）であるため、仮想化経路は `Screen` / `Node::List` を経由できない。
//!   代わりに `fandhe_core::ui::ListRecycler::acquire`（`Range<usize>` を外部から渡す
//!   自己完結 API、Issue #86）へ [`item_at`] を渡して呼び出す。`ListRecycler` は
//!   `Node::VirtualList` の有無に依存しない。
//! - **再利用統計（`built_count` / `reused_count` / `slot_count`）**: 「スクロール中に
//!   新規流入項目数ぶんのみ `built` が増える」ことを
//!   `scrolling_recycler_reuses_cached_nodes_and_bounds_memory` テストで実際に assert する。
//!
//! ## 全展開経路とウィンドウ経路で「同じ仕事」を測る
//!
//! 全展開（ベースライン）・ウィンドウ限定生成の双方を、同一の
//! [`visit_node_for_work`]（`render_headless` と同じ「ノード種別ごとに文字列を
//! 組み立てて `Vec` へ積む」処理）に通す。こうしないと、計測差分が「仮想化の効果」
//! ではなく「`render_headless` 側の `String` アロケーションの有無」を測ってしまう
//! （両経路が同一の per-node コストを負担する設計であることが、ノード数の差＝
//! 仮想化の効果、という比較の前提となる）。

use fandhe_core::ui::ListRecycler;
use fandhe_core::{Node, Screen, compute_visible_range};

/// 300 項目規模の性能確認テスト用の状態。決定的なデータのみを持つ（乱数不使用）。
struct PerfState {
    items: Vec<String>,
}

impl PerfState {
    /// `count` 件の決定的な項目（`item-0`, `item-1`, ...)を持つ状態を作る。
    fn new(count: usize) -> Self {
        Self {
            items: (0..count).map(|i| format!("item-{i}")).collect(),
        }
    }
}

/// 全展開ベースライン用の `Node::List::expand`。`PerfState` の全項目を毎回
/// `Vec<Node<PerfState>>` として新規生成する（immediate mode のコストそのもの）。
fn expand_all(state: &PerfState) -> Vec<Node<PerfState>> {
    state.items.iter().map(|s| Node::Text(s.clone())).collect()
}

/// ウィンドウ限定生成用の項目生成関数。素の関数ポインタ
/// `fn(&PerfState, usize) -> Node<PerfState>` に限定する（クロージャの環境キャプチャ
/// 禁止。`Node::List::expand` / `Node::Button::on_click` と同じ AI 型安全 DSL の制約、
/// 設計文書 §3.3 の `VirtualList::item` 案と同一契約）。`ListRecycler::acquire` の
/// `expand_item` 引数としてそのまま渡せる契約でもある。
fn item_at(state: &PerfState, index: usize) -> Node<PerfState> {
    match state.items.get(index) {
        Some(s) => Node::Text(s.clone()),
        // index が範囲外でも panic せず安全側の空テキストを返す
        // （coding-rust.md のエラーハンドリング方針、`saturating_*` 系と同じ思想）。
        None => Node::Text(String::new()),
    }
}

/// 全展開ベースライン・ウィンドウ経路の双方が通る共通の per-node コスト。
///
/// `render_headless`（`crates/fandhe-core/src/ui/render.rs`）と同じ
/// 「ノード種別ごとに文字列を組み立てて `out` へ積む」処理を行う。両経路がこの関数
/// のみをコストの発生源として共有することで、計測差分が純粋に「訪問ノード数の差」
/// （＝仮想化の効果）を表すようにする（ファイル冒頭コメント参照）。
fn visit_node_for_work(node: &Node<PerfState>, state: &PerfState, out: &mut Vec<String>) {
    match node {
        Node::Text(text) => out.push(text.clone()),
        Node::DynamicText { get } => out.push(get(state)),
        Node::StyledText { get, .. } => out.push(get(state)),
        Node::TextField { label, get, .. } => out.push(format!("[{}: {}]", label, get(state))),
        Node::Button { label, .. } => out.push(format!("({label})")),
        Node::IndexedButton { label, .. } => out.push(format!("({label})")),
        Node::List { .. } => {}
        // 本テストは `ListRecycler` 経由でウィンドウ経路を測定するため
        // `Node::VirtualList` を直接使わない（本ファイルのスコープ外）。
        Node::VirtualList { .. } => {}
        // 本テストは `Node::Widget` を使わない（本ファイルのスコープ外）。
        Node::Widget(_) => {}
        Node::RgbaImage { .. } => {}
    }
}

/// 検証観点 1a: 全展開ベースラインの訪問ノード数は総項目数に比例する。
///
/// immediate mode（`Screen::visit_expanded` による全件展開）のコストを定量化する
/// ベースライン測定。300 項目なら 300 ノード（+ 固定ヘッダの `Text` 1 件）を訪問する。
#[test]
fn baseline_full_expand_visits_all_items_proportional_to_len() {
    for len in [300usize, 3000usize] {
        let state = PerfState::new(len);
        let screen: Screen<PerfState> = Screen::new("性能確認画面")
            .text("固定ヘッダ")
            .list(expand_all);

        let mut visited = Vec::new();
        screen.visit_expanded(&state, |node| {
            visit_node_for_work(node, &state, &mut visited)
        });

        // 固定ヘッダ 1 件（Text）+ List 自身（無出力）+ 展開された len 件の Text。
        assert_eq!(
            visited.len(),
            len + 1,
            "全展開ベースラインの訪問ノード数は総項目数 {len} に比例するはず"
        );
    }
}

/// 検証観点 1b: ウィンドウ経路の生成ノード数はウィンドウ幅に比例し、総項目数
/// （300 と 3000 の 10 倍差）に依存せず一定であることを assert する
/// （「ウィンドウ幅比例・総項目数非比例」という設計文書 §3.5 の主張そのものの検証）。
///
/// viewport 600px・item_extent 40px・overscan 3 の入力では、
/// `visible_count = ceil(600 / 40) + 1 = 16`、offset = 0 のとき
/// `start = max(0 - 3, 0) = 0`、`end = min(0 + 16 + 3, len) = 19`
/// → ウィンドウ幅 19（`visible_count + overscan` 分。先頭付近は
/// `saturating_sub` によるクランプで overscan 分の非対称が生じるため、
/// 理論上限の `visible_count + overscan * 2 = 22` 以下であることを assert する）。
#[test]
fn windowed_generation_count_is_bounded_by_window_not_by_total_len() {
    const VIEWPORT_EXTENT: f32 = 600.0;
    const ITEM_EXTENT: f32 = 40.0;
    const OVERSCAN: usize = 3;
    const UPPER_BOUND: usize = 16 + OVERSCAN * 2; // visible_count(16) + overscan*2

    let mut window_widths = Vec::new();
    for len in [300usize, 3000usize] {
        let state = PerfState::new(len);
        let range = compute_visible_range(VIEWPORT_EXTENT, 0.0, ITEM_EXTENT, len, OVERSCAN);

        // ウィンドウ内ノード取得は `ListRecycler::acquire`（Issue #86）を経由する。
        // 初回呼び出しのため全件新規構築になるが、取得件数自体はウィンドウ幅に一致する。
        let mut recycler: ListRecycler<PerfState> = ListRecycler::new();
        let nodes = recycler.acquire(range.clone(), 1, &state, item_at);
        let mut visited = Vec::new();
        for node in &nodes {
            visit_node_for_work(node, &state, &mut visited);
        }

        assert!(
            visited.len() <= UPPER_BOUND,
            "ウィンドウ経路の生成ノード数 {} が上限 {} を超えている(len={len})",
            visited.len(),
            UPPER_BOUND
        );
        window_widths.push(range.len());
    }

    // 総項目数が 300→3000 と 10 倍になってもウィンドウ幅は変化しない
    // （「ウィンドウ幅に比例し、総項目数に比例しない」という主張そのものの検証）。
    assert_eq!(
        window_widths[0], window_widths[1],
        "ウィンドウ幅は総項目数に依存せず一定であるべき"
    );
}

/// 検証観点 1c: スクロールシミュレーションでウィンドウ内のライブノード数
/// （1 フレームあたりの生成数）が終始ウィンドウ幅で有界であることを確認する
/// （メモリ有界性）。
#[test]
fn scrolling_keeps_live_node_count_bounded_across_frames() {
    const VIEWPORT_EXTENT: f32 = 600.0;
    const ITEM_EXTENT: f32 = 40.0;
    const OVERSCAN: usize = 3;
    const UPPER_BOUND: usize = 16 + OVERSCAN * 2;
    const LEN: usize = 300;

    // ウィンドウ算出は状態 `S` の値そのものではなく件数（`LEN`）のみに依存するため、
    // 本テストでは状態インスタンスを生成しない（`PerfState` 未使用）。
    let content_extent = LEN as f32 * ITEM_EXTENT;

    // offset を 0 → 末尾まで 40 段階に分けて進める（1,000 フレーム相当の代わりに
    // 決定的な代表フレーム集合を用いる。CI での実行時間を抑えつつ全域を網羅する）。
    let steps = 40;
    for step in 0..=steps {
        let offset = content_extent * (step as f32 / steps as f32);
        let range = compute_visible_range(VIEWPORT_EXTENT, offset, ITEM_EXTENT, LEN, OVERSCAN);

        assert!(
            range.start <= range.end && range.end <= LEN,
            "ウィンドウは 0..len を超えてはならない(offset={offset}, range={range:?})"
        );
        assert!(
            range.len() <= UPPER_BOUND,
            "フレーム(offset={offset})のライブノード数 {} が上限 {} を超えている",
            range.len(),
            UPPER_BOUND
        );
    }
}

/// 検証観点 1e: `ListRecycler`（Issue #86）を通したスクロールシミュレーションで、
/// 「新規構築（`built_count`）は新たに可視域へ入った項目数ぶんのみ増加し、既存項目は
/// 再利用（`reused_count`）される」こと、および `slot_count` がウィンドウ幅で有界に
/// 保たれることを assert する（設計文書 §3.5 が S3 へ引き継いだ検証観点そのもの）。
#[test]
fn scrolling_recycler_reuses_cached_nodes_and_bounds_memory() {
    const VIEWPORT_EXTENT: f32 = 600.0;
    const ITEM_EXTENT: f32 = 40.0;
    const OVERSCAN: usize = 3;
    const UPPER_BOUND: usize = 16 + OVERSCAN * 2;
    const LEN: usize = 300;
    // 状態バージョンは固定（= スクロール中に状態 `S` 自体は変化しない想定）。
    const VERSION: u64 = 1;

    let state = PerfState::new(LEN);
    let content_extent = LEN as f32 * ITEM_EXTENT;
    let mut recycler: ListRecycler<PerfState> = ListRecycler::new();

    let steps = 40;
    for step in 0..=steps {
        let offset = content_extent * (step as f32 / steps as f32);
        let range = compute_visible_range(VIEWPORT_EXTENT, offset, ITEM_EXTENT, LEN, OVERSCAN);

        recycler.acquire(range, VERSION, &state, item_at);

        assert!(
            recycler.slot_count() <= UPPER_BOUND,
            "フレーム(step={step})の slot_count {} が上限 {} を超えている",
            recycler.slot_count(),
            UPPER_BOUND
        );
    }

    // スクロールは単調に前進する（offset が 0 → 末尾まで単調増加）ため、各項目
    // index は生涯で高々 1 回だけ新規にウィンドウへ入り、以降は再利用され続ける。
    // したがって累計新規構築数（built_count）の理論上限は「総項目数 LEN（各 index が
    // 最大 1 回だけ built される）」に収まるはずである（初回フレームのウィンドウ幅は
    // LEN に含まれる index の一部であり、二重に加算すべきではない）。この上限は
    // 「毎フレームウィンドウ全体を再構築する」場合のコスト（300 項目 × 41 フレーム
    // = 12,300）よりはるかに小さく、上限を超えていれば再利用が機能していないことを
    // 検知できる（実測値: built=300、これは「各 index がちょうど 1 回だけ built
    // される」という理論どおりの最小値そのもの）。
    let full_reexpand_cost = LEN as u64 * (steps as u64 + 1);
    let reuse_upper_bound = LEN as u64;
    assert!(
        recycler.built_count() <= reuse_upper_bound,
        "built_count({}) が再利用契約の理論上限({reuse_upper_bound})を超えている\
         （全展開コスト {full_reexpand_cost} に近づくほど再利用が機能していないことを示す）",
        recycler.built_count()
    );
    // 再利用（reused_count）が実際に発生していること（初回フレーム以降、隣接フレーム間の
    // 重複ウィンドウ分は必ず再利用される）を確認する。
    assert!(
        recycler.reused_count() > 0,
        "スクロール中に一度も再利用が発生していない"
    );
}

/// 検証観点 1d: 設計文書 §3.2 の境界条件表のうち S4 に関わるものを確認する
/// （詳細網羅は S2 のユニットテストのスコープのため、ここでは代表的な境界のみ）。
#[test]
fn window_computation_handles_boundary_conditions_without_panicking() {
    // 空リスト: 空範囲を返す。
    assert_eq!(compute_visible_range(600.0, 0.0, 40.0, 0, 3), 0..0);

    // 末尾超過: 終端が len でクランプされる。
    let range = compute_visible_range(600.0, 100_000.0, 40.0, 300, 3);
    assert!(range.end <= 300, "終端は総項目数 300 を超えてはならない");

    // viewport がコンテンツ全高より大きい: 可視範囲は 0..len 全体になる。
    let range = compute_visible_range(100_000.0, 0.0, 40.0, 10, 3);
    assert_eq!(range, 0..10);

    // item_extent = 0: ゼロ除算を回避し、panic しない(空範囲を返す安全側の挙動)。
    let range = compute_visible_range(600.0, 0.0, 0.0, 300, 3);
    assert_eq!(range, 0..0);

    // 負のオフセット(浮動小数点の丸め等に由来): first を 0 にクランプする。
    let range = compute_visible_range(600.0, -50.0, 40.0, 300, 3);
    assert_eq!(range.start, 0);
}

/// 検証観点 2: フレームタイム計測(REQ-5 の 95 パーセンタイル 16.6ms 基準との比較)。
///
/// release ビルドでの手動実測用。通常の CI(debug ビルド)では実行しない
/// (debug 計測値・CI runner の負荷変動による flaky 化を避けるため。
/// CI では上記の決定的アサーション(検証観点 1a〜1d)のみを回す)。
///
/// 実行コマンド: `cargo test -p fandhe-core --release -- --ignored --nocapture`
///
/// 本計測はコアロジック(ノード生成 + `visit_node_for_work` 相当の走査)のみの
/// ヘッドレス計測であり、REQ-5 の最終合否(iOS/Android 実機・固定 60Hz)の代替では
/// ない。egui 描画込みの計測は S5(#88)以降・実機検証(人間担当)の領域である。
#[test]
#[ignore = "release ビルドでの手動実測用"]
fn frame_time_headless_measurement_release_only() {
    use std::time::Instant;

    const LEN: usize = 300;
    const VIEWPORT_EXTENT: f32 = 600.0;
    const ITEM_EXTENT: f32 = 40.0;
    const OVERSCAN: usize = 3;
    const FRAMES: usize = 1_000;

    let state = PerfState::new(LEN);
    let content_extent = LEN as f32 * ITEM_EXTENT;

    // --- 全展開ベースライン ---
    let screen: Screen<PerfState> = Screen::new("性能確認画面")
        .text("固定ヘッダ")
        .list(expand_all);

    let mut baseline_samples = Vec::with_capacity(FRAMES);
    for _frame in 0..FRAMES {
        let started = Instant::now();
        let mut visited = Vec::new();
        screen.visit_expanded(&state, |node| {
            visit_node_for_work(node, &state, &mut visited)
        });
        std::hint::black_box(&visited);
        baseline_samples.push(started.elapsed());
    }

    // --- ウィンドウ経路(スクロール 1,000 フレーム相当。ListRecycler 経由で
    //     フレーム間の再利用込みの実測値を取る) ---
    let mut recycler: ListRecycler<PerfState> = ListRecycler::new();
    let mut windowed_samples = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES {
        let offset = content_extent * ((frame % FRAMES) as f32 / FRAMES as f32);
        let started = Instant::now();
        let range = compute_visible_range(VIEWPORT_EXTENT, offset, ITEM_EXTENT, LEN, OVERSCAN);
        let nodes = recycler.acquire(range, 1, &state, item_at);
        let mut visited = Vec::new();
        for node in &nodes {
            visit_node_for_work(node, &state, &mut visited);
        }
        std::hint::black_box(&visited);
        windowed_samples.push(started.elapsed());
    }

    let percentile = |samples: &mut [std::time::Duration], p: f64| -> std::time::Duration {
        samples.sort();
        let idx = ((samples.len() as f64 - 1.0) * p).round() as usize;
        samples[idx]
    };

    let mut baseline_sorted = baseline_samples.clone();
    let mut windowed_sorted = windowed_samples.clone();
    let baseline_p50 = percentile(&mut baseline_sorted, 0.50);
    let baseline_p95 = percentile(&mut baseline_sorted, 0.95);
    let windowed_p50 = percentile(&mut windowed_sorted, 0.50);
    let windowed_p95 = percentile(&mut windowed_sorted, 0.95);

    println!("=== TASK-5.5-S4 フレームタイム計測(len={LEN}, frames={FRAMES}) ===");
    println!("全展開ベースライン: p50={baseline_p50:?} p95={baseline_p95:?}");
    println!("ウィンドウ経路      : p50={windowed_p50:?} p95={windowed_p95:?}");
    if windowed_p95.as_nanos() > 0 {
        let ratio = baseline_p95.as_nanos() as f64 / windowed_p95.as_nanos() as f64;
        println!("p95 比(ベースライン / ウィンドウ) = {ratio:.2}倍");
    }

    const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_micros(16_600);
    assert!(
        windowed_p95 < FRAME_BUDGET,
        "ウィンドウ経路の p95({windowed_p95:?})が REQ-5 の 16.6ms 基準を超えている"
    );
}
