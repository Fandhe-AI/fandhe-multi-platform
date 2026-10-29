//! NFC タグ読み取りの抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/nfc.json`、
//! 親 #157、Issue #1044）。
//!
//! 扱う範囲は「操作 1 回で、期限 [`READ_DEADLINE`] の間に最初にかざした 1 枚の NDEF を読む」
//! ことまで。書き込み・UID・生データ・連続読み取り・intent の配送は扱わない。
//! `fandhe-egui-proxy` の effect ドライバ（`device/nfc.rs`）が [`NfcReader`] をワーカースレッドから
//! 呼び、Android の OS 層が実装する。Web は `fandhe-web-dom` が同じ表示型・定数で受け持つ。
//! NDEF の解釈は OS 非依存の純関数（[`parse_ndef`]）にまとめ、Android は
//! `NdefMessage.toByteArray()` を渡すだけにする。
//!
//! # 契約
//!
//! - **結果の種類**: 成功は [`NfcReadOutcome::Read`] と [`NfcReadOutcome::EmptyTag`]、
//!   失敗は [`NfcReadFailure`]。利用者が取り消す操作は置かない。
//! - **失敗の写像表**（OS 列は後続の子 (b-android)・(c) への指示を兼ねる）:
//!
//! | 状況 | Android | Web | 返す値 |
//! | --- | --- | --- | --- |
//! | 非対応 | `NfcAdapter` が無い | `NDEFReader` が無い・非セキュア文脈 | `Unsupported` |
//! | NFC がオフ | `isEnabled()` が偽 | — | `Disabled` |
//! | 権限の拒否 | 権限が無い | `NotAllowedError` | `PermissionDenied` |
//! | タグが空 | NDEF が 0 レコード | レコード 0 件 | `EmptyTag`（成功） |
//! | 読めないタグ | NDEF でない・解析不能 | 同左 | `Unreadable` |
//! | 上限超過 | [`MAX_NDEF_BYTES`] 超 | 同左 | `TooLarge` |
//! | 期限切れ | Rust の期限 | 同左 | `TimedOut` |
//! | 読み取り中の中断 | `onPause` 等 | ページ非表示・abort | `Interrupted` |
//! | OS の失敗・未知のコード | 例外・`null` | その他の例外 | `Failed` |
//!
//! - **OS を正にする**: NFC の有無・オン / オフは要求のたびに OS へ問う。機種判定は置かない。
//! - **期限**（契約表 3）: 起点は要求を観測した時の `Instant`（単調時計）で、`read_one` へは
//!   残り時間を渡す。OS 層はリスナー・reader mode を解除し終えてから結果を返す。期限後に届いた
//!   成功は成功として受け取らない。値は仮で、確認用 issue #1040 で見直す。
//! - **三値**（契約表 10）: 成功は `Read` / `EmptyTag`、中断は `Interrupted`（失敗の色にしない）、
//!   失敗はそれ以外。
//! - **秘密値**（契約表 8）: タグの中身・UID・OS の文言をログ・`Debug` に出さない。テキストは
//!   trim しない。URL は文字列として表示するだけで、開かない。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（空・読めない・オフ・非対応を
//! 成功にしない）、リスナーの解除。守らない範囲: 複数プロセス、かざした後の消失の時間差、
//! 複数枚を同時にかざした場合にどの 1 枚が読まれるか。

use std::fmt;
use std::time::Duration;

/// 要求を観測してから結果を得るまでの期限（Rust のドライバだけが持つ）。仮の値で、#1040 で見直す。
pub const READ_DEADLINE: Duration = Duration::from_secs(30);

/// NDEF メッセージのバイト数上限。Java・Web が参照する SSOT（片側へ複製しない）。
pub const MAX_NDEF_BYTES: usize = 32 * 1024;

