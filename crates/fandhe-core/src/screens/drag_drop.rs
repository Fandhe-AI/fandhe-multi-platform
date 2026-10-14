//! Phase 11 の拡充ループ型サンプル「ドラッグ & ドロップ」（親 #1013、Issue #1014）。
//! ウィンドウ全体をドロップ領域とし、Finder/エクスプローラーからファイルを落とすと
//! 名前とサイズの一覧を表示する 1 画面。`FANDHE_PROXY_APP=drag_drop` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod drag_drop;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。実際のホバー/ドロップの
//! 検知・読み取りは本モジュール外（`fandhe-egui-proxy` の `device/drag_drop.rs`）が
//! egui `RawInput::hovered_files`/`dropped_files` を観測し、[`begin_hover`]/
//! [`cancel_hover`]/[`finish_drop`] を呼んで結果を注入する。
//!
//! ドロップはユーザーの操作そのもの（要求 → OS 応答の往復が無い）であるため、
//! `file_pick`/`tsv_import` のような世代番号（`generation`）は持たない。1 フレームに
//! 1 度しか状態を進められない push 型のイベント（egui-winit がドロップ一覧を
//! `take` で消費する）なので、二重発火の心配が無い（`device/drag_drop.rs`
//! モジュール doc 参照）。
//!
//! # 成功・部分失敗・0 件・キャンセルを混同しない理由
//!
//! `file_pick`/`tsv_import` と同じ再発防止方針（契約表 6「嘘の成功」）:
//! フォルダ・8 MiB 超・読めない・複数件中の一部失敗を「受け取りました」に丸めると、
//! ユーザーは自分の落としたファイルのうちどれが失敗したのか区別できない。
//! 一部でも失敗があれば `status`（成功の文言）は空にし、`error`
//! （`TextRole::Error`）へ件数付きの案内を出す。個々の成否は [`expand_items`]
//! の一覧に 1 行ずつ出す。
//!
//! ```text
//! Idle --（ホバー開始、ウィンドウ外から入ってきた）--> begin_hover: hovering = true
//!   --（ウィンドウ外へ出た/取り消された）--> cancel_hover: hovering = false, status=キャンセル
//!   --（ドロップされた）--> finish_drop: hovering = false, items を反映
//! ```

use crate::{Node, Screen};

/// 未操作時の案内。
pub const IDLE_HINT: &str = "ここへファイルをドロップしてください";
/// ホバー中（ドラッグしてウィンドウ上にある）の案内。
pub const HOVER_HINT: &str = "離すと受け取ります";
/// バックグラウンド読み取り中（[`begin_drop_read`] 済みで [`finish_drop_with_overflow`]
/// 未到達）の案内。読み取りに時間がかかる間、前回の成功表示を残さず
/// 処理中であることを示す（codex 指摘 #1018 対応、レビュー指摘 2）。
pub const READING_HINT: &str = "読み込んでいます…";
/// キャンセルされたとき（領域外へ出た/取り消された）の案内。失敗ではない
/// （モジュール doc・契約表 10「キャンセル・正常終了・失敗の三値」参照）。
pub const CANCELLED_STATUS: &str = "ドロップを取り消しました";
/// 失敗時の案内の接頭辞（`{接頭辞}{理由}` の形で表示する。`file_pick` と同じ文言）。
pub const FAILED_PREFIX: &str = "失敗しました: ";

/// ドロップされた 1 件の結果。`result` は成功時のバイト数、失敗時は
/// 表示用の理由文言（パスは含めない、security.md A09）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropItem {
    /// ファイル名（パスを含まない）。
    pub name: String,
    /// 成功: 受け取ったバイト数。失敗: 理由の文言。
    pub result: Result<usize, String>,
}

