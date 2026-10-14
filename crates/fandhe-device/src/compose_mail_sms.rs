//! メール・SMS の作成画面を OS で開く機能の抽象（Phase 11 の拡充ループ、親 #1142、Issue #1384）。
//!
//! 範囲は「宛先・件名・本文を渡して OS の作成画面を開く」ことだけ。添付（#1381）・CC / BCC・
//! 複数宛先（#1382）・Windows の SMS（#1380）は扱わない。検証の SSOT は
//! [`ComposeRequest::mail`] / [`ComposeRequest::sms`]（これ以外では要求を作れない）で、
//! macOS・Windows・Web が共用する URL の組み立ても [`ComposeRequest::to_url`] に置く。
//! iOS / Android は URL を使わず OS の作成画面（MessageUI / Intent）へ値を直接渡す。
//! 画面（`fandhe_core::screens::compose_mail_sms`、端末依存ゼロ）へは `fandhe-egui-proxy` の
//! effect ドライバが写像する。
//!
//! # 結果の種類（[`ComposeOutcome`]）
//!
//! - `Queued`: 送信待ちに入った（iOS の `MFMailComposeResultSent` / `MFMessageComposeResultSent`）。
//!   **送信済みではない**
//! - `Saved`: 下書きとして保存された（iOS のメール）
//! - `Cancelled`: 利用者が作成画面を閉じた。失敗ではない
//! - `HandedOff`: OS が依頼を受理しただけ。開いたか・送ったかは分からない（macOS・Windows・Android）
//! - `NoHandler`: メール / SMS を扱えるアプリや設定が無い。失敗ではない
//!
//! OS ごとに到達する値: macOS・Windows は `HandedOff` / `NoHandler`、iOS は `Queued` / `Saved` /
//! `Cancelled` / `NoHandler`、Android は `HandedOff` / `NoHandler`。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 入力不正 | `InvalidInput("compose to" / "compose subject" / "compose body")` |
//! | 非対応（Windows の SMS・Linux・Web ほか） | `Unsupported("compose")` |
//! | 作成画面を出せない（ウィンドウ・NSURL 生成失敗など） | `Internal("compose ...")` |
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（世代の照合）、入力不正を OS へ渡さないこと、
//! 利用者の値による URL への注入（余計なパラメータ・宛先・ヘッダ）の防止、`HandedOff` を
//! `Queued` や送信済みと見せないこと、`NoHandler` / `Unsupported` を成功に畳まないこと。
//!
//! 守らない: 複数プロセスからの同時操作、作成画面を開いた後に利用者が何をしたか
//! （`HandedOff` では確認できない）、既定のメールアプリ未設定時の OS 側の挙動の差。
//!
//! 既知の天井: 宛先は 1 件。Windows の `mailto:` URL は 2083 文字が上限（件名 200・本文 1000 文字
//! の上限内でも、日本語は 1 文字 9 バイトに膨らむため超えうる。超えたら OS の失敗として
//! `Internal` に倒れる）。宛先が空の `sms:?body=...` は RFC 5724 の文法外で、Messages が受けるかは
//! 実機確認（#1383）で確かめる。固定タイムアウトは掛けない。宛先・件名・本文・URL はログへ出さない
//! （security.md A09）。

use crate::DeviceError;

/// 作成画面の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeKind {
    /// メール。
    Mail,
    /// SMS。
    Sms,
}

/// 作成画面を開いた結果。`Queued` 以外を「送信待ちに入った」と見せない（モジュール doc）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeOutcome {
    /// 送信待ちに入った。送信済みではない。
    Queued,
    /// 下書きとして保存された。
    Saved,
    /// 利用者が閉じた。
    Cancelled,
    /// OS が依頼を受理しただけ。開いたか・送ったかは不明。
    HandedOff,
    /// メール / SMS を扱えるアプリや設定が無い。失敗ではない。
    NoHandler,
}

const MAIL_TO_MAX_BYTES: usize = 254;
const SUBJECT_MAX_CHARS: usize = 200;
const BODY_MAX_CHARS: usize = 1000;

