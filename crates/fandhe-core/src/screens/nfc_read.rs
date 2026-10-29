//! Phase 11（端末機能の拡充ループ、親 #157）型サンプル「NFC タグ読み取り」（Issue #1044）。
//! かざした NFC タグの NDEF を 1 枚読み、レコードの種類と内容を表示する 1 画面。
//! `FANDHE_PROXY_APP=nfc_read` で選ばれ、`crates/fandhe-egui-proxy/src/screens/registry.rs` の
//! `effect` 区画から呼ばれる（`screens/mod.rs` の `pub mod nfc_read;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import せず、時計も持たない）。OS 呼び出しと期限は
//! 本モジュール外（`fandhe-egui-proxy` の `device/nfc.rs`）が [`take_request`] で要求を観測し、
//! [`mark_running`]・[`finish_read`] で状態を進める。Web（`fandhe-web-dom`）も同じ関数と文言を使う。
//! 全対象 OS の分の状態・文言はここで定義するため、OS 別の子 issue はこのファイルを触らない。
//!
//! 取り消しボタンは置かない。中断（`Interrupted`）は失敗の色にしない（契約表 10）。
//! タグの中身・UID・OS のエラー文は `Debug` とエラー文に出さない（契約表 8）。
//!
//! ```text
//! Idle --ボタン--> Requested (generation += 1, 前回の結果を消す)
//!   --effect--> Running
//!   --finish_read（Running かつ世代一致のみ）--> Idle（result に Ok(タグ) か Err(失敗)）
//! ```

use std::fmt;

use crate::{Screen, WidgetId};

const READ_BUTTON_ID: WidgetId = WidgetId::from_raw(1);

/// タイトル。
pub const TITLE: &str = "NFC タグ読み取り";
/// ボタンの文言。
pub const READ_LABEL: &str = "タグを読む";
/// 未操作時の案内。
pub const IDLE_HINT: &str = "NFC タグを 1 枚読んで、中身の種類と内容を表示します（Android と Chrome for Android で使えます。1 枚読んだら終わります。書き込みと UID は扱いません）";
/// 処理中の案内。
pub const RUNNING_STATUS: &str = "タグをかざしてください…";
/// タグが空のときの案内（失敗ではない）。
pub const EMPTY_MESSAGE: &str = "タグは空です";
/// 中断されたときの案内（失敗の色にしない）。
pub const INTERRUPTED_MESSAGE: &str = "中断しました";
/// 失敗時の案内の接頭辞（`{接頭辞}{メッセージ}` の形で表示する）。
pub const FAILED_PREFIX: &str = "読み取れませんでした: ";
/// 読めないタグのときの案内。
pub const UNREADABLE_MESSAGE: &str = "読めないタグです";
/// タグが上限を超えたときの案内。
pub const TOO_LARGE_MESSAGE: &str = "タグの内容が大きすぎます";
/// NFC がオフのときの案内。
pub const DISABLED_MESSAGE: &str = "NFC がオフです";
/// 権限が拒否されたときの案内。
pub const PERMISSION_DENIED_MESSAGE: &str = "許可されていません";
/// 期限切れのときの案内。
pub const TIMED_OUT_MESSAGE: &str = "期限切れです。もう一度お試しください";
/// 非対応のときの案内（接頭辞なし。端末とブラウザの両方を指す）。
pub const UNSUPPORTED_MESSAGE: &str = "この環境では使えません";
/// それ以外の失敗の案内。
pub const FAILED_MESSAGE: &str = "原因不明のエラーが起きました";

/// 表示するレコード（`fandhe_device::nfc::NdefRecordView` から写す責務は effect 側）。
/// `Debug` は中身を出さない。
#[derive(Clone, PartialEq, Eq)]
pub enum ShownRecord {
    /// テキスト。
    Text {
        /// 言語コード。
        lang: String,
        /// 本文。
        text: String,
    },
    /// URL（開かない。文字列として表示するだけ）。
    Uri(String),
    /// MIME メディア。
    Mime {
        /// 型名。
        type_name: String,
        /// バイト数。
        len: usize,
    },
    /// 外部型。
    External {
        /// 型名。
        type_name: String,
        /// バイト数。
        len: usize,
    },
    /// その他。
    Other {
        /// 型名。
        type_name: String,
        /// バイト数。
        len: usize,
    },
}

impl fmt::Debug for ShownRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Text { .. } => "Text",
            Self::Uri(_) => "Uri",
            Self::Mime { .. } => "Mime",
            Self::External { .. } => "External",
            Self::Other { .. } => "Other",
        };
        write!(f, "ShownRecord::{kind}")
    }
}

