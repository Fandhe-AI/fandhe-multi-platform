//! Phase 11 の拡充ループ（親 #102、Issue #416）型サンプル「ファイルの関連付け」。
//! `FANDHE_PROXY_APP=file_association_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod file_association_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない）。OS から届いたファイルの検査・保持は
//! `fandhe_device::file_association`、スナップショットを本モジュールへ注入するのは
//! `fandhe-egui-proxy` の effect ドライバ（`device/file_association.rs`）の責務。
//! Web のエントリポイント（`fandhe-web-dom`）も [`apply_snapshot`] 等を同じ形で呼ぶ。
//!
//! 全 OS 分の状態・文言をここに持つ。Windows はアプリ側の宣言だけでは関連付けを
//! 選べないため、画面の操作でレジストリ登録・解除を行う（`registrable` が立った OS だけ
//! ボタンを出す。要求の受け渡しは `url_scheme_show` と同じ `Phase` / 世代番号。
//! 型は `url_scheme_show` のものを共用し、状態遷移の関数だけ本モジュールに持つ）。
//!
//! 「使えない」「まだ届いていない」「開けなかった」は別の状態で、文言も分ける。
//! 検査で弾いたファイルは中身も名前も表示せず件数だけ出す。パスは出さない。

use crate::screens::background_task::format_utc;
pub use crate::screens::url_scheme_show::{
    Action, ActionFailure, ActionOutcome, Phase, REGISTER_BUTTON_ID, Request,
};
use crate::{Screen, WidgetId};

/// 開けなかった理由（`fandhe_device::file_association::OpenFailure` に対応。device を
/// import しないので画面側で定義する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailureKind {
    /// 上限（8 MiB）を超えた。
    TooLarge,
    /// 読めなかった。
    Unreadable,
    /// フォルダだった。
    Folder,
}

/// 受信状態の表示区分。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FileView {
    /// まだ読み取っていない（effect ドライバが最初の結果を注入するまで）。
    #[default]
    Unread,
    /// この OS では受信できない（失敗ではない）。
    Unsupported,
    /// 受信できる状態だが、まだ何も受け取っていない（通常起動。失敗ではない）。
    Waiting,
    /// 最後に受理したファイル。
    Received {
        /// ファイル名（パスを含まない）。
        name: String,
        /// 全体のサイズ（バイト）。
        size: usize,
        /// 先頭バイト（16 進で表示する。解釈はしない）。
        head: Vec<u8>,
        /// 受信時刻（UNIX ms）。
        received_at_unix_ms: u64,
    },
    /// 検査は通ったが開けなかった。
    Failed(OpenFailureKind),
}

/// 「ファイルの関連付け」画面の状態。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileAssociationShowState {
    view: FileView,
    rejected: u64,
    seq: Option<u64>,
    registrable: bool,
    web_hint: bool,
    phase: Phase,
    generation: u64,
    status: String,
    error: String,
}

/// 「登録を解除する」ボタンの id。
const UNREGISTER_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

impl FileAssociationShowState {
    /// 最後に適用したスナップショットの世代番号（effect ドライバが変化検出に使う）。
    pub fn seq(&self) -> Option<u64> {
        self.seq
    }

    /// 画面からの登録・解除が要る OS として確定済みか。
    pub fn registrable(&self) -> bool {
        self.registrable
    }

    /// 非対応として確定済みか（effect ドライバが一度だけ送るために使う）。
    pub fn is_unsupported(&self) -> bool {
        self.view == FileView::Unsupported
    }
}

/// この OS が画面からの登録・解除を要するか（effect ドライバが一度だけ立てる）。
pub fn set_registrable(state: &mut FileAssociationShowState) {
    state.registrable = true;
}

/// Web の案内（インストール済みアプリから開いたときだけ届く）を出す。
pub fn set_web_hint(state: &mut FileAssociationShowState) {
    state.web_hint = true;
}