/// ドラッグ & ドロップ画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DragDropState {
    /// ドラッグ中のファイルがウィンドウ上にあるか。
    pub hovering: bool,
    /// 直近のドロップで受け取った一覧（新しいドロップで丸ごと置き換える）。
    pub items: Vec<DropItem>,
    /// 成功・0 件・キャンセルの案内文言（通常色で表示）。
    pub status: String,
    /// 失敗理由（`TextRole::Error` で表示）。成功・キャンセル時は空文字列。
    pub error: String,
    /// 直近に開始したバックグラウンド読み取りの世代番号（契約表 2「並行性・
    /// 世代」）。`device/drag_drop.rs::drive` がドロップのたびに
    /// [`begin_drop_read`] で確定させ、読み取り完了時に [`is_current_generation`]
    /// で照合する。一致しなければ、後続のドロップに上書きされた古い結果として
    /// 読み取り結果を捨てる（読み取りを別スレッドへ逃がしたことで生じる
    /// 「要求と完了の往復」を成立させるための最小限の世代管理、codex 指摘
    /// #1018 対応）。
    pub generation: u64,
    /// バックグラウンド読み取りが進行中か（契約表 4 相当、レビュー指摘 2）。
    /// [`begin_drop_read`] で `true`、[`finish_drop_with_overflow`] で `false`
    /// に倒す。`true` の間は前回の `items`/`status`/`error` を出さず
    /// [`READING_HINT`] を出す（新しい操作の結果と誤認されるのを防ぐ）。
    pub reading: bool,
}

/// ホバー開始（ドラッグ中のファイルがウィンドウ上に入ってきた）を反映する。
/// 呼び出し元: `device/drag_drop.rs` の effect ドライバ（`RawInput::hovered_files`
/// が空でなく、直前は `hovering == false` だったフレーム）。
pub fn begin_hover(state: &mut DragDropState) {
    state.hovering = true;
}

/// ホバー取り消し（ウィンドウ外へ出た/Esc 等で取り消された）を反映する。
/// `hovering` が既に `false`（そもそもホバーしていなかった）なら何もしない
/// （panic を API 契約にしない、coding-rust.md）。前回までの `items` は
/// 保持したまま（`file_pick` が前回の選択結果を残すのと同じ判断）。
///
/// `state.reading`（[`begin_drop_read`] 済みで読み取りがまだ進行中）の間は
/// `status` を上書きしない。直前のドロップの背景読み取りが続いている最中に
/// 別のホバー（新しいドラッグ）が領域外へ出て取り消されると、進行中の読み取り
/// とは無関係な取り消しのはずが `CANCELLED_STATUS` を書き込んでしまい、
/// [`READING_HINT`]（まだ完了していない）の横に「取り消しました」が並んで
/// 表示され、実際は継続中のドロップがキャンセルされたかのように見える
/// （Cursor Bugbot 指摘、PR #1018 是正）。`hovering` 自体は読み取り中でも
/// 素直に `false` へ戻す。
pub fn cancel_hover(state: &mut DragDropState) {
    if !state.hovering {
        return;
    }
    state.hovering = false;
    if state.reading {
        return;
    }
    state.status = CANCELLED_STATUS.to_string();
    state.error = String::new();
}

/// バックグラウンド読み取りを開始する世代番号を確定する。呼び出し元
/// `device/drag_drop.rs::drive` は、ワーカースレッドを起動する前にこの結果を
/// 完了クロージャ経由で `state.generation` へ同期的に書き込む（ワーカー起動と
/// 世代確定の間に別のドロップが割り込んでも、両者が同じ番号を払い出さない
/// ようにするため）。読み取り本体を UI スレッドから追い出したことで生じる
/// 「要求 → 完了」の往復を成立させる最小限の世代管理（契約表 2、codex 指摘
/// #1018 対応）。
pub fn next_generation(state: &DragDropState) -> u64 {
    state.generation + 1
}