/// 検査済みの作成要求。[`ComposeRequest::mail`] / [`ComposeRequest::sms`] 以外では作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeRequest {
    kind: ComposeKind,
    to: String,
    subject: String,
    body: String,
}

const INVALID_TO: DeviceError = DeviceError::InvalidInput("compose to");
const INVALID_SUBJECT: DeviceError = DeviceError::InvalidInput("compose subject");
const INVALID_BODY: DeviceError = DeviceError::InvalidInput("compose body");

fn valid_mail_to(to: &str) -> bool {
    let Some((local, domain)) = to.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && to.len() <= MAIL_TO_MAX_BYTES
        && !to
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>\",;?&#%".contains(c))
}

fn valid_sms_to(to: &str) -> bool {
    to.len() <= MAIL_TO_MAX_BYTES
        && to
            .char_indices()
            .all(|(i, c)| c.is_ascii_digit() || "-.()".contains(c) || (i == 0 && c == '+'))
}

fn valid_subject(s: &str) -> bool {
    s.chars().count() <= SUBJECT_MAX_CHARS && !s.chars().any(char::is_control)
}

/// `\n` / `\r\n` 以外の制御文字（単独の `\r` を含む）を拒否する。
fn valid_body(s: &str) -> bool {
    if s.chars().count() > BODY_MAX_CHARS {
        return false;
    }
    let normalized = s.replace("\r\n", "\n");
    !normalized.chars().any(|c| c.is_control() && c != '\n')
}