fn request(state: &mut FileAssociationShowState, action: Action) {
    if state.phase != Phase::Idle {
        return;
    }
    state.phase = Phase::Requested(action);
    state.generation = state.generation.wrapping_add(1);
    state.status = String::new();
    state.error = String::new();
}

/// 「関連付けを登録する」ボタンのハンドラ。処理中は no-op（二重要求の防止）。
pub fn register(state: &mut FileAssociationShowState) {
    request(state, Action::Register);
}

/// 「登録を解除する」ボタンのハンドラ。処理中は no-op。
pub fn unregister(state: &mut FileAssociationShowState) {
    request(state, Action::Unregister);
}

/// effect ドライバが未発送の要求を観測する（状態は変えない）。
pub fn take_request(state: &FileAssociationShowState) -> Option<Request> {
    match state.phase {
        Phase::Requested(action) => Some(Request {
            generation: state.generation,
            action,
        }),
        _ => None,
    }
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_in_flight(state: &mut FileAssociationShowState) {
    if matches!(state.phase, Phase::Requested(_)) {
        state.phase = Phase::InFlight;
    }
}

/// 操作結果を反映する。世代が違う、または要求が無いときの完了は無視する。
/// 失敗は `error` にだけ入れ、成功文言を出さない。既定アプリになったとは書かない。
pub fn finish_action(
    state: &mut FileAssociationShowState,
    generation: u64,
    action: Action,
    result: Result<ActionOutcome, ActionFailure>,
) {
    if state.phase == Phase::Idle || state.generation != generation {
        return;
    }
    state.phase = Phase::Idle;
    match result {
        Ok(ActionOutcome::Done) => {
            state.status = match action {
                Action::Register => "登録しました".to_string(),
                Action::Unregister => "登録を解除しました".to_string(),
            };
        }
        Ok(ActionOutcome::AwaitingConfirmation) => {
            state.status = "登録を要求しました".to_string();
        }
        Err(ActionFailure::Unsupported) => {
            state.error = "この環境では登録できません".to_string();
        }
        Err(ActionFailure::Failed(message)) => state.error = message,
    }
}

/// 登録・解除の結果文言（未操作なら空）。
pub fn registration_text(state: &FileAssociationShowState) -> String {
    state.status.clone()
}

/// 登録・解除の失敗文言（`TextRole::Error` で描画される）。
pub fn error_text(state: &FileAssociationShowState) -> String {
    state.error.clone()
}

/// [`apply_snapshot`] に渡す最新の結果。成功は `(名前, サイズ, 先頭バイト, 受信時刻 UNIX ms)`。
pub type LatestFile = Result<(String, usize, Vec<u8>, u64), OpenFailureKind>;

/// 受信状態のスナップショットを反映する。`latest` は成功なら
/// `Ok((名前, サイズ, 先頭バイト, 受信時刻 UNIX ms))`、失敗なら `Err(理由)`。
pub fn apply_snapshot(
    state: &mut FileAssociationShowState,
    seq: u64,
    latest: Option<LatestFile>,
    rejected: u64,
) {
    state.view = match latest {
        Some(Ok((name, size, head, received_at_unix_ms))) => FileView::Received {
            name,
            size,
            head,
            received_at_unix_ms,
        },
        Some(Err(kind)) => FileView::Failed(kind),
        None => FileView::Waiting,
    };
    state.rejected = rejected;
    state.seq = Some(seq);
}

/// この OS では受信できないことを反映する。
pub fn apply_unsupported(state: &mut FileAssociationShowState) {
    state.view = FileView::Unsupported;
}

/// 先頭バイトを `"68 65 6c"` 形式の 16 進にする（中身は解釈も描画もしない）。
pub fn head_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 状態行の文言。
pub fn status_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Unread => "読み取り中".to_string(),
        FileView::Unsupported => "この OS では未対応です".to_string(),
        FileView::Waiting => {
            "まだ受け取っていません（.txt を「このアプリで開く」「共有」から開くと届きます）"
                .to_string()
        }
        FileView::Received { .. } => "受け取りました".to_string(),
        FileView::Failed(_) => "開けませんでした".to_string(),
    }
}

