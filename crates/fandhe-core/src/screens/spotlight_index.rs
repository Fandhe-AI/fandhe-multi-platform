//! Phase 11（端末機能の拡充ループ、親 #117）型サンプル「OS の検索への登録」（Issue #560）。
//! 固定のメモ 1 件を OS の検索へ登録・自ドメインの全削除をする 1 画面。OS の検索結果から開かれたら
//! `fandhe://spotlight/<id>` として届く URL の id を表示する（Android は受信経路なし）。
//! `FANDHE_PROXY_APP=spotlight_index` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod spotlight_index;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/spotlight_index.rs`）が [`take_request`] で要求を
//! 観測し、[`mark_running`]・[`finish`]・[`apply_opened`] で状態を進める。全対象 OS で同じ UI・文言を
//! 使うため、OS 別の子 issue はこのファイルを触らない。
//!
//! ```text
//! Idle/Done/Failed --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish（Running かつ世代一致のみ）--> Done | Failed
//! ```

use crate::{Node, Screen, WidgetId};

const INDEX_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
const REMOVE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 登録する固定メモの項目 id。
pub const ITEM_ID: &str = "demo-memo";
/// 登録する固定メモの題名。
pub const ITEM_TITLE: &str = "Fandhe デモメモ";
/// 登録する固定メモの本文。
pub const ITEM_TEXT: &str = "OS の検索から開けるデモ用のメモです";
/// OS の検索から開いたときに届く URL の接頭辞（`url_scheme` の入口を使う）。
pub const URL_PREFIX: &str = "fandhe://spotlight/";

/// 未操作時の案内。
pub const IDLE_HINT: &str = "登録すると OS の検索に固定のメモが 1 件出ます。開くとここに項目の id を表示します（Android は表示しません）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "処理しています…";
/// 登録成功の案内。
pub const INDEXED_MESSAGE: &str = "登録しました（検索に出るまで時間がかかることがあります）";
/// 全削除成功の案内。
pub const REMOVED_MESSAGE: &str = "削除しました（検索から消えるまで時間がかかることがあります）";
/// 失敗時の案内の接頭辞。
pub const FAILED_PREFIX: &str = "失敗しました: ";
/// 非対応 OS のときの案内。
pub const UNSUPPORTED_MESSAGE: &str = "この端末では使えません";
/// 期限切れのときの案内。
pub const TIMEOUT_MESSAGE: &str = "時間内に終わりませんでした（OS 側では済んでいることがあります）";
/// それ以外の失敗の固定文言（OS の文言は出さない）。
pub const FAILED_MESSAGE: &str = "登録または削除できませんでした";
/// 開いた id の表示の接頭辞。
pub const OPENED_PREFIX: &str = "検索から開いた項目: ";

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    /// 固定メモを登録する。
    #[default]
    Index,
    /// 自ドメインの項目を全削除する。
    RemoveAll,
}

/// 失敗の種別（`fandhe_device::DeviceError` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFailure {
    /// 非対応 OS・OS が古い。
    Unsupported,
    /// 期限切れ。
    Timeout,
    /// それ以外。
    Failed,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// OS の応答待ち。
    Running,
    /// 成功。
    Done,
    /// 失敗。
    Failed(IndexFailure),
}

/// 画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpotlightIndexState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の要求の種類。
    pub action: Action,
    /// OS の検索から開かれた項目の id。
    pub opened: Option<String>,
    /// 最後に反映した URL 受信の通し番号（ドライバが変化を検出するために使う）。
    pub inbox_seq: Option<u64>,
}

fn is_busy(state: &SpotlightIndexState) -> bool {
    matches!(state.phase, Phase::Requested | Phase::Running)
}

/// 要求を立てる。処理中は no-op。前回の結果は消す。
pub fn request(state: &mut SpotlightIndexState, action: Action) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.action = action;
}

fn request_index(state: &mut SpotlightIndexState) {
    request(state, Action::Index);
}

fn request_remove(state: &mut SpotlightIndexState) {
    request(state, Action::RemoveAll);
}

