//! ローカル通知の抽象（Phase 11、親 #1003、Issue #1004）。
//!
//! `Share`/`Tts` と同じ request/response の一発呼び出し型（1 回の `notify()` = OS へ
//! 通知の表示を 1 回依頼する）。継続 push の共有状態は持たない。プッシュ通知
//! （APNs/FCM）は扱わない（対応表 `docs/device/capabilities/notification.json` の
//! トップレベル `note` 参照。本 Issue はローカル通知のみ）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 結果は 3 値: OS から表示されたことを確かめられた `Ok(Shown)`（本 Issue の
//!   macOS/Windows 実装では返さない。iOS の `willPresent`・Web の `onshow` 等、
//!   確認手段を持つ実装が後続 Issue で使う）/ OS へ渡したが表示されたかは確かめ
//!   られない `Ok(Submitted)`（macOS `osascript`・Windows Toast はこちら）/
//!   権限ダイアログを閉じただけで未決定の `Ok(Cancelled)`（失敗ではない。
//!   `device-bridge-contracts.md` の「10. キャンセル・正常終了・失敗の三値」参照。
//!   本 Issue の macOS/Windows 実装では発生しない）
//! - 失敗は `Err(DeviceError)`。写像は次のとおり
//!   - タイトル・本文の検証違反 → `InvalidInput`（[`NotificationRequest::new`]）
//!   - 利用者が通知を拒否 → `PermissionDenied("notification")`
//!   - OS 全体・グループポリシーで通知が無効 → `Disabled("notification")`
//!   - API・コマンドが無い（Linux、`osascript` 不在等） → `Unsupported("notification")`
//!   - 子プロセス異常終了・期限超過・OS 呼び出し失敗 → `Internal("notification ...")`
//! - タイトル・本文はログにも、エラーの文言にも出さない（security.md A09）
//! - 初期化に失敗した実装は `Unsupported` を返し、ダミーの成功実装へは倒さない
//!   （`device-bridge-contracts.md` の「6. 嘘の成功」参照。[`UnsupportedNotifier`] を使う）

use crate::DeviceError;

/// タイトルの最大長（バイト）。macOS 実装は `osascript` へ標準入力経由で
/// AppleScript ソースを渡す（[`osascript_script`] 参照。argv には出さない、
/// codex 指摘 P1、PR #1008）が、通知の実用上十分な長さとして OS 標準的な
/// 通知のタイトル欄の目安を採用し、際限なく長い入力は事前に弾く。
const TITLE_MAX_LEN: usize = 256;

/// 本文の最大長（バイト）。[`TITLE_MAX_LEN`] と同じ理由。通知本文は複数行を
/// 想定するため、タイトルより広く確保する。
const BODY_MAX_LEN: usize = 4096;

/// 通知の結果。[`Notifier::notify`] の契約（モジュール doc）を参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyOutcome {
    /// OS から実際に表示されたことを確かめられた（本 Issue の実装では返さない）。
    Shown,
    /// OS へ渡したが、表示されたかどうかは確かめられない。
    Submitted,
    /// 利用者が権限ダイアログを閉じるだけで未決定のまま終わった（失敗ではない）。
    Cancelled,
}

/// 検証済みの通知内容。[`NotificationRequest::new`] 以外で作れない（検証の SSOT、
/// `fandhe_device::share::ShareRequest::new` と同じ方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRequest {
    title: String,
    body: String,
}