/// 失敗の理由（失敗以外は空）。
pub fn failure_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Failed(OpenFailureKind::TooLarge) => "大きすぎます（8 MiB まで）".to_string(),
        FileView::Failed(OpenFailureKind::Unreadable) => "読めませんでした".to_string(),
        FileView::Failed(OpenFailureKind::Folder) => "フォルダは開けません".to_string(),
        _ => String::new(),
    }
}

/// OS ごとの案内（該当しなければ空）。
pub fn hint_text(state: &FileAssociationShowState) -> String {
    if state.registrable {
        "「関連付けを登録する」を押してから、エクスプローラーの「プログラムから開く」で選ぶと届きます"
            .to_string()
    } else if state.web_hint {
        "インストール済みのアプリから開いたときだけ届きます".to_string()
    } else {
        String::new()
    }
}

/// 受け取ったファイルの名前（未受信・失敗なら空）。
pub fn name_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Received { name, .. } => format!("名前: {name}"),
        _ => String::new(),
    }
}

/// 受け取ったファイルのサイズ。
pub fn size_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Received { size, .. } => format!("サイズ: {size} バイト"),
        _ => String::new(),
    }
}

/// 先頭 16 バイトの 16 進表示。
pub fn head_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Received { head, .. } => format!("先頭 16 バイト: {}", head_hex(head)),
        _ => String::new(),
    }
}

/// 受信時刻。
pub fn time_text(state: &FileAssociationShowState) -> String {
    match &state.view {
        FileView::Received {
            received_at_unix_ms,
            ..
        } => format!("受信時刻: {}", format_utc(*received_at_unix_ms)),
        _ => String::new(),
    }
}

/// 検査で弾いた件数の文言。
pub fn rejected_text(state: &FileAssociationShowState) -> String {
    format!("検査で弾いた件数: {}", state.rejected)
}

