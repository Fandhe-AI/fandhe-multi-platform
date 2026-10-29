//! メール・SMS の作成画面を OS で開く機能の抽象（Phase 11 の拡充ループ、親 #1142、Issue #1384）。
//!
//! 範囲は「宛先・件名・本文を渡して OS の作成画面を開く」ことだけ。メールは To（複数）・CC・BCC を
//! 渡せる（Issue #33）。添付はメールに 1 件だけ（Issue #32、[`ComposeAttachment`]、iOS / Android / macOS / Windows）で、
//! SMS の添付・複数宛先は扱わない。検証の SSOT は
//! [`ComposeRequest::mail`] / [`ComposeRequest::sms`]（これ以外では要求を作れない）で、
//! Web と、添付なしの macOS / Windows が共用する URL の組み立ても [`ComposeRequest::to_url`] に置く。
//! iOS / Android と、添付付きの macOS / Windows は URL を使わず OS の作成画面（MessageUI / Intent /
//! `NSSharingService` / Simple MAPI）へ値を直接渡す。
//! 画面（`fandhe_core::screens::compose_mail_sms`、端末依存ゼロ）へは `fandhe-egui-proxy` の
//! effect ドライバが写像する。
//!
//! # 結果の種類（[`ComposeOutcome`]）
//!
//! - `Queued`: 送信待ちに入った（iOS の `MFMailComposeResultSent` / `MFMessageComposeResultSent`）。
//!   **送信済みではない**
//! - `Saved`: 下書きとして保存された（iOS のメール）
//! - `Cancelled`: 利用者が作成画面を閉じた。失敗ではない（取り消しを知れるのは iOS と Windows の添付だけ）
//! - `HandedOff`: OS が依頼を受理しただけ。開いたか・送ったかは分からない（macOS・Windows・Android）
//! - `NoHandler`: メール / SMS を扱えるアプリや設定が無い。失敗ではない
//!
//! OS ごとに到達する値: macOS は `HandedOff` / `NoHandler`、Windows は `HandedOff` / `NoHandler`
//! （添付付きは `HandedOff` / `Cancelled`）、iOS は `Queued` / `Saved` /
//! `Cancelled` / `NoHandler`、Android は `HandedOff` / `NoHandler`。
//!
//! | 状況 | 返す値 |
//! |---|---|
//! | 入力不正 | `InvalidInput("compose to" / "compose cc" / "compose bcc" / "compose subject" / "compose body")` |
//! | 入力不正（添付） | `InvalidInput("compose attachment")` |
//! | 非対応（Linux・Web ほか） | `Unsupported("compose")` |
//! | 添付付き要求を Web で受けた（URL で添付を渡せない） | `Unsupported("compose attachment")` |
//! | CC / BCC 付きの添付要求を macOS で受けた（`NSSharingService` に渡す API が無い） | `Unsupported("compose cc")` |
//! | 添付の一時ファイルを用意できない | `Internal(ATTACHMENT_FILE)`（`"compose attachment file"`） |
//! | 前回の一時ファイルを消せず、OS を呼ばなかった | `Internal(CLEANUP)`（`"compose cleanup"`） |
//! | 前の作成画面が開いたまま（Windows） | `Internal(BUSY)`（`"compose busy"`） |
//! | MAPI が失敗を返した（Windows。`NoHandler` に読み替えない） | `Internal(MAPI)`（`"compose mapi"`） |
//! | 作成画面を出せない（ウィンドウ・NSURL 生成失敗など） | `Internal("compose ...")` |
//!
//! # 宛先の配列（To / CC / BCC）
//!
//! メールの To は 1〜10 件、CC と BCC は各 0〜10 件。1 件ずつ同じ規則で検査し、1 件でも不正、または
//! 件数が範囲外なら、そのフィールドのエラー（`compose to` / `compose cc` / `compose bcc`）で拒否する
//! （黙って切り詰めない）。宛先に `,` `;` `&` `?` `\n` などの区切りは現れないので、区切りによる
//! 宛先・パラメータ・ヘッダの差し込みは起きない。SMS の宛先は従来どおり 0〜1 件。
//!
//! - Web と、添付なしの macOS / Windows: `mailto:{to,to}?cc=..&bcc=..&subject=..&body=..`。宛先は 1 件ずつ
//!   percent-encode し、リテラルの `,` で連結する（RFC 6068）。CC / BCC が空ならパラメータを出さない
//! - iOS・Android: URL を組み立てず配列で渡す。FFI / JNI の境界では各リストを `\n` で連結した
//!   1 本の文字列で渡し、受け側で配列に戻す（`\n` は検証で必ず拒否されるため宛先の中に現れない。
//!   空文字列は空配列）
//!
//! # 添付（Issue #32）
//!
//! 添付はメールに 1 件、ファイル名 `[A-Za-z0-9._-]` 1〜64 バイト（先頭 `.` 不可）・MIME は
//! `type/subtype` の 1 組（パラメータ不可）・データ 1 バイト〜1 MiB。0 バイトは嘘の成功を避けるため拒否する。
//! 出どころは画面側の固定値で、選択ファイルとの連携は扱わない。proxy は macOS だけ、添付付きの要求から
//! CC / BCC を外す（`NSSharingService` が渡せないため。iOS / Android は渡す）。`mailto:` は添付を持てないため、
//! [`ComposeRequest::to_url`] が `Unsupported("compose attachment")` を返し、添付を黙って落として
//! 開くことはしない。macOS は `NSSharingService`（`composeEmail`）へ To・件名・本文・添付だけを渡し、
//! CC / BCC 付きは `Unsupported("compose cc")`、完了は待たず `HandedOff`。Windows は Simple MAPI（`MAPISendMailW`、#1043）を
//! 作業スレッドで呼び、`MAPI_USER_ABORT` は `Cancelled`、返ったらすぐ一時ファイルを消す。CC / BCC は渡す。iOS は `addAttachmentData`（一時ファイルなし）、Android は `FileProvider` の
//! content URI（`cache/compose/` に要求ごとに残し、10 分超の古いものだけ回収する）で渡す。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内での要求と完了の順序（世代の照合）、入力不正を OS へ渡さないこと、
//! 利用者の値による URL への注入（余計なパラメータ・宛先・ヘッダ）の防止、`HandedOff` を
//! `Queued` や送信済みと見せないこと、`NoHandler` / `Unsupported` を成功に畳まないこと。
//!
//! Android の添付の一時ファイルは、書き込み失敗と `startActivity` の例外では即削除し、`HandedOff` では
//! 受け手が後から読むため残す。要求ごとのディレクトリに保存し、次回の添付要求時に 10 分より
//! 古いものだけを回収する（10 分以内の複数要求では複数件が残る）。OS の cacheDir 掃除でも消える。
//!
//! macOS の添付の一時ファイルは、プロセスごとに新しく作る `temp_dir()/fandhe-compose-<pid>-<n>/`
//! （既存のディレクトリは受け入れない）の下の `<pid>-<seq>/` に要求ごとに作る。
//! 渡した後も残し、同じプロセスの次の添付要求で 10 分（単調時計）より古いものを回収する。
//! 回収は自プロセスが作った一覧だけが対象で、置き場の走査はしない。前のプロセスが残したものは
//! 回収せず、OS の一時領域の掃除に任せる。
//! 解放に失敗したものは static に積み、次の添付要求の前に消し直す。消せなければ OS を呼ばず
//! `CLEANUP` で失敗する。今回の結果は削除の失敗と独立にそのまま返す。
//!
//! 三値（契約表 10）: 開いた（`HandedOff`）/ 利用者が閉じた（`Cancelled`、Windows のみ）/ 失敗。
//! 想定どおりの終了を失敗にしない。macOS は取り消しの通知が無いため、推測のタイマーで
//! `Cancelled` や送信済みを作らない。
//!
//! 守らない: Mail 以外が既定の macOS の挙動差、`didShareItems:` の到着、新しい Outlook や MAPI の
//! 設定画面、作成画面を開いたまま画面を離れた場合（結果は id 照合で捨てる）、複数プロセス・複数窓、
//! 複数プロセスからの同時操作、受け手が 10 分を超えて読まずに別の添付要求が来た場合の
//! 古い添付の読み出し（各 1 MiB 以下）、前のプロセスが残した macOS の添付（OS の一時領域の掃除に
//! 任せる。各 1 MiB 以下）、作成画面を開いた後に利用者が何をしたか
//! （`HandedOff` では確認できない）、既定のメールアプリ未設定時の OS 側の挙動の差。
//!
//! 既知の天井: Windows の旧 Outlook は宛先の `;` 区切りを期待する場合があり、実機で確かめる。
//! Android の一部メールアプリは `ACTION_SENDTO` の `EXTRA_CC` / `EXTRA_BCC` を無視しうる。
//! 宛先が増えるほど URL は長くなる。Windows の `mailto:` URL は 2083 文字が上限（件名 200・本文 1000 文字
//! の上限内でも、日本語は 1 文字 9 バイトに膨らむため超えうる。超えたら OS の失敗として
//! `Internal` に倒れる）。宛先が空の `sms:?body=...` は RFC 5724 の文法外で、Messages が受けるかは
//! 実機確認（#1383、Windows は #34）で確かめる。固定タイムアウトは掛けない。宛先・件名・本文・URL はログへ出さない
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
/// To / CC / BCC それぞれの上限件数。
const RECIPIENTS_MAX_PER_FIELD: usize = 10;
const ATTACH_NAME_MAX_BYTES: usize = 64;
const ATTACH_MIME_MAX_BYTES: usize = 127;
const ATTACH_DATA_MAX_BYTES: usize = 1024 * 1024;