impl ShownRecord {
    fn line(&self) -> String {
        match self {
            Self::Text { lang, text } => format!("テキスト（{lang}）: {text}"),
            Self::Uri(u) => format!("URL: {u}"),
            Self::Mime { type_name, len } => format!("MIME: {type_name}（{len} バイト）"),
            Self::External { type_name, len } => format!("外部型: {type_name}（{len} バイト）"),
            Self::Other { type_name, len } => format!("その他: {type_name}（{len} バイト）"),
        }
    }
}

/// 読み取りの成功値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShownTag {
    /// 1 件以上のレコード。
    Records(Vec<ShownRecord>),
    /// タグは空。
    Empty,
}

/// 読めなかった理由（`fandhe_device::nfc::NfcReadFailure` から写す責務は effect 側）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreFailure {
    /// 読めないタグ。
    Unreadable,
    /// 上限超過。
    TooLarge,
    /// NFC がオフ。
    Disabled,
    /// 権限の拒否。
    PermissionDenied,
    /// 期限切れ。
    TimedOut,
    /// 途中の中断。
    Interrupted,
    /// 非対応。
    Unsupported,
    /// それ以外。
    Failed,
}

/// 操作の進行状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// 未操作、または完了後。
    #[default]
    Idle,
    /// ボタンが押され、effect ドライバがまだ処理していない。
    Requested,
    /// タグを待っている。
    Running,
}

/// NFC タグ読み取り画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NfcReadState {
    /// 操作の進行状態。
    pub phase: Phase,
    /// 直近に立てた要求の世代番号（完了通知の照合に使う）。
    pub generation: u64,
    /// 直近の結果。未完了なら `None`。
    pub result: Option<Result<ShownTag, CoreFailure>>,
}

fn is_busy(state: &NfcReadState) -> bool {
    state.phase != Phase::Idle
}

/// 要求を立てる。処理中（`Requested` / `Running`）は no-op。前回の結果は消す。
pub fn request_read(state: &mut NfcReadState) {
    if is_busy(state) {
        return;
    }
    state.phase = Phase::Requested;
    state.generation = state.generation.wrapping_add(1);
    state.result = None;
}

/// effect ドライバが未発送の要求の世代を観測する（状態は変えない）。世代は [`finish_read`] へ
/// そのまま渡す。
pub fn take_request(state: &NfcReadState) -> Option<u64> {
    (state.phase == Phase::Requested).then_some(state.generation)
}

/// 要求を処理中へ進める。`Requested` 以外では何もしない。
pub fn mark_running(state: &mut NfcReadState) {
    if state.phase == Phase::Requested {
        state.phase = Phase::Running;
    }
}

/// 結果を反映する。「`Running` かつ同じ世代」のときだけ適用し、それ以外（古い世代・
/// 期限で確定済み・未要求）は無視する。反映したかどうかを返す。
pub fn finish_read(
    state: &mut NfcReadState,
    generation: u64,
    result: Result<ShownTag, CoreFailure>,
) -> bool {
    if state.phase != Phase::Running || state.generation != generation {
        return false;
    }
    state.phase = Phase::Idle;
    state.result = Some(result);
    true
}

/// 状態の案内文言（処理中・読めた・空・中断）。レコードは 1 行ずつ `\n` でつなぐ。
pub fn status_text(state: &NfcReadState) -> String {
    if is_busy(state) {
        return RUNNING_STATUS.to_string();
    }
    match &state.result {
        Some(Ok(ShownTag::Records(rs))) => rs
            .iter()
            .map(ShownRecord::line)
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Ok(ShownTag::Empty)) => EMPTY_MESSAGE.to_string(),
        Some(Err(CoreFailure::Interrupted)) => INTERRUPTED_MESSAGE.to_string(),
        _ => String::new(),
    }
}

/// 失敗時の案内文言（`TextRole::Error` として描画される）。中断は失敗として扱わず空にする。
/// タグの中身・OS の文言は含めない。
pub fn error_text(state: &NfcReadState) -> String {
    let Some(Err(f)) = state.result else {
        return String::new();
    };
    let message = match f {
        CoreFailure::Interrupted => return String::new(),
        CoreFailure::Unsupported => return UNSUPPORTED_MESSAGE.to_string(),
        CoreFailure::Unreadable => UNREADABLE_MESSAGE,
        CoreFailure::TooLarge => TOO_LARGE_MESSAGE,
        CoreFailure::Disabled => DISABLED_MESSAGE,
        CoreFailure::PermissionDenied => PERMISSION_DENIED_MESSAGE,
        CoreFailure::TimedOut => TIMED_OUT_MESSAGE,
        CoreFailure::Failed => FAILED_MESSAGE,
    };
    format!("{FAILED_PREFIX}{message}")
}