/// 表示用の NDEF レコード。`Debug` は種類とバイト数だけを出す（本文・URL・lang は出さない）。
#[derive(Clone, PartialEq, Eq)]
pub enum NdefRecordView {
    /// Well-known `T`（テキスト）。
    Text {
        /// 言語コード。
        lang: String,
        /// 本文（trim しない）。
        text: String,
    },
    /// Well-known `U`（接頭辞を展開済み）または絶対 URI。
    Uri(String),
    /// MIME メディア。
    Mime {
        /// MIME 型名。
        type_name: String,
        /// ペイロードのバイト数。
        len: usize,
    },
    /// 外部型。
    External {
        /// 型名。
        type_name: String,
        /// ペイロードのバイト数。
        len: usize,
    },
    /// 上記以外（Smart Poster 等）。
    Other {
        /// 型名（TNF 1 は RTD の型、TNF 5 は空）。
        type_name: String,
        /// ペイロードのバイト数。
        len: usize,
    },
}

impl fmt::Debug for NdefRecordView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text, .. } => write!(f, "Text(len={})", text.len()),
            Self::Uri(u) => write!(f, "Uri(len={})", u.len()),
            Self::Mime { len, .. } => write!(f, "Mime(len={len})"),
            Self::External { len, .. } => write!(f, "External(len={len})"),
            Self::Other { len, .. } => write!(f, "Other(len={len})"),
        }
    }
}

/// 読み取りの成功値。作る経路は [`NfcReadOutcome::from_records`] に限る（0 件の `Read` を作れない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NfcReadOutcome {
    /// 1 件以上のレコードを読めた。
    Read(Vec<NdefRecordView>),
    /// タグは空（レコード 0 件）。失敗ではない。
    EmptyTag,
}

impl NfcReadOutcome {
    /// レコード列から成功値を作る。空なら `EmptyTag`。
    pub fn from_records(records: Vec<NdefRecordView>) -> Self {
        if records.is_empty() {
            Self::EmptyTag
        } else {
            Self::Read(records)
        }
    }
}

/// 読めなかった理由（タグの中身・UID・OS の文言・コードは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfcReadFailure {
    /// 読めないタグ（NDEF でない・解析不能）。
    Unreadable,
    /// 上限超過。
    TooLarge,
    /// NFC がオフ。
    Disabled,
    /// 権限の拒否。
    PermissionDenied,
    /// 期限切れ。
    TimedOut,
    /// 読み取り中の中断。
    Interrupted,
    /// 非対応。
    Unsupported,
    /// それ以外。
    Failed,
}

/// NFC タグ読み取り。ブロッキング呼び出しで、ワーカーから呼ぶ。
pub trait NfcReader: Send + Sync {
    /// 最初にかざした 1 枚を読む。`remaining` は期限までの残り時間。
    fn read_one(&self, remaining: Duration) -> Result<NfcReadOutcome, NfcReadFailure>;
}

/// 非対応 OS 用。常に `Unsupported`（ダミーの成功へは倒さない）。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedNfcReader;

impl NfcReader for UnsupportedNfcReader {
    fn read_one(&self, _: Duration) -> Result<NfcReadOutcome, NfcReadFailure> {
        Err(NfcReadFailure::Unsupported)
    }
}

/// NFC Forum URI RTD の接頭辞表（0x00〜0x23）。
const URI_PREFIXES: [&str; 0x24] = [
    "",
    "http://www.",
    "https://www.",
    "http://",
    "https://",
    "tel:",
    "mailto:",
    "ftp://anonymous:anonymous@",
    "ftp://ftp.",
    "ftps://",
    "sftp://",
    "smb://",
    "nfs://",
    "ftp://",
    "dav://",
    "news:",
    "telnet://",
    "imap:",
    "rtsp://",
    "urn:",
    "pop:",
    "sip:",
    "sips:",
    "tftp:",
    "btspp://",
    "btl2cap://",
    "btgoep://",
    "tcpobex://",
    "irdaobex://",
    "file://",
    "urn:epc:id:",
    "urn:epc:tag:",
    "urn:epc:pat:",
    "urn:epc:raw:",
    "urn:epc:",
    "urn:nfc:",
];