/// 添付の一時ファイルを用意できなかった（`Internal` の固定ラベル。OS 実装が共用する SSOT）。
pub const ATTACHMENT_FILE: &str = "compose attachment file";
/// 前回の一時ファイルを消せず OS を呼ばなかった。
pub const CLEANUP: &str = "compose cleanup";
/// 前の作成画面がまだ開いている（Windows の添付付き）。
pub const BUSY: &str = "compose busy";
/// MAPI が失敗を返した（Windows の添付付き。`NoHandler` に読み替えない）。
pub const MAPI: &str = "compose mapi";

const INVALID_ATTACHMENT: DeviceError = DeviceError::InvalidInput("compose attachment");

fn valid_attach_name(n: &str) -> bool {
    (1..=ATTACH_NAME_MAX_BYTES).contains(&n.len())
        && !n.starts_with('.')
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn valid_mime_part(p: &str) -> bool {
    let mut b = p.bytes();
    matches!(b.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".+-".contains(&c))
}

fn valid_attach_mime(m: &str) -> bool {
    m.len() <= ATTACH_MIME_MAX_BYTES
        && m.split_once('/')
            .is_some_and(|(t, s)| valid_mime_part(t) && valid_mime_part(s))
}

/// 検査済みの添付 1 件。[`ComposeAttachment::new`] 以外では作れない（検証の SSOT）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeAttachment {
    file_name: String,
    mime_type: String,
    data: Vec<u8>,
}