/// 「ファイルの関連付け」画面。
pub fn file_association_show_screen() -> Screen<FileAssociationShowState> {
    Screen::new("ファイルの関連付け")
        .dynamic_text(status_text)
        .error_text(failure_text)
        .dynamic_text(hint_text)
        .dynamic_text(name_text)
        .dynamic_text(size_text)
        .dynamic_text(head_text)
        .dynamic_text(time_text)
        .dynamic_text(rejected_text)
        .dynamic_text(registration_text)
        .error_text(error_text)
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 登録ボタンは登録が要る OS（`registrable`）で、処理中でないときだけ出す。
pub fn current_screen(state: &FileAssociationShowState) -> Screen<FileAssociationShowState> {
    let screen = file_association_show_screen();
    if !state.registrable || state.phase != Phase::Idle {
        return screen;
    }
    screen
        .button(REGISTER_BUTTON_ID, "関連付けを登録する", register)
        .button(UNREGISTER_BUTTON_ID, "登録を解除する", unregister)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn render(s: &FileAssociationShowState) -> String {
        render_headless(&current_screen(s), s).join("\n")
    }

    #[test]
    fn unsupported_waiting_and_failed_differ() {
        let mut un = FileAssociationShowState::default();
        apply_unsupported(&mut un);
        let mut wait = FileAssociationShowState::default();
        apply_snapshot(&mut wait, 0, None, 0);
        let mut failed = FileAssociationShowState::default();
        apply_snapshot(&mut failed, 1, Some(Err(OpenFailureKind::Folder)), 0);
        assert_ne!(status_text(&un), status_text(&wait));
        assert_ne!(status_text(&wait), status_text(&failed));
        assert!(un.is_unsupported());
        assert_eq!(wait.seq(), Some(0));
        // 通常起動（ファイルなし）は失敗ではない。
        assert_eq!(failure_text(&wait), "");
        assert!(!render(&wait).contains("開けませんでした"));
    }

    #[test]
    fn received_shows_fields_without_path() {
        let mut s = FileAssociationShowState::default();
        apply_snapshot(
            &mut s,
            3,
            Some(Ok(("memo.txt".into(), 19, b"hel".to_vec(), 0))),
            2,
        );
        let out = render(&s);
        assert!(out.contains("受け取りました"));
        assert!(out.contains("memo.txt"));
        assert!(out.contains("19 バイト"));
        assert!(out.contains("68 65 6c"));
        assert!(out.contains("1970-01-01 00:00:00 UTC"));
        assert!(out.contains("検査で弾いた件数: 2"));
        assert!(!out.contains('/'));
    }

    #[test]
    fn failed_shows_reason_not_success() {
        for (kind, text) in [
            (OpenFailureKind::TooLarge, "大きすぎます"),
            (OpenFailureKind::Unreadable, "読めませんでした"),
            (OpenFailureKind::Folder, "フォルダは開けません"),
        ] {
            let mut s = FileAssociationShowState::default();
            apply_snapshot(&mut s, 1, Some(Err(kind)), 0);
            let out = render(&s);
            assert!(!out.contains("受け取りました"));
            assert!(out.contains(text));
            assert_eq!(name_text(&s), "");
        }
    }

    #[test]
    fn buttons_only_when_registrable_and_idle() {
        let mut s = FileAssociationShowState::default();
        assert!(!render(&s).contains("登録を解除する"));
        set_registrable(&mut s);
        assert!(render(&s).contains("登録を解除する"));
        assert!(render(&s).contains("プログラムから開く"));
        register(&mut s);
        assert!(!render(&s).contains("登録を解除する"));
        mark_in_flight(&mut s);
        assert!(!render(&s).contains("登録を解除する"));
    }

    #[test]
    fn web_hint_only_when_set() {
        let mut s = FileAssociationShowState::default();
        assert_eq!(hint_text(&s), "");
        set_web_hint(&mut s);
        assert!(hint_text(&s).contains("インストール済み"));
    }

    #[test]
    fn stale_and_duplicate_completions_are_ignored_and_failure_keeps_status_empty() {
        let mut s = FileAssociationShowState::default();
        // 要求なしの完了は無視。
        finish_action(&mut s, 0, Action::Register, Ok(ActionOutcome::Done));
        assert_eq!(registration_text(&s), "");
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        mark_in_flight(&mut s);
        assert_eq!(take_request(&s), None);
        // 世代違いは無視。
        finish_action(
            &mut s,
            req.generation + 1,
            req.action,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(s.phase, Phase::InFlight);
        // 権限拒否などの失敗は status を空のままにする。
        finish_action(
            &mut s,
            req.generation,
            req.action,
            Err(ActionFailure::Failed("失敗しました".into())),
        );
        assert_eq!(registration_text(&s), "");
        assert_eq!(error_text(&s), "失敗しました");
        // 重複した完了は無視。
        finish_action(&mut s, req.generation, req.action, Ok(ActionOutcome::Done));
        assert_eq!(registration_text(&s), "");
    }

    #[test]
    fn done_text_never_claims_default_app() {
        let mut s = FileAssociationShowState::default();
        register(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            Action::Register,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(registration_text(&s), "登録しました");
        assert!(!registration_text(&s).contains("既定"));
        unregister(&mut s);
        let req = take_request(&s).expect("要求がある");
        finish_action(
            &mut s,
            req.generation,
            Action::Unregister,
            Ok(ActionOutcome::Done),
        );
        assert_eq!(registration_text(&s), "登録を解除しました");
    }

    #[test]
    fn head_hex_format() {
        assert_eq!(head_hex(b"hel"), "68 65 6c");
        assert_eq!(head_hex(&[]), "");
    }
}