/// effect ドライバが未発送の要求の `(世代, 種類)` を観測する（状態は変えない）。
pub fn take_request(state: &SpotlightIndexState) -> Option<(u64, Action)> {
    (state.phase == Phase::Requested).then_some((state.generation, state.action))
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut SpotlightIndexState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み）は無視する。
pub fn finish(state: &mut SpotlightIndexState, generation: u64, result: Result<(), IndexFailure>) {
    if state.phase != Phase::Running || state.generation != generation {
        return;
    }
    state.phase = match result {
        Ok(()) => Phase::Done,
        Err(f) => Phase::Failed(f),
    };
}

/// 受信した URL を反映する。`seq` は常に記録し、`URL_PREFIX` + 妥当な id のときだけ表示を更新する
/// （それ以外の URL・不正な id は表示を変えない）。
pub fn apply_opened(state: &mut SpotlightIndexState, seq: u64, url: Option<String>) {
    state.inbox_seq = Some(seq);
    if let Some(id) = url.as_deref().and_then(|u| u.strip_prefix(URL_PREFIX)) {
        if is_valid_id(id) {
            state.opened = Some(id.to_string());
        }
    }
}

/// 表示してよい id か（`[A-Za-z0-9._-]` の 1〜64 文字）。
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// 処理中・成功の案内文言。
pub fn status_text(state: &SpotlightIndexState) -> String {
    match (state.phase, state.action) {
        (Phase::Requested | Phase::Running, _) => RUNNING_STATUS.to_string(),
        (Phase::Done, Action::Index) => INDEXED_MESSAGE.to_string(),
        (Phase::Done, Action::RemoveAll) => REMOVED_MESSAGE.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。
pub fn error_text(state: &SpotlightIndexState) -> String {
    let Phase::Failed(f) = state.phase else {
        return String::new();
    };
    let message = match f {
        IndexFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        IndexFailure::Timeout => TIMEOUT_MESSAGE,
        IndexFailure::Failed => FAILED_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

fn opened_nodes(state: &SpotlightIndexState) -> Vec<Node<SpotlightIndexState>> {
    match &state.opened {
        Some(id) => vec![Node::Text(format!("{OPENED_PREFIX}{id}"))],
        None => Vec::new(),
    }
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す。
pub fn current_screen(state: &SpotlightIndexState) -> Screen<SpotlightIndexState> {
    let screen = Screen::new("OS の検索への登録")
        .text(IDLE_HINT)
        .text(format!("固定のメモ: {ITEM_TITLE}（id: {ITEM_ID}）"))
        .dynamic_text(status_text)
        .error_text(error_text)
        .list(opened_nodes);
    if is_busy(state) {
        return screen;
    }
    screen
        .button(INDEX_BUTTON_ID, "登録する", request_index)
        .button(REMOVE_BUTTON_ID, "全部削除する", request_remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> SpotlightIndexState {
        let mut s = SpotlightIndexState::default();
        request(&mut s, Action::Index);
        mark_running(&mut s);
        s
    }

    #[test]
    fn request_ignored_while_busy_and_clears_previous_result() {
        let mut s = SpotlightIndexState::default();
        request(&mut s, Action::Index);
        assert_eq!(take_request(&s), Some((1, Action::Index)));
        request(&mut s, Action::RemoveAll);
        assert_eq!((s.generation, s.action), (1, Action::Index));
        mark_running(&mut s);
        request(&mut s, Action::RemoveAll);
        assert_eq!(s.generation, 1);
        finish(&mut s, 1, Ok(()));
        assert_eq!(s.phase, Phase::Done);
        request(&mut s, Action::RemoveAll);
        assert_eq!(s.generation, 2);
        assert_eq!(status_text(&s), RUNNING_STATUS);
    }

    #[test]
    fn stale_and_non_running_finish_are_ignored() {
        let mut s = running();
        finish(&mut s, 0, Ok(()));
        assert_eq!(s.phase, Phase::Running);
        finish(&mut s, 1, Err(IndexFailure::Timeout));
        // 期限で確定済みの後に届いた成功は適用しない
        finish(&mut s, 1, Ok(()));
        assert_eq!(s.phase, Phase::Failed(IndexFailure::Timeout));
        let mut idle = SpotlightIndexState::default();
        finish(&mut idle, 0, Ok(()));
        assert_eq!(idle.phase, Phase::Idle);
    }

    #[test]
    fn every_failure_has_nonempty_text() {
        for f in [
            IndexFailure::Unsupported,
            IndexFailure::Timeout,
            IndexFailure::Failed,
        ] {
            let mut s = running();
            finish(&mut s, 1, Err(f));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "");
        }
    }

    #[test]
    fn opened_accepts_only_valid_ids() {
        let mut s = SpotlightIndexState::default();
        for bad in [
            "fandhe://other/demo-memo",
            "fandhe://spotlight/",
            "fandhe://spotlight/a/b",
            "fandhe://spotlight/a b",
            "fandhe://spotlight/<script>",
        ] {
            apply_opened(&mut s, 1, Some(bad.into()));
            assert_eq!(s.opened, None, "{bad}");
        }
        let long = format!("{URL_PREFIX}{}", "a".repeat(65));
        apply_opened(&mut s, 2, Some(long));
        assert_eq!(s.opened, None);
        apply_opened(&mut s, 3, None);
        assert_eq!(s.opened, None);
        apply_opened(&mut s, 4, Some("fandhe://spotlight/demo-memo".into()));
        assert_eq!(s.opened.as_deref(), Some("demo-memo"));
        assert_eq!(s.inbox_seq, Some(4));
        // 不正な URL は直前の表示を変えない
        apply_opened(&mut s, 5, Some("fandhe://other/x".into()));
        assert_eq!(s.opened.as_deref(), Some("demo-memo"));
    }

    #[test]
    fn buttons_hidden_while_busy() {
        // 固定 5 ノード（案内・メモ・状態・失敗・開いた id）+ ボタン 2 つ
        let count = |s: &SpotlightIndexState| current_screen(s).len() - 5;
        let mut s = SpotlightIndexState::default();
        assert_eq!(count(&s), 2);
        request(&mut s, Action::Index);
        assert_eq!(count(&s), 0);
        mark_running(&mut s);
        assert_eq!(count(&s), 0);
        finish(&mut s, 1, Err(IndexFailure::Timeout));
        assert_eq!(count(&s), 2);
    }
}