/// Well-known `T` のペイロードを読む。UTF-8 / UTF-16 とも不正なら `Unreadable`（lossy にしない）。
pub fn decode_text(payload: &[u8]) -> Result<NdefRecordView, NfcReadFailure> {
    let (&status, rest) = payload.split_first().ok_or(NfcReadFailure::Unreadable)?;
    let lang_len = (status & 0x3F) as usize;
    if rest.len() < lang_len {
        return Err(NfcReadFailure::Unreadable);
    }
    let (lang, body) = rest.split_at(lang_len);
    let lang = std::str::from_utf8(lang)
        .map_err(|_| NfcReadFailure::Unreadable)?
        .to_string();
    let text = if status & 0x80 == 0 {
        std::str::from_utf8(body)
            .map_err(|_| NfcReadFailure::Unreadable)?
            .to_string()
    } else {
        if body.len() % 2 != 0 {
            return Err(NfcReadFailure::Unreadable);
        }
        // BOM があれば従い、無ければ BE。
        let (body, little) = match body {
            [0xFF, 0xFE, r @ ..] => (r, true),
            [0xFE, 0xFF, r @ ..] => (r, false),
            b => (b, false),
        };
        let units = body.as_chunks::<2>().0.iter().map(|&c| {
            if little {
                u16::from_le_bytes(c)
            } else {
                u16::from_be_bytes(c)
            }
        });
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| NfcReadFailure::Unreadable)?
    };
    Ok(NdefRecordView::Text { lang, text })
}

/// Well-known `U` のペイロードを読み、接頭辞を展開する。未知の接頭辞・空・不正な UTF-8 は `Unreadable`。
pub fn decode_uri(payload: &[u8]) -> Result<String, NfcReadFailure> {
    let (&code, rest) = payload.split_first().ok_or(NfcReadFailure::Unreadable)?;
    let prefix = URI_PREFIXES
        .get(code as usize)
        .ok_or(NfcReadFailure::Unreadable)?;
    let tail = std::str::from_utf8(rest).map_err(|_| NfcReadFailure::Unreadable)?;
    Ok(format!("{prefix}{tail}"))
}

fn utf8(b: &[u8]) -> Result<String, NfcReadFailure> {
    std::str::from_utf8(b)
        .map(str::to_string)
        .map_err(|_| NfcReadFailure::Unreadable)
}

