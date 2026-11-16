//! macOS 向けローカル通知実装（Phase 11、親 #1003、Issue #1004）。`osascript display notification` を子プロセスで起動する
//! [`OsascriptNotifier`]。`crate::driver` の macOS 経路が使う（`UNUserNotificationCenter` を使わない理由は `crate::driver` の
//! モジュール doc 参照）。ホストの `cargo test` でも純関数（AppleScript の組み立て・エスケープ）と子プロセスの挙動を検証できるよう、
//! cfg は macOS とテストにしている（実体は std だけで OS 非依存）。
//!
//! 守る範囲: タイトル・本文を argv に出さないこと（`ps` からの露出防止）、スクリプト文字列へのインジェクションの禁止、
//! 子プロセスの確実な回収（期限・失敗の全経路）。守らない範囲: 通知が実際に表示されたかの確認（`Submitted` までで `Shown` は返さない）。

use crate::contract::{NotificationRequest, Notifier, NotifyOutcome};
use crate::error::NotificationError;

/// macOS 向け `osascript` 起動コマンドの絶対パス。PATH 探索による差し替え
/// （PATH 乗っ取り）を避けるため常に絶対パスで起動する（security.md A03/A05）。
const OSASCRIPT_PATH: &str = "/usr/bin/osascript";

/// `osascript` の子プロセスに設けるタイムアウト。この時間を超えても終了しない
/// 場合は kill + wait で回収する（`device-bridge-contracts.md` の「3. 期限・キャンセル」
/// 「9. 失敗時のリソース解放」参照）。
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `try_wait` のポーリング間隔（`fandhe_device::tts::WAIT_OUTCOME_POLL_INTERVAL` と
/// 同型の考え方。短すぎる busy loop を避けつつ、タイムアウト検知の遅延を体感で
/// 問題にならない範囲に収める）。
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// AppleScript の二重引用符文字列リテラルへ安全に埋め込めるようエスケープする
/// 純関数。バックスラッシュ（`\` → `\\`）を先に、二重引用符（`"` → `\"`）を
/// 後に置換する（順序を逆にすると二重引用符のエスケープで挿入したバックスラッシュ
/// を再度エスケープしてしまう）。AppleScript の文字列リテラルは生の改行
/// （LF/CR）を含められずコンパイルエラーになるため、改行を含む値は
/// [`applescript_string_literal`] を使う（本関数はセグメント単位のエスケープの
/// み担う）。
fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 任意の文字列を AppleScript の文字列式へ変換する純関数。生の LF/CR を
/// 含む文字列リテラルは `osascript` のコンパイルに失敗する（cursor[bot]
/// 指摘、PR #1008 #discussion_r4135556169）ため、改行位置で分割し
/// AppleScript の `linefeed`（LF）/`return`（CR）定数との連結式
/// （`"a" & linefeed & "b"`）へ組み立て直す。改行を含まない場合は単純な
/// 二重引用符リテラルを返す。
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
    pub fn new() -> Result<Self, NotificationError> {
        Self::with_program(std::path::PathBuf::from(OSASCRIPT_PATH))
    }

    /// 任意のプログラムパスで構築する（テストから存在しないパス・`/bin/sh` 等の
    /// 代替コマンドを注入するための入口。`fandhe_device::tts::CommandTts::with_program`
    /// と同型）。
    fn with_program(program: std::path::PathBuf) -> Result<Self, NotificationError> {
        if !program.is_file() {
            return Err(NotificationError::Unsupported("notification"));
        }
        Ok(Self { program })
    }
}

impl Notifier for OsascriptNotifier {
    fn notify(&self, request: &NotificationRequest) -> Result<NotifyOutcome, NotificationError> {
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
            .map_err(|_| NotificationError::Internal("notification send"))?;

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
                    return Err(NotificationError::Internal("notification send"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 期限超過。書き込みがブロックしたまま。子プロセスを kill して
                    // パイプの読み手を失わせ、`writer` スレッドのブロックを解除して
                    // から回収する（device-bridge-contracts.md「9. 失敗時のリソース
                    // 解放」。kill せずに join すると join 自体が無期限に止まりうる）。
                    let _ = child.kill();
                    let _ = writer.join();
                    let _ = child.wait();
                    return Err(NotificationError::Internal("notification timeout"));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // 送信側スレッドが panic 等で `tx.send` に届かず終了した異常系。
                    let _ = writer.join();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(NotificationError::Internal("notification send"));
                }
            }
        }

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(NotifyOutcome::Submitted)
                    } else {
                        Err(NotificationError::Internal("notification send"))
                    };
                }
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        // 期限超過。確保済みの子プロセスを回収してから失敗を返す
                        // （`device-bridge-contracts.md` の「9. 失敗時のリソース解放」）。
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(NotificationError::Internal("notification timeout"));
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
                    return Err(NotificationError::Internal("notification send"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn osascript_notifier_with_nonexistent_program_is_unsupported() {
        let err =
            OsascriptNotifier::with_program(std::path::PathBuf::from("/nonexistent/osascript"))
                .unwrap_err();
        assert!(matches!(
            err,
            NotificationError::Unsupported("notification")
        ));
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
        assert!(matches!(
            err,
            NotificationError::Internal("notification send")
        ));
    }
}