impl NotificationRequest {
    /// `title`・`body` を検証して構築する。
    ///
    /// - `title`: trim 後に空なら `InvalidInput("notification title")`。NUL 混入・
    ///   [`TITLE_MAX_LEN`] 超過も同様
    /// - `body`: 空でもよい（trim 後の空判定はしない）。NUL 混入・[`BODY_MAX_LEN`]
    ///   超過は `InvalidInput("notification body")`
    /// - 値は trim せずそのまま保持する（`device-bridge-contracts.md` の「8. 秘密値」の
    ///   「入力どおりに保存する」方針を通知文にも適用する）
    /// - 長さ上限は際限のない入力による子プロセスへの負荷・スクリプト生成コストを
    ///   避けるための事前検証（codex 指摘、PR #1008）
    pub fn new(title: &str, body: &str) -> Result<Self, DeviceError> {
        if title.trim().is_empty() {
            return Err(DeviceError::InvalidInput("notification title"));
        }
        if title.contains('\0') {
            return Err(DeviceError::InvalidInput("notification title"));
        }
        if title.len() > TITLE_MAX_LEN {
            return Err(DeviceError::InvalidInput("notification title"));
        }
        if body.contains('\0') {
            return Err(DeviceError::InvalidInput("notification body"));
        }
        if body.len() > BODY_MAX_LEN {
            return Err(DeviceError::InvalidInput("notification body"));
        }
        Ok(Self {
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    /// 通知のタイトル（trim 前・NUL 無しを保証）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 通知の本文（空文字列を許容、NUL 無しを保証）。
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// ローカル通知の契約（モジュール doc の「契約」節を参照）。
pub trait Notifier: Send + Sync {
    /// OS へ通知の表示を 1 回依頼する。
    fn notify(&self, request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError>;
}

/// 対応不可プラットフォーム用（Linux、初期化失敗時のフォールバック）。常に
/// `Unsupported("notification")` を返す（ダミーの成功実装へは倒さない契約）。
pub struct UnsupportedNotifier;

impl Notifier for UnsupportedNotifier {
    fn notify(&self, _request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError> {
        Err(DeviceError::Unsupported("notification"))
    }
}

/// macOS 向け `osascript` 起動コマンドの絶対パス。PATH 探索による差し替え
/// （PATH 乗っ取り）を避けるため常に絶対パスで起動する（security.md A03/A05）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const OSASCRIPT_PATH: &str = "/usr/bin/osascript";

/// `osascript` の子プロセスに設けるタイムアウト。この時間を超えても終了しない
/// 場合は kill + wait で回収する（`device-bridge-contracts.md` の「3. 期限・キャンセル」
/// 「9. 失敗時のリソース解放」参照）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `try_wait` のポーリング間隔（`fandhe_device::tts::WAIT_OUTCOME_POLL_INTERVAL` と
/// 同型の考え方。短すぎる busy loop を避けつつ、タイムアウト検知の遅延を体感で
/// 問題にならない範囲に収める）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// AppleScript の二重引用符文字列リテラルへ安全に埋め込めるようエスケープする
/// 純関数。バックスラッシュ（`\` → `\\`）を先に、二重引用符（`"` → `\"`）を
/// 後に置換する（順序を逆にすると二重引用符のエスケープで挿入したバックスラッシュ
/// を再度エスケープしてしまう）。AppleScript の文字列リテラルは生の改行
/// （LF/CR）を含められずコンパイルエラーになるため、改行を含む値は
/// [`applescript_string_literal`] を使う（本関数はセグメント単位のエスケープの
/// み担う）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 任意の文字列を AppleScript の文字列式へ変換する純関数。生の LF/CR を
/// 含む文字列リテラルは `osascript` のコンパイルに失敗する（cursor[bot]
/// 指摘、PR #1008 #discussion_r4135556169）ため、改行位置で分割し
/// AppleScript の `linefeed`（LF）/`return`（CR）定数との連結式
/// （`"a" & linefeed & "b"`）へ組み立て直す。改行を含まない場合は単純な
/// 二重引用符リテラルを返す。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn applescript_string_literal(value: &str) -> String {
    if !value.contains(['\n', '\r']) {
        return format!("\"{}\"", escape_applescript_string(value));
    }
    let mut parts = Vec::new();
    let mut segment = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => {
                parts.push(format!("\"{}\"", escape_applescript_string(&segment)));
                segment.clear();
                parts.push("linefeed".to_string());
            }
            '\r' => {
                parts.push(format!("\"{}\"", escape_applescript_string(&segment)));
                segment.clear();
                parts.push("return".to_string());
            }
            _ => segment.push(ch),
        }
    }
    parts.push(format!("\"{}\"", escape_applescript_string(&segment)));
    parts.join(" & ")
}

/// `osascript` へ標準入力として渡す AppleScript ソースを組み立てる純関数。
/// タイトル・本文は **argv には一切出さない**（`ps`/プロセス一覧から通知内容が
/// 読める機密情報露出を防ぐ、codex 指摘 P1、PR #1008）。スクリプト文字列への
/// 連結によるインジェクションは [`escape_applescript_string`] のエスケープで
/// 防ぐ（security.md A03。シェルコマンドではなく AppleScript の文字列リテラル
/// 文脈への埋め込みのため、シェルメタ文字ではなく `\`/`"` のみが特別な意味を
/// 持つ）。title/body は生の改行を含み得るため、リテラルの組み立ては
/// [`applescript_string_literal`] に委譲する（改行を `linefeed`/`return`
/// 定数との連結式へ変換し、コンパイル失敗を防ぐ）。改行を含む値は
/// `applescript_string_literal` が `"a" & linefeed & "b"` の連結式を返すため、
/// `display notification`/`with title` の各引数を括弧で括ってグルーピングする
/// （cursor Bugbot 指摘、PR #1008 #discussion_r4135838290。括弧なしだと
/// AppleScript のラベル付き引数構文が `&` 連結を単一の引数として解釈できず
/// コンパイルに失敗する）。単純な二重引用符リテラルの場合も括弧は無害。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn osascript_script(title: &str, body: &str) -> String {
    format!(
        "on run\n  display notification ({}) with title ({})\nend run\n",
        applescript_string_literal(body),
        applescript_string_literal(title),
    )
}

/// `osascript display notification` による macOS 実装。スクリプトエディタ名義で
/// 通知が出る（`osascript` を起動するプロセスの既定挙動）。表示されたか・
/// 通知設定が無効かは `osascript` から判定できないため、成功時は常に `Submitted`
/// を返す（`Shown` は返さない。モジュール doc の契約節参照）。
#[derive(Debug)]
pub struct OsascriptNotifier {
    program: std::path::PathBuf,
}

impl OsascriptNotifier {
    /// 既定のパス（[`OSASCRIPT_PATH`]）で構築する。存在しなければ
    /// `Err(Unsupported("notification"))`（ダミーへは倒さない）。
    pub fn new() -> Result<Self, DeviceError> {
        Self::with_program(std::path::PathBuf::from(OSASCRIPT_PATH))
    }

    /// 任意のプログラムパスで構築する（テストから存在しないパス・`/bin/sh` 等の
    /// 代替コマンドを注入するための入口。`fandhe_device::tts::CommandTts::with_program`
    /// と同型）。
    fn with_program(program: std::path::PathBuf) -> Result<Self, DeviceError> {
        if !program.is_file() {
            return Err(DeviceError::Unsupported("notification"));
        }
        Ok(Self { program })
    }
}

impl Notifier for OsascriptNotifier {
    fn notify(&self, request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        // タイトル・本文を argv に出さない（`ps`/プロセス一覧からの機密情報露出を
        // 防ぐ、codex 指摘 P1、PR #1008）。`osascript -` は標準入力から AppleScript
        // ソースを読む契約（Apple 公式ドキュメント）。
        let script = osascript_script(request.title(), request.body());
        let mut child = Command::new(&self.program)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DeviceError::Internal("notification send"))?;

        // 期限は書き込み開始時点から測る。子プロセスが標準入力を読まず、かつ
        // スクリプトがパイプ容量（OS 既定で数十 KB）を超える場合、書き込み側の
        // `write_all` がブロックしうる（codex 指摘 P1、PR #1008
        // #discussion_r4135813630）。`write_all` 自体には期限を渡せないため、
        // 別スレッドで実行し、この期限までに完了しなければ子プロセスを
        // kill してパイプの読み手を失わせ、書き込みのブロックを解除する
        // （kill 後は書き込み側が EPIPE で即座に返る。Rust はデフォルトで
        // SIGPIPE を無視するため、シグナルではなく `Err` として観測できる）。
        let deadline = std::time::Instant::now() + SEND_TIMEOUT;

        // `stdin` は `Stdio::piped()` 指定時に必ず `Some` になる（`std::process` の契約）。
        // 書き込み完了後に drop で明示的に閉じないと、標準入力を読み切る `osascript` が
        // EOF を待ち続けてハングする（`writer` スレッド内の drop がこれを担う）。
        if let Some(stdin) = child.stdin.take() {
            let (tx, rx) = std::sync::mpsc::channel();
            let writer = std::thread::spawn(move || {
                // ブロックを抜けた時点で `stdin` を閉じる（`drop(stdin)` と同じ意味。
                // wasm32 の `ChildStdin` は `Drop` を持たず `clippy::drop_non_drop` に
                // なるため、明示の `drop` ではなくスコープで閉じる）。
                let result = {
                    let mut stdin = stdin;
                    stdin.write_all(script.as_bytes())
                };
                let _ = tx.send(result);
            });

            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(Ok(())) => {
                    // 書き込み完了。`writer` は `stdin` を drop 済みのはずだが、
                    // OS スケジューリングの猶予のため join で確実に合流する。
                    let _ = writer.join();
                }
                Ok(Err(_)) => {
                    // 書き込み失敗（子プロセスが早期終了して破棄されたパイプ等）でも
                    // ハンドルは確保済みのため、期限超過分岐と同じく回収してから返す
                    // （device-bridge-contracts.md「9. 失敗時のリソース解放」）。
                    let _ = writer.join();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeviceError::Internal("notification send"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 期限超過。書き込みがブロックしたまま。子プロセスを kill して
                    // パイプの読み手を失わせ、`writer` スレッドのブロックを解除して
                    // から回収する（device-bridge-contracts.md「9. 失敗時のリソース
                    // 解放」。kill せずに join すると join 自体が無期限に止まりうる）。
                    let _ = child.kill();
                    let _ = writer.join();
                    let _ = child.wait();
                    return Err(DeviceError::Internal("notification timeout"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // 送信側スレッドが panic 等で `tx.send` に届かず終了した異常系。
                    let _ = writer.join();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeviceError::Internal("notification send"));
                }
            }
        }

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(NotifyOutcome::Submitted)
                    } else {
                        Err(DeviceError::Internal("notification send"))
                    };
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        // 期限超過。確保済みの子プロセスを回収してから失敗を返す
                        // （`device-bridge-contracts.md` の「9. 失敗時のリソース解放」）。
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(DeviceError::Internal("notification timeout"));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => {
                    // `try_wait` 自体の失敗（OS 側のプロセス監視エラー等）でも子プロセスは
                    // 残存しうるため、タイムアウト分岐と同じく kill + wait で回収してから
                    // 返す（Bugbot 指摘、PR #1008。`device-bridge-contracts.md` の
                    // 「9. 失敗時のリソース解放」参照。Unix ではゾンビ化を防ぐ）。
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeviceError::Internal("notification send"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_title() {
        let err = NotificationRequest::new("", "body").unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_blank_only_title() {
        let err = NotificationRequest::new("   ", "body").unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_title_with_embedded_nul() {
        let err = NotificationRequest::new("a\0b", "body").unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_rejects_body_with_embedded_nul() {
        let err = NotificationRequest::new("title", "a\0b").unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification body")
        ));
    }

    #[test]
    fn new_rejects_title_over_max_len() {
        let title = "a".repeat(TITLE_MAX_LEN + 1);
        let err = NotificationRequest::new(&title, "body").unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification title")
        ));
    }

    #[test]
    fn new_accepts_title_at_max_len() {
        let title = "a".repeat(TITLE_MAX_LEN);
        assert!(NotificationRequest::new(&title, "body").is_ok());
    }

    #[test]
    fn new_rejects_body_over_max_len() {
        let body = "a".repeat(BODY_MAX_LEN + 1);
        let err = NotificationRequest::new("title", &body).unwrap_err();
        assert!(matches!(
            err,
            DeviceError::InvalidInput("notification body")
        ));
    }

    #[test]
    fn new_accepts_body_at_max_len() {
        let body = "a".repeat(BODY_MAX_LEN);
        assert!(NotificationRequest::new("title", &body).is_ok());
    }

    #[test]
    fn new_accepts_empty_body() {
        let request = NotificationRequest::new("title", "").unwrap();
        assert_eq!(request.title(), "title");
        assert_eq!(request.body(), "");
    }

    #[test]
    fn new_does_not_trim_title_or_body() {
        let request = NotificationRequest::new("  title  ", "  body  ").unwrap();
        assert_eq!(request.title(), "  title  ");
        assert_eq!(request.body(), "  body  ");
    }

    #[test]
    fn unsupported_notifier_returns_unsupported_error() {
        let request = NotificationRequest::new("title", "body").unwrap();
        let err = UnsupportedNotifier.notify(&request).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("notification")));
    }

    #[test]
    fn osascript_notifier_with_nonexistent_program_is_unsupported() {
        let err =
            OsascriptNotifier::with_program(std::path::PathBuf::from("/nonexistent/osascript"))
                .unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported("notification")));
    }

    #[test]
    fn escape_applescript_string_escapes_quote_and_backslash() {
        assert_eq!(
            escape_applescript_string("a\"b\\c"),
            "a\\\"b\\\\c".to_string()
        );
    }

    #[test]
    fn escape_applescript_string_backslash_before_quote_escape() {
        // 順序が逆（`"` を先に `\` を後に置換）だと、`"` の置換で挿入した `\` を
        // 再エスケープしてしまい `\\\"` になる。正しい順序では `\"` のままになる。
        assert_eq!(escape_applescript_string("\""), "\\\"");
    }

    #[test]
    fn osascript_script_embeds_title_and_body_as_escaped_literals() {
        let script = osascript_script("hello", "world");
        assert!(script.contains("display notification (\"world\") with title (\"hello\")"));
    }

    #[test]
    fn osascript_script_neutralizes_quote_injection_in_title_and_body() {
        // タイトル・本文に含まれる `"` がスクリプト文字列リテラルの境界を
        // 抜け出して任意の AppleScript 文を注入できないことを確認する
        // （security.md A03）。
        let script = osascript_script("\" & (do shell script \"id\") & \"", "body");
        assert!(!script.contains("do shell script \"id\""));
        assert!(script.contains("\\\" & (do shell script \\\"id\\\") & \\\""));
    }

    #[test]
    fn applescript_string_literal_without_newline_is_plain_quoted_string() {
        assert_eq!(applescript_string_literal("hello"), "\"hello\"");
    }

    #[test]
    fn applescript_string_literal_splits_lf_into_linefeed_concatenation() {
        assert_eq!(
            applescript_string_literal("line1\nline2"),
            "\"line1\" & linefeed & \"line2\""
        );
    }

    #[test]
    fn applescript_string_literal_splits_cr_into_return_concatenation() {
        assert_eq!(
            applescript_string_literal("line1\rline2"),
            "\"line1\" & return & \"line2\""
        );
    }

    #[test]
    fn osascript_script_embeds_newline_containing_body_as_concatenation_not_raw_newline() {
        // 改行を生のまま二重引用符リテラルへ埋め込むと `osascript` のコンパイルに
        // 失敗する（cursor[bot] 指摘、PR #1008 #discussion_r4135556169）。
        // `display notification` の行に生の LF/CR が含まれないことを確認する。
        let script = osascript_script("title", "line1\nline2");
        let notification_line = script
            .lines()
            .find(|line| line.contains("display notification"))
            .expect("display notification 行が存在する");
        assert!(notification_line.contains("\"line1\" & linefeed & \"line2\""));
    }

    #[test]
    fn osascript_script_groups_concatenation_in_parentheses() {
        // 改行を含む値は `applescript_string_literal` が `&` 連結式を返すため、
        // `display notification`/`with title` の各引数を括弧で括らないと
        // AppleScript のラベル付き引数構文が連結式を単一引数として解釈できず
        // コンパイルに失敗する（cursor Bugbot 指摘、PR #1008
        // #discussion_r4135838290）。
        let script = osascript_script("t1\nt2", "b1\nb2");
        assert!(script.contains("display notification (\"b1\" & linefeed & \"b2\") with title (\"t1\" & linefeed & \"t2\")"));
    }

    #[cfg(unix)]
    #[test]
    fn osascript_notifier_reports_internal_error_on_nonzero_exit() {
        // `/usr/bin/false` は常に exit 1 を返す（引数の有無に関わらず）。
        let notifier = OsascriptNotifier::with_program(std::path::PathBuf::from("/usr/bin/false"))
            .expect("/usr/bin/false は存在する");
        let request = NotificationRequest::new("title", "body").unwrap();
        let err = notifier.notify(&request).unwrap_err();
        assert!(matches!(err, DeviceError::Internal("notification send")));
    }

    /// モック `Notifier`（`PermissionDenied` 固定）が成功へ化けないことを固定する
    /// （`share.rs::AlwaysCancelledShare` と同じ「契約側の経路を固定する」目的）。
    struct AlwaysDeniedNotifier;

    impl Notifier for AlwaysDeniedNotifier {
        fn notify(&self, _request: &NotificationRequest) -> Result<NotifyOutcome, DeviceError> {
            Err(DeviceError::PermissionDenied("notification"))
        }
    }

    #[test]
    fn mock_permission_denied_does_not_become_success() {
        let request = NotificationRequest::new("title", "body").unwrap();
        let err = AlwaysDeniedNotifier.notify(&request).unwrap_err();
        assert!(matches!(err, DeviceError::PermissionDenied("notification")));
    }
}