impl ComposeAttachment {
    /// 添付を作る。規則はモジュール doc。違反は `InvalidInput("compose attachment")`。
    pub fn new(file_name: &str, mime_type: &str, data: &[u8]) -> Result<Self, DeviceError> {
        if !valid_attach_name(file_name)
            || !valid_attach_mime(mime_type)
            || data.is_empty()
            || data.len() > ATTACH_DATA_MAX_BYTES
        {
            return Err(INVALID_ATTACHMENT);
        }
        Ok(Self {
            file_name: file_name.to_string(),
            mime_type: mime_type.to_string(),
            data: data.to_vec(),
        })
    }
    /// ファイル名。
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
    /// MIME type。
    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }
    /// 中身。
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// 検査済みの作成要求。[`ComposeRequest::mail`] / [`ComposeRequest::sms`] 以外では作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeRequest {
    kind: ComposeKind,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    body: String,
    attachment: Option<ComposeAttachment>,
}

const INVALID_TO: DeviceError = DeviceError::InvalidInput("compose to");
const INVALID_CC: DeviceError = DeviceError::InvalidInput("compose cc");
const INVALID_BCC: DeviceError = DeviceError::InvalidInput("compose bcc");
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

/// 件数が `min..=RECIPIENTS_MAX_PER_FIELD` で、全件が [`valid_mail_to`] を満たす。
fn valid_recipients<S: AsRef<str>>(list: &[S], min: usize) -> bool {
    (min..=RECIPIENTS_MAX_PER_FIELD).contains(&list.len())
        && list.iter().all(|a| valid_mail_to(a.as_ref()))
}

fn owned<S: AsRef<str>>(list: &[S]) -> Vec<String> {
    list.iter().map(|s| s.as_ref().to_string()).collect()
}