/// [`next_generation`] が返した値を確定させる。ワーカースレッド起動前に
/// 同期で送る完了の中から呼ぶ。
///
/// `hovering` もここで即座に `false` へ倒す（codex 指摘 #1018 対応）。
/// 倒さないと、ドロップ直後の次フレームでは egui-winit のホバー一覧が既に
/// 空（`device/drag_drop.rs` モジュール doc の 0.33.3 挙動）なのに
/// `state.hovering` はまだ `true` のままになり、`device/drag_drop.rs::drive`
/// の「hovering=false かつ state.hovering=true」分岐が発火して
/// [`cancel_hover`] が呼ばれる。バックグラウンド読み取りが終わるまでの間、
/// 本来ドロップ結果が出るはずの画面に「ドロップを取り消しました」が
/// 誤表示される（契約表 1 相当の誤検知、Issue #1014 レビュー指摘）。
///
/// 併せて `reading` を `true` へ倒し、前回の `items`/`status`/`error` を
/// クリアする。読み取りに時間がかかるファイル・フォルダをドロップした場合、
/// クリアしないと完了まで前回の「受け取りました」という結果と一覧が
/// 表示され続け、新しい操作の結果と誤認される（codex 指摘 #1018 対応、
/// レビュー指摘 2）。
pub fn begin_drop_read(state: &mut DragDropState, generation: u64) {
    state.generation = generation;
    state.hovering = false;
    state.reading = true;
    state.items.clear();
    state.status = String::new();
    state.error = String::new();
}

/// `generation` が現在も最新の世代か（契約表 2「古い世代の完了通知を照合して
/// 捨てているか」）。バックグラウンド読み取り完了時にこれで確認し、一致しなければ
/// 後続のドロップに上書きされた古い結果として読み取り結果を捨てる。
pub fn is_current_generation(state: &DragDropState, generation: u64) -> bool {
    state.generation == generation
}

/// ドロップの結果を反映する。`items` が空（受け取れるファイルが 1 件も無かった）
/// 場合は成功に丸めず失敗として扱う（契約表 6）。1 件でも失敗があれば `status`
/// は空にし、`error` へ「{全件数} 件中 {失敗数} 件を受け取れませんでした」を出す
/// （一部失敗を成功色で出さない、モジュール doc 参照）。
///
/// `fandhe_device::drag_drop::MAX_DROP_FILES` 超過が無い通常経路向けの薄い
/// ラッパー（`overflow = 0`）。超過分を 1 件へ集約して渡す場合は
/// [`finish_drop_with_overflow`] を使う。
pub fn finish_drop(state: &mut DragDropState, items: Vec<DropItem>) {
    finish_drop_with_overflow(state, items, 0);
}

/// [`finish_drop`] の本体。`overflow` は `device/drag_drop.rs::read_all` が
/// 上限超過分を 1 件の集約 [`DropItem`] へまとめた際の実件数（集約 1 件は
/// 既に `items` に含まれているため、そのまま合算すると集約行の 1 件分が
/// 二重計上される。`overflow` から 1 を引いた分だけ上乗せする）。総件数・
/// 失敗件数の表示が実数より少なくなるのを防ぐ（codex 指摘 #1018 対応。
/// 例: 532 件ドロップし 500 件が上限超過（残り 32 件は全件成功）なら
/// 「532 件中 500 件を受け取れませんでした」と正しく出る）。
pub fn finish_drop_with_overflow(state: &mut DragDropState, items: Vec<DropItem>, overflow: usize) {
    state.hovering = false;
    state.reading = false;
    if items.is_empty() {
        state.items.clear();
        state.status = String::new();
        state.error = format!("{FAILED_PREFIX}受け取れるファイルがありませんでした");
        return;
    }

    // overflow > 0 のときは items 末尾の集約行が既に 1 件分（総件数・失敗件数
    // 双方）を計上済みなので、実際の超過件数との差分だけを上乗せする。
    let extra = overflow.saturating_sub(1);
    let total = items.len() + extra;
    let failed = items.iter().filter(|item| item.result.is_err()).count() + extra;
    if failed == 0 {
        state.status = format!("{total} 件受け取りました");
        state.error = String::new();
    } else {
        state.status = String::new();
        state.error = format!("{FAILED_PREFIX}{total} 件中 {failed} 件を受け取れませんでした");
    }
    state.items = items;
}

/// 案内文言（ホバー状態に応じて切り替わる、`DynamicText` の `get`）。
pub fn hint_text(state: &DragDropState) -> String {
    if state.hovering {
        HOVER_HINT.to_string()
    } else if state.reading {
        READING_HINT.to_string()
    } else {
        IDLE_HINT.to_string()
    }
}