/// 画面を組み立てる。ボタンは処理中以外のときだけ出す（再押下を受け付けない）。
pub fn current_screen(state: &NfcReadState) -> Screen<NfcReadState> {
    let screen = Screen::new(TITLE)
        .text(IDLE_HINT)
        .dynamic_text(status_text)
        .error_text(error_text);
    if is_busy(state) {
        return screen;
    }
    screen.button(READ_BUTTON_ID, READ_LABEL, request_read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> NfcReadState {
        let mut s = NfcReadState::default();
        request_read(&mut s);
        mark_running(&mut s);
        s
    }

    fn tag() -> ShownTag {
        ShownTag::Records(vec![
            ShownRecord::Text {
                lang: "ja".into(),
                text: "やあ".into(),
            },
            ShownRecord::Uri("https://a".into()),
            ShownRecord::Mime {
                type_name: "x/y".into(),
                len: 3,
            },
        ])
    }

    const ALL: [CoreFailure; 8] = [
        CoreFailure::Unreadable,
        CoreFailure::TooLarge,
        CoreFailure::Disabled,
        CoreFailure::PermissionDenied,
        CoreFailure::TimedOut,
        CoreFailure::Interrupted,
        CoreFailure::Unsupported,
        CoreFailure::Failed,
    ];

    #[test]
    fn transitions_and_busy_request_ignored() {
        let mut s = NfcReadState::default();
        assert_eq!(take_request(&s), None);
        request_read(&mut s);
        assert_eq!(take_request(&s), Some(1));
        mark_running(&mut s);
        request_read(&mut s);
        assert_eq!(
            (s.phase, s.generation, take_request(&s)),
            (Phase::Running, 1, None)
        );
        assert!(finish_read(&mut s, 1, Ok(tag())));
        request_read(&mut s);
        assert_eq!((s.generation, s.result.is_none()), (2, true));
    }

    #[test]
    fn stale_or_non_running_finish_is_ignored() {
        let mut s = running();
        assert!(!finish_read(&mut s, 99, Ok(tag())));
        assert!(s.result.is_none());
        let mut requested = NfcReadState::default();
        request_read(&mut requested);
        assert!(!finish_read(&mut requested, 1, Ok(tag())));
        // 確定済みの後の 2 回目は上書きしない。
        finish_read(&mut s, 1, Err(CoreFailure::TimedOut));
        assert!(!finish_read(&mut s, 1, Ok(tag())));
        assert_eq!(s.result, Some(Err(CoreFailure::TimedOut)));
    }

    #[test]
    fn interrupted_is_not_an_error() {
        let mut s = running();
        finish_read(&mut s, 1, Err(CoreFailure::Interrupted));
        assert_eq!(error_text(&s), "");
        assert_eq!(status_text(&s), INTERRUPTED_MESSAGE);
    }

    #[test]
    fn empty_tag_is_plain_status() {
        let mut s = running();
        finish_read(&mut s, 1, Ok(ShownTag::Empty));
        assert_eq!(
            (status_text(&s).as_str(), error_text(&s)),
            (EMPTY_MESSAGE, String::new())
        );
    }

    #[test]
    fn every_other_failure_shows_error_only() {
        for f in ALL.into_iter().filter(|f| *f != CoreFailure::Interrupted) {
            let mut s = running();
            finish_read(&mut s, 1, Err(f));
            assert!(!error_text(&s).is_empty(), "{f:?}");
            assert_eq!(status_text(&s), "");
        }
        let mut s = running();
        finish_read(&mut s, 1, Err(CoreFailure::Unsupported));
        assert_eq!(error_text(&s), UNSUPPORTED_MESSAGE);
        let mut s = running();
        finish_read(&mut s, 1, Err(CoreFailure::TimedOut));
        assert!(error_text(&s).contains("期限切れ"));
    }

    #[test]
    fn records_render_one_line_each() {
        let mut s = running();
        finish_read(&mut s, 1, Ok(tag()));
        assert_eq!(
            status_text(&s),
            "テキスト（ja）: やあ\nURL: https://a\nMIME: x/y（3 バイト）"
        );
        assert_eq!(error_text(&s), "");
    }

    #[test]
    fn button_hidden_while_busy() {
        // 固定 3 ノード（案内・状態・失敗）+ ボタン 1 つ
        let count = |s: &NfcReadState| current_screen(s).len() - 3;
        assert_eq!(count(&running()), 0);
        assert_eq!(count(&NfcReadState::default()), 1);
        assert_eq!(status_text(&running()), RUNNING_STATUS);
    }

    #[test]
    fn debug_hides_content() {
        let mut s = running();
        finish_read(
            &mut s,
            1,
            Ok(ShownTag::Records(vec![ShownRecord::Text {
                lang: "xx".into(),
                text: "himitsu".into(),
            }])),
        );
        assert!(!format!("{s:?}").contains("himitsu"));
    }
}
