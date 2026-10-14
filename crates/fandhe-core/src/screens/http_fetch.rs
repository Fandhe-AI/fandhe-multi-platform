//! Phase 8（親 #752）型サンプル「HTTP 取得」（Issue #748）。「取得」ボタン →
//! 取得中表示 → 一覧、失敗時はエラー文言 + 再試行の 3 状態を持つ 1 画面。
//! `FANDHE_PROXY_APP=http_fetch` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs`
//! の `effect` 区画から呼び出される（`screens/mod.rs` の `pub mod http_fetch;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`
//! 「新規画面ファイルの置き場」節の制約どおり）。取得結果は本モジュール外
//! （`fandhe-egui-proxy` の effect ドライバ、`device/http_fetch.rs` 参照）が
//! [`finish_fetch`] を通して注入する契約。
//!
//! # 状態機械
//!
//! ```text
//! Idle --start_fetch--> Requested --(effect が観測)--> [mark_loading 完了] --drain--> Loading
//!   ^                                                                                   |
//!   |                                                                          finish_fetch
//!   +---------------------------- Loaded(items) <---- Ok --------------------------------+
//!   +---------------------------- Failed(msg)   <---- Err -------------------------------+
//!                    (Failed から start_fetch で Requested へ戻れる = 再試行)
//! ```
//!
//! `Requested` → `Loading` を二段階に分けているのは、effect ドライバが
//! `fn(&S, &CompletionSender<S>)` という**関数ポインタで状態を持たない**ため
//! （`fandhe-egui-proxy::app::EffectFn`）。ドライバは `Requested` を見た瞬間に
//! 起動要求を検知するが、自分がすでにスレッドを起動済みかを覚えておけない。
//! そこで「起動済みか」を状態側（`Loading`）で表現し、ドライバは `Loading` へは
//! 一度も自分で遷移させず、`mark_loading` の完了を送るだけにする。完了は
//! `ProxyApp::update` のフレーム先頭（`drain_completions`）で `apply` されるため、
//! 同一フレーム内で `Requested` が 2 回観測されることはなく、二重取得は起きない
//! （フレーム内順序「drain → 描画/dispatch → effect」が契約、`app.rs` 参照）。
//!
//! `Loading` 以外で届いた [`finish_fetch`] は無視する（`Idle` に戻った後や、
//! 前の取得が正しく終わる前に古い完了が届いても状態を上書きしない防御、
//! `docs/ai/SCHEMA.md` パターン 9 の「apply は無条件に実行される」契約を
//! 呼び出し元（本モジュール）側で安全に使うための判定）。

use crate::{Node, Screen, WidgetId};

/// 「取得」ボタンの `WidgetId`。
const FETCH_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「再試行」ボタンの `WidgetId`。
const RETRY_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 取得状態の遷移（モジュール冒頭の状態機械図参照）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FetchPhase {
    /// 初期状態・取得結果のリセット後。
    #[default]
    Idle,
    /// ボタン押下直後。effect ドライバがまだ観測していない可能性がある区間。
    Requested,
    /// effect ドライバがスレッドを起動済み（二重起動防止のマーカー）。
    Loading,
    /// 取得成功（ダミー一覧）。
    Loaded(Vec<String>),
    /// 取得失敗（エラー文言。応答本文は含めない契約、security.md A09）。
    Failed(String),
}

/// HTTP 取得画面が保持する状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpFetchState {
    /// 現在の取得フェーズ。
    pub phase: FetchPhase,
}

/// 「取得」/「再試行」ボタン共通のハンドラ。`Requested | Loading` 中（取得処理中）は
/// 二重取得を防ぐため no-op（panic を API 契約にしない、coding-rust.md）。
pub fn start_fetch(state: &mut HttpFetchState) {
    match state.phase {
        FetchPhase::Idle | FetchPhase::Loaded(_) | FetchPhase::Failed(_) => {
            state.phase = FetchPhase::Requested;
        }
        FetchPhase::Requested | FetchPhase::Loading => {}
    }
}

/// effect ドライバがスレッド起動直後に送る完了（`Requested` → `Loading`）。
/// `Requested` 以外で届いた場合は無視する（モジュール冒頭のコメント参照）。
pub fn mark_loading(state: &mut HttpFetchState) {
    if state.phase == FetchPhase::Requested {
        state.phase = FetchPhase::Loading;
    }
}

/// 取得完了（成功・失敗いずれか）を反映する。`Loading` 以外のときに届いた完了は
/// 古い応答とみなして無視する（モジュール冒頭のコメント参照）。
pub fn finish_fetch(state: &mut HttpFetchState, result: Result<Vec<String>, String>) {
    if state.phase != FetchPhase::Loading {
        return;
    }
    state.phase = match result {
        Ok(items) => FetchPhase::Loaded(items),
        Err(message) => FetchPhase::Failed(message),
    };
}

/// フェーズに応じた案内文（`DynamicText`）。
pub fn status_text(state: &HttpFetchState) -> String {
    match &state.phase {
        FetchPhase::Idle => "ローカルのダミー API から一覧を取得します".to_string(),
        FetchPhase::Requested | FetchPhase::Loading => "取得中…".to_string(),
        FetchPhase::Loaded(items) => format!("{} 件取得", items.len()),
        FetchPhase::Failed(message) => format!("エラー: 取得に失敗しました（{message}）"),
    }
}

/// 一覧区画の展開（`Node::List::expand`）。`Loaded` 以外は空（区画自体が現れない）。
pub fn expand_items(state: &HttpFetchState) -> Vec<Node<HttpFetchState>> {
    match &state.phase {
        FetchPhase::Loaded(items) => items.iter().cloned().map(Node::Text).collect(),
        _ => Vec::new(),
    }
}

/// HTTP 取得画面。フェーズに応じて操作ボタンを組み替える（`Fixed` 画面はボタンを
/// 条件表示できない制約、`screens/confirm_dialog.rs` と同型の `nav` 方式、Issue #726）。
/// `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から
/// `fn(&S) -> Screen<S>` として呼ばれる契約。
pub fn current_screen(state: &HttpFetchState) -> Screen<HttpFetchState> {
    let screen = Screen::new("HTTP 取得")
        .dynamic_text(status_text)
        .list(expand_items);
    match &state.phase {
        FetchPhase::Requested | FetchPhase::Loading => screen,
        FetchPhase::Failed(_) => screen.button(RETRY_BUTTON_ID, "再試行", start_fetch),
        FetchPhase::Idle | FetchPhase::Loaded(_) => {
            screen.button(FETCH_BUTTON_ID, "取得", start_fetch)
        }
    }
}