/// 成功・0 件・キャンセルの案内文言（`DynamicText` の `get`）。
pub fn status_text(state: &DragDropState) -> String {
    state.status.clone()
}

/// 失敗時の案内文言（`error_text` の `get`。`TextRole::Error` として描画される）。
pub fn error_text(state: &DragDropState) -> String {
    state.error.clone()
}

/// 受け取った一覧を `Node::Text` 一覧へ展開する（`.list()` の `expand`）。
/// 成功は「{name}（{size} バイト）」、失敗は「{name}: 受け取れませんでした（{理由}）」。
fn expand_items(state: &DragDropState) -> Vec<Node<DragDropState>> {
    state
        .items
        .iter()
        .map(|item| {
            Node::Text(match &item.result {
                Ok(size) => format!("{}（{size} バイト）", item.name),
                Err(reason) => format!("{}: 受け取れませんでした（{reason}）", item.name),
            })
        })
        .collect()
}

/// ドラッグ & ドロップ画面を組み立てる。`registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。ボタンは置かず、ウィンドウ全体を
/// ドロップ領域とする（実装計画 §3.2、ドラッグ中の強調は [`hint_text`] の
/// 文言切り替えで表す）。
pub fn current_screen(_state: &DragDropState) -> Screen<DragDropState> {
    Screen::new("ドラッグ & ドロップ")
        .dynamic_text(hint_text)
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(expand_items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_hover_then_cancel_shows_cancelled_status_without_error() {
        let mut state = DragDropState::default();
        begin_hover(&mut state);
        assert!(state.hovering);
        cancel_hover(&mut state);
        assert!(!state.hovering);
        assert_eq!(state.status, CANCELLED_STATUS);
        assert!(state.error.is_empty());
        assert!(!state.status.contains("受け取りました"));
    }

    #[test]
    fn cancel_hover_is_noop_when_not_hovering() {
        let mut state = DragDropState {
            status: "前回の状態".to_string(),
            ..Default::default()
        };
        cancel_hover(&mut state);
        assert_eq!(state.status, "前回の状態");
    }

    #[test]
    fn finish_drop_all_success_reports_count_without_error() {
        let mut state = DragDropState::default();
        begin_hover(&mut state);
        finish_drop(
            &mut state,
            vec![
                DropItem {
                    name: "a.txt".to_string(),
                    result: Ok(10),
                },
                DropItem {
                    name: "b.txt".to_string(),
                    result: Ok(20),
                },
            ],
        );
        assert_eq!(state.status, "2 件受け取りました");
        assert!(state.error.is_empty());
    }

    #[test]
    fn finish_drop_partial_failure_reports_error_and_keeps_all_items() {
        let mut state = DragDropState::default();
        finish_drop(
            &mut state,
            vec![
                DropItem {
                    name: "a.txt".to_string(),
                    result: Ok(10),
                },
                DropItem {
                    name: "b".to_string(),
                    result: Err("フォルダは受け取れません".to_string()),
                },
            ],
        );
        assert!(state.status.is_empty());
        assert_eq!(
            state.error,
            format!("{FAILED_PREFIX}2 件中 1 件を受け取れませんでした")
        );
        assert_eq!(state.items.len(), 2);
    }

    #[test]
    fn finish_drop_with_overflow_counts_real_overflow_not_the_aggregate_row() {
        // 532 件ドロップし、501 件目以降（32 件）が 1 件の集約行へ畳まれたケース。
        // 総件数・失敗件数とも集約行の 1 件分ではなく実数（32 件）が反映される
        // ことを確認する（codex 指摘 #1018 対応）。
        let mut state = DragDropState::default();
        let overflow = 32;
        finish_drop_with_overflow(
            &mut state,
            vec![
                DropItem {
                    name: "a.txt".to_string(),
                    result: Ok(10),
                },
                DropItem {
                    name: format!("（他 {overflow} 件）"),
                    result: Err("一度に受け取れるのは...".to_string()),
                },
            ],
            overflow,
        );
        assert!(state.status.is_empty());
        assert_eq!(
            state.error,
            format!("{FAILED_PREFIX}33 件中 32 件を受け取れませんでした")
        );
    }

    #[test]
    fn finish_drop_empty_is_reported_as_failure() {
        let mut state = DragDropState::default();
        finish_drop(&mut state, vec![]);
        assert!(state.status.is_empty());
        assert!(!state.error.is_empty());
        assert!(state.items.is_empty());
    }

    #[test]
    fn finish_drop_resets_hovering() {
        let mut state = DragDropState::default();
        begin_hover(&mut state);
        finish_drop(
            &mut state,
            vec![DropItem {
                name: "a.txt".to_string(),
                result: Ok(1),
            }],
        );
        assert!(!state.hovering);
    }

    #[test]
    fn stale_generation_is_detected_after_newer_drop_begins() {
        // `device/drag_drop.rs::drive` は、ワーカースレッドの読み取り完了時に
        // この照合で古い世代（後続のドロップに上書きされた分）の結果を捨てる
        // （契約表 2、codex 指摘 #1018 対応）。
        let mut state = DragDropState::default();
        let first = next_generation(&state);
        begin_drop_read(&mut state, first);
        let second = next_generation(&state);
        begin_drop_read(&mut state, second);

        assert!(!is_current_generation(&state, first));
        assert!(is_current_generation(&state, second));
    }

    #[test]
    fn begin_drop_read_clears_previous_result_and_shows_reading_hint() {
        // レビュー指摘 2: 前回成功した後、読み取りに時間がかかるドロップを
        // 行うと、完了まで前回の結果が表示され続けて新しい操作と誤認される。
        let mut state = DragDropState::default();
        finish_drop(
            &mut state,
            vec![DropItem {
                name: "a.txt".to_string(),
                result: Ok(10),
            }],
        );
        assert_eq!(state.status, "1 件受け取りました");

        let generation = next_generation(&state);
        begin_drop_read(&mut state, generation);

        assert!(state.reading);
        assert!(state.items.is_empty());
        assert!(state.status.is_empty());
        assert!(state.error.is_empty());
        assert_eq!(hint_text(&state), READING_HINT);

        finish_drop(
            &mut state,
            vec![DropItem {
                name: "b.txt".to_string(),
                result: Ok(20),
            }],
        );
        assert!(!state.reading);
    }

    #[test]
    fn cancel_hover_while_reading_does_not_overwrite_reading_hint_with_cancelled_status() {
        // Cursor Bugbot 指摘（PR #1018）: 背景読み取り中に別のホバーが取り消されても、
        // 進行中の読み取りとは無関係な CANCELLED_STATUS を書き込んではいけない。
        let mut state = DragDropState::default();
        let generation = next_generation(&state);
        begin_drop_read(&mut state, generation);
        assert!(state.reading);

        // 読み取り中に新しいホバーが始まり、領域外へ出て取り消されたケース。
        begin_hover(&mut state);
        cancel_hover(&mut state);

        assert!(!state.hovering);
        assert!(state.reading);
        assert!(state.status.is_empty());
        assert_eq!(hint_text(&state), READING_HINT);
    }

    #[test]
    fn hint_text_switches_while_hovering() {
        let mut state = DragDropState::default();
        assert_eq!(hint_text(&state), IDLE_HINT);
        begin_hover(&mut state);
        assert_eq!(hint_text(&state), HOVER_HINT);
    }

    #[test]
    fn expand_items_renders_one_line_per_item_with_reason() {
        let state = DragDropState {
            items: vec![
                DropItem {
                    name: "a.txt".to_string(),
                    result: Ok(5),
                },
                DropItem {
                    name: "b".to_string(),
                    result: Err("フォルダは受け取れません".to_string()),
                },
            ],
            ..Default::default()
        };
        let nodes = expand_items(&state);
        assert_eq!(nodes.len(), 2);
        match &nodes[1] {
            Node::Text(text) => assert!(text.contains("フォルダは受け取れません")),
            other => panic!("Node::Text を期待: {other:?}"),
        }
    }
}