/// unreserved（`A-Z a-z 0-9 - . _ ~`）以外を UTF-8 バイト単位で `%XX`（大文字）にする。
/// `keep_at` は宛先の `@` を残すためだけに使う。
fn percent_encode(s: &str, keep_at: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (keep_at && b == b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl ComposeRequest {
    /// メール要求。宛先は 1 件（`@` がちょうど 1 つ、空白・`< > " , ; ? & # %` と制御文字なし、
    /// 254 バイト以下）、件名は 200 文字以下・制御文字なし、本文は 1000 文字以下
    /// （改行は `\n` / `\r\n` のみ）。trim はしない。
    pub fn mail(to: &str, subject: &str, body: &str) -> Result<Self, DeviceError> {
        if !valid_mail_to(to) {
            return Err(INVALID_TO);
        }
        if !valid_subject(subject) {
            return Err(INVALID_SUBJECT);
        }
        if !valid_body(body) {
            return Err(INVALID_BODY);
        }
        Ok(Self {
            kind: ComposeKind::Mail,
            to: to.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        })
    }

    /// SMS 要求。宛先は空を許し、文字は数字・先頭の `+`・`- . ( )` だけ。
    pub fn sms(to: &str, body: &str) -> Result<Self, DeviceError> {
        if !valid_sms_to(to) {
            return Err(INVALID_TO);
        }
        if !valid_body(body) {
            return Err(INVALID_BODY);
        }
        Ok(Self {
            kind: ComposeKind::Sms,
            to: to.to_string(),
            subject: String::new(),
            body: body.to_string(),
        })
    }

    /// 作成画面の種類。
    pub fn kind(&self) -> ComposeKind {
        self.kind
    }
    /// 宛先（SMS は空もありうる）。
    pub fn to(&self) -> &str {
        &self.to
    }
    /// 件名（SMS は空）。
    pub fn subject(&self) -> &str {
        &self.subject
    }
    /// 本文。
    pub fn body(&self) -> &str {
        &self.body
    }

    /// `mailto:`（RFC 6068）/ `sms:`（RFC 5724）の URL を組み立てる。macOS・Windows・Web が共用する。
    /// 本文の改行は `\r\n` に正規化してから encode する。
    pub fn to_url(&self) -> String {
        let to = percent_encode(&self.to, true);
        let body = percent_encode(
            &self.body.replace("\r\n", "\n").replace('\n', "\r\n"),
            false,
        );
        match self.kind {
            ComposeKind::Mail => format!(
                "mailto:{to}?subject={}&body={body}",
                percent_encode(&self.subject, false)
            ),
            ComposeKind::Sms => format!("sms:{to}?body={body}"),
        }
    }
}

/// メール・SMS の作成画面を開く OS 実装の抽象。呼び出し元は `fandhe-egui-proxy` の effect ドライバ。
pub trait ComposeMailSms: Send + Sync {
    /// 作成画面を開く。結果の意味と OS ごとの到達値はモジュール doc。
    fn compose(&self, req: &ComposeRequest) -> Result<ComposeOutcome, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported` を返し、成功を作らない。
pub struct UnsupportedComposeMailSms;

impl ComposeMailSms for UnsupportedComposeMailSms {
    fn compose(&self, _req: &ComposeRequest) -> Result<ComposeOutcome, DeviceError> {
        Err(DeviceError::Unsupported("compose"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_to_rejects_injection() {
        for to in [
            "",
            "a",
            "a@b.c?cc=x",
            "a@b.c&bcc=x",
            "a@b.c,d@e.f",
            "a@b.c;d@e.f",
            "a@b@c",
            "@b.c",
            "a@",
            "a b@c.d",
            "a@b.c\r\nBcc:x@y.z",
            "a%0A@b.c",
            "a@b.c#x",
            "<a@b.c>",
        ] {
            assert!(
                matches!(
                    ComposeRequest::mail(to, "s", "b"),
                    Err(DeviceError::InvalidInput("compose to"))
                ),
                "{to:?}"
            );
        }
        let long = format!("{}@b.c", "a".repeat(MAIL_TO_MAX_BYTES - 3));
        assert!(ComposeRequest::mail(&long, "s", "b").is_err());
        assert!(ComposeRequest::mail("a@b.c", "s", "b").is_ok());
    }

    #[test]
    fn subject_and_body_rejections() {
        for s in ["x\r\nBcc: a@b.c", "x\ny", "x\0"] {
            assert!(matches!(
                ComposeRequest::mail("a@b.c", s, "b"),
                Err(DeviceError::InvalidInput("compose subject"))
            ));
        }
        assert!(ComposeRequest::mail("a@b.c", &"あ".repeat(200), "b").is_ok());
        assert!(ComposeRequest::mail("a@b.c", &"あ".repeat(201), "b").is_err());
        for b in ["a\rb", "a\0b", "a\u{7}b"] {
            assert!(matches!(
                ComposeRequest::mail("a@b.c", "s", b),
                Err(DeviceError::InvalidInput("compose body"))
            ));
        }
        assert!(ComposeRequest::sms("", &"a".repeat(1001)).is_err());
        assert!(ComposeRequest::sms("", "a\r\nb\nc").is_ok());
    }

    #[test]
    fn sms_to_rules() {
        for to in [
            "", "+81 90", "090,1", "090;1", "090?x", "09&0", "0+90", "abc",
        ] {
            let r = ComposeRequest::sms(to, "b");
            assert_eq!(r.is_ok(), to.is_empty(), "{to:?}");
        }
        assert!(ComposeRequest::sms("+81-90.(12)", "b").is_ok());
    }

    /// テスト用の小さな decoder。
    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn percent_encode_round_trips_and_never_leaks_delimiters() {
        let raw = "a&b=c?d#e 日本語\n%41+";
        let enc = percent_encode(raw, false);
        assert_eq!(decode(&enc), raw);
        assert!(!enc.contains(['&', '=', '?', '#', ' ', '\n', '+']));

        let req = ComposeRequest::mail("a@b.c", "S&T=1?#日本", "x\ny&z").unwrap();
        let url = req.to_url();
        assert!(url.starts_with("mailto:a@b.c?subject="));
        assert_eq!(url.matches('&').count(), 1);
        assert_eq!(url.matches('?').count(), 1);
        assert!(url.contains("body=x%0D%0Ay%26z"));
        let sms = ComposeRequest::sms("", "a&b").unwrap().to_url();
        assert_eq!(sms, "sms:?body=a%26b");
    }

    #[test]
    fn unsupported_never_succeeds() {
        let req = ComposeRequest::sms("", "b").unwrap();
        assert!(matches!(
            UnsupportedComposeMailSms.compose(&req),
            Err(DeviceError::Unsupported("compose"))
        ));
    }
}