/// NDEF メッセージのバイト列を解釈する（Android は `NdefMessage.toByteArray()` を渡す）。
///
/// 長さは解析の前に [`MAX_NDEF_BYTES`] で検査し、各レコードの長さは範囲を確かめてから読む
/// （タグの中身は信頼できない入力）。分割レコード（CF）・TNF 6/7 などは `Unreadable`。
pub fn parse_ndef(bytes: &[u8]) -> Result<NfcReadOutcome, NfcReadFailure> {
    if bytes.len() > MAX_NDEF_BYTES {
        return Err(NfcReadFailure::TooLarge);
    }
    if bytes.is_empty() {
        return Err(NfcReadFailure::Unreadable);
    }
    let mut pos = 0usize;
    let mut records = Vec::new();
    let mut first = true;
    loop {
        let take = |pos: &mut usize, n: usize| -> Result<&[u8], NfcReadFailure> {
            let end = pos.checked_add(n).ok_or(NfcReadFailure::Unreadable)?;
            let s = bytes.get(*pos..end).ok_or(NfcReadFailure::Unreadable)?;
            *pos = end;
            Ok(s)
        };
        let header = take(&mut pos, 1)?[0];
        let (mb, me, cf, sr, il, tnf) = (
            header & 0x80 != 0,
            header & 0x40 != 0,
            header & 0x20 != 0,
            header & 0x10 != 0,
            header & 0x08 != 0,
            header & 0x07,
        );
        if mb != first || cf || tnf >= 6 {
            return Err(NfcReadFailure::Unreadable);
        }
        first = false;
        let type_len = take(&mut pos, 1)?[0] as usize;
        let payload_len = if sr {
            take(&mut pos, 1)?[0] as usize
        } else {
            let b = take(&mut pos, 4)?;
            u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
        };
        let id_len = if il {
            take(&mut pos, 1)?[0] as usize
        } else {
            0
        };
        let type_name = take(&mut pos, type_len)?;
        take(&mut pos, id_len)?; // ID は読み飛ばし、外へ出さない
        let payload = take(&mut pos, payload_len)?;
        let rec = match tnf {
            0 => {
                if type_len != 0 || payload_len != 0 {
                    return Err(NfcReadFailure::Unreadable);
                }
                None
            }
            1 => Some(match type_name {
                b"T" => decode_text(payload)?,
                b"U" => NdefRecordView::Uri(decode_uri(payload)?),
                _ => NdefRecordView::Other {
                    type_name: utf8(type_name)?,
                    len: payload.len(),
                },
            }),
            2 => Some(NdefRecordView::Mime {
                type_name: utf8(type_name)?,
                len: payload.len(),
            }),
            3 => Some(NdefRecordView::Uri(utf8(type_name)?)),
            4 => Some(NdefRecordView::External {
                type_name: utf8(type_name)?,
                len: payload.len(),
            }),
            // TNF 5（Unknown）。
            _ => Some(NdefRecordView::Other {
                type_name: String::new(),
                len: payload.len(),
            }),
        };
        records.extend(rec);
        if me {
            break;
        }
    }
    if pos != bytes.len() {
        return Err(NfcReadFailure::Unreadable);
    }
    Ok(NfcReadOutcome::from_records(records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use NfcReadFailure as F;

    /// 単一レコードのメッセージ（MB|ME|SR）。
    fn short(tnf: u8, ty: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xD0 | tnf, ty.len() as u8, payload.len() as u8];
        v.extend_from_slice(ty);
        v.extend_from_slice(payload);
        v
    }

    fn text_payload(status: u8, lang: &[u8], body: &[u8]) -> Vec<u8> {
        let mut p = vec![status];
        p.extend_from_slice(lang);
        p.extend_from_slice(body);
        p
    }

    fn one(r: Result<NfcReadOutcome, F>) -> NdefRecordView {
        match r {
            Ok(NfcReadOutcome::Read(mut v)) if v.len() == 1 => v.remove(0),
            o => panic!("1 件のはず: {o:?}"),
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedNfcReader.read_one(Duration::ZERO),
            Err(F::Unsupported)
        );
    }

    #[test]
    fn text_utf8_keeps_whitespace() {
        let m = short(1, b"T", &text_payload(2, b"en", b" hi "));
        assert_eq!(
            one(parse_ndef(&m)),
            NdefRecordView::Text {
                lang: "en".into(),
                text: " hi ".into()
            }
        );
    }

    #[test]
    fn text_utf16_bom_and_default_be() {
        let le = text_payload(0x82, b"en", &[0xFF, 0xFE, b'a', 0, b'b', 0]);
        let be = text_payload(0x82, b"en", &[0, b'a', 0, b'b']);
        for p in [le, be] {
            assert_eq!(
                one(parse_ndef(&short(1, b"T", &p))),
                NdefRecordView::Text {
                    lang: "en".into(),
                    text: "ab".into()
                }
            );
        }
    }

    #[test]
    fn text_bad_inputs_are_unreadable() {
        for p in [
            text_payload(0x82, b"en", &[0, b'a', 0]),       // 奇数長
            text_payload(5, b"en", b""),                    // 言語長が範囲外
            text_payload(2, b"en", &[0xFF, 0xFE]),          // 不正な UTF-8
            text_payload(0x82, b"en", &[0xD8, 0, 0, 0x41]), // 不正なサロゲート
            vec![],
        ] {
            assert_eq!(decode_text(&p), Err(F::Unreadable));
        }
    }

    #[test]
    fn uri_prefix_expansion() {
        assert_eq!(decode_uri(&[4, b'a']), Ok("https://a".into()));
        assert_eq!(decode_uri(&[0, b'x']), Ok("x".into()));
        assert_eq!(decode_uri(&[0x24, b'x']), Err(F::Unreadable));
        assert_eq!(decode_uri(&[]), Err(F::Unreadable));
        assert_eq!(decode_uri(&[0, 0xFF]), Err(F::Unreadable));
    }

    #[test]
    fn long_form_length_and_overflow() {
        // SR=0: MB|ME|TNF2、type 1 バイト、payload 長 4 バイト。
        let mut m = vec![0xC2, 1, 0, 0, 0, 2, b'x', 9, 9];
        assert_eq!(
            one(parse_ndef(&m)),
            NdefRecordView::Mime {
                type_name: "x".into(),
                len: 2
            }
        );
        m = vec![0xC2, 1, 0xFF, 0xFF, 0xFF, 0xFF, b'x'];
        assert_eq!(parse_ndef(&m), Err(F::Unreadable));
        m = vec![0xC2, 1, 0, 0, 1, 0, b'x'];
        assert_eq!(parse_ndef(&m), Err(F::Unreadable));
    }

    #[test]
    fn structural_errors_are_unreadable() {
        let ok = short(2, b"x", b"y");
        let mut cf = ok.clone();
        cf[0] |= 0x20;
        let mut no_mb = ok.clone();
        no_mb[0] &= !0x80;
        let mut no_me = ok.clone();
        no_me[0] &= !0x40;
        let mut trailing = ok.clone();
        trailing.push(0);
        for m in [
            cf,
            no_mb,
            no_me,
            trailing,
            short(6, b"x", b"y"),
            short(7, b"x", b"y"),
            short(0, b"", b"y"),
            vec![],
        ] {
            assert_eq!(parse_ndef(&m), Err(F::Unreadable), "{m:?}");
        }
    }

    #[test]
    fn id_is_skipped_and_not_exposed() {
        // MB|ME|SR|IL|TNF2、type 1、payload 1、id 2。
        let m = vec![0xDA, 1, 1, 2, b'x', b'I', b'D', b'p'];
        let r = one(parse_ndef(&m));
        assert_eq!(
            r,
            NdefRecordView::Mime {
                type_name: "x".into(),
                len: 1
            }
        );
        assert!(!format!("{r:?}").contains("ID"));
    }

    #[test]
    fn other_kinds_carry_type_and_len() {
        assert_eq!(
            one(parse_ndef(&short(4, b"a:b", b"zz"))),
            NdefRecordView::External {
                type_name: "a:b".into(),
                len: 2
            }
        );
        assert_eq!(
            one(parse_ndef(&short(1, b"Sp", b"zz"))),
            NdefRecordView::Other {
                type_name: "Sp".into(),
                len: 2
            }
        );
        assert_eq!(
            one(parse_ndef(&short(3, b"http://a", b""))),
            NdefRecordView::Uri("http://a".into())
        );
        assert_eq!(
            one(parse_ndef(&short(5, b"", b"zz"))),
            NdefRecordView::Other {
                type_name: String::new(),
                len: 2
            }
        );
        assert_eq!(parse_ndef(&short(2, &[0xFF], b"z")), Err(F::Unreadable));
    }

    #[test]
    fn too_large_is_checked_before_parsing() {
        assert_eq!(parse_ndef(&vec![0; MAX_NDEF_BYTES + 1]), Err(F::TooLarge));
    }

    #[test]
    fn empty_record_message_is_empty_tag() {
        assert_eq!(
            parse_ndef(&short(0, b"", b"")),
            Ok(NfcReadOutcome::EmptyTag)
        );
        assert_eq!(
            NfcReadOutcome::from_records(vec![]),
            NfcReadOutcome::EmptyTag
        );
    }

    #[test]
    fn debug_hides_content() {
        let r = NdefRecordView::Text {
            lang: "ja".into(),
            text: "himitsu".into(),
        };
        let u = NdefRecordView::Uri("https://secret.example".into());
        let s = format!("{r:?}{u:?}");
        assert!(!s.contains("himitsu") && !s.contains("secret") && !s.contains("ja"));
    }
}