/// 宛先を 1 件ずつ encode し、リテラルの `,` で連結する（RFC 6068）。
fn join_encoded(list: &[String]) -> String {
    list.iter()
        .map(|a| percent_encode(a, true))
        .collect::<Vec<_>>()
        .join(",")
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
    /// メール要求。宛先は To 1〜10 件・CC / BCC 各 0〜10 件で、1 件ごとに `@` がちょうど 1 つ、
    /// 空白・`< > " , ; ? & # %` と制御文字なし、254 バイト以下。件名は 200 文字以下・制御文字なし、本文は 1000 文字以下
    /// （改行は `\n` / `\r\n` のみ）。trim はしない。
    pub fn mail<S: AsRef<str>>(
        to: &[S],
        cc: &[S],
        bcc: &[S],
        subject: &str,
        body: &str,
    ) -> Result<Self, DeviceError> {
        if !valid_recipients(to, 1) {
            return Err(INVALID_TO);
        }
        if !valid_recipients(cc, 0) {
            return Err(INVALID_CC);
        }
        if !valid_recipients(bcc, 0) {
            return Err(INVALID_BCC);
        }
        if !valid_subject(subject) {
            return Err(INVALID_SUBJECT);
        }
        if !valid_body(body) {
            return Err(INVALID_BODY);
        }
        Ok(Self {
            kind: ComposeKind::Mail,
            to: owned(to),
            cc: owned(cc),
            bcc: owned(bcc),
            subject: subject.to_string(),
            body: body.to_string(),
            attachment: None,
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
            to: if to.is_empty() {
                Vec::new()
            } else {
                vec![to.to_string()]
            },
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: body.to_string(),
            attachment: None,
        })
    }

    /// 添付を付ける。SMS の要求には付けられない（`InvalidInput("compose attachment")`）。
    pub fn with_attachment(mut self, a: ComposeAttachment) -> Result<Self, DeviceError> {
        if self.kind != ComposeKind::Mail {
            return Err(INVALID_ATTACHMENT);
        }
        self.attachment = Some(a);
        Ok(self)
    }
    /// 添付（メールのみ）。
    pub fn attachment(&self) -> Option<&ComposeAttachment> {
        self.attachment.as_ref()
    }

    /// 作成画面の種類。
    pub fn kind(&self) -> ComposeKind {
        self.kind
    }
    /// 宛先（SMS は 0〜1 件）。
    pub fn to(&self) -> &[String] {
        &self.to
    }
    /// CC（SMS は常に空）。
    pub fn cc(&self) -> &[String] {
        &self.cc
    }
    /// BCC（SMS は常に空）。
    pub fn bcc(&self) -> &[String] {
        &self.bcc
    }
    /// 件名（SMS は空）。
    pub fn subject(&self) -> &str {
        &self.subject
    }
    /// 本文。
    pub fn body(&self) -> &str {
        &self.body
    }

    /// `mailto:`（RFC 6068）/ `sms:`（RFC 5724）の URL を組み立てる。Web と、添付なしの macOS / Windows が共用する。
    /// 本文の改行は `\r\n` に正規化してから encode する。URL は添付を持てないため、
    /// 添付付きは `Unsupported("compose attachment")`（添付を黙って落とさない）。
    pub fn to_url(&self) -> Result<String, DeviceError> {
        if self.attachment.is_some() {
            return Err(DeviceError::Unsupported("compose attachment"));
        }
        let to = join_encoded(&self.to);
        let body = percent_encode(
            &self.body.replace("\r\n", "\n").replace('\n', "\r\n"),
            false,
        );
        match self.kind {
            ComposeKind::Mail => {
                let mut params = String::new();
                for (name, list) in [("cc", &self.cc), ("bcc", &self.bcc)] {
                    if !list.is_empty() {
                        params.push_str(&format!("{name}={}&", join_encoded(list)));
                    }
                }
                Ok(format!(
                    "mailto:{to}?{params}subject={}&body={body}",
                    percent_encode(&self.subject, false)
                ))
            }
            ComposeKind::Sms => Ok(format!("sms:{to}?body={body}")),
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
                    ComposeRequest::mail(&[to], &[], &[], "s", "b"),
                    Err(DeviceError::InvalidInput("compose to"))
                ),
                "{to:?}"
            );
            assert!(
                matches!(
                    ComposeRequest::mail(&["a@b.c"], &[to], &[], "s", "b"),
                    Err(DeviceError::InvalidInput("compose cc"))
                ),
                "cc {to:?}"
            );
            assert!(
                matches!(
                    ComposeRequest::mail(&["a@b.c"], &[], &[to], "s", "b"),
                    Err(DeviceError::InvalidInput("compose bcc"))
                ),
                "bcc {to:?}"
            );
        }
        let long = format!("{}@b.c", "a".repeat(MAIL_TO_MAX_BYTES - 3));
        assert!(ComposeRequest::mail(&[long.as_str()], &[], &[], "s", "b").is_err());
        assert!(ComposeRequest::mail(&["a@b.c"], &[], &[], "s", "b").is_ok());
    }

    #[test]
    fn recipient_counts() {
        let ten = ["a@b.c"; 10];
        let eleven = ["a@b.c"; 11];
        let none: [&str; 0] = [];
        assert!(ComposeRequest::mail(&ten, &ten, &ten, "s", "b").is_ok());
        assert!(matches!(
            ComposeRequest::mail(&none, &none, &none, "s", "b"),
            Err(DeviceError::InvalidInput("compose to"))
        ));
        assert!(matches!(
            ComposeRequest::mail(&eleven, &none, &none, "s", "b"),
            Err(DeviceError::InvalidInput("compose to"))
        ));
        assert!(matches!(
            ComposeRequest::mail(&ten, &eleven, &none, "s", "b"),
            Err(DeviceError::InvalidInput("compose cc"))
        ));
        assert!(matches!(
            ComposeRequest::mail(&ten, &none, &eleven, "s", "b"),
            Err(DeviceError::InvalidInput("compose bcc"))
        ));
    }

    #[test]
    fn subject_and_body_rejections() {
        for s in ["x\r\nBcc: a@b.c", "x\ny", "x\0"] {
            assert!(matches!(
                ComposeRequest::mail(&["a@b.c"], &[], &[], s, "b"),
                Err(DeviceError::InvalidInput("compose subject"))
            ));
        }
        assert!(ComposeRequest::mail(&["a@b.c"], &[], &[], &"あ".repeat(200), "b").is_ok());
        assert!(ComposeRequest::mail(&["a@b.c"], &[], &[], &"あ".repeat(201), "b").is_err());
        for b in ["a\rb", "a\0b", "a\u{7}b"] {
            assert!(matches!(
                ComposeRequest::mail(&["a@b.c"], &[], &[], "s", b),
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

        let req = ComposeRequest::mail(&["a@b.c"], &[], &[], "S&T=1?#日本", "x\ny&z").unwrap();
        let url = req.to_url().unwrap();
        assert!(url.starts_with("mailto:a@b.c?subject="));
        assert_eq!(url.matches('&').count(), 1);
        assert_eq!(url.matches('?').count(), 1);
        assert!(url.contains("body=x%0D%0Ay%26z"));
        let multi = ComposeRequest::mail(&["a@b.c", "d@e.f"], &["g@h.i"], &["j@k.l"], "s", "b")
            .unwrap()
            .to_url()
            .unwrap();
        assert_eq!(
            multi,
            "mailto:a@b.c,d@e.f?cc=g@h.i&bcc=j@k.l&subject=s&body=b"
        );
        assert_eq!(multi.matches('?').count(), 1);
        assert_eq!(multi.matches('&').count(), 3);
        let plain = ComposeRequest::mail(&["a@b.c"], &[], &[], "s", "b").unwrap();
        assert_eq!(plain.to_url().unwrap(), "mailto:a@b.c?subject=s&body=b");
        let sms = ComposeRequest::sms("", "a&b").unwrap().to_url().unwrap();
        assert_eq!(sms, "sms:?body=a%26b");
    }

    #[test]
    fn attachment_rules() {
        let long = "a".repeat(65);
        for n in [
            "",
            ".a",
            "a/b",
            "a\\b",
            "a b",
            "a;b",
            "a\u{7}b",
            long.as_str(),
        ] {
            assert!(
                ComposeAttachment::new(n, "text/plain", b"x").is_err(),
                "{n:?}"
            );
        }
        for m in [
            "text",
            "text/plain; x=y",
            "Text/plain",
            "text/ plain",
            "/plain",
            "text/",
        ] {
            assert!(ComposeAttachment::new("a.txt", m, b"x").is_err(), "{m:?}");
        }
        assert!(ComposeAttachment::new("a.txt", "text/plain", b"").is_err());
        let big = vec![0u8; ATTACH_DATA_MAX_BYTES + 1];
        assert!(ComposeAttachment::new("a.txt", "text/plain", &big).is_err());
        assert!(
            ComposeAttachment::new("a.txt", "text/plain", &big[..ATTACH_DATA_MAX_BYTES]).is_ok()
        );
        let a = ComposeAttachment::new("a.txt", "text/plain", b"x").unwrap();
        assert!(
            ComposeRequest::sms("", "b")
                .unwrap()
                .with_attachment(a.clone())
                .is_err()
        );
        let req = ComposeRequest::mail(&["a@b.c"], &[], &[], "s", "b")
            .unwrap()
            .with_attachment(a)
            .unwrap();
        assert!(req.attachment().is_some());
        assert!(matches!(
            req.to_url(),
            Err(DeviceError::Unsupported("compose attachment"))
        ));
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
